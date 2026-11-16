//! rune in the call bar: what is playing on this PC's rune (the music app, its local HTTP API), play or pause, next and
//! previous, and a one-click "listen along" with someone in the call who shares a Spotify song.
//!
//! rune is looked at only while a call is open, and the bar shows nothing at all while rune does not answer (two misses in a
//! row; then it is asked only every ten seconds, as the core does for sharing). The reading and the four controls are
//! `commons_core::rune::{current, control}`; rune is never started and nothing here ever plays on its own.
//! Listening along is the core's own feature (`listen_join`, mega-listen.js): it plays the friend's song on this rune, in step.
use crate::app::Mega;
use crate::look;
use commons_core::rune::Current;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{json, Value};

/// How often rune is asked while it answers, and while it does not.
const EVERY: f64 = 2.0;
const ABSENT_EVERY: f64 = 10.0;
const ABSENT_AFTER: u32 = 2;
/// How often the call's people are checked for a song to join.
const ALONG_EVERY: f64 = 3.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Friend { pub account: String, pub name: String, pub title: String, pub artist: String }

#[derive(Default)]
pub struct RuneUi {
    pub now: Option<Current>,
    misses: u32,
    next: f64,
    busy: bool,
    /// People in the call who share a Spotify song one can join.
    pub friends: Vec<Friend>,
    /// Who we listen along with now.
    pub along: Option<String>,
    along_next: f64,
    along_busy: bool,
    pub controls: u32,
}

/// From the core's `members` answer: those in `here` who share a Spotify song, except me.
pub fn friends(members: &Value, here: &dyn Fn(&str) -> bool) -> Vec<Friend> {
    members.as_array().map(|l| l.iter().filter_map(|m| {
        let acct = m["account"].as_str()?;
        if m["me"].as_bool() == Some(true) || !here(acct) { return None; }
        let l = &m["listening"];
        l["spotify"].is_object().then(|| Friend { account: acct.into(), name: m["name"].as_str().unwrap_or("").into(), title: l["title"].as_str().unwrap_or("").into(), artist: l["artist"].as_str().unwrap_or("").into() })
    }).collect()).unwrap_or_default()
}

/// The line shown for a song.
pub fn song_line(c: &Current) -> String { if c.artist.is_empty() { c.title.clone() } else { format!("{} \u{2013} {}", c.title, c.artist) } }

