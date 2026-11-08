//! The models the bot can answer with: a local Ollama server (the default, nothing leaves the computer) and an
//! optional DeepSeek client whose text does leave it, clearly labelled whenever it is used.
//!
//! Both speak the same small conversation type ([`Msg`]). That is the only place a tool call can come from, and the
//! only tool either is ever offered is the one in `search.rs`.
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

pub const DEFAULT_OLLAMA: &str = "http://127.0.0.1:11434";
pub const DEFAULT_DEEPSEEK_URL: &str = "https://api.deepseek.com";
pub const DEFAULT_PREFERRED_MODEL: &str = "qwen3.5:9b";
pub const DEFAULT_DEEPSEEK_MODEL: &str = "deepseek-chat";
/// How long Ollama keeps the model in VRAM after each request. Ollama unloads it five minutes after the last one by
/// itself; the bot's GPU watcher frees it sooner when a game or Studio needs the card.
pub const KEEP_ALIVE: &str = "5m";

/// A request from the model to run one tool.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// The provider's id for the call, when it gives one. Ollama does not.
    pub id: Option<String>,
    pub name: String,
    /// The arguments as JSON text (an object such as `{"query":"..."}`).
    pub arguments: String,
}

/// One turn of the conversation as a model sees it.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    User(String),
    Assistant { content: String, tool_calls: Vec<ToolCall> },
    Tool { call_id: Option<String>, name: String, content: String },
}

/// One tool the model may call. `parameters` is a JSON Schema object.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// What came back from one completion: words, tool calls, or both.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelReply {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

/// How the model is asked to use a tool. Ollama's `/api/show` says whether the chosen model has tool support; models
/// without it get a plain text protocol instead. DeepSeek always takes the function-calling form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolMode {
    FunctionCalling,
    TextProtocol,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AskError {
    /// Nothing answered at the address: Ollama is not running, or DeepSeek could not be reached.
    Unavailable(String),
    /// Something answered, but with an error or without an answer.
    Failed(String),
}
impl std::fmt::Display for AskError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self { AskError::Unavailable(s) | AskError::Failed(s) => write!(f, "{s}") }
    }
}

pub trait Llm: Send + Sync {
    fn complete(&self, system: &str, messages: &[Msg], tools: &[ToolSpec]) -> Result<ModelReply, AskError>;
    /// Whether this model can take tool definitions, or needs the text protocol.
    fn tool_mode(&self) -> ToolMode { ToolMode::FunctionCalling }
    /// Free anything this model has loaded on this computer. Safe and cheap to call often; a no-op when there is
    /// nothing local to free.
    fn unload(&self) -> Result<(), AskError> { Ok(()) }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(timeout).build()
}

/// Turn a ureq failure into the one distinction the bot cares about: "nothing is there" or "it said no".
fn failed(what: &str, e: ureq::Error) -> AskError {
    match e {
        ureq::Error::Status(code, resp) => {
            let body: String = resp.into_string().unwrap_or_default();
            let brief: String = body.chars().filter(|c| !c.is_control()).take(200).collect();
            if code >= 500 { AskError::Unavailable(format!("{what} answered HTTP {code}: {brief}")) } else { AskError::Failed(format!("{what} answered HTTP {code}: {brief}")) }
        }
        ureq::Error::Transport(t) => AskError::Unavailable(format!("{what} is unreachable: {t}")),
    }
}

fn tool_specs_json(tools: &[ToolSpec]) -> Value {
    Value::Array(tools.iter().map(|t| json!({
        "type": "function",
        "function": {"name": t.name, "description": t.description, "parameters": t.parameters},
    })).collect())
}

fn args_value(arguments: &str) -> Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| json!({}))
}

/// Installed models that are not chat models: an embedding-only model cannot answer a question.
pub fn is_chat_model(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    !["embed", "bge-", "bge:", "nomic", "all-minilm", "gte-", "e5-", "snowflake", "mxbai", "paraphrase"].iter().any(|k| n.contains(k))
}

/// The parameter count in an Ollama model tag: "9b", "1.5b", "70b". Names without one are small or unknown.
pub fn model_size_b(name: &str) -> Option<f64> {
    let tail = name.rsplit(':').next().unwrap_or(name).to_ascii_lowercase();
    let body = tail.strip_suffix('b')?;
    let start = body.rfind(|c: char| !(c.is_ascii_digit() || c == '.')).map(|i| i + 1).unwrap_or(0);
    let n = &body[start..];
    if n.is_empty() { return None; }
    n.parse().ok()
}

fn model_matches(installed: &str, wanted: &str) -> bool {
    installed == wanted || installed == format!("{wanted}:latest") || wanted == format!("{installed}:latest")
}

