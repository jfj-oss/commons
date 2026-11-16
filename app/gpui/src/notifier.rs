//! Desktop notifications with a quick reply (Linux).
//!
//! The core's notify hub (`commons_core::notify`) decides what is worth a notification: the global mode, a room's own
//! mute, who was mentioned, and whether a window is being looked at (the window's heartbeat, app.rs `beat`). It hands the
//! finished note to a [`Sink`]. This is the sink: it talks to `org.freedesktop.Notifications` directly, so it can do what
//! the generic one (notify-rust) cannot:
//!
//! - **Inline reply.** When the notification service lists the `inline-reply` capability (KDE Plasma does), the note
//!   carries an `inline-reply` action; the service shows a text field, and what is typed comes back as the
//!   `NotificationReplied(id, text)` signal. The text is sent into the note's room as an ordinary message through the core.
//! - **Open.** The `default` action (a click on the note, and a button "Open" on services that show buttons) brings the
//!   window forward and opens the room. The window picks the request up from [`Inbox`].
//! - **Without inline reply** (GNOME, a bare daemon, no service at all) only Open is offered; with no service at all
//!   nothing is shown and nothing breaks.
//!
//! A newer note of the same room replaces the older one (`replaces_id`), like the web's `tag`. A note's text is plain: when
//! the service reads markup in the body, `&`, `<` and `>` are escaped so a message cannot format itself.
//!
//! Tests point `MEGA_GPUI_NOTIFY_BUS=<address>` at a private bus with a mock service; the user's own bus is never used then.
use commons_core::notify::{Note, Sink};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Something a notification asks of the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inbound {
    /// Bring the window forward; open this room when there is one.
    Open(Option<String>),
    /// A quick reply was sent into a room (or could not be).
    Replied { gid: String, ok: bool },
}

/// The window reads this (every half second) and acts on what the notifications ask.
#[derive(Default)]
pub struct Inbox(Mutex<VecDeque<Inbound>>);
impl Inbox {
    pub fn push(&self, x: Inbound) { let mut q = self.0.lock().unwrap_or_else(|p| p.into_inner()); if q.len() < 64 { q.push_back(x); } }
    pub fn take(&self) -> Vec<Inbound> { self.0.lock().unwrap_or_else(|p| p.into_inner()).drain(..).collect() }
}

/// Where the notification service is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bus { Session, Address(String) }
impl Bus {
    pub fn from_env() -> Bus { std::env::var("MEGA_GPUI_NOTIFY_BUS").ok().filter(|a| !a.is_empty()).map(Bus::Address).unwrap_or(Bus::Session) }
}

/// Sends a quick reply into a room: (gid, text) -> Ok or the reason.
pub type Reply = Arc<dyn Fn(&str, &str) -> Result<(), String> + Send + Sync>;

/// How long a note stays on screen (the service may keep it in its history longer).
const TIMEOUT_MS: i32 = 10_000;
const KNOWN: usize = 64;

#[cfg(target_os = "linux")]
pub use linux::Notifier;
#[cfg(not(target_os = "linux"))]
pub use other::Notifier;

