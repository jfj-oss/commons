//! The window could not be opened. On Linux that nearly always means no working Vulkan driver (GPUI draws through
//! Vulkan only, with no GL fallback), and without this the app simply exited with a panic nobody sees. Now: the reason
//! in plain words on stderr, in `logs/no-window.txt` beside the panic log, and (unless `MEGA_GPUI_NO_DIALOG` is set, for
//! tests) in a message box from whichever of kdialog, zenity or notify-send exists. Then the process ends with status 1.
use std::path::Path;

/// The text shown to the person. `err` is what the window system said, shown last for whoever has to look into it.
pub fn message(err: &str) -> String {
    let what = if cfg!(target_os = "linux") {
        "mega could not open its window.\n\nOn Linux it needs a working Vulkan graphics driver, and none was found. \
Install your graphics card's Vulkan driver (AMD: vulkan-radeon / mesa-vulkan-drivers; Intel: vulkan-intel / mesa-vulkan-drivers; \
NVIDIA: the proprietary driver with its Vulkan files), then check that the command vulkaninfo lists your card and start mega again.\n\n\
A virtual machine needs 3D acceleration switched on. Nothing was changed on your computer."
    } else {
        "mega could not open its window. Your graphics system did not start; restart the computer and try again."
    };
    format!("{what}\n\nDetails: {err}")
}

/// Reports `err` and ends the process. `data` is the app's data folder (the log goes in `logs/`).
pub fn fail(err: &str, data: &Path) -> ! {
    let text = message(err);
    eprintln!("{text}");
    let dir = data.join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("no-window.txt"), format!("{text}\n"));
    if std::env::var_os("MEGA_GPUI_NO_DIALOG").is_none() { dialog(&text); }
    std::process::exit(1)
}

fn dialog(text: &str) {
    use std::process::{Command, Stdio};
    let try_cmd = |cmd: &str, args: &[&str]| Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().is_ok();
    #[cfg(target_os = "macos")]
    { let _ = try_cmd("/usr/bin/osascript", &["-e", &format!("display dialog {:?} with title \"mega\" buttons {{\"OK\"}} default button 1 with icon stop", text)]); }
    #[cfg(not(target_os = "macos"))]
    {
        // a dialog stays up after this process has gone
        let _ = try_cmd("kdialog", &["--title", "mega", "--error", text])
            || try_cmd("zenity", &["--error", "--title=mega", "--no-markup", "--width=480", &format!("--text={text}")])
            || try_cmd("notify-send", &["--urgency=critical", "mega could not open its window", text]);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn says_what_is_needed() {
        let m = super::message("No GPU adapters found");
        assert!(m.contains("Details: No GPU adapters found"));
        if cfg!(target_os = "linux") { assert!(m.contains("Vulkan") && m.contains("vulkaninfo")); }
    }
}
