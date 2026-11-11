'use strict';
// mega: the phone's gestures. Phones and touch only (html.mg-touch); a computer keeps every behaviour it had.
//
//  - Between the levels: a swipe to the right goes from a chat to its community's rooms, and from the rooms to all communities;
//    a swipe to the left goes the other way, into the card in focus. The page follows the finger and lets go with the swipe's
//    speed; a swipe that ends short springs back. Only transforms and opacity move: nothing is laid out while a finger is down.
//  - On a message: a short swipe to the right replies to it (the row follows the finger a little, a reply mark appears, and
//    past the mark it is a reply). Keep going across the screen and the swipe becomes the page's own: the row springs back and
//    the chat slides. A swipe that starts off the rows (the header, the composer, a gap, a day label) is the page's at once.
//  - Holding a message opens its menu (reactions, reply, copy, edit and delete when allowed). Tapping it twice gives it a ❤.
//  - A picture opens full screen: pinch to zoom and pan, swipe down to close, sideways for the room's other pictures.
//
// What never swipes: the dots (they have their own: a swipe in from the edge or along them opens the cards), the call strip and its
// tiles, the camera stack, anything that scrolls sideways (the bar, the header's buttons, code), a drawing in progress, text boxes,
// sliders, and any float (menus, the picker, People, modals, the viewer). A gesture is horizontal only when clearly so after 10 px;
// anything else is the browser's scroll. iOS's own back gesture is off in the app (wry's default), so nothing fights ours.
//
// The pure part (direction, thresholds, speeds, pinch arithmetic) is MegaTouch, tested without a browser: node --test tools/mega-touch.test.mjs
(function (root) {
  const T = {
    SLOP: 10, REPLY: 56, HANDOVER: .4, COMMIT: .35, FLICK: .45,
    /** direction of a drag: null while under the slop, 'h' only when clearly sideways, otherwise 'v' */
    lock(dx, dy, slop = T.SLOP) { const ax = Math.abs(dx), ay = Math.abs(dy); if (ax < slop && ay < slop) return null; return ax > ay * 1.3 ? 'h' : 'v'; },
    /** px/ms over the last `win` ms of [{ t, x }] samples (0 with fewer than two points in the window) */
    velocity(samples, now, win = 80) {
      if (samples.length < 2) return 0;
      const last = samples[samples.length - 1]; let first = last;
      for (let i = samples.length - 2; i >= 0 && now - samples[i].t <= win; i--) first = samples[i];
      if (first === last) first = samples[samples.length - 2];   // nothing else in the window: the last two points
      const dt = last.t - first.t; return dt > 0 ? (last.x - first.x) / dt : 0;
    },
    /** a page swipe commits past a third of the width, or sooner when flicked that way */
    decide(dx, vx, width) { return dx >= width * T.COMMIT || (dx >= 40 && vx >= T.FLICK); },
    /** a rubber band past `limit`: the page shows there is nothing there */
    damp(dx, limit = 60) { return dx <= limit ? dx : limit + (dx - limit) * .3; },
    /** how far a row follows a reply swipe: fully to the mark, then a third of the rest */
    replyX(dx) { return dx <= T.REPLY ? dx : T.REPLY + (dx - T.REPLY) * .35; },
    replyDecide(dx, vx) { return dx >= T.REPLY || (dx >= T.REPLY / 2 && vx >= .5); },
    /** a swipe on a row that goes this far across is the page's, not a reply */
    handover(dx, width) { return dx >= width * T.HANDOVER; },
    /** two taps on the same thing within 320 ms and 30 px */
    doubleTap(prev, now, x, y, id) { return !!prev && prev.id === id && now - prev.t < 320 && Math.hypot(x - prev.x, y - prev.y) < 30; },
    /** pinch: the start pair p0,p1 and the current pair q0,q1 (each { x, y }), the start scale and shift, the picture's centre on screen → { s, tx, ty }.
        The point of the picture that was between the fingers stays between them. */
    pinch(p0, p1, q0, q1, s0, tx0, ty0, cx, cy, max = 5) {
      const d0 = Math.hypot(p1.x - p0.x, p1.y - p0.y) || 1, d1 = Math.hypot(q1.x - q0.x, q1.y - q0.y);
      const s = Math.max(1, Math.min(max, s0 * d1 / d0));
      const mx0 = (p0.x + p1.x) / 2, my0 = (p0.y + p1.y) / 2, mx1 = (q0.x + q1.x) / 2, my1 = (q0.y + q1.y) / 2;
      const ux = (mx0 - cx - tx0) / s0, uy = (my0 - cy - ty0) / s0;   // the picture point under the fingers at the start
      return { s, tx: mx1 - cx - ux * s, ty: my1 - cy - uy * s };
    },
    /** zoom to `s` about a screen point (a double tap) */
    zoomAt(px, py, s0, tx0, ty0, s, cx, cy) { const ux = (px - cx - tx0) / s0, uy = (py - cy - ty0) / s0; return { s, tx: px - cx - ux * s, ty: py - cy - uy * s }; },
    /** keep a zoomed picture's edges outside the viewport (a picture smaller than the viewport stays centred) */
    clampPan(tx, ty, s, w, h, vw, vh) { const mx = Math.max(0, (w * s - vw) / 2), my = Math.max(0, (h * s - vh) / 2); return { tx: Math.max(-mx, Math.min(mx, tx)), ty: Math.max(-my, Math.min(my, ty)) }; },
    /** how long a slide to the edge should take at this speed: 120-260 ms */
    slideMs(dist, v) { const ms = Math.abs(v) > .05 ? Math.abs(dist) / Math.abs(v) : 260; return Math.max(120, Math.min(260, ms)); },
  };
  root.MegaTouch = T;
  if (typeof document === 'undefined' || !root.__mega) return;

  // ------------------------------------------------------------------ the page
  const M = root.__mega, A = M.api, html = document.documentElement;
  const $ = (s, r = document) => r.querySelector(s);
  const calm = () => matchMedia('(prefers-reduced-motion: reduce)').matches;
  const SPRING = 'cubic-bezier(.2, 1.2, .3, 1)';
  const touchy = () => matchMedia('(pointer: coarse)').matches || navigator.maxTouchPoints > 0 || root.__commonsPlatform === 'mobile';
  const phone = () => matchMedia('(max-width: 760px)').matches && touchy();
  const mark = () => html.classList.toggle('mg-touch', phone());
  mark(); matchMedia('(max-width: 760px)').addEventListener('change', mark); matchMedia('(pointer: coarse)').addEventListener('change', mark);
  new MutationObserver(mark).observe(document.body, { attributes: true, attributeFilter: ['data-platform'] });   // app.js learns it is the phone app a moment after loading
  const stream = $('#stream'), main = $('#main'), wrap = $('#stream-wrap');
  const msgOf = id => (S.msgs && S.msgs.messages.find(x => String(x.id) === String(id))) || null;
  const hi = () => getComputedStyle(html).getPropertyValue('--mg-hi').trim() || '#efedf2';

  /** nothing swipes while a float has the screen, a card is being drawn on, or the dots are open */
  const blocked = () => !M.started || M.busy || M.rail || M.pal || A.paintOpen() || A.modalOpen() || S.people
    || html.classList.contains('mg-drawing') || html.classList.contains('mg-drawing-card') || !!$('.lightbox, #mega-view, .emojis, #call-pop, .nm-menu, .mpop');
  const NOSWIPE = '#mega-rail, #call-stage, #call-dock, #call-ppl, #people, #mega-paint, .emojis, .menu, #modal-root, #mega-wallhit, #mega-wallhit-card, #mega-wallbar, input, textarea, select, [contenteditable], .vwave, #mic, .mediaslot, video, audio, .mbar, #head .head-actions, .mscrub, #jump, #mega-deckshow';
  /** a touch that another gesture owns: sideways scrollers, touch-action: none, text boxes, the dots, the strips */
  function owned(t) {
    if (!t || !t.closest) return true;
    if (t.closest(NOSWIPE)) return true;
    for (let e = t; e && e !== document.body; e = e.parentElement) {
      const cs = getComputedStyle(e);
      if (cs.touchAction === 'none') return true;
      if (e.scrollWidth > e.clientWidth + 1 && /auto|scroll/.test(cs.overflowX)) return true;
    }
    const sel = getSelection(); if (sel && !sel.isCollapsed && sel.rangeCount) return true;
    return false;
  }

  // ---- the plans: what moves and what happens, per level and direction (+1 = to the right, -1 = to the left)
  function plan(dir) {
    const rubber = { rubber: true };
    if (M.level === 'chat') return dir > 0 ? { els: [M.opts ? $('#mega-opts') : main], commit: () => A.openFromDots() } : null;
    const list = M.list; if (!list) return null;
    const el = list.el, x = list.settled ? list.item : null;
    if (M.level === 'server') {
      if (dir > 0) return A.comms().length > 1 ? { els: [el], commit: () => A.showHome(M.cid) } : rubber;
      if (!x || x.kind === 'voice') return rubber;   // nothing joins a call by a swipe
      if (x.locked) return { ...rubber, say: 'This room is still waiting for keys.' };
      return { els: [el], commit: () => A.enterRoom(x) };
    }
    if (M.level === 'home') {
      if (dir > 0) return rubber;
      if (!x) return rubber;
      if (x.kind === 'community') return { els: [el], commit: () => A.showServer(x.cid) };
      if (x.kind === 'news') return { els: [el], commit: () => root.__megaNew && __megaNew.openFirst(x) };
      return { els: [el], commit: () => A.enterCommunity(x), stay: true };   // "you" opens settings over the cards
    }
    return null;
  }
  const place = (els, x, p) => els.forEach(e => { e.style.transform = x ? `translateX(${x}px)` : ''; e.style.opacity = p ? String(Math.max(.3, 1 - .5 * p)) : ''; });
  const lift = (els, on) => els.forEach(e => { e.style.willChange = on ? 'transform, opacity' : ''; });
  const anim = (el, frames, ms, easing) => el.animate(frames, { duration: ms, easing, fill: 'none' }).finished.catch(() => { });

  // ---- one gesture at a time
  let G = null;
  function start(e) {
    if (G || e.touches.length !== 1 || !phone() || blocked()) return;
    const t = e.touches[0], tg = e.target;
    if (owned(tg)) return;
    const row = M.level === 'chat' && !M.opts ? tg.closest('#stream .msg') : null;
    G = { id: t.identifier, x0: t.clientX, y0: t.clientY, t0: performance.now(), lock: null, kind: row ? 'reply' : 'nav', row, m: row ? msgOf(row.dataset.id) : null, plan: null, dir: 0, off: 0, x: 0, samples: [], scrolled: false };
    if (G.kind === 'reply' && !G.m) G.kind = 'nav';
  }
  function move(e) {
    const g = G; if (!g) return;
    const t = [...e.touches].find(k => k.identifier === g.id); if (!t || e.touches.length !== 1) { end(e, true); return; }
    const now = performance.now(), dx = t.clientX - g.x0, dy = t.clientY - g.y0;
    g.samples.push({ t: now, x: t.clientX }); if (g.samples.length > 12) g.samples.shift();
    if (!g.lock) {
      g.lock = T.lock(dx, dy); if (!g.lock) return;
      if (g.lock === 'v') { G = null; return; }   // the browser's scroll
      if (g.kind === 'nav') { if (!begin(g, dx > 0 ? 1 : -1)) return; }
      else { g.dir = dx > 0 ? 1 : -1; if (g.dir < 0) { G = null; return; } replyBegin(g); }
    }
    if (g.scrolled) { end(e, true); return; }
    if (g.kind === 'reply') {
      if (T.handover(dx, innerWidth)) { replyEnd(g, false); g.kind = 'nav'; g.off = dx; if (!begin(g, 1)) return; }   // the page takes over from here, with no jump
      else { replyMove(g, Math.max(0, dx)); return; }
    }
    navMove(g, dx);
  }
  function begin(g, dir) {
    g.dir = dir; g.plan = plan(dir);
    if (!g.plan) { G = null; return false; }
    if (g.plan.els) lift(g.plan.els, true);
    return true;
  }
  function navMove(g, dx) {
    const p = g.plan, d = (dx - g.off) * g.dir;   // distance along the swipe's direction, from the handover point
    if (d < 0) { g.x = 0; if (p.els) place(p.els, 0, 0); return; }
    if (p.rubber) { g.x = T.damp(d, 40) * g.dir; if (!calm()) place([M.list ? M.list.el : main], g.x, 0); return; }
    g.x = d * g.dir; if (!calm()) place(p.els, g.x, d / innerWidth);
  }
  async function end(e, cancel) {
    const g = G; if (!g) return; G = null;
    if (!g.lock || g.lock === 'v') return;
    const now = performance.now(), v = T.velocity(g.samples, now) * (g.dir || 1), w = innerWidth;
    if (g.kind === 'reply') { replyEnd(g, !cancel && !g.scrolled && T.replyDecide(g.x, v)); return; }
    const p = g.plan; if (!p) return;
    if (p.rubber) { const el = M.list ? M.list.el : main; if (g.x && !calm()) await anim(el, [{ transform: `translateX(${g.x}px)` }, { transform: 'none' }], 260, SPRING); el.style.transform = ''; if (p.say && !cancel) toast(p.say); return; }
    const d = Math.abs(g.x), go = !cancel && !g.scrolled && T.decide(d, v, w);
    if (go) {
      if (!calm() && !p.stay && d > 0) {   // it ends off screen and stays there until the next level is up over it
        const from = { transform: `translateX(${g.x}px)`, opacity: Math.max(.3, 1 - .5 * d / w) }; place(p.els, g.dir * w, 1);
        await anim(p.els[0], [from, { transform: `translateX(${g.dir * w}px)`, opacity: .5 }], T.slideMs(w - d, v), 'cubic-bezier(.3, .7, .4, 1)');
      }
      if (p.stay || calm()) { place(p.els, 0, 0); lift(p.els, false); }
      try { await p.commit(); } catch (er) { console.error(er); }
      // the chat slid out and the cards came up over it: put it back where it lives, under them
      place(p.els, 0, 0); lift(p.els, false);
    } else {
      if (d && !calm()) await Promise.all(p.els.map(el => anim(el, [{ transform: `translateX(${g.x}px)`, opacity: el.style.opacity || 1 }, { transform: 'none', opacity: 1 }], 240, SPRING)));
      place(p.els, 0, 0); lift(p.els, false);
    }
  }
  // a scroll under a sideways gesture means the browser took it: ours lets go
  document.addEventListener('scroll', () => { if (G && G.lock === 'h') G.scrolled = true; }, { passive: true, capture: true });
  document.addEventListener('touchstart', start, { passive: true });
  document.addEventListener('touchmove', move, { passive: true });
  document.addEventListener('touchend', e => end(e, false), { passive: true });
  document.addEventListener('touchcancel', e => end(e, true), { passive: true });

  // ---- swipe a message to reply
  let replyIcon = null;
  function replyBegin(g) {
    if (!replyIcon) { replyIcon = document.createElement('div'); replyIcon.id = 'mega-swreply'; replyIcon.setAttribute('aria-hidden', 'true'); replyIcon.innerHTML = '<svg class="ic"><use href="#i-reply"/></svg>'; wrap.append(replyIcon); }
    const r = g.row.getBoundingClientRect(), w = wrap.getBoundingClientRect();
    replyIcon.style.top = r.top - w.top + r.height / 2 - 15 + 'px'; replyIcon.classList.add('on'); replyIcon.classList.remove('go');
    g.row.classList.add('mswp'); g.row.style.setProperty('--sx', '0px'); g.x = 0;
  }
  function replyMove(g, dx) {
    g.x = dx; const x = T.replyX(dx), p = Math.min(1, dx / T.REPLY);
    if (calm()) { replyIcon.style.opacity = dx >= T.REPLY ? '1' : '0'; return; }
    g.row.style.setProperty('--sx', x.toFixed(1) + 'px');
    replyIcon.style.opacity = p.toFixed(2); replyIcon.style.transform = `translateX(${(x * .3).toFixed(1)}px) scale(${(.6 + .4 * p).toFixed(3)})`;
    replyIcon.classList.toggle('go', dx >= T.REPLY);
  }
  function replyEnd(g, go) {
    const row = g.row, x = T.replyX(g.x);
    row.classList.remove('mswp'); row.style.removeProperty('--sx');
    if (replyIcon) { replyIcon.classList.remove('on', 'go'); replyIcon.style.opacity = ''; replyIcon.style.transform = ''; }
    if (x && !calm() && row.isConnected) anim(row, [{ transform: `translateX(${x.toFixed(1)}px)` }, { transform: 'none' }], 240, SPRING);
    if (go && g.m) reply(g.m);
  }
  function reply(m) { if (!m || S.gid == null) return; S.reply = m; S.edit = null; renderComposer(); $('#input').focus(); }

  // ---- hold a message for its menu; tap it twice for a heart
  const QUICKS = typeof QUICK !== 'undefined' ? QUICK : ['👍', '❤️', '😂', '🎉', '😮', '🙏', '👀', '🥂'];
  const HEART = '❤️';
  const CONTROL = 'a, button, .react, .imgslot, .mediaslot, video, audio, input, textarea, select, .quote, .dl';
  const LP = { t: 0, row: null, x: 0, y: 0, fired: false }, TAP = { prev: null };
  const admin = () => { const c = typeof currentCommunity === 'function' && currentCommunity(); return !!c && (c.role === 'admin' || c.role === 'owner'); };
  async function react(m, em) { try { const mine = !!m.reactions.find(r => r.emoji === em && r.mine); await rpc('react', { gid: S.gid, id: m.id, emoji: em, on: !mine }); tick(); soon(); return !mine; } catch (er) { toast(er.message); return false; } }
  function menuFor(row, x, y) {
    const m = msgOf(row.dataset.id); if (!m || m.deleted) return;
    const text = m.kind === 'text' && m.text ? m.text : (m.file && m.text) || '';
    const items = [['reply', 'reply'], ...(text ? [['copy', 'copy text']] : []), ...(m.mine && m.kind === 'text' && !m.tmp ? [['edit', 'edit']] : []), ...((m.mine || admin()) && !m.tmp ? [['del', 'delete']] : [])];
    const html_ = `<div class="mmenu mtm" role="menu"><div class="mtm-e" role="group" aria-label="React">${QUICKS.map(em => `<button type="button" role="menuitem" data-e="${em}" class="${m.reactions.find(r => r.emoji === em && r.mine) ? 'on' : ''}" aria-label="React with ${em}">${em}</button>`).join('')}</div>${items.map(([k, l]) => `<button type="button" role="menuitem" data-k="${k}" class="${k === 'del' ? 'bad' : ''}">${l}</button>`).join('')}</div>`;
    row.classList.add('mheld');
    const p = A.popAt(html_, Math.min(x - 120, innerWidth - 260), y - 40);
    p.classList.add('mtouch');
    const was = M.closers[M.closers.length - 1]; M.closers[M.closers.length - 1] = () => { was(); row.classList.remove('mheld'); };
    p.onclick = async ev => {
      const b = ev.target.closest('button'); if (!b) return;
      A.closePops();
      if (b.dataset.e) { react(m, b.dataset.e); return; }
      const k = b.dataset.k;
      if (k === 'reply') reply(m);
      else if (k === 'copy') copy(text);
      else if (k === 'edit') startEdit(m);
      else if (k === 'del') { try { await rpc(m.file && m.file.large ? 'file_delete' : 'delete', { gid: S.gid, id: m.id }); tick(); soon(); } catch (er) { toast(er.message); } }
    };
    if (!calm()) anim(row, [{ transform: 'scale(1)' }, { transform: 'scale(1.02)' }, { transform: 'scale(1)' }], 260, 'ease-out');
  }
  async function heart(row, m, x, y) {
    const on = await react(m, HEART);
    if (on && !calm()) A.burst(x, y, hi());
  }
  stream.addEventListener('touchstart', e => {
    clearTimeout(LP.t); LP.fired = false; LP.row = null;
    if (e.touches.length !== 1 || !phone() || blocked()) return;
    const t = e.touches[0], row = e.target.closest && e.target.closest('.msg');
    if (!row || e.target.closest(CONTROL)) return;
    LP.row = row; LP.x = t.clientX; LP.y = t.clientY;
    LP.t = setTimeout(() => { LP.fired = true; G = null; menuFor(row, LP.x, LP.y); }, 450);
  }, { passive: true });
  stream.addEventListener('touchmove', e => { if (LP.t && Math.hypot(e.touches[0].clientX - LP.x, e.touches[0].clientY - LP.y) > 10) { clearTimeout(LP.t); LP.t = 0; } }, { passive: true });
  stream.addEventListener('touchcancel', () => { clearTimeout(LP.t); LP.t = 0; LP.fired = false; }, { passive: true });
  // not passive: the tap that ends a hold, and the second tap of a pair, must not also click
  stream.addEventListener('touchend', e => {
    clearTimeout(LP.t); const held = LP.fired; LP.fired = false; LP.t = 0;
    if (held) { e.preventDefault(); TAP.prev = null; return; }
    if (!LP.row || !phone() || e.changedTouches.length !== 1) return;
    const t = e.changedTouches[0], now = performance.now(), row = LP.row; LP.row = null;
    if (Math.hypot(t.clientX - LP.x, t.clientY - LP.y) > 10 || (G && G.lock)) { TAP.prev = null; return; }   // a drag, not a tap
    const id = row.dataset.id;
    if (T.doubleTap(TAP.prev, now, t.clientX, t.clientY, id)) { TAP.prev = null; e.preventDefault(); const m = msgOf(id); if (m && !m.deleted && !m.tmp) heart(row, m, t.clientX, t.clientY); return; }
    TAP.prev = { t: now, x: t.clientX, y: t.clientY, id };
  }, { passive: false });

  // ---- the picture viewer
  function viewer(src, alt) {
    const pics = [...document.querySelectorAll('#stream .imgslot img')].map(i => ({ src: i.src, alt: i.alt })); let idx = pics.findIndex(p => p.src === src);
    if (idx < 0) { pics.length = 0; pics.push({ src, alt }); idx = 0; }
    const v = document.createElement('div'); v.id = 'mega-view'; v.setAttribute('role', 'dialog'); v.setAttribute('aria-label', 'Picture');
    v.innerHTML = '<div class="mv-stage"><img alt=""></div><button type="button" class="mv-x" aria-label="Close">×</button><div class="mv-cap"><span class="mv-n"></span><span class="mv-t"></span></div>';
    const img = $('img', v), stage = $('.mv-stage', v);
    const Z = { s: 1, tx: 0, ty: 0 };
    const vw = () => innerWidth, vh = () => innerHeight;
    const apply = () => { img.style.transform = `translate(${Z.tx.toFixed(1)}px, ${Z.ty.toFixed(1)}px) scale(${Z.s.toFixed(4)})`; };
    const size = () => { const r = img.getBoundingClientRect(); return { w: r.width / Z.s, h: r.height / Z.s }; };
    const clamp = () => { const { w, h } = size(); Object.assign(Z, T.clampPan(Z.tx, Z.ty, Z.s, w, h, vw(), vh())); };
    const show = (i, from) => {
      idx = i; img.src = pics[i].src; img.alt = pics[i].alt || ''; $('.mv-n', v).textContent = pics.length > 1 ? `${i + 1} / ${pics.length}` : ''; $('.mv-t', v).textContent = pics[i].alt || '';
      Z.s = 1; Z.tx = 0; Z.ty = 0; apply(); v.style.setProperty('--mv-a', '1');
      if (from && !calm()) anim(img, [{ transform: `translateX(${from}px)`, opacity: .4 }, { transform: 'none', opacity: 1 }], 220, 'cubic-bezier(.2, .8, .3, 1)');
      const n = pics[i + 1], p = pics[i - 1]; if (n) new Image().src = n.src; if (p) new Image().src = p.src;
    };
    let gone = false;
    const close = async dy => { if (gone) return; gone = true; document.removeEventListener('keydown', key); if (!calm()) await anim(v, [{ opacity: 1, transform: dy ? `translateY(${dy}px)` : 'none' }, { opacity: 0, transform: `translateY(${dy ? dy + 160 : 0}px)` }], 180, 'ease-in'); v.remove(); };
    const key = e => { if (e.key === 'Escape') { e.stopPropagation(); close(0); } else if (e.key === 'ArrowRight' && pics[idx + 1]) show(idx + 1, 60); else if (e.key === 'ArrowLeft' && pics[idx - 1]) show(idx - 1, -60); };
    document.addEventListener('keydown', key); $('.mv-x', v).onclick = () => close(0);
    // touches: one finger pans a zoomed picture, or pulls down to close, or swipes to the next; two fingers pinch; two taps zoom
    let P = null, tapT = 0, lastTap = null;
    const pt = t => ({ x: t.clientX, y: t.clientY });
    v.addEventListener('touchstart', e => {
      clearTimeout(tapT); img.style.willChange = 'transform';
      if (e.touches.length >= 2) { const [a, b] = e.touches; P = { mode: 'pinch', p0: pt(a), p1: pt(b), s0: Z.s, tx0: Z.tx, ty0: Z.ty }; return; }
      const t = e.touches[0]; P = { mode: null, x0: t.clientX, y0: t.clientY, t0: performance.now(), tx0: Z.tx, ty0: Z.ty, samples: [], moved: false };
    }, { passive: true });
    v.addEventListener('touchmove', e => {
      e.preventDefault(); if (!P) return;
      if (P.mode === 'pinch') { if (e.touches.length < 2) return; const [a, b] = e.touches, r = v.getBoundingClientRect(); Object.assign(Z, T.pinch(P.p0, P.p1, pt(a), pt(b), P.s0, P.tx0, P.ty0, r.width / 2, r.height / 2)); apply(); return; }
      if (e.touches.length !== 1) return;
      const t = e.touches[0], dx = t.clientX - P.x0, dy = t.clientY - P.y0, now = performance.now();
      P.samples.push({ t: now, x: t.clientX, y: t.clientY }); if (P.samples.length > 12) P.samples.shift();
      if (!P.mode) {
        if (Z.s > 1.02) { if (Math.hypot(dx, dy) < 4) return; P.mode = 'pan'; }
        else { const l = T.lock(dx, dy); if (!l) return; P.mode = l === 'h' ? 'swipe' : dy > 0 ? 'down' : 'none'; }
        P.moved = true;
      }
      if (P.mode === 'pan') { Z.tx = P.tx0 + dx; Z.ty = P.ty0 + dy; clamp(); apply(); }
      else if (P.mode === 'down') { Z.ty = Math.max(0, dy); Z.s = Math.max(.7, 1 - dy / 1200); apply(); v.style.setProperty('--mv-a', Math.max(.2, 1 - dy / 500).toFixed(2)); }
      else if (P.mode === 'swipe') { const has = dx < 0 ? pics[idx + 1] : pics[idx - 1]; Z.tx = has ? dx : Math.sign(dx) * T.damp(Math.abs(dx), 30); apply(); }
    }, { passive: false });
    const endTouch = e => {
      const p = P; if (!p) return; if (e.touches.length) { if (p.mode === 'pinch' && e.touches.length === 1) { const t = e.touches[0]; P = { mode: Z.s > 1.02 ? 'pan' : 'none', x0: t.clientX, y0: t.clientY, t0: performance.now(), tx0: Z.tx, ty0: Z.ty, samples: [], moved: true }; } return; }
      P = null; img.style.willChange = '';
      const now = performance.now();
      if (p.mode === 'pinch') { if (Z.s < 1.05) { settle(1, 0, 0); } else { clamp(); apply(); } return; }
      if (p.mode === 'down') { const vy = T.velocity(p.samples.map(s => ({ t: s.t, x: s.y })), now); if (Z.ty > 90 || vy > .5) close(Z.ty); else settle(1, 0, 0); return; }
      if (p.mode === 'swipe') {
        const vx = T.velocity(p.samples, now), dx = Z.tx, dir = dx < 0 ? 1 : -1, has = pics[idx + dir];
        if (has && (Math.abs(dx) > 80 || Math.abs(vx) > .5)) { const w = vw(); (calm() ? Promise.resolve() : anim(img, [{ transform: `translateX(${dx}px)`, opacity: 1 }, { transform: `translateX(${-dir * w}px)`, opacity: .3 }], T.slideMs(w - Math.abs(dx), vx), 'ease-out')).then(() => show(idx + dir, dir * w * .4)); }
        else settle(1, 0, 0);
        return;
      }
      if (p.mode === 'pan') { clamp(); apply(); return; }
      if (!p.moved && now - p.t0 < 350) {   // a tap: two zoom, one closes a picture that is not zoomed
        const t = e.changedTouches[0], x = t.clientX, y = t.clientY;
        if (T.doubleTap(lastTap, now, x, y, 'v')) { lastTap = null; const r = v.getBoundingClientRect(); if (Z.s > 1.02) settle(1, 0, 0); else { const z = T.zoomAt(x, y, 1, 0, 0, 2.5, r.width / 2, r.height / 2), { w, h } = size(), c = T.clampPan(z.tx, z.ty, z.s, w, h, vw(), vh()); settle(z.s, c.tx, c.ty); } return; }
        lastTap = { t: now, x, y, id: 'v' };
        if (Z.s <= 1.02 && !e.target.closest('.mv-x')) tapT = setTimeout(() => close(0), 300);
      }
    };
    v.addEventListener('touchend', endTouch, { passive: true }); v.addEventListener('touchcancel', endTouch, { passive: true });
    function settle(s, tx, ty) { const from = img.style.transform; Z.s = s; Z.tx = tx; Z.ty = ty; apply(); v.style.setProperty('--mv-a', '1'); if (!calm()) anim(img, [{ transform: from || 'none' }, { transform: img.style.transform }], 260, SPRING); }
    v.addEventListener('click', e => { if (e.target === stage || e.target === v) { if (Z.s <= 1.02 && !matchMedia('(pointer: coarse)').matches) close(0); } });
    document.body.append(v); show(idx, 0);
    return v;
  }
  // the chat's picture click opens this viewer on a phone; a computer keeps the plain lightbox
  if (typeof root.lightbox === 'function') { const base = root.lightbox; root.lightbox = (src, alt) => (phone() ? viewer(src, alt) : base(src, alt)); }

  M.touch = { get gesture() { return G; }, viewer, menuFor, reply, mark, phone, T };
})(typeof window !== 'undefined' ? window : globalThis);