/// The text of a note as a service that reads markup must get it.
pub fn plain_body(body: &str, markup: bool) -> String {
    if markup { body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;") } else { body.to_string() }
}

/// What goes on the wire for one note, without any bus: the actions offered and the hints, given what the service can do.
#[derive(Debug, PartialEq, Eq)]
pub struct Wire { pub actions: Vec<String>, pub reply: bool, pub body: String }
pub fn wire(caps: &[String], has_room: bool, body: &str) -> Wire {
    let has = |c: &str| caps.iter().any(|x| x == c);
    let mut actions = vec!["default".to_string(), "Open".to_string()];
    let reply = has("inline-reply") && has_room;
    if reply { actions.push("inline-reply".into()); actions.push("Reply".into()); }
    Wire { actions, reply, body: plain_body(body, has("body-markup")) }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::collections::BTreeMap;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
    use zbus::zvariant::Value;
    use zbus::{Connection, Proxy};

    const DEST: &str = "org.freedesktop.Notifications";
    const PATH: &str = "/org/freedesktop/Notifications";

    enum Cmd { Show { tag: String, title: String, body: String, gid: Option<String> }, Close }

    /// The notification sink and the way to show a plain one (a ring, a missed call). Cheap to share.
    pub struct Notifier { tx: UnboundedSender<Cmd> }

    impl Notifier {
        /// Starts the notifier's thread. `reply` sends a quick reply; `inbox` receives what a click asks of the window.
        pub fn start(bus: Bus, reply: Reply, inbox: Arc<Inbox>) -> Arc<Notifier> {
            let (tx, rx) = unbounded_channel();
            let spawned = std::thread::Builder::new().name("mega-notify".into()).spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
                rt.block_on(run(bus, rx, reply, inbox));
            });
            if let Err(e) = spawned { eprintln!("notifier thread: {e}"); }
            Arc::new(Notifier { tx })
        }
        /// A note that is not a message: `tag` replaces an older one with the same tag; a click opens the window (and the
        /// room, when `gid` is given).
        pub fn show_plain(&self, tag: &str, title: &str, body: &str, gid: Option<&str>) {
            let _ = self.tx.send(Cmd::Show { tag: tag.into(), title: title.into(), body: body.into(), gid: gid.map(String::from) });
        }
        /// Takes down the note with this tag (a ring that was answered).
        pub fn close_all(&self) { let _ = self.tx.send(Cmd::Close); }
    }
    impl Sink for Notifier {
        fn show(&self, note: &Note) { self.show_plain(&format!("room:{}", note.gid), &note.title, &note.body, Some(&note.gid)); }
    }

    async fn connect(bus: &Bus) -> Result<Connection, String> {
        match bus {
            Bus::Session => Connection::session().await.map_err(|e| format!("no session bus: {e}")),
            Bus::Address(a) => zbus::connection::Builder::address(a.as_str()).map_err(|e| format!("bad bus address: {e}"))?.build().await.map_err(|e| format!("no bus at that address: {e}")),
        }
    }

    /// One live connection to the service: its capabilities and the three signals we care about.
    struct Live { _conn: Connection, proxy: Proxy<'static>, caps: Vec<String> }

    async fn attach(bus: &Bus) -> Result<Live, String> {
        let conn = connect(bus).await?;
        let proxy = Proxy::new(&conn, DEST, PATH, DEST).await.map_err(|e| e.to_string())?;
        let caps: Vec<String> = tokio::time::timeout(Duration::from_secs(3), proxy.call("GetCapabilities", &())).await
            .map_err(|_| "the notification service did not answer".to_string())?.map_err(|e| format!("no notification service: {e}"))?;
        Ok(Live { _conn: conn, proxy, caps })
    }

    async fn run(bus: Bus, mut rx: tokio::sync::mpsc::UnboundedReceiver<Cmd>, reply: Reply, inbox: Arc<Inbox>) {
        let mut live: Option<Live> = None;
        let mut acted: Option<zbus::proxy::SignalStream<'static>> = None;
        let mut closed: Option<zbus::proxy::SignalStream<'static>> = None;
        let mut replied: Option<zbus::proxy::SignalStream<'static>> = None;
        let mut known: BTreeMap<u32, (String, Option<String>)> = BTreeMap::new();   // notification id -> (tag, room)
        let mut by_tag: HashMap<String, u32> = HashMap::new();
        loop {
            tokio::select! {
                cmd = rx.recv() => {
                    let Some(cmd) = cmd else { return };
                    match cmd {
                        Cmd::Close => {
                            if let Some(l) = &live { for id in known.keys() { let _ = l.proxy.call_method("CloseNotification", &(*id,)).await; } }
                            known.clear(); by_tag.clear();
                        }
                        Cmd::Show { tag, title, body, gid } => {
                            if live.is_none() {
                                match attach(&bus).await {
                                    Ok(l) => {
                                        acted = l.proxy.receive_signal("ActionInvoked").await.ok();
                                        closed = l.proxy.receive_signal("NotificationClosed").await.ok();
                                        replied = l.proxy.receive_signal("NotificationReplied").await.ok();
                                        live = Some(l);
                                    }
                                    Err(e) => { eprintln!("notifications: {e}"); continue; }
                                }
                            }
                            let l = live.as_ref().unwrap();
                            let w = wire(&l.caps, gid.is_some(), &body);
                            let replaces = by_tag.get(&tag).copied().unwrap_or(0);
                            let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
                            hints.insert("category", Value::from("im.received"));
                            hints.insert("desktop-entry", Value::from("app.commons.chat.gpui"));
                            if w.reply { hints.insert("x-kde-reply-placeholder-text", Value::from("Reply")); }
                            let actions: Vec<&str> = w.actions.iter().map(|s| s.as_str()).collect();
                            let sent: Result<u32, _> = l.proxy.call("Notify", &("mega", replaces, "mega", title.as_str(), w.body.as_str(), actions, hints, TIMEOUT_MS)).await;
                            match sent {
                                Ok(id) => {
                                    by_tag.insert(tag.clone(), id);
                                    known.insert(id, (tag, gid));
                                    while known.len() > KNOWN { if let Some((&old, _)) = known.iter().next() { if let Some((t, _)) = known.remove(&old) { if by_tag.get(&t) == Some(&old) { by_tag.remove(&t); } } } }
                                }
                                Err(e) => { eprintln!("notifications: {e}"); live = None; acted = None; closed = None; replied = None; }
                            }
                        }
                    }
                }
                m = next(acted.as_mut()) => {
                    let Some(m) = m else { live = None; acted = None; closed = None; replied = None; continue };
                    if let Ok((id, key)) = m.body().deserialize::<(u32, String)>() {
                        if let Some((_, gid)) = known.get(&id) {
                            if key == "default" || key == "Open" { inbox.push(Inbound::Open(gid.clone())); }
                        }
                    }
                }
                m = next(closed.as_mut()) => {
                    let Some(m) = m else { live = None; acted = None; closed = None; replied = None; continue };
                    if let Ok((id, _why)) = m.body().deserialize::<(u32, u32)>() {
                        if let Some((tag, _)) = known.remove(&id) { if by_tag.get(&tag) == Some(&id) { by_tag.remove(&tag); } }
                    }
                }
                m = next(replied.as_mut()) => {
                    let Some(m) = m else { replied = None; continue };
                    let Ok((id, text)) = m.body().deserialize::<(u32, String)>() else { continue };
                    let Some((_, Some(gid))) = known.get(&id).cloned() else { continue };
                    let text = text.trim().to_string();
                    if text.is_empty() { continue; }
                    // the core call blocks: off this thread, so the next signal is not held up
                    let (reply, inbox) = (reply.clone(), inbox.clone());
                    std::thread::spawn(move || {
                        let ok = reply(&gid, &text).map_err(|e| eprintln!("quick reply: {e}")).is_ok();
                        inbox.push(Inbound::Replied { gid, ok });
                    });
                }
            }
        }
    }

    /// The next message of an optional stream; waits forever when there is none, so `select!` just ignores that branch.
    async fn next(s: Option<&mut zbus::proxy::SignalStream<'static>>) -> Option<zbus::Message> {
        use futures::StreamExt;
        match s { Some(s) => s.next().await, None => std::future::pending().await }
    }
}

