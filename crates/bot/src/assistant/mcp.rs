//! The permission-prompt tool for Claude Code: a tiny stdio MCP server (this binary, `megatron-bot assist-mcp`) that
//! Claude Code starts itself. Each request is passed to the bot over a private unix socket; the bot asks the owner in
//! the assistant room and sends back the owner's decision. The socket only ever carries a *request* in and a *decision
//! made in chat* out, so a process that talks to it cannot approve anything for itself.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

pub const SERVER_NAME: &str = "approver";
pub const TOOL_NAME: &str = "ask";
/// What `--permission-prompt-tool` is given.
pub fn prompt_tool_id() -> String { format!("mcp__{SERVER_NAME}__{TOOL_NAME}") }

/// A request as it reaches the bot from the MCP child.
pub struct ApprovalReq {
    pub tool_name: String,
    pub input: Value,
    pub tool_use_id: String,
    pub reply: Sender<Value>,
}

const MAX_LINE: usize = 2_000_000;

fn read_line_capped(r: &mut impl BufRead) -> Option<String> {
    let mut buf = Vec::new();
    loop {
        let (done, used) = {
            let chunk = r.fill_buf().ok()?;
            if chunk.is_empty() { return if buf.is_empty() { None } else { String::from_utf8(buf).ok() }; }
            match chunk.iter().position(|b| *b == b'\n') {
                Some(i) => { buf.extend_from_slice(&chunk[..i]); (true, i + 1) }
                None => { buf.extend_from_slice(chunk); (false, chunk.len()) }
            }
        };
        r.consume(used);
        if buf.len() > MAX_LINE { return None; }
        if done { return String::from_utf8(buf).ok(); }
    }
}

// --------------------------------------------------------------------- the bot's side of the socket

pub struct ApprovalSocket { pub path: PathBuf }

impl ApprovalSocket {
    /// Listen on `path` (made owner-only). Every connection must present `token`. Requests arrive on `tx`.
    pub fn bind(path: &Path, token: &str, tx: Sender<ApprovalReq>, wait: Duration) -> std::io::Result<ApprovalSocket> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        listener.set_nonblocking(true)?;
        let token = token.to_string();
        let p = path.to_path_buf();
        std::thread::spawn(move || {
            // ends when the socket file is removed (the task is over)
            while p.exists() {
                match listener.accept() {
                    Ok((s, _)) => {
                        let (tx, token) = (tx.clone(), token.clone());
                        std::thread::spawn(move || serve_one(s, &token, tx, wait));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                    Err(_) => break,
                }
            }
        });
        Ok(ApprovalSocket { path: path.to_path_buf() })
    }
    pub fn close(&self) { let _ = std::fs::remove_file(&self.path); }
}

fn deny_json(msg: &str) -> Value { json!({"behavior": "deny", "message": msg}) }

fn serve_one(s: UnixStream, token: &str, tx: Sender<ApprovalReq>, wait: Duration) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok(mut w) = s.try_clone() else { return };
    let mut r = BufReader::new(s);
    let Some(line) = read_line_capped(&mut r) else { return };
    let v: Value = serde_json::from_str(&line).unwrap_or_default();
    let answer = if v["token"].as_str() != Some(token) {
        deny_json("Not authorised.")
    } else {
        let (rtx, rrx) = channel();
        let req = ApprovalReq {
            tool_name: v["tool_name"].as_str().unwrap_or("unknown").to_string(),
            input: v["input"].clone(),
            tool_use_id: v["tool_use_id"].as_str().unwrap_or("").to_string(),
            reply: rtx,
        };
        if tx.send(req).is_err() { deny_json("The assistant task is over.") }
        else { rrx.recv_timeout(wait + Duration::from_secs(30)).unwrap_or_else(|_| deny_json("No answer in time.")) }
    };
    let _ = w.write_all(format!("{answer}\n").as_bytes());
}

// --------------------------------------------------------------------- the MCP child

fn ask_bot(socket: &str, token: &str, args: &Value) -> Value {
    let Ok(mut s) = UnixStream::connect(socket) else { return deny_json("The assistant is not reachable; nothing is approved.") };
    let _ = s.set_read_timeout(Some(Duration::from_secs(60 * 20)));
    let req = json!({"token": token, "tool_name": args["tool_name"], "input": args["input"], "tool_use_id": args["tool_use_id"]});
    if s.write_all(format!("{req}\n").as_bytes()).is_err() { return deny_json("The assistant is not reachable; nothing is approved."); }
    let mut r = BufReader::new(s);
    match read_line_capped(&mut r).and_then(|l| serde_json::from_str::<Value>(&l).ok()) {
        Some(v) if v["behavior"] == "allow" && v["updatedInput"].is_object() => v,
        Some(v) if v["behavior"] == "deny" => v,
        _ => deny_json("No valid answer; denied."),
    }
}

fn respond(out: &mut impl Write, id: &Value, result: Value) {
    let _ = writeln!(out, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result}));
    let _ = out.flush();
}

/// The MCP server loop on stdin/stdout. Returns when stdin closes.
pub fn run_stdio(socket: &str, token: &str) -> i32 {
    let stdin = std::io::stdin();
    let mut r = BufReader::new(stdin.lock());
    let mut out = std::io::stdout().lock();
    while let Some(line) = read_line_capped(&mut r) {
        let Ok(msg): Result<Value, _> = serde_json::from_str(&line) else { continue };
        let id = msg["id"].clone();
        let method = msg["method"].as_str().unwrap_or("");
        if id.is_null() { continue; } // notifications need no answer
        match method {
            "initialize" => respond(&mut out, &id, json!({
                "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": SERVER_NAME, "version": "1"},
            })),
            "ping" => respond(&mut out, &id, json!({})),
            "tools/list" => respond(&mut out, &id, json!({"tools": [{
                "name": TOOL_NAME,
                "description": "Ask the owner, in the chat room, whether Claude Code may use a tool. Used only as the permission prompt tool.",
                "inputSchema": {"type": "object", "properties": {
                    "tool_name": {"type": "string"}, "input": {"type": "object"}, "tool_use_id": {"type": "string"}
                }, "required": ["tool_name", "input"]},
            }]})),
            "tools/call" => {
                let decision = if msg["params"]["name"] == TOOL_NAME { ask_bot(socket, token, &msg["params"]["arguments"]) } else { deny_json("Unknown tool.") };
                respond(&mut out, &id, json!({"content": [{"type": "text", "text": decision.to_string()}]}));
            }
            _ => {
                let _ = writeln!(out, "{}", json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}}));
                let _ = out.flush();
            }
        }
    }
    0
}
