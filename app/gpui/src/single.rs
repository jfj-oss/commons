//! One window per account folder (the Tauri app's single-instance plugin, app/src-tauri/src/lib.rs): starting mega a second
//! time raises the window that is already open and hands it what it was given (an invite link), then the new process ends.
//!
//! How: a lock file and a local socket in the data folder (`instance.lock`, `mega.sock`). The first process holds the lock
//! for as long as it lives and listens on the socket; a later one finds the lock taken, connects, says what it wants and
//! leaves. The lock is the truth (a crash releases it, the socket file left behind is simply replaced); the socket is only
//! how the second process talks. Separate data folders (`COMMONS_DATA`, the tests, a second account on purpose) never meet.
//!
//! The same socket is how a notification click reaches the window when the click was handled by another program (the
//! Linux launcher, `mega-gpui --open <room>`): the message says which room to open.
//!
//! Protocol, one line each way: `{"show":true,"open":"<gid>","link":"<invite link>"}` and the answer `ok`.
use crate::notifier::{Inbound, Inbox};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub enum Claim {
    /// This process is the window. Keep the guard (it holds the lock) and `serve` it.
    First(Option<Primary>),
    /// Another process has the account and was told; this one should end.
    Handed,
}

/// What a second launch asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ask { pub open: Option<String>, pub link: Option<String> }

pub fn ask_from_args(args: &[String]) -> Ask {
    let mut ask = Ask::default();
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        if a == "--open" { ask.open = it.next().cloned().filter(|g| !g.is_empty()); }
        else if let Some(g) = a.strip_prefix("--open=") { ask.open = Some(g.to_string()).filter(|g| !g.is_empty()); }
        else if crate::members::is_link(a) && ask.link.is_none() { ask.link = Some(a.trim().to_string()); }
    }
    ask
}

pub fn encode(ask: &Ask) -> String {
    serde_json::json!({"show": true, "open": ask.open, "link": ask.link}).to_string()
}
pub fn decode(line: &str) -> Option<Ask> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if v["show"].as_bool() != Some(true) { return None; }
    let s = |k: &str| v[k].as_str().filter(|x| !x.is_empty() && x.len() < 4096).map(String::from);
    Some(Ask { open: s("open"), link: s("link") })
}

/// The socket's path: in the data folder, or (a path that would not fit a socket's address, about a hundred bytes) in the
/// runtime folder under a name made from the data folder's.
pub fn socket_path(dir: &Path) -> PathBuf {
    let p = dir.join("mega.sock");
    if p.as_os_str().len() < 96 { return p; }
    let mut h: u64 = 0xcbf29ce484222325;
    for b in dir.as_os_str().as_encoded_bytes() { h = (h ^ *b as u64).wrapping_mul(0x100000001b3); }
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|d| d.is_dir()).unwrap_or_else(std::env::temp_dir);
    base.join(format!("mega-{h:016x}.sock"))
}

#[cfg(unix)]
pub use unix::{claim, Primary};
#[cfg(not(unix))]
pub fn claim(_: &Path, _: &Ask) -> Claim { Claim::First(None) }
#[cfg(not(unix))]
pub struct Primary;
#[cfg(not(unix))]
impl Primary { pub fn serve(self, _: Arc<Inbox>) {} }

#[cfg(unix)]
mod unix {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::time::{Duration, Instant};

    pub struct Primary { lock: std::fs::File, listener: UnixListener, path: PathBuf }

