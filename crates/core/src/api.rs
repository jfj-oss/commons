//! JSON dispatch used by both the Tauri shell and the local dev bridge: the UI talks to the Rust core only
//! through `App::call(method, args)`. Keys and plaintext never leave this process.
use crate::client::*;
use crate::state::*;
use crate::transport::HttpTransport;
use crate::vault::KdfParams;
use commons_proto::{b64, unb64};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub struct App {
    dir: PathBuf,
    default_relay: String,
    kdf: KdfParams,
    client: Mutex<Option<Client>>,
    /// Pairing in progress (new device waiting for approval).
    wake: (Mutex<bool>, Condvar),
    stop: Mutex<bool>,
}

fn s(v: &Value, k: &str) -> String { v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string() }
type R = Result<Value, String>;
fn e<T: std::fmt::Display>(x: T) -> String { format!("{x}") }

impl App {
    pub fn new(dir: PathBuf, default_relay: &str) -> Arc<App> {
        Arc::new(App { dir, default_relay: default_relay.into(), kdf: KdfParams::STRONG, client: Mutex::new(None), wake: (Mutex::new(false), Condvar::new()), stop: Mutex::new(false) })
    }
    pub fn with_kdf(dir: PathBuf, default_relay: &str, kdf: KdfParams) -> Arc<App> {
        Arc::new(App { dir, default_relay: default_relay.into(), kdf, client: Mutex::new(None), wake: (Mutex::new(false), Condvar::new()), stop: Mutex::new(false) })
    }
    pub fn shutdown(&self) { *self.stop.lock().unwrap() = true; self.nudge(); }
    fn nudge(&self) { *self.wake.0.lock().unwrap() = true; self.wake.1.notify_all(); }

    /// Background sync loop: every second (or immediately after a user action).
    pub fn start_background(self: &Arc<Self>) {
        let app = self.clone();
        std::thread::spawn(move || loop {
            {
                let g = app.wake.0.lock().unwrap();
                let _ = app.wake.1.wait_timeout_while(g, Duration::from_millis(1500), |w| !*w).unwrap();
                *app.wake.0.lock().unwrap() = false;
            }
            if *app.stop.lock().unwrap() { break; }
            let mut g = app.client.lock().unwrap();
            if let Some(c) = g.as_mut() { let _ = c.tick(0); }
        });
    }

    pub fn call(&self, method: &str, a: Value) -> R {
        let r = self.dispatch(method, a);
        if !matches!(method, "snapshot" | "messages" | "state") { self.nudge(); }
        r
    }

    fn with<T>(&self, f: impl FnOnce(&mut Client) -> Result<T, String>) -> Result<T, String> {
        let mut g = self.client.lock().unwrap();
        match g.as_mut() { Some(c) => f(c), None => Err("locked".into()) }
    }

    fn open_with(&self, f: impl FnOnce(Arc<dyn crate::transport::Transport>) -> anyhow::Result<Client>, relay: &str) -> R {
        let c = f(HttpTransport::new(relay).into_arc()).map_err(|x| format!("{x:#}"))?;
        *self.client.lock().unwrap() = Some(c);
        Ok(json!({"ok": true}))
    }

