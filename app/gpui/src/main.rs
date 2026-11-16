//! mega on GPUI: the desktop app without a web view. The core is linked directly and asked through the same
//! `App::call(method, args)` JSON API as the Tauri shell and the browser bridge (see core.rs).
mod app;
mod ascii;
mod backdrop;
mod bench;
#[cfg(feature = "calls")]
#[allow(dead_code)]
mod calls;
mod core;
mod callui;
mod deck;
mod popout;
mod video;
mod fonts;
mod model;
mod prefs;
mod presence;
mod views;
mod look;
#[cfg(feature = "calls")]
mod videobench;

use gpui_kit::*;

fn main() {
    bench::mark("main");
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

        let bounds = Bounds::centered(None, size(px(1100.), px(760.)), cx);
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(720.), px(520.))),
            titlebar: Some(TitlebarOptions { title: Some("mega".into()), ..Default::default() }),
            app_id: Some("app.commons.chat.gpui".into()),
            ..Default::default()
        };
        gpui_kit::open_window(opts, cx, |window, cx| cx.new(|cx| app::Mega::new(core, window, cx))).expect("window");
        cx.activate(true);
    });
}
