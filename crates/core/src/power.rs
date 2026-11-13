//! Whether this device is running on battery, for the app's "Battery saver: on battery" setting.
//! None when it cannot tell (a desktop without a battery, or a system this does not read): the app then treats it as
//! mains power, so the saver only switches itself on where it is sure.

pub fn on_battery() -> Option<bool> { imp() }

// A laptop battery that is discharging. Batteries of mice, headsets and pads report scope "Device" and are skipped.
#[cfg(target_os = "linux")]
fn imp() -> Option<bool> {
    let mut seen = false;
    for e in std::fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
        let p = e.path();
        let read = |f: &str| std::fs::read_to_string(p.join(f)).map(|s| s.trim().to_string()).unwrap_or_default();
        if read("type") != "Battery" || read("scope") == "Device" { continue; }
        seen = true;
        if read("status") == "Discharging" { return Some(true); }
    }
    if seen { Some(false) } else { None }
}

// pmset's first line names the power source: "Now drawing from 'Battery Power'" or "'AC Power'".
#[cfg(target_os = "macos")]
fn imp() -> Option<bool> {
    let out = std::process::Command::new("/usr/bin/pmset").args(["-g", "ps"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.lines().next()?;
    if first.contains("Battery Power") { Some(true) } else if first.contains("AC Power") { Some(false) } else { None }
}

#[cfg(target_os = "windows")]
fn imp() -> Option<bool> {
    // GetSystemPowerStatus: ACLineStatus 0 = offline (battery), 1 = online, 255 = unknown; BatteryFlag 128 = no battery.
    #[repr(C)]
    #[allow(non_snake_case)]
    struct SystemPowerStatus { ACLineStatus: u8, BatteryFlag: u8, BatteryLifePercent: u8, SystemStatusFlag: u8, BatteryLifeTime: u32, BatteryFullLifeTime: u32 }
    #[link(name = "kernel32")]
    extern "system" { fn GetSystemPowerStatus(s: *mut SystemPowerStatus) -> i32; }
    let mut s = SystemPowerStatus { ACLineStatus: 255, BatteryFlag: 255, BatteryLifePercent: 255, SystemStatusFlag: 0, BatteryLifeTime: 0, BatteryFullLifeTime: 0 };
    if unsafe { GetSystemPowerStatus(&mut s) } == 0 || s.BatteryFlag == 128 { return None; }
    match s.ACLineStatus { 0 => Some(true), 1 => Some(false), _ => None }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn imp() -> Option<bool> { None }

#[cfg(test)]
mod tests {
    #[test]
    fn answers_without_panicking() { let _ = super::on_battery(); }
}
