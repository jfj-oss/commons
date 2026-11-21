//! mega on GPUI: the desktop app without a web view. The core is linked directly and asked through the same
//! `App::call(method, args)` JSON API as the Tauri shell and the browser bridge (see core.rs).
mod app;
mod arrive;
mod avatars;
mod reading;
mod select;
mod ascii;
mod backdrop;
mod typing;
mod bench;
mod cards;
mod nav;
mod rail;
#[cfg(feature = "calls")]
#[allow(dead_code)]
mod calls;
mod core;
mod callui;
mod calldock;
mod callsettings;
mod icons;
mod vtext;
mod deck;
mod popout;
mod video;
mod fonts;
mod files;
mod media;
mod playback;
mod xfer;
mod linen;
mod members;
mod squircle;
mod model;
mod notifier;
mod ringui;
mod runebar;
mod uitest;
mod prefs;
mod update;
mod updateui;
mod presence;
mod views;
mod msgact;
mod rich;
mod look;
mod paint;
mod wall;
#[cfg(feature = "calls")]
mod videobench;

use gpui_kit::*;

/// Every panic is written to `panic.log` (message, place, backtrace) before the process goes: on macOS a panic inside an
/// AppKit callback aborts at once and the crash report never carries the message.
fn log_panics() {
    let dir = core::data_dir().join("logs");
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = std::fs::create_dir_all(&dir);
        let bt = std::backtrace::Backtrace::force_capture();
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        let line = format!("==== {} v{} thread {thread}\n{info}\n{bt}\n", now_text(), env!("CARGO_PKG_VERSION"));
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("panic.log")) { let _ = f.write_all(line.as_bytes()); }
        prev(info);
    }));
}
fn now_text() -> String {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("unix {s}")
}

fn main() {
    log_panics();
    bench::mark("main");
    // a new version that keeps failing to start is replaced by the previous one (update.rs)
    update::guard_start(&core::data_dir());
    #[cfg(feature = "calls")]
    if calls::headless::wanted() { calls::headless::run(); return; }
    gpui_kit::application().run(|cx: &mut App| {
        gpui_kit::init(cx);
        if let Err(e) = cx.text_system().add_fonts(fonts::all()) { eprintln!("fonts: {e}"); }
        if std::env::var_os("MEGA_GPUI_FONTS").is_some() { for n in cx.text_system().all_font_names() { if ["FK", "Figtree", "Plex", "Bellefair"].iter().any(|k| n.contains(k)) { eprintln!("font: {n}"); } } }
        look::apply_theme(cx);
        cx.on_window_closed(|cx, _| { if cx.windows().is_empty() { cx.quit(); } }).detach();

        #[cfg(feature = "calls")]
        if let Some(secs) = videobench::wanted() {
            let bounds = Bounds::centered(None, size(px(1280.), px(720.)), cx);
            let opts = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), app_id: Some("app.commons.chat.gpui.videobench".into()), ..Default::default() };
            gpui_kit::open_window(opts, cx, |window, cx| cx.new(|cx| videobench::VideoBench::new(secs, window, cx))).expect("window");
            cx.activate(true);
            return;
        }
        let core = core::Core::open();
        let quit_core = core.clone();
        cx.on_app_quit(move |_| { let c = quit_core.clone(); async move { c.app.shutdown(); } }).detach();

        // tests only: MEGA_GPUI_SIZE=1280x900 opens the window at that size (the window's content, without a title bar)
        let (ww, wh) = std::env::var("MEGA_GPUI_SIZE").ok().and_then(|v| { let (a, b) = v.split_once('x')?; Some((a.parse::<f32>().ok()?, b.parse::<f32>().ok()?)) }).unwrap_or((1100., 760.));
        let bounds = Bounds::centered(None, size(px(ww), px(wh)), cx);
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(ww.min(720.)), px(wh.min(520.)))),
            titlebar: Some(TitlebarOptions { title: Some("mega".into()), ..Default::default() }),
            app_id: Some("app.commons.chat.gpui".into()),
            ..Default::default()
        };
        gpui_kit::open_window(opts, cx, |window, cx| cx.new(|cx| app::Mega::new(core, window, cx))).expect("window");
        cx.activate(true);
    });
}