/// The owner's preferred model if it is installed, else `qwen3.5:9b` if it is, else the largest chat model installed.
/// Ties are broken by name so the choice is stable between restarts.
pub fn pick_model(installed: &[String], preferred: Option<&str>, fallback_preferred: &str) -> Option<String> {
    let chat: Vec<&String> = installed.iter().filter(|m| is_chat_model(m)).collect();
    if chat.is_empty() { return None; }
    if let Some(p) = preferred.map(str::trim).filter(|p| !p.is_empty()) {
        if let Some(m) = chat.iter().find(|m| model_matches(m, p)) { return Some((*m).clone()); }
    }
    if let Some(m) = chat.iter().find(|m| model_matches(m, fallback_preferred)) { return Some((*m).clone()); }
    chat.into_iter().max_by(|a, b| {
        let (sa, sb) = (model_size_b(a).unwrap_or(0.0), model_size_b(b).unwrap_or(0.0));
        sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.cmp(b))
    }).cloned()
}

pub struct Ollama {
    base: String,
    preferred: Option<String>,
    timeout: Duration,
    /// Filled in on the first successful /api/tags; the tag list can change (a model is pulled or removed) but a
    /// running bot does not need to chase that.
    model: Mutex<Option<String>>,
    /// Whether the chosen model advertises tool support, asked once from /api/show.
    supports_tools: Mutex<Option<bool>>,
}

impl Ollama {
    pub fn new(base: &str, preferred: Option<&str>, timeout: Duration) -> Ollama {
        Ollama {
            base: base.trim_end_matches('/').to_string(),
            preferred: preferred.map(str::to_string),
            timeout,
            model: Mutex::new(None),
            supports_tools: Mutex::new(None),
        }
    }
    pub fn base(&self) -> &str { &self.base }
    fn tags(&self) -> Result<Vec<String>, AskError> {
        let r = agent(self.timeout).get(&format!("{}/api/tags", self.base)).call().map_err(|e| failed("Ollama", e))?;
        let v: Value = r.into_json().map_err(|e| AskError::Failed(format!("Ollama's model list could not be read: {e}")))?;
        Ok(v["models"].as_array().map(|a| a.iter().filter_map(|m| m["name"].as_str().map(str::to_string)).collect()).unwrap_or_default())
    }
    /// The model this bot will use. Asked once; the answer is kept. `None` only if Ollama has no model at all.
    pub fn model(&self) -> Result<Option<String>, AskError> {
        if let Some(m) = self.model.lock().unwrap_or_else(|p| p.into_inner()).clone() { return Ok(Some(m)); }
        let tags = self.tags()?;
        let picked = pick_model(&tags, self.preferred.as_deref(), DEFAULT_PREFERRED_MODEL);
        if let Some(m) = &picked { *self.model.lock().unwrap_or_else(|p| p.into_inner()) = Some(m.clone()); }
        Ok(picked)
    }
    /// Ask `/api/show` whether the model lists "tools" among its capabilities. A missing list (or an old Ollama that
    /// does not answer) means the model gets the text protocol instead.
    fn fetch_supports_tools(&self, model: &str) -> bool {
        let r = agent(self.timeout)
            .post(&format!("{}/api/show", self.base))
            .set("Content-Type", "application/json")
            .send_json(json!({"model": model}));
        let Ok(r) = r else { return false };
        let Ok(v) = r.into_json::<Value>() else { return false };
        v["capabilities"].as_array().is_some_and(|a| a.iter().any(|c| c.as_str() == Some("tools")))
    }
    fn ollama_message(m: &Msg) -> Value {
        match m {
            Msg::User(c) => json!({"role": "user", "content": c}),
            Msg::Assistant { content, tool_calls } if tool_calls.is_empty() => json!({"role": "assistant", "content": content}),
            Msg::Assistant { content, tool_calls } => json!({
                "role": "assistant",
                "content": content,
                "tool_calls": tool_calls.iter().map(|t| json!({
                    "function": {"name": t.name, "arguments": args_value(&t.arguments)},
                })).collect::<Vec<_>>(),
            }),
            Msg::Tool { name, content, .. } => json!({"role": "tool", "content": content, "tool_name": name}),
        }
    }
    fn parse_reply(v: &Value) -> Result<ModelReply, AskError> {
        let message = &v["message"];
        let content = message["content"].as_str().unwrap_or("").trim().to_string();
        let tool_calls: Vec<ToolCall> = message["tool_calls"].as_array().map(|a| a.iter().filter_map(|c| {
            let name = c["function"]["name"].as_str()?.to_string();
            let arguments = match &c["function"]["arguments"] {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            Some(ToolCall { id: c["id"].as_str().map(str::to_string), name, arguments })
        }).collect()).unwrap_or_default();
        if content.is_empty() && tool_calls.is_empty() {
            return Err(AskError::Failed("the local model returned an empty answer".into()));
        }
        Ok(ModelReply { content, tool_calls })
    }
}

impl Llm for Ollama {
    fn tool_mode(&self) -> ToolMode {
        if let Some(v) = *self.supports_tools.lock().unwrap_or_else(|p| p.into_inner()) {
            return if v { ToolMode::FunctionCalling } else { ToolMode::TextProtocol };
        }
        let supported = match self.model() {
            Ok(Some(m)) => self.fetch_supports_tools(&m),
            _ => false,
        };
        *self.supports_tools.lock().unwrap_or_else(|p| p.into_inner()) = Some(supported);
        if supported { ToolMode::FunctionCalling } else { ToolMode::TextProtocol }
    }

