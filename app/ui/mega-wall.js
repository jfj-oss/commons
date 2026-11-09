'use strict';
// mega: the shared ASCII wall. Anyone in a room can draw on the room's backdrop, live, for everyone.
//
// The wall itself lives in the encrypted core (crates/core/src/wall.rs): it merges cell by cell, enforces the owner's lock and
// clear (kept in the owner-signed style document), validates everything that comes in, and keeps the wall on this device. This file
// is the picture and the pen: a canvas over the backdrop, a small toolbar, and the pacing.
//
// Pacing, because every event is kept forever on the relay and makes the sender's device rewrite its state (see mega-live.js):
//  - nothing is sent while the pen is idle; a stroke is batched and goes out at most twice a second, 200 cells at most per event;
//  - nothing is sent into a room where nobody is around to see it (cells stay on this device and reach others in a snapshot when
//    they open the room): somebody is around when they sent a wall event lately, or posted in the room lately.
// Fixed grid: 120 x 54 cells in every room on every device, centred and scaled to the window (so a phone draws the same picture, smaller).
(() => {
  const root = document.documentElement;
  const COLS = 120, ROWS = 54, CW = 10, CH = 17, GLYPHS = ' #*+.o=~/|-x';
  const GAP = 500, BATCH = 200, PRESENT_MS = 180000, MSG_MS = 600000, ASK_EVERY = 600000;
  const W = { gid: null, canvas: null, cells: new Map(), pend: new Map(), out: new Map(), rev: 0, gen: 0, locked: false, on: false, brush: 1, col: 0,
    peerAt: 0, snapAt: 0, heard: 0, lastMask: 0, maskT: 0, timer: 0, sendT: 0, tries: 0, flushing: false, lastSend: 0, busy: false, txt: null, raf: 0, answer: 0, askedAt: 0, last: null, ro: null, sizeKey: '' };
  const $q = (s, r = document) => r.querySelector(s);
  const main = () => document.getElementById('main');
  const theme = () => (window.__megaTheme && W.gid && window.__megaTheme.colours(W.gid)) || ['#9aa0b0', '#ffffff'];
  const comm = () => typeof S !== 'undefined' && S.snap && S.snap.communities.find(c => c.channels.some(x => x.gid === W.gid));
  const isOwner = () => (comm() || {}).role === 'owner';
  const store = { get(k) { try { return Number(localStorage.getItem('mega.' + k)) || 0; } catch { return 0; } }, set(k, v) { try { localStorage.setItem('mega.' + k, String(v)); } catch { } } };

  // ------------------------------------------------------------------ geometry and painting
  function geo() {
    const b = main().getBoundingClientRect(), s = Math.min(b.width / (COLS * CW), b.height / (ROWS * CH));
    return { b, s, cw: CW * s, ch: CH * s, ox: (b.width - COLS * CW * s) / 2, oy: (b.height - ROWS * CH * s) / 2 };
  }
  function size() {
    const c = W.canvas, m = main(); if (!c || !m) return;
    const dpr = Math.min(devicePixelRatio || 1, 2), w = m.clientWidth, h = m.clientHeight, key = w + 'x' + h + 'x' + dpr;
    if (key === W.sizeKey) return; W.sizeKey = key;
    c.width = Math.round(w * dpr); c.height = Math.round(h * dpr); schedule();
  }
  function schedule() { if (!W.raf) W.raf = requestAnimationFrame(() => { W.raf = 0; paint(); }); }
  function marks() {   // what to show: the core's wall with my not-yet-confirmed cells on top
    const m = new Map(W.cells);
    for (const [p, v] of W.pend) { if (v[0] === 0) m.delete(p); else m.set(p, v); }
    return m;
  }
  function paint() {
    const c = W.canvas; if (!c) return;
    const g = c.getContext('2d'), dpr = c.width / (main().clientWidth || 1), G = geo();
    g.clearRect(0, 0, c.width, c.height);
    const [c0, c1] = theme();
    g.font = `700 ${Math.max(6, Math.round(14 * G.s * dpr))}px "Geist Mono", "IBM Plex Mono", ui-monospace, monospace`; g.textAlign = 'center'; g.textBaseline = 'middle';
    const hide = W.on ? null : W.txt;   // while drawing, show everything; otherwise marks stay out from under the words
    for (const [p, v] of marks()) {
      const x = p % COLS, y = (p / COLS) | 0;
      if (hide && hide[p]) continue;
      g.fillStyle = v[1] ? c1 : c0;
      g.fillText(GLYPHS[v[0]], (G.ox + (x + .5) * G.cw) * dpr, (G.oy + (y + .5) * G.ch) * dpr);
    }
  }
  /** Which wall cells have words on top, for painting around them. Called after the backdrop has looked at the page. */
  function remask() {
    if (!W.canvas || !window.__mega || !__mega.eachRect) return;
    const wait = W.lastMask + 120 - Date.now();
    if (wait > 0) { if (!W.maskT) W.maskT = setTimeout(() => { W.maskT = 0; remask(); }, wait); return; }
    W.lastMask = Date.now();
    const G = geo(), t = W.txt && W.txt.length === COLS * ROWS ? W.txt : (W.txt = new Uint8Array(COLS * ROWS)); t.fill(0);
    __mega.eachRect(r => {
      if (!r.width && !r.height) return;
      const x0 = Math.max(0, Math.floor((r.left - G.b.left - G.ox) / G.cw)), x1 = Math.min(COLS - 1, Math.floor((r.right - G.b.left - G.ox) / G.cw));
      const y0 = Math.max(0, Math.floor((r.top - G.b.top - G.oy) / G.ch)), y1 = Math.min(ROWS - 1, Math.floor((r.bottom - G.b.top - G.oy) / G.ch));
      for (let y = y0; y <= y1; y++) for (let x = x0; x <= x1; x++) t[y * COLS + x] = 1;
    });
    schedule();
  }
  /** The backdrop's own mask (its grid is different): cover every cell that holds a mark, so the ASCII flows around the drawing too. */
  function fillMask(mask, cols, rows, b) {
    if (!W.gid) return;
    const G = geo(), kw = b.width / cols, kh = b.height / rows;
    for (const p of marks().keys()) {
      const x = p % COLS, y = (p / COLS) | 0;
      const l = G.b.left + G.ox + x * G.cw - b.left, t = G.b.top + G.oy + y * G.ch - b.top;
      const x0 = Math.max(0, Math.floor(l / kw)), x1 = Math.min(cols - 1, Math.floor((l + G.cw) / kw)), y0 = Math.max(0, Math.floor(t / kh)), y1 = Math.min(rows - 1, Math.floor((t + G.ch) / kh));
      for (let yy = y0; yy <= y1; yy++) for (let xx = x0; xx <= x1; xx++) mask[yy * cols + xx] = 1;
    }
  }

  // ------------------------------------------------------------------ the room's wall from the core
  async function refresh() {
    const gid = W.gid; if (!gid || W.busy || document.visibilityState !== 'visible') return;
    W.busy = true;
    try {
      const r = await rpc('wall_get', { gid, rev: W.rev });
      if (W.gid !== gid || r.busy) return;
      W.gen = r.gen; W.peerAt = (r.peer_at || 0) * 1000; W.snapAt = (r.snap_at || 0) * 1000;
      shareBack();
      const lockChanged = W.locked !== r.locked; W.locked = r.locked;
      if (!r.same) {
        W.rev = r.rev; W.cells = new Map(r.cells.filter(c => c[1] !== 0).map(c => [c[0], [c[1], c[2]]]));
        for (const [p, v] of [...W.pend]) { const k = W.cells.get(p); if (!W.out.has(p) && ((k && k[0] === v[0] && k[1] === v[1]) || (!k && v[0] === 0))) W.pend.delete(p); }
        schedule(); if (window.__mega && __mega.relook) __mega.relook();
      }
      if (lockChanged || !r.same) chrome();
    } catch { } finally { W.busy = false; }
  }
  function present() {
    const now = Date.now();
    if (now - W.peerAt < PRESENT_MS || now - W.heard < PRESENT_MS) return true;
    const ms = typeof S !== 'undefined' && S.msgs && S.gid === W.gid ? S.msgs.messages : [];
    for (let i = ms.length - 1; i >= 0 && i >= ms.length - 30; i--) { const m = ms[i]; if (m && !m.mine && m.kind === 'text' && now / 1000 - m.ts < MSG_MS / 1000) return true; }
    return false;
  }

  // ------------------------------------------------------------------ the pen
  function put(x, y) {
    if (x < 0 || y < 0 || x >= COLS || y >= ROWS) return;
    const p = y * COLS + x, v = [W.brush, W.col];
    const cur = W.pend.get(p) || W.cells.get(p);
    if (cur && cur[0] === v[0] && cur[1] === v[1]) return;
    if (!cur && v[0] === 0) return;
    W.pend.set(p, v); W.out.set(p, v);
  }
  function cellAt(ev) { const G = geo(); return [Math.floor((ev.clientX - G.b.left - G.ox) / G.cw), Math.floor((ev.clientY - G.b.top - G.oy) / G.ch)]; }
  function stroke(ev, from) {
    const [x, y] = cellAt(ev), [x0, y0] = from || [x, y], n = Math.max(Math.abs(x - x0), Math.abs(y - y0), 1);
    for (let i = 0; i <= n; i++) put(Math.round(x0 + (x - x0) * i / n), Math.round(y0 + (y - y0) * i / n));
    schedule(); queue(); return [x, y];
  }
  function queue() {
    if (W.sendT || W.flushing || !W.out.size) return;
    W.sendT = setTimeout(flush, Math.max(60, W.lastSend + GAP - Date.now()));
  }
  async function flush() {
    W.sendT = 0; const gid = W.gid; if (!gid || !W.out.size || W.flushing) return;
    W.flushing = true; W.lastSend = Date.now();
    const keys = [...W.out.keys()].slice(0, BATCH), cells = keys.map(p => [p % COLS, (p / COLS) | 0, W.out.get(p)[0], W.out.get(p)[1]]);
    const share = present();
    if (!share) { W.localOnly = gid; ask('draw'); }   // nobody seen yet: it stays here until someone turns up, and is then sent as a snapshot
    let r = null; try { r = await rpc('wall_draw', { gid, cells, share }); } catch (e) { r = { error: true }; }
    W.flushing = false;
    if (W.gid !== gid) return;
    if (r.error) { for (const k of keys) { W.out.delete(k); W.pend.delete(k); } }   // refused (locked, bad cell): nothing is kept
    else if (r.sent) { for (const k of keys) W.out.delete(k); W.tries = 0; if (share) __megaLive.count('wall.d'); refresh(); }
    else if (++W.tries > 12) { for (const k of keys) W.out.delete(k); W.tries = 0; }   // the room could not take it for a long while: it stays on this device
    queue();
  }

  // ------------------------------------------------------------------ asking for, and answering with, a snapshot
  function ask(why) {
    const gid = W.gid; if (!gid) return;
    const key = 'wallask.' + gid, last = store.get(key), now = Date.now();
    if (now - last < (why === 'draw' ? 120000 : ASK_EVERY)) return;
    store.set(key, now); W.askedAt = now;
    let n = 0; const go = async () => { if (W.gid !== gid) return; if (!(await __megaLive.send(gid, 'wall.q', '{}')) && ++n < 4) setTimeout(go, 3000); }; go();
  }
  function onLive(e) {
    if (!W.gid || e.gid !== W.gid) return;
    if (e.kind === 'wall.q') { W.heard = Date.now(); wantAnswer(Date.now()); }
    else if (e.kind === 'wall.h') { W.heard = Date.now(); clearTimeout(W.answer); W.answer = 0; }
    shareBack();
  }
  function wantAnswer(since) {
    if (W.answer) return;
    const gid = W.gid;
    W.answer = setTimeout(async () => {
      W.answer = 0; if (W.gid !== gid) return;
      await refresh();
      if (W.snapAt > since) return;   // somebody else already answered
      if (!(await sendSnapshot(gid))) __megaLive.send(gid, 'wall.h', '{}');
    }, 300 + Math.random() * 1200);
  }
  async function sendSnapshot(gid) {
    let chunks = []; try { chunks = (await rpc('wall_snapshot', { gid })).chunks || []; } catch { }
    for (let i = 0; i < chunks.length && W.gid === gid; i++) { if (i) await new Promise(r => setTimeout(r, 650)); await __megaLive.send(gid, 'wall.s', chunks[i]); }
    return chunks.length;
  }
  // What was drawn while nobody seemed to be around goes out once, as soon as someone is.
  function shareBack() {
    const gid = W.gid; if (!gid || W.localOnly !== gid || !present()) return;
    W.localOnly = null;
    setTimeout(() => { if (W.gid === gid) sendSnapshot(gid); }, 300 + Math.random() * 700);
  }

  // ------------------------------------------------------------------ the header button and the toolbar
  function chrome() {
    const bar = $q('#head .head-actions'); if (!bar || !W.gid) return;
    let b = $q('#btn-wall');
    if (!b) {
      b = document.createElement('button'); b.id = 'btn-wall'; b.type = 'button'; b.className = 'btn ghost'; b.innerHTML = '<span class="lbl">Draw</span>';
      b.title = 'Draw on this room\'s backdrop, for everyone'; b.onclick = () => (W.on ? off() : on());
      bar.insertBefore(b, $q('#btn-call', bar) || $q('#btn-invite', bar) || bar.firstChild);
    }
    const blocked = W.locked && !isOwner();
    b.hidden = blocked; b.setAttribute('aria-pressed', W.on ? 'true' : 'false');
    if (blocked && W.on) { off(); try { toast('The owner locked the wall'); } catch { } }
    const t = $q('#mega-wallbar');
    if (t) { const lk = $q('#mw-lock', t); if (lk) { lk.setAttribute('aria-pressed', W.locked ? 'true' : 'false'); lk.textContent = W.locked ? 'locked' : 'lock'; } }
  }
  function toolbar() {
    let t = $q('#mega-wallbar'); if (t) t.remove();
    const m = main(); t = document.createElement('div'); t.id = 'mega-wallbar'; t.setAttribute('role', 'toolbar'); t.setAttribute('aria-label', 'Drawing tools');
    const [c0, c1] = theme();
    t.innerHTML = `<span class="mwb-tag">everyone sees this</span><div class="mwb-g" id="mw-glyphs">${[...GLYPHS].map((g, i) => i ? `<button type="button" data-g="${i}" aria-pressed="${i === W.brush}" aria-label="glyph ${g}">${g === '<' ? '&lt;' : g}</button>` : '').join('')}<button type="button" data-g="0" aria-pressed="${W.brush === 0}">erase</button></div>
      <div class="mwb-c" id="mw-cols"><button type="button" data-c="0" aria-pressed="${W.col === 0}" aria-label="main colour" style="--sw:${c0}"></button><button type="button" data-c="1" aria-pressed="${W.col === 1}" aria-label="highlight colour" style="--sw:${c1}"></button></div>
      ${isOwner() ? `<button type="button" class="mwb-b" id="mw-lock" aria-pressed="${W.locked}">${W.locked ? 'locked' : 'lock'}</button><button type="button" class="mwb-b" id="mw-clear">clear</button>` : ''}
      <button type="button" class="mwb-b" id="mw-done">done</button>`;
    m.append(t);
    t.onclick = async ev => {
      const b = ev.target.closest('button'); if (!b) return;
      if (b.dataset.g != null) { W.brush = +b.dataset.g; $$q('#mw-glyphs button', t).forEach(x => x.setAttribute('aria-pressed', String(x === b))); }
      else if (b.dataset.c != null) { W.col = +b.dataset.c; $$q('#mw-cols button', t).forEach(x => x.setAttribute('aria-pressed', String(x === b))); }
      else if (b.id === 'mw-done') off();
      else if (b.id === 'mw-lock') { try { await rpc('set_wall', { cid: (comm() || {}).id, room: W.gid, lock: W.locked ? '0' : '1' }); W.locked = !W.locked; chrome(); } catch (e) { try { toast(e.message); } catch { } } }
      else if (b.id === 'mw-clear') {
        if (b.dataset.sure) {
          delete b.dataset.sure; b.textContent = 'clear';
          try { await rpc('set_wall', { cid: (comm() || {}).id, room: W.gid, clear: true }); W.pend.clear(); W.out.clear(); W.cells = new Map(); W.rev = 0; schedule(); refresh(); } catch (e) { try { toast(e.message); } catch { } }
        } else { b.dataset.sure = '1'; b.textContent = 'clear for everyone?'; setTimeout(() => { if (b.isConnected) { delete b.dataset.sure; b.textContent = 'clear'; } }, 4000); }
      }
    };
  }
  const $$q = (s, r = document) => [...r.querySelectorAll(s)];
  function on() {
    if (!W.gid || W.on || (W.locked && !isOwner())) return;
    W.on = true; root.classList.add('mg-drawing');
    toolbar();
    const hit = document.createElement('div'); hit.id = 'mega-wallhit'; main().append(hit);
    let from = null;
    hit.onpointerdown = ev => { if (ev.button > 0) return; try { hit.setPointerCapture(ev.pointerId); } catch { } from = stroke(ev, null); hit.onpointermove = e2 => { from = stroke(e2, from); }; };
    hit.onpointerup = hit.onpointercancel = () => { hit.onpointermove = null; from = null; };
    if (!present()) ask('draw');
    chrome(); schedule();
  }
  function off() {
    if (!W.on) return; W.on = false; root.classList.remove('mg-drawing');
    $q('#mega-wallbar')?.remove(); $q('#mega-wallhit')?.remove();
    clearTimeout(W.sendT); W.sendT = 0; if (W.out.size) queue();
    chrome(); schedule(); if (window.__mega && __mega.relook) __mega.relook();
  }

  // ------------------------------------------------------------------ a room opens and closes
  function open(item) {
    if (W.gid === item.gid && W.canvas && W.canvas.isConnected) return;
    close();
    const m = main(); if (!m) return;
    W.gid = item.gid; W.cells = new Map(); W.pend = new Map(); W.out = new Map(); W.rev = 0; W.gen = 0; W.locked = false; W.sizeKey = ''; W.tries = 0; W.txt = null;
    const c = document.createElement('canvas'); c.id = 'mega-wall'; c.setAttribute('aria-hidden', 'true');
    const bd = $q('#mega-bd', m); if (bd) bd.after(c); else m.prepend(c);
    W.canvas = c;
    W.ro = new ResizeObserver(() => { size(); remask(); }); W.ro.observe(m);
    size(); chrome();
    W.timer = setInterval(() => { refresh(); chrome(); }, 800);
    __megaLive.sub('wall', () => (W.gid ? [W.gid] : []), onLive);
    refresh().then(() => { if (W.gid === item.gid) ask('open'); });
    setTimeout(remask, 300);
  }
  function close() {
    if (W.on) off();
    clearInterval(W.timer); W.timer = 0; clearTimeout(W.sendT); W.sendT = 0; clearTimeout(W.answer); W.answer = 0;
    if (W.ro) { W.ro.disconnect(); W.ro = null; }
    if (W.canvas) { W.canvas.remove(); W.canvas = null; }
    $q('#btn-wall')?.remove();
    if (window.__megaLive) __megaLive.unsub('wall');
    W.gid = null; W.cells = new Map(); W.pend = new Map(); W.out = new Map();
    if (window.__mega && __mega.relook) __mega.relook();
  }
  window.__megaWall = {
    open, close, on, off, isOn: () => W.on, fillMask, remask, recolour: schedule, refresh,
    state: () => ({ gid: W.gid, on: W.on, gen: W.gen, locked: W.locked, cells: W.cells.size, pending: W.pend.size, out: W.out.size, present: present(), marks: marks().size, canvas: !!W.canvas, button: (() => { const b = $q('#btn-wall'); return !!b && !b.hidden; })(), bar: !!$q('#mega-wallbar'), owner: !!(typeof S !== 'undefined' && S.snap && isOwner()), geo: W.canvas ? geo() : null }),
    cells: () => [...marks()].map(([p, v]) => [p, v[0], v[1]]),
  };
})();