    fn dispatch(&self, m: &str, a: Value) -> R {
        match m {
            "state" => {
                let g = self.client.lock().unwrap();
                match g.as_ref() {
                    None => Ok(json!({"locked": true, "exists": Client::exists(&self.dir), "default_relay": self.default_relay})),
                    Some(c) => Ok(json!({"locked": false, "has_identity": c.has_identity(), "relay": c.st.relay})),
                }
            }
            "create_account" => {
                let relay = if s(&a, "relay").is_empty() { self.default_relay.clone() } else { s(&a, "relay") };
                let (pass, name) = (s(&a, "passphrase"), s(&a, "name"));
                if pass.len() < 8 { return Err("Passphrase must be at least 8 characters.".into()); }
                if name.trim().is_empty() { return Err("Please choose a display name.".into()); }
                self.open_with(|t| Client::create(&self.dir, &pass, &relay, name.trim(), self.kdf, t), &relay)
            }
            "recover_account" => {
                let relay = if s(&a, "relay").is_empty() { self.default_relay.clone() } else { s(&a, "relay") };
                let (pass, name, key) = (s(&a, "passphrase"), s(&a, "name"), s(&a, "recovery_key"));
                if pass.len() < 8 { return Err("Passphrase must be at least 8 characters.".into()); }
                self.open_with(|t| Client::create_from_recovery(&self.dir, &pass, &relay, name.trim(), &key, self.kdf, t), &relay)
            }
            "start_pairing" => {
                let relay = if s(&a, "relay").is_empty() { self.default_relay.clone() } else { s(&a, "relay") };
                let pass = s(&a, "passphrase");
                if pass.len() < 8 { return Err("Passphrase must be at least 8 characters.".into()); }
                let mut out = String::new();
                let mut chk = String::new();
                self.open_with(|t| { let (c, code) = Client::create_for_pairing(&self.dir, &pass, &relay, "", self.kdf, t)?; out = code; chk = c.pairing_check_new().unwrap_or_default(); Ok(c) }, &relay)?;
                Ok(json!({"code": out, "qr": Client::qr_svg(&out), "check": chk}))
            }
            "pairing_status" => self.with(|c| { let done = c.pairing_poll().map_err(e)?; Ok(json!({"done": done})) }),
            "unlock" => {
                let pass = s(&a, "passphrase");
                let c = Client::open_with(&self.dir, &pass, |relay| HttpTransport::new(relay).into_arc())
                    .map_err(|x| if format!("{x}").contains("wrong passphrase") { "Wrong passphrase.".to_string() } else { format!("{x:#}") })?;
                *self.client.lock().unwrap() = Some(c);
                Ok(json!({"ok": true}))
            }
            "lock" => { *self.client.lock().unwrap() = None; Ok(json!({"ok": true})) }
            "snapshot" => self.with(|c| Ok(snapshot(c))),
            "messages" => self.with(|c| Ok(messages(c, &s(&a, "gid")))),
            "mark_read" => self.with(|c| { c.mark_read(&s(&a, "gid")); c.save().map_err(e)?; Ok(json!({})) }),
            "send_text" => self.with(|c| { let r = a.get("reply").and_then(|x| x.as_str()).map(|x| x.to_string()); c.send_text(&s(&a, "gid"), &s(&a, "text"), r).map(|id| json!({"id": id})).map_err(e) }),
            "edit" => self.with(|c| c.edit_message(&s(&a, "gid"), &s(&a, "id"), &s(&a, "text")).map(|_| json!({})).map_err(e)),
            "delete" => self.with(|c| c.delete_message(&s(&a, "gid"), &s(&a, "id")).map(|_| json!({})).map_err(e)),
            "react" => self.with(|c| c.react(&s(&a, "gid"), &s(&a, "id"), &s(&a, "emoji"), a["on"].as_bool().unwrap_or(true)).map(|_| json!({})).map_err(e)),
            "typing" => self.with(|c| { c.send_typing(&s(&a, "gid")); Ok(json!({})) }),
            "send_file" => self.with(|c| {
                let data = unb64(&s(&a, "data")).map_err(e)?;
                c.send_file(&s(&a, "gid"), &s(&a, "name"), &s(&a, "mime"), &data, &s(&a, "caption")).map(|id| json!({"id": id})).map_err(e)
            }),
            "fetch_file" => self.with(|c| c.fetch_file(&s(&a, "gid"), &s(&a, "id")).map(|(n, m, d)| json!({"name": n, "mime": m, "data": b64(&d)})).map_err(e)),
            "create_community" => self.with(|c| c.create_community(&s(&a, "name")).map(|id| json!({"id": id})).map_err(e)),
            "create_channel" => self.with(|c| c.create_channel(&s(&a, "cid"), &s(&a, "name")).map(|id| json!({"id": id})).map_err(e)),
            "create_invite" => self.with(|c| {
                let uses = a.get("uses").and_then(|x| x.as_u64()).map(|x| x as u32);
                let ttl = a.get("ttl").and_then(|x| x.as_u64()).unwrap_or(7 * 86400);
                let link = c.create_invite(&s(&a, "cid"), uses, ttl).map_err(e)?;
                Ok(json!({"link": link, "qr": Client::qr_svg(&link)}))
            }),
            "join" => self.with(|c| c.join_with_link(&s(&a, "link")).map(|id| json!({"id": id})).map_err(e)),
            "kick" => self.with(|c| c.kick(&s(&a, "cid"), &s(&a, "account")).map(|_| json!({})).map_err(e)),
            "set_role" => self.with(|c| c.set_role(&s(&a, "cid"), &s(&a, "account"), if s(&a, "role") == "admin" { Role::Admin } else { Role::Member }).map(|_| json!({})).map_err(e)),
            "leave" => self.with(|c| c.leave(&s(&a, "cid")).map(|_| json!({})).map_err(e)),
            "rename_community" => self.with(|c| c.rename_community(&s(&a, "cid"), &s(&a, "name")).map(|_| json!({})).map_err(e)),
            "revoke_invite" => self.with(|c| c.revoke_invite(&s(&a, "cid"), &s(&a, "id")).map(|_| json!({})).map_err(e)),
            "members" => self.with(|c| Ok(json!(c.members(&s(&a, "cid")).into_iter().map(|m| json!({"account": m.account, "name": m.name, "role": m.role.name(), "devices": m.devices, "verified": m.verified, "me": m.is_me})).collect::<Vec<_>>()))),
            "safety" => self.with(|c| {
                let acct = s(&a, "account");
                let num = safety_number(&c.my_account(), &acct);
                let payload = format!("commons-verify:{}", num.replace(' ', ""));
                Ok(json!({"number": num, "qr": Client::qr_svg(&payload), "verified": c.st.verified.contains(&acct), "name": c.display_name(&acct)}))
            }),
            "set_verified" => self.with(|c| c.set_verified(&s(&a, "account"), a["on"].as_bool().unwrap_or(true)).map(|_| json!({})).map_err(e)),
            "devices" => self.with(|c| {
                let me = c.my_device();
                let list = c.my_devices().map_err(e)?;
                Ok(json!(list.into_iter().map(|(d, rev)| json!({"device": d.device, "created": d.created, "revoked": rev, "this": d.device == me})).collect::<Vec<_>>()))
            }),
            "revoke_device" => self.with(|c| c.revoke_device(&s(&a, "device")).map(|_| json!({})).map_err(e)),
            "approve_pairing" => self.with(|c| c.pairing_approve(&s(&a, "code")).map(|chk| json!({"check": chk})).map_err(e)),
            "recovery_key" => self.with(|c| c.recovery_key().map(|k| json!({"key": k})).map_err(e)),
            "search" => self.with(|c| {
                let r = c.search(&s(&a, "q"));
                Ok(json!(r.into_iter().map(|(cid, gid, m)| {
                    let cname = c.st.communities.get(&cid).map(|x| x.name.clone()).unwrap_or_default();
                    let chname = c.st.communities.get(&cid).and_then(|x| x.channels.iter().find(|ch| ch.id == gid)).map(|ch| ch.name.clone()).unwrap_or_default();
                    let text = match &m.body { MsgBody::Text(t) => t.clone(), MsgBody::File { name, .. } => format!("[file] {name}"), MsgBody::Notice(t) => t.clone() };
                    json!({"cid": cid, "gid": gid, "id": m.id, "community": cname, "channel": chname, "from": c.display_name(&m.sender), "ts": m.ts, "text": text})
                }).collect::<Vec<_>>()))
            }),
            "set_name" => self.with(|c| c.set_name(&s(&a, "name")).map(|_| json!({})).map_err(e)),
            "set_typing_enabled" => self.with(|c| { c.st.typing_enabled = a["on"].as_bool().unwrap_or(false); c.save().map_err(e)?; Ok(json!({})) }),
            "notices" => self.with(|c| Ok(json!(c.st.notices.iter().rev().take(80).map(|n| json!({"ts": n.ts, "kind": n.kind, "text": n.text})).collect::<Vec<_>>()))),
            _ => Err(format!("unknown method {m}")),
        }
    }
}

