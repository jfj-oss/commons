//! macOS notifications with a click that opens the room, and a quick reply.
//!
//! The core's notify hub decides what is worth a note (as on Linux, notifier.rs) and hands it to this sink. It uses
//! `UNUserNotificationCenter`, the system's own notification API (the old `NSUserNotificationCenter` that notify-rust and
//! mac-notification-sys use is gone on current macOS, and its click only ever brought the app forward without saying which
//! room). The app is the center's delegate: `didReceiveNotificationResponse` is called on the main thread when a note
//! is clicked, and puts `Inbound::Open(room)` in the inbox the window reads, exactly as the Linux notifier does. A note
//! carries a "Reply" text action (category `mega-message`); what is typed is sent into the room through the core.
//!
//! The room travels in the request's identifier: `room:<gid>` for a message (a newer note of the room replaces the
//! older, as on the web), `<tag>\x1f<gid>` for a plain note about a room. Needs a bundle (`mega.app`): outside one the
//! system's center is not there, and nothing is shown and nothing breaks.
use crate::notifier::{Bus, Inbound, Inbox, Reply};
use block2::RcBlock;
use commons_core::notify::{Note, Sink};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol, NSSet, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction, UNNotificationActionOptions,
    UNNotificationCategory, UNNotificationCategoryOptions, UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNTextInputNotificationAction, UNTextInputNotificationResponse, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use std::ptr::NonNull;
use std::sync::Arc;

const CATEGORY: &str = "mega-message";
const REPLY: &str = "mega-reply";

/// The room a request identifier names, if it names one.
pub fn room_of(identifier: &str) -> Option<String> {
    if let Some(g) = identifier.strip_prefix("room:") { return Some(g.to_string()).filter(|g| !g.is_empty()); }
    identifier.split_once('\x1f').map(|(_, g)| g.to_string()).filter(|g| !g.is_empty())
}
/// The identifier of a request: the tag, with the room after it when the tag does not already say it.
pub fn identifier_of(tag: &str, gid: Option<&str>) -> String {
    match gid { Some(g) if !tag.starts_with("room:") => format!("{tag}\x1f{g}"), _ => tag.to_string() }
}

/// Whether this process is inside an app bundle (the center only exists then).
fn in_bundle() -> bool {
    use objc2_foundation::NSBundle;
    NSBundle::mainBundle().bundleIdentifier().is_some() && NSBundle::mainBundle().bundlePath().to_string().ends_with(".app")
}

struct Ivars { inbox: Arc<Inbox>, reply: Reply }

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and `Delegate` has no Drop.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        /// The note arrives while mega is the front app: still show it (the core only sends one when no window is looked at).
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(&self, _center: &UNUserNotificationCenter, _n: &UNNotification, done: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>) {
            done.call((UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List,));
        }

        /// A click on the note, or its Reply.
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(&self, _center: &UNUserNotificationCenter, response: &UNNotificationResponse, done: &block2::DynBlock<dyn Fn()>) {
            let id = response.notification().request().identifier().to_string();
            let action = response.actionIdentifier().to_string();
            let gid = room_of(&id);
            let iv = self.ivars();
            let any: &AnyObject = response.as_ref();
            if let (Some(t), Some(g)) = (any.downcast_ref::<UNTextInputNotificationResponse>(), gid.clone()) {
                let text = t.userText().to_string();
                eprintln!("notification reply to room {g}");
                let (reply, inbox) = (iv.reply.clone(), iv.inbox.clone());
                std::thread::spawn(move || { let ok = reply(&g, &text).is_ok(); inbox.push(Inbound::Replied { gid: g, ok }); });
            } else if !action.ends_with("DismissActionIdentifier") {
                eprintln!("notification clicked: room {}", gid.as_deref().unwrap_or("-"));
                iv.inbox.push(Inbound::Open(gid));
            }
            done.call(());
        }
    }
);

/// The notification sink for macOS. Cheap to share.
pub struct Notifier { _live: bool }

