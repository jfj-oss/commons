//! Who decides whether the microphone is on: a port of `app/ui/mic-gate.js`. Two voices, the person (manual) and a game
//! key (game). Either one can keep it off, and neither can overrule the other's "off": a game key letting go never
//! unmutes someone who muted themselves. Changes go out one at a time, and only the latest wish counts, so a fast press
//! and release cannot end on the wrong state.
use futures::future::BoxFuture;
use std::sync::Arc;

/// Switches the microphone: `true` is on. Errors are the publisher's own message.
pub type Publish = Arc<dyn Fn(bool) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

#[derive(Default)]
struct State { manual: bool, game: bool, sent: Option<bool>, ready: bool }
impl State { fn off(&self) -> bool { self.manual || self.game } }

pub struct MicGate {
    st: parking_lot::Mutex<State>,
    /// Held while a change is going out; a later wish waits its turn and then sends only if it still differs.
    busy: tokio::sync::Mutex<()>,
    publish: Publish,
}

impl MicGate {
    pub fn new(publish: Publish) -> MicGate { MicGate { st: Default::default(), busy: tokio::sync::Mutex::new(()), publish } }
    pub fn off(&self) -> bool { self.st.lock().off() }
    pub fn manual(&self) -> bool { self.st.lock().manual }
    pub fn game(&self) -> bool { self.st.lock().game }
    /// Set the flags without sending anything (a fresh session starts from what the person had).
    pub fn preset(&self, manual: bool, game: bool) { let mut s = self.st.lock(); s.manual = manual; s.game = game; }

    pub async fn set_manual(&self, m: bool) -> Result<(), String> { self.st.lock().manual = m; self.flush().await }
    pub async fn set_game(&self, g: bool) -> Result<(), String> { self.st.lock().game = g; self.flush().await }
    /// The helper or the app went away: nothing is known about the keys, so they hold nothing.
    pub async fn lost(&self) -> Result<(), String> { self.set_game(false).await }
    /// There is a mic to talk to from now on (joined).
    pub async fn open(&self) -> Result<(), String> { { let mut s = self.st.lock(); s.ready = true; s.sent = None; } self.flush().await }
    /// No mic any more (left).
    pub fn close(&self) { let mut s = self.st.lock(); s.ready = false; s.game = false; s.sent = None; }

    pub async fn flush(&self) -> Result<(), String> {
        let _turn = self.busy.lock().await;
        loop {
            let want_off = {
                let mut s = self.st.lock();
                if !s.ready || s.sent == Some(s.off()) { return Ok(()); }
                let off = s.off();
                s.sent = Some(off);
                off
            };
            if let Err(e) = (self.publish)(!want_off).await { self.st.lock().sent = None; return Err(e); }
        }
    }
}

#[cfg(test)]
mod tests {
    // tools/mic-gate.test.mjs, case for case
    use super::*;
    use std::time::Duration;

    fn gate(ms: u64) -> (Arc<MicGate>, Arc<parking_lot::Mutex<Vec<bool>>>) {
        let sent: Arc<parking_lot::Mutex<Vec<bool>>> = Default::default();
        let s2 = sent.clone();
        let g = MicGate::new(Arc::new(move |on| { let s = s2.clone(); Box::pin(async move { s.lock().push(on); if ms > 0 { tokio::time::sleep(Duration::from_millis(ms)).await; } Ok(()) }) }));
        (Arc::new(g), sent)
    }
    fn mic_on(s: &Arc<parking_lot::Mutex<Vec<bool>>>) -> bool { *s.lock().last().unwrap() }

    #[tokio::test]
    async fn a_game_key_mutes_and_letting_go_comes_back() {
        let (g, sent) = gate(0); g.open().await.unwrap();
        g.set_game(true).await.unwrap(); assert!(!mic_on(&sent));
        g.set_game(false).await.unwrap(); assert!(mic_on(&sent));
    }
    #[tokio::test]
    async fn a_manual_mute_outlives_a_game_key() {
        let (g, sent) = gate(0); g.open().await.unwrap();
        g.set_manual(true).await.unwrap(); g.set_game(true).await.unwrap(); g.set_game(false).await.unwrap();
        assert!(!mic_on(&sent), "still muted: the person said so");
    }
    #[tokio::test]
    async fn muting_by_hand_during_a_game_key_stays_after_it() {
        let (g, sent) = gate(0); g.open().await.unwrap();
        g.set_game(true).await.unwrap(); g.set_manual(true).await.unwrap(); g.set_game(false).await.unwrap();
        assert!(!mic_on(&sent));
    }
    #[tokio::test]
    async fn unmuting_by_hand_while_a_key_is_held_keeps_the_mic_off_until_it_lets_go() {
        let (g, sent) = gate(0); g.open().await.unwrap();
        g.set_manual(true).await.unwrap(); g.set_game(true).await.unwrap(); g.set_manual(false).await.unwrap();
        assert!(!mic_on(&sent));
        g.set_game(false).await.unwrap(); assert!(mic_on(&sent));
    }
    #[tokio::test]
    async fn losing_the_helper_goes_back_to_what_the_person_had_set() {
        for manual in [false, true] {
            let (g, sent) = gate(0); g.open().await.unwrap();
            g.set_manual(manual).await.unwrap(); g.set_game(true).await.unwrap(); g.lost().await.unwrap();
            assert_eq!(mic_on(&sent), !manual);
        }
    }
    #[tokio::test]
    async fn a_fast_press_and_release_ends_on_the_last_wish_in_order() {
        let (g, sent) = gate(5); g.open().await.unwrap();
        // polled in this order, so the wishes land in this order while the first switch is still going out
        let _ = futures::join!(g.set_game(true), g.set_game(false), g.set_game(true), g.set_game(false));
        g.flush().await.unwrap();
        assert!(mic_on(&sent)); assert!(!g.off());
        assert!(sent.lock().len() <= 5, "{:?}", sent.lock());   // the opening "on" plus at most four changes
    }
    #[tokio::test]
    async fn no_publishing_before_the_mic_exists_or_after_leaving() {
        let (g, sent) = gate(0);
        g.set_game(true).await.unwrap(); assert!(sent.lock().is_empty());
        g.open().await.unwrap(); assert_eq!(*sent.lock(), vec![false], "joins muted if the key is down");
        g.close(); g.set_manual(false).await.unwrap(); assert_eq!(*sent.lock(), vec![false]);
    }
    #[tokio::test]
    async fn a_failed_switch_is_retried_on_the_next_wish() {
        let fail = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (f2, sent) = (fail.clone(), Arc::new(parking_lot::Mutex::new(vec![])));
        let s2 = sent.clone();
        let g = MicGate::new(Arc::new(move |on| { let (f, s) = (f2.clone(), s2.clone()); Box::pin(async move { if f.load(std::sync::atomic::Ordering::SeqCst) { return Err("device busy".into()); } s.lock().push(on); Ok(()) }) }));
        assert!(g.open().await.is_err());
        fail.store(false, std::sync::atomic::Ordering::SeqCst);
        g.flush().await.unwrap();
        assert_eq!(*sent.lock(), vec![true]);
    }
}
