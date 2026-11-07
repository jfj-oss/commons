//! The operating-system part. Everything here can only look: no key is grabbed, nothing is injected, no game process is
//! opened. Programs are known by name from the system's process list. Only the keys in `wanted` are read or kept.
use crate::{keys, Settings};

pub trait Keys: Send {
    /// Which of `wanted` are down now. An error means this way of seeing keys is gone.
    fn down(&mut self, wanted: &[String]) -> Result<Vec<String>, String>;
}

/// What this computer can really do, for the settings page.
#[derive(Debug, Clone, PartialEq)]
pub struct Info { pub backend: &'static str, pub hold: bool, pub toggle: bool, pub mouse: bool, pub note: String, pub permission: bool }

pub use imp::{info, open, processes, request_permission};

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::collections::{BTreeSet, HashSet};
    use std::fs::File;
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    /// Comm for native programs; the name in the first argument for Wine and Proton, where comm is cut to 15 characters
    /// and the real name is `Z:\games\Game.exe`.
    pub fn processes() -> Vec<String> {
        let mut out = BTreeSet::new();
        for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            let n = e.file_name();
            if !n.to_string_lossy().bytes().all(|c| c.is_ascii_digit()) { continue; }
            let cmd = std::fs::read(e.path().join("cmdline")).unwrap_or_default();
            if cmd.is_empty() { continue; } // a kernel thread
            if let Ok(c) = std::fs::read_to_string(e.path().join("comm")) { let c = c.trim(); if !c.is_empty() { out.insert(c.to_string()); } }
            let first = cmd.split(|b| *b == 0).next().unwrap_or_default();
            let first = String::from_utf8_lossy(first);
            if let Some(b) = first.rsplit(['/', '\\']).next() { if !b.is_empty() { out.insert(b.to_string()); } }
        }
        out.into_iter().collect()
    }

    fn event_files() -> Vec<std::path::PathBuf> {
        std::fs::read_dir("/dev/input").into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("event"))).collect()
    }
    fn evdev_readable() -> bool {
        event_files().iter().any(|p| std::fs::OpenOptions::new().read(true).custom_flags(libc_nonblock()).open(p).is_ok())
    }
    const fn libc_nonblock() -> i32 { 0o4000 }

    /// Reads the kernel's key events without grabbing the device, so everything still reaches whoever it was going to reach.
    pub struct Evdev { files: Vec<File>, down: HashSet<u16> }
    impl Evdev {
        pub fn open() -> Result<Evdev, String> {
            let files: Vec<File> = event_files().iter().filter_map(|p| std::fs::OpenOptions::new().read(true).custom_flags(libc_nonblock()).open(p).ok()).collect();
            if files.is_empty() { return Err("no readable input device".into()); }
            Ok(Evdev { files, down: HashSet::new() })
        }
    }
    /// `struct input_event` on 64-bit Linux: 16 bytes of time, then type, code and value.
    pub fn apply_events(bytes: &[u8], wanted: &HashSet<u16>, down: &mut HashSet<u16>) {
        for e in bytes.chunks_exact(24) {
            let (ty, code, value) = (u16::from_ne_bytes([e[16], e[17]]), u16::from_ne_bytes([e[18], e[19]]), i32::from_ne_bytes([e[20], e[21], e[22], e[23]]));
            if ty != 1 || !wanted.contains(&code) { continue; } // EV_KEY, and only the keys asked for: the rest is not even kept
            match value { 0 => { down.remove(&code); } 1 => { down.insert(code); } _ => {} }
        }
    }
    impl Keys for Evdev {
        fn down(&mut self, wanted: &[String]) -> Result<Vec<String>, String> {
            let want: HashSet<u16> = wanted.iter().filter_map(|k| keys::find(k)).map(|k| k.linux).collect();
            let mut buf = [0u8; 24 * 64];
            self.files.retain_mut(|f| loop {
                match f.read(&mut buf) {
                    Ok(0) => break false,
                    Ok(n) => apply_events(&buf[..n], &want, &mut self.down),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break true,
                    Err(_) => break false, // unplugged
                }
            });
            if self.files.is_empty() { return Err("all input devices went away".into()); }
            self.down.retain(|c| want.contains(c));
            Ok(wanted.iter().filter(|k| keys::find(k).is_some_and(|x| self.down.contains(&x.linux))).cloned().collect())
        }
    }

    /// The X server's own idea of which keys are down. Under Wayland that is Xwayland's, which only knows about keys while an
    /// X11 window has focus: that covers Wine and Proton games, and not native Wayland ones. Mouse side buttons are not in it.
    pub struct X11 { c: x11rb::rust_connection::RustConnection }
    impl X11 {
        pub fn open() -> Result<X11, String> { x11rb::connect(None).map(|(c, _)| X11 { c }).map_err(|e| format!("no X server: {e}")) }
    }
    impl Keys for X11 {
        fn down(&mut self, wanted: &[String]) -> Result<Vec<String>, String> {
            use x11rb::protocol::xproto::ConnectionExt;
            let km = self.c.query_keymap().map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?.keys;
            Ok(wanted.iter().filter(|k| keys::find(k).is_some_and(|x| { let kc = x.linux as usize + 8; kc < 256 && km[kc / 8] & (1 << (kc % 8)) != 0 })).cloned().collect())
        }
    }

    pub fn open(s: &Settings) -> Result<Box<dyn Keys>, String> {
        if s.use_portal { return crate::portal::open(s).map(|b| Box::new(b) as Box<dyn Keys>); }
        if let Ok(e) = Evdev::open() { return Ok(Box::new(e)); }
        X11::open().map(|x| Box::new(x) as Box<dyn Keys>).map_err(|e| format!("{e}. On Wayland with no Xwayland, try the system shortcut option."))
    }
    pub fn info(s: &Settings) -> Info {
        if s.use_portal {
            return Info { backend: "portal", hold: true, toggle: true, mouse: false, permission: false,
                note: "The desktop asks you once to approve the keys, and you can change them there. Beware: KDE keeps a system shortcut for itself, so your game may stop receiving that key. Use it only for a key the game does not need, or use press to toggle if a hold is not seen.".into() };
        }
        if evdev_readable() {
            return Info { backend: "input devices", hold: true, toggle: true, mouse: true, permission: false, note: "Reading your input devices directly, without taking anything away from the game.".into() };
        }
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        if std::env::var_os("DISPLAY").is_some() {
            return Info { backend: "X11", hold: true, toggle: true, mouse: false, permission: false, note: if wayland {
                "Works while a Wine, Proton or other X11 window has focus, which covers most games. Native Wayland games cannot be seen this way; for those, add your user to the input group, or use the system shortcut option.".into()
            } else { "Works with every window on this X11 desktop. Mouse side buttons are not available.".into() } };
        }
        Info { backend: "none", hold: false, toggle: false, mouse: false, permission: false, note: "No way of seeing keys here. Try the system shortcut option, or add your user to the input group.".into() }
    }
    pub fn request_permission() -> bool { true }

    #[cfg(test)]
    mod tests {
        use super::*;
        fn ev(ty: u16, code: u16, value: i32) -> Vec<u8> { let mut b = vec![0u8; 16]; b.extend(ty.to_ne_bytes()); b.extend(code.to_ne_bytes()); b.extend(value.to_ne_bytes()); b }
        #[test]
        fn only_wanted_keys_are_kept() {
            let (want, mut down) = (HashSet::from([47u16]), HashSet::new());
            let bytes = [ev(1, 47, 1), ev(1, 30, 1), ev(0, 0, 0), ev(2, 47, 0)].concat();
            apply_events(&bytes, &want, &mut down);
            assert_eq!(down, HashSet::from([47]), "another key leaves no trace, and neither does a mouse move");
            apply_events(&ev(1, 47, 2), &want, &mut down);
            assert!(down.contains(&47), "auto-repeat is still down");
            apply_events(&ev(1, 47, 0), &want, &mut down);
            assert!(down.is_empty());
        }
        #[test]
        fn this_process_is_on_the_list() {
            let me = std::env::current_exe().unwrap();
            let name = me.file_name().unwrap().to_string_lossy().to_string();
            assert!(processes().iter().any(|p| name.starts_with(p.as_str()) || *p == name), "{name}");
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

    /// A snapshot of names from the system. No handle to any process is opened, so an anti-cheat has nothing to see.
    pub fn processes() -> Vec<String> {
        let mut out = std::collections::BTreeSet::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE { return vec![]; }
            let mut e: PROCESSENTRY32W = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut e) != 0;
            while ok {
                let n = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
                out.insert(String::from_utf16_lossy(&e.szExeFile[..n]));
                ok = Process32NextW(snap, &mut e) != 0;
            }
            CloseHandle(snap);
        }
        out.into_iter().collect()
    }
    /// Polling: no hook in the input path, so the game's input cannot be delayed or lost, whatever happens to this program.
    struct Poll;
    impl Keys for Poll {
        fn down(&mut self, wanted: &[String]) -> Result<Vec<String>, String> {
            Ok(wanted.iter().filter(|k| keys::find(k).is_some_and(|x| unsafe { GetAsyncKeyState(x.win as i32) } as u16 & 0x8000 != 0)).cloned().collect())
        }
    }
    pub fn open(_: &Settings) -> Result<Box<dyn Keys>, String> { Ok(Box::new(Poll)) }
    pub fn info(_: &Settings) -> Info {
        Info { backend: "key state polling", hold: true, toggle: true, mouse: true, permission: false, note: "Commons only reads whether your chosen keys are down. If a game runs as administrator, Commons may have to as well to see its keys.".into() }
    }
    pub fn request_permission() -> bool { true }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use crate::keys::MAC_BUTTON;
    use std::ffi::c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
        fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
        fn CGPreflightListenEventAccess() -> bool;
        fn CGRequestListenEventAccess() -> bool;
    }
    extern "C" {
        fn proc_listallpids(buffer: *mut c_void, size: i32) -> i32;
        fn proc_name(pid: i32, buffer: *mut c_void, size: u32) -> i32;
    }
    const HID: i32 = 1; // kCGEventSourceStateHIDSystemState

    /// Names from the kernel, by pid. No task port and nothing of the process is touched.
    pub fn processes() -> Vec<String> {
        let mut out = std::collections::BTreeSet::new();
        unsafe {
            let mut pids = vec![0i32; 8192];
            let n = proc_listallpids(pids.as_mut_ptr() as *mut c_void, (pids.len() * 4) as i32);
            for pid in pids.iter().take(n.max(0) as usize) {
                let mut b = [0u8; 256];
                let l = proc_name(*pid, b.as_mut_ptr() as *mut c_void, 256);
                if l > 0 { out.insert(String::from_utf8_lossy(&b[..l as usize]).to_string()); }
            }
        }
        out.into_iter().collect()
    }
    struct Poll;
    impl Keys for Poll {
        fn down(&mut self, wanted: &[String]) -> Result<Vec<String>, String> {
            if !unsafe { CGPreflightListenEventAccess() } { return Err("Input Monitoring is not allowed for Commons".into()); }
            Ok(wanted.iter().filter(|k| keys::find(k).is_some_and(|x| unsafe { if x.mac & MAC_BUTTON != 0 { CGEventSourceButtonState(HID, (x.mac & !MAC_BUTTON) as u32) } else { CGEventSourceKeyState(HID, x.mac) } })).cloned().collect())
        }
    }
    pub fn open(_: &Settings) -> Result<Box<dyn Keys>, String> {
        if !unsafe { CGPreflightListenEventAccess() } { return Err("Commons needs the Input Monitoring permission to see your game key. Press Allow in Game keys, then switch Commons on in System Settings, Privacy & Security, Input Monitoring.".into()); }
        Ok(Box::new(Poll))
    }
    pub fn info(_: &Settings) -> Info {
        let ok = unsafe { CGPreflightListenEventAccess() };
        Info { backend: "key state polling", hold: ok, toggle: ok, mouse: ok, permission: !ok, note: if ok { "Commons only reads whether your chosen keys are down.".into() } else { "macOS has to allow this first: Commons asks for Input Monitoring, and only ever looks at the keys you set here.".into() } }
    }
    /// Shows the system's own prompt once. True if already allowed.
    pub fn request_permission() -> bool { unsafe { CGRequestListenEventAccess() } }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod imp {
    use super::*;
    pub fn processes() -> Vec<String> { vec![] }
    pub fn open(_: &Settings) -> Result<Box<dyn Keys>, String> { Err("not available on this system".into()) }
    pub fn info(_: &Settings) -> Info { Info { backend: "none", hold: false, toggle: false, mouse: false, note: "Not available on this system.".into(), permission: false } }
    pub fn request_permission() -> bool { false }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_process_list_has_names_in_it() {
        let l = super::processes();
        assert!(l.len() > 1, "{l:?}");
        assert!(l.iter().all(|p| !p.is_empty()));
        let i = super::info(&Default::default());
        assert!(!i.note.is_empty());
    }
}
