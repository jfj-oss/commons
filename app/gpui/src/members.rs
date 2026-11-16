//! People and admin: a community's members with their roles and presence, one person's card, the owner's and admins' settings
//! (name, rooms, colours, wall lock, bans, invites, handing over), making invites (a password or a link), joining with one,
//! and the safety number. Web reference: app.js (people drawer, `personAction`, invite and join dialogs), mega-admin.js, mega-invite.js.
//! Everything is the core's own call (`members`, `set_role`, `kick`, `unban`, `rename_community`, `create_channel`, `rename_channel`,
//! `delete_channel`, `set_style`, `set_wall`, `create_invite`, `revoke_invite`, `join`, `join_password`, `cancel_join`, `leave`,
//! `transfer_ownership`, `safety`, `set_verified`); nothing here decides what a role may do, the core refuses what it does not allow,
//! the buttons only follow the same rules so a person is not offered what will fail (mega-admin.js `canRemove`, `canRole`).
use crate::app::{Level, Mega};
use crate::look::{self, Duo};
use crate::model::Community;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Member { pub account: String, pub name: String, pub role: String, pub devices: u64, pub verified: bool, pub me: bool, pub tag: String, pub same_name: bool, pub listening: String }

impl Member {
    fn parse(v: &Value) -> Member {
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        let l = &v["listening"];
        let listening = if l.is_object() { format!("{} {}", l["title"].as_str().unwrap_or(""), l["artist"].as_str().map(|a| format!("· {a}")).unwrap_or_default()).trim().to_string() } else { String::new() };
        Member { account: s("account"), name: s("name"), role: s("role"), devices: v["devices"].as_u64().unwrap_or(0), verified: v["verified"].as_bool().unwrap_or(false), me: v["me"].as_bool().unwrap_or(false), tag: s("tag"), same_name: v["same_name"].as_bool().unwrap_or(false), listening }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Panel {
    People(String),
    Person(String, String),
    Admin(String),
    Invite(String),
    Join,
    NewCommunity,
    Safety(Safety),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Safety { pub cid: String, pub account: String, pub name: String, pub number: String, pub verified: bool, pub ticked: Vec<bool> }

/// What a "sure?" question will do once answered.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Kick { cid: String, account: String },
    Unban { cid: String, account: String },
    Revoke { cid: String, id: String },
    DeleteRoom { cid: String, room: String },
    Transfer { cid: String, account: String },
    Leave { cid: String },
    ClearWall { cid: String, room: String },
}
#[derive(Clone, Debug, PartialEq)]
pub struct Confirm { pub title: String, pub body: String, pub label: String, pub act: Act }

/// What the invite dialog made: the text to tell a friend, and what to copy.
#[derive(Clone, Debug, Default)]
pub struct Made { pub tell: String, pub copy: String, pub copy_label: String, pub note: String }

pub struct MembersUi {
    pub panel: Option<Panel>,
    pub members: Vec<Member>,
    members_cid: String,
    members_at: Option<Instant>,
    loading: bool,
    pub confirm: Option<Confirm>,
    pub say: Option<(String, bool, Instant)>,
    pub community_name: Entity<InputState>,
    pub room_name: Entity<InputState>,
    pub room_voice: bool,
    pub room_edit: Option<String>,
    pub room_edit_name: Entity<InputState>,
    pub invite_pw: Entity<InputState>,
    pub invite_by_password: bool,
    pub invite_ttl: u64,
    pub invite_uses: Option<u32>,
    pub invite_hint: (String, u8),
    invite_seq: u64,
    pub made: Option<Made>,
    pub join_in: Entity<InputState>,
    pub new_name: Entity<InputState>,
    pub busy: bool,
}

impl MembersUi {
    pub fn new(window: &mut Window, cx: &mut Context<Mega>) -> (MembersUi, Vec<Subscription>) {
        let community_name = cx.new(|cx| InputState::new(window, cx).placeholder("Community name"));
        let room_name = cx.new(|cx| InputState::new(window, cx).placeholder("New room name"));
        let room_edit_name = cx.new(|cx| InputState::new(window, cx).placeholder("Room name"));
        let invite_pw = cx.new(|cx| InputState::new(window, cx).placeholder("a password to tell your friend"));
        let join_in = cx.new(|cx| InputState::new(window, cx).placeholder("an invite password, or paste the whole link"));
        let new_name = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. Book club"));
        let subs = vec![
            cx.subscribe_in(&invite_pw, window, |this: &mut Mega, _, ev: &InputEvent, window, cx| { if let InputEvent::Change = ev { this.invite_check(window, cx); } }),
            cx.subscribe_in(&join_in, window, |this: &mut Mega, _, ev: &InputEvent, window, cx| { if let InputEvent::PressEnter { .. } = ev { this.join_submit(window, cx); } }),
            cx.subscribe_in(&new_name, window, |this: &mut Mega, _, ev: &InputEvent, window, cx| { if let InputEvent::PressEnter { .. } = ev { this.new_community_submit(window, cx); } }),
        ];
        (MembersUi {
            panel: None, members: vec![], members_cid: String::new(), members_at: None, loading: false, confirm: None, say: None, community_name, room_name, room_voice: false, room_edit: None, room_edit_name,
            invite_pw, invite_by_password: true, invite_ttl: 604800, invite_uses: Some(1), invite_hint: (String::new(), 0), invite_seq: 0, made: None, join_in, new_name, busy: false,
        }, subs)
    }
}

// ---- what a role may do (mega-admin.js) ----
pub fn is_owner(c: &Community) -> bool { c.role == "owner" }
pub fn is_admin(c: &Community) -> bool { c.role == "owner" || c.role == "admin" }
fn can_remove(c: &Community, m: &Member) -> bool { !m.me && m.role != "owner" && (is_owner(c) || (is_admin(c) && m.role == "member")) }
fn can_role(c: &Community, m: &Member) -> bool { is_owner(c) && !m.me && m.role != "owner" }

fn when(expires: f64) -> String {
    let s = expires - crate::ringui::unix();
    if s <= 0. { return "expired".into(); }
    let (d, h, m) = ((s / 86400.).floor() as i64, ((s % 86400.) / 3600.).floor() as i64, ((s % 3600.) / 60.).floor() as i64);
    let plural = |n: i64, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
    format!("expires in {}", if d > 0 { plural(d, "day") } else if h > 0 { plural(h, "hour") } else { plural(m.max(1), "minute") })
}
fn uses_text(n: Option<u64>) -> String { match n { None => "unlimited uses".into(), Some(n) => format!("{n} use{} left", if n == 1 { "" } else { "s" }) } }
fn is_link(v: &str) -> bool { let v = v.trim().to_lowercase(); v.starts_with("commons:") || v.starts_with("http:") || v.starts_with("https:") }

fn pill(id: impl Into<ElementId>, on: bool, text: impl Into<SharedString>) -> Stateful<Div> {
    div().id(id).px_3().py_1().rounded_full().border_1().cursor_pointer().font_family(look::F_NAME).text_size(px(13.)).border_color(if on { look::paper() } else { look::line() })
        .when(on, |d| d.bg(look::paper()).text_color(rgb(0x0c0d12))).child(text.into())
}
fn danger_pill(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    div().id(id).px_3().py_1().rounded_full().border_1().cursor_pointer().font_family(look::F_NAME).text_size(px(13.)).border_color(rgb(look::DANGER)).text_color(rgb(look::DANGER)).child(text.into())
}
fn label(t: &str) -> Div { div().mt_3().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(t.to_string()) }
fn note(t: impl Into<SharedString>) -> Div { div().text_size(px(13.)).text_color(look::mute()).child(t.into()) }
fn chip(t: &str, on: bool) -> Div { div().px_2().rounded_full().border_1().border_color(if on { look::paper() } else { look::line() }).font_family(look::F_NAME).text_size(px(11.)).text_color(if on { look::paper() } else { look::mute() }).child(t.to_string()) }
fn small_avatar(name: &str, size: f32) -> Div {
    let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    div().size(px(size)).flex_none().rounded(px(size * 0.3)).bg(look::face(name)).text_color(rgb(0x0c0d12)).font_family(look::F_SHARP).text_size(px(size * 0.66)).line_height(px(size))
        .flex().items_center().justify_center().child(initial)
}

impl Mega {
    fn comm_of(&self, cid: &str) -> Option<Community> { self.snap.as_ref().and_then(|s| s.community(cid)).cloned() }
    pub fn say_members(&mut self, text: impl Into<String>, bad: bool, cx: &mut Context<Self>) { self.mem.say = Some((text.into(), bad, Instant::now())); cx.notify(); }

    // ---- opening and closing ----
    pub fn open_panel(&mut self, p: Panel, window: &mut Window, cx: &mut Context<Self>) {
        self.mem.confirm = None; self.mem.say = None; self.mem.made = None;
        match &p {
            Panel::People(cid) | Panel::Person(cid, _) | Panel::Admin(cid) | Panel::Invite(cid) => { let cid = cid.clone(); self.load_members(&cid, true, window, cx); }
            _ => {}
        }
        match &p {
            Panel::Admin(cid) => {
                let n = self.comm_of(cid).map(|c| c.name).unwrap_or_default();
                self.mem.community_name.update(cx, |i, cx| i.set_value(n, window, cx));
                self.mem.room_edit = None;
            }
            Panel::Invite(_) => {
                self.mem.invite_pw.update(cx, |i, cx| i.set_value("", window, cx));
                self.mem.invite_hint = (String::new(), 0);
                self.suggest_password(window, cx);
            }
            Panel::Join => { self.mem.join_in.update(cx, |i, cx| { i.set_value("", window, cx); i.focus(window, cx); }); }
            Panel::NewCommunity => { self.mem.new_name.update(cx, |i, cx| { i.set_value("", window, cx); i.focus(window, cx); }); }
            _ => {}
        }
        self.mem.panel = Some(p);
        cx.notify();
    }
    pub fn close_panel(&mut self, cx: &mut Context<Self>) { self.mem.panel = None; self.mem.confirm = None; self.mem.say = None; cx.notify(); }

    /// The members of one community, from the core (at most every two seconds unless forced).
    pub fn load_members(&mut self, cid: &str, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.mem.loading { return; }
        if !force && self.mem.members_cid == cid && self.mem.members_at.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) { return; }
        self.mem.loading = true;
        let cid = cid.to_string();
        let want = cid.clone();
        self.rpc("members", json!({"cid": cid}), window, cx, move |this, r, _, cx| {
            this.mem.loading = false;
            let Ok(v) = r else { return };
            if let Some(Panel::People(c) | Panel::Person(c, _) | Panel::Admin(c) | Panel::Invite(c)) = &this.mem.panel { if *c != want { return; } } else { return; }
            let ms: Vec<Member> = v.as_array().map(|l| l.iter().map(Member::parse).collect()).unwrap_or_default();
            this.mem.members_cid = want; this.mem.members_at = Some(Instant::now());
            if ms != this.mem.members { this.mem.members = ms; cx.notify(); }
        });
    }
    /// Every quarter second: keep the open panel's list fresh, let a message fade, close what lost its community.
    pub fn mem_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mem.say.as_ref().is_some_and(|(_, bad, at)| !*bad && at.elapsed() > Duration::from_secs(6)) { self.mem.say = None; cx.notify(); }
        let Some(p) = self.mem.panel.clone() else { return };
        let cid = match &p { Panel::People(c) | Panel::Person(c, _) | Panel::Admin(c) | Panel::Invite(c) => c.clone(), Panel::Safety(s) => s.cid.clone(), _ => return };
        let c = self.comm_of(&cid);
        let gone = c.as_ref().map_or(true, |c| c.removed) && self.snap.is_some();
        let lost_admin = matches!(p, Panel::Admin(_) | Panel::Invite(_)) && c.as_ref().is_some_and(|c| !is_admin(c));
        if gone || lost_admin { self.mem.panel = None; cx.notify(); return; }
        if !matches!(p, Panel::Safety(_)) { self.load_members(&cid, false, window, cx); }
    }

