//! Windows of their own: a call or one screen share popped out to drag to another monitor, and the small always-on-top
//! "mini" call window (faces, speaking ring, mute). They draw what the main window's call state holds (the engine is
//! one, the pictures are the same `Frames`), so nothing is decoded twice and a popped window never differs from the call.
use crate::app::Mega;
use crate::calls::engine::Command;
use crate::look;
use crate::video::Shown;
use gpui_kit::component::button::Button;
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

fn faces(m: &Mega, size: f32) -> Div {
    let mut row = h_flex().gap_2().items_center();
    for p in &m.call.snap.peers {
        let initial = p.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
        row = row.child(div().relative().flex_none().size(px(size)).rounded_full().border_2().border_color(if p.speaking { rgb(look::SPEAK).into() } else { hsla(0., 0., 0., 0.) })
            .child(div().size_full().rounded_full().bg(look::face(&p.name)).text_color(rgb(0x0c0d12)).font_family(look::F_SHARP).text_size(px(size * 0.58)).flex().items_center().justify_center().when(p.muted, |d| d.opacity(0.5)).child(initial))
            .when(p.muted, |d| d.child(div().absolute().right(px(-1.)).bottom(px(-1.)).size(px(10.)).rounded_full().bg(rgb(look::DANGER)))));
    }
    row
}

impl Render for PopView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main = self.main.clone();
        let m = main.read(cx);
        let base = div().size_full().bg(rgb(look::VOID)).text_color(look::paper()).font_family(look::F_UI).text_size(px(14.));
        if !m.call.in_call() { return base.flex().items_center().justify_center().font_family(look::F_NAME).text_color(look::mute()).child("The call has ended.").into_any_element(); }
        let muted = m.call.snap.muted;
        let mute_btn = {
            let main = main.clone();
            Button::new("pop-mute").label(if muted { "Unmute" } else { "Mute" }).on_click(move |_, _, cx| { main.update(cx, |m, cx| { m.call_cmd(Command::SetMuted(!muted)); cx.notify(); }); })
        };
        match self.kind.clone() {
            PopKind::Mini => base.p_2().child(h_flex().size_full().gap_3().items_center().child(faces(m, 38.)).child(div().flex_1()).child(mute_btn)).into_any_element(),
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
                    .child(h_flex().flex_none().gap_3().px_4().pt_3().items_center().child(div().font_family(look::F_SHARP).text_size(px(32.)).line_height(px(32.)).child(title)).child(faces(m, 34.)).child(div().flex_1()).child(mute_btn)
                        .child(Button::new("pop-leave").label("Leave").on_click(move |_, _, cx| { main2.update(cx, |m, cx| m.leave_call(cx)); })))
                    .child(grid)).into_any_element()
            }
        }
    }
}

impl Mega {
    fn open_pop(&mut self, kind: PopKind, title: &str, size_px: (f32, f32), mini: bool, cx: &mut Context<Self>) -> Option<AnyWindowHandle> {
        let main = cx.entity();
        let bounds = Bounds::centered(None, size(px(size_px.0), px(size_px.1)), cx);
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(200.), px(60.))),
            titlebar: Some(TitlebarOptions { title: Some(title.to_string().into()), ..Default::default() }),
            app_id: Some(if mini { "app.commons.chat.gpui.mini" } else { "app.commons.chat.gpui.pop" }.into()),
            kind: if mini { WindowKind::PopUp } else { WindowKind::Normal },
            ..Default::default()
        };
        match gpui_kit::open_window(opts, cx, move |_, cx| cx.new(|cx| PopView::new(kind, main, cx))) {
            Ok((h, _)) => Some(h),
            Err(e) => { eprintln!("pop-out window: {e}"); None }
        }
    }
    pub fn pop_call(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.call.in_call() { return; }
        let Some(h) = self.open_pop(PopKind::Call, "mega · call", (980., 640.), false, cx) else { return };
        self.call.popped = true;
        // when that window closes the column comes back
        let me = cx.entity().downgrade();
        let _ = h.update(cx, |root, window, cx| { let me = me.clone(); window.on_window_should_close(cx, move |_, cx| { let _ = me.update(cx, |m, cx| { m.call.popped = false; cx.notify(); }); true }); let _ = root; });
        cx.notify();
    }
    pub fn pop_tile(&mut self, sid: String, _window: &mut Window, cx: &mut Context<Self>) {
        let title = self.call.tile(&sid).map(|t| format!("mega · {}", t.name)).unwrap_or_else(|| "mega".into());
        self.open_pop(PopKind::Tile(sid), &title, (960., 540.), false, cx);
    }
    pub fn toggle_mini(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(h) = self.call.mini.take() { let _ = h.update(cx, |_, window, _| window.remove_window()); cx.notify(); return; }
        self.call.mini = self.open_pop(PopKind::Mini, "mega", (330., 64.), true, cx);
        cx.notify();
    }
}
