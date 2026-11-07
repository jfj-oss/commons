//! Linux: keys as system shortcuts through the desktop portal (org.freedesktop.portal.GlobalShortcuts). The desktop asks the
//! person once. It reports press and release where the desktop does (KDE, version 2 of the portal), and nothing else about
//! the keyboard. Its cost: the compositor owns such a key, so the focused game may not get it. See docs/GAME-KEYS.md.
use crate::backend::Keys;
use crate::{keys, Settings};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
const APP_ID: &str = "app.commons.chat";

#[derive(Default)]
struct Shared { down: BTreeSet<String>, ids: BTreeMap<String, String>, session: String, gone: bool }

pub struct Portal { conn: Connection, shared: Arc<Mutex<Shared>>, session: String, checked: std::time::Instant }

fn token() -> String { format!("commons{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0)) }

/// A portal call answers on a Request object, and the answer can be a long way off: a person is reading a dialog.
fn request(conn: &Connection, method: &str, args: &(impl serde::Serialize + zbus::zvariant::DynamicType), tok: &str) -> Result<HashMap<String, OwnedValue>, String> {
    let name = conn.unique_name().ok_or("no bus name")?.trim_start_matches(':').replace('.', "_");
    let path = format!("{PATH}/request/{name}/{tok}");
    let req = Proxy::new(conn, DEST, path.as_str(), "org.freedesktop.portal.Request").map_err(|e| e.to_string())?;
    let mut answers = req.receive_signal("Response").map_err(|e| e.to_string())?;
    let p = Proxy::new(conn, DEST, PATH, IFACE).map_err(|e| e.to_string())?;
    let _: OwnedObjectPath = p.call(method, args).map_err(|e| format!("{method}: {e}"))?;
    let m = answers.next().ok_or("the portal went away")?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = m.body().deserialize().map_err(|e| e.to_string())?;
    if code != 0 { return Err("the shortcut request was declined".into()); }
    Ok(results)
}

/// The portal's spelling, as a suggestion: the desktop and the person have the last word.
fn trigger(code: &str) -> Option<String> {
    if let Some(l) = code.strip_prefix("Key") { return Some(l.to_lowercase()); }
    if let Some(d) = code.strip_prefix("Digit") { return Some(d.to_string()); }
    if code.starts_with('F') && code[1..].parse::<u8>().is_ok() { return Some(code.to_string()); }
    None
}

pub fn open(s: &Settings) -> Result<Portal, String> {
    let conn = Connection::session().map_err(|e| format!("no session bus: {e}"))?;
    // Programs outside a sandbox say who they are; a portal without this registry just does not have the method.
    if let Ok(r) = Proxy::new(&conn, DEST, PATH, "org.freedesktop.host.portal.Registry") {
        let _: Result<(), _> = r.call("Register", &(APP_ID, HashMap::<String, Value>::new()));
    }
    let tok = token();
    let opts: HashMap<&str, Value> = HashMap::from([("handle_token", Value::from(format!("{tok}a"))), ("session_handle_token", Value::from(format!("{tok}s")))]);
    let r = request(&conn, "CreateSession", &(opts,), &format!("{tok}a"))?;
    let session = r.get("session_handle").and_then(|v| v.downcast_ref::<String>().ok().or_else(|| v.downcast_ref::<ObjectPath>().ok().map(|p| p.to_string()))).ok_or("the portal gave no session")?;
    let mut ids = BTreeMap::new();
    let mut list: Vec<(String, HashMap<&str, Value>)> = vec![];
    let codes: BTreeSet<&String> = s.rules.iter().map(|r| &r.key).collect();
    for c in codes {
        if !keys::known(c) { continue; }
        let id = format!("mute-{}", c.to_lowercase());
        ids.insert(id.clone(), c.clone());
        let mut o: HashMap<&str, Value> = HashMap::from([("description", Value::from(format!("Mute Commons ({c})")))]);
        if let Some(t) = trigger(c) { o.insert("preferred_trigger", Value::from(t)); }
        list.push((id, o));
    }
    let path = ObjectPath::try_from(session.as_str()).map_err(|e| e.to_string())?;
    let bt = format!("{tok}b");
    let bopts: HashMap<&str, Value> = HashMap::from([("handle_token", Value::from(bt.clone()))]);
    request(&conn, "BindShortcuts", &(path, list, "", bopts), &bt)?;

    let shared = Arc::new(Mutex::new(Shared { ids, session: session.clone(), ..Default::default() }));
    let (c2, s2) = (conn.clone(), shared.clone());
    std::thread::spawn(move || {
        let run = || -> Result<(), String> {
            let p = Proxy::new(&c2, DEST, PATH, IFACE).map_err(|e| e.to_string())?;
            for m in p.receive_all_signals().map_err(|e| e.to_string())? {
                let h = m.header();
                let member = h.member().map(|x| x.to_string()).unwrap_or_default();
                let Ok((sess, id, _, _)): Result<(OwnedObjectPath, String, u64, HashMap<String, OwnedValue>), _> = m.body().deserialize() else { continue };
                let mut g = s2.lock().unwrap_or_else(|p| p.into_inner());
                if sess.as_str() != g.session { continue; }
                let Some(code) = g.ids.get(&id).cloned() else { continue };
                match member.as_str() { "Activated" => { g.down.insert(code); } "Deactivated" => { g.down.remove(&code); } _ => {} }
            }
            Ok(())
        };
        let _ = run();
        s2.lock().unwrap_or_else(|p| p.into_inner()).gone = true;
    });
    Ok(Portal { conn, shared, session, checked: std::time::Instant::now() })
}

impl Keys for Portal {
    fn down(&mut self, wanted: &[String]) -> Result<Vec<String>, String> {
        // A portal that crashes sends no goodbye, and a key it saw go down would stay down.
        if self.checked.elapsed() >= std::time::Duration::from_secs(2) {
            self.checked = std::time::Instant::now();
            let bus = Proxy::new(&self.conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").map_err(|e| e.to_string())?;
            let alive: bool = bus.call("NameHasOwner", &(DEST,)).map_err(|e| e.to_string())?;
            if !alive { return Err("the portal went away".into()); }
        }
        let g = self.shared.lock().unwrap_or_else(|p| p.into_inner());
        if g.gone { return Err("the portal went away".into()); }
        Ok(wanted.iter().filter(|k| g.down.contains(*k)).cloned().collect())
    }
}
impl Drop for Portal {
    fn drop(&mut self) {
        if let Ok(s) = Proxy::new(&self.conn, DEST, self.session.as_str(), "org.freedesktop.portal.Session") { let _: Result<(), _> = s.call("Close", &()); }
        self.shared.lock().unwrap_or_else(|p| p.into_inner()).session.clear();
    }
}
