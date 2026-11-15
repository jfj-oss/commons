//! What the core's JSON answers hold, as far as this app draws it.
use serde_json::Value;

fn st(v: &Value, k: &str) -> String { v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string() }
fn un(v: &Value, k: &str) -> u64 { v.get(k).and_then(|x| x.as_u64()).unwrap_or(0) }
fn bo(v: &Value, k: &str) -> bool { v.get(k).and_then(|x| x.as_bool()).unwrap_or(false) }

#[derive(Clone, Debug, Default)]
pub struct Channel { pub gid: String, pub name: String, pub kind: String, pub joined: bool, pub unread: u64, pub status: String }

#[derive(Clone, Debug, Default)]
pub struct Community {
    pub id: String, pub name: String, pub role: String, pub members: u64, pub removed: bool, pub unread: u64,
    pub channels: Vec<Channel>,
    /// Owner-chosen colours: the community's own and per room (gid -> (main, hi)).
    pub colours: Option<(String, String)>,
    pub room_colours: Vec<(String, (String, String))>,
    /// A direct (one-to-one) conversation: its room and the other person.
    pub direct: bool, pub peer: String, pub room: String,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub me_account: String, pub me_name: String, pub relay: String, pub net: String, pub net_detail: String,
    pub communities: Vec<Community>, pub direct: Vec<Community>,
    pub typing_enabled: bool, pub read_receipts: bool, pub share_listening: bool, pub outbox: u64,
    pub problems: Vec<String>,
}

fn duo(v: &Value) -> Option<(String, String)> {
    let (m, h) = (st(v, "main"), st(v, "hi"));
    let ok = |s: &str| s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit());
    (ok(&m) && ok(&h)).then(|| (m.to_lowercase(), h.to_lowercase()))
}
fn channels(v: &Value) -> Vec<Channel> {
    v["channels"].as_array().map(|l| l.iter().map(|c| Channel { gid: st(c, "gid"), name: st(c, "name"), kind: st(c, "kind"), joined: bo(c, "joined"), unread: un(c, "unread"), status: st(c, "status") }).collect()).unwrap_or_default()
}

impl Snapshot {
    pub fn parse(v: &Value) -> Snapshot {
        let communities = v["communities"].as_array().map(|l| l.iter().map(|c| Community {
            id: st(c, "id"), name: st(c, "name"), role: st(c, "role"), members: un(c, "members"), removed: bo(c, "removed"), unread: un(c, "unread"), channels: channels(c),
            colours: duo(&c["style"]["community"]),
            room_colours: c["style"]["rooms"].as_object().map(|m| m.iter().filter_map(|(g, d)| duo(d).map(|d| (g.clone(), d))).collect()).unwrap_or_default(),
            ..Default::default()
        }).collect()).unwrap_or_default();
        let direct = v["direct"].as_array().map(|l| l.iter().map(|d| Community {
            id: st(d, "id"), name: st(d, "name"), members: un(d, "members"), channels: channels(d), direct: true, peer: st(d, "peer"), room: st(d, "room"), ..Default::default()
        }).collect()).unwrap_or_default();
        Snapshot {
            me_account: st(&v["me"], "account"), me_name: st(&v["me"], "name"), relay: st(v, "relay"), net: st(v, "net"), net_detail: st(v, "net_detail"),
            communities, direct, typing_enabled: bo(v, "typing_enabled"), read_receipts: bo(v, "read_receipts"), share_listening: bo(v, "share_listening"), outbox: un(v, "outbox"),
            problems: v["problems"].as_array().map(|l| l.iter().map(|p| st(p, "text")).collect()).unwrap_or_default(),
        }
    }
    pub fn community(&self, cid: &str) -> Option<&Community> { self.communities.iter().chain(self.direct.iter()).find(|c| c.id == cid) }
    pub fn total_unread(&self) -> u64 { self.communities.iter().filter(|c| !c.removed).map(|c| c.unread).sum() }
}

#[derive(Clone, Debug, Default)]
pub struct Reaction { pub emoji: String, pub count: u64, pub mine: bool }

