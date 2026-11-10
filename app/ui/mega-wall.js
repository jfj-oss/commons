'use strict';
// mega: the shared ASCII wall. Anyone in a room can draw on the room's backdrop, live, for everyone. The same drawing shows on the room's
// card, and a community card has a wall of its own.
//
// The wall itself lives in the encrypted core (crates/core/src/wall.rs): it merges cell by cell, enforces the owner's lock and
// clear (kept in the owner-signed style document), validates everything that comes in, and keeps the wall on this device. This file
// is the picture and the pen: a canvas over the backdrop (or over a card), a small toolbar, and the pacing.
//
// Keys: a room's wall is kept under the room's id; the community card's under "cw:" + the community id. A community wall's live events
// (cwall.d / .s / .q / .h) travel in one text room of the community (the core picks it, wall_get names it) and are never mixed up with a
// room's own wall.d / wall.s, which older apps would draw on that room's wall.
//
// Pacing, because every event is kept forever on the relay and makes the sender's device rewrite its state (see mega-live.js):
//  - nothing is sent while the pen is idle; a stroke is batched and goes out at most twice a second, 200 cells at most per event;
//  - nothing is sent into a room where nobody is around to see it (cells stay on this device and reach others in a snapshot when
//    they open the room): somebody is around when they sent a wall event lately, or posted in the room lately;
//  - resting on or scrolling past a card sends nothing at all; only drawing does (and entering a room, as before). Only the focused
//    card is polled; the cards next to it are read once when they appear.
// Fixed grid: 120 x 54 cells in every room on every device. In a room it is centred and scaled to the window (so a phone draws the same
// picture, smaller); on a card it is stretched over the card when the card is about that shape, else centred like in a room.
(() => {
  const root = document.documentElement;
  const COLS = 120, ROWS = 54, CW = 10, CH = 17, GLYPHS = ' #*+.o=~/|-x', CWP = 'cw:';
  // Colours: 0 and 1 are the room's (or card's) own main and highlight ink; 2 to 11 are fixed, chosen to read on the dark backdrop. The numbers are what the core stores
  // and sends (crates/core/src/wall.rs WALL_COLOURS); older apps only know 0 and 1 and ignore a stroke that uses any other.
  const PAL = [['white', '#ffffff'], ['light grey', '#c4c8d4'], ['red', '#ff5a5f'], ['orange', '#ff9f43'], ['yellow', '#ffe14d'], ['green', '#5fe08a'], ['teal', '#2fd8c8'], ['blue', '#5aa8ff'], ['purple', '#b58cff'], ['pink', '#ff7ac8']];
  const inkOf = (ink, c) => (c < 2 ? ink[c] : (PAL[c - 2] || PAL[0])[1]);
  const GAP = 500, BATCH = 200, PRESENT_MS = 180000, MSG_MS = 600000, ASK_EVERY = 600000;
  // mode 'room': the open room's wall behind the chat. mode 'card': the focused card of the list (polled, and drawable).
  const W = { mode: 'room', gid: null, cid: null, canvas: null, sq: null, item: null, hit: null, pend: new Map(), out: new Map(), gen: 0, locked: false, on: false, brush: 1, col: 0,
    peerAt: 0, snapAt: 0, heard: 0, lastMask: 0, maskT: 0, timer: 0, sendT: 0, tries: 0, flushing: false, lastSend: 0, busy: false, txt: null, raf: 0, askedAt: 0, ro: null, sizeKey: '', carrier: null, localOnly: null };
  const CACHE = new Map();   // key -> { cells: Map(pos -> [glyph, colour]), rev, at }: what the core last said, for the focused card, its neighbours and the room
  const CARDS = new Map();   // list-card ascii object -> { key, canvas, txt }
  const RESP = new Map();    // key -> timer of a pending answer to somebody's request for the wall
  const stats = { cardPaints: 0, roomPaints: 0, fetches: 0, polls: 0 };
  const $q = (s, r = document) => r.querySelector(s);
  const $$q = (s, r = document) => [...r.querySelectorAll(s)];
  const main = () => document.getElementById('main');
  const comms = () => (typeof S !== 'undefined' && S.snap ? S.snap.communities : []);
  const isCw = k => typeof k === 'string' && k.startsWith(CWP);
  const cidOfGid = g => { const c = comms().find(c => c.channels.some(x => x.gid === g)); return c ? c.id : null; };
  const cidOf = k => (isCw(k) ? k.slice(CWP.length) : cidOfGid(k));
  const comm = () => comms().find(c => c.id === W.cid);
  const isOwner = () => (comm() || {}).role === 'owner';
  const prefix = k => (isCw(k) ? 'cwall.' : 'wall.');
  const store = { get(k) { try { return Number(localStorage.getItem('mega.' + k)) || 0; } catch { return 0; } }, set(k, v) { try { localStorage.setItem('mega.' + k, String(v)); } catch { } } };
  const entry = k => CACHE.get(k) || (CACHE.set(k, { cells: new Map(), rev: 0, at: 0 }), CACHE.get(k));
  const roomTheme = () => (window.__megaTheme && W.gid && window.__megaTheme.colours(W.gid)) || ['#9aa0b0', '#ffffff'];
  const theme = () => (W.mode === 'card' && W.item && window.__mega && __mega.wallInk ? __mega.wallInk(W.item) : roomTheme());

  // ------------------------------------------------------------------ geometry and painting
  /** Where the grid goes inside a box of w x h (layout pixels). A card about the grid's shape stretches it over the whole card; any other box centres it. */
  function gridIn(w, h, card) {
    const s = Math.min(w / (COLS * CW), h / (ROWS * CH)), ratio = (w / h) / ((COLS * CW) / (ROWS * CH));
    if (card && ratio > .6 && ratio < 1.8) { const cw = w / COLS, ch = h / ROWS; return { s, cw, ch, ox: 0, oy: 0, fs: Math.max(6, Math.min(ch * .86, cw * 1.5)) }; }
    return { s, cw: CW * s, ch: CH * s, ox: (w - COLS * CW * s) / 2, oy: (h - ROWS * CH * s) / 2, fs: Math.max(6, 14 * s) };
  }
  /** b: the box on screen (for the pointer); u: layout pixels per screen pixel (a card may be scaled by the list). */
  function geo() {
    if (W.mode === 'card' && W.sq && W.sq.isConnected) {
      const b = W.sq.getBoundingClientRect(), w = W.sq.offsetWidth, h = W.sq.offsetHeight;
      return { b, u: b.width ? w / b.width : 1, ...gridIn(w, h, true) };
    }
    const m = main(), b = m ? m.getBoundingClientRect() : { left: 0, top: 0, width: 1, height: 1 };
    return { b, u: 1, ...gridIn(b.width, b.height, false) };
  }
  function size() {
    const c = W.canvas, m = main(); if (!c || !m) return;
    const dpr = Math.min(devicePixelRatio || 1, 2), w = m.clientWidth, h = m.clientHeight, key = w + 'x' + h + 'x' + dpr;
    if (key === W.sizeKey) return; W.sizeKey = key;
    c.width = Math.round(w * dpr); c.height = Math.round(h * dpr); schedule();
  }
  function schedule() { if (!W.raf) W.raf = requestAnimationFrame(() => { W.raf = 0; paint(); }); }
  function marksOf(key) {   // what to show: the core's wall with my not-yet-confirmed cells on top
    const m = new Map(entry(key).cells);
    if (key === W.gid) for (const [p, v] of W.pend) { if (v[0] === 0) m.delete(p); else m.set(p, v); }
    return m;
  }
  const marks = () => marksOf(W.gid);
  function paint() {
    if (W.canvas) {   // the room's wall behind the chat
      stats.roomPaints++;
      const m = main(), G = geo(), dpr = W.canvas.width / (m.clientWidth || 1);
      drawMarksAt(W.canvas, G, W.gid, roomTheme(), W.on ? null : W.txt, dpr);
    }
    for (const [a, e] of CARDS) {   // the cards that are on screen
      if (!a.sq.isConnected || !e.canvas.isConnected) continue;
      stats.cardPaints++;
      const w = a.sq.offsetWidth, h = a.sq.offsetHeight, dpr = e.canvas.width / (w || 1), G = gridIn(w, h, true);
      drawMarksAt(e.canvas, G, e.key, __mega.wallInk(a.x), W.on && W.gid === e.key ? null : e.txt, dpr);
    }
  }
  function drawMarksAt(c, G, key, ink, hide, dpr) {
    const g = c.getContext('2d');
    g.clearRect(0, 0, c.width, c.height);
    g.font = `700 ${Math.round(G.fs * dpr)}px "Geist Mono", "IBM Plex Mono", ui-monospace, monospace`; g.textAlign = 'center'; g.textBaseline = 'middle';
    for (const [p, v] of marksOf(key)) {
      if (hide && hide[p]) continue;
      const x = p % COLS, y = (p / COLS) | 0;
      g.fillStyle = inkOf(ink, v[1]);
      g.fillText(GLYPHS[v[0]], (G.ox + (x + .5) * G.cw) * dpr, (G.oy + (y + .5) * G.ch) * dpr);
    }
  }
  /** Which wall cells have words on top, for painting around them. Called after the backdrop has looked at the page. */
  function remask() {
    if (!W.canvas || !window.__mega || !__mega.eachRect) return;
    const wait = W.lastMask + 32 - Date.now();   // about every other frame while scrolling, so drawings keep up with the words
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
    if (!W.gid || W.mode !== 'room') return;
    const G = geo(), kw = b.width / cols, kh = b.height / rows;
    for (const p of marks().keys()) {
      const x = p % COLS, y = (p / COLS) | 0;
      const l = G.b.left + G.ox + x * G.cw - b.left, t = G.b.top + G.oy + y * G.ch - b.top;
      const x0 = Math.max(0, Math.floor(l / kw)), x1 = Math.min(cols - 1, Math.floor((l + G.cw) / kw)), y0 = Math.max(0, Math.floor(t / kh)), y1 = Math.min(rows - 1, Math.floor((t + G.ch) / kh));
      for (let yy = y0; yy <= y1; yy++) for (let xx = x0; xx <= x1; xx++) mask[yy * cols + xx] = 1;
    }
  }

  // ------------------------------------------------------------------ the wall on a card
  const keyOf = x => (!x ? null : x.kind === 'community' ? CWP + x.cid : (x.kind === 'text' || x.kind === 'voice') && !x.locked ? x.gid : null);
  /** A list card (its ascii object `a`) is on screen: give it a canvas for the drawing and read the wall once if we have not lately. */
  function cardOn(a) {
    const key = keyOf(a.x); if (!key || !a.sq) return;
    cardOff(a);
    const c = document.createElement('canvas'); c.className = 'mwallc'; c.setAttribute('aria-hidden', 'true');
    const dpr = Math.min(devicePixelRatio || 1, 2); c.width = Math.round(a.sq.offsetWidth * dpr); c.height = Math.round(a.sq.offsetHeight * dpr);
    c.style.width = a.sq.offsetWidth + 'px'; c.style.height = a.sq.offsetHeight + 'px';
    if (a.c && a.c.isConnected) a.c.after(c); else a.sq.prepend(c);
    CARDS.set(a, { key, canvas: c, txt: null });
    if (W.on && W.gid === key) rebind(a.sq);
    const e = CACHE.get(key);
    if (key !== W.gid && (!e || Date.now() - e.at > 6000)) fetchCard(key);
    schedule();
  }
  function cardOff(a) { const e = CARDS.get(a); if (!e) return; e.canvas.remove(); CARDS.delete(a); }
  async function fetchCard(key) {
    const e = entry(key); if (e.fetching || document.visibilityState !== 'visible') return;
    e.fetching = true; e.at = Date.now(); stats.fetches++;
    try { const r = await rpc('wall_get', { gid: key }); if (!r.busy) applyView(key, r); } catch { } finally { e.fetching = false; }
  }
  /** The text mask for a card: which wall cells lie under words (hidden when painting), and the card's ascii must flow around the drawing. */
  function cardMask(a, hit) {
    const e = CARDS.get(a); if (!e) return;
    const w = a.sq.offsetWidth, h = a.sq.offsetHeight, G = gridIn(w, h, true);
    const t = e.txt && e.txt.length === COLS * ROWS ? e.txt : (e.txt = new Uint8Array(COLS * ROWS));
    for (let y = 0; y < ROWS; y++) for (let x = 0; x < COLS; x++) {
      const ax = Math.min(a.cols - 1, Math.floor((G.ox + (x + .5) * G.cw) / a.cw)), ay = Math.min(a.rows - 1, Math.floor((G.oy + (y + .5) * G.ch) / a.ch));
      t[y * COLS + x] = a.mask[ay * a.cols + ax];
    }
    for (const p of marksOf(e.key).keys()) {   // the drawing is an obstacle for the card's ascii
      const x = p % COLS, y = (p / COLS) | 0;
      const x0 = Math.max(0, Math.floor((G.ox + x * G.cw) / a.cw)), x1 = Math.min(a.cols - 1, Math.floor((G.ox + (x + 1) * G.cw) / a.cw));
      const y0 = Math.max(0, Math.floor((G.oy + y * G.ch) / a.ch)), y1 = Math.min(a.rows - 1, Math.floor((G.oy + (y + 1) * G.ch) / a.ch));
      for (let yy = y0; yy <= y1; yy++) for (let xx = x0; xx <= x1; xx++) a.mask[yy * a.cols + xx] = 1;
    }
    schedule();
  }
  /** Something on a card changed what is drawn: its ascii must flow around the new marks. */
  function touchCards(key) { if (window.__mega && __mega.rewall) __mega.rewall(key); schedule(); }

  // ------------------------------------------------------------------ the wall from the core
  function applyView(key, r) {
    const e = entry(key); e.at = Date.now();
    if (r.same && e.rev === r.rev) return false;
    e.rev = r.rev; e.cells = new Map(r.cells.filter(c => c[1] !== 0).map(c => [c[0], [c[1], c[2]]])); e.gen = r.gen; e.locked = r.locked;
    return true;
  }
  async function refresh() {
    const gid = W.gid; if (!gid || W.busy || document.visibilityState !== 'visible') return;
    W.busy = true; stats.polls++;
    try {
      const e = entry(gid);
      const r = await rpc('wall_get', { gid, rev: e.rev });
      if (W.gid !== gid || r.busy) return;
      W.gen = r.gen; W.peerAt = (r.peer_at || 0) * 1000; W.snapAt = (r.snap_at || 0) * 1000; W.carrier = r.carrier || null;
      shareBack();
      const lockChanged = W.locked !== r.locked; W.locked = r.locked;
      if (!r.same) {
        applyView(gid, r);
        for (const [p, v] of [...W.pend]) { const k = e.cells.get(p); if (!W.out.has(p) && ((k && k[0] === v[0] && k[1] === v[1]) || (!k && v[0] === 0))) W.pend.delete(p); }
        schedule(); touchCards(gid); if (W.mode === 'room' && window.__mega && __mega.relook) __mega.relook();
      }
      if (lockChanged || !r.same) chrome();
    } catch { } finally { W.busy = false; }
  }
  function present() {
    const now = Date.now();
    if (now - W.peerAt < PRESENT_MS || now - W.heard < PRESENT_MS) return true;
    if (isCw(W.gid)) return false;
    const ms = typeof S !== 'undefined' && S.msgs && S.gid === W.gid ? S.msgs.messages : [];
    for (let i = ms.length - 1; i >= 0 && i >= ms.length - 30; i--) { const m = ms[i]; if (m && !m.mine && m.kind === 'text' && now / 1000 - m.ts < MSG_MS / 1000) return true; }
    return false;
  }

  // ------------------------------------------------------------------ the pen
  function put(x, y) {
    if (x < 0 || y < 0 || x >= COLS || y >= ROWS) return;
    const p = y * COLS + x, v = [W.brush, W.brush ? W.col : 0];   // an erase has no colour
    const cur = W.pend.get(p) || entry(W.gid).cells.get(p);
    if (cur && cur[0] === v[0] && cur[1] === v[1]) return;
    if (!cur && v[0] === 0) return;
    W.pend.set(p, v); W.out.set(p, v);
  }
  function cellAt(ev) { const G = geo(); return [Math.floor(((ev.clientX - G.b.left) * G.u - G.ox) / G.cw), Math.floor(((ev.clientY - G.b.top) * G.u - G.oy) / G.ch)]; }
  function stroke(ev, from) {
    const [x, y] = cellAt(ev), [x0, y0] = from || [x, y], n = Math.max(Math.abs(x - x0), Math.abs(y - y0), 1);
    for (let i = 0; i <= n; i++) put(Math.round(x0 + (x - x0) * i / n), Math.round(y0 + (y - y0) * i / n));
    schedule(); touchCards(W.gid); queue(); return [x, y];
  }
  function queue() {
    if (W.sendT || W.flushing || !W.out.size) return;
    W.sendT = setTimeout(flush, Math.max(60, W.lastSend + GAP - Date.now()));
  }
  async function flush() {
    W.sendT = 0; const gid = W.gid; if (!gid || !W.out.size || W.flushing) return;
    W.flushing = true; W.lastSend = Date.now();
    // One kind of stroke per event: the room's two colours (every app version shows them) or a palette colour (newer apps only; older ones drop the whole event)
    const pal = p => W.out.get(p)[1] > 1, first = [...W.out.keys()][0];
    const keys = [...W.out.keys()].filter(p => pal(p) === pal(first)).slice(0, BATCH), cells = keys.map(p => [p % COLS, (p / COLS) | 0, W.out.get(p)[0], W.out.get(p)[1]]);
    const share = present();
    if (!share) { W.localOnly = gid; ask('draw'); }   // nobody seen yet: it stays here until someone turns up, and is then sent as a snapshot
    let r = null; try { r = await rpc('wall_draw', { gid, cells, share }); } catch (e) { r = { error: true }; }
    W.flushing = false;
    if (W.gid !== gid) return;
    if (r.error) { for (const k of keys) { W.out.delete(k); W.pend.delete(k); } }   // refused (locked, bad cell): nothing is kept
    else if (r.sent) { for (const k of keys) W.out.delete(k); W.tries = 0; if (share) __megaLive.count(prefix(gid) + 'd'); refresh(); }
    else if (++W.tries > 12) { for (const k of keys) W.out.delete(k); W.tries = 0; }   // the room could not take it for a long while: it stays on this device
    queue();
  }

  // ------------------------------------------------------------------ asking for, and answering with, a snapshot
  const sendGid = (key, carrier) => (isCw(key) ? carrier : key);
  const serving = key => !!W.gid && (W.gid === key || (W.mode === 'room' && key === CWP + W.cid));
  function ask(why) {
    const gid = W.gid; if (!gid) return;
    const key = 'wallask.' + gid, last = store.get(key), now = Date.now();
    if (now - last < (why === 'draw' ? 120000 : ASK_EVERY)) return;
    store.set(key, now); W.askedAt = now;
    let n = 0; const go = async () => { if (W.gid !== gid) return; const g = sendGid(gid, W.carrier); if (!(g && await __megaLive.send(g, prefix(gid) + 'q', '{}')) && ++n < 4) setTimeout(go, 3000); }; go();
  }
  function onLive(e) {
    const cw = e.kind.startsWith('cwall.'), rm = e.kind.startsWith('wall.'); if (!cw && !rm) return;
    const key = cw ? (cidOfGid(e.gid) ? CWP + cidOfGid(e.gid) : null) : e.gid;
    if (!key || !serving(key)) return;
    const k = e.kind.slice(e.kind.indexOf('.') + 1);
    if (key === W.gid && (k === 'q' || k === 'h')) W.heard = Date.now();
    if (k === 'q') respondLater(key, Date.now());
    else if (k === 'h') { clearTimeout(RESP.get(key)); RESP.delete(key); }
    if (key === W.gid) shareBack();
  }
  /** Somebody asked for the wall: answer after a short random wait unless someone else gets there first (a snapshot or an "empty" arrives meanwhile). */
  function respondLater(key, since) {
    if (RESP.has(key)) return;
    RESP.set(key, setTimeout(async () => {
      RESP.delete(key); if (!serving(key)) return;
      let r;
      for (let i = 0; i < 8; i++) {                                      // "busy" means the core is occupied for a moment, not that there is nothing to send
        try { r = await rpc('wall_get', { gid: key }); } catch { return; }
        if (!r.busy) break;
        await new Promise(z => setTimeout(z, 400));
      }
      if (r.busy || (r.snap_at || 0) * 1000 > since || !serving(key)) return;   // somebody else already answered
      const g = sendGid(key, r.carrier); if (!g) return;
      if (!(await sendSnapshot(key, g))) __megaLive.send(g, prefix(key) + 'h', '{}');
    }, 300 + Math.random() * 1200));
  }
  async function sendSnapshot(key, g) {
    let chunks = []; try { chunks = (await rpc('wall_snapshot', { gid: key })).chunks || []; } catch { }
    for (let i = 0; i < chunks.length && serving(key); i++) { if (i) await new Promise(r => setTimeout(r, 650)); await __megaLive.send(g, prefix(key) + 's', chunks[i]); }
    return chunks.length;
  }
  // What was drawn while nobody seemed to be around goes out once, as soon as someone is.
  function shareBack() {
    const gid = W.gid; if (!gid || W.localOnly !== gid || !present()) return;
    const g = sendGid(gid, W.carrier); if (!g) return;
    W.localOnly = null;
    setTimeout(() => { if (W.gid === gid) sendSnapshot(gid, g); }, 300 + Math.random() * 700);
  }
  const subGids = () => {
    if (!W.gid) return [];
    if (W.mode === 'card') return isCw(W.gid) ? ((comm() || {}).channels || []).map(x => x.gid) : [W.gid];
    return [W.gid, ...(((comm() || {}).channels) || []).map(x => x.gid)];   // a room also answers for its community's card wall
  };

  // ------------------------------------------------------------------ the header button and the toolbar
  function chrome() {
    if (!W.gid) return;
    const blocked = W.locked && !isOwner();
    if (W.mode === 'room') {
      const bar = $q('#head .head-actions'); if (!bar) return;
      let b = $q('#btn-wall');
      if (!b) {
        b = document.createElement('button'); b.id = 'btn-wall'; b.type = 'button'; b.className = 'btn ghost'; b.innerHTML = '<span class="lbl">Draw</span>';
        b.title = 'Draw on this room\'s backdrop, for everyone'; b.onclick = () => (W.on ? off() : on());
        bar.insertBefore(b, $q('#btn-call', bar) || $q('#btn-invite', bar) || bar.firstChild);
      }
      b.hidden = blocked; b.setAttribute('aria-pressed', W.on ? 'true' : 'false');
    }
    if (blocked && W.on) { off(); try { toast('The owner locked the wall'); } catch { } }
    const t = $q('#mega-wallbar');
    if (t) { const lk = $q('#mw-lock', t); if (lk) { lk.setAttribute('aria-pressed', W.locked ? 'true' : 'false'); lk.textContent = W.locked ? 'locked' : 'lock'; } }
    if (W.mode === 'card' && window.__mega && __mega.drawChip) __mega.drawChip();
  }
  const wallRoom = () => (isCw(W.gid) ? W.cid : W.gid);
  function toolbar() {
    let t = $q('#mega-wallbar'); if (t) t.remove();
    const host = W.mode === 'card' ? document.getElementById('mega') : main();
    t = document.createElement('div'); t.id = 'mega-wallbar'; t.setAttribute('role', 'toolbar'); t.setAttribute('aria-label', 'Drawing tools');
    const [c0, c1] = theme();
    t.innerHTML = `<span class="mwb-tag">everyone sees this</span><div class="mwb-g" id="mw-glyphs">${[...GLYPHS].map((g, i) => i ? `<button type="button" data-g="${i}" aria-pressed="${i === W.brush}" aria-label="glyph ${g}">${g === '<' ? '&lt;' : g}</button>` : '').join('')}<button type="button" data-g="0" aria-pressed="${W.brush === 0}">erase</button></div>
      <div class="mwb-c" id="mw-cols" role="group" aria-label="Colour">${[['room colour', c0], ['room highlight', c1], ...PAL].map(([n, hx], i) => `<button type="button" data-c="${i}" aria-pressed="${i === W.col}" aria-label="${n}" title="${n}" style="--sw:${hx}"></button>`).join('')}</div>
      ${isOwner() ? `<button type="button" class="mwb-b" id="mw-lock" aria-pressed="${W.locked}">${W.locked ? 'locked' : 'lock'}</button><button type="button" class="mwb-b" id="mw-clear">clear</button>` : ''}
      <button type="button" class="mwb-b" id="mw-done">done</button>`;
    host.append(t);
    const pickColour = (i, focus) => { W.col = i; $$q('#mw-cols button', t).forEach(x => { const on = +x.dataset.c === i; x.setAttribute('aria-pressed', String(on)); if (on && focus) x.focus(); }); };
    t.onkeydown = ev => {   // arrow keys walk the colours (and pick them), as in a radio group
      const b = ev.target.closest && ev.target.closest('#mw-cols button'); if (!b || !/^Arrow(Left|Right|Up|Down)$/.test(ev.key)) return;
      ev.preventDefault(); const n = 2 + PAL.length; pickColour((+b.dataset.c + (ev.key === 'ArrowLeft' || ev.key === 'ArrowUp' ? n - 1 : 1)) % n, true);
    };
    t.onclick = async ev => {
      const b = ev.target.closest('button'); if (!b) return;
      if (b.dataset.g != null) { W.brush = +b.dataset.g; $$q('#mw-glyphs button', t).forEach(x => x.setAttribute('aria-pressed', String(x === b))); }
      else if (b.dataset.c != null) pickColour(+b.dataset.c);
      else if (b.id === 'mw-done') off();
      else if (b.id === 'mw-lock') { try { await rpc('set_wall', { cid: W.cid, room: wallRoom(), lock: W.locked ? '0' : '1' }); W.locked = !W.locked; chrome(); } catch (e) { try { toast(e.message); } catch { } } }
      else if (b.id === 'mw-clear') {
        if (b.dataset.sure) {
          delete b.dataset.sure; b.textContent = 'clear';
          try { await rpc('set_wall', { cid: W.cid, room: wallRoom(), clear: true }); W.pend.clear(); W.out.clear(); const e = entry(W.gid); e.cells = new Map(); e.rev = 0; schedule(); touchCards(W.gid); refresh(); } catch (e) { try { toast(e.message); } catch { } }
        } else { b.dataset.sure = '1'; b.textContent = 'clear for everyone?'; setTimeout(() => { if (b.isConnected) { delete b.dataset.sure; b.textContent = 'clear'; } }, 4000); }
      }
    };
  }
  /** The layer that catches the pen. In a room it covers the room; on a card it floats over the focused card (outside the card, so a card refresh cannot remove it). */
  function placeHit() {
    const h = W.hit; if (!h) return;
    if (W.mode !== 'card') return;
    const r = W.sq && W.sq.isConnected ? W.sq.getBoundingClientRect() : null; if (!r) return;
    Object.assign(h.style, { left: r.left + 'px', top: r.top + 'px', width: r.width + 'px', height: r.height + 'px', borderRadius: getComputedStyle(W.sq).borderRadius });
  }
  function rebind(sq) { if (W.mode !== 'card' || !W.on) return; W.sq = sq; placeHit(); }
  function on() {
    if (!W.gid || W.on || (W.locked && !isOwner())) return;
    W.on = true; root.classList.add(W.mode === 'card' ? 'mg-drawing-card' : 'mg-drawing');
    toolbar();
    const hit = document.createElement('div'); hit.id = W.mode === 'card' ? 'mega-wallhit-card' : 'mega-wallhit'; W.hit = hit;
    (W.mode === 'card' ? document.getElementById('mega') : main()).append(hit); placeHit();
    let from = null;
    hit.onclick = ev => { ev.stopPropagation(); };
    hit.onwheel = ev => { ev.preventDefault(); };
    hit.onpointerdown = ev => { if (ev.button > 0) return; try { hit.setPointerCapture(ev.pointerId); } catch { } from = stroke(ev, null); hit.onpointermove = e2 => { from = stroke(e2, from); }; };
    hit.onpointerup = hit.onpointercancel = () => { hit.onpointermove = null; from = null; };
    if (!present()) ask('draw');
    chrome(); schedule(); touchCards(W.gid);
  }
  function off() {
    if (!W.on) return; W.on = false; root.classList.remove('mg-drawing', 'mg-drawing-card');
    $q('#mega-wallbar')?.remove(); W.hit?.remove(); W.hit = null;
    clearTimeout(W.sendT); W.sendT = 0; if (W.out.size) queue();
    chrome(); schedule(); touchCards(W.gid); if (window.__mega && __mega.relook) __mega.relook();
  }

  // ------------------------------------------------------------------ a room opens and closes / a card takes focus
  function reset(key, mode) {
    W.gid = key; W.mode = mode; W.cid = cidOf(key); W.pend = new Map(); W.out = new Map(); W.gen = 0; W.locked = !!(CACHE.get(key) || {}).locked; W.tries = 0; W.txt = null; W.carrier = null; W.snapAt = 0; W.peerAt = 0; W.heard = 0; W.localOnly = null;
  }
  function open(item) {
    if (W.mode === 'room' && W.gid === item.gid && W.canvas && W.canvas.isConnected) return;
    close();
    const m = main(); if (!m) return;
    reset(item.gid, 'room'); W.sizeKey = '';
    const c = document.createElement('canvas'); c.id = 'mega-wall'; c.setAttribute('aria-hidden', 'true');
    const bd = $q('#mega-bd', m); if (bd) bd.after(c); else m.prepend(c);
    W.canvas = c;
    W.ro = new ResizeObserver(() => { size(); remask(); }); W.ro.observe(m);
    size(); chrome();
    W.timer = setInterval(() => { refresh(); chrome(); }, 800);
    __megaLive.sub('wall', subGids, onLive);
    refresh().then(() => { if (W.gid === item.gid) ask('open'); });
    setTimeout(remask, 300);
  }
  function close() {
    if (W.mode !== 'room') return;   // a focused card is not a room that closed
    if (W.on) off();
    stop();
    if (W.canvas) { W.canvas.remove(); W.canvas = null; }
    $q('#btn-wall')?.remove();
    if (window.__mega && __mega.relook) __mega.relook();
  }
  function stop() {
    clearInterval(W.timer); W.timer = 0; clearTimeout(W.sendT); W.sendT = 0;
    for (const t of RESP.values()) clearTimeout(t); RESP.clear();
    if (W.ro) { W.ro.disconnect(); W.ro = null; }
    if (window.__megaLive) __megaLive.unsub('wall');
    W.gid = null; W.sq = null; W.item = null; W.pend = new Map(); W.out = new Map();
  }
  /** A card of the list is the focused one: it is the only card that is polled, and the one a pen can go on. */
  function focus(item, sq) {
    const key = keyOf(item);
    if (W.mode === 'card' && W.gid === key && key) { W.item = item; W.sq = sq; return; }
    if (W.mode === 'room' && W.gid && W.canvas) return;   // a room is open: it owns the wall
    unfocus();
    if (!key) return;
    reset(key, 'card'); W.item = item; W.sq = sq;
    W.timer = setInterval(() => { refresh(); chrome(); }, 1000);
    __megaLive.sub('wall', subGids, onLive);
    refresh();
  }
  function unfocus() { if (W.mode !== 'card' || !W.gid) return; if (W.on) off(); stop(); W.mode = 'room'; if (window.__mega && __mega.drawChip) __mega.drawChip(); }
  /** The list was torn down or the person moved on. */
  function leave() { unfocus(); }
  /** Whether the person may draw on this focused card right now (the lock is known from the owner's document). */
  function drawable(item) {
    const key = keyOf(item); if (!key) return false;
    const c = comms().find(c => c.id === item.cid), st = c && c.style, id = isCw(key) ? c && c.id : key;
    const locked = !!(st && st.locked && st.locked.includes(id));
    return !locked || (c && c.role === 'owner');
  }
  function drawCard() { if (W.mode !== 'card' || !W.gid) return false; if (W.on) { off(); return true; } on(); return W.on; }
  addEventListener('resize', () => { placeHit(); schedule(); });

  window.__megaWall = {
    open, close, on, off, isOn: () => W.on, fillMask, remask, recolour: schedule, refresh,
    cardOn, cardOff, cardMask, focus, leave, drawable, drawCard, keyOf, rebind,
    state: () => ({ gid: W.gid, mode: W.mode, on: W.on, gen: W.gen, locked: W.locked, cells: entry(W.gid || '').cells.size, pending: W.pend.size, out: W.out.size, present: present(), marks: W.gid ? marks().size : 0, canvas: !!W.canvas, button: (() => { const b = $q('#btn-wall'); return !!b && !b.hidden; })(), bar: !!$q('#mega-wallbar'), owner: !!(typeof S !== 'undefined' && S.snap && isOwner()), geo: W.gid ? geo() : null, carrier: W.carrier,
      cards: [...CARDS.values()].map(e => ({ key: e.key, marks: marksOf(e.key).size })), ...stats }),
    cells: () => [...marks()].map(([p, v]) => [p, v[0], v[1]]),
    cellsOf: key => [...marksOf(key)].map(([p, v]) => [p, v[0], v[1]]),
  };
  if (window.__mega && __mega.attachWalls) __mega.attachWalls();
})();
