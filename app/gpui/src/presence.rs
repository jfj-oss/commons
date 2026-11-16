//! Live presence on the cards (app/ui/mega-presence.js): who is looking at which card, as small dots in their colour.
//! Nothing is stored. It travels only on the relay's unstored live channel, never on the log fallback, and the switch
//! in settings stops all of it. The wire format is the web UI's, so both kinds of app see each other:
//! kind "pres.f", data {"v":1,"c":<community>,"r":<room or "">} or {"v":1,"c":<community>,"o":1} for leaving.
use crate::app::{Level, Mega};
use gpui_kit::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const KIND: &str = "pres.f";
const HOLD: Duration = Duration::from_millis(700);
const GAP: Duration = Duration::from_millis(520);
const BEAT: Duration = Duration::from_secs(20);
const GONE: Duration = Duration::from_secs(45);
pub const SHOW: usize = 6;

#[derive(Clone, Debug)]
pub struct Peer { pub cid: String, pub room: String, pub at: Instant, pub name: String }

#[derive(Default)]
pub struct Presence {
    pub peers: HashMap<String, Peer>,
    /// Who is in each room's call, by the relay's word (`call_presence`).
    pub calls: HashMap<String, Vec<String>>,
    since: u64,
    started: bool,
    names: HashMap<String, (Instant, HashMap<String, String>)>,
    /// The card under the pointer (community, room or ""), and since when.
    pub cand: Option<(String, String)>,
    cand_at: Option<Instant>,
    sent: Option<(String, String)>,
    last_send: Option<Instant>,
    chan_ok: bool,
    chan_at: Option<Instant>,
    busy: bool,
    calls_at: Option<Instant>,
}

fn is_id(s: &str) -> bool { (8..=128).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) }

impl Mega {
    pub fn people_on(&self, cid: &str, room: &str) -> Vec<String> {
        let mut v: Vec<(&Instant, &String)> = self.pres.peers.values().filter(|p| p.cid == cid && p.room == room && p.at.elapsed() < GONE).map(|p| (&p.at, &p.name)).collect();
        v.sort();
        v.into_iter().map(|(_, n)| n.clone()).collect()
    }
    pub fn focus_card(&mut self, card: Option<(String, String)>) {
        if self.pres.cand != card { self.pres.cand = card; self.pres.cand_at = Some(Instant::now()); }
    }

    /// Every 700 ms: read what others sent, drop who went quiet, and say where I am if that changed.
    pub fn presence_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != crate::app::Screen::Main { return; }
        let on_cards = matches!(self.level, Level::Home | Level::Server(_));
        let before = self.pres.peers.len();
        self.pres.peers.retain(|_, p| p.at.elapsed() < GONE);
        if self.pres.peers.len() != before { cx.notify(); }
        if !on_cards { self.pres.cand = None; }