impl Mega {
    /// Runs a blocking job off the UI thread, then `then` back on it.
    pub fn bg<T: Send + 'static>(&mut self, work: impl FnOnce() -> T + Send + 'static, window: &mut Window, cx: &mut Context<Self>, then: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static) {
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_spawn(async move { work() }).await;
            let _ = this.update_in(cx, |this, window, cx| then(this, r, window, cx));
        }).detach();
    }

    /// Every half second, from the window's clock; does its own pacing.
    pub fn rune_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.call.in_call() {
            if self.rune.now.is_some() || !self.rune.friends.is_empty() || self.rune.along.is_some() { self.rune = Default::default(); cx.notify(); }
            return;
        }
        let now = crate::ringui::unix();
        let url = self.snap.as_ref().map(|s| s.rune_url.clone()).unwrap_or_default();
        if !url.is_empty() && !self.rune.busy && now >= self.rune.next {
            self.rune.busy = true;
            self.rune.next = now + if self.rune.misses >= ABSENT_AFTER { ABSENT_EVERY } else { EVERY };
            self.bg(move || commons_core::rune::current(&url), window, cx, |this, r, _, cx| {
                this.rune.busy = false;
                match r {
                    Ok(c) => { this.rune.misses = 0; if this.rune.now != c { this.rune.now = c; cx.notify(); } }
                    Err(_) => { this.rune.misses = this.rune.misses.saturating_add(1); if this.rune.misses >= ABSENT_AFTER && this.rune.now.take().is_some() { cx.notify(); } }
                }
            });
        }
        // someone in the call sharing a song to join, and whether we already are
        if self.rune.now.is_some() && !self.rune.along_busy && now >= self.rune.along_next {
            let Some(cid) = self.call.snap.gid.as_deref().and_then(|g| self.channel(g)).map(|(c, _)| c.id.clone()) else { return };
            self.rune.along_busy = true;
            self.rune.along_next = now + ALONG_EVERY;
            let (core, others) = (self.core.clone(), self.call.others());
            self.bg(move || {
                let members = core.call("members", json!({"cid": cid})).unwrap_or(Value::Null);
                let st = core.call("listen_status", json!({})).unwrap_or(Value::Null);
                (friends(&members, &|a| others.contains(a)), st)
            }, window, cx, |this, (f, st), _, cx| {
                this.rune.along_busy = false;
                let on = (st["state"].as_str() == Some("on")).then(|| st["acct"].as_str().unwrap_or("").to_string());
                if this.rune.friends != f || this.rune.along != on { this.rune.friends = f; this.rune.along = on; cx.notify(); }
            });
        }
    }

    /// pause, resume, next or prev on the zone that is showing.
    pub fn rune_control(&mut self, what: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(c), Some(url)) = (self.rune.now.clone(), self.snap.as_ref().map(|s| s.rune_url.clone())) else { return };
        self.rune.controls += 1;
        if what == "pause" || what == "resume" { if let Some(n) = self.rune.now.as_mut() { n.playing = what == "resume"; } cx.notify(); }
        let zone = c.zone.clone();
        self.bg(move || commons_core::rune::control(&url, &zone, what), window, cx, |this, r, window, cx| {
            if let Err(e) = r { this.rune_say(&format!("rune did not take that: {e}"), cx); }
            this.rune.next = 0.;            // look again at once
            this.rune_tick(window, cx);
        });
    }
    fn rune_say(&mut self, t: &str, cx: &mut Context<Self>) { self.ring.notes.push((t.into(), crate::ringui::unix() + 6.)); cx.notify(); }

    /// One click: join the first friend's song, or leave the one we follow.
    pub fn rune_along(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rune.along.is_some() {
            self.rpc("listen_leave", json!({}), window, cx, |this, _, _, cx| { this.rune.along = None; this.rune.along_next = 0.; cx.notify(); });
            return;
        }
        let Some(f) = self.rune.friends.first().cloned() else { return };
        self.rpc("listen_join", json!({"account": f.account, "target": ""}), window, cx, move |this, r, window, cx| {
            match r {
                Ok(st) if st["state"].as_str() == Some("on") => { this.rune.along = Some(f.account.clone()); }
                Ok(st) => {
                    let m = st["message"].as_str().filter(|m| !m.is_empty()).unwrap_or("Could not listen along: rune is not answering.").to_string();
                    this.rune_say(&m, cx);
                    this.rpc("listen_leave", json!({}), window, cx, |_, _, _, _| {});
                }
                Err(e) => this.rune_say(&e, cx),
            }
            this.rune.along_next = 0.;
            cx.notify();
        });
    }

    /// The part of the call bar that is rune's: nothing unless rune answers.
    pub fn rune_segment(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let now = self.rune.now.clone()?;
        let small = |id: &'static str, text: SharedString| div().id(id).px_2().py_1().rounded_full().border_1().border_color(look::line()).cursor_pointer().font_family(look::F_NAME).text_size(px(12.)).child(text);
        let what: &'static str = if now.playing { "pause" } else { "resume" };
        let along = self.rune.along.is_some();
        let friend = self.rune.friends.first().cloned();
        Some(h_flex().id("rune-bar").flex_none().gap_2().items_center().max_w(px(520.))
            .child(div().id("rune-song").min_w_0().max_w(px(220.)).overflow_hidden().whitespace_nowrap().text_ellipsis().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute())
                .child(format!("{} {}", if now.playing { "\u{266a}" } else { "(paused)" }, song_line(&now))))
            .child(small("rune-prev", "prev".into()).on_click(cx.listener(|this, _, window, cx| this.rune_control("prev", window, cx))))
            .child(small("rune-play", if now.playing { "pause" } else { "play" }.into()).on_click(cx.listener(move |this, _, window, cx| this.rune_control(what, window, cx))))
            .child(small("rune-next", "next".into()).on_click(cx.listener(|this, _, window, cx| this.rune_control("next", window, cx))))
            .when(along || friend.is_some(), |d| {
                let label = if along { "Stop listening along".to_string() } else { format!("Listen along with {}", friend.as_ref().map(|f| f.name.as_str()).unwrap_or("them")) };
                d.child(small("rune-along", label.into()).when(along, |x| x.bg(look::paper()).text_color(rgb(0x0c0d12))).on_click(cx.listener(|this, _, window, cx| this.rune_along(window, cx))))
            })
            .into_any_element())
    }

    pub fn rune_dump(&self) -> Value {
        json!({"now": self.rune.now, "friends": self.rune.friends.iter().map(|f| json!({"account": f.account, "name": f.name, "title": f.title})).collect::<Vec<_>>(),
               "along": self.rune.along, "controls": self.rune.controls})
    }
}


#[cfg(test)]
mod tests {
    // not `super::*`: that brings in GPUI's own `test` attribute
    use super::{friends, song_line};
    use commons_core::rune::Current;
    use serde_json::Value;

    #[test]
    fn only_people_in_the_call_with_a_joinable_song_are_offered() {
        let m: Value = serde_json::from_str(r#"[
            {"account": "me", "name": "Me", "me": true, "listening": {"title": "x", "artist": "y", "spotify": {"id": "a", "paused": false}}},
            {"account": "ben", "name": "Ben", "me": false, "listening": {"title": "Song", "artist": "Band", "spotify": {"id": "a", "paused": false}}},
            {"account": "ann", "name": "Ann", "me": false, "listening": {"title": "Local file", "artist": "Band"}},
            {"account": "cy", "name": "Cy", "me": false, "listening": {"title": "Away", "artist": "", "spotify": {"id": "b", "paused": true}}},
            {"account": "di", "name": "Di", "me": false, "listening": null}
        ]"#).unwrap();
        let f = friends(&m, &|a| a != "cy");
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].account.as_str(), f[0].title.as_str()), ("ben", "Song"));
        assert!(friends(&Value::Null, &|_| true).is_empty());
    }

    #[test]
    fn the_song_line_reads_title_then_artist() {
        let c = |a: &str| Current { zone: "pc".into(), zone_name: "PC".into(), playing: true, title: "Song".into(), artist: a.into() };
        assert_eq!(song_line(&c("Band")), "Song \u{2013} Band");
        assert_eq!(song_line(&c("")), "Song");
    }
}