#[cfg(not(target_os = "linux"))]
mod other {
    use super::*;
    /// Elsewhere the core's own native sink shows messages; there is no quick reply here (yet).
    pub struct Notifier;
    impl Notifier {
        pub fn start(_: Bus, _: Reply, _: Arc<Inbox>) -> Arc<Notifier> { Arc::new(Notifier) }
        pub fn show_plain(&self, _: &str, _: &str, _: &str, _: Option<&str>) {}
        pub fn close_all(&self) {}
    }
    impl Sink for Notifier { fn show(&self, _: &Note) {} }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};
    use std::time::Instant;
    use zbus::zvariant::OwnedValue;

    /// A private session bus: its own daemon and socket, never the user's.
    struct PrivateBus { child: Child, dir: std::path::PathBuf, addr: String }
    impl PrivateBus {
        fn start(tag: &str) -> Option<PrivateBus> {
            let base = std::env::var_os("MEGA_TEST_TMP").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
            let dir = base.join(format!("mega-notify-bus-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).ok()?;
            let addr = format!("unix:path={}", dir.join("bus").display());
            // its own small configuration: no service directories, so no name on this bus can ever start a real program
            let conf = dir.join("bus.conf");
            std::fs::write(&conf, format!("<busconfig><type>session</type><listen>{addr}</listen><auth>EXTERNAL</auth><policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/><allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>")).ok()?;
            let child = Command::new("dbus-daemon").args(["--nofork", "--config-file"]).arg(&conf).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()?;
            let sock = dir.join("bus");
            let t = Instant::now();
            while !sock.exists() { if t.elapsed() > Duration::from_secs(5) { return None; } std::thread::sleep(Duration::from_millis(20)); }
            Some(PrivateBus { child, dir, addr })
        }
    }
    impl Drop for PrivateBus { fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); let _ = std::fs::remove_dir_all(&self.dir); } }

    #[derive(Clone, Debug)]
    struct Got { id: u32, replaces: u32, summary: String, body: String, actions: Vec<String>, hints: Vec<String>, category: String }

    struct Mock { got: Arc<Mutex<Vec<Got>>>, caps: Vec<String> }
    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl Mock {
        #[allow(clippy::too_many_arguments)]
        async fn notify(&self, _app: String, replaces: u32, _icon: String, summary: String, body: String, actions: Vec<String>, hints: HashMap<String, OwnedValue>, _timeout: i32) -> u32 {
            let mut g = self.got.lock().unwrap();
            let id = if replaces != 0 { replaces } else { g.iter().map(|x| x.id).max().unwrap_or(100) + 1 };
            let category = hints.get("category").and_then(|v| <&str>::try_from(&**v).ok()).unwrap_or("").to_string();
            g.push(Got { id, replaces, summary, body, actions, hints: hints.keys().cloned().collect(), category });
            id
        }
        fn get_capabilities(&self) -> Vec<String> { self.caps.clone() }
        fn close_notification(&self, _id: u32) {}
        fn get_server_information(&self) -> (String, String, String, String) { ("mock".into(), "mega".into(), "1".into(), "1.2".into()) }
    }

    struct Service { conn: zbus::Connection, got: Arc<Mutex<Vec<Got>>> }
    impl Service {
        async fn start(addr: &str, caps: &[&str]) -> Service {
            let got: Arc<Mutex<Vec<Got>>> = Default::default();
            let conn = zbus::connection::Builder::address(addr).unwrap().name(DEST_NAME).unwrap()
                .serve_at("/org/freedesktop/Notifications", Mock { got: got.clone(), caps: caps.iter().map(|s| s.to_string()).collect() }).unwrap().build().await.unwrap();
            Service { conn, got }
        }
        async fn signal(&self, name: &str, id: u32, text: &str) {
            self.conn.emit_signal(None::<&str>, "/org/freedesktop/Notifications", "org.freedesktop.Notifications", name, &(id, text)).await.unwrap();
        }
    }
    const DEST_NAME: &str = "org.freedesktop.Notifications";

    fn note(gid: &str, title: &str, body: &str) -> Note { Note { gid: gid.into(), title: title.into(), body: body.into() } }
    async fn until(what: &str, mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while !f() { assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}"); tokio::time::sleep(Duration::from_millis(20)).await; }
    }
    type Sent = Arc<Mutex<Vec<(String, String)>>>;
    fn recorder() -> (Reply, Sent) {
        let sent: Sent = Default::default();
        let s2 = sent.clone();
        (Arc::new(move |g: &str, t: &str| { s2.lock().unwrap().push((g.into(), t.into())); Ok(()) }), sent)
    }

    #[test]
    fn what_goes_on_the_wire_follows_what_the_service_can_do() {
        let caps = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let kde = wire(&caps(&["actions", "body", "body-markup", "inline-reply"]), true, "a <b> & c");
        assert_eq!(kde.actions, ["default", "Open", "inline-reply", "Reply"]);
        assert_eq!(kde.body, "a &lt;b&gt; &amp; c");
        let plain = wire(&caps(&["actions", "body"]), true, "a <b> & c");
        assert_eq!(plain.actions, ["default", "Open"]);
        assert_eq!(plain.body, "a <b> & c", "a service that reads no markup gets the text as it is");
        let ring = wire(&caps(&["inline-reply"]), false, "x");
        assert_eq!(ring.actions, ["default", "Open"], "a note with no room has nothing to reply to");
    }

    #[test]
    fn a_quick_reply_goes_into_the_room_and_a_click_opens_it() {
        let Some(bus) = PrivateBus::start("reply") else { eprintln!("skipped: no dbus-daemon"); return };
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let svc = rt.block_on(Service::start(&bus.addr, &["actions", "body", "body-markup", "inline-reply"]));
        let (reply, sent) = recorder();
        let inbox = Arc::new(Inbox::default());
        let n = Notifier::start(Bus::Address(bus.addr.clone()), reply, inbox.clone());
        rt.block_on(async {
            n.show(&note("gid-1", "mega", "New message from Ben in #lounge <hi>"));
            until("the note", || svc.got.lock().unwrap().len() == 1).await;
            let g = svc.got.lock().unwrap()[0].clone();
            assert_eq!((g.summary.as_str(), g.category.as_str()), ("mega", "im.received"));
            assert_eq!(g.body, "New message from Ben in #lounge &lt;hi&gt;");
            assert_eq!(g.actions, ["default", "Open", "inline-reply", "Reply"]);
            assert!(g.hints.iter().any(|h| h == "x-kde-reply-placeholder-text"));
            // the person types a reply into the note
            svc.signal("NotificationReplied", g.id, "  on my way  ").await;
            until("the reply", || !sent.lock().unwrap().is_empty()).await;
            assert_eq!(*sent.lock().unwrap(), [("gid-1".to_string(), "on my way".to_string())]);
            until("the reply is reported", || inbox.take().contains(&Inbound::Replied { gid: "gid-1".into(), ok: true })).await;
            // an empty reply, a reply to a note we never made, and a click on a note of ours
            svc.signal("NotificationReplied", g.id, "   ").await;
            svc.signal("NotificationReplied", 999, "stray").await;
            svc.signal("ActionInvoked", 999, "default").await;
            svc.signal("ActionInvoked", g.id, "default").await;
            until("the click", || { let q = inbox.take(); q.contains(&Inbound::Open(Some("gid-1".into()))) }).await;
            assert_eq!(sent.lock().unwrap().len(), 1, "nothing else was sent");
            // a newer note of the same room replaces the older one
            n.show(&note("gid-1", "mega", "another"));
            until("second note", || svc.got.lock().unwrap().len() == 2).await;
            let g2 = svc.got.lock().unwrap()[1].clone();
            assert_eq!(g2.replaces, g.id);
            // a note of another room does not
            n.show(&note("gid-2", "mega", "elsewhere"));
            until("third note", || svc.got.lock().unwrap().len() == 3).await;
            assert_eq!(svc.got.lock().unwrap()[2].replaces, 0);
        });
    }

    #[test]
    fn without_inline_reply_only_open_is_offered_and_it_still_opens() {
        let Some(bus) = PrivateBus::start("noreply") else { eprintln!("skipped: no dbus-daemon"); return };
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let svc = rt.block_on(Service::start(&bus.addr, &["actions", "body"]));
        let (reply, sent) = recorder();
        let inbox = Arc::new(Inbox::default());
        let n = Notifier::start(Bus::Address(bus.addr.clone()), reply, inbox.clone());
        rt.block_on(async {
            n.show(&note("g", "mega", "hello"));
            until("the note", || svc.got.lock().unwrap().len() == 1).await;
            let g = svc.got.lock().unwrap()[0].clone();
            assert_eq!(g.actions, ["default", "Open"]);
            assert!(!g.hints.iter().any(|h| h.starts_with("x-kde-reply")));
            svc.signal("ActionInvoked", g.id, "Open").await;
            until("open", || inbox.take().contains(&Inbound::Open(Some("g".into())))).await;
        });
        assert!(sent.lock().unwrap().is_empty());
    }

    #[test]
    fn a_plain_note_without_a_room_opens_only_the_window() {
        let Some(bus) = PrivateBus::start("plain") else { eprintln!("skipped: no dbus-daemon"); return };
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let svc = rt.block_on(Service::start(&bus.addr, &["actions", "inline-reply"]));
        let (reply, _) = recorder();
        let inbox = Arc::new(Inbox::default());
        let n = Notifier::start(Bus::Address(bus.addr.clone()), reply, inbox.clone());
        rt.block_on(async {
            n.show_plain("ring", "Alice is calling you", "porch", None);
            until("the note", || svc.got.lock().unwrap().len() == 1).await;
            let g = svc.got.lock().unwrap()[0].clone();
            assert_eq!(g.actions, ["default", "Open"]);
            svc.signal("ActionInvoked", g.id, "default").await;
            until("open", || inbox.take().contains(&Inbound::Open(None))).await;
        });
    }

    #[test]
    fn no_notification_service_is_not_an_error() {
        let Some(bus) = PrivateBus::start("none") else { eprintln!("skipped: no dbus-daemon"); return };
        let (reply, _) = recorder();
        let n = Notifier::start(Bus::Address(bus.addr.clone()), reply.clone(), Arc::new(Inbox::default()));
        n.show(&note("g", "mega", "nobody hears this"));
        let n2 = Notifier::start(Bus::Address("unix:path=/nonexistent/mega-notify-bus".into()), reply, Arc::new(Inbox::default()));
        n2.show(&note("g", "mega", "nor this"));
        std::thread::sleep(Duration::from_millis(400));
        // a service that appears later is found at the next note
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let svc = rt.block_on(Service::start(&bus.addr, &["actions"]));
        n.show(&note("g", "mega", "now"));
        rt.block_on(until("the late service", || svc.got.lock().unwrap().len() == 1));
    }
}