fn snapshot(c: &Client) -> Value {
    let me = c.my_account();
    let comms: Vec<Value> = c.st.communities.values().map(|co| {
        let role = co.role_of(&me).unwrap_or(Role::Member);
        let chans: Vec<Value> = c.channels_of(&co.id).into_iter().map(|ch| json!({"gid": ch.gid, "name": ch.name, "joined": ch.joined, "unread": ch.unread, "status": c.group_status(&ch.gid)})).collect();
        let unread: usize = c.channels_of(&co.id).iter().map(|x| x.unread).sum();
        json!({"id": co.id, "name": if co.name.is_empty() { "…".to_string() } else { co.name.clone() }, "role": role.name(), "removed": co.removed, "removed_reason": co.removed_reason, "channels": chans, "unread": unread,
               "invites": co.invites.iter().map(|i| json!({"id": i.id, "expires": i.expires, "uses_left": i.uses_left, "mine": co.invite_sks.contains_key(&i.id)})).collect::<Vec<_>>(),
               "status": c.group_status(&co.id)})
    }).collect();
    let joining: Vec<Value> = c.st.joining.values().map(|l| json!({"id": l.community, "name": l.name, "sent": c.st.join_sent.contains(&l.community)})).collect();
    let problems: Vec<Value> = c.st.notices.iter().rev().filter(|n| matches!(n.kind.as_str(), "decrypt" | "broken" | "rollback" | "reject" | "gap" | "welcome" | "relay")).take(5).map(|n| json!({"ts": n.ts, "kind": n.kind, "text": n.text})).collect();
    json!({"me": {"account": me, "name": c.st.name, "device": c.my_device()}, "relay": c.st.relay, "communities": comms, "joining": joining,
           "typing_enabled": c.st.typing_enabled, "outbox": c.st.outbox.len(), "problems": problems,
           "version": c.activity})
}

