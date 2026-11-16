//! The window's side of the native updater (update.rs has the checks and the install): a card at the top of Home that
//! offers a newer version, shows the download, asks "Restart now?", and, once, shows what is new in the version that
//! was just installed. No menu, no dialog: nothing else in the app changes.
//!
//! Rules (docs/UPDATES.md): the feed is read a little after unlocking and every four hours; nothing is offered while a
//! call runs on this device; nothing restarts the app except the person's own click on "Restart now" (and that is not
//! possible during a call); an installed update that is not restarted into simply runs at the next start.
use crate::app::{Mega, Screen};
use crate::look;
use crate::update::{self, Phase, State};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex, Disableable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct UpdateUi {
    pub phase: Phase,
    pub state: State,
    pub current: String,
    pub target: Result<update::Target, String>,
    pub enabled: bool,
    next_check: Instant,
    started: Instant,
    boot_open: bool,
    got: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
}

impl UpdateUi {
    pub fn new(dir: &std::path::Path) -> UpdateUi {
        let exe = std::env::current_exe().unwrap_or_default();
        let first = std::env::var("MEGA_GPUI_UPDATE_FIRST_CHECK_SECS").ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(15);
        let state = State::load(dir);
        let current = update::current_version();
        UpdateUi {
            phase: Phase::Idle, boot_open: state.boot.as_ref().is_some_and(|(v, _)| *v == current), state, current, target: update::target_of(&exe),
            enabled: std::env::var("MEGA_GPUI_UPDATES").map(|v| v != "off").unwrap_or(true) && update::platform().is_some(),
            next_check: Instant::now() + Duration::from_secs(first), started: Instant::now(), got: Default::default(), total: Default::default(),
        }
    }
    /// The notes of the version that is running, if it was just installed here and they have not been dismissed.
    pub fn whatsnew(&self) -> Option<&(String, String)> { self.state.whatsnew.as_ref().filter(|(v, _)| *v == self.current) }
}

/// The task that runs the checks: a look every few seconds, for what is due.
pub fn task(window: &mut Window, cx: &mut Context<Mega>) -> Task<()> {
    cx.spawn_in(window, async move |this, cx| loop {
        cx.background_executor().timer(Duration::from_secs(2)).await;
        if this.update_in(cx, |this, window, cx| this.update_tick(window, cx)).is_err() { break; }
    })
}

impl Mega {
    fn busy_with_call(&self) -> bool { self.call.in_call() || self.core.app.call_active() }

