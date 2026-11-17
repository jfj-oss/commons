//! Real input for a window inside a PRIVATE headless KWin (never the desktop's): key, pointer and wheel events sent
//! through KWin's fake-input protocol, so they reach the app as a keyboard and a mouse would (seat, serials, xkb).
//! It refuses to run unless WAYLAND_DISPLAY names the test compositor.
//!   fakekeys type:hello key:shift+enter key:ctrl+v key:enter move:400,300 click wheel:15x200@16 sleep:500
use wayland_client::{protocol::wl_registry, Connection, Dispatch, QueueHandle};
use wayland_protocols_plasma::fake_input::client::org_kde_kwin_fake_input::OrgKdeKwinFakeInput;

struct S { fake: Option<OrgKdeKwinFakeInput> }
impl Dispatch<wl_registry::WlRegistry, ()> for S {
    fn event(s: &mut Self, reg: &wl_registry::WlRegistry, ev: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = ev {
            if interface == "org_kde_kwin_fake_input" { s.fake = Some(reg.bind(name, version.min(4), qh, ())); }
        }
    }
}
impl Dispatch<OrgKdeKwinFakeInput, ()> for S {
    fn event(_: &mut Self, _: &OrgKdeKwinFakeInput, _: <OrgKdeKwinFakeInput as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

/// evdev key codes, US layout.
fn code(k: &str) -> Option<(u32, bool)> {
    const ROWS: [(&str, u32); 4] = [("1234567890", 2), ("qwertyuiop", 16), ("asdfghjkl", 30), ("zxcvbnm", 44)];
    Some(match k {
        "enter" => (28, false), "space" | " " => (57, false), "backspace" => (14, false), "tab" => (15, false), "escape" => (1, false),
        "shift" => (42, false), "ctrl" => (29, false), "alt" => (56, false), "up" => (103, false), "down" => (108, false), "pageup" => (104, false), "pagedown" => (109, false),
        "end" => (107, false), "left" => (105, false), "right" => (106, false), "`" => (41, false), "home" => (102, false), "-" => (12, false), "." => (52, false), "," => (51, false), "!" => (2, true), "?" => (53, true),
        _ => {
            let c = k.chars().next()?;
            let lower = c.to_ascii_lowercase();
            let (row, base) = ROWS.iter().find(|(r, _)| r.contains(lower))?;
            (base + row.find(lower)? as u32, c.is_ascii_uppercase())
        }
    })
}

fn main() {
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
    assert!(display == "wayland-megatest" || display == "wayland-uishot" || display == "wayland-updshot" || display == "wayland-selshot", "fakekeys only talks to a private test compositor (WAYLAND_DISPLAY=wayland-megatest, wayland-uishot or wayland-updshot), not {display:?}");
    let conn = Connection::connect_to_env().expect("compositor");
    let mut q = conn.new_event_queue::<S>();
    let qh = q.handle();
    conn.display().get_registry(&qh, ());
    let mut s = S { fake: None };
    q.roundtrip(&mut s).unwrap();
    let f = s.fake.clone().expect("this compositor has no fake input");
    f.authenticate("mega-gpui checks".into(), "driving a test window".into());
    q.roundtrip(&mut s).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(150));
    let flush = |q: &mut wayland_client::EventQueue<S>, s: &mut S| { q.roundtrip(s).unwrap(); };
    let ms = |n: u64| std::thread::sleep(std::time::Duration::from_millis(n));
    let tap = |f: &OrgKdeKwinFakeInput, c: u32, shift: bool| { if shift { f.keyboard_key(42, 1); let _ = conn.flush(); std::thread::sleep(std::time::Duration::from_millis(15)); } f.keyboard_key(c, 1); f.keyboard_key(c, 0); if shift { f.keyboard_key(42, 0); } };
    for a in std::env::args().skip(1) {
        let (cmd, arg) = a.split_once(':').unwrap_or((a.as_str(), ""));
        match cmd {
            "type" => for c in arg.chars() { let (k, sh) = code(&c.to_string()).unwrap_or_else(|| panic!("no key for {c:?}")); tap(&f, k, sh); flush(&mut q, &mut s); ms(12); },
            "key" => {
                let parts: Vec<&str> = arg.split('+').collect();
                let codes: Vec<u32> = parts.iter().map(|p| code(p).unwrap_or_else(|| panic!("no key {p}")).0).collect();
                for c in &codes { f.keyboard_key(*c, 1); }
                for c in codes.iter().rev() { f.keyboard_key(*c, 0); }
                flush(&mut q, &mut s); ms(30);
            }
            "move" => { let (x, y) = arg.split_once(',').unwrap(); f.pointer_motion_absolute(x.parse().unwrap(), y.parse().unwrap()); flush(&mut q, &mut s); ms(30); }
            "dblclick" => { for _ in 0..2 { f.button(0x110, 1); f.button(0x110, 0); flush(&mut q, &mut s); ms(60); } }
            "mousedown" => { f.button(0x110, 1); flush(&mut q, &mut s); ms(60); }
            "mouseup" => { f.button(0x110, 0); flush(&mut q, &mut s); ms(60); }
            "keydown" => { f.keyboard_key(code(arg).unwrap_or_else(|| panic!("no key {arg}")).0, 1); flush(&mut q, &mut s); ms(30); }
            "keyup" => { f.keyboard_key(code(arg).unwrap_or_else(|| panic!("no key {arg}")).0, 0); flush(&mut q, &mut s); ms(30); }
            "rclick" => { f.button(0x111, 1); f.button(0x111, 0); flush(&mut q, &mut s); ms(60); }
            "click" => { f.button(0x110, 1); f.button(0x110, 0); flush(&mut q, &mut s); ms(60); }
            // wheel:<value>x<count>@<interval-ms>: vertical scroll, negative is up
            "wheel" => {
                let (v, rest) = arg.split_once('x').unwrap();
                let (n, iv) = rest.split_once('@').unwrap();
                let (v, n, iv): (f64, usize, u64) = (v.parse().unwrap(), n.parse().unwrap(), iv.parse().unwrap());
                let t0 = std::time::Instant::now();
                for i in 0..n {
                    f.axis(0, v); flush(&mut q, &mut s);
                    let due = std::time::Duration::from_millis(iv * (i as u64 + 1));
                    if let Some(d) = due.checked_sub(t0.elapsed()) { std::thread::sleep(d); }
                }
            }
            "sleep" => ms(arg.parse().unwrap()),
            _ => panic!("unknown step {a}"),
        }
    }
    flush(&mut q, &mut s);
}