    fn complete(&self, system: &str, history: &[Msg], tools: &[ToolSpec]) -> Result<ModelReply, AskError> {
        let Some(model) = self.model()? else {
            return Err(AskError::Failed("no chat model is installed in Ollama (try `ollama pull qwen3.5:9b`)".into()));
        };
        let mut messages = vec![json!({"role": "system", "content": system})];
        for m in history { messages.push(Self::ollama_message(m)); }
        let mut body = json!({
            "model": model,
            "messages": messages,
            "stream": false,
            "keep_alive": KEEP_ALIVE,
            // thinking models (qwen3.x) otherwise spend the whole budget reasoning and return no answer
            "think": false,
            // small uncensored models loop on the same words without a repetition penalty (every answer ran to the cap)
            "options": {"temperature": 0.85, "top_p": 0.9, "top_k": 40, "repeat_penalty": 1.18, "repeat_last_n": 256,
                        "presence_penalty": 0.5, "num_predict": 700},
        });
        let with_tools = self.tool_mode() == ToolMode::FunctionCalling && !tools.is_empty();
        if with_tools { body["tools"] = tool_specs_json(tools); }
        let send = |b: &Value| agent(self.timeout).post(&format!("{}/api/chat", self.base)).set("Content-Type", "application/json").send_json(b.clone());
        // Small models sometimes write a tool call Ollama cannot parse ("invalid tool call arguments"), which Ollama
        // answers with HTTP 500. That is the model's mistake, not Ollama being down: try once more, then answer
        // without tools rather than telling people the model is unreachable.
        let mut tries = 0;
        let r = loop {
            match send(&body) {
                Ok(r) => break r,
                Err(ureq::Error::Status(500, resp)) if with_tools => {
                    let text = resp.into_string().unwrap_or_default();
                    if !text.contains("tool call") { return Err(AskError::Unavailable(format!("Ollama answered HTTP 500: {}", text.chars().take(200).collect::<String>()))); }
                    tries += 1;
                    eprintln!("[bot] the model wrote a broken tool call (try {tries}); {}", if tries < 2 { "asking again" } else { "answering without tools" });
                    if tries >= 2 { body.as_object_mut().map(|o| o.remove("tools")); }
                    if tries >= 3 { return Err(AskError::Failed("the model kept writing broken tool calls".into())); }
                }
                Err(e) => return Err(failed("Ollama", e)),
            }
        };
        let v: Value = r.into_json().map_err(|e| AskError::Failed(format!("Ollama's answer could not be read: {e}")))?;
        Self::parse_reply(&v)
    }