#[derive(Clone, Debug, Default)]
pub struct Msg {
    pub id: String, pub sender: String, pub name: String, pub mine: bool, pub ts: u64, pub kind: String, pub text: String, pub status: String,
    pub file: Option<(String, u64)>, pub reply: Option<(String, String)>, pub edited: bool, pub deleted: bool, pub reactions: Vec<Reaction>, pub read_by: Vec<String>,
}
impl Msg {
    pub fn parse(v: &Value) -> Msg {
        Msg {
            id: st(v, "id"), sender: st(v, "sender"), name: st(v, "name"), mine: bo(v, "mine"), ts: un(v, "ts"), kind: st(v, "kind"), text: st(v, "text"), status: st(v, "status"),
            file: v.get("file").filter(|f| f.is_object()).map(|f| (st(f, "name"), un(f, "size"))),
            reply: v.get("reply").filter(|r| r.is_object()).map(|r| (st(r, "name"), st(r, "text"))),
            edited: bo(v, "edited"), deleted: bo(v, "deleted"),
            reactions: v["reactions"].as_array().map(|l| l.iter().map(|r| Reaction { emoji: st(r, "emoji"), count: un(r, "count"), mine: bo(r, "mine") }).collect()).unwrap_or_default(),
            read_by: v["read_by"].as_array().map(|l| l.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        }
    }
}

/// One room's window of messages as the core last described it, kept so the next answer can be only the difference
/// (api.rs `messages_answer`; this is app.js `mergeMessages`).
#[derive(Default)]
pub struct Window {
    pub key: String,
    pub hash: String,
    pub raw: Vec<Value>,
    pub msgs: Vec<Msg>,
    pub more: bool,
    pub typing: Vec<String>,
    pub status: String,
}

/// What changed in the list, for the virtual list: nothing, rows appended/changed in place, or everything.
pub enum Change { Same, InPlace { changed: Vec<usize>, appended: usize }, Reset }

impl Window {
    /// Apply an answer to `messages`. `Err(())` means the answer was a difference to a window this one does not hold: ask again in full.
    pub fn apply(&mut self, r: &Value) -> Result<Change, ()> {
        if r["same"].as_bool() == Some(true) { return Ok(Change::Same); }
        let typing = |r: &Value| r["typing"].as_array().map(|l| l.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        if r["delta"].as_bool() == Some(true) {
            if self.hash.is_empty() { return Err(()); }
            let gone: Vec<&str> = r["drop"].as_array().map(|l| l.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
            let old_ids: Vec<String> = self.msgs.iter().map(|m| m.id.clone()).collect();
            let mut raw: Vec<Value> = self.raw.iter().filter(|x| !gone.contains(&x["id"].as_str().unwrap_or(""))).cloned().collect();
            let mut touched: Vec<String> = vec![];
            for row in r["rows"].as_array().cloned().unwrap_or_default() {
                let id = row["id"].as_str().unwrap_or("").to_string();
                match raw.iter().position(|x| x["id"].as_str() == Some(id.as_str())) { Some(i) => raw[i] = row, None => raw.push(row) }
                touched.push(id);
            }
            if let Some(order) = r["order"].as_array() {
                raw = order.iter().filter_map(|id| raw.iter().find(|x| x["id"] == *id).cloned()).collect();
            }
            self.msgs = raw.iter().map(Msg::parse).collect();
            self.raw = raw;
            self.hash = st(r, "hash"); self.more = bo(r, "more"); self.typing = typing(r); self.status = st(r, "status");
            let prefix = self.msgs.len() >= old_ids.len() && self.msgs.iter().zip(old_ids.iter()).all(|(m, o)| &m.id == o);
            if !prefix { return Ok(Change::Reset); }
            let changed = touched.iter().filter_map(|id| old_ids.iter().position(|o| o == id)).collect();
            return Ok(Change::InPlace { changed, appended: self.msgs.len() - old_ids.len() });
        }
        self.raw = r["messages"].as_array().cloned().unwrap_or_default();
        self.msgs = self.raw.iter().map(Msg::parse).collect();
        self.hash = st(r, "hash"); self.more = bo(r, "more"); self.typing = typing(r); self.status = st(r, "status");
        Ok(Change::Reset)
    }
}
