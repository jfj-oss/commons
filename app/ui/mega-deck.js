'use strict';
// mega: the camera and screenshare stack. Every camera and screen share in the call you are in sits in one column on the left
// (a strip across the top on a phone); dragging a tile reorders it for everyone in the call, live.
// The column's right edge (the strip's lower edge on a phone) drags to make the pictures bigger or smaller. That size is this
// device's own: it is kept in localStorage and never sent to anyone.
//
// The tiles themselves are call-ui.js's (#call-stage > #c-shares > .ctile); this file only lays them out and orders them. The
// order is visual only (CSS `order`), so no <video> is ever moved in the DOM and no picture flickers.
//
// What goes over the live channel (see mega-live.js for why every event counts):
//   deck.m  {s, o, n, b, e, a}  a grab / a move / a drop / an answer.  s = the tile held ('' for none), o = the order (stable track
//           ids), n and b = the order's logical stamp (counter and the account that made it), e = 1 on release, a = 1 when it
//           answers a question.
//   deck.q  {}                  "what is the order?", asked once by a person who just joined a call that has other people in it.
// Budget: a 3 second drag is at most about 9 events (grab, one move per 450 ms at most, drop). Nothing is sent while idle.
//
// Rules a receiver keeps, whatever it is sent: only accounts currently in the call count; unknown tiles are ignored; the local
// order always holds every current tile exactly once; a higher stamp (counter, then account) wins everywhere, so all converge;
// a tile held by someone is theirs until 3 s of silence or until they leave, and two simultaneous grabs go to the lower account.
(() => {
  const root = document.documentElement;
  const HOLD_MS = 3000, MOVE_GAP = 450, MAX_TILES = 64;
  const D = { order: [], stamp: [0, ''], hi: 0, holds: new Map(), drag: null, hid: false, askedKey: '', ask: null, answer: null, lastSend: 0, trail: 0, sweep: 0 };
  const sidOk = s => typeof s === 'string' && s.length > 0 && s.length <= 200;
  const acctOk = s => typeof s === 'string' && /^[0-9a-f]{64}$/.test(s);
  const myAcct = () => (typeof S !== 'undefined' && S.snap && S.snap.me && S.snap.me.account) || '';
  const callObj = () => window.__commonsCall;
  const snap = () => { try { return callObj() && callObj().snapshot(); } catch { return null; } };
  const stage = () => document.getElementById('call-stage');
  const host = () => document.getElementById('c-shares');
  const sidOf = el => { const k = el.dataset.key || ''; return k.slice(k.indexOf('/') + 1); };
  const tiles = () => { const h = host(); return h ? [...h.querySelectorAll(':scope > .ctile')] : []; };
  const still = () => matchMedia('(prefers-reduced-motion: reduce)').matches || root.classList.contains('fx-lite');
  const cmp = (a, b) => a[0] - b[0] || (a[1] < b[1] ? -1 : a[1] > b[1] ? 1 : 0);
  /** accounts of the other people in the call (members only, never "unknown device"), with their names */
  function others() {
    const s = snap(), m = new Map();
    if (s && (s.phase === 'connected' || s.phase === 'reconnecting')) for (const p of s.peers) if (!p.me && p.member && p.account) m.set(p.account, p.name);
    return m;
  }
  const callGid = () => { const s = snap(); return s && (s.phase === 'connected' || s.phase === 'reconnecting') ? s.gid : null; };

  // ------------------------------------------------------------------ layout: the local order, always valid
  function layout(animate) {
    const els = tiles(), by = new Map(els.map(e => [sidOf(e), e]));
    const seen = new Set(), next = [];
    for (const s of D.order) if (by.has(s) && !seen.has(s)) { seen.add(s); next.push(s); }
    for (const e of els) { const s = sidOf(e); if (!seen.has(s)) { seen.add(s); next.push(s); } }
    D.order = next;
    const before = animate && !still() ? new Map(els.map(e => [e, e.getBoundingClientRect()])) : null;
    next.forEach((s, i) => { const e = by.get(s); if (e.style.order !== String(i)) e.style.order = String(i); });
    if (before) for (const e of els) {
      if (D.drag && D.drag.el === e) continue;
      const a = before.get(e), b = e.getBoundingClientRect(), dx = a.left - b.left, dy = a.top - b.top;
      if (Math.abs(dx) + Math.abs(dy) > 1) e.animate([{ transform: `translate(${dx}px,${dy}px)` }, { transform: 'none' }], { duration: 240, easing: 'cubic-bezier(.2,1,.3,1)' });
    }
    marks();
  }
  /** the received order, made valid against what is really here: unknown ids dropped, missing tiles keep their place at the end */
  function merged(o) {
    const known = new Set(tiles().map(sidOf)), out = [], seen = new Set();
    for (const s of o) if (known.has(s) && !seen.has(s)) { seen.add(s); out.push(s); }
    for (const s of D.order) if (known.has(s) && !seen.has(s)) { seen.add(s); out.push(s); }
    return out;
  }
  function marks() {
    const who = others(), now = Date.now();
    for (const e of tiles()) {
      const h = D.holds.get(sidOf(e)), live = h && now - h.at <= HOLD_MS && (h.acct === myAcct() || who.has(h.acct));
      if (live && h.acct !== myAcct()) { e.dataset.holder = who.get(h.acct) || 'someone'; e.classList.add('held'); } else { delete e.dataset.holder; e.classList.remove('held'); }
    }
  }

  // ------------------------------------------------------------------ sending
  function payload(sid, extra) { return JSON.stringify({ s: sid || '', o: D.order, n: D.stamp[0], b: D.stamp[1], ...extra }); }
  function sendMove(sid, end) {
    const gid = callGid(); if (!gid || !others().size) return;   // nobody else in the call: nothing to tell
    clearTimeout(D.trail); D.trail = 0; D.lastSend = Date.now();
    __megaLive.send(gid, 'deck.m', payload(sid, { e: end ? 1 : 0 }));
  }
  function moved(sid) {   // the order changed because of me: new stamp, sent at most every MOVE_GAP
    D.stamp = [Math.max(D.stamp[0], D.hi) + 1, myAcct()]; D.hi = D.stamp[0];
    const wait = D.lastSend + MOVE_GAP - Date.now();
    if (wait <= 0) sendMove(sid, false);
    else if (!D.trail) D.trail = setTimeout(() => { D.trail = 0; if (D.drag) sendMove(D.drag.sid, false); }, wait);
  }

  // ------------------------------------------------------------------ receiving
  function onEvent(e) {
    if (e.kind !== 'deck.m' && e.kind !== 'deck.q') return;
    if (e.gid !== callGid()) return;
    const who = others(); if (!who.has(e.account)) return;   // only people in the call count
    let d; try { d = JSON.parse(e.data); } catch { return; }
    if (!d || typeof d !== 'object') return;
    if (e.kind === 'deck.q') { wantAnswer(); return; }
    const n = d.n, o = d.o, s = d.s === '' || d.s == null ? '' : d.s;
    if (!Number.isInteger(n) || n < 0 || n > 2 ** 40 || !Array.isArray(o) || o.length > MAX_TILES || !o.every(sidOk) || (s !== '' && !sidOk(s))) return;
    const stamp = [n, acctOk(d.b) ? d.b : e.account];
    if (d.a) { clearTimeout(D.answer); D.answer = 0; }   // somebody answered the question: stay quiet
    // the hold
    let take = true;
    if (s && tiles().some(t => sidOf(t) === s)) {
      const h = D.holds.get(s), now = Date.now();
      if (d.e) { if (h && h.acct === e.account) D.holds.delete(s); }
      else {
        const mine = D.drag && D.drag.sid === s;
        const free = !h || h.acct === e.account || now - h.at > HOLD_MS || !who.has(h.acct);
        if (mine) { if (e.account < myAcct()) yieldDrag(); else take = false; }
        else if (!free && e.account > h.acct) take = false;
        if (take) D.holds.set(s, { acct: e.account, at: now });
      }
      kickSweep();
    }
    if (!take) return;
    // the order: a higher stamp wins; keep my own place for a tile I am dragging right now
    D.hi = Math.max(D.hi, n);
    if (cmp(stamp, D.stamp) > 0) {
      D.stamp = stamp;
      let next = merged(o);
      if (D.drag) { const at = D.order.indexOf(D.drag.sid); next = next.filter(x => x !== D.drag.sid); next.splice(Math.min(Math.max(at, 0), next.length), 0, D.drag.sid); }
      D.order = next; layout(true);
    }
    marks();
  }
  function wantAnswer() {
    if (D.stamp[0] === 0 || D.answer) return;   // nobody has reordered anything: the default order is already everyone's
    D.answer = setTimeout(() => {
      D.answer = 0;
      const gid = callGid(); if (!gid) return;
      __megaLive.send(gid, 'deck.m', payload('', { e: 1, a: 1 }));
    }, 250 + Math.random() * 1000);
  }
  function askOnce() {
    const s = snap(), gid = callGid();
    if (!gid) { D.askedKey = ''; return; }
    const key = gid + '|' + (s.attempt || 0);
    if (D.askedKey === key || !others().size || !tiles().length) return;
    D.askedKey = key;
    D.ask = setTimeout(() => { D.ask = 0; if (callGid() === gid && others().size) __megaLive.send(gid, 'deck.q', '{}'); }, 800);
  }

  // ------------------------------------------------------------------ holds expire by themselves
  function kickSweep() { if (!D.sweep) D.sweep = setInterval(sweep, 500); }
  function sweep() {
    const who = others(), now = Date.now();
    for (const [s, h] of [...D.holds]) if (now - h.at > HOLD_MS || (h.acct !== myAcct() && !who.has(h.acct))) D.holds.delete(s);
    marks();
    if (!D.holds.size) { clearInterval(D.sweep); D.sweep = 0; }
  }

  // ------------------------------------------------------------------ dragging
  function yieldDrag() { const g = D.drag; if (!g) return; endDrag(false); const e = g.el; e.classList.add('busy'); setTimeout(() => e.classList.remove('busy'), 700); }
  function endDrag(send) {
    const g = D.drag; if (!g) return; D.drag = null;
    clearTimeout(g.lp); clearTimeout(D.trail); D.trail = 0;
    g.el.classList.remove('dragging'); g.el.style.transform = ''; g.el.style.zIndex = '';
    try { g.el.releasePointerCapture(g.id); } catch { }
    removeEventListener('pointermove', onMove, true); removeEventListener('pointerup', onUp, true); removeEventListener('pointercancel', onUp, true);
    if (g.active) { D.holds.delete(g.sid); if (send) sendMove(g.sid, true); layout(true); }
  }
  function startDrag(g) {
    const h = D.holds.get(g.sid), now = Date.now();
    if (h && h.acct !== myAcct() && now - h.at <= HOLD_MS && others().has(h.acct)) { g.el.classList.add('busy'); setTimeout(() => g.el.classList.remove('busy'), 700); endDrag(false); return; }   // somebody has it
    g.active = true; g.el.classList.add('dragging'); g.el.style.zIndex = '5';
    const r = g.el.getBoundingClientRect(); g.gx = g.x - r.left; g.gy = g.y - r.top;
    D.holds.set(g.sid, { acct: myAcct(), at: now });
    sendMove(g.sid, false);   // the grab: everyone sees whose it is
  }
  function onMove(ev) {
    const g = D.drag; if (!g || ev.pointerId !== g.id) return;
    g.x = ev.clientX; g.y = ev.clientY;
    if (!g.active) { if (g.touch) { if (Math.hypot(g.x - g.x0, g.y - g.y0) > 10) endDrag(false); return; } if (Math.hypot(g.x - g.x0, g.y - g.y0) < 6) return; startDrag(g); if (!D.drag) return; }
    ev.preventDefault();
    place(g);
  }
  function place(g) {
    const h = host(), el = g.el; if (!h) return;
    const row = getComputedStyle(h).flexDirection === 'row';
    el.style.transform = 'none';
    const base = el.getBoundingClientRect();
    el.style.transform = `translate(${g.x - g.gx - base.left}px,${g.y - g.gy - base.top}px)`;
    // where would it sit now: after every other tile whose middle is before the middle of the dragged one
    const hr = h.getBoundingClientRect();
    const mid = row ? g.x - g.gx - hr.left + el.offsetWidth / 2 : g.y - g.gy - hr.top + el.offsetHeight / 2;
    const rest = D.order.filter(s => s !== g.sid), by = new Map(tiles().map(t => [sidOf(t), t]));
    let at = 0;
    for (const s of rest) { const t = by.get(s); if (!t) continue; const c = row ? t.offsetLeft + t.offsetWidth / 2 : t.offsetTop + t.offsetHeight / 2; if (c < mid) at++; }
    const cur = D.order.indexOf(g.sid);
    if (at !== cur) {
      rest.splice(at, 0, g.sid); D.order = rest; layout(true);
      el.style.transform = 'none'; const b2 = el.getBoundingClientRect(); el.style.transform = `translate(${g.x - g.gx - b2.left}px,${g.y - g.gy - b2.top}px)`;
      moved(g.sid);
    }
    D.holds.set(g.sid, { acct: myAcct(), at: Date.now() });
  }
  function onUp(ev) { const g = D.drag; if (!g || ev.pointerId !== g.id) return; endDrag(ev.type === 'pointerup'); }
  function onDown(ev) {
    const el = ev.target.closest && ev.target.closest('.ctile');
    if (!el || ev.button > 0 || ev.target.closest('.bar select, .bar button, .bar a') || D.drag) return;
    const sid = sidOf(el); if (!sid || !callGid()) return;
    const touch = ev.pointerType === 'touch';
    const g = D.drag = { el, sid, id: ev.pointerId, x0: ev.clientX, y0: ev.clientY, x: ev.clientX, y: ev.clientY, active: false, touch, lp: 0 };
    if (touch) g.lp = setTimeout(() => { g.touch = false; if (D.drag === g && !g.active) { startDrag(g); if (D.drag === g) place(g); } }, 320);
    try { el.setPointerCapture(ev.pointerId); } catch { }
    addEventListener('pointermove', onMove, true); addEventListener('pointerup', onUp, true); addEventListener('pointercancel', onUp, true);
  }

  // ------------------------------------------------------------------ the column, hiding it, sliding the rest over
  function head() {
    const st = stage(); if (!st || document.getElementById('mega-deckhead')) return;
    const h = document.createElement('div'); h.id = 'mega-deckhead';
    h.innerHTML = '<span class="mdh-t">cameras and screens</span><button type="button" id="mega-deckhide" aria-label="Hide the camera stack for me">hide</button>';
    st.prepend(h);
    h.querySelector('button').onclick = () => { D.hid = true; update(); };
    const show = document.createElement('button'); show.id = 'mega-deckshow'; show.type = 'button'; show.hidden = true;
    show.onclick = () => { D.hid = false; update(); };
    document.body.append(show);
  }
  function update() {
    head();
    const st = stage(), n = tiles().length, call = callGid();
    if (!n || !call) D.hid = false;
    if (st && st.hidden) D.hid = false;
    const open = !!st && !st.hidden && !D.hid && !!call;
    st && st.classList.toggle('mega-hid', D.hid);
    root.classList.toggle('deck-open', open);
    const show = document.getElementById('mega-deckshow');
    if (show) { show.hidden = !(D.hid && call); show.textContent = n ? `show ${n} ${n === 1 ? 'tile' : 'tiles'}` : 'show player'; }
    layout(false);
    askOnce();
    if (!call) { D.holds.clear(); D.stamp = [0, '']; D.hi = 0; D.order = []; D.askedKey = ''; }
  }

  // ------------------------------------------------------------------ the size: drag the edge
  // Wide: the column is 200px at least and leaves the chat 420px. Phone: the strip is 84px at least, and stops where one picture fills the width (60% of the height at most).
  // mega.css clamps the same way, so a window made smaller later never leaves the column too wide.
  const SZ = { w: 'mega.deck.w', h: 'mega.deck.h', drag: null };
  const phone = () => matchMedia('(max-width: 760px)').matches;
  const limits = ph => ph ? { min: 84, max: Math.max(84, Math.round(Math.min(innerHeight * .6, innerWidth * .5625 + 14))), def: 104 } : { min: 200, max: Math.max(200, innerWidth - 420), def: Math.round(Math.min(270, innerWidth * .24)) };
  const sizeGet = k => { try { const v = Number(localStorage.getItem(k)); return v > 0 && v < 20000 ? v : 0; } catch { return 0; } };
  const sizeNow = ph => { const st = stage(), r = st && st.getBoundingClientRect(); return r && r.width ? Math.round(ph ? r.height : r.width) : limits(ph).def; };
  /** v: pixels, or 0 for "as it comes". Returns what was really set. */
  function sizeSet(ph, v, save) {
    const L = limits(ph), cls = ph ? 'deck-hset' : 'deck-wset', prop = ph ? '--m-deckhset' : '--m-deckset';
    if (v) { v = Math.round(Math.min(L.max, Math.max(L.min, v))); root.style.setProperty(prop, v + 'px'); root.classList.add(cls); }
    else { root.style.removeProperty(prop); root.classList.remove(cls); }
    if (save) { try { if (v) localStorage.setItem(ph ? SZ.h : SZ.w, String(v)); else localStorage.removeItem(ph ? SZ.h : SZ.w); } catch { } }
    gripSay();
    return v;
  }
  function gripSay() {
    const g = document.getElementById('mega-deckgrip'); if (!g) return;
    const ph = phone(), L = limits(ph);
    g.setAttribute('aria-orientation', ph ? 'horizontal' : 'vertical');
    g.setAttribute('aria-valuemin', L.min); g.setAttribute('aria-valuemax', L.max); g.setAttribute('aria-valuenow', Math.min(L.max, Math.max(L.min, sizeNow(ph))));
  }
  function sizeEnd(ev) {
    const d = SZ.drag; if (!d || (ev && ev.pointerId !== d.id)) return;
    SZ.drag = null; cancelAnimationFrame(d.raf);
    root.classList.remove('deck-sizing');
    try { d.el.releasePointerCapture(d.id); } catch { }
    if (d.moved) sizeSet(d.ph, d.v, true);
  }
  function grip() {
    if (document.getElementById('mega-deckgrip')) return;
    const g = document.createElement('div'); g.id = 'mega-deckgrip'; g.tabIndex = 0;
    g.setAttribute('role', 'separator'); g.setAttribute('aria-label', 'Size of the cameras and screens'); g.title = 'Drag to resize. Double-click to put it back.';
    document.body.append(g);
    g.addEventListener('pointerdown', ev => {
      if (ev.button > 0 || SZ.drag) return;
      ev.preventDefault();
      const ph = phone(), now = sizeNow(ph);
      SZ.drag = { el: g, id: ev.pointerId, ph, from: ph ? ev.clientY : ev.clientX, start: now, v: now, moved: false, raf: 0 };
      try { g.setPointerCapture(ev.pointerId); } catch { }
      root.classList.add('deck-sizing');
    });
    g.addEventListener('pointermove', ev => {
      const d = SZ.drag; if (!d || ev.pointerId !== d.id) return;
      ev.preventDefault();
      const v = d.start + (d.ph ? ev.clientY : ev.clientX) - d.from;
      if (!d.moved && Math.abs(v - d.start) < 3) return;   // a click or a double-click is not a resize
      d.moved = true; d.v = v;
      if (!d.raf) d.raf = requestAnimationFrame(() => { d.raf = 0; if (SZ.drag === d) d.v = sizeSet(d.ph, d.v, false); });   // the chat re-flows with it: once a frame at most
    });
    g.addEventListener('pointerup', sizeEnd); g.addEventListener('pointercancel', sizeEnd); g.addEventListener('lostpointercapture', sizeEnd);
    g.addEventListener('touchmove', e => { if (SZ.drag) e.preventDefault(); }, { passive: false });
    g.addEventListener('dblclick', () => sizeSet(phone(), 0, true));
    g.addEventListener('keydown', ev => {
      const ph = phone(), k = ev.key;
      const step = (ph ? { ArrowDown: 1, ArrowUp: -1 } : { ArrowRight: 1, ArrowLeft: -1 })[k];
      if (step) { ev.preventDefault(); sizeSet(ph, sizeNow(ph) + step * (ev.shiftKey ? 96 : 24), true); }
      else if (k === 'Home') { ev.preventDefault(); sizeSet(ph, limits(ph).min, true); }
      else if (k === 'End') { ev.preventDefault(); sizeSet(ph, limits(ph).max, true); }
    });
    for (const ph of [false, true]) { const v = sizeGet(ph ? SZ.h : SZ.w); if (v) { root.style.setProperty(ph ? '--m-deckhset' : '--m-deckset', v + 'px'); root.classList.add(ph ? 'deck-hset' : 'deck-wset'); } }
    addEventListener('resize', gripSay);
    gripSay();
  }

  function boot() {
    const h = host(); if (!h) return setTimeout(boot, 400);
    head(); grip();
    h.addEventListener('pointerdown', onDown);
    h.addEventListener('touchmove', e => { if (D.drag && D.drag.active) e.preventDefault(); }, { passive: false });
    new MutationObserver(() => update()).observe(h, { childList: true });
    new MutationObserver(() => update()).observe(stage(), { attributes: true, attributeFilter: ['hidden'] });
    setInterval(update, 700);
    __megaLive.sub('deck', () => { const g = callGid(); return g ? [g] : []; }, onEvent);
    update();
  }
  window.__megaDeck = {
    order: () => [...D.order], stamp: () => [...D.stamp], holds: () => [...D.holds].map(([s, h]) => [s, h.acct, Date.now() - h.at]),
    tiles: () => tiles().map(sidOf), hidden: () => D.hid, update, onEvent,
    size: () => ({ phone: phone(), now: sizeNow(phone()), ...limits(phone()) }), setSize: v => sizeSet(phone(), v, true),
  };
  boot();
})();