    fn unload(&self) -> Result<(), AskError> {
        let Some(model) = self.model()? else { return Ok(()) };
        let timeout = self.timeout.min(Duration::from_secs(5));
        agent(timeout)
            .post(&format!("{}/api/generate", self.base))
            .set("Content-Type", "application/json")
            .send_json(json!({"model": model, "keep_alive": 0}))
            .map_err(|e| failed("Ollama", e))?;
        Ok(())
    }
}

/// DeepSeek's chat API is OpenAI-shaped. Only ever called when an admin has switched a room to it.
pub struct DeepSeek {
    base: String,
    key_file: PathBuf,
    model: String,
    timeout: Duration,
}

impl DeepSeek {
    pub fn new(base: &str, key_file: PathBuf, model: &str, timeout: Duration) -> DeepSeek {
        DeepSeek { base: base.trim_end_matches('/').to_string(), key_file, model: model.to_string(), timeout }
    }
    pub fn key_file(&self) -> &PathBuf { &self.key_file }
    fn key(&self) -> Result<String, AskError> {
        let key = std::fs::read_to_string(&self.key_file).map_err(|_| AskError::Failed(format!("no DeepSeek key at {}", self.key_file.display())))?;
        let key = key.trim().to_string();
        if key.is_empty() { return Err(AskError::Failed(format!("no DeepSeek key at {}", self.key_file.display()))); }
        Ok(key)
    }
    fn openai_message(m: &Msg) -> Value {
        match m {
            Msg::User(c) => json!({"role": "user", "content": c}),
            Msg::Assistant { content, tool_calls } if tool_calls.is_empty() => json!({"role": "assistant", "content": content}),
            Msg::Assistant { content, tool_calls } => json!({
                "role": "assistant",
                "content": content,
                "tool_calls": tool_calls.iter().enumerate().map(|(i, t)| json!({
                    "id": t.id.clone().unwrap_or_else(|| format!("call_{i}")),
                    "type": "function",
                    "function": {"name": t.name, "arguments": t.arguments},
                })).collect::<Vec<_>>(),
            }),
            Msg::Tool { call_id, content, .. } => json!({
                "role": "tool",
                "tool_call_id": call_id.clone().unwrap_or_default(),
                "content": content,
            }),
        }
    }
}

impl Llm for DeepSeek {
    fn complete(&self, system: &str, history: &[Msg], tools: &[ToolSpec]) -> Result<ModelReply, AskError> {
        let key = self.key()?;
        let mut messages = vec![json!({"role": "system", "content": system})];
        for m in history { messages.push(Self::openai_message(m)); }
        let mut body = json!({"model": self.model, "messages": messages, "stream": false, "temperature": 0.7});
        if !tools.is_empty() { body["tools"] = tool_specs_json(tools); }
        let r = agent(self.timeout)
            .post(&format!("{}/chat/completions", self.base))
            .set("Content-Type", "application/json")
            .set("Authorization", &format!("Bearer {key}"))
            .send_json(body)
            .map_err(|e| failed("DeepSeek", e))?;
        let v: Value = r.into_json().map_err(|e| AskError::Failed(format!("DeepSeek's answer could not be read: {e}")))?;
        let message = &v["choices"][0]["message"];
        let text = message["content"].as_str().unwrap_or("").trim().to_string();
        let tool_calls: Vec<ToolCall> = message["tool_calls"].as_array().map(|a| a.iter().filter_map(|c| {
            let name = c["function"]["name"].as_str()?.to_string();
            let arguments = match &c["function"]["arguments"] {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            Some(ToolCall { id: c["id"].as_str().map(str::to_string), name, arguments })
        }).collect()).unwrap_or_default();
        if text.is_empty() && tool_calls.is_empty() {
            return Err(AskError::Failed("DeepSeek returned an empty answer".into()));
        }
        Ok(ModelReply { content: text, tool_calls })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owners_model_wins_when_installed() {
        let tags = vec!["llama3.2:3b".to_string(), "qwen3.5:9b".to_string(), "nomic-embed-text:latest".to_string()];
        assert_eq!(pick_model(&tags, None, DEFAULT_PREFERRED_MODEL).as_deref(), Some("qwen3.5:9b"));
        assert_eq!(pick_model(&tags, Some("llama3.2:3b"), DEFAULT_PREFERRED_MODEL).as_deref(), Some("llama3.2:3b"));
    }

    #[test]
    fn without_the_preference_the_biggest_chat_model_is_used() {
        let tags = vec!["mistral:7b".to_string(), "llama3.1:70b".to_string(), "qwen3.5:9b".to_string(), "bge-m3:latest".to_string()];
        assert_eq!(pick_model(&tags, None, "not-installed:1b").as_deref(), Some("llama3.1:70b"));
        // embeddings never answer a question, even when they are the only large model
        let tags = vec!["tinyllama:1.1b".to_string(), "nomic-embed-text:latest".to_string()];
        assert_eq!(pick_model(&tags, None, "not-installed:1b").as_deref(), Some("tinyllama:1.1b"));
        assert_eq!(pick_model(&["nomic-embed-text:latest".to_string()], None, "x:1b"), None);
        // a model without a size still works when it is all there is
        let tags = vec!["phi:latest".to_string()];
        assert_eq!(pick_model(&tags, None, "not-installed:1b").as_deref(), Some("phi:latest"));
        assert_eq!(pick_model(&[], None, "x:1b"), None);
    }

    #[test]
    fn model_sizes_are_read_from_tags() {
        assert_eq!(model_size_b("qwen3.5:9b"), Some(9.0));
        assert_eq!(model_size_b("gemma3:1.5b"), Some(1.5));
        assert_eq!(model_size_b("llama3.1:latest"), None);
        assert_eq!(model_size_b("phi"), None);
    }

    #[test]
    fn a_missing_key_is_reported_without_network() {
        let d = DeepSeek::new("http://127.0.0.1:1", PathBuf::from("/nonexistent/deepseek-key"), "deepseek-chat", Duration::from_millis(200));
        match d.complete("sys", &[], &[]) { Err(AskError::Failed(m)) => assert!(m.contains("deepseek-key")), other => panic!("{other:?}") }
    }

    #[test]
    fn every_ollama_request_keeps_the_model_for_five_minutes() {
        assert_eq!(KEEP_ALIVE, "5m");
    }
}
