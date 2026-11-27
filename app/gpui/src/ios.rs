//! The iPhone build (spike): the same window as the desktop (app::Mega) on gpui-mobile's iOS platform
//! (vendor/gpui-mobile). app/ios/main.m calls `mega_ios_start` from the app delegate; gpui-mobile's own C entry points
//! (gpui_ios_get_window, gpui_ios_request_frame, ...) drive frames, touches and the keyboard from there.
//! Not here yet: the phone layout, safe areas, notifications, background audio for calls (IPHONE.md).
use gpui_kit::*;

/// Log lines and panics go to NSLog, so they show in the device console (Console.app, or `idevicesyslog`).
struct NsLog;
impl log::Log for NsLog {
    fn enabled(&self, m: &log::Metadata) -> bool { m.level() <= log::Level::Info }
    fn log(&self, r: &log::Record) { if self.enabled(r.metadata()) { nslog(&format!("mega [{}] {}", r.level(), r.args())); } }
    fn flush(&self) {}
}
static LOGGER: NsLog = NsLog;

fn nslog(msg: &str) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    extern "C" { fn NSLog(fmt: *mut AnyObject, ...); }
    let Ok(c) = std::ffi::CString::new(msg.replace('%', "%%")) else { return };
    unsafe {
        let s: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()];
        NSLog(s);
    }
}

/// Called once by the app delegate (main.m) before it asks gpui-mobile for the window.
#[unsafe(no_mangle)]
pub extern "C" fn mega_ios_start() {
    let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Info));
    crate::log_panics();
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| { nslog(&format!("mega PANIC: {info}")); prev(info); }));
    nslog(&format!("mega {} starting, data in {}", env!("CARGO_PKG_VERSION"), crate::core::data_dir().display()));
    gpui_mobile::ios::ffi::set_app_callback(Box::new(|cx: &mut App| {
        gpui_kit::init(cx);
        if let Err(e) = cx.text_system().add_fonts(crate::fonts::all()) { nslog(&format!("fonts: {e}")); }
        crate::look::apply_theme(cx);
        let core = crate::core::Core::open();
        // a phone window is the whole screen: no bounds, no title bar
        let opts = WindowOptions { window_bounds: None, ..Default::default() };
        if let Err(e) = gpui_kit::open_window(opts, cx, |window, cx| cx.new(|cx| crate::app::Mega::new(core, window, cx))) {
            nslog(&format!("mega: the window did not open: {e:#}"));
        }
        cx.activate(true);
    }));
    gpui_mobile::ios::ffi::run_app();
}
