//! Notifications that work with the window closed.
//!
//! The background part (the browser version's `bridge`, or the desktop app's core) looks at the rooms itself and shows
//! an operating system notification for a new message when no mega window is being looked at. Nothing goes through any
//! push server: the text is read from the local history and handed to this computer's own notification service.
//!
//! Who shows what (`Hub::route`):
//!  * the page is open and being looked at (visible and focused): nobody. The rooms' own marks and the chime do it.
//!  * the page is open, hidden or unfocused, and has permission for the browser's Notification API: the page shows it
//!    (the bridge hands the note over in the answer to the page's heartbeat), so a click lands in the right tab.
//!  * no heartbeat from a page for `BEAT_FRESH`, or a page that cannot show notifications: the background part shows it
//!    through the `Sink` (native: freedesktop on Linux, Notification Center on macOS, a toast on Windows).
//! A notification is never shown by both.
//!
//! The rules (global mode, per-room mutes, privacy of the text) live here and are stored in `notify.json` in the data
//! folder, so the page and the background part always agree. The page reads and writes them through `notify_*` calls.
use crate::client::Client;
use crate::state::MsgBody;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A page that has not sent a heartbeat for this long is gone (closed, asleep, crashed): the background part takes over.
pub const BEAT_FRESH: Duration = Duration::from_secs(10);
/// One notification per room per this long; what arrives meanwhile is folded into the next one.
pub const COALESCE: Duration = Duration::from_secs(5);
/// A note waiting for the page to pick it up is shown natively instead after this long.
const PAGE_PICKUP: Duration = Duration::from_secs(8);
/// A click on a notification asks the next page that connects to open its room, if that happens within this long.
const OPEN_WINDOW: Duration = Duration::from_secs(90);
const TEXT_MAX: usize = 100;
const NAME_MAX: usize = 40;

// ---------------------------------------------------------------------------------------------------------- rules

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode { #[default] All, Mentions, Off }
impl Mode {
    pub fn parse(s: &str) -> Option<Mode> { match s { "all" => Some(Mode::All), "mentions" => Some(Mode::Mentions), "off" => Some(Mode::Off), _ => None } }
    pub fn word(self) -> &'static str { match self { Mode::All => "all", Mode::Mentions => "mentions", Mode::Off => "off" } }
}

/// What this device does with one room. A room with no rule follows the global mode.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Rule {
    /// Silent until the time (unix seconds), or until unmuted.
    Muted { #[serde(default)] until: Option<u64> },
    /// Only @mentions of me and replies to me.
    Mentions,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Default)]
