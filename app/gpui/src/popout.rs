//! Windows of their own: a call or one screen share popped out to drag to another monitor, and the small always-on-top
//! "mini" call window (faces, speaking ring, mute). They draw what the main window's call state holds (the engine is
//! one, the pictures are the same `Frames`), so nothing is decoded twice and a popped window never differs from the call.
use crate::app::Mega;
use crate::calls::engine::Command;
use crate::look;
use crate::video::Shown;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

#[derive(Clone, PartialEq)]
pub enum PopKind { Call, Tile(String), Mini }

pub struct PopView {
    kind: PopKind,
    main: Entity<Mega>,
    shown: Shown,
    _subs: Vec<Subscription>,
}

impl PopView {
    fn new(kind: PopKind, main: Entity<Mega>, cx: &mut Context<Self>) -> PopView {
        // the main window is the one source of truth: when its call state or a picture changes, this window draws again
        let sub = cx.observe(&main, |_, _, cx| cx.notify());
        PopView { kind, main, shown: Shown::default(), _subs: vec![sub] }
    }
}

/// The people as the call card shows them: squares, the one talking ringed in the room's highlight.
fn faces(m: &Mega, size: f32) -> Div {
    let duo = m.room_duo(m.call.snap.gid.as_deref().unwrap_or(""));
    let mut row = h_flex().gap(px(8.)).items_center();
    for p in &m.call.snap.peers {
        let r = size * 22. / 48.;
        let mut cell = div().relative().flex_none().size(px(size));
        if p.speaking {
            cell = cell.child(div().absolute().left(px(-4.)).top(px(-4.)).size(px(size + 8.)).child(crate::squircle::back(r + 4., rgb(duo.hi))))
                .child(div().absolute().left(px(-2.)).top(px(-2.)).size(px(size + 4.)).child(crate::squircle::back(r + 2., rgb(look::VOID))));
        }
        row = row.child(cell.child(div().relative().size(px(size)).when(p.muted || p.deafened, |d| d.opacity(0.55)).child(crate::calldock::face(p.account.as_deref(), &p.name, size, r))));
    }
    row
}

/// An icon button of the call card's style (48 x 32, the squircle in near black, lit paper when on).
fn ibtn(id: &'static str, icon: crate::icons::Icon, on: bool, main: Entity<Mega>, act: impl Fn(&mut Mega, &mut Context<Mega>) + 'static) -> Stateful<Div> {
    let (bg, ink): (Hsla, Hsla) = if on { (rgb(0xefedf2).into(), rgb(0x050407).into()) } else { (hsla(0., 0., 1., 0.1), rgb(0xefedf2).into()) };
    div().id(id).relative().flex_none().w(px(48.)).h(px(32.)).flex().items_center().justify_center().cursor_pointer()
        .child(crate::squircle::back(22., bg)).child(div().relative().child(crate::icons::icon(icon, 16., ink)))
        .on_click(move |_, _, cx| { main.update(cx, |m, cx| act(m, cx)); })
}

impl Render for PopView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main = self.main.clone();
        let m = main.read(cx);
        let base = div().size_full().bg(rgb(look::VOID)).text_color(look::paper()).font_family(look::F_UI).text_size(px(14.));
        if !m.call.in_call() { return base.flex().items_center().justify_center().font_family(look::F_NAME).text_color(look::mute()).child("The call has ended.").into_any_element(); }
        let muted = m.call.snap.muted;
        let mute_btn = ibtn("pop-mute", if muted { crate::icons::Icon::MicOff } else { crate::icons::Icon::Mic }, muted, main.clone(), move |m, cx| { m.call_cmd(Command::SetMuted(!muted)); cx.notify(); });
        match self.kind.clone() {
            PopKind::Mini => base.p_2().child(h_flex().size_full().gap_3().items_center().child(faces(m, 40.)).child(div().flex_1()).child(mute_btn)).into_any_element(),
            PopKind::Tile(sid) => {
                let (frames, tile) = (m.call.frames.clone(), m.call.tile(&sid).cloned());
                let img_ = self.shown.take(&frames, &sid, window);
                let Some(t) = tile else { return base.flex().items_center().justify_center().text_color(look::mute()).child("This screen is not shared any more.").into_any_element() };
                base.bg(rgb(0x000000)).relative()
                    .child(match img_ { Some(i) => div().size_full().child(img(ImageSource::Render(i)).size_full().object_fit(ObjectFit::Contain)), None => div().size_full() })
                    .child(div().absolute().left(px(14.)).bottom(px(12.)).px_2().py_0p5().rounded_full().bg(hsla(0., 0., 0., 0.55)).font_family(look::F_NAME).text_size(px(13.)).child(t.name)).into_any_element()
            }
            PopKind::Call => {
                let (frames, order) = (m.call.frames.clone(), m.call.ordered());
                let title = m.call.snap.room_name.clone();
                let mut grid = h_flex().id("pop-grid").flex_1().min_h_0().flex_wrap().gap_3().p_4().overflow_y_scroll().content_start();
                for t in &order {
                    let img_ = self.shown.take(&frames, &t.sid, window);
                    grid = grid.child(div().relative().w(px(360.)).flex_none().rounded(px(18.)).overflow_hidden().bg(rgb(0x0b0a10))
                        .child(div().w_full().pb(relative(0.5625)))
                        .child(match img_ { Some(i) => div().absolute().inset_0().child(img(ImageSource::Render(i)).size_full().object_fit(ObjectFit::Cover)), None => div().absolute().inset_0() })
                        .child(div().absolute().left(px(10.)).bottom(px(8.)).px_2().rounded_full().bg(hsla(0., 0., 0., 0.55)).font_family(look::F_NAME).text_size(px(12.)).child(t.name.clone())));
                }
                let main2 = main.clone();
                base.child(v_flex().size_full()
                    .child(h_flex().flex_none().gap_3().px_4().pt_3().items_center().child(div().font_family(look::F_SHARP).text_size(px(32.)).line_height(px(32.)).child(title)).child(faces(m, 36.)).child(div().flex_1()).child(mute_btn)
                        .child(ibtn("pop-leave", crate::icons::Icon::Leave, false, main2, |m, cx| m.leave_call(cx))))
                    .child(grid)).into_any_element()
            }
        }
    }
}

