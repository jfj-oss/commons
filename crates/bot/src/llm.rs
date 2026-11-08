//! The models the bot can answer with: a local Ollama server (the default, nothing leaves the computer) and an
//! optional DeepSeek client whose text does leave it, clearly labelled whenever it is used.
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

pub const DEFAULT_OLLAMA: &str = "http://127.0.0.1:11434";
pub const DEFAULT_DEEPSEEK_URL: &str = "https://api.deepseek.com";
pub const DEFAULT_PREFERRED_MODEL: &str = "qwen3.5:9b";
pub const DEFAULT_DEEPSEEK_MODEL: &str = "deepseek-chat";

/// One turn of the conversation as the model sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMsg {
    pub role: &'static str, // "user" or "assistant"
    pub content: String,
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
    fn ask(&self, system: &str, history: &[ChatMsg]) -> Result<String, AskError>;
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
}

impl Ollama {
    pub fn new(base: &str, preferred: Option<&str>, timeout: Duration) -> Ollama {
        Ollama { base: base.trim_end_matches('/').to_string(), preferred: preferred.map(str::to_string), timeout, model: Mutex::new(None) }
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
}

impl Llm for Ollama {
    fn ask(&self, system: &str, history: &[ChatMsg]) -> Result<String, AskError> {
        let Some(model) = self.model()? else {
            return Err(AskError::Failed("no chat model is installed in Ollama (try `ollama pull qwen3.5:9b`)".into()));
        };
        let mut messages = vec![json!({"role": "system", "content": system})];
        for m in history { messages.push(json!({"role": m.role, "content": m.content})); }
        let body = json!({
            "model": model,
            "messages": messages,
            "stream": false,
            "options": {"temperature": 0.7, "num_predict": 600},
        });
        let r = agent(self.timeout).post(&format!("{}/api/chat", self.base)).set("Content-Type", "application/json").send_json(body).map_err(|e| failed("Ollama", e))?;
        let v: Value = r.into_json().map_err(|e| AskError::Failed(format!("Ollama's answer could not be read: {e}")))?;
        let text = v["message"]["content"].as_str().unwrap_or("").trim().to_string();
        if text.is_empty() { return Err(AskError::Failed("the local model returned an empty answer".into())); }
        Ok(text)
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
}

impl Llm for DeepSeek {
    fn ask(&self, system: &str, history: &[ChatMsg]) -> Result<String, AskError> {
        let key = self.key()?;
        let mut messages = vec![json!({"role": "system", "content": system})];
        for m in history { messages.push(json!({"role": m.role, "content": m.content})); }
        let body = json!({"model": self.model, "messages": messages, "stream": false, "temperature": 0.7});
        let r = agent(self.timeout)
            .post(&format!("{}/chat/completions", self.base))
            .set("Content-Type", "application/json")
            .set("Authorization", &format!("Bearer {key}"))
            .send_json(body)
            .map_err(|e| failed("DeepSeek", e))?;
        let v: Value = r.into_json().map_err(|e| AskError::Failed(format!("DeepSeek's answer could not be read: {e}")))?;
        let text = v["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_string();
        if text.is_empty() { return Err(AskError::Failed("DeepSeek returned an empty answer".into())); }
        Ok(text)
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
        match d.ask("sys", &[]) { Err(AskError::Failed(m)) => assert!(m.contains("deepseek-key")), other => panic!("{other:?}") }
    }
}