// The delegate lives as long as the process; the center keeps only a weak reference to it.
struct Keep(#[allow(dead_code)] Retained<Delegate>);
unsafe impl Send for Keep {}
unsafe impl Sync for Keep {}
static DELEGATE: std::sync::OnceLock<Keep> = std::sync::OnceLock::new();

impl Notifier {
    /// Called on the main thread (the app's start): sets the delegate, the Reply category and asks for permission.
    pub fn start(_bus: Bus, reply: Reply, inbox: Arc<Inbox>) -> Arc<Notifier> {
        let (Some(mtm), true) = (MainThreadMarker::new(), in_bundle()) else {
            eprintln!("notifications: not started from the main thread of an app bundle; none will be shown");
            return Arc::new(Notifier { _live: false });
        };
        let d = Delegate::alloc(mtm).set_ivars(Ivars { inbox, reply });
        let d: Retained<Delegate> = unsafe { msg_send![super(d), init] };
        let center = UNUserNotificationCenter::currentNotificationCenter();
        center.setDelegate(Some(ProtocolObject::from_ref(&*d)));
        let act = UNTextInputNotificationAction::actionWithIdentifier_title_options_textInputButtonTitle_textInputPlaceholder(
            &NSString::from_str(REPLY), &NSString::from_str("Reply"), UNNotificationActionOptions::empty(), &NSString::from_str("Send"), &NSString::from_str("Reply"));
        let actions = NSArray::from_retained_slice(&[Retained::into_super(act)]);
        let cat = UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(&NSString::from_str(CATEGORY), &actions, &NSArray::<NSString>::new(), UNNotificationCategoryOptions::empty());
        center.setNotificationCategories(&NSSet::from_retained_slice(&[cat]));
        let done = RcBlock::new(|granted: Bool, err: *mut NSError| {
            if let Some(e) = NonNull::new(err) { eprintln!("notifications: permission request failed: {}", unsafe { e.as_ref() }.localizedDescription()); }
            else { eprintln!("notifications: permission {}", if granted.as_bool() { "granted" } else { "not granted" }); }
        });
        center.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound, &done);
        let _ = DELEGATE.set(Keep(d));
        Arc::new(Notifier { _live: true })
    }

    fn post(&self, tag: &str, title: &str, body: &str, gid: Option<&str>, reply: bool) {
        if !self._live { return; }
        let c = UNMutableNotificationContent::new();
        c.setTitle(&NSString::from_str(title));
        c.setBody(&NSString::from_str(body));
        if let Some(g) = gid { c.setThreadIdentifier(&NSString::from_str(g)); }
        if reply && gid.is_some() { c.setCategoryIdentifier(&NSString::from_str(CATEGORY)); }
        let req = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(&identifier_of(tag, gid)), &c, None);
        let done = RcBlock::new(|err: *mut NSError| { if let Some(e) = NonNull::new(err) { eprintln!("notifications: {}", unsafe { e.as_ref() }.localizedDescription()); } });
        UNUserNotificationCenter::currentNotificationCenter().addNotificationRequest_withCompletionHandler(&req, Some(&done));
    }

    /// A note that is not a message: a click opens the window (and the room, when `gid` is given).
    pub fn show_plain(&self, tag: &str, title: &str, body: &str, gid: Option<&str>) { self.post(tag, title, body, gid, false); }
    /// Takes down what was shown (a ring that was answered).
    pub fn close_all(&self) { if self._live { UNUserNotificationCenter::currentNotificationCenter().removeAllDeliveredNotifications(); } }
}

impl Sink for Notifier {
    fn show(&self, note: &Note) { self.post(&format!("room:{}", note.gid), &note.title, &note.body, Some(&note.gid), true); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identifiers_carry_the_room() {
        assert_eq!(room_of(&identifier_of("room:abc", Some("abc"))).as_deref(), Some("abc"));
        assert_eq!(room_of(&identifier_of("ring:1", Some("g9"))).as_deref(), Some("g9"));
        assert_eq!(room_of(&identifier_of("missed", None)), None);
    }
}
