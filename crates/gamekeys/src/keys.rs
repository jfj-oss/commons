//! Key names: `KeyboardEvent.code`, which is what the settings page gets when you press a key, and what each system calls it.
//! Columns: Linux input code (an X11 keycode is that plus 8), Windows virtual key, macOS virtual key.
pub struct Key { pub code: &'static str, pub linux: u16, pub win: u16, pub mac: u16 }
const fn k(code: &'static str, linux: u16, win: u16, mac: u16) -> Key { Key { code, linux, win, mac } }
/// `mac` holds a mouse button number here, not a key.
pub const MAC_BUTTON: u16 = 0x8000;

pub const KEYS: &[Key] = &[
    k("KeyA", 30, 0x41, 0x00), k("KeyB", 48, 0x42, 0x0B), k("KeyC", 46, 0x43, 0x08), k("KeyD", 32, 0x44, 0x02), k("KeyE", 18, 0x45, 0x0E),
    k("KeyF", 33, 0x46, 0x03), k("KeyG", 34, 0x47, 0x05), k("KeyH", 35, 0x48, 0x04), k("KeyI", 23, 0x49, 0x22), k("KeyJ", 36, 0x4A, 0x26),
    k("KeyK", 37, 0x4B, 0x28), k("KeyL", 38, 0x4C, 0x25), k("KeyM", 50, 0x4D, 0x2E), k("KeyN", 49, 0x4E, 0x2D), k("KeyO", 24, 0x4F, 0x1F),
    k("KeyP", 25, 0x50, 0x23), k("KeyQ", 16, 0x51, 0x0C), k("KeyR", 19, 0x52, 0x0F), k("KeyS", 31, 0x53, 0x01), k("KeyT", 20, 0x54, 0x11),
    k("KeyU", 22, 0x55, 0x20), k("KeyV", 47, 0x56, 0x09), k("KeyW", 17, 0x57, 0x0D), k("KeyX", 45, 0x58, 0x07), k("KeyY", 21, 0x59, 0x10), k("KeyZ", 44, 0x5A, 0x06),
    k("Digit1", 2, 0x31, 0x12), k("Digit2", 3, 0x32, 0x13), k("Digit3", 4, 0x33, 0x14), k("Digit4", 5, 0x34, 0x15), k("Digit5", 6, 0x35, 0x17),
    k("Digit6", 7, 0x36, 0x16), k("Digit7", 8, 0x37, 0x1A), k("Digit8", 9, 0x38, 0x1C), k("Digit9", 10, 0x39, 0x19), k("Digit0", 11, 0x30, 0x1D),
    k("F1", 59, 0x70, 0x7A), k("F2", 60, 0x71, 0x78), k("F3", 61, 0x72, 0x63), k("F4", 62, 0x73, 0x76), k("F5", 63, 0x74, 0x60), k("F6", 64, 0x75, 0x61),
    k("F7", 65, 0x76, 0x62), k("F8", 66, 0x77, 0x64), k("F9", 67, 0x78, 0x65), k("F10", 68, 0x79, 0x6D), k("F11", 87, 0x7A, 0x67), k("F12", 88, 0x7B, 0x6F),
    k("F13", 183, 0x7C, 0x69), k("F14", 184, 0x7D, 0x6B), k("F15", 185, 0x7E, 0x71), k("F16", 186, 0x7F, 0x6A), k("F17", 187, 0x80, 0x40), k("F18", 188, 0x81, 0x4F), k("F19", 189, 0x82, 0x50),
    k("Space", 57, 0x20, 0x31), k("Tab", 15, 0x09, 0x30), k("CapsLock", 58, 0x14, 0x39), k("Enter", 28, 0x0D, 0x24), k("Backspace", 14, 0x08, 0x33), k("Escape", 1, 0x1B, 0x35),
    k("ShiftLeft", 42, 0xA0, 0x38), k("ShiftRight", 54, 0xA1, 0x3C), k("ControlLeft", 29, 0xA2, 0x3B), k("ControlRight", 97, 0xA3, 0x3E), k("AltLeft", 56, 0xA4, 0x3A), k("AltRight", 100, 0xA5, 0x3D),
    k("Backquote", 41, 0xC0, 0x32), k("Minus", 12, 0xBD, 0x1B), k("Equal", 13, 0xBB, 0x18), k("BracketLeft", 26, 0xDB, 0x21), k("BracketRight", 27, 0xDD, 0x1E), k("Backslash", 43, 0xDC, 0x2A),
    k("Semicolon", 39, 0xBA, 0x29), k("Quote", 40, 0xDE, 0x27), k("Comma", 51, 0xBC, 0x2B), k("Period", 52, 0xBE, 0x2F), k("Slash", 53, 0xBF, 0x2C),
    k("ArrowUp", 103, 0x26, 0x7E), k("ArrowDown", 108, 0x28, 0x7D), k("ArrowLeft", 105, 0x25, 0x7B), k("ArrowRight", 106, 0x27, 0x7C),
    k("Insert", 110, 0x2D, 0x72), k("Delete", 111, 0x2E, 0x75), k("Home", 102, 0x24, 0x73), k("End", 107, 0x23, 0x77), k("PageUp", 104, 0x21, 0x74), k("PageDown", 109, 0x22, 0x79),
    k("Numpad0", 82, 0x60, 0x52), k("Numpad1", 79, 0x61, 0x53), k("Numpad2", 80, 0x62, 0x54), k("Numpad3", 81, 0x63, 0x55), k("Numpad4", 75, 0x64, 0x56),
    k("Numpad5", 76, 0x65, 0x57), k("Numpad6", 77, 0x66, 0x58), k("Numpad7", 71, 0x67, 0x59), k("Numpad8", 72, 0x68, 0x5B), k("Numpad9", 73, 0x69, 0x5C),
    k("Mouse4", 0x113, 0x05, MAC_BUTTON | 3), k("Mouse5", 0x114, 0x06, MAC_BUTTON | 4),
];
pub fn find(code: &str) -> Option<&'static Key> { KEYS.iter().find(|k| k.code == code) }
pub fn known(code: &str) -> bool { find(code).is_some() }

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    #[test]
    fn no_key_is_listed_twice_on_any_system() {
        for f in [|k: &Key| k.linux, |k: &Key| k.win, |k: &Key| k.mac] {
            let mut seen = HashSet::new();
            for k in KEYS { assert!(seen.insert(f(k)), "{} clashes", k.code); }
        }
        assert_eq!(KEYS.iter().map(|k| k.code).collect::<HashSet<_>>().len(), KEYS.len());
    }
    #[test]
    fn spot_checks_against_the_headers() {
        // linux/input-event-codes.h, WinUser.h, HIToolbox/Events.h
        let v = find("KeyV").unwrap(); assert_eq!((v.linux, v.win, v.mac), (47, 0x56, 9));
        let c = find("CapsLock").unwrap(); assert_eq!((c.linux, c.win, c.mac), (58, 0x14, 57));
        let s = find("Space").unwrap(); assert_eq!((s.linux, s.win, s.mac), (57, 0x20, 49));
    }
}