        // receiving (only while a card screen is up and the window can be seen, like the page)
        if on_cards || !self.pres.started || self.wall.room_mode {
            let since = self.pres.since;
            self.rpc("live_poll", json!({"since": since}), window, cx, |this, r, window, cx| {
                let Ok(r) = r else { return };
                let head = r["head"].as_u64().unwrap_or(0);
                // start from "now": events from before this window looked are not interesting
                if !std::mem::replace(&mut this.pres.started, true) { this.pres.since = this.pres.since.max(head); return; }
                let events = r["events"].as_array().cloned().unwrap_or_default();
                for e in &events { if let Some(id) = e["id"].as_u64() { this.pres.since = this.pres.since.max(id); } this.wall_event(e); this.presence_event(e, window, cx); }
                if events.is_empty() && head > this.pres.since { this.pres.since = head; }
            });
        }
        // who is in a call, every five seconds while cards show
        if on_cards && !self.pres.calls_at.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
            self.pres.calls_at = Some(Instant::now());
            let cids: Vec<String> = self.snap.as_ref().map(|s| s.communities.iter().filter(|c| !c.removed).map(|c| c.id.clone()).collect()).unwrap_or_default();
            if !cids.is_empty() {
                self.rpc("call_presence", json!({"cids": cids}), window, cx, |this, r, _, cx| {
                    let Ok(r) = r else { return };
                    let calls: HashMap<String, Vec<String>> = r["rooms"].as_object().map(|m| m.iter().map(|(g, l)| (g.clone(), l.as_array().map(|l| l.iter().filter_map(|p| p["name"].as_str().map(String::from)).collect()).unwrap_or_default())).collect()).unwrap_or_default();
                    if calls != this.pres.calls { this.pres.calls = calls; cx.notify(); }
                });
            }
        }

        // sending
        if self.pres.busy { return; }
        let active = window.is_window_active();
        let want = if on_cards && active && self.prefs.presence { self.pres.cand.clone().filter(|_| self.pres.cand_at.is_some_and(|t| t.elapsed() >= HOLD)) } else { None };
        if !self.prefs.presence { self.pres.sent = None; return; }   // the switch: not one event
        if self.pres.last_send.is_some_and(|t| t.elapsed() < GAP) { return; }
        let sent = self.pres.sent.clone();
        let job: Option<(String, Value, Option<(String, String)>)> = match (&sent, &want) {
            (Some(s), w) if w.as_ref().map(|w| &w.0) != Some(&s.0) => Some((s.0.clone(), json!({"v": 1, "c": s.0, "o": 1}), None)),
            (s, Some(w)) if s.as_ref() != Some(w) => Some((w.0.clone(), json!({"v": 1, "c": w.0, "r": w.1}), Some(w.clone()))),
            (Some(_), Some(w)) if self.pres.last_send.is_some_and(|t| t.elapsed() >= BEAT) => Some((w.0.clone(), json!({"v": 1, "c": w.0, "r": w.1}), Some(w.clone()))),
            _ => None,
        };
        let Some((cid, body, after)) = job else { return };
        // the rooms of the community that can carry it, in the same order on every device
        let mut carriers: Vec<String> = self.snap.as_ref().and_then(|s| s.community(&cid)).map(|c| c.channels.iter().filter(|x| x.kind != "voice" && x.joined).map(|x| x.gid.clone()).collect()).unwrap_or_default();
        carriers.sort(); carriers.truncate(3);
        if carriers.is_empty() { return; }
        self.pres.busy = true;
        let core = self.core.clone();
        let check_chan = !self.pres.chan_at.is_some_and(|t| t.elapsed() < Duration::from_secs(5));
        let chan_was = self.pres.chan_ok;
        cx.spawn_in(window, async move |this, cx| {
            let (chan, ok) = cx.background_spawn(async move {
                // only when live events really travel on the unstored channel: on the log fallback each would be a permanent entry
                let chan = if check_chan { core.call("live_chan", json!({})).map(|r| r["chan"].as_bool() == Some(true)).unwrap_or(false) } else { chan_was };
                if !chan { return (chan, false); }
                let data = body.to_string();
                (chan, carriers.iter().any(|g| core.call("live_send", json!({"gid": g, "kind": KIND, "data": data})).map(|r| r["sent"].as_bool() == Some(true)).unwrap_or(false)))
            }).await;
            let _ = this.update(cx, |this, _| {
                this.pres.busy = false;
                if check_chan { this.pres.chan_ok = chan; this.pres.chan_at = Some(Instant::now()); }
                if ok { this.pres.last_send = Some(Instant::now()); this.pres.sent = after; }
            });
        }).detach();
    }

    /// One event from somebody else. Every field is untrusted: the community must be one we are in, the room one of its
    /// rooms, and the sender a member of it as far as this device knows.
    fn presence_event(&mut self, e: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(kind), Some(data), Some(account), Some(gid)) = (e["kind"].as_str(), e["data"].as_str(), e["account"].as_str(), e["gid"].as_str()) else { return };
        let Some(snap) = &self.snap else { return };
        if kind != KIND || data.len() > 300 || account.is_empty() || account == snap.me_account { return; }
        let Some(c) = snap.communities.iter().find(|c| !c.removed && c.channels.iter().any(|x| x.gid == gid)) else { return };
        let Ok(d) = serde_json::from_str::<Value>(data) else { return };
        if d["v"].as_u64() != Some(1) || d["c"].as_str() != Some(c.id.as_str()) { return; }
        let cid = c.id.clone();
        if d["o"].as_u64() == Some(1) {
            if self.pres.peers.get(account).is_some_and(|p| p.cid == cid) { self.pres.peers.remove(account); cx.notify(); }
            return;
        }
        let room = d["r"].as_str().unwrap_or("").to_string();
        if !(room.is_empty() || (is_id(&room) && c.channels.iter().any(|x| x.gid == room))) { return; }
        let account = account.to_string();
        // the names of a community's members, fetched at most once a minute (and not twice in 8 s for an unknown account)
        let cached = self.pres.names.get(&cid).filter(|(at, m)| at.elapsed() < Duration::from_secs(60) && (m.contains_key(&account) || at.elapsed() < Duration::from_secs(8)));
        if let Some((_, m)) = cached {
            if let Some(name) = m.get(&account).cloned() { self.pres.peers.insert(account, Peer { cid, room, at: Instant::now(), name }); cx.notify(); }
            return;
        }
        self.rpc("members", json!({"cid": cid.clone()}), window, cx, move |this, r, _, cx| {
            let m: HashMap<String, String> = r.ok().and_then(|v| v.as_array().map(|l| l.iter().filter_map(|x| Some((x["account"].as_str()?.to_string(), x["name"].as_str()?.chars().take(40).collect()))).collect())).unwrap_or_default();
            if let Some(name) = m.get(&account).cloned() { this.pres.peers.insert(account, Peer { cid: cid.clone(), room, at: Instant::now(), name }); cx.notify(); }
            this.pres.names.insert(cid, (Instant::now(), m));
        });
    }
}