fn messages(c: &Client, gid: &str) -> Value {
    let me = c.my_account();
    let all = c.messages(gid);
    let find = |id: &str| all.iter().find(|m| m.id == id);
    let out: Vec<Value> = all.iter().map(|m| {
        let (kind, text, file) = match &m.body {
            MsgBody::Text(t) => ("text", t.clone(), Value::Null),
            MsgBody::File { name, mime, size, caption, .. } => ("file", caption.clone(), json!({"name": name, "mime": mime, "size": size})),
            MsgBody::Notice(t) => ("notice", t.clone(), Value::Null),
        };
        let reply = m.reply.as_deref().and_then(find).map(|r| json!({"id": r.id, "name": c.display_name(&r.sender), "text": match &r.body { MsgBody::Text(t) if !r.deleted => t.chars().take(120).collect::<String>(), MsgBody::File { name, .. } => format!("[file] {name}"), _ => "(deleted)".into() }}));
        json!({"id": m.id, "sender": m.sender, "name": if m.sender.is_empty() { String::new() } else { c.display_name(&m.sender) }, "mine": m.sender == me, "ts": m.ts, "seq": m.seq,
               "status": m.status, "kind": kind, "text": text, "file": file, "reply": reply, "edited": m.edited, "deleted": m.deleted,
               "reactions": m.reactions.iter().map(|(e, who)| json!({"emoji": e, "count": who.len(), "mine": who.contains(&me), "names": who.iter().map(|w| c.display_name(w)).collect::<Vec<_>>()})).collect::<Vec<_>>()})
    }).collect();
    json!({"messages": out, "typing": c.typing_now(gid), "status": c.group_status(gid)})
}