pub struct Settings {
    #[serde(default)] pub mode: Mode,
    /// Off by default: the notification says who and where, not what.
    #[serde(default)] pub show_text: bool,
    #[serde(default)] pub rooms: BTreeMap<String, Rule>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Why { Plain, Mention, Reply }

impl Settings {
    /// The rule in force for a room now (a mute that has run out is not).
    pub fn rule(&self, gid: &str, now: u64) -> Option<Rule> {
        match self.rooms.get(gid) {
            Some(Rule::Muted { until: Some(t) }) if *t <= now => None,
            r => r.copied(),
        }
    }
    /// May a message reach this person at all (sound and notifications alike)?
    pub fn allows(&self, gid: &str, why: Why, now: u64) -> bool {
        let named = why != Why::Plain;
        match self.rule(gid, now) {
            Some(Rule::Muted { .. }) => return false,
            Some(Rule::Mentions) => return named,
            None => {}
        }
        match self.mode { Mode::All => true, Mode::Mentions => named, Mode::Off => false }
    }
    fn prune(&mut self, now: u64) -> bool {
        let n = self.rooms.len();
        self.rooms.retain(|_, r| !matches!(r, Rule::Muted { until: Some(t) } if *t <= now));
        n != self.rooms.len()
    }
    pub fn to_json(&self, now: u64) -> Value {
        let rooms: serde_json::Map<String, Value> = self.rooms.iter().filter(|(g, _)| self.rule(g, now).is_some()).map(|(g, r)| (g.clone(), serde_json::to_value(r).unwrap_or(Value::Null))).collect();
        json!({"mode": self.mode.word(), "show_text": self.show_text, "rooms": rooms, "now": now})
    }
}

// ---------------------------------------------------------------------------------------------------------- text

/// One line of plain text, at most `max` characters: control characters and line breaks become spaces.
pub fn clean(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max { return flat; }
    let mut cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    while cut.ends_with(' ') { cut.pop(); }
    cut.push('…');
    cut
}

/// Does the text name this person with an `@`? The page's rule (app.js `mentionsMe`): "@Name" or "@First" (the first word of
/// the name), any case, not glued to a letter, digit or underscore on either side.
pub fn mentions_name(text: &str, name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() { return false; }
    let hay: Vec<char> = text.to_lowercase().chars().collect();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut alts: Vec<Vec<char>> = vec![name.to_lowercase().chars().collect()];
    if let Some(first) = name.split_whitespace().next() { alts.push(first.to_lowercase().chars().collect()); }
    for (i, &c) in hay.iter().enumerate() {
        if c != '@' { continue; }
        if i > 0 && (word(hay[i - 1]) || hay[i - 1] == '@') { continue; }
        for a in &alts {
            if a.is_empty() || hay.len() < i + 1 + a.len() || hay[i + 1..i + 1 + a.len()] != a[..] { continue; }
            if hay.get(i + 1 + a.len()).is_some_and(|&n| word(n)) { continue; }
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------------------------------------- events

/// One new message, as far as a notification needs to know.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ev { pub gid: String, pub room: String, pub who: String, pub text: String, pub file: bool, pub why: Why }

/// What a notification says. `tag` is the room: a newer note of the same room replaces the older one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Note { pub gid: String, pub title: String, pub body: String }

/// "Ben", "Ben and 1 other", "Ben and 3 others".
fn who_line(evs: &[Ev]) -> (String, bool) {
    let mut names: Vec<&str> = vec![];
    for e in evs { if !names.contains(&e.who.as_str()) { names.push(&e.who); } }
    let line = match names.len() {
        0 => String::new(),
        1 => names[0].to_string(),
        n => format!("{} and {} other{}", names[0], n - 1, if n == 2 { "" } else { "s" }),
    };
    (line, names.len() <= 1)
}

/// The notification for a burst of messages in one room. Without `show_text` nothing of the messages is in it.
pub fn compose(evs: &[Ev], show_text: bool) -> Note {
    let first = &evs[0];
    let room = format!("#{}", clean(&first.room, NAME_MAX));
    let (who, one_sender) = who_line(evs);
    let n = evs.len();
    let named = evs.iter().find(|e| e.why == Why::Mention).map(|_| Why::Mention).or_else(|| evs.iter().find(|e| e.why == Why::Reply).map(|_| Why::Reply));
    let (title, body) = if show_text {
        let last = &evs[n - 1];
        let mut text = if last.file { if last.text.is_empty() { "sent a file".to_string() } else { format!("sent a file: {}", last.text) } } else { last.text.clone() };
        text = clean(&text, TEXT_MAX);
        if n > 1 { text = format!("{text} (+{} more)", n - 1); }
        let head = match named { Some(Why::Mention) => format!("{who} mentioned you in {room}"), Some(Why::Reply) => format!("{who} replied to you in {room}"), _ => format!("{who} in {room}") };
        (head, text)
    } else {
        let line = match named {
            Some(Why::Mention) => format!("{who} mentioned you in {room}"),
            Some(Why::Reply) => format!("{who} replied to you in {room}"),
            _ if n == 1 => format!("New message from {who} in {room}"),
            _ if one_sender => format!("{n} new messages from {who} in {room}"),
            _ => format!("New messages from {who} in {room}"),
        };
        ("mega".to_string(), line)
    };
    Note { gid: first.gid.clone(), title, body }
}

/// Folds bursts together: a room shows at most one note per `window`.
pub struct Coalescer { window: Duration, rooms: HashMap<String, (Option<Instant>, Vec<Ev>)> }
impl Coalescer {
    pub fn new(window: Duration) -> Self { Coalescer { window, rooms: HashMap::new() } }
    pub fn push(&mut self, ev: Ev) { self.rooms.entry(ev.gid.clone()).or_default().1.push(ev); }
    /// The notes that are due: rooms with something waiting whose last note is at least a window old.
    pub fn drain(&mut self, now: Instant, show_text: bool) -> Vec<Note> {
        let mut out = vec![];
        for (_, (last, pending)) in self.rooms.iter_mut() {
            if pending.is_empty() || last.is_some_and(|t| now.duration_since(t) < self.window) { continue; }
            out.push(compose(pending, show_text));
            pending.clear();
            *last = Some(now);
        }
        out.sort_by(|a, b| a.gid.cmp(&b.gid));
        self.rooms.retain(|_, (last, pending)| !pending.is_empty() || last.is_some_and(|t| now.duration_since(t) < self.window));
        out
    }
    pub fn clear(&mut self) { for (_, (_, p)) in self.rooms.iter_mut() { p.clear(); } }
}

// ---------------------------------------------------------------------------------------------------------- finding new messages

/// A room as the scanner sees it.
#[derive(Clone, Debug)]
pub struct RoomView { pub gid: String, pub name: String, pub unread: usize }
/// A message that could be notified about (not mine, not deleted, not a system line), oldest first.
#[derive(Clone, Debug)]
pub struct MsgView { pub who: String, pub text: String, pub file: bool, pub why: Why }

/// Remembers each room's unread count; a count that grew means new messages.
#[derive(Default)]
pub struct Scanner { prev: HashMap<String, usize> }
impl Scanner {
    /// `tail(gid)` gives the room's candidate messages, oldest first. The first look only learns the counts, so
    /// starting the program never announces what was already waiting.
    pub fn scan(&mut self, rooms: &[RoomView], tail: &mut dyn FnMut(&str) -> Vec<MsgView>) -> Vec<Ev> {
        let mut out = vec![];
        let mut seen = HashSet::new();
        for r in rooms {
            seen.insert(r.gid.clone());
            let was = self.prev.insert(r.gid.clone(), r.unread);
            let Some(was) = was else { continue };
            if r.unread <= was { continue; }
            let list = tail(&r.gid);
            let take = (r.unread - was).min(list.len());
            for m in &list[list.len() - take..] {
                out.push(Ev { gid: r.gid.clone(), room: r.name.clone(), who: clean(&m.who, NAME_MAX), text: m.text.clone(), file: m.file, why: m.why });
            }
        }
        self.prev.retain(|g, _| seen.contains(g));
        out
    }
}

/// The scanner's view of a real client. Rooms of one-to-one calls are left out (calls ring), and so are rooms this device
/// has no keys for. Nothing here can show text that was not decrypted: the history holds only what was.
pub fn scan_client(c: &Client, sc: &mut Scanner) -> Vec<Ev> {
    let me = c.my_account();
    let name = c.st.name.clone();
    let rooms: Vec<RoomView> = c.st.communities.values().filter(|co| !co.removed && !c.is_direct(co))
        .flat_map(|co| c.channels_of(&co.id)).filter(|ch| ch.joined && ch.kind != crate::envelope::ROOM_VOICE)
        .map(|ch| RoomView { gid: ch.gid, name: ch.name, unread: ch.unread }).collect();
    sc.scan(&rooms, &mut |gid| {
        let log = c.message_log(gid);
        let mine: HashSet<&str> = log.iter().filter(|m| m.sender == me).map(|m| m.id.as_str()).collect();
        log.iter().filter(|m| m.sender != me && !m.deleted && !matches!(m.body, MsgBody::Notice(_))).map(|m| {
            let (text, file) = match &m.body { MsgBody::Text(t) => (t.clone(), false), MsgBody::File { caption, name, .. } => (if caption.is_empty() { name.clone() } else { caption.clone() }, true), MsgBody::Notice(_) => (String::new(), false) };
            let why = if mentions_name(&text, &name) { Why::Mention } else if m.reply.as_deref().is_some_and(|r| mine.contains(r)) { Why::Reply } else { Why::Plain };
            MsgView { who: c.display_name(&m.sender), text, file, why }
        }).collect()
    })
}

// ---------------------------------------------------------------------------------------------------------- the hub

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Route { Silent, Page, Native }

#[derive(Clone, Copy)]
struct Beat { at: Instant, attended: bool, web: bool }

/// Where a note finally goes when the background part shows it.
pub trait Sink: Send + Sync {
    fn show(&self, note: &Note);
}

struct Inner {
    settings: Settings,
    beat: Option<Beat>,
    queue: Vec<(Instant, Note)>,
    co: Coalescer,
    open_room: Option<(String, Instant)>,
}

pub struct Hub {
    file: Option<PathBuf>,
    inner: Mutex<Inner>,
    sink: Mutex<Option<Arc<dyn Sink>>>,
    scan: Mutex<Scanner>,
}

fn unix() -> u64 { commons_proto::now_secs() }

impl Hub {
    /// `dir` is the data folder (`notify.json` lives in it); None keeps the rules in memory only.
    pub fn new(dir: Option<PathBuf>) -> Hub {
        let file = dir.map(|d| d.join("notify.json"));
        let settings = file.as_ref().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Hub { file, inner: Mutex::new(Inner { settings, beat: None, queue: vec![], co: Coalescer::new(COALESCE), open_room: None }), sink: Mutex::new(None), scan: Mutex::new(Scanner::default()) }
    }
    fn g(&self) -> std::sync::MutexGuard<'_, Inner> { self.inner.lock().unwrap_or_else(|p| p.into_inner()) }
    /// Installs the way notes are shown by this process. Without one the hub only keeps the rules.
    pub fn set_sink(&self, s: Arc<dyn Sink>) { *self.sink.lock().unwrap_or_else(|p| p.into_inner()) = Some(s); }
    pub fn has_sink(&self) -> bool { self.sink.lock().unwrap_or_else(|p| p.into_inner()).is_some() }

    fn save(&self, s: &Settings) {
        let Some(f) = &self.file else { return };
        let tmp = f.with_extension("json.tmp");
        if let Ok(b) = serde_json::to_vec_pretty(s) { if std::fs::write(&tmp, b).is_ok() { let _ = std::fs::rename(&tmp, f); } }
    }
    pub fn settings(&self) -> Settings { let mut g = self.g(); let t = unix(); if g.settings.prune(t) { let s = g.settings.clone(); drop(g); self.save(&s); return s; } g.settings.clone() }
    pub fn get(&self) -> Value { self.settings().to_json(unix()) }
    pub fn set_mode(&self, m: Mode) { let s = { let mut g = self.g(); g.settings.mode = m; g.settings.clone() }; self.save(&s); }
    pub fn set_show_text(&self, on: bool) { let s = { let mut g = self.g(); g.settings.show_text = on; g.settings.clone() }; self.save(&s); }
    /// `rule` None unmutes. A rule for a room is dropped again when it says what the global mode would.
    pub fn set_room(&self, gid: &str, rule: Option<Rule>) {
        let s = { let mut g = self.g(); match rule { Some(r) => { g.settings.rooms.insert(gid.to_string(), r); } None => { g.settings.rooms.remove(gid); } } g.settings.clone() };
        self.save(&s);
    }

    /// Who shows a notification now.
    pub fn route(&self, now: Instant) -> Route { Self::route_of(self.g().beat, now) }
    fn route_of(beat: Option<Beat>, now: Instant) -> Route {
        match beat {
            Some(b) if now.duration_since(b.at) < BEAT_FRESH => if b.attended { Route::Silent } else if b.web { Route::Page } else { Route::Native },
            _ => Route::Native,
        }
    }

    /// The page's heartbeat (every few seconds, from a timer that a hidden tab does not slow down). It says whether the
    /// page is being looked at and whether it can show notifications itself. The answer carries what the page should
    /// show now, and a room to open if a notification was clicked while no page was there.
    pub fn beat(&self, attended: bool, web: bool, now: Instant) -> Value {
        let mut g = self.g();
        g.beat = Some(Beat { at: now, attended, web });
        let notes: Vec<Note> = if attended || !web { vec![] } else { g.queue.drain(..).map(|(_, n)| n).collect() };
        if attended { g.queue.clear(); }
        let open = g.open_room.take().filter(|(_, t)| now.duration_since(*t) < OPEN_WINDOW).map(|(gid, _)| gid);
        json!({"notes": notes, "open": open})
    }
    /// A native notification was clicked. True when a window has to be started for it (no page is connected); the room
    /// is then opened by the page that comes up.
    pub fn clicked(&self, gid: &str, now: Instant) -> bool {
        let mut g = self.g();
        let live = g.beat.is_some_and(|b| now.duration_since(b.at) < BEAT_FRESH);
        if !live { g.open_room = Some((gid.to_string(), now)); }
        !live
    }

    /// New messages arrive; returns the notes the background part should show now.
    pub fn step(&self, evs: Vec<Ev>, now: Instant, unix_now: u64) -> Vec<Note> {
        let mut g = self.g();
        g.settings.prune(unix_now);
        let route = Self::route_of(g.beat, now);
        if route != Route::Silent {
            for e in evs {
                if g.settings.allows(&e.gid, e.why, unix_now) { g.co.push(e); }
            }
        }
        let show_text = g.settings.show_text;
        let due = g.co.drain(now, show_text);
        let mut native = vec![];
        match route {
            Route::Silent => { g.queue.clear(); g.co.clear(); }
            Route::Page => g.queue.extend(due.into_iter().map(|n| (now, n))),
            Route::Native => { native.extend(g.queue.drain(..).map(|(_, n)| n)); native.extend(due); }
        }
        // a note the page never picked up (it stopped asking) goes out natively
        if route == Route::Page {
            let stale = g.queue.iter().any(|(t, _)| now.duration_since(*t) >= PAGE_PICKUP);
            if stale { native.extend(g.queue.drain(..).map(|(_, n)| n)); }
        }
        native
    }

    /// One look at the client and whatever follows from it. Called about once a second by the core.
    pub fn tick(&self, c: &Client) {
        if !self.has_sink() { return; }
        let evs = scan_client(c, &mut self.scan.lock().unwrap_or_else(|p| p.into_inner()));
        self.deliver(evs);
    }
    /// The part of `tick` after the client has been looked at (so a test can feed it events).
    pub fn deliver(&self, evs: Vec<Ev>) {
        let notes = self.step(evs, Instant::now(), unix());
        if notes.is_empty() { return; }
        let sink = self.sink.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(s) = sink { for n in &notes { s.show(n); } }
    }

    /// The `notify_*` calls of the page.
    pub fn call(&self, method: &str, a: &Value) -> Result<Value, String> {
        let s = |k: &str| a.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        match method {
            "notify_get" => Ok(self.get()),
            "notify_set" => {
                if let Some(m) = a.get("mode").and_then(|x| x.as_str()) { self.set_mode(Mode::parse(m).ok_or("unknown mode")?); }
                if let Some(t) = a.get("show_text").and_then(|x| x.as_bool()) { self.set_show_text(t); }
                Ok(self.get())
            }
            "notify_room" => {
                let gid = s("gid");
                if gid.is_empty() || gid.len() > 128 { return Err("no room".into()); }
                let rule = match s("kind").as_str() {
                    "none" | "unmute" => None,
                    "mentions" => Some(Rule::Mentions),
                    "mute" => Some(Rule::Muted { until: a.get("secs").and_then(|x| x.as_u64()).filter(|s| *s > 0).map(|s| unix() + s.min(366 * 86400)) }),
                    _ => return Err("unknown rule".into()),
                };
                self.set_room(&gid, rule);
                Ok(self.get())
            }
            "notify_beat" => {
                let flag = |k: &str| a.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
                let mut v = self.beat(flag("attended"), flag("web"), Instant::now());
                v["settings"] = self.get();
                Ok(v)
            }
            _ => Err(format!("unknown call {method}")),
        }
    }
}

/// Writes each note as a line of JSON to a file. For checks that must see what would have been shown without showing it.
pub struct FileSink(PathBuf);
impl FileSink { pub fn new(path: PathBuf) -> FileSink { FileSink(path) } }
impl Sink for FileSink {
    fn show(&self, note: &Note) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.0) { let _ = writeln!(f, "{}", serde_json::to_string(note).unwrap_or_default()); }
    }
}

// ---------------------------------------------------------------------------------------------------------- the native sink

/// Shows notes through the operating system. `launcher` is the program that opens mega's window (a click on a note
/// starts it when no window is open); None makes a click do nothing more than dismiss the note.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub struct NativeSink { hub: std::sync::Weak<Hub>, launcher: Option<PathBuf>, app_id: String, waiting: Arc<std::sync::atomic::AtomicUsize> }

#[cfg(not(any(target_os = "ios", target_os = "android")))]
impl NativeSink {
    pub fn new(hub: &Arc<Hub>, launcher: Option<PathBuf>, app_id: &str) -> NativeSink {
        NativeSink { hub: Arc::downgrade(hub), launcher, app_id: app_id.to_string(), waiting: Arc::new(std::sync::atomic::AtomicUsize::new(0)) }
    }
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
impl Sink for NativeSink {
    fn show(&self, note: &Note) {
        let mut n = notify_rust::Notification::new();
        // The freedesktop spec lets a server read markup in the body; an `<` or `&` in a message must not be taken for it.
        let body = if cfg!(all(unix, not(target_os = "macos"))) { note.body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;") } else { note.body.clone() };
        n.summary(&note.title).body(&body).appname("mega").timeout(notify_rust::Timeout::Milliseconds(8000));
        #[cfg(all(unix, not(target_os = "macos")))]
        { n.icon("mega").hint(notify_rust::Hint::Category("im.received".into())).action("default", "Open"); }
        // macOS: a click activates the app whose bundle id is set here (the launcher's bundle), which opens the window.
        #[cfg(target_os = "macos")]
        { let _ = notify_rust::set_application(&self.app_id); }
        let (hub, launcher, gid, waiting) = (self.hub.clone(), self.launcher.clone(), note.gid.clone(), self.waiting.clone());
        // Showing can wait on the notification service; never hold up the core's loop for it.
        std::thread::spawn(move || {
            let shown = n.show();
            // A click while no window is open starts one; the page that comes up is told which room to show (Hub::clicked).
            // macOS has no run loop here to wait on, and its click already activates the app.
            #[cfg(any(all(unix, not(target_os = "macos")), windows))]
            if let Ok(h) = shown {
                use std::sync::atomic::Ordering;
                if waiting.fetch_add(1, Ordering::SeqCst) < 8 {
                    let click = move || {
                        let start = hub.upgrade().is_some_and(|h| h.clicked(&gid, Instant::now()));
                        if let (true, Some(l)) = (start, &launcher) { let _ = std::process::Command::new(l).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn(); }
                    };
                    #[cfg(not(windows))]
                    h.wait_for_action(|action| if action == "default" { click() });
                    #[cfg(windows)]
                    { let _ = h.wait_for_response(|r: &notify_rust::NotificationResponse| if r.is_default_action() { click() }); }
                }
                waiting.fetch_sub(1, Ordering::SeqCst);
            }
            #[cfg(not(any(all(unix, not(target_os = "macos")), windows)))]
            { let _ = (shown, hub, launcher, gid, waiting); }
        });
    }
}

// ---------------------------------------------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(gid: &str, who: &str, text: &str, why: Why) -> Ev { Ev { gid: gid.into(), room: "music".into(), who: who.into(), text: text.into(), file: false, why } }
    const T0: u64 = 1_000_000;

    #[test]
    fn global_modes_and_room_rules() {
        let mut s = Settings::default();
        for (why, want) in [(Why::Plain, true), (Why::Mention, true), (Why::Reply, true)] { assert_eq!(s.allows("r", why, T0), want); }
        s.mode = Mode::Mentions;
        assert!(!s.allows("r", Why::Plain, T0) && s.allows("r", Why::Mention, T0) && s.allows("r", Why::Reply, T0));
        s.mode = Mode::Off;
        assert!(!s.allows("r", Why::Plain, T0) && !s.allows("r", Why::Mention, T0));
        // a room's own rule beats the global mode in both directions, except that a mute is total
        s.mode = Mode::All;
        s.rooms.insert("r".into(), Rule::Mentions);
        assert!(!s.allows("r", Why::Plain, T0) && s.allows("r", Why::Mention, T0) && s.allows("other", Why::Plain, T0));
        s.rooms.insert("r".into(), Rule::Muted { until: None });
        assert!(!s.allows("r", Why::Mention, T0) && !s.allows("r", Why::Reply, T0));
        s.mode = Mode::Mentions;
        s.rooms.insert("r".into(), Rule::Mentions);
        assert!(!s.allows("r", Why::Plain, T0) && s.allows("r", Why::Reply, T0));
        // a timed mute ends
        s.mode = Mode::All;
        s.rooms.insert("r".into(), Rule::Muted { until: Some(T0 + 3600) });
        assert!(!s.allows("r", Why::Plain, T0 + 3599));
        assert!(s.allows("r", Why::Plain, T0 + 3600));
        assert!(s.rule("r", T0 + 3600).is_none());
        assert!(s.prune(T0 + 3600) && s.rooms.is_empty());
    }

    #[test]
    fn mentions_follow_the_pages_rule() {
        assert!(mentions_name("hey @Ben look", "Ben Carter"));
        assert!(mentions_name("hey @ben carter look", "Ben Carter"));
        assert!(mentions_name("@Ben", "Ben Carter"));
        assert!(mentions_name("(@BEN)", "ben"));
        assert!(!mentions_name("hey Ben", "Ben"));
        assert!(!mentions_name("mail me@Ben.org", "Ben"));
        assert!(!mentions_name("hey @Benjamin", "Ben"));
        assert!(!mentions_name("hey @@Ben", "Ben"));
        assert!(!mentions_name("hey @Ben_x", "Ben"));
        assert!(mentions_name("hey @Ben, hi", "Ben"));
        assert!(!mentions_name("@x", ""));
    }

    #[test]
    fn text_is_private_by_default_and_short_when_shown() {
        let secret = "the launch code is 1234";
        let off = compose(&[ev("g", "Ben", secret, Why::Plain)], false);
        assert_eq!(off.body, "New message from Ben in #music");
        assert!(!off.title.contains("1234") && !off.body.contains("launch"));
        let on = compose(&[ev("g", "Ben", secret, Why::Plain)], true);
        assert_eq!((on.title.as_str(), on.body.as_str()), ("Ben in #music", secret));
        let long = "x".repeat(300);
        let c = compose(&[ev("g", "Ben", &long, Why::Plain)], true);
        assert_eq!(c.body.chars().count(), 100);
        assert!(c.body.ends_with('…'));
        let nl = compose(&[ev("g", "Ben", "one\ntwo\u{7}\tthree", Why::Plain)], true);
        assert_eq!(nl.body, "one two three");
        // a mention names no text when text is off
        let m = compose(&[ev("g", "Ben", secret, Why::Mention)], false);
        assert_eq!(m.body, "Ben mentioned you in #music");
        let r = compose(&[ev("g", "Ben", secret, Why::Reply)], false);
        assert_eq!(r.body, "Ben replied to you in #music");
    }

    #[test]
    fn bursts_read_naturally() {
        let one = vec![ev("g", "Ben", "a", Why::Plain), ev("g", "Ben", "b", Why::Plain), ev("g", "Ben", "c", Why::Plain)];
        assert_eq!(compose(&one, false).body, "3 new messages from Ben in #music");
        let many = vec![ev("g", "Ben", "a", Why::Plain), ev("g", "Ann", "b", Why::Plain), ev("g", "Cy", "c", Why::Plain), ev("g", "Di", "d", Why::Plain)];
        assert_eq!(compose(&many, false).body, "New messages from Ben and 3 others in #music");
        let two = vec![ev("g", "Ben", "a", Why::Plain), ev("g", "Ann", "b", Why::Plain)];
        assert_eq!(compose(&two, false).body, "New messages from Ben and 1 other in #music");
        let with_text = compose(&many, true);
        assert_eq!((with_text.title.as_str(), with_text.body.as_str()), ("Ben and 3 others in #music", "d (+3 more)"));
        // a mention anywhere in the burst says so
        let mixed = vec![ev("g", "Ben", "a", Why::Plain), ev("g", "Ann", "hi @me", Why::Mention)];
        assert_eq!(compose(&mixed, false).body, "Ben and 1 other mentioned you in #music");
    }

    #[test]
    fn coalescing_one_note_per_room_per_window() {
        let mut c = Coalescer::new(Duration::from_secs(5));
        let t = Instant::now();
        c.push(ev("a", "Ben", "1", Why::Plain));
        c.push(ev("b", "Ann", "1", Why::Plain));
        let first = c.drain(t, false);
        assert_eq!(first.len(), 2, "each room shows at once the first time");
        // a burst inside the window is held, then comes out as one
        for i in 0..6 { c.push(ev("a", if i % 2 == 0 { "Ben" } else { "Cy" }, "x", Why::Plain)); }
        assert!(c.drain(t + Duration::from_secs(2), false).is_empty());
        assert!(c.drain(t + Duration::from_secs(4), false).is_empty());
        let later = c.drain(t + Duration::from_secs(5), false);
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].body, "New messages from Ben and 1 other in #music");
        assert!(c.drain(t + Duration::from_secs(6), false).is_empty(), "nothing left over");
        c.push(ev("a", "Ben", "y", Why::Plain));
        assert!(c.drain(t + Duration::from_secs(7), false).is_empty(), "still inside the window of the last note");
        assert_eq!(c.drain(t + Duration::from_secs(10), false).len(), 1);
    }

    fn rooms(list: &[(&str, usize)]) -> Vec<RoomView> { list.iter().map(|(g, u)| RoomView { gid: g.to_string(), name: "music".into(), unread: *u }).collect() }
    fn msgs(list: &[(&str, &str)]) -> Vec<MsgView> { list.iter().map(|(w, t)| MsgView { who: w.to_string(), text: t.to_string(), file: false, why: Why::Plain }).collect() }

    #[test]
    fn scanner_reports_only_what_grew_and_not_at_start() {
        let mut sc = Scanner::default();
        let log = msgs(&[("A", "old1"), ("B", "old2"), ("C", "new1"), ("D", "new2")]);
        let mut calls = 0;
        // start-up: 2 were waiting, nothing is announced
        assert!(sc.scan(&rooms(&[("g", 2)]), &mut |_| { calls += 1; log.clone() }).is_empty());
        assert_eq!(calls, 0);
        // two arrive: exactly those two, in order
        let evs = sc.scan(&rooms(&[("g", 4)]), &mut |_| log.clone());
        assert_eq!(evs.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), ["new1", "new2"]);
        // nothing changed, then read (count drops), then one more
        assert!(sc.scan(&rooms(&[("g", 4)]), &mut |_| log.clone()).is_empty());
        assert!(sc.scan(&rooms(&[("g", 0)]), &mut |_| log.clone()).is_empty());
        let evs = sc.scan(&rooms(&[("g", 1)]), &mut |_| log.clone());
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].text, "new2");
        // a room that appears later is learned silently
        assert!(sc.scan(&rooms(&[("g", 1), ("h", 5)]), &mut |_| log.clone()).is_empty());
    }

    fn hub() -> Hub { Hub::new(None) }
    fn texts(v: &[Note]) -> Vec<String> { v.iter().map(|n| n.body.clone()).collect() }

    #[test]
    fn heartbeat_hands_over_between_page_and_background() {
        let h = hub();
        let t = Instant::now();
        assert_eq!(h.route(t), Route::Native, "no page ever: the background part");
        h.beat(true, true, t);
        assert_eq!(h.route(t + Duration::from_secs(1)), Route::Silent, "looked at: nobody");
        h.beat(false, true, t);
        assert_eq!(h.route(t + Duration::from_secs(9)), Route::Page, "hidden with permission: the page");
        assert_eq!(h.route(t + Duration::from_secs(10)), Route::Native, "ten quiet seconds: the background part takes over");
        h.beat(false, false, t + Duration::from_secs(20));
        assert_eq!(h.route(t + Duration::from_secs(21)), Route::Native, "a page that cannot show notifications");
    }

    #[test]
    fn never_both() {
        let h = hub();
        let t = Instant::now();
        // hidden page with permission: the note is queued for the page and the background part shows nothing
        h.beat(false, true, t);
        let native = h.step(vec![ev("g", "Ben", "hi", Why::Plain)], t + Duration::from_secs(1), T0);
        assert!(native.is_empty());
        let got = h.beat(false, true, t + Duration::from_secs(2));
        assert_eq!(got["notes"].as_array().unwrap().len(), 1);
        assert_eq!(got["notes"][0]["body"], "New message from Ben in #music");
        assert_eq!(h.beat(false, true, t + Duration::from_secs(3))["notes"].as_array().unwrap().len(), 0, "handed over once");
        // the page vanishes with a note still queued: it goes out natively, once
        let t2 = t + Duration::from_secs(10);
        h.beat(false, true, t2);
        assert!(h.step(vec![ev("h", "Ann", "hi", Why::Plain)], t2 + Duration::from_secs(1), T0).is_empty());
        let out = h.step(vec![], t2 + Duration::from_secs(12), T0);
        assert_eq!(texts(&out), ["New message from Ann in #music"]);
        assert!(h.step(vec![], t2 + Duration::from_secs(13), T0).is_empty());
        // a page being looked at: silent, and what was waiting is dropped
        h.beat(true, true, t2 + Duration::from_secs(14));
        assert!(h.step(vec![ev("g", "Ben", "yo", Why::Plain)], t2 + Duration::from_secs(15), T0).is_empty());
        assert!(h.beat(false, true, t2 + Duration::from_secs(16))["notes"].as_array().unwrap().is_empty());
        // no page at all: native
        let t3 = t2 + Duration::from_secs(60);
        assert_eq!(texts(&h.step(vec![ev("g", "Ben", "yo", Why::Plain)], t3, T0)), ["New message from Ben in #music"]);
    }

    #[test]
    fn mutes_apply_before_anything_is_shown() {
        let h = hub();
        let t = Instant::now();
        h.set_room("quiet", Some(Rule::Muted { until: None }));
        h.set_room("at", Some(Rule::Mentions));
        let evs = vec![ev("quiet", "Ben", "a", Why::Mention), ev("at", "Ben", "b", Why::Plain), ev("at", "Ann", "c", Why::Mention), ev("loud", "Cy", "d", Why::Plain)];
        let out = h.step(evs, t, T0);
        assert_eq!(out.iter().map(|n| n.gid.as_str()).collect::<Vec<_>>(), ["at", "loud"]);
        assert_eq!(out[0].body, "Ann mentioned you in #music");
        h.set_mode(Mode::Off);
        assert!(h.step(vec![ev("loud", "Cy", "e", Why::Mention)], t + Duration::from_secs(60), T0).is_empty());
        h.set_mode(Mode::Mentions);
        assert!(h.step(vec![ev("loud", "Cy", "f", Why::Plain)], t + Duration::from_secs(120), T0).is_empty());
        assert_eq!(h.step(vec![ev("loud", "Cy", "g", Why::Reply)], t + Duration::from_secs(180), T0).len(), 1);
    }

    #[test]
    fn text_setting_reaches_the_note() {
        let h = hub();
        let t = Instant::now();
        assert_eq!(h.step(vec![ev("g", "Ben", "secret plan", Why::Plain)], t, T0)[0].body, "New message from Ben in #music");
        h.set_show_text(true);
        assert_eq!(h.step(vec![ev("g", "Ben", "secret plan", Why::Plain)], t + Duration::from_secs(10), T0)[0].body, "secret plan");
    }

    #[test]
    fn click_asks_the_next_page_for_the_room() {
        let h = hub();
        let t = Instant::now();
        assert!(h.clicked("g", t), "nobody is there: start a window");
        let first = h.beat(true, true, t + Duration::from_secs(3));
        assert_eq!(first["open"], "g");
        assert!(h.beat(true, true, t + Duration::from_secs(6))["open"].is_null(), "once");
        assert!(!h.clicked("g", t + Duration::from_secs(7)), "a page is there: nothing to start");
        assert!(h.clicked("h", t + Duration::from_secs(60)));
        assert!(h.beat(true, true, t + Duration::from_secs(60 + 100))["open"].is_null(), "too late is forgotten");
    }

    #[test]
    fn rules_are_kept_on_disk_and_calls_work() {
        let d = tempfile::tempdir().unwrap();
        let h = Hub::new(Some(d.path().to_path_buf()));
        h.call("notify_set", &json!({"mode": "mentions", "show_text": true})).unwrap();
        h.call("notify_room", &json!({"gid": "r1", "kind": "mute", "secs": 3600})).unwrap();
        h.call("notify_room", &json!({"gid": "r2", "kind": "mute"})).unwrap();
        h.call("notify_room", &json!({"gid": "r3", "kind": "mentions"})).unwrap();
        h.call("notify_room", &json!({"gid": "r4", "kind": "mute"})).unwrap();
        let v = h.call("notify_room", &json!({"gid": "r4", "kind": "unmute"})).unwrap();
        assert_eq!(v["mode"], "mentions");
        assert_eq!(v["show_text"], true);
        assert_eq!(v["rooms"]["r2"]["kind"], "muted");
        assert!(v["rooms"]["r2"]["until"].is_null());
        assert!(v["rooms"]["r1"]["until"].as_u64().unwrap() > unix());
        assert_eq!(v["rooms"]["r3"]["kind"], "mentions");
        assert!(v["rooms"].get("r4").is_none());
        // another process (the bridge after a restart) reads the same
        let again = Hub::new(Some(d.path().to_path_buf()));
        assert_eq!(again.settings(), h.settings());
        assert!(h.call("notify_set", &json!({"mode": "loud"})).is_err());
        assert!(h.call("notify_room", &json!({"gid": "x", "kind": "weird"})).is_err());
        assert!(h.call("notify_room", &json!({"kind": "mute"})).is_err());
        // a broken file is ignored, not fatal
        std::fs::write(d.path().join("notify.json"), b"{nope").unwrap();
        assert_eq!(Hub::new(Some(d.path().to_path_buf())).settings(), Settings::default());
    }

    struct Fake(Mutex<Vec<Note>>);
    impl Sink for Fake { fn show(&self, n: &Note) { self.0.lock().unwrap().push(n.clone()); } }

    #[test]
    fn deliver_goes_to_the_sink() {
        let h = hub();
        let f = Arc::new(Fake(Mutex::new(vec![])));
        h.set_sink(f.clone());
        h.deliver(vec![ev("g", "Ben", "hi", Why::Plain)]);
        assert_eq!(f.0.lock().unwrap().len(), 1);
    }
}
