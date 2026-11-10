//! Approvals in chat. A request gets a short code and a message in the room; only the owner's reply in that room
//! (`yes CODE`, `no CODE`, `edit CODE <new command>`) decides it, and silence for the timeout is a no.
use rand::Rng;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    Deny(String),
    /// Run this instead (only for requests that carry a command).
    Edit(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Yes(String),
    No(String),
    Edit(String, String),
}

pub const NO_ANSWER: &str = "No answer in time.";

struct Pending { gid: String, tx: Sender<Decision>, deadline: Instant, can_edit: bool }

pub struct Broker { pending: Mutex<HashMap<String, Pending>>, timeout: Duration }

/// What came of an owner's reply.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    Done(String),
    /// A code nobody is waiting on (in this room).
    Unknown(String),
    /// `edit` on something that has no command to edit.
    NotEditable(String),
}

const ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
const CODE_LEN: usize = 4;

/// Reads `yes K7QX`, `no K7QX`, `edit K7QX df -h /`. Anything else is not an approval reply.
pub fn parse_reply(text: &str) -> Option<Reply> {
    let t = text.trim();
    let mut it = t.splitn(2, char::is_whitespace);
    let verb = it.next()?.to_ascii_lowercase();
    let rest = it.next()?.trim_start();
    let code_ok = |c: &str| c.len() == CODE_LEN && c.bytes().all(|b| ALPHABET.contains(&b.to_ascii_uppercase()));
    match verb.as_str() {
        "yes" | "no" => {
            let code = rest.trim_end_matches(['.', '!']);
            if !code_ok(code) { return None; }
            let code = code.to_ascii_uppercase();
            Some(if verb == "yes" { Reply::Yes(code) } else { Reply::No(code) })
        }
        "edit" => {
            let mut p = rest.splitn(2, char::is_whitespace);
            let code = p.next()?;
            if !code_ok(code) { return None; }
            let cmd = p.next().unwrap_or("").trim();
            Some(Reply::Edit(code.to_ascii_uppercase(), cmd.to_string()))
        }
        _ => None,
    }
}

impl Broker {
    pub fn new(timeout: Duration) -> Broker { Broker { pending: Mutex::new(HashMap::new()), timeout } }
    pub fn timeout(&self) -> Duration { self.timeout }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Pending>> { self.pending.lock().unwrap_or_else(|p| p.into_inner()) }

    /// A new request. The returned code goes into the chat message; the receiver gets the decision.
    pub fn register(&self, gid: &str, can_edit: bool) -> (String, Receiver<Decision>) {
        let (tx, rx) = channel();
        let mut map = self.lock();
        let mut rng = rand::thread_rng();
        let code = loop {
            let c: String = (0..CODE_LEN).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect();
            if !map.contains_key(&c) { break c; }
        };
        map.insert(code.clone(), Pending { gid: gid.to_string(), tx, deadline: Instant::now() + self.timeout, can_edit });
        (code, rx)
    }

    pub fn count(&self, gid: &str) -> usize { self.lock().values().filter(|p| p.gid == gid).count() }

    /// The owner's reply, in room `gid`.
    pub fn resolve(&self, gid: &str, reply: &Reply) -> Resolved {
        let code = match reply { Reply::Yes(c) | Reply::No(c) | Reply::Edit(c, _) => c.clone() };
        let mut map = self.lock();
        let Some(p) = map.get(&code) else { return Resolved::Unknown(code) };
        if p.gid != gid { return Resolved::Unknown(code); }
        let decision = match reply {
            Reply::Yes(_) => Decision::Allow,
            Reply::No(_) => Decision::Deny("The owner said no.".into()),
            Reply::Edit(_, cmd) => {
                if !p.can_edit || cmd.is_empty() { return Resolved::NotEditable(code); }
                Decision::Edit(cmd.clone())
            }
        };
        let p = map.remove(&code).expect("present");
        let _ = p.tx.send(decision);
        Resolved::Done(code)
    }

    /// Requests whose time is up: each is denied; the codes come back so the room can be told.
    pub fn expire(&self, gid: &str, now: Instant) -> Vec<String> {
        let mut map = self.lock();
        let due: Vec<String> = map.iter().filter(|(_, p)| p.gid == gid && p.deadline <= now).map(|(c, _)| c.clone()).collect();
        due.into_iter().filter_map(|c| map.remove(&c).map(|p| { let _ = p.tx.send(Decision::Deny(NO_ANSWER.into())); c })).collect()
    }

    /// Withdraw one request (the asker stopped waiting).
    pub fn cancel(&self, code: &str) { self.lock().remove(code); }

    /// Deny everything waiting in a room (the task was stopped, or the room stopped being an assistant room).
    pub fn deny_all(&self, gid: &str, why: &str) -> usize {
        let mut map = self.lock();
        let codes: Vec<String> = map.iter().filter(|(_, p)| p.gid == gid).map(|(c, _)| c.clone()).collect();
        let n = codes.len();
        for c in codes { if let Some(p) = map.remove(&c) { let _ = p.tx.send(Decision::Deny(why.to_string())); } }
        n
    }
}

/// The chat message that asks for approval.
pub fn approval_message(code: &str, what: &str, detail: &str, can_edit: bool, timeout: Duration, note: &str) -> String {
    let mut s = format!("Approval needed [{code}]\n{what}\n{detail}\n");
    if !note.is_empty() { s.push_str(note); s.push('\n'); }
    s.push_str(&format!("Reply: yes {code}  /  no {code}"));
    if can_edit { s.push_str(&format!("  /  edit {code} <new command>")); }
    s.push_str(&format!("\nNo answer in {} minutes counts as no.", (timeout.as_secs() / 60).max(1)));
    s
}