    /// Tells a running window what we want. True when it answered.
    fn hand_over(sock: &Path, ask: &Ask) -> bool {
        let Ok(mut s) = UnixStream::connect(sock) else { return false };
        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
        if writeln!(s, "{}", encode(ask)).is_err() { return false; }
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).is_ok() && line.trim() == "ok"
    }

    pub fn claim(dir: &Path, ask: &Ask) -> Claim {
        if std::env::var("MEGA_GPUI_SINGLE").as_deref() == Ok("off") { return Claim::First(None); }
        if std::fs::create_dir_all(dir).is_err() { return Claim::First(None); }
        let sock = socket_path(dir);
        if hand_over(&sock, ask) { return Claim::Handed; }
        let Ok(lock) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("instance.lock")) else { return Claim::First(None) };
        match lock.try_lock() {
            Ok(()) => {
                let _ = std::fs::remove_file(&sock);
                match UnixListener::bind(&sock) {
                    Ok(listener) => Claim::First(Some(Primary { lock, listener, path: sock })),
                    Err(e) => { eprintln!("single instance: no socket at {}: {e}", sock.display()); Claim::First(None) }
                }
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                // someone holds the lock: it is starting up (no socket yet) or running. Give it a few seconds.
                let t = Instant::now();
                while t.elapsed() < Duration::from_secs(4) {
                    if hand_over(&sock, ask) { return Claim::Handed; }
                    std::thread::sleep(Duration::from_millis(150));
                }
                Claim::First(None)
            }
            Err(_) => Claim::First(None),
        }
    }

    impl Primary {
        /// Listens for the next launches for the rest of the process's life.
        pub fn serve(self, inbox: Arc<Inbox>) {
            let Primary { lock, listener, path } = self;
            let _ = std::thread::Builder::new().name("mega-single".into()).spawn(move || {
                let _keep = (lock, path);   // the lock is held as long as this thread runs, which is as long as the process
                for conn in listener.incoming() {
                    let Ok(mut s) = conn else { continue };
                    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut line = String::new();
                    if BufReader::new(&s).take(8192).read_line(&mut line).is_err() { continue; }
                    let Some(ask) = decode(&line) else { continue };
                    inbox.push(Inbound::Show { open: ask.open, link: ask.link });
                    let _ = writeln!(s, "ok");
                }
            });
        }
    }
    use std::io::Read as _;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let base = std::env::var_os("MEGA_TEST_TMP").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let d = base.join(format!("mega-single-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn arguments_say_which_room_and_which_link() {
        let a = |l: &[&str]| ask_from_args(&l.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&["mega"]), Ask::default());
        assert_eq!(a(&["mega", "--open", "g1"]).open.as_deref(), Some("g1"));
        assert_eq!(a(&["mega", "--open=g2"]).open.as_deref(), Some("g2"));
        assert_eq!(a(&["mega", "commons://join/abc"]).link.as_deref(), Some("commons://join/abc"));
        assert_eq!(a(&["mega", "--open"]).open, None);
    }

    #[test]
    fn a_message_round_trips_and_nothing_else_is_taken_for_one() {
        let ask = Ask { open: Some("room-9".into()), link: Some("commons://join/x".into()) };
        assert_eq!(decode(&encode(&ask)), Some(ask));
        assert_eq!(decode(&encode(&Ask::default())), Some(Ask::default()));
        assert_eq!(decode("hello"), None);
        assert_eq!(decode("{\"show\":false}"), None);
    }

    #[test]
    fn a_long_folder_name_still_gets_a_socket_that_fits() {
        let long = PathBuf::from(format!("/tmp/{}", "x".repeat(200)));
        assert!(socket_path(&long).as_os_str().len() < 108);
        assert_eq!(socket_path(&long), socket_path(&long));
        assert_ne!(socket_path(&long), socket_path(&PathBuf::from(format!("/tmp/{}", "y".repeat(200)))));
        assert_eq!(socket_path(Path::new("/tmp/a")), Path::new("/tmp/a/mega.sock"));
    }

    #[cfg(unix)]
    #[test]
    fn the_second_launch_is_handed_to_the_first_and_a_crash_frees_the_account() {
        let d = dir("two");
        let first = match claim(&d, &Ask::default()) { Claim::First(Some(p)) => p, _ => panic!("the first launch is the window") };
        let inbox = Arc::new(Inbox::default());
        first.serve(inbox.clone());
        let ask = Ask { open: Some("g7".into()), link: None };
        assert!(matches!(claim(&d, &ask), Claim::Handed), "the second launch ends");
        assert!(matches!(claim(&d, &Ask { open: None, link: Some("commons://join/z".into()) }), Claim::Handed));
        let got = inbox.take();
        assert_eq!(got, vec![Inbound::Show { open: Some("g7".into()), link: None }, Inbound::Show { open: None, link: Some("commons://join/z".into()) }]);
        // another account folder is another window
        let d2 = dir("other");
        assert!(matches!(claim(&d2, &Ask::default()), Claim::First(Some(_))));
        let _ = std::fs::remove_dir_all(&d); let _ = std::fs::remove_dir_all(&d2);
    }

    #[cfg(unix)]
    #[test]
    fn a_stale_socket_and_an_unlocked_file_do_not_block_a_start() {
        let d = dir("stale");
        std::fs::create_dir_all(&d).unwrap();
        // a socket file nobody listens on (the last process was killed)
        drop(std::os::unix::net::UnixListener::bind(d.join("mega.sock")).unwrap());
        assert!(matches!(claim(&d, &Ask::default()), Claim::First(Some(_))));
        let _ = std::fs::remove_dir_all(&d);
    }
}
