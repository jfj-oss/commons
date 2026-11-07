//! The portal conversation against a stand-in on a private bus, because the real thing opens a dialog on the desktop.
//! This proves our side of the protocol as the portal documentation describes it, not that KDE behaves like the stand-in.
#![cfg(target_os = "linux")]
use commons_gamekeys::backend::Keys;
use commons_gamekeys::{portal, Mode, Rule, Settings};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zbus::blocking::{connection, Connection};
use zbus::message::Header;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";

#[derive(Default)]
struct Seen { bound: Vec<(String, HashMap<String, OwnedValue>)> }
struct Mock { conn: Arc<Mutex<Option<Connection>>>, seen: Arc<Mutex<Seen>>, decline: bool }
impl Mock {
    fn answer(&self, h: &Header<'_>, token: &str, code: u32, results: HashMap<String, OwnedValue>) -> String {
        let name = h.sender().unwrap().trim_start_matches(':').replace('.', "_");
        let path = format!("{PATH}/request/{name}/{token}");
        let (c, p) = (self.conn.lock().unwrap().clone().unwrap(), path.clone());
        std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(30)); c.emit_signal(None::<()>, p.as_str(), "org.freedesktop.portal.Request", "Response", &(code, results)).unwrap(); });
        path
    }
}
#[zbus::interface(name = "org.freedesktop.portal.GlobalShortcuts")]
impl Mock {
    #[zbus(name = "CreateSession")]
    fn create_session(&self, #[zbus(header)] h: Header<'_>, o: HashMap<String, OwnedValue>) -> ObjectPath<'static> {
        let tok = String::try_from(o["handle_token"].try_clone().unwrap()).unwrap();
        let r = HashMap::from([("session_handle".to_string(), OwnedValue::try_from(Value::from("/s/1".to_string())).unwrap())]);
        ObjectPath::try_from(self.answer(&h, &tok, 0, r)).unwrap()
    }
    #[zbus(name = "BindShortcuts")]
    fn bind(&self, #[zbus(header)] h: Header<'_>, _s: ObjectPath<'_>, list: Vec<(String, HashMap<String, OwnedValue>)>, _parent: String, o: HashMap<String, OwnedValue>) -> ObjectPath<'static> {
        self.seen.lock().unwrap().bound = list;
        let tok = String::try_from(o["handle_token"].try_clone().unwrap()).unwrap();
        ObjectPath::try_from(self.answer(&h, &tok, if self.decline { 1 } else { 0 }, HashMap::new())).unwrap()
    }
}

struct Bus(Child);
impl Drop for Bus { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
fn bus() -> (Bus, String) {
    let mut c = Command::new("dbus-daemon").args(["--session", "--nofork", "--print-address=1"]).stdout(Stdio::piped()).spawn().expect("dbus-daemon");
    let mut line = String::new();
    BufReader::new(c.stdout.take().unwrap()).read_line(&mut line).unwrap();
    (Bus(c), line.trim().to_string())
}
fn mock(addr: &str, decline: bool) -> (Connection, Arc<Mutex<Seen>>) {
    let (holder, seen) = (Arc::new(Mutex::new(None)), Arc::new(Mutex::new(Seen::default())));
    let c = connection::Builder::address(addr).unwrap().name("org.freedesktop.portal.Desktop").unwrap().serve_at(PATH, Mock { conn: holder.clone(), seen: seen.clone(), decline }).unwrap().build().unwrap();
    *holder.lock().unwrap() = Some(c.clone());
    (c, seen)
}
fn rules() -> Settings { Settings { rules: vec![Rule { process: Some("game".into()), key: "KeyV".into(), mode: Mode::Hold }], use_portal: true } }
fn signal(c: &Connection, what: &str) { c.emit_signal(None::<()>, PATH, IFACE, what, &(ObjectPath::try_from("/s/1").unwrap(), "mute-keyv", 1u64, HashMap::<String, Value>::new())).unwrap(); std::thread::sleep(Duration::from_millis(80)); }

// One test, because the session bus address is process-wide.
#[test]
fn press_release_decline_and_loss() {
    let (_b, addr) = bus();
    std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &addr);
    let (c, seen) = mock(&addr, false);
    let mut p = portal::open(&rules()).unwrap();
    let bound = seen.lock().unwrap().bound.clone();
    assert_eq!(bound.len(), 1);
    assert_eq!(bound[0].0, "mute-keyv");
    assert_eq!(String::try_from(bound[0].1["preferred_trigger"].try_clone().unwrap()).unwrap(), "v");
    let w = vec!["KeyV".to_string()];
    std::thread::sleep(Duration::from_millis(100));
    assert!(p.down(&w).unwrap().is_empty());
    signal(&c, "Activated");
    assert_eq!(p.down(&w).unwrap(), w, "press reported");
    signal(&c, "Deactivated");
    assert!(p.down(&w).unwrap().is_empty(), "release reported");
    signal(&c, "Activated");
    // The portal crashes with the key down: no release ever comes.
    c.release_name("org.freedesktop.portal.Desktop").unwrap();
    std::thread::sleep(Duration::from_millis(2200));
    assert!(p.down(&w).is_err(), "losing the portal has to be noticed");
    drop(p);
    let (_c2, _) = mock(&addr, true);
    let e = portal::open(&rules()).err().expect("a refusal is an error");
    assert!(e.contains("declined"), "{e}");
}