    /// Do something to a community and say how it went; both the list and the snapshot are read again at once.
    fn act(&mut self, method: &'static str, args: Value, ok: String, window: &mut Window, cx: &mut Context<Self>) {
        self.rpc(method, args, window, cx, move |this, r, window, cx| {
            match r { Ok(_) => this.mem.say = Some((ok, false, Instant::now())), Err(e) => this.mem.say = Some((format!("Could not do that: {}", e.trim_start_matches("[retry] ")), true, Instant::now())) }
            this.snap_hash.clear(); this.mem.members_at = None;
            this.tick(window, cx);
            if let Some(Panel::People(c) | Panel::Person(c, _) | Panel::Admin(c) | Panel::Invite(c)) = this.mem.panel.clone() { this.load_members(&c, true, window, cx); }
            cx.notify();
        });
    }
    pub fn run_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.mem.confirm.take() else { return };
        match c.act {
            Act::Kick { cid, account } => { self.act("kick", json!({"cid": cid, "account": account}), "Removing them. It finishes as soon as an admin has mega open.".into(), window, cx); if let Some(Panel::Person(c, _)) = self.mem.panel.clone() { self.mem.panel = Some(Panel::People(c)); } }
            Act::Unban { cid, account } => self.act("unban", json!({"cid": cid, "account": account}), "No longer banned.".into(), window, cx),
            Act::Revoke { cid, id } => self.act("revoke_invite", json!({"cid": cid, "id": id}), "Invite revoked.".into(), window, cx),
            Act::DeleteRoom { cid, room } => { self.mem.room_edit = None; self.act("delete_channel", json!({"cid": cid, "room": room}), "Room deleted.".into(), window, cx) }
            Act::Transfer { cid, account } => self.act("transfer_ownership", json!({"cid": cid, "account": account}), "Ownership handed over.".into(), window, cx),
            Act::Leave { cid } => { self.act("leave", json!({"cid": cid}), "Leaving. It is done as soon as an admin has mega open.".into(), window, cx); self.mem.panel = None; self.go(Level::Home, window, cx); }
            Act::ClearWall { cid, room } => self.act("set_wall", json!({"cid": cid, "room": room, "clear": true}), "Wall cleared.".into(), window, cx),
        }
        cx.notify();
    }
    fn ask(&mut self, title: impl Into<String>, body: impl Into<String>, label: &str, act: Act, cx: &mut Context<Self>) {
        self.mem.confirm = Some(Confirm { title: title.into(), body: body.into(), label: label.into(), act });
        cx.notify();
    }

    // ---- joining, creating ----
    pub fn join_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mem.busy { return; }
        let v = self.mem.join_in.read(cx).value().trim().to_string();
        if v.is_empty() { self.say_members("Type the invite password you were given, or paste the link.", true, cx); return; }
        self.join_with(v, window, cx);
    }
    /// A link (an invite someone sent, or one given on the command line) or a password asks to join; an admin lets the account in when mega is open.
    pub fn join_with(&mut self, v: String, window: &mut Window, cx: &mut Context<Self>) {
        self.mem.busy = true;
        let (method, args) = if is_link(&v) { ("join", json!({"link": v})) } else { ("join_password", json!({"password": v})) };
        self.rpc(method, args, window, cx, |this, r, window, cx| {
            this.mem.busy = false;
            match r {
                Ok(_) => { this.mem.panel = None; this.mem.say = None; this.say("Request sent. An admin has to be online to let you in.", cx); }
                Err(e) => this.mem.say = Some((e.trim_start_matches("[retry] ").to_string(), true, Instant::now())),
            }
            this.snap_hash.clear(); this.tick(window, cx); cx.notify();
        });
    }
    pub fn new_community_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.mem.new_name.read(cx).value().trim().to_string();
        if n.is_empty() { self.say_members("A community needs a name.", true, cx); return; }
        self.rpc("create_community", json!({"name": n}), window, cx, |this, r, window, cx| {
            match r { Ok(v) => { this.mem.panel = None; this.snap_hash.clear(); this.tick(window, cx); if let Some(id) = v["id"].as_str() { this.go(Level::Server(id.to_string()), window, cx); } } Err(e) => this.mem.say = Some((e, true, Instant::now())) }
            cx.notify();
        });
    }
    pub fn cancel_join(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.rpc("cancel_join", json!({"id": id}), window, cx, |this, _, window, cx| { this.snap_hash.clear(); this.tick(window, cx); });
    }

    // ---- invites ----
    fn suggest_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rpc("suggest_password", json!({}), window, cx, |this, r, window, cx| {
            if let Ok(v) = r { let p = v["password"].as_str().unwrap_or("").to_string(); this.mem.invite_pw.update(cx, |i, cx| i.set_value(p, window, cx)); }
            this.invite_check(window, cx);
        });
    }
    fn invite_check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pw = self.mem.invite_pw.read(cx).value().trim().to_string();
        self.mem.invite_seq += 1;
        let my = self.mem.invite_seq;
        if pw.is_empty() { self.mem.invite_hint = (String::new(), 0); cx.notify(); return; }
        self.rpc("password_strength", json!({"password": pw}), window, cx, move |this, r, _, cx| {
            if my != this.mem.invite_seq { return; }
            let Ok(v) = r else { return };
            let (ok, warn) = (v["ok"].as_bool().unwrap_or(false), v["warn"].as_bool().unwrap_or(false));
            this.mem.invite_hint = if !ok { ("Too easy to guess: the server could work it out and read the invite. Use the suggestion, or five or more unrelated words.".into(), 3) }
                else if warn { ("Fine for a few days. A longer password is safer.".into(), 2) } else { ("Good.".into(), 1) };
            cx.notify();
        });
    }
    fn invite_create(&mut self, cid: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.mem.busy { return; }
        let by_pw = self.mem.invite_by_password;
        let pw = self.mem.invite_pw.read(cx).value().trim().to_string();
        let (ttl, uses) = (self.mem.invite_ttl, self.mem.invite_uses);
        if by_pw && (pw.is_empty() || self.mem.invite_hint.1 == 3) { self.say_members("Choose a password that is hard to guess first.", true, cx); return; }
        let mut args = json!({"cid": cid, "ttl": ttl});
        if by_pw { args["password"] = pw.clone().into(); args["uses"] = uses.unwrap_or(200).into(); } else if let Some(u) = uses { args["uses"] = u.into(); }
        let (name, relay) = (self.comm_of(&cid).map(|c| c.name).unwrap_or_default(), self.snap.as_ref().map(|s| s.relay.clone()).unwrap_or_default());
        self.mem.busy = true;
        self.rpc("create_invite", args, window, cx, move |this, r, _, cx| {
            this.mem.busy = false;
            match r {
                Ok(v) => {
                    let until = chrono_until(ttl);
                    let who = match uses { Some(1) => "one person".to_string(), Some(n) => format!("up to {n} people"), None => "anyone with it".to_string() };
                    let secret = if v["signup_needs_secret"].as_bool() == Some(true) { format!("This device does not know the server's sign-up secret, so this only works for people who already have an account on {relay}.") } else { String::new() };
                    this.mem.made = Some(if by_pw {
                        let dl = v["download"].as_str().unwrap_or("").to_string();
                        Made { tell: format!("Join \"{name}\" on mega.\n1. Get the app: {dl}\n2. Open it and type this invite password: {pw}\n3. Choose your name and a password for your computer.\nThe password works for {who} until {until}."),
                            copy: pw.clone(), copy_label: "Copy the password".into(), note: format!("mega does not keep the password: write it down now. {secret}") }
                    } else {
                        let link = v["link"].as_str().unwrap_or("").to_string();
                        Made { tell: format!("Invite link for \"{name}\" (valid until {until}, admits {who}):\n{link}"), copy: link, copy_label: "Copy the link".into(), note: format!("Send this link privately. An admin has to be online (now or later) to let them in. {secret}") }
                    });
                }
                Err(e) => this.mem.say = Some((e.trim_start_matches("[retry] ").to_string(), true, Instant::now())),
            }
            this.snap_hash.clear(); cx.notify();
        });
    }

    // ---- safety numbers ----
    fn open_safety(&mut self, cid: String, account: String, window: &mut Window, cx: &mut Context<Self>) {
        self.rpc("safety", json!({"account": account}), window, cx, move |this, r, _, cx| {
            match r {
                Ok(v) => {
                    let number = v["number"].as_str().unwrap_or("").to_string();
                    let n = number.split(' ').count();
                    this.mem.panel = Some(Panel::Safety(Safety { cid, account, name: v["name"].as_str().unwrap_or("").to_string(), number, verified: v["verified"].as_bool().unwrap_or(false), ticked: vec![false; n] }));
                }
                Err(e) => this.mem.say = Some((e, true, Instant::now())),
            }
            cx.notify();
        });
    }

    // ---- rooms and colours ----
    fn set_style_for(&mut self, cid: String, room: String, duo: Option<(u32, u32)>, window: &mut Window, cx: &mut Context<Self>) {
        let (main, hi) = duo.map(|(m, h)| (look::hex(m), look::hex(h))).unwrap_or_default();
        self.act("set_style", json!({"cid": cid, "room": room, "main": main, "hi": hi}), "Colours changed.".into(), window, cx);
    }
    fn swatches(&mut self, prefix: String, cid: String, room: String, cx: &mut Context<Self>) -> Div {
        let mut row = h_flex().gap_2().items_center().flex_wrap();
        for (i, (_n, main, hi)) in look::PRESETS.iter().enumerate() {
            let (m, h, cid, room) = (*main, *hi, cid.clone(), room.clone());
            row = row.child(div().id(SharedString::from(format!("{prefix}-p{i}"))).size(px(30.)).rounded_full().cursor_pointer().overflow_hidden().border_1().border_color(look::line())
                .child(div().size_full().bg(linear_gradient(135., linear_color_stop(rgb(m), 0.5), linear_color_stop(rgb(h), 0.5))))
                .on_click(cx.listener(move |this, _, window, cx| this.set_style_for(cid.clone(), room.clone(), Some((m, h)), window, cx))));
        }
        let (cid, room) = (cid.clone(), room.clone());
        row.child(pill(SharedString::from(format!("{prefix}-def")), false, "default").on_click(cx.listener(move |this, _, window, cx| this.set_style_for(cid.clone(), room.clone(), None, window, cx))))
    }

    // ---- drawing ----
    /// The panel over the window, if one is open.
    pub fn members_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let panel = self.mem.panel.clone()?;
        let cid = match &panel { Panel::People(c) | Panel::Person(c, _) | Panel::Admin(c) | Panel::Invite(c) => Some(c.clone()), Panel::Safety(s) => Some(s.cid.clone()), _ => None };
        let comm = cid.as_ref().and_then(|c| self.comm_of(c));
        if cid.is_some() && comm.is_none() { return None; }
        let duo = comm.as_ref().map(|c| Duo::of(&c.id, c.colours.as_ref())).unwrap_or(Duo { main: 0x6a35ff, hi: 0xff8c1a });
        let body: AnyElement = if let Some(c) = self.mem.confirm.clone() { self.confirm_view(c, cx).into_any_element() } else { match panel.clone() {
            Panel::People(_) => self.people_view(comm.unwrap(), cx).into_any_element(),
            Panel::Person(_, acct) => self.person_view(comm.unwrap(), acct, cx).into_any_element(),
            Panel::Admin(_) => self.admin_view(comm.unwrap(), cx).into_any_element(),
            Panel::Invite(_) => self.invite_view(comm.unwrap(), cx).into_any_element(),
            Panel::Join => self.join_view(cx).into_any_element(),
            Panel::NewCommunity => self.new_view(cx).into_any_element(),
            Panel::Safety(s) => self.safety_view(s, cx).into_any_element(),
        } };
        let say = self.mem.say.clone();
        Some(div().id("mem-overlay").absolute().inset_0().flex().items_center().justify_center().occlude()
            .child(div().id("mem-scrim").absolute().inset_0().bg(hsla(0., 0., 0., 0.62)).on_click(cx.listener(|this, _, _, cx| this.close_panel(cx))))
            .child(v_flex().id("mem-card").relative().w(px(600.)).max_w_full().max_h(px(660.)).mx_4().rounded(px(30.)).bg(rgb(look::SLIP)).border_1().border_color(look::line()).shadow_lg().overflow_hidden()
                .child(div().flex_none().h(px(8.)).w_full().bg(linear_gradient(90., linear_color_stop(rgb(duo.main), 0.5), linear_color_stop(rgb(duo.hi), 0.5))))
                .child(v_flex().id("mem-body").flex_1().min_h_0().overflow_y_scroll().px_6().pt_4().pb_5().gap_2()
                    .when_some(say, |d, (t, bad, _)| d.child(div().id("mem-say").px_3().py_1().rounded(px(12.)).border_1().border_color(if bad { rgb(look::DANGER).into() } else { look::line() }).text_color(if bad { rgb(look::DANGER).into() } else { look::paper() }).text_size(px(13.)).child(t)))
                    .child(body)))
            .into_any_element())
    }

    fn title(&self, t: impl Into<SharedString>, sub: impl Into<SharedString>, cx: &mut Context<Self>) -> Div {
        h_flex().items_end().gap_3()
            .child(v_flex().flex_1().min_w_0().child(div().font_family(look::F_SHARP).text_size(px(44.)).line_height(px(42.)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(t.into()))
                .child(div().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(sub.into())))
            .child(pill("mem-close", false, "close").on_click(cx.listener(|this, _, _, cx| this.close_panel(cx))))
    }

    fn confirm_view(&mut self, c: Confirm, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().gap_3().child(self.title(c.title.clone(), "are you sure", cx)).child(div().text_size(px(15.)).child(c.body.clone()))
            .child(h_flex().gap_2().justify_end()
                .child(pill("conf-no", false, "Cancel").on_click(cx.listener(|this, _, _, cx| { this.mem.confirm = None; cx.notify(); })))
                .child(danger_pill("conf-yes", c.label.clone()).on_click(cx.listener(|this, _, window, cx| this.run_confirm(window, cx)))))
    }

    fn online(&self, m: &Member) -> bool {
        m.me || self.pres.peers.contains_key(&m.account) || self.pres.calls.values().any(|l| l.iter().any(|n| *n == m.name)) || self.call.others().contains(&m.account)
    }

    fn people_view(&mut self, c: Community, cx: &mut Context<Self>) -> impl IntoElement {
        let mut ms = self.mem.members.clone();
        let rank = |m: &Member| match m.role.as_str() { "owner" => 0, "admin" => 1, _ => 2 };
        ms.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        let others: Vec<&Member> = ms.iter().filter(|m| !m.me).collect();
        let done = others.iter().filter(|m| m.verified).count();
        let mut list = v_flex().gap_1();
        for m in &ms {
            let (acct, cid) = (m.account.clone(), c.id.clone());
            let on = self.online(m);
            list = list.child(h_flex().id(SharedString::from(format!("person-{}", m.account))).gap_3().items_center().px_3().py_2().rounded(px(18.)).border_1().border_color(look::line()).cursor_pointer()
                .child(div().size(px(10.)).flex_none().rounded_full().when(on, |d| d.bg(rgb(look::SPEAK))).when(!on, |d| d.border_1().border_color(look::mute())))
                .child(small_avatar(&m.name, 36.))
                .child(v_flex().flex_1().min_w_0()
                    .child(h_flex().gap_2().items_center().child(div().font_family(look::F_SHARP).text_size(px(24.)).line_height(px(24.)).child(m.name.clone()))
                        .when(m.me, |d| d.child(chip("you", false)))
                        .when(m.role != "member", |d| d.child(chip(&m.role, true)))
                        .when(m.verified, |d| d.child(chip("verified", false))))
                    .child(div().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute())
                        .child(format!("{}{} device{}{}", if m.same_name { format!("{} · ", m.tag) } else { String::new() }, m.devices, if m.devices == 1 { "" } else { "s" }, if m.listening.is_empty() { String::new() } else { format!(" · {}", m.listening) }))))
                .on_click(cx.listener(move |this, _, window, cx| this.open_panel(Panel::Person(cid.clone(), acct.clone()), window, cx))));
        }
        if ms.is_empty() { list = list.child(note("loading…")); }
        let cid = c.id.clone();
        v_flex().gap_3().child(self.title(format!("{} · people", c.name), format!("{} {}", ms.len(), if ms.len() == 1 { "person" } else { "people" }), cx))
            .when(!others.is_empty(), |d| d.child(note(if done > 0 { format!("You have verified {done} of {}.", others.len()) } else { "Nobody verified yet. Open a person to compare safety numbers.".to_string() })))
            .child(list)
            .child(h_flex().gap_2().flex_wrap().pt_2()
                .when(is_admin(&c), |d| { let (a, b) = (cid.clone(), cid.clone()); d
                    .child(pill("p-invite", false, "Invite someone").on_click(cx.listener(move |this, _, window, cx| this.open_panel(Panel::Invite(a.clone()), window, cx))))
                    .child(pill("p-manage", false, "Manage community").on_click(cx.listener(move |this, _, window, cx| this.open_panel(Panel::Admin(b.clone()), window, cx)))) })
                .when(!is_owner(&c), |d| { let (cid, name) = (cid.clone(), c.name.clone()); d.child(danger_pill("p-leave", "Leave community").on_click(cx.listener(move |this, _, _, cx| {
                    this.ask(format!("Leave {name}?"), "You stop getting its messages. What you already received stays readable on this device. To come back later you need a new invite.", "Leave", Act::Leave { cid: cid.clone() }, cx);
                }))) }))
    }

    fn person_view(&mut self, c: Community, acct: String, cx: &mut Context<Self>) -> AnyElement {
        let Some(m) = self.mem.members.iter().find(|m| m.account == acct).cloned() else {
            let cid = c.id.clone();
            return v_flex().gap_3().child(self.title("…", "this person is not in the list", cx)).child(pill("back", false, "back to people").on_click(cx.listener(move |this, _, window, cx| this.open_panel(Panel::People(cid.clone()), window, cx)))).into_any_element();
        };
        let cid = c.id.clone();
        let in_call = self.call.others().contains(&m.account);
        let direct = self.snap.as_ref().and_then(|s| s.direct.iter().find(|d| d.peer == m.account)).map(|d| (d.id.clone(), d.room.clone()));
        let on = self.online(&m);
        let mut acts = h_flex().gap_2().flex_wrap();
        if !m.me {
            let (a, n) = (m.account.clone(), m.name.clone());
            if let Some((dcid, room)) = direct { acts = acts.child(pill("pc-msg", false, "Message").on_click(cx.listener(move |this, _, window, cx| { this.mem.panel = None; this.open_room(dcid.clone(), room.clone(), window, cx); }))); }
            acts = acts.child(pill("pc-call", false, "Call").on_click(cx.listener(move |this, _, window, cx| { this.mem.panel = None; this.dial(a.clone(), n.clone(), window, cx); })));
            if in_call { let (a, n) = (m.account.clone(), m.name.clone()); acts = acts.child(pill("pc-vol", false, "Volume in the call").on_click(cx.listener(move |this, _, window, cx| { this.mem.panel = None; this.open_person(a.clone(), n.clone(), window, cx); }))); }
            let (a, cc) = (m.account.clone(), cid.clone());
            acts = acts.child(pill("pc-safety", false, if m.verified { "Safety number (verified)" } else { "Verify safety number" }).on_click(cx.listener(move |this, _, window, cx| this.open_safety(cc.clone(), a.clone(), window, cx))));
        }
        let mut admin = h_flex().gap_2().flex_wrap();
        if can_role(&c, &m) {
            let (cc, a, to) = (cid.clone(), m.account.clone(), if m.role == "admin" { "member" } else { "admin" });
            admin = admin.child(pill("pc-role", false, if m.role == "admin" { "Make a regular member" } else { "Make admin" }).on_click(cx.listener(move |this, _, window, cx| {
                this.act("set_role", json!({"cid": cc.clone(), "account": a.clone(), "role": to}), if to == "admin" { "They are now an admin.".into() } else { "They are now a regular member.".into() }, window, cx);
            })));
            let (cc, a, n, cn) = (cid.clone(), m.account.clone(), m.name.clone(), c.name.clone());
            admin = admin.child(danger_pill("pc-owner", "Make them the owner").on_click(cx.listener(move |this, _, _, cx| {
                this.ask(format!("Make {n} the owner?"), format!("{n} becomes the owner of {cn}. You stay on as an admin and can leave afterwards. You cannot take it back: only {n} could hand it over again. You will lose the owner-only controls, such as roles and colours."), "Hand over ownership", Act::Transfer { cid: cc.clone(), account: a.clone() }, cx);
            })));
        }
        if can_remove(&c, &m) {
            let (cc, a, n) = (cid.clone(), m.account.clone(), m.name.clone());
            admin = admin.child(danger_pill("pc-kick", "Remove and ban").on_click(cx.listener(move |this, _, _, cx| {
                this.ask(format!("Remove and ban {n}?"), format!("{n} is removed from every room and the keys change, so they cannot read anything sent from now on. They keep what they already received. They are banned, so an old invite will not let them back in. An admin can lift the ban later."), "Remove and ban", Act::Kick { cid: cc.clone(), account: a.clone() }, cx);
            })));
        }
        let back = cid.clone();
        v_flex().gap_3()
            .child(h_flex().gap_4().items_center().child(small_avatar(&m.name, 84.))
                .child(v_flex().flex_1().min_w_0().gap_1()
                    .child(div().font_family(look::F_SHARP).text_size(px(52.)).line_height(px(48.)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(m.name.clone()))
                    .child(h_flex().gap_2().items_center().child(div().size(px(10.)).rounded_full().when(on, |d| d.bg(rgb(look::SPEAK))).when(!on, |d| d.border_1().border_color(look::mute())))
                        .child(div().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(if on { "here now" } else { "not seen right now" }))
                        .child(chip(if m.role == "member" { "member" } else { &m.role }, m.role != "member")).when(m.me, |d| d.child(chip("you", false))).when(m.verified, |d| d.child(chip("verified", false))))))
            .child(note(format!("{}{} device{} · in {}{}", if m.same_name { format!("{} · ", m.tag) } else { String::new() }, m.devices, if m.devices == 1 { "" } else { "s" }, c.name, if m.listening.is_empty() { String::new() } else { format!(" · listening to {}", m.listening) })))
            .child(acts)
            .when(can_role(&c, &m) || can_remove(&c, &m), |d| d.child(label("as an admin")).child(admin))
            .child(h_flex().pt_2().child(pill("pc-back", false, "back to people").on_click(cx.listener(move |this, _, window, cx| this.open_panel(Panel::People(back.clone()), window, cx)))))
            .into_any_element()
    }

    fn safety_view(&mut self, s: Safety, cx: &mut Context<Self>) -> impl IntoElement {
        let parts: Vec<String> = s.number.split(' ').map(String::from).collect();
        let n = s.ticked.iter().filter(|x| **x).count();
        let mut grid = h_flex().gap_2().flex_wrap();
        for (i, g) in parts.iter().enumerate() {
            let on = s.ticked.get(i).copied().unwrap_or(false);
            grid = grid.child(div().id(SharedString::from(format!("sg-{i}"))).px_3().py_2().rounded(px(14.)).border_1().cursor_pointer().border_color(if on { look::paper() } else { look::line() }).when(on, |d| d.bg(look::paper()).text_color(rgb(0x0c0d12)))
                .font_family(look::F_MONO).text_size(px(20.)).child(g.clone())
                .on_click(cx.listener(move |this, _, _, cx| { if let Some(Panel::Safety(s)) = &mut this.mem.panel { if let Some(t) = s.ticked.get_mut(i) { *t = !*t; } } cx.notify(); })));
        }
        let (cid, acct, verified) = (s.cid.clone(), s.account.clone(), s.verified);
        let back = (cid.clone(), acct.clone());
        v_flex().gap_3().child(self.title(format!("Verify {}", s.name), if s.verified { "verified by you" } else { "not verified yet" }, cx))
            .child(note(format!("Compare these numbers with {} in person or over a call. If they match, nobody (including the relay) is sitting between you.", s.name)))
            .child(grid)
            .child(note(if n == parts.len() && n > 0 { format!("All {n} groups compared. If every one matched, mark them verified.") } else if n > 0 { format!("{n} of {} groups compared.", parts.len()) } else { "Tap each group as you read it out.".to_string() }))
            .child(h_flex().gap_2().justify_end()
                .child(pill("sf-back", false, "back").on_click(cx.listener(move |this, _, window, cx| this.open_panel(Panel::Person(back.0.clone(), back.1.clone()), window, cx))))
                .child(pill("sf-ok", true, if verified { "Mark as not verified" } else { "They match, mark verified" }).on_click(cx.listener(move |this, _, window, cx| {
                    let (cid, acct) = (cid.clone(), acct.clone());
                    this.rpc("set_verified", json!({"account": acct.clone(), "on": !verified}), window, cx, move |this, _, window, cx| { this.open_panel(Panel::Person(cid.clone(), acct.clone()), window, cx); });
                }))))
    }

    fn join_view(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.mem.busy;
        v_flex().gap_3().child(self.title("Join a community", "with an invite", cx))
            .child(note("Type the invite password you were given, or paste the whole invite link. An admin has to be online (now or later) to let you in."))
            .child(Input::new(&self.mem.join_in))
            .child(h_flex().justify_end().child(Button::new("join-go").primary().label(if busy { "Asking…" } else { "Join" }).on_click(cx.listener(|this, _, window, cx| this.join_submit(window, cx)))))
    }
    fn new_view(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().gap_3().child(self.title("New community", "a private place with its own rooms", cx))
            .child(note("You will be its owner, and only people you invite can read it."))
            .child(Input::new(&self.mem.new_name))
            .child(h_flex().justify_end().child(Button::new("new-go").primary().label("Create").on_click(cx.listener(|this, _, window, cx| this.new_community_submit(window, cx)))))
    }

    fn invite_view(&mut self, c: Community, cx: &mut Context<Self>) -> impl IntoElement {
        let cid = c.id.clone();
        let m = &self.mem;
        let (by_pw, ttl, uses, hint, made, busy) = (m.invite_by_password, m.invite_ttl, m.invite_uses, m.invite_hint.clone(), m.made.clone(), m.busy);
        let mut col = v_flex().gap_3().child(self.title(format!("Invite to {}", c.name), "make a password or a link", cx));
        if let Some(made) = made {
            let copy = made.copy.clone();
            col = col.child(label("Tell your friend")).child(div().id("made-tell").p_3().rounded(px(16.)).border_1().border_color(look::line()).font_family(look::F_MONO).text_size(px(13.)).child(made.tell.clone()))
                .child(note(made.note.clone()))
                .child(h_flex().gap_2().justify_end()
                    .child(pill("made-copy", false, made.copy_label.clone()).on_click(cx.listener(move |this, _, _, cx| { cx.write_to_clipboard(ClipboardItem::new_string(copy.clone())); this.say_members("Copied.", false, cx); })))
                    .child(pill("made-copy-all", true, "Copy the message").on_click({ let t = made.tell.clone(); cx.listener(move |this, _, _, cx| { cx.write_to_clipboard(ClipboardItem::new_string(t.clone())); this.say_members("Copied.", false, cx); }) }))
                    .child(pill("made-more", false, "Make another").on_click(cx.listener(|this, _, window, cx| { this.mem.made = None; this.suggest_password(window, cx); cx.notify(); }))));
        } else {
            col = col.child(h_flex().gap_2()
                .child(pill("iv-pw", by_pw, "Password").on_click(cx.listener(|this, _, _, cx| { this.mem.invite_by_password = true; cx.notify(); })))
                .child(pill("iv-link", !by_pw, "Link").on_click(cx.listener(|this, _, _, cx| { this.mem.invite_by_password = false; cx.notify(); }))));
            if by_pw {
                col = col.child(label("a password to tell your friend"))
                    .child(h_flex().gap_2().child(div().flex_1().child(Input::new(&self.mem.invite_pw))).child(pill("iv-another", false, "another").on_click(cx.listener(|this, _, window, cx| this.suggest_password(window, cx)))))
                    .when(!hint.0.is_empty(), |d| d.child(div().text_size(px(13.)).text_color(if hint.1 == 3 { rgb(look::DANGER).into() } else if hint.1 == 2 { look::paper() } else { rgb(look::SPEAK).into() }).child(hint.0.clone())));
            }
            col = col.child(label("valid for"));
            let mut r = h_flex().gap_2().flex_wrap();
            for (i, (s, t)) in [(3600u64, "1 hour"), (86400, "1 day"), (259200, "3 days"), (604800, "7 days"), (2592000, "30 days")].into_iter().enumerate() {
                r = r.child(pill(SharedString::from(format!("iv-ttl{i}")), ttl == s, t).on_click(cx.listener(move |this, _, _, cx| { this.mem.invite_ttl = s; cx.notify(); })));
            }
            col = col.child(r).child(label("how many people can use it"));
            let mut r = h_flex().gap_2().flex_wrap();
            for (i, (u, t)) in [(Some(1u32), "one person"), (Some(5), "up to 5"), (Some(20), "up to 20"), (None, "no limit")].into_iter().enumerate() {
                if u.is_none() && by_pw { continue; }
                r = r.child(pill(SharedString::from(format!("iv-use{i}")), uses == u, t).on_click(cx.listener(move |this, _, _, cx| { this.mem.invite_uses = u; cx.notify(); })));
            }
            let cc = cid.clone();
            col = col.child(r).child(note(if by_pw { "Anyone who guesses the password can join, so it expires and has a limit. You can revoke it at any time." } else { "Send the link privately. Whoever has it can ask to join; an admin has to be online (now or later) to let them in." }))
                .child(h_flex().justify_end().child(Button::new("iv-go").primary().label(if busy { "Working…" } else if by_pw { "Create invite" } else { "Create link" }).on_click(cx.listener(move |this, _, window, cx| this.invite_create(cc.clone(), window, cx)))));
        }
        let now = crate::ringui::unix();
        let open: Vec<_> = c.invites.iter().filter(|i| i.mine && i.expires > now).cloned().collect();
        if !open.is_empty() {
            col = col.child(label("your open invites"));
            for i in open {
                let (cc, id) = (cid.clone(), i.id.clone());
                col = col.child(h_flex().gap_3().items_center().child(div().flex_1().text_size(px(14.)).child(format!("{} · {} · {}", if i.password { "Password" } else { "Link" }, uses_text(i.uses_left), when(i.expires))))
                    .child(danger_pill(SharedString::from(format!("rev-{}", i.id)), "revoke").on_click(cx.listener(move |this, _, _, cx| this.ask("Revoke this invite?", "The link stops working for anyone who has not used it yet. People already in are not affected.", "Revoke invite", Act::Revoke { cid: cc.clone(), id: id.clone() }, cx)))));
            }
        }
        col
    }

    fn admin_view(&mut self, c: Community, cx: &mut Context<Self>) -> impl IntoElement {
        let cid = c.id.clone();
        let owner = is_owner(&c);
        let mut col = v_flex().gap_2().child(self.title(format!("{} · settings", c.name), if owner { "you own this community" } else { "you are an admin here" }, cx))
            .child(note("Everyone in the community sees these changes."));
        // name
        let cc = cid.clone();
        col = col.child(label("name")).child(h_flex().gap_2().child(div().flex_1().child(Input::new(&self.mem.community_name)))
            .child(pill("ad-rename", false, "save").on_click(cx.listener(move |this, _, window, cx| {
                let name = this.mem.community_name.read(cx).value().trim().to_string();
                if name.is_empty() { this.say_members("A community needs a name.", true, cx); return; }
                this.act("rename_community", json!({"cid": cc.clone(), "name": name.clone()}), format!("Renamed to {name}."), window, cx);
            }))));
        // colours of the community's own card (owner only)
        if owner { col = col.child(label("the community's colours")).child(self.swatches("ad-cc".into(), cid.clone(), String::new(), cx)); }
        // rooms
        col = col.child(label("rooms"));
        let editing = self.mem.room_edit.clone();
        for ch in &c.channels {
            let (gid, name) = (ch.gid.clone(), ch.name.clone());
            let open = editing.as_deref() == Some(gid.as_str());
            let g2 = gid.clone();
            col = col.child(h_flex().id(SharedString::from(format!("rm-{gid}"))).gap_3().items_center().px_3().py_1().rounded(px(14.)).border_1().border_color(if open { look::paper() } else { look::line() }).cursor_pointer()
                .child(div().w(px(46.)).font_family(look::F_NAME).text_size(px(11.)).text_color(look::mute()).child(if ch.kind == "voice" { "voice" } else { "text" }))
                .child(div().flex_1().font_family(look::F_SHARP).text_size(px(22.)).child(ch.name.clone()))
                .child(div().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(if open { "close" } else { "edit" }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.mem.room_edit.as_deref() == Some(g2.as_str()) { this.mem.room_edit = None; } else { this.mem.room_edit = Some(g2.clone()); let n = name.clone(); this.mem.room_edit_name.update(cx, |i, cx| i.set_value(n, window, cx)); }
                    cx.notify();
                })));
            if open {
                let (a, b, d) = (cid.clone(), cid.clone(), cid.clone());
                let (g_a, g_b, g_c, g_d, g_e) = (gid.clone(), gid.clone(), gid.clone(), gid.clone(), gid.clone());
                let wall_locked = c.locked.contains(&gid);
                let n_rooms = c.channels.len();
                let mut ed = v_flex().gap_2().ml_4().p_3().rounded(px(14.)).bg(hsla(0., 0., 1., 0.04))
                    .child(h_flex().gap_2().child(div().flex_1().child(Input::new(&self.mem.room_edit_name)))
                        .child(pill("rm-rename", false, "rename").on_click(cx.listener(move |this, _, window, cx| {
                            let name = this.mem.room_edit_name.read(cx).value().trim().to_string();
                            if name.is_empty() { this.say_members("A room needs a name.", true, cx); return; }
                            this.act("rename_channel", json!({"cid": a.clone(), "room": g_a.clone(), "name": name}), "Renamed the room.".into(), window, cx);
                        }))));
                if owner {
                    ed = ed.child(div().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child("colours of this room")).child(self.swatches(format!("rm-{gid}"), cid.clone(), gid.clone(), cx))
                        .child(h_flex().gap_2().flex_wrap()
                            .child(pill("rm-lock", wall_locked, if wall_locked { "wall locked: unlock" } else { "lock the wall" }).on_click(cx.listener(move |this, _, window, cx| {
                                this.act("set_wall", json!({"cid": b.clone(), "room": g_b.clone(), "lock": if wall_locked { "0" } else { "1" }}), if wall_locked { "The wall is open to everyone again.".into() } else { "The wall is locked: only you can draw.".into() }, window, cx);
                            })))
                            .child(danger_pill("rm-clear", "clear the wall").on_click(cx.listener(move |this, _, _, cx| {
                                this.ask("Clear the wall?", "Everything drawn on this room's wall is wiped for everyone.", "Clear the wall", Act::ClearWall { cid: d.clone(), room: g_c.clone() }, cx);
                            }))));
                }
                if n_rooms > 1 {
                    let (cc, n) = (cid.clone(), ch.name.clone());
                    ed = ed.child(h_flex().child(danger_pill("rm-del", "delete this room").on_click(cx.listener(move |this, _, _, cx| {
                        this.ask(format!("Delete {n}?"), format!("The room {n} disappears for everyone, and nobody can open it again. This cannot be undone."), "Delete room", Act::DeleteRoom { cid: cc.clone(), room: g_d.clone() }, cx);
                    }))));
                } else { ed = ed.child(note("A community keeps at least one room, so the last one cannot be deleted.")); }
                let _ = g_e;
                col = col.child(ed);
            }
        }
        let (cc, voice) = (cid.clone(), self.mem.room_voice);
        col = col.child(h_flex().gap_2().items_center().child(div().flex_1().child(Input::new(&self.mem.room_name)))
            .child(pill("ad-text", !voice, "text").on_click(cx.listener(|this, _, _, cx| { this.mem.room_voice = false; cx.notify(); })))
            .child(pill("ad-voice", voice, "voice").on_click(cx.listener(|this, _, _, cx| { this.mem.room_voice = true; cx.notify(); })))
            .child(pill("ad-addroom", false, "add room").on_click(cx.listener(move |this, _, window, cx| {
                let name = this.mem.room_name.read(cx).value().trim().to_string();
                if name.is_empty() { this.say_members("Give the room a name first.", true, cx); return; }
                let voice = this.mem.room_voice;
                this.mem.room_name.update(cx, |i, cx| i.set_value("", window, cx));
                this.act("create_channel", json!({"cid": cc.clone(), "name": name.clone(), "kind": if voice { "voice" } else { "" }}), format!("Added the room {name}."), window, cx);
            }))));
        // people and invites
        let (a, b) = (cid.clone(), cid.clone());
        col = col.child(label("people and invites")).child(h_flex().gap_2().flex_wrap()
            .child(pill("ad-people", false, "people and roles").on_click(cx.listener(move |this, _, window, cx| this.open_panel(Panel::People(a.clone()), window, cx))))
            .child(pill("ad-invite", false, "invite someone").on_click(cx.listener(move |this, _, window, cx| this.open_panel(Panel::Invite(b.clone()), window, cx)))));
        let now = crate::ringui::unix();
        let open: Vec<_> = c.invites.iter().filter(|i| i.expires > now).cloned().collect();
        col = col.child(label("open invites"));
        if open.is_empty() { col = col.child(note("No open invites.")); }
        for i in open {
            let (cc, id) = (cid.clone(), i.id.clone());
            col = col.child(h_flex().gap_3().items_center().child(div().flex_1().text_size(px(14.)).child(format!("{} · {}{}", uses_text(i.uses_left), when(i.expires), if i.mine { " · yours" } else { "" })))
                .child(danger_pill(SharedString::from(format!("adrev-{}", i.id)), "revoke").on_click(cx.listener(move |this, _, _, cx| this.ask("Revoke this invite?", "The link stops working for anyone who has not used it yet. People already in are not affected.", "Revoke invite", Act::Revoke { cid: cc.clone(), id: id.clone() }, cx)))));
        }
        col = col.child(label("banned"));
        if c.banned.is_empty() { col = col.child(note("Nobody is banned.")); }
        for (acct, name) in c.banned.clone() {
            let (cc, a, n) = (cid.clone(), acct.clone(), name.clone());
            col = col.child(h_flex().gap_3().items_center().child(div().flex_1().child(name.clone()))
                .child(pill(SharedString::from(format!("unban-{acct}")), false, "unban").on_click(cx.listener(move |this, _, _, cx| this.ask(format!("Unban {n}?"), format!("{n} can be invited again. They are not added back by themselves."), "Unban", Act::Unban { cid: cc.clone(), account: a.clone() }, cx)))));
        }
        col.child(note("Removing someone also bans them: they cannot come back with an old invite. To hand over ownership, open the person under people."))
    }

    /// The buttons that open these panels, for the top bar of a community's cards, and for the home screen.
    pub fn community_buttons(&mut self, cid: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(c) = self.comm_of(cid) else { return vec![] };
        let mut out: Vec<AnyElement> = vec![];
        let a = cid.to_string();
        out.push(crate::cards::chip("b-people", "people").on_click(cx.listener(move |this, _, window, cx| this.open_panel(Panel::People(a.clone()), window, cx))).into_any_element());
        if is_admin(&c) {
            let (a, b) = (cid.to_string(), cid.to_string());
            out.push(crate::cards::chip("b-invite", "invite").on_click(cx.listener(move |this, _, window, cx| this.open_panel(Panel::Invite(a.clone()), window, cx))).into_any_element());
            out.push(crate::cards::chip("b-manage", "manage").on_click(cx.listener(move |this, _, window, cx| this.open_panel(Panel::Admin(b.clone()), window, cx))).into_any_element());
        }
        out
    }
    pub fn home_buttons(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            crate::cards::chip("b-join", "join").on_click(cx.listener(|this, _, window, cx| this.open_panel(Panel::Join, window, cx))).into_any_element(),
            crate::cards::chip("b-new", "new").on_click(cx.listener(|this, _, window, cx| this.open_panel(Panel::NewCommunity, window, cx))).into_any_element(),
        ]
    }
    /// The small notes on the home screen about communities this account asked to join or was turned away from.
    pub fn joining_notes(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(s) = self.snap.clone() else { return vec![] };
        let mut out: Vec<AnyElement> = vec![];
        for j in s.joining {
            let id = j.id.clone();
            out.push(h_flex().id(SharedString::from(format!("joining-{}", j.id))).gap_3().items_center().px_4().py_2().rounded(px(18.)).bg(rgb(look::SLIP)).border_1().border_color(look::line())
                .child(div().flex_1().font_family(look::F_NAME).text_size(px(13.)).child(format!("Asked to join \"{}\". An admin has to let you in; this can take a while if nobody is online.", j.name)))
                .child(pill(SharedString::from(format!("joining-x-{}", j.id)), false, "cancel").on_click(cx.listener(move |this, _, window, cx| this.cancel_join(id.clone(), window, cx)))).into_any_element());
        }
        for j in s.refused {
            let id = j.id.clone();
            out.push(h_flex().id(SharedString::from(format!("refused-{}", j.id))).gap_3().items_center().px_4().py_2().rounded(px(18.)).bg(rgb(look::SLIP)).border_1().border_color(rgb(look::DANGER))
                .child(div().flex_1().font_family(look::F_NAME).text_size(px(13.)).child(format!("Could not join \"{}\": {}.", j.name, j.reason)))
                .child(pill(SharedString::from(format!("refused-x-{}", j.id)), false, "dismiss").on_click(cx.listener(move |this, _, window, cx| this.cancel_join(id.clone(), window, cx)))).into_any_element());
        }
        out
    }

    /// For the test port: what the people panel and the communities show.
    pub fn members_dump(&self) -> Value {
        json!({"panel": format!("{:?}", self.mem.panel), "members": self.mem.members.iter().map(|m| json!({"name": m.name, "role": m.role, "me": m.me, "verified": m.verified})).collect::<Vec<_>>(),
               "say": self.mem.say.as_ref().map(|(t, b, _)| json!({"text": t, "bad": b})), "confirm": self.mem.confirm.as_ref().map(|c| c.title.clone()),
               "made": self.mem.made.as_ref().map(|m| json!({"tell": m.tell, "copy": m.copy})),
               "communities": self.snap.as_ref().map(|s| s.communities.iter().map(|c| json!({"id": c.id, "name": c.name, "role": c.role, "removed": c.removed, "members": c.members, "invites": c.invites.len(), "banned": c.banned.len()})).collect::<Vec<_>>())})
    }
}

