//! System-wide push-to-talk, mute and deafen. On Linux this talks to the desktop portal's
//! `org.freedesktop.portal.GlobalShortcuts` interface (KDE Plasma and GNOME implement it on Wayland and X11): the app
//! registers three shortcuts, the person approves the keys once in the desktop's own dialog, and the portal then sends
//! `Activated`/`Deactivated` for them whatever window has the focus. Nothing secret is stored: the portal remembers
//! the approved keys, and the app asks for them again on every start (the same ids give the same keys).
//!
//! Without the portal (no session bus, no such interface, or the person said no) the engine simply keeps what it had:
//! the keys work while mega's window is focused, and `Snapshot.global_shortcuts` says why they are not global.
//! macOS and Windows have a stub that reports `unsupported`.
//!
//! Two halves: [`VoiceState`] is the pure state machine (no I/O, unit tested), and the portal glue below it.

/// The three shortcuts. The ids are what the portal stores the person's keys under, so they never change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut { PushToTalk, ToggleMute, ToggleDeafen }
impl Shortcut {
    pub const ALL: [Shortcut; 3] = [Shortcut::PushToTalk, Shortcut::ToggleMute, Shortcut::ToggleDeafen];
    pub fn id(self) -> &'static str {
        match self { Shortcut::PushToTalk => "push-to-talk", Shortcut::ToggleMute => "toggle-mute", Shortcut::ToggleDeafen => "toggle-deafen" }
    }
    pub fn description(self) -> &'static str {
        match self { Shortcut::PushToTalk => "Push to talk (hold)", Shortcut::ToggleMute => "Toggle mute", Shortcut::ToggleDeafen => "Toggle deafen" }
    }
    pub fn from_id(id: &str) -> Option<Shortcut> { Shortcut::ALL.into_iter().find(|s| s.id() == id) }
}

// ------------------------------------------------------------------------------------------------ the state machine

/// What the three shortcuts and the in-window controls act on. The engine keeps these four flags; every change to them
/// goes through [`VoiceState::key`] / [`VoiceState::set_deafened`], so a global key and a click can never disagree.
///
/// Rules:
/// - Push to talk (press) opens the mic only while push-to-talk mode is on and the call is not deafened; release closes
///   it again, always (a release is never lost, even if you were deafened in between).
/// - Toggle mute flips `muted` (both ways, on key press only). Unmuting while deafened also undeafens: you cannot be
///   deafened and listening to yourself talk. While push-to-talk is held it flips like anywhere else; the release then
///   closes the mic.
/// - Toggle deafen flips `deafened`; deafening mutes, undeafening leaves the mic as it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VoiceState { pub ptt_mode: bool, pub muted: bool, pub deafened: bool, pub ptt_held: bool }

impl VoiceState {
    /// A shortcut went down (`down`) or up. Returns the new state; the caller applies the difference.
    pub fn key(mut self, s: Shortcut, down: bool) -> VoiceState {
        match s {
            Shortcut::PushToTalk => {
                if down {
                    if self.ptt_mode && !self.deafened { self.ptt_held = true; self.muted = false; }
                } else {
                    // the key is up: the mic closes (push-to-talk mode), and a release is never lost
                    self.ptt_held = false;
                    if self.ptt_mode { self.muted = true; }
                }
            }
            Shortcut::ToggleMute => if down { self = self.set_muted(!self.muted); },
            Shortcut::ToggleDeafen => if down { self = self.set_deafened(!self.deafened); },
        }
        self
    }
    /// Mute or unmute by choice. Unmuting a deafened call undeafens it.
    pub fn set_muted(mut self, m: bool) -> VoiceState {
        self.muted = m;
        if !m && self.deafened { self.deafened = false; }
        self
    }
    pub fn set_deafened(mut self, d: bool) -> VoiceState {
        self.deafened = d;
        if d { self.muted = true; }
        self
    }
}

// ------------------------------------------------------------------------------------------------ what the engine sees

/// Why the shortcuts are or are not global, as `Snapshot.global_shortcuts` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Status {
    /// Not asked for (tests, `MEGA_GPUI_SHORTCUTS=off`): focused-window keys only.
    #[default] Off,
    /// Talking to the portal; the desktop's approval dialog may be open.
    Pending,
    /// The portal accepted the shortcuts: they work with any window focused.
    Active,
    /// The person declined or closed the dialog; ask again with `Command::RebindShortcuts`.
    Denied,
    /// No session bus, or no GlobalShortcuts portal: focused-window keys only.
    Unavailable,
    /// This platform has no global shortcuts support yet: focused-window keys only.
    Unsupported,
}
impl Status {
    pub fn as_str(self) -> &'static str {
        match self { Status::Off => "off", Status::Pending => "pending", Status::Active => "active", Status::Denied => "denied", Status::Unavailable => "unavailable", Status::Unsupported => "unsupported" }
    }
}