    pub fn update_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A new version that has stayed up long enough is a good start: the previous one is no longer needed for that.
        if self.upd.boot_open && self.screen != Screen::Starting && self.upd.started.elapsed() >= update::BOOT_CONFIRM {
            self.upd.boot_open = false;
            self.upd.state.boot = None;
            self.upd.state.save(&self.core.dir);
        }
        if !self.upd.enabled || self.screen != Screen::Main || self.upd.target.is_err() { return; }
        if matches!(self.upd.phase, Phase::Idle) && Instant::now() >= self.upd.next_check && !self.busy_with_call() { self.update_check(window, cx); }
    }

    fn update_check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(platform) = update::platform() else { return };
        self.upd.phase = Phase::Checking;
        let (core, current) = (self.core.clone(), self.upd.current.clone());
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_spawn(async move {
                let relay = core.call("state", serde_json::json!({}))?["relay"].as_str().filter(|r| !r.is_empty()).map(String::from).ok_or("locked")?;
                update::check(&relay, &current, &platform)
            }).await;
            let _ = this.update_in(cx, |this, _, cx| {
                this.upd.phase = Phase::Idle;
                this.upd.next_check = Instant::now() + match &r { Ok(_) => update::CHECK_EVERY, Err(_) => Duration::from_secs(3600) };
                match r {
                    Ok(Some(rel)) => this.upd.phase = Phase::Available(rel),
                    Ok(None) => {}
                    Err(e) => eprintln!("update check: {e}"),
                }
                cx.notify();
            });
        }).detach();
    }

    /// "Later": nothing for four hours (one hour if the relay says this version is too old to go on with).
    pub fn update_later(&mut self, cx: &mut Context<Self>) {
        let required = matches!(&self.upd.phase, Phase::Available(r) if r.required);
        self.upd.phase = Phase::Idle;
        self.upd.next_check = Instant::now() + if required { Duration::from_secs(3600) } else { update::CHECK_EVERY };
        cx.notify();
    }

    pub fn update_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Phase::Available(rel) = self.upd.phase.clone() else { return };
        let Ok(target) = self.upd.target.clone() else { return };
        self.upd.phase = Phase::Downloading(rel.clone());
        self.upd.got.store(0, Ordering::Relaxed);
        self.upd.total.store(0, Ordering::Relaxed);
        let (got, total, key) = (self.upd.got.clone(), self.upd.total.clone(), update::public_key());
        cx.spawn_in(window, async move |this, cx| {
            let rel2 = rel.clone();
            let r = cx.background_spawn(async move {
                let data = update::fetch_verified(&rel2, &key, &got, &total)?;
                update::install(&target, &data)
            }).await;
            let _ = this.update_in(cx, |this, _, cx| {
                match r {
                    Ok(()) => {
                        this.upd.state.whatsnew = Some((rel.version.clone(), rel.notes.clone()));
                        this.upd.state.boot = Some((rel.version.clone(), 0));
                        this.upd.state.rolled_back = None;
                        this.upd.state.save(&this.core.dir);
                        this.upd.phase = Phase::Ready(rel.version);
                    }
                    Err(e) => { this.upd.next_check = Instant::now() + update::CHECK_EVERY; this.upd.phase = Phase::Failed(e); }
                }
                cx.notify();
            });
        }).detach();
    }

    /// "Restart now": never during a call.
    pub fn update_restart(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.upd.phase, Phase::Ready(_)) || self.busy_with_call() { return; }
        let Ok(target) = self.upd.target.clone() else { return };
        update::restart_into(&target, false);
        cx.quit();
    }

    pub fn update_dismiss(&mut self, cx: &mut Context<Self>) {
        match self.upd.phase { Phase::Failed(_) => self.upd.phase = Phase::Idle, _ => {} }
        cx.notify();
    }
    pub fn whatsnew_dismiss(&mut self, cx: &mut Context<Self>) {
        self.upd.state.whatsnew = None;
        self.upd.state.rolled_back = None;
        self.upd.state.save(&self.core.dir);
        cx.notify();
    }

    fn card_shell(&self) -> Div {
        v_flex().relative().flex_none().w_full().gap_2().p_4().border_1().border_color(hsla(0., 0., 0., 0.))
            .child(crate::squircle::back(18., rgb(look::SLIP))).child(crate::squircle::front(18., None, Some((1., look::line())), None))
    }
    fn card_title(t: String) -> Div { div().font_family(look::F_NAME).text_size(px(20.)).line_height(px(24.)).text_color(look::paper()).child(t) }
    fn card_text(t: String) -> Div { div().font_family(look::F_UI).text_size(px(14.)).line_height(px(20.)).text_color(look::mute()).child(t) }

    /// The cards for Home: a rolled-back notice, what's new, and the update's own state. Nothing while locked.
    pub fn update_cards(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out: Vec<AnyElement> = vec![];
        if !self.upd.enabled && self.upd.state.whatsnew.is_none() { return out; }
        if let Some(v) = self.upd.state.rolled_back.clone() {
            out.push(self.card_shell().id("update-rolled-back")
                .child(Self::card_title(format!("Version {v} did not start well")))
                .child(Self::card_text("The previous version is back. Nothing was lost.".into()))
                .child(h_flex().gap_2().child(Button::new("rb-ok").small().label("OK").on_click(cx.listener(|this, _, _, cx| this.whatsnew_dismiss(cx)))))
                .into_any_element());
        }
        if let Some((v, notes)) = self.upd.whatsnew().cloned() {
            out.push(self.card_shell().id("whatsnew")
                .child(Self::card_title(format!("mega is now version {v}")))
                .when(!notes.trim().is_empty(), |d| d.child(div().text_color(look::paper()).child(Self::card_text(notes.clone()).text_color(look::paper()))))
                .when(notes.trim().is_empty(), |d| d.child(Self::card_text("Updated.".into())))
                .child(h_flex().gap_2().child(Button::new("wn-ok").small().label("Dismiss").on_click(cx.listener(|this, _, _, cx| this.whatsnew_dismiss(cx)))))
                .into_any_element());
        }
        let in_call = self.busy_with_call();
        match self.upd.phase.clone() {
            Phase::Available(rel) if !in_call => {
                out.push(self.card_shell().id("update-available")
                    .child(Self::card_title(format!("Update available: version {}", rel.version)))
                    .when(rel.required, |d| d.child(Self::card_text("This version of mega is too old for this server to go on without updating.".into())))
                    .child(Self::card_text("The new version is checked against the update key before anything is replaced. It starts the next time mega opens, or when you choose to restart.".into()))
                    .child(h_flex().gap_2()
                        .child(Button::new("up-install").small().primary().label("Install now").on_click(cx.listener(|this, _, window, cx| this.update_install(window, cx))))
                        .child(Button::new("up-later").small().ghost().label("Later").on_click(cx.listener(|this, _, _, cx| this.update_later(cx)))))
                    .into_any_element());
            }
            Phase::Downloading(rel) => {
                let (g, t) = (self.upd.got.load(Ordering::Relaxed), self.upd.total.load(Ordering::Relaxed));
                let line = if t > 0 { format!("Downloading version {}: {} of {} MB", rel.version, g / 1_000_000, t / 1_000_000) } else { format!("Downloading version {}…", rel.version) };
                out.push(self.card_shell().id("update-downloading").child(Self::card_title(line)).into_any_element());
            }
            Phase::Ready(v) => {
                out.push(self.card_shell().id("update-ready")
                    .child(Self::card_title(format!("Version {v} is installed")))
                    .child(Self::card_text(if in_call { "It starts when mega is next opened. You can restart once your call has ended.".into() } else { "Restart mega to start using it, or it starts the next time mega opens.".into() }))
                    .child(h_flex().gap_2().child(Button::new("up-restart").small().primary().label("Restart now").disabled(in_call).on_click(cx.listener(|this, _, _, cx| this.update_restart(cx)))))
                    .into_any_element());
            }
            Phase::Failed(e) => {
                out.push(self.card_shell().id("update-failed")
                    .child(Self::card_title("The update was not installed".into()))
                    .child(Self::card_text(format!("{e} Nothing was changed. mega will try again later.")))
                    .child(h_flex().gap_2().child(Button::new("up-fail-ok").small().label("OK").on_click(cx.listener(|this, _, _, cx| this.update_dismiss(cx)))))
                    .into_any_element());
            }
            _ => {}
        }
        out
    }
}