fn chrono_until(ttl: u64) -> String {
    use chrono::TimeZone as _;
    let t = crate::ringui::unix() as i64 + ttl as i64;
    chrono::Local.timestamp_opt(t, 0).single().map(|t| t.format("%a %-d %b %H:%M").to_string()).unwrap_or_default()
}

/// An invite link given on the command line (`mega-gpui commons://join/…` or an https invite link), if one was.
pub fn link_from_args() -> Option<String> {
    std::env::args().skip(1).find(|a| is_link(a)).map(|a| a.trim().to_string())
}

impl Mega {
    /// Test port (uitest.rs `do`): what the buttons of the panels do, by names instead of clicks. Each goes through the same
    /// function the button's handler calls (`act`, `ask` + `run_confirm`, `invite_create`).
    pub fn ui_do(&mut self, cmd: &Value, window: &mut Window, cx: &mut Context<Self>) -> Value {
        let s = |k: &str| cmd[k].as_str().unwrap_or("").to_string();
        if s("what") == "new_community" { self.mem.new_name.update(cx, |i, cx| i.set_value(s("new"), window, cx)); self.new_community_submit(window, cx); return json!({"ok": true}); }
        let Some(c) = self.snap.as_ref().and_then(|sn| sn.communities.iter().find(|c| c.name == s("name"))).cloned() else { return json!({"error": "no such community"}) };
        let cid = c.id.clone();
        let who = s("who");
        let acct = self.mem.members.iter().find(|m| m.name == who).map(|m| m.account.clone()).or_else(|| c.banned.iter().find(|(_, n)| *n == who).map(|(a, _)| a.clone())).unwrap_or_default();
        let room = c.channels.iter().find(|r| r.name == s("room")).map(|r| r.gid.clone()).unwrap_or_default();
        match s("what").as_str() {
            "role" => { if acct.is_empty() { return json!({"error": "no such person"}); } self.act("set_role", json!({"cid": cid, "account": acct, "role": s("to")}), "ok".into(), window, cx); }
            "kick" => { if acct.is_empty() { return json!({"error": "no such person"}); } self.ask("Remove and ban?", "", "Remove and ban", Act::Kick { cid, account: acct }, cx); self.run_confirm(window, cx); }
            "unban" => { if acct.is_empty() { return json!({"error": "no such person"}); } self.ask("Unban?", "", "Unban", Act::Unban { cid, account: acct }, cx); self.run_confirm(window, cx); }
            "transfer" => { if acct.is_empty() { return json!({"error": "no such person"}); } self.ask("Hand over?", "", "Hand over", Act::Transfer { cid, account: acct }, cx); self.run_confirm(window, cx); }
            "leave" => { self.ask("Leave?", "", "Leave", Act::Leave { cid }, cx); self.run_confirm(window, cx); }
            "revoke" => { let Some(i) = c.invites.iter().find(|i| i.mine && i.id.starts_with(&s("invite"))) else { return json!({"error": "no such invite"}) }; self.ask("Revoke?", "", "Revoke", Act::Revoke { cid, id: i.id.clone() }, cx); self.run_confirm(window, cx); }
            "rename" => self.act("rename_community", json!({"cid": cid, "name": s("new")}), "ok".into(), window, cx),
            "room_add" => self.act("create_channel", json!({"cid": cid, "name": s("new"), "kind": if cmd["voice"].as_bool() == Some(true) { "voice" } else { "" }}), "ok".into(), window, cx),
            "room_rename" => self.act("rename_channel", json!({"cid": cid, "room": room, "name": s("new")}), "ok".into(), window, cx),
            "room_delete" => { self.ask("Delete?", "", "Delete", Act::DeleteRoom { cid, room }, cx); self.run_confirm(window, cx); }
            "colours" => { let i = cmd["preset"].as_i64().unwrap_or(-1); let duo = look::PRESETS.get(i as usize).map(|p| (p.1, p.2)); self.set_style_for(cid, room, duo, window, cx); }
            "wall_lock" => self.act("set_wall", json!({"cid": cid, "room": room, "lock": if cmd["on"].as_bool() == Some(false) { "0" } else { "1" }}), "ok".into(), window, cx),
            "wall_clear" => { self.ask("Clear?", "", "Clear", Act::ClearWall { cid, room }, cx); self.run_confirm(window, cx); }
            "invite" => {
                self.mem.panel = Some(Panel::Invite(cid.clone()));
                self.mem.invite_by_password = s("mode") != "link";
                self.mem.invite_ttl = cmd["ttl"].as_u64().unwrap_or(604800);
                self.mem.invite_uses = cmd["uses"].as_u64().map(|u| u as u32);
                let pw = s("password");
                if !pw.is_empty() { self.mem.invite_pw.update(cx, |i, cx| i.set_value(pw, window, cx)); self.mem.invite_hint = (String::new(), 1); }
                self.mem.made = None;
                self.invite_create(cid, window, cx);
            }
            "verify" => { self.open_safety(cid, acct, window, cx); }
            "tick" => {}
            _ => return json!({"error": "unknown action"}),
        }
        json!({"ok": true})
    }
}