impl Mega {
    /// Opens a window for the call. It is done after the current update ends: the new window draws at once and reads this
    /// entity, which cannot be read while it is being updated.
    fn open_pop(&mut self, kind: PopKind, title: &str, size_px: (f32, f32), mini: bool, cx: &mut Context<Self>, then: impl FnOnce(&mut Mega, AnyWindowHandle, Entity<PopView>, &mut Context<Mega>) + 'static) {
        let main = cx.entity();
        let title = title.to_string();
        cx.defer(move |cx| {
            let bounds = Bounds::centered(None, size(px(size_px.0), px(size_px.1)), cx);
            let opts = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(200.), px(60.))),
                titlebar: Some(TitlebarOptions { title: Some(title.into()), ..Default::default() }),
                app_id: Some(if mini { "app.commons.chat.gpui.mini" } else { "app.commons.chat.gpui.pop" }.into()),
                kind: if mini { WindowKind::Floating } else { WindowKind::Normal },
                ..Default::default()
            };
            let m2 = main.clone();
            match gpui_kit::open_window(opts, cx, move |_, cx| cx.new(|cx| PopView::new(kind, m2, cx))) {
                Ok((h, view)) => { main.update(cx, |m, cx| then(m, h, view, cx)); }
                Err(e) => eprintln!("pop-out window: {e}"),
            }
        });
    }
    pub fn pop_call(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.call.in_call() || self.call.popped { return; }
        self.open_pop(PopKind::Call, "mega · call", (980., 640.), false, cx, |m, h, _view, cx| {
            // the column leaves the main window until that window is closed (`call_tick` notices)
            m.call.popped = true;
            m.call.pop_handle = Some(h);
            cx.notify();
        });
    }
    pub fn pop_tile(&mut self, sid: String, _window: &mut Window, cx: &mut Context<Self>) {
        let title = self.call.tile(&sid).map(|t| format!("mega · {}", t.name)).unwrap_or_else(|| "mega".into());
        self.open_pop(PopKind::Tile(sid), &title, (960., 540.), false, cx, |_, _, _, _| {});
    }
    pub fn toggle_mini(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(h) = self.call.mini.take() { let _ = h.update(cx, |_, window, _| window.remove_window()); cx.notify(); return; }
        self.open_pop(PopKind::Mini, "mega", (330., 64.), true, cx, |m, h, _view, cx| {
            m.call.mini = Some(h);
            cx.notify();
        });
    }
}

impl Mega {
    /// A pop-out window the user closed is forgotten: the column comes back, the mini button goes off.
    pub fn pop_windows_check(&mut self, cx: &mut Context<Self>) {
        let open: Vec<AnyWindowHandle> = cx.windows();
        let mut changed = false;
        if self.call.pop_handle.is_some_and(|h| !open.contains(&h)) { self.call.pop_handle = None; self.call.popped = false; changed = true; }
        if self.call.mini.is_some_and(|h| !open.contains(&h)) { self.call.mini = None; changed = true; }
        if changed { cx.notify(); }
    }
}