/// One shortcut as the portal knows it, for the UI to show ("Push to talk (hold): Ctrl+Alt+V").
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct ShortcutInfo { pub id: String, pub description: String, pub trigger: String }

/// What the portal task tells the engine.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Status { status: Status, note: String },
    Triggers(Vec<ShortcutInfo>),
    Key { shortcut: Shortcut, down: bool },
}

/// Where the engine should look for the portal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Not at all.
    Off,
    /// The user's session bus (`DBUS_SESSION_BUS_ADDRESS`).
    Session,
    /// A given bus address: tests run against a private `dbus-daemon` with a mock portal.
    Bus(String),
}
impl Mode {
    /// `MEGA_GPUI_SHORTCUTS=off` turns it off; `bus:<address>` picks an address; anything else is the session bus.
    pub fn from_env() -> Mode {
        match std::env::var("MEGA_GPUI_SHORTCUTS").ok().as_deref() {
            Some("off") | Some("0") => Mode::Off,
            Some(s) if s.starts_with("bus:") => Mode::Bus(s[4..].to_string()),
            _ => Mode::Session,
        }
    }
}

/// Messages to the running portal task.
#[derive(Debug)]
pub enum Ctl { Rebind }

pub type Sink = std::sync::Arc<dyn Fn(Event) + Send + Sync>;

/// Starts the portal task on `rt` and returns the way to talk to it (`None` when `mode` is off or unsupported; the
/// status has then been reported to `sink`).
pub fn spawn(rt: &tokio::runtime::Handle, mode: Mode, app_id: &str, sink: Sink) -> Option<tokio::sync::mpsc::UnboundedSender<Ctl>> {
    if mode == Mode::Off { sink(Event::Status { status: Status::Off, note: String::new() }); return None; }
    #[cfg(target_os = "linux")]
    {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let app_id = app_id.to_string();
        rt.spawn(portal::run(mode, app_id, sink, rx));
        Some(tx)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (rt, app_id);
        sink(Event::Status { status: Status::Unsupported, note: "global shortcuts are not available on this platform yet".into() });
        None
    }
}

// ------------------------------------------------------------------------------------------------ the portal

#[cfg(target_os = "linux")]
pub(crate) mod portal {
    use super::*;
    use futures::StreamExt;
    use serde::Serialize;
    use std::collections::HashMap;
    use std::time::Duration;
    use zbus::zvariant::{DynamicType, ObjectPath, OwnedObjectPath, OwnedValue, Value};
    use zbus::{proxy::CacheProperties, Connection, Proxy};

    pub const DEST: &str = "org.freedesktop.portal.Desktop";
    pub const PATH: &str = "/org/freedesktop/portal/desktop";
    pub const IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
    pub const REQUEST: &str = "org.freedesktop.portal.Request";
    /// Asking for the interface or for a session must be quick; only the bind may wait for a person (the approval dialog).
    const QUICK: Duration = Duration::from_secs(8);

    pub type Dict = HashMap<String, OwnedValue>;

    fn say(sink: &Sink, status: Status, note: impl Into<String>) { sink(Event::Status { status, note: note.into() }); }

    /// The keys offered to the person in the portal's dialog. They are suggestions; the person decides.
    fn preferred(s: Shortcut) -> &'static str {
        match s { Shortcut::PushToTalk => "CTRL+ALT+V", Shortcut::ToggleMute => "CTRL+ALT+M", Shortcut::ToggleDeafen => "CTRL+ALT+N" }
    }

    enum Ended { Gone(String), Closed }

    pub async fn run(mode: Mode, app_id: String, sink: Sink, mut ctl: tokio::sync::mpsc::UnboundedReceiver<Ctl>) {
        loop {
            say(&sink, Status::Pending, "");
            match session(&mode, &app_id, &sink, &mut ctl).await {
                Ended::Closed => return,
                Ended::Gone(why) => say(&sink, Status::Unavailable, why),
            }
            // fell back to the window's own keys; try again when the person asks
            match ctl.recv().await { Some(Ctl::Rebind) => {} None => return }
        }
    }

    async fn connect(mode: &Mode) -> Result<Connection, String> {
        match mode {
            Mode::Off => Err("off".into()),
            Mode::Session => Connection::session().await.map_err(|e| format!("no session bus: {e}")),
            Mode::Bus(addr) => zbus::connection::Builder::address(addr.as_str()).map_err(|e| format!("bad bus address: {e}"))?.build().await.map_err(|e| format!("no bus at that address: {e}")),
        }
    }

    /// Unsandboxed apps tell the portal who they are (xdg-desktop-portal 1.19+); the portal keys the person's approved
    /// shortcuts on it. Best effort: older portals do not have the call, and then it is derived from the process.
    async fn register(conn: &Connection, app_id: &str) {
        let opts: HashMap<&str, Value<'_>> = HashMap::new();
        let _ = tokio::time::timeout(QUICK, conn.call_method(Some(DEST), PATH, Some("org.freedesktop.host.portal.Registry"), "Register", &(app_id, opts))).await;
    }

    async fn portal_proxy(conn: &Connection) -> Result<Proxy<'static>, String> {
        zbus::proxy::Builder::new(conn).destination(DEST).and_then(|b| b.path(PATH)).and_then(|b| b.interface(IFACE)).map_err(|e| e.to_string())?
            .cache_properties(CacheProperties::No).build().await.map_err(|e| e.to_string())
    }

    fn token(what: &str) -> String { format!("mega_{what}_{}", std::process::id()) }

    /// One portal request: the method answers with a request handle, and the real answer comes as a `Response` signal
    /// on it. The signal is subscribed to before the call (the handle is predictable from our name and the token), so
    /// a quick answer cannot be missed. `wait` is `None` for the bind, which waits as long as the dialog is open.
    async fn request<B>(conn: &Connection, proxy: &Proxy<'_>, method: &str, tok: &str, body: &B, wait: Option<Duration>) -> Result<(u32, Dict), String>
    where B: Serialize + DynamicType {
        let sender = conn.unique_name().map(|n| n.as_str().trim_start_matches(':').replace('.', "_")).unwrap_or_default();
        let handle = format!("/org/freedesktop/portal/desktop/request/{sender}/{tok}");
        let rp = Proxy::new(conn, DEST, handle, REQUEST).await.map_err(|e| e.to_string())?;
        let mut resp = rp.receive_signal("Response").await.map_err(|e| e.to_string())?;
        let reply = tokio::time::timeout(QUICK, proxy.call_method(method, body)).await.map_err(|_| format!("{method}: the portal did not answer"))?.map_err(|e| format!("{method}: {e}"))?;
        // a portal that picked another handle than the predicted one: listen there instead
        if let Ok(actual) = reply.body().deserialize::<OwnedObjectPath>() {
            if actual.as_str() != rp.path().as_str() {
                let rp2 = Proxy::new(conn, DEST, actual, REQUEST).await.map_err(|e| e.to_string())?;
                resp = rp2.receive_signal("Response").await.map_err(|e| e.to_string())?;
            }
        }
        let next = async { resp.next().await.ok_or_else(|| format!("{method}: the portal went away")) };
        let msg = match wait { Some(d) => tokio::time::timeout(d, next).await.map_err(|_| format!("{method}: no response"))??, None => next.await? };
        msg.body().deserialize::<(u32, Dict)>().map_err(|e| format!("{method}: odd response: {e}"))
    }

    fn as_string(v: &OwnedValue) -> Option<String> {
        if let Ok(s) = <&str>::try_from(&**v) { return Some(s.to_string()); }
        ObjectPath::try_from(&**v).ok().map(|p| p.to_string())
    }

    /// `a(sa{sv})` from a response or `ShortcutsChanged`: the shortcuts the portal holds for us, with their keys.
    fn infos(list: Vec<(String, Dict)>) -> Vec<ShortcutInfo> {
        let mut out: Vec<ShortcutInfo> = Shortcut::ALL.iter().map(|s| ShortcutInfo { id: s.id().into(), description: s.description().into(), trigger: String::new() }).collect();
        for (id, d) in list {
            if let Some(i) = out.iter_mut().find(|i| i.id == id) {
                if let Some(t) = d.get("trigger_description").and_then(as_string) { i.trigger = t; }
            }
        }
        out
    }
    fn shortcut_list(v: &OwnedValue) -> Option<Vec<(String, Dict)>> { v.try_clone().ok().and_then(|v| Vec::<(String, Dict)>::try_from(v).ok()) }

    async fn bind(conn: &Connection, proxy: &Proxy<'_>, session: &str, sink: &Sink) -> Result<(), String> {
        let Ok(sess) = ObjectPath::try_from(session) else { return Err("odd session handle".into()) };
        let list: Vec<(&str, HashMap<&str, Value<'_>>)> = Shortcut::ALL.iter().map(|s| {
            let mut d: HashMap<&str, Value<'_>> = HashMap::new();
            d.insert("description", Value::from(s.description()));
            d.insert("preferred_trigger", Value::from(preferred(*s)));
            (s.id(), d)
        }).collect();
        let mut opts: HashMap<&str, Value<'_>> = HashMap::new();
        opts.insert("handle_token", Value::from(token("bind")));
        say(sink, Status::Pending, "waiting for the desktop to approve the shortcuts");
        let (code, results) = request(conn, proxy, "BindShortcuts", &token("bind"), &(sess, list, "", opts), None).await?;
        match code {
            0 => {
                if let Some(l) = results.get("shortcuts").and_then(shortcut_list) { sink(Event::Triggers(infos(l))); }
                say(sink, Status::Active, "");
            }
            1 => say(sink, Status::Denied, "the shortcuts were not approved"),
            _ => say(sink, Status::Denied, "the desktop could not set the shortcuts up"),
        }
        Ok(())
    }

    async fn session(mode: &Mode, app_id: &str, sink: &Sink, ctl: &mut tokio::sync::mpsc::UnboundedReceiver<Ctl>) -> Ended {
        let conn = match connect(mode).await { Ok(c) => c, Err(e) => return Ended::Gone(e) };
        register(&conn, app_id).await;
        let proxy = match portal_proxy(&conn).await { Ok(p) => p, Err(e) => return Ended::Gone(e) };
        let version = match tokio::time::timeout(QUICK, proxy.get_property::<u32>("version")).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return Ended::Gone(format!("the desktop has no GlobalShortcuts portal ({e})")),
            Err(_) => return Ended::Gone("the desktop portal did not answer".into()),
        };
        // the signals first, so nothing the portal sends right after the bind is lost
        let (mut act, mut deact, mut changed) = match (proxy.receive_signal("Activated").await, proxy.receive_signal("Deactivated").await, proxy.receive_signal("ShortcutsChanged").await) {
            (Ok(a), Ok(d), Ok(c)) => (a, d, c),
            _ => return Ended::Gone("could not listen to the desktop portal".into()),
        };
        let mut opts: HashMap<&str, Value<'_>> = HashMap::new();
        opts.insert("handle_token", Value::from(token("create")));
        opts.insert("session_handle_token", Value::from(token("session")));
        let session = match request(&conn, &proxy, "CreateSession", &token("create"), &opts, Some(QUICK)).await {
            Ok((0, r)) => match r.get("session_handle").and_then(as_string) { Some(s) => s, None => return Ended::Gone("the portal gave no session".into()) },
            Ok((_, _)) => return Ended::Gone("the portal refused a shortcuts session".into()),
            Err(e) => return Ended::Gone(e),
        };
        // the bind may wait on a dialog; keep an eye on Rebind requests (there is nothing to redo yet) and signals after
        if let Err(e) = bind(&conn, &proxy, &session, sink).await { return Ended::Gone(e); }
        loop {
            tokio::select! {
                m = act.next() => match m { Some(m) => key(&m, &session, true, sink), None => return Ended::Gone("lost the desktop portal".into()) },
                m = deact.next() => match m { Some(m) => key(&m, &session, false, sink), None => return Ended::Gone("lost the desktop portal".into()) },
                m = changed.next() => match m {
                    Some(m) => if let Ok((s, l)) = m.body().deserialize::<(OwnedObjectPath, Vec<(String, Dict)>)>() { if s.as_str() == session { sink(Event::Triggers(infos(l))); } },
                    None => return Ended::Gone("lost the desktop portal".into()),
                },
                c = ctl.recv() => match c {
                    None => return Ended::Closed,
                    Some(Ctl::Rebind) => {
                        // version 2 can open the desktop's own shortcut settings; older ones ask again
                        let sess = ObjectPath::try_from(session.as_str()).ok();
                        let opened = match (version >= 2, sess) {
                            (true, Some(s)) => { let o: HashMap<&str, Value<'_>> = HashMap::new(); tokio::time::timeout(QUICK, proxy.call_method("ConfigureShortcuts", &(s, "", o))).await.is_ok_and(|r| r.is_ok()) }
                            _ => false,
                        };
                        if !opened { if let Err(e) = bind(&conn, &proxy, &session, sink).await { return Ended::Gone(e); } }
                    }
                },
            }
        }
    }

    fn key(m: &zbus::Message, session: &str, down: bool, sink: &Sink) {
        let Ok((s, id, _ts, _o)) = m.body().deserialize::<(OwnedObjectPath, String, u64, Dict)>() else { return };
        if s.as_str() != session { return; }
        if let Some(shortcut) = Shortcut::from_id(&id) { sink(Event::Key { shortcut, down }); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Shortcut::*;

    fn st(ptt_mode: bool, muted: bool, deafened: bool, ptt_held: bool) -> VoiceState { VoiceState { ptt_mode, muted, deafened, ptt_held } }

    #[test]
    fn hold_opens_the_mic_and_release_closes_it() {
        let s = st(true, true, false, false).key(PushToTalk, true);
        assert_eq!(s, st(true, false, false, true));
        let s = s.key(PushToTalk, false);
        assert_eq!(s, st(true, true, false, false));
    }

    #[test]
    fn push_to_talk_does_nothing_with_the_mode_off() {
        let s = st(false, false, false, false);
        assert_eq!(s.key(PushToTalk, true), s);
        assert_eq!(s.key(PushToTalk, false), s, "a release never mutes someone who is not in push-to-talk mode");
        let m = st(false, true, false, false);
        assert_eq!(m.key(PushToTalk, true), m, "no hold-to-unmute without the mode");
    }

    #[test]
    fn repeated_presses_and_stray_releases_are_harmless() {
        let s = st(true, true, false, false).key(PushToTalk, true).key(PushToTalk, true);
        assert_eq!(s, st(true, false, false, true));
        let s = s.key(PushToTalk, false).key(PushToTalk, false);
        assert_eq!(s, st(true, true, false, false));
    }

    #[test]
    fn toggle_mute_flips_on_the_press_only() {
        let s = st(false, false, false, false);
        let s = s.key(ToggleMute, true);
        assert!(s.muted);
        assert_eq!(s.key(ToggleMute, false), s, "the release does nothing");
        assert!(!s.key(ToggleMute, true).muted);
    }

    #[test]
    fn toggle_mute_while_push_to_talk_is_held_then_release_closes_the_mic() {
        let held = st(true, true, false, false).key(PushToTalk, true);
        let toggled = held.key(ToggleMute, true);
        assert!(toggled.muted && toggled.ptt_held, "the toggle mutes even though the key is held");
        let toggled_back = toggled.key(ToggleMute, true);
        assert!(!toggled_back.muted && toggled_back.ptt_held);
        let released = toggled_back.key(PushToTalk, false);
        assert_eq!(released, st(true, true, false, false), "the release closes the mic whatever happened in between");
        // and a toggle that left it muted stays muted after the release
        assert!(toggled.key(PushToTalk, false).muted);
    }

    #[test]
    fn deafen_implies_mute_and_undeafen_leaves_the_mic_alone() {
        let s = st(false, false, false, false).key(ToggleDeafen, true);
        assert!(s.deafened && s.muted);
        let s = s.key(ToggleDeafen, true);
        assert!(!s.deafened && s.muted, "undeafening does not open the mic");
    }

    #[test]
    fn unmuting_a_deafened_call_undeafens_it() {
        let s = st(false, true, true, false).key(ToggleMute, true);
        assert!(!s.muted && !s.deafened);
    }

    #[test]
    fn push_to_talk_cannot_open_the_mic_while_deafened_but_its_release_is_not_lost() {
        let s = st(true, true, true, false);
        assert_eq!(s.key(PushToTalk, true), s);
        // deafened while the key was held: the release still clears the hold
        let held = st(true, false, false, true);
        let d = held.key(ToggleDeafen, true);
        assert!(d.deafened && d.muted && d.ptt_held);
        let r = d.key(PushToTalk, false);
        assert_eq!(r, st(true, true, true, false));
    }

    #[test]
    fn ids_round_trip_and_status_names_are_stable() {
        for s in Shortcut::ALL { assert_eq!(Shortcut::from_id(s.id()), Some(s)); }
        assert_eq!(Shortcut::from_id("nope"), None);
        let names: Vec<_> = [Status::Off, Status::Pending, Status::Active, Status::Denied, Status::Unavailable, Status::Unsupported].iter().map(|s| s.as_str()).collect();
        assert_eq!(names, ["off", "pending", "active", "denied", "unavailable", "unsupported"]);
    }
}

/// The D-Bus half against a private session bus with a mock GlobalShortcuts portal. The user's own bus and portal are
/// never touched: every test starts its own `dbus-daemon` on a socket in a directory of its own.
#[cfg(all(test, target_os = "linux"))]
mod portal_tests {
    use super::portal::{Dict, DEST, IFACE, PATH, REQUEST};
    use super::*;
    use std::collections::HashMap;
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

    struct Bus { child: Child, dir: std::path::PathBuf, addr: String }
    impl Bus {
        fn start(tag: &str) -> Option<Bus> {
            let dir = std::env::temp_dir().join(format!("mega-ptt-bus-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).ok()?;
            let addr = format!("unix:path={}", dir.join("bus").display());
            let child = Command::new("dbus-daemon").args(["--session", "--nofork", "--address", &addr]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()?;
            let sock = dir.join("bus");
            let t = Instant::now();
            while !sock.exists() { if t.elapsed() > Duration::from_secs(5) { return None; } std::thread::sleep(Duration::from_millis(20)); }
            Some(Bus { child, dir, addr })
        }
    }
    impl Drop for Bus { fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); let _ = std::fs::remove_dir_all(&self.dir); } }

    /// How the mock answers a bind.
    #[derive(Clone, Copy, PartialEq)]
    enum Answer { Approve, Decline }

    struct Mock { answer: Arc<Mutex<Answer>>, binds: Arc<Mutex<u32>>, bound_ids: Arc<Mutex<Vec<String>>>, version: u32 }

    fn request_path(hdr: &zbus::message::Header<'_>, options: &HashMap<String, OwnedValue>) -> String {
        let sender = hdr.sender().map(|s| s.as_str().trim_start_matches(':').replace('.', "_")).unwrap_or_default();
        let tok = options.get("handle_token").and_then(|v| <&str>::try_from(&**v).ok()).unwrap_or("x").to_string();
        format!("/org/freedesktop/portal/desktop/request/{sender}/{tok}")
    }
    async fn respond(conn: &zbus::Connection, path: String, code: u32, results: HashMap<String, OwnedValue>) {
        std::thread::sleep(Duration::from_millis(30));   // the service runs on zbus's executor, not tokio's
        let _ = conn.emit_signal(None::<&str>, path, REQUEST, "Response", &(code, results)).await;
    }

    #[zbus::interface(name = "org.freedesktop.portal.GlobalShortcuts")]
    impl Mock {
        // the real portal's property is lowercase `version`
        #[zbus(property, name = "version")]
        fn version(&self) -> u32 { self.version }

        async fn create_session(&self, options: HashMap<String, OwnedValue>, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> OwnedObjectPath {
            let path = request_path(&hdr, &options);
            let mut r: HashMap<String, OwnedValue> = HashMap::new();
            r.insert("session_handle".into(), OwnedValue::try_from(Value::from("/org/freedesktop/portal/desktop/session/mock/s1")).unwrap());
            { let c = conn.clone(); let p = path.clone(); std::thread::spawn(move || futures::executor::block_on(respond(&c, p, 0, r))); }
            OwnedObjectPath::try_from(path).unwrap()
        }

        async fn bind_shortcuts(&self, session: ObjectPath<'_>, shortcuts: Vec<(String, HashMap<String, OwnedValue>)>, _parent: String, options: HashMap<String, OwnedValue>,
                                #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> OwnedObjectPath {
            let _ = session;
            *self.binds.lock().unwrap() += 1;
            *self.bound_ids.lock().unwrap() = shortcuts.iter().map(|(i, _)| i.clone()).collect();
            let path = request_path(&hdr, &options);
            let (code, results) = if *self.answer.lock().unwrap() == Answer::Approve {
                let list: Vec<(String, HashMap<String, OwnedValue>)> = shortcuts.iter().map(|(id, _)| {
                    let mut d: HashMap<String, OwnedValue> = HashMap::new();
                    d.insert("trigger_description".into(), OwnedValue::try_from(Value::from(format!("Press {id}"))).unwrap());
                    (id.clone(), d)
                }).collect();
                let mut r: HashMap<String, OwnedValue> = HashMap::new();
                r.insert("shortcuts".into(), OwnedValue::try_from(Value::new(list)).unwrap());
                (0, r)
            } else { (1, HashMap::new()) };
            { let c = conn.clone(); let p = path.clone(); std::thread::spawn(move || futures::executor::block_on(respond(&c, p, code, results))); }
            OwnedObjectPath::try_from(path).unwrap()
        }
    }

    struct Portal { conn: zbus::Connection, binds: Arc<Mutex<u32>>, bound_ids: Arc<Mutex<Vec<String>>>, answer: Arc<Mutex<Answer>> }
    impl Portal {
        async fn start(addr: &str, answer: Answer, version: u32) -> Portal {
            let (a, b, ids) = (Arc::new(Mutex::new(answer)), Arc::new(Mutex::new(0)), Arc::new(Mutex::new(vec![])));
            let conn = zbus::connection::Builder::address(addr).unwrap().name(DEST).unwrap()
                .serve_at(PATH, Mock { answer: a.clone(), binds: b.clone(), bound_ids: ids.clone(), version }).unwrap().build().await.unwrap();
            Portal { conn, binds: b, bound_ids: ids, answer: a }
        }
        /// The portal sending what the desktop sends when the person presses or lets go of a key.
        async fn key(&self, id: &str, down: bool) {
            let opts: Dict = HashMap::new();
            let sess = ObjectPath::try_from("/org/freedesktop/portal/desktop/session/mock/s1").unwrap();
            self.conn.emit_signal(None::<&str>, PATH, IFACE, if down { "Activated" } else { "Deactivated" }, &(sess, id, 1u64, opts)).await.unwrap();
        }
    }

    fn collect() -> (Sink, Arc<Mutex<Vec<Event>>>) {
        let ev: Arc<Mutex<Vec<Event>>> = Default::default();
        let e2 = ev.clone();
        (Arc::new(move |e| e2.lock().unwrap().push(e)), ev)
    }
    async fn until(what: &str, mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while !f() { assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}"); tokio::time::sleep(Duration::from_millis(20)).await; }
    }
    fn last_status(ev: &Arc<Mutex<Vec<Event>>>) -> Option<Status> { ev.lock().unwrap().iter().rev().find_map(|e| if let Event::Status { status, .. } = e { Some(*status) } else { None }) }
    fn keys(ev: &Arc<Mutex<Vec<Event>>>) -> Vec<(Shortcut, bool)> { ev.lock().unwrap().iter().filter_map(|e| if let Event::Key { shortcut, down } = e { Some((*shortcut, *down)) } else { None }).collect() }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_portal_binds_three_shortcuts_and_reports_their_keys() {
        let Some(bus) = Bus::start("bind") else { eprintln!("skipped: no dbus-daemon"); return };
        let portal = Portal::start(&bus.addr, Answer::Approve, 1).await;
        let (sink, ev) = collect();
        let ctl = super::spawn(&tokio::runtime::Handle::current(), Mode::Bus(bus.addr.clone()), "app.commons.chat.test", sink).expect("task");
        until("active", || last_status(&ev) == Some(Status::Active)).await;
        assert_eq!(*portal.bound_ids.lock().unwrap(), ["push-to-talk", "toggle-mute", "toggle-deafen"]);
        let triggers = ev.lock().unwrap().iter().find_map(|e| if let Event::Triggers(t) = e { Some(t.clone()) } else { None }).expect("triggers");
        assert_eq!(triggers.len(), 3);
        assert_eq!(triggers[0].trigger, "Press push-to-talk");
        portal.key("push-to-talk", true).await;
        portal.key("toggle-mute", true).await;
        portal.key("toggle-mute", false).await;
        portal.key("nonsense", true).await;
        portal.key("push-to-talk", false).await;
        until("keys", || keys(&ev).len() == 4).await;
        assert_eq!(keys(&ev), [(PushToTalk, true), (ToggleMute, true), (ToggleMute, false), (PushToTalk, false)]);
        drop(ctl);
    }
    use Shortcut::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_declined_dialog_reports_denied_and_a_rebind_can_still_succeed() {
        let Some(bus) = Bus::start("deny") else { eprintln!("skipped: no dbus-daemon"); return };
        let portal = Portal::start(&bus.addr, Answer::Decline, 1).await;
        let (sink, ev) = collect();
        let ctl = super::spawn(&tokio::runtime::Handle::current(), Mode::Bus(bus.addr.clone()), "app.commons.chat.test", sink).expect("task");
        until("denied", || last_status(&ev) == Some(Status::Denied)).await;
        *portal.answer.lock().unwrap() = Answer::Approve;
        ctl.send(Ctl::Rebind).unwrap();
        until("active after rebind", || last_status(&ev) == Some(Status::Active)).await;
        assert_eq!(*portal.binds.lock().unwrap(), 2);
        portal.key("toggle-deafen", true).await;
        until("a key after the rebind", || keys(&ev) == [(ToggleDeafen, true)]).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_portal_on_the_bus_falls_back_cleanly() {
        let Some(bus) = Bus::start("none") else { eprintln!("skipped: no dbus-daemon"); return };
        let (sink, ev) = collect();
        let _ctl = super::spawn(&tokio::runtime::Handle::current(), Mode::Bus(bus.addr.clone()), "app.commons.chat.test", sink).expect("task");
        until("unavailable", || last_status(&ev) == Some(Status::Unavailable)).await;
        assert!(keys(&ev).is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_bus_at_all_falls_back_cleanly() {
        let (sink, ev) = collect();
        let _ctl = super::spawn(&tokio::runtime::Handle::current(), Mode::Bus("unix:path=/nonexistent/mega-ptt-bus".into()), "app.commons.chat.test", sink).expect("task");
        until("unavailable", || last_status(&ev) == Some(Status::Unavailable)).await;
    }

    #[tokio::test]
    async fn off_mode_asks_for_nothing() {
        let (sink, ev) = collect();
        assert!(super::spawn(&tokio::runtime::Handle::current(), Mode::Off, "x", sink).is_none());
        assert_eq!(last_status(&ev), Some(Status::Off));
    }

    /// The whole chain: a real engine, started against the mock portal, follows the global keys. No call is needed:
    /// push-to-talk, mute and deafen are state the engine keeps with or without a room.
    #[test]
    fn the_engine_follows_the_global_keys_and_reports_the_status() {
        use crate::calls::engine::{AudioMode, CallEngine, CoreLink, Options};
        struct NoCore;
        impl CoreLink for NoCore { fn call(&self, m: &str, _: serde_json::Value) -> Result<serde_json::Value, String> { Err(format!("no core in this test: {m}")) } }
        let Some(bus) = Bus::start("engine") else { eprintln!("skipped: no dbus-daemon"); return };
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let portal = rt.block_on(Portal::start(&bus.addr, Answer::Approve, 2));
        let mut settings = crate::calls::settings::CallSettings::default();
        settings.push_to_talk = true;
        let engine = CallEngine::start(Arc::new(NoCore), Options { audio: AudioMode::Silent { tone_hz: 900. }, settings, settings_path: None, shortcuts: Mode::Bus(bus.addr.clone()), frames: None });
        let wait = |what: &str, f: &dyn Fn(&crate::calls::engine::Snapshot) -> bool| {
            let t = Instant::now();
            while !f(&engine.snapshot()) { assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}: {:?}", engine.snapshot().global_shortcuts); std::thread::sleep(Duration::from_millis(30)); }
        };
        wait("global shortcuts active", &|s| s.global_shortcuts == "active");
        assert_eq!(engine.snapshot().shortcuts[1].trigger, "Press toggle-mute");
        engine.send(crate::calls::engine::Command::SetPushToTalk(true));
        wait("push to talk mode muted", &|s| s.push_to_talk && s.muted);
        rt.block_on(portal.key("push-to-talk", true));
        wait("held and open", &|s| s.ptt_held && !s.muted);
        rt.block_on(portal.key("push-to-talk", false));
        wait("released and closed", &|s| !s.ptt_held && s.muted);
        rt.block_on(portal.key("toggle-mute", true));
        wait("toggle mute", &|s| !s.muted);
        rt.block_on(portal.key("toggle-deafen", true));
        wait("deafened implies muted", &|s| s.deafened && s.muted);
        rt.block_on(portal.key("toggle-deafen", true));
        wait("undeafened", &|s| !s.deafened);
        // the real window keys go through the very same path
        engine.send(crate::calls::engine::Command::Shortcut { shortcut: Shortcut::ToggleMute, down: true });
        wait("window key toggles", &|s| !s.muted);
        // the portal dies: the keys fall back to the window's own, with a flag saying so
        drop(portal);
        drop(bus);
        wait("unavailable after the bus died", &|s| s.global_shortcuts == "unavailable");
        engine.send(crate::calls::engine::Command::Shortcut { shortcut: Shortcut::PushToTalk, down: true });
        wait("window push to talk still works", &|s| s.ptt_held && !s.muted);
    }
}
