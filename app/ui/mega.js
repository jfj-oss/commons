'use strict';
// mega: the navigation. Home (communities) -> a community's rooms -> the chat with dots beside it.
// Real data comes from S.snap (see app.js); the chat itself, the composer, the modals, RPC, encryption and the call
// engine are the existing ones. This file only decides what is on screen around them.
//
// Rules this file keeps:
//  - Nothing joins a call except an explicit click on a join button, a click on a voice dot, or Enter on a voice card.
//    Scrolling or resting on a card only enlarges it.
//  - Nothing is marked read while the cards are showing (app.js asks __megaIdle).
(() => {
  const root = document.documentElement;
  root.classList.add('mega');

  // ---------------------------------------------------------------- small helpers
  const H = s => [...String(s)].reduce((a, c) => (a * 31 + c.charCodeAt(0)) >>> 0, 7);
  const wait = ms => new Promise(r => setTimeout(r, ms));
  const rgb = h => [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16));
  const hex = a => '#' + a.map(v => Math.round(Math.max(0, Math.min(255, v))).toString(16).padStart(2, '0')).join('');
  const mix = (a, b, t) => { const x = rgb(a), y = rgb(b); return hex(x.map((v, i) => v + (y[i] - v) * t)); };
  const lum = h => { const [r, g, b] = rgb(h).map(v => { v /= 255; return v <= .03928 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4; }); return .2126 * r + .7152 * g + .0722 * b; };
  const contrast = (a, b) => { const x = lum(a), y = lum(b); return (Math.max(x, y) + .05) / (Math.min(x, y) + .05); };
  const bestInk = f => contrast(f, '#0c0d12') >= contrast(f, '#ffffff') ? '#0c0d12' : '#ffffff';
  const brighten = c => { let i = 0; while (contrast(c, '#050407') < 3.4 && i++ < 12) c = mix(c, '#ffffff', .14); return c; };
  const store = { get(k, d) { try { const v = localStorage.getItem('mega.' + k); return v == null ? d : JSON.parse(v); } catch { return d; } }, set(k, v) { try { localStorage.setItem('mega.' + k, JSON.stringify(v)); } catch { } } };

  // Colours per room: this phase uses defaults derived from the room's id. (Owner-chosen colours need syncing in the core.)
  const DEF = [['#ff5a1f', '#ffe14d'], ['#101640', '#45f0d0'], ['#f3c6d8', '#c2124a'], ['#0b3d2e', '#d8ff3e'], ['#e8e4d8', '#1b33ff'], ['#1a1a1a', '#ff2e63'], ['#6a35ff', '#ffb3f0'], ['#00a6a6', '#fff275'], ['#ffd23f', '#5a189a'], ['#2d6a4f', '#ffb703'], ['#e63946', '#f1faee'], ['#264653', '#e9c46a']];
  const fieldOf = x => x.kind === 'options' ? '#a9adb8' : x.theme[0];
  const hiOf = x => x.kind === 'options' ? '#4a4e5c' : x.theme[1];
  const dotOf = x => { if (x.kind === 'options') return '#b5bac8'; const t = x.theme; return contrast(t[0], '#050407') >= 3.4 ? t[0] : brighten(t[1]); };
  const markOf = x => (window.__megaNew ? __megaNew.mark(x) : dotOf(x));   // the ring that says unread / mention, in the room's own highlight colour
  const cardVars = (x, snap) => { const f = fieldOf(x), ink = bestInk(f); return `--m-mesh:${snap ? snapOf(x) : f};--m-cink:${ink};--m-solid:${ink};--m-on:${f}`; };
  const faceColor = n => `hsl(${H(n) % 360} 78% 74%)`;
  const mface = (name, cls = '') => `<span class="mface ${cls}" style="--fc:${faceColor(name)}" aria-hidden="true">${esc(([...String(name || '?')][0] || '?').toUpperCase())}</span>`;
  const mfaces = names => `<span class="mfaces">${names.slice(0, 5).map(n => mface(n)).join('')}</span>`;

  // ---------------------------------------------------------------- ascii over the colour (only the room's two colours)
  const RAMP = ' .:-=+*#%@';
  const nh = (x, y) => { const s = Math.sin(x * 127.1 + y * 311.7) * 43758.5453; return s - Math.floor(s); };
  const vn = (x, y) => { const i = Math.floor(x), j = Math.floor(y), f = x - i, g = y - j, u = f * f * (3 - 2 * f), w = g * g * (3 - 2 * g); return (nh(i, j) * (1 - u) + nh(i + 1, j) * u) * (1 - w) + (nh(i, j + 1) * (1 - u) + nh(i + 1, j + 1) * u) * w; };
  const FX = [
    (x, y, t) => .5 + .5 * Math.sin(x * .11 + t + Math.sin(y * .09 - t * .6) * 2.2) * Math.cos(y * .16 - t * .4),
    (x, y, t, a) => { const cx = a.cols * (.5 + .3 * Math.sin(t * .31)), cy = a.rows * (.5 + .35 * Math.cos(t * .23)), d = Math.hypot(x - cx, (y - cy) * 1.9); return .5 + .5 * Math.sin(d * .34 - t * 1.6); },
    (x, y, t, a) => { const c = a.col[x % a.col.length], p = ((y + t * c.v * 7 + c.o) % c.l) / c.l; return p > .4 ? ((p - .4) / .6) ** 1.6 : 0; },
    (x, y, t) => Math.min(1, Math.max(0, (vn(x * .05 + t * .1, y * .11 - t * .06) * .65 + vn(x * .14 - t * .15, y * .28) * .35 - .22) / .56)),
    (x, y, t, a) => { const dx = x - a.cols / 2, dy = (y - a.rows / 2) * 1.9; return .5 + .5 * Math.sin(Math.atan2(dy, dx) * 5 + Math.log(1 + Math.hypot(dx, dy)) * 4 - t * 1.4); },
    (x, y, t) => .5 + .5 * Math.sin((x + y * 1.9) * .18 + t) * Math.sin((x - y * 1.9) * .14 - t * .7),
    (x, y, t) => .34 * (.5 + .5 * Math.sin(x * .07 + y * .19 + t * .3)) + .1,
    // ---- appended patterns (never reorder: a number in an owner's look and the automatic choice of every card point into this list).
    // Each is per cell per frame: arithmetic only, no allocation. They return 0..1, and at a still frame (reduced motion) they are a picture.
    // 7 aurora: tall curtains that sway slowly
    (x, y, t) => { const c = .5 + .5 * Math.sin(x * .13 + Math.sin(t * .4 + x * .045) * 2.5 + t * .3); return c * c * (.55 + .45 * Math.sin(y * .08 - t * .5 + x * .02)); },
    // 8 tide: broad horizontal bands rolling up and down
    (x, y, t) => .5 + .5 * Math.sin(y * .5 - t * 1.2 + Math.sin(x * .05 + t * .5) * 1.4),
    // 9 tunnel: rectangles moving inward
    (x, y, t, a) => { const dx = Math.abs(x - a.cols / 2) / (a.cols / 2), dy = Math.abs(y - a.rows / 2) / (a.rows / 2), d = dx > dy ? dx : dy; return .5 + .5 * Math.sin(d * 15 + t * 2); },
    // 10 orbit: soft blobs circling and merging
    (x, y, t, a) => {
      let s = 0;
      for (let i = 0; i < 3; i++) { const dx = x - a.cols * (.5 + .27 * Math.cos(t * .5 + i * 2.1)), dy = (y - a.rows * (.5 + .3 * Math.sin(t * .7 + i * 2.1))) * 1.9; s += 70 / (dx * dx + dy * dy + 1); }
      const v = (s - .3) * 1.3; return v < 0 ? 0 : v > 1 ? 1 : v;
    },
    // 11 snow: sparse dots drifting down, wandering sideways
    (x, y, t, a) => { const c = a.col[x % a.col.length], xx = x + Math.round(Math.sin(t * .5 + y * .3 + c.o) * 1.5), n = nh(xx * 1.7, Math.floor(y - t * (1.2 + c.v)) + c.o); return n > .93 ? .55 + (n - .93) * 6 : 0; },
    // 12 embers: noise rising from the bottom, fading toward the top
    (x, y, t, a) => { const f = y / a.rows, v = (vn(x * .11, y * .27 - t * 1.3) * 1.25 - .22) * f * f * 1.7; return v < 0 ? 0 : v > 1 ? 1 : v; },
    // 13 radar: a rotating sweep with a fading trail
    (x, y, t, a) => { const d = ((t * 1.1 - Math.atan2((y - a.rows / 2) * 1.9, x - a.cols / 2)) % 6.2832 + 6.2832) % 6.2832; return d < 3 ? (1 - d / 3) * (1 - d / 3) : 0; },
    // 14 weave: two crossing diagonal lattices sliding past each other
    (x, y, t) => { const p = .5 + .5 * Math.sin((x + y * 1.9) * .35 + t * .6), q = .5 + .5 * Math.sin((x - y * 1.9) * .35 - t * .5), v = (p > .8 ? p : 0) + (q > .8 ? q : 0); return v > 1 ? 1 : v; },
    // 15 comets: fast diagonal streaks with tails
    (x, y, t) => {
      const w = x + y * 1.9, lane = Math.floor(w / 5), h = nh(lane, 3.7);
      if (h < .35 || w - lane * 5 > 2.2) return 0;
      const l = x * 1.9 - y, d = (((t * (10 + h * 14) + h * 120 - l) % 130) + 130) % 130; return d < 24 ? (1 - d / 24) * (1 - d / 24) : 0;
    },
    // 16 dunes: slow ridged noise
    (x, y, t) => { const r = 1 - Math.abs(vn(x * .045 + t * .03, y * .16) * 2 - 1), v = (r * r * r - .3) / .7; return v < 0 ? 0 : v > 1 ? 1 : v; },
  ];
  // the names shown in the chooser (same order as FX)
  const FXNAMES = ['waves', 'ripples', 'rain', 'clouds', 'spiral', 'lattice', 'haze', 'aurora', 'tide', 'tunnel', 'orbit', 'snow', 'embers', 'radar', 'weave', 'comets', 'dunes'];
  const colFor = (n, seed) => Array.from({ length: Math.max(1, n) }, (_, i) => ({ v: .6 + nh(i, seed) * 1.5, o: nh(seed, i) * 60, l: 16 + nh(i + 9, seed) * 30 }));
  const MONO = '"IBM Plex Mono", ui-monospace, monospace';
  let fontKey = 0;
  const SNAP = new Map();
  function snapOf(x) {
    const f = fieldOf(x), hi = hiOf(x), key = x.id + f + hi + fontKey + ':' + x.fx; if (SNAP.has(key)) return SNAP.get(key);
    const c = document.createElement('canvas'); c.width = 240; c.height = 96; const g = c.getContext('2d'); g.font = '500 11px ' + MONO; g.textAlign = 'center'; g.textBaseline = 'middle';
    const a = { cols: 34, rows: 8, col: colFor(34, H(x.id)) };
    for (let y = 0; y < 8; y++) for (let k = 0; k < 34; k++) { if (y > 3 && k < 27) continue; const v = FX[x.fx](k, y, 7.3 + H(x.id) % 40, a); if (v < .2) continue; g.fillStyle = mix(f, hi, v < .64 ? .3 : v < .85 ? .6 : 1); g.fillText(RAMP[Math.min(9, v * 10 | 0)], k * 7 + 3.5, y * 12 + 6); }
    const u = `url(${c.toDataURL('image/png')}) center/cover, ${f}`; SNAP.set(key, u); return u;
  }
  const THUMB = new Map();
  function fxThumb(n, f, hi) {
    const key = n + f + hi + fontKey; if (THUMB.has(key)) return THUMB.get(key);
    if (THUMB.size > 160) THUMB.clear();
    const c = document.createElement('canvas'); c.width = 176; c.height = 88; const g = c.getContext('2d');
    g.fillStyle = f; g.fillRect(0, 0, 176, 88); g.font = '500 10px ' + MONO; g.textAlign = 'center'; g.textBaseline = 'middle';
    const cols = 22, rows = 8, a = { cols, rows, col: colFor(cols, 5) };
    for (let y = 0; y < rows; y++) for (let k = 0; k < cols; k++) { const v = FX[n](k, y, 7.3, a); if (v < .2) continue; g.fillStyle = mix(f, hi, v < .64 ? .45 : v < .85 ? .75 : 1); g.fillText(RAMP[Math.min(9, v * 10 | 0)], k * 8 + 4, y * 11 + 6); }
    const u = `url(${c.toDataURL('image/png')})`; THUMB.set(key, u); return u;
  }
  const ATLAS = new Map();
  function atlasOf(f, hi, sw, sh, fs) {
    const key = [f, hi, sw, sh, fs, fontKey].join('|'); if (ATLAS.has(key)) return ATLAS.get(key);
    if (ATLAS.size > 40) ATLAS.clear();
    const c = document.createElement('canvas'); c.width = sw * 10; c.height = sh * 3; const g = c.getContext('2d');
    g.font = `500 ${fs}px ${MONO}`; g.textAlign = 'center'; g.textBaseline = 'middle';
    [.3, .6, 1].forEach((t, row) => { g.fillStyle = mix(f, hi, t); [...RAMP].forEach((ch, i) => g.fillText(ch, i * sw + sw / 2, row * sh + sh / 2)); });
    ATLAS.set(key, c); return c;
  }
  const fade = c => { if (calm() || root.classList.contains('fx-lite')) return; c.classList.remove('mfade'); void c.offsetWidth; c.classList.add('mfade'); };
  const ACTIVE = new Set(); let clock = 0, energy = 0, lastT = 0, tickN = 0;
  function asciiOn(sq, x) {
    if (!sq.offsetWidth) return null;
    const c = document.createElement('canvas'); c.className = 'masc'; c.setAttribute('aria-hidden', 'true'); sq.prepend(c);
    const dpr = Math.min(devicePixelRatio || 1, 2), small = sq.offsetWidth < 560, cw = small ? 8 : 10, ch = small ? 13 : 17, fs = Math.round((small ? 11 : 14) * dpr), sw = Math.round(cw * dpr), sh = Math.round(ch * dpr);
    const cols = Math.ceil(sq.offsetWidth / cw), rows = Math.ceil(sq.offsetHeight / ch); c.width = cols * sw; c.height = rows * sh; c.style.width = cols * cw + 'px'; c.style.height = rows * ch + 'px';
    const live = x.kind === 'voice' && x.people.length, frozen = x.locked || (x.kind === 'voice' && !x.people.length);
    const a = { c, ctx: c.getContext('2d'), sq, x, cols, rows, cw, ch, sw, sh, fs, mask: new Uint8Array(cols * rows), field: FX[x.fx], hiT: frozen ? 2 : live ? .76 : .9 - .22 * Math.min(1, (x.act || 0) / 12), frozen, col: colFor(cols, H(x.id)), focus: false, mx: -999, my: -999, bloom: 0 };
    a.at = atlasOf(fieldOf(x), hiOf(x), sw, sh, fs); ACTIVE.add(a); if (window.__megaWall) __megaWall.cardOn(a); remask(a); asciiDraw(a, clock); return a;
  }
  function asciiOff(a) { if (!a) return; ACTIVE.delete(a); if (window.__megaWall) __megaWall.cardOff(a); a.c.remove(); }
  // the ascii flows around the words: every text box on the card is cut out of the pattern
  function remask(a) {
    const sr = a.sq.getBoundingClientRect(); if (!sr.width) return;
    const k = a.sq.offsetWidth / sr.width, settled = a.sq.closest('.mslot')?.classList.contains('settled'); a.mask.fill(0);
    const hit = r => { const x0 = Math.max(0, Math.floor((r.left - sr.left) * k / a.cw) - 1), x1 = Math.min(a.cols, Math.ceil((r.right - sr.left) * k / a.cw) + 1), y0 = Math.max(0, Math.floor((r.top - sr.top) * k / a.ch)), y1 = Math.min(a.rows, Math.ceil((r.bottom - sr.top) * k / a.ch)); for (let y = y0; y < y1; y++) a.mask.fill(1, y * a.cols + x0, y * a.cols + Math.max(x0, x1)); };
    const rg = document.createRange();
    $$('.mtop>span:not(.mat),.mti,.mnu' + (settled ? ',.mpreview' : ''), a.sq).forEach(e => { rg.selectNodeContents(e); [...rg.getClientRects()].forEach(hit); });
    $$('.mat' + (settled ? ',.mwho>span,.mvact button,.mfaces-row' : ''), a.sq).forEach(e => hit(e.getBoundingClientRect()));
    if (window.__megaWall) __megaWall.cardMask(a);   // the words are cut out; now the drawing on the card is an obstacle too
  }
  const remaskAll = () => ACTIVE.forEach(remask);
  function asciiDraw(a, t) {
    const { ctx, cols, rows, mask, sw, sh, at, field, hiT, bloom, mx, my } = a; if (a.frozen) t = 11.7;
    ctx.clearRect(0, 0, a.c.width, a.c.height);
    for (let y = 0, i = 0; y < rows; y++) for (let x = 0; x < cols; x++, i++) {
      if (mask[i]) continue;
      let v = field(x, y, t, a);
      if (bloom > .02) { const dx = x - mx, dy = (y - my) * 1.8; v += bloom * Math.exp(-(dx * dx + dy * dy) / 90) * .85; }
      if (v < .2) continue;
      ctx.drawImage(at, Math.min(9, v * 10 | 0) * sw, (v < .64 ? 0 : v < hiT ? 1 : 2) * sh, sw, sh, x * sw, y * sh, sw, sh);
    }
  }
  // Only the cards near the focused one animate; reduced motion keeps the still frame it was drawn with.
  function asciiTick(now) {
    requestAnimationFrame(asciiTick);
    if (!ACTIVE.size || document.hidden || now - lastT < 40 || calm()) return;
    const dt = Math.min(.1, (now - lastT) / 1000); lastT = now; tickN++;
    clock += dt * (1 + energy * 3.5); energy *= .93;
    ACTIVE.forEach(a => { a.bloom *= .95; if (a.focus || tickN % 3 === 0) asciiDraw(a, clock); });
  }

  // ---------------------------------------------------------------- kerned titles (the prototype's "tight" setting)
  const KERN = new Map();
  for (const [v, ps] of [[-.05, 'ra rd re ro ta te to ty va vo wa ye yo fa fo ff'], [-.03, 'av aw ay ev ew ey ov ow oy rc rs rg tc ts ld lk gh ll tt'], [-.018, 'ar er or ur an en on un']]) ps.split(' ').forEach(p => KERN.set(p, v));
  const letters = t => {
    const a = [...t];
    return a.map((ch, i) => {
      if (ch === ' ') return '<i class="sp">&nbsp;</i>';
      const k = i ? KERN.get((a[i - 1] + ch).toLowerCase()) || 0 : 0;
      return `<i aria-hidden="true" style="--k:${k.toFixed(3)}">${esc(ch)}</i>`;
    }).join('');
  };
  const ti = (t, cls = 'mti', tag = 'h2') => `<${tag} class="${cls}" aria-label="${esc(t)}">${letters(String(t))}</${tag}>`;

  // ---------------------------------------------------------------- state and data (everything comes from S.snap)
  const M = { started: false, level: 'home', cid: null, list: null, rail: false, opts: false, adm: false, busy: false, dirty: false, opening: false, sig: '', barSig: '', pal: null, closers: [] };
  const mega = document.createElement('div'); mega.id = 'mega'; mega.hidden = true; mega.setAttribute('aria-label', 'mega');
  const fxl = document.createElement('div'); fxl.id = 'mega-fx'; fxl.setAttribute('aria-hidden', 'true');
  const rail = document.createElement('nav'); rail.id = 'mega-rail'; rail.setAttribute('aria-label', 'Rooms'); rail.hidden = true;
  const optsEl = document.createElement('div'); optsEl.id = 'mega-opts'; optsEl.hidden = true; optsEl.setAttribute('aria-label', 'options');
  const tip = document.createElement('div'); tip.id = 'mega-tip'; tip.hidden = true;
  document.body.append(mega, rail, optsEl, fxl, tip);

  const comms = () => S.snap ? S.snap.communities.filter(c => !c.removed) : [];
  const comm = id => comms().find(c => c.id === id);
  const mentionsIn = c => c.channels.reduce((n, x) => n + (S.mention.get(x.gid) || 0), 0);
  const unreadIn = c => c.channels.reduce((n, x) => n + (x.unread || 0), 0);
  const PREV = new Map();   // gid -> { sig, who, text }: the newest line, for the card
  // ---------------------------------------------------------------- owner colours
  // The core carries what the owner chose (S.snap.communities[].style, already checked against the owner's signature).
  // SHOWN is what is on screen this moment, so a change from someone else can fade in instead of jumping.
  const SHOWN = new Map(), GOAL = new Map(), HOLD = new Map(), SEEN = new Map(), TW = new Map();
  const isHex = v => /^#[0-9a-f]{6}$/i.test(v || '');
  // the colours the owner set for this community card (gid null) or room, or the defaults derived from the id
  const targetOf = (cid, gid) => { const c = comm(cid), st = c && c.style, d = st && (gid ? st.rooms && st.rooms[gid] : st.community); return d && isHex(d.main) && isHex(d.hi) ? [d.main.toLowerCase(), d.hi.toLowerCase()] : DEF[H(gid || cid) % DEF.length]; };
  const themeOf = o => SHOWN.get(o.id) || (o.kind === 'options' ? DEF[H(o.id) % DEF.length] : targetOf(o.cid || o.id, o.kind === 'community' ? null : o.gid));
  // The owner's look (card pattern, chat pattern, backdrop strength), from the same signed document. A pattern number this version does not know
  // means automatic, so patterns can be added later. LHOLD keeps what the owner is editing right now on this screen until the snapshot catches up.
  const DIM_MAX = 160;
  const LHOLD = new Map(), DIMS = new Map(), LSEEN = new Map();
  const lookStored = (cid, id) => { const c = comm(cid), st = c && c.style; return (st && st.looks && st.looks[id]) || null; };
  const lookOf = (cid, id) => { const h = LHOLD.get(id); if (h && h.until > Date.now()) return h.look; return lookStored(cid, id) || {}; };
  const patOf = n => (Number.isInteger(n) && n >= 0 && n < FX.length ? n : null);
  const dimOf = lk => (Number.isFinite(lk.dim) ? Math.max(0, Math.min(DIM_MAX, Math.round(lk.dim))) : 100);
  const mk = (o, sigParts) => { o.theme = o.theme || themeOf(o); o.sig = JSON.stringify([sigParts, o.theme]); return o; };

  // who is in a call, for any community's card: the open one comes from app.js, the others from the home poll below
  const CP = new Map();   // gid -> people, from the relay's presence for every community at once
  const peopleIn = gid => callPeople(gid) || CP.get(gid) || [];
  function commItem(c) {
    const un = unreadIn(c), at = mentionsIn(c), voice = c.channels.filter(x => x.kind === 'voice' && peopleIn(x.gid).length);
    const first = voice[0], people = first ? peopleIn(first.gid) : [];
    // the newest line over all of the community's readable text rooms
    let pv = null; for (const x of c.channels) { const p = x.kind !== 'voice' && x.joined && PREV.get(x.gid); if (p && p.text && (!pv || (p.ts || 0) > (pv.ts || 0))) pv = p; }
    const auto = [4, 3, 0, 5, 2][H(c.name) % 5], lk = lookOf(c.id, c.id), nm = c.members | 0;
    return mk({ key: 'c:' + c.id, id: c.id, cid: c.id, kind: 'community', name: c.name, unread: un, at, act: un, fxAuto: auto, fx: patOf(lk.card) ?? auto, rooms: c.channels.length, members: nm, pv, voiceName: first && first.name, people: people.map(p => p.name) },
      [c.name, un, at, c.channels.length, nm, pv && pv.who, pv && pv.text, first && first.name, people.map(p => p.name)]);
  }
  const youItem = () => mk({ key: 'you', id: 'you', kind: 'you', name: 'you', unread: 0, at: 0, act: 0, fx: 4, theme: ['#d9dce4', '#2b3bff'] }, ['you']);
  function roomItems(c) {
    const off = H(c.id), out = [];
    const room = ch => {
      const voice = ch.kind === 'voice', i = c.channels.indexOf(ch), people = voice ? peopleIn(ch.gid) : [], pv = PREV.get(ch.gid) || {};
      const auto = voice ? 1 : [0, 3, 4, 5, 2][(off + i) % 5], lk = lookOf(c.id, ch.gid);
      const it = { key: ch.gid, id: ch.gid, gid: ch.gid, cid: c.id, kind: voice ? 'voice' : 'text', name: ch.name, unread: ch.unread || 0, at: S.mention.get(ch.gid) || 0, locked: !ch.joined, status: ch.joined && ch.status && ch.status !== 'ok' ? ch.status : '', people, here: voice && inCallHere(ch.gid), fxAuto: auto, fx: patOf(lk.card) ?? auto, chatFx: patOf(lk.chat) ?? auto, dim: DIMS.has(ch.gid) ? DIMS.get(ch.gid) : dimOf(lk), pv };
      it.act = it.unread;
      return mk(it, [it.name, it.unread, it.at, it.locked, it.status, people.map(p => [p.name, !!p.speaking, !!p.muted, !!p.deafened, !!p.me]), it.here, pv.who, pv.text]);
    };
    c.channels.filter(x => x.kind !== 'voice').forEach(x => out.push(room(x)));
    c.channels.filter(x => x.kind === 'voice').forEach(x => out.push(room(x)));
    if (window.__megaAdmin && __megaAdmin.may(c)) out.push(mk({ key: 'adm:' + c.id, id: 'adm:' + c.id, cid: c.id, kind: 'admin', name: 'server settings', unread: 0, at: 0, act: 0, fx: 2 }, ['admin', c.role]));
    out.push(mk({ key: 'opt:' + c.id, id: 'opt:' + c.id, cid: c.id, kind: 'options', name: 'options', unread: 0, at: 0, act: 0, fx: 6 }, ['options']));
    return out;
  }

  // newest line of each readable room, for the cards; asks the core only when a room's count changed (or its line is older than ttl)
  async function prevOf(ch, ttl) {
    const sig = ch.unread + ':' + ch.status, p = PREV.get(ch.gid);
    if (p && p.sig === sig && Date.now() - p.t < ttl) return false;   // the snapshot has no "last activity", so look again every so often
    let n;
    try {
      const m = await rpc('messages', { gid: ch.gid, limit: 8 });
      const last = [...m.messages].reverse().find(x => !x.deleted && x.kind !== 'notice' && x.kind !== 'member');
      n = { sig, t: Date.now(), ts: last ? last.ts || 0 : 0, who: last ? last.name : '', text: last ? plain(last.text || (last.file ? last.file.name : '')) : '' };
    } catch { n = { sig, t: Date.now(), ts: 0, who: '', text: '' }; }
    PREV.set(ch.gid, n);
    return !p || p.who !== n.who || p.text !== n.text;
  }
  async function loadPreviews(c) {
    if (loadPreviews.busy) return; loadPreviews.busy = true;
    try {
      for (const ch of c.channels) {
        if (ch.kind === 'voice' || !ch.joined) continue;
        if (await prevOf(ch, 4000) && M.level === 'server' && M.cid === c.id) render();
      }
    } finally { loadPreviews.busy = false; }
  }
  // home: the newest line of every community, a few rooms per pass (rooms with unread first), each looked at again only every 30 s
  async function homePreviews() {
    if (homePreviews.busy || M.level !== 'home' || document.hidden) return; homePreviews.busy = true;
    try {
      let left = 10, changed = false;
      const rooms = comms().flatMap(c => c.channels.filter(x => x.kind !== 'voice' && x.joined)).sort((a, b) => (b.unread > 0) - (a.unread > 0));
      for (const ch of rooms) {
        if (M.level !== 'home' || left <= 0) break;
        const p = PREV.get(ch.gid); if (p && p.sig === ch.unread + ':' + ch.status && Date.now() - p.t < 30000) continue;
        left--; if (await prevOf(ch, 30000)) changed = true;
      }
      if (changed && M.level === 'home') render();
    } finally { homePreviews.busy = false; }
  }
  // home: who is in a call in every community, in one request every 8 s while the cards are on screen (the relay takes 64 rooms at most)
  let cpAt = 0, cpBackoff = 0, cpSig = '', cpBusy = false;
  async function homePresence() {
    if (cpBusy || M.level !== 'home' || document.hidden || Date.now() < cpBackoff || Date.now() - cpAt < 8000) return;
    const cids = comms().filter(c => c.channels.some(x => x.kind === 'voice' && x.joined)).map(c => c.id); if (!cids.length) return;
    cpAt = Date.now(); cpBusy = true;
    try {
      const r = await rpc('call_presence', { cids }), rooms = (r && r.rooms) || {}, sig = JSON.stringify(rooms);
      if (sig !== cpSig) { cpSig = sig; CP.clear(); for (const [g, v] of Object.entries(rooms)) if (Array.isArray(v) && v.length) CP.set(g, v); if (M.level === 'home') render(); }
    } catch (e) { cpBackoff = Date.now() + (/no call server/.test(String((e && e.message) || e)) ? 60000 : 16000); }
    finally { cpBusy = false; }
  }
  setInterval(() => { if (M.started && M.level === 'home') { homePresence(); homePreviews(); } }, 2500);

  function cardHTML(x) {
    if (x.kind === 'news') return __megaNew.html(x);   // the "what's new" card (mega-new.js)
    let mid = '', sub = '', num = '';
    if (x.kind === 'community') {
      sub = x.members ? `${x.members} ${x.members === 1 ? 'person' : 'people'}` : `${x.rooms} room${x.rooms === 1 ? '' : 's'}`; num = x.unread || '';
      if (x.voiceName && x.people.length) mid += `<div class="mfaces-row">${mfaces(x.people)}<span>${x.people.length} in ${esc(x.voiceName)}</span></div>`;
      if (x.pv && x.pv.text) mid += `<div class="mpreview"><b>${esc(x.pv.who)}</b> ${esc(x.pv.text)}</div>`;
    } else if (x.kind === 'you') { sub = 'settings'; mid = '<div class="mpreview">your profile and account</div>'; }
    else if (x.kind === 'voice') {
      sub = 'voice'; num = x.people.length || '';
      mid = x.people.length ? `<div class="mwho">${x.people.map(p => `<span>${mface(p.name, p.speaking ? 'talk' : '')}${esc(p.me ? 'you' : p.name)}</span>`).join('')}</div>` : '<div class="mpreview">nobody here yet</div>';
      if (x.locked) mid = '<div class="mpreview">this device does not have the keys for this room yet</div>';
      else mid += `<div class="mvact"><button class="mjoin" data-act="${x.here ? 'leave' : 'join'}" type="button">${x.here ? 'leave' : 'join'}</button></div>`;
      if (x.locked) sub = 'waiting for keys';
    } else if (x.kind === 'admin') { sub = 'owners and admins'; mid = '<div class="mpreview">name, rooms, members, invites. Everyone here sees what you change.</div>'; }
    else if (x.kind === 'options') { sub = 'only on this device'; mid = '<div class="mpreview">alerts, sound and look. Nobody else can see these.</div>'; }
    else {
      sub = x.locked ? 'waiting for keys' : x.status ? x.status : x.unread ? 'unread' : 'caught up'; num = x.unread || '';
      if (x.locked) mid = '<div class="mpreview">this device does not have the keys for this room yet</div>';
      else if (x.pv && x.pv.text) mid = `<div class="mpreview"><b>${esc(x.pv.who)}</b> ${esc(x.pv.text)}</div>`;
    }
    return `<div class="msq" style="${cardVars(x)}"><div class="mtop"><span>${esc(sub)}</span>${x.at ? `<span class="mat" aria-label="${x.at} mention${x.at === 1 ? '' : 's'}">@${x.at}</span>` : ''}</div>
      <div class="mmid">${mid}</div>${ti(x.name)}${num !== '' ? `<div class="mnu" aria-label="${num}">${letters(String(num))}</div>` : ''}</div>`;
  }
  const cardLabel = x => [x.name, x.kind === 'voice' ? 'voice room' : '', x.kind === 'voice' && x.people.length ? x.people.length + ' in the call' : '', x.unread && x.kind !== 'voice' ? x.unread + ' unread' : '', x.at ? x.at + ' mentions of you' : '', x.locked ? 'waiting for keys' : ''].filter(Boolean).join(', ');

  // ---------------------------------------------------------------- the snap list
  // The card nearest the middle is the focused one. Resting on it only grows it; opening needs a click or Enter.
  function List(items, opts = {}) {
    const el = document.createElement('div'); el.className = 'mlist mrise'; el.tabIndex = -1; el.setAttribute('role', 'list');
    el.innerHTML = items.map((x, i) => (x.kind === 'voice' && (i === 0 || items[i - 1].kind !== 'voice') ? '<div class="msect" aria-hidden="true">voice</div>' : '') + `<div class="mslot" data-i="${i}" role="listitem"><article class="mcard ${x.locked ? 'locked' : ''}" aria-label="${esc(cardLabel(x))}">${cardHTML(x)}</article></div>`).join('');
    mega.append(el);
    const sc = document.createElement('div'); sc.className = 'mscrub'; sc.setAttribute('aria-hidden', 'true');
    const scrubCls = x => `${x.unread ? 'un' : ''} ${x.unread && x.at ? 'at' : ''} ${x.kind === 'voice' ? 'v' : x.kind === 'options' ? 'o' : ''}`;
    sc.innerHTML = items.map((x, i) => `<button type="button" tabindex="-1" data-i="${i}" class="${scrubCls(x)}" style="--c:${dotOf(x)};--mh:${markOf(x)}"></button>`).join('');
    sc.onclick = e => { const b = e.target.closest('[data-i]'); if (b) go(+b.dataset.i); };
    mega.append(sc);
    const slots = $$('.mslot', el), cards = slots.map(s => $('.mcard', s));
    const live = new Map();
    let cur = opts.start || 0, raf = 0, t = 0, settled = -1, settledTop = 0, tops = [], sh = 1;
    const measure = () => { tops = slots.map(s => s.offsetTop); sh = slots[0] ? slots[0].offsetHeight : 1; };
    function frame() {
      raf = 0;
      const mid = el.scrollTop + el.clientHeight / 2; let best = 0, bd = 1e9;
      slots.forEach((s, i) => {
        const a = Math.abs(tops[i] + sh / 2 - mid) / sh;
        if (a < bd) { bd = a; best = i; }
        const k = Math.min(a, 1.5);
        cards[i].style.transform = `scale(${(1 - .26 * k).toFixed(4)})`;
        cards[i].style.opacity = Math.max(0, 1 - .72 * a).toFixed(3);
        cards[i].style.visibility = a > 1.7 ? 'hidden' : '';
        if (a < 1.35) { if (!live.has(i)) { const o = asciiOn($('.msq', s), items[i]); if (o) live.set(i, o); } } else if (live.has(i)) { asciiOff(live.get(i)); live.delete(i); }
      });
      live.forEach((e, i) => e.focus = i === best);
      if (cur !== best || !sc.querySelector('.on')) { sc.querySelector('.on')?.classList.remove('on'); sc.children[best]?.classList.add('on'); }
      cur = best;
    }
    const settle = () => {
      if (settled === cur) return;
      if (settled >= 0) slots[settled]?.classList.remove('settled');
      settled = cur; settledTop = el.scrollTop; slots[cur].classList.add('settled'); remaskAll(); setTimeout(remaskAll, 850);
      if (opts.onSettle) opts.onSettle(items[cur], $('.msq', slots[cur]));
    };
    el.addEventListener('scroll', () => {
      if (!raf) raf = requestAnimationFrame(frame);
      energy = 1;
      if (settled >= 0 && Math.abs(el.scrollTop - settledTop) > 3) { slots[settled]?.classList.remove('settled'); settled = -1; remaskAll(); }
      clearTimeout(t); t = setTimeout(settle, 150);
    }, { passive: true });
    const go = (i, now) => { if (!now && root.classList.contains('mg-drawing-card')) return; i = Math.max(0, Math.min(items.length - 1, i)); el.scrollTo({ top: tops[i] - (el.clientHeight - sh) / 2, behavior: now || calm() ? 'auto' : 'smooth' }); };
    el.addEventListener('click', e => {
      const s = e.target.closest('.mslot'); if (!s) return;
      const i = +s.dataset.i;
      if (i !== cur) { go(i); return; }
      const x = items[i];
      // a voice card is never joined by resting on it or by clicking its body: only its own join button does
      if (x.kind === 'voice') { const b = e.target.closest('[data-act]'); if (b && opts.join) opts.join(x, b.dataset.act); return; }
      if (settled !== i) return;
      opts.enter(x, i);
    });
    el.addEventListener('pointermove', e => { const a = live.get(cur); if (!a) return; const r = a.sq.getBoundingClientRect(), k = a.sq.offsetWidth / r.width; a.mx = (e.clientX - r.left) * k / a.cw; a.my = (e.clientY - r.top) * k / a.ch; a.bloom = 1; }, { passive: true });
    const dropAscii = () => { live.forEach(asciiOff); live.clear(); };
    let rz = 0, firstRz = true; const ro = new ResizeObserver(() => { if (!el.offsetHeight) return; measure(); go(cur, true); frame(); if (firstRz) { firstRz = false; return; } clearTimeout(rz); rz = setTimeout(() => { dropAscii(); frame(); remaskAll(); }, 160); }); ro.observe(el);
    measure(); go(cur, true); frame(); settle();
    return {
      el, go, move: d => go(cur + d), get cur() { return cur; }, get item() { return items[cur]; }, get items() { return items; }, get settled() { return settled === cur; },
      // keep the cards, redraw only the ones whose content changed; false when the set of cards itself changed
      refresh(next) {
        if (next.length !== items.length || next.some((x, i) => x.key !== items[i].key)) return false;
        next.forEach((x, i) => {
          const was = items[i]; items[i] = x;
          if (x.sig === was.sig) return;
          if (live.has(i)) { asciiOff(live.get(i)); live.delete(i); }
          $('.mcard', slots[i]).innerHTML = cardHTML(x); $('.mcard', slots[i]).setAttribute('aria-label', cardLabel(x)); $('.mcard', slots[i]).classList.toggle('locked', !!x.locked);
          sc.children[i].className = scrubCls(x);
        });
        frame(); setTimeout(remaskAll, 60); return true;
      },
      // new colours only: same cards, new paint (no content is rebuilt, so the ascii keeps flowing)
      repaint(next) {
        if (next.length !== items.length) return;
        next.forEach((x, i) => { if (x.key !== items[i].key) return; items[i] = x; $('.msq', slots[i]).style.cssText = cardVars(x); sc.children[i].style.setProperty('--c', dotOf(x)); sc.children[i].style.setProperty('--mh', markOf(x)); });
        live.forEach((a, i) => {
          a.x = items[i]; a.at = atlasOf(fieldOf(a.x), hiOf(a.x), a.sw, a.sh, a.fs);
          if (a.field !== FX[a.x.fx]) { a.field = FX[a.x.fx]; fade(a.c); }   // a new pattern fades in over the old picture
          asciiDraw(a, clock);
        });
      },
      visible() { return slots.map((s, i) => ({ x: items[i], i, a: Math.abs(i - cur) })).filter(o => o.a <= 1).map(o => ({ ...o, r: $('.msq', slots[o.i]).getBoundingClientRect() })); },
      rectOf(i) { return $('.msq', slots[i]).getBoundingClientRect(); },
      sq() { return $('.msq', slots[cur]); },
      destroy() { ro.disconnect(); clearTimeout(t); clearTimeout(rz); dropAscii(); el.remove(); sc.remove(); },
    };
  }

  // ---------------------------------------------------------------- levels
  const guard = fn => async (...a) => { if (M.busy) return; M.busy = true; try { await fn(...a); } finally { M.busy = false; if (M.dirty) { M.dirty = false; render(); } } };
  const sq = () => getComputedStyle(root).getPropertyValue('--m-sq');
  function fly(r, m) { const f = document.createElement('div'); f.className = 'mfly'; f.style.cssText = `left:${r.left}px;top:${r.top}px;width:${r.width}px;height:${r.height}px;--m-mesh:${m}`; fxl.append(f); return f; }
  function killList() { if (window.__megaWall) __megaWall.leave(); if (M.list) { M.list.destroy(); M.list = null; } mega.innerHTML = ''; hideTip(); closeRail(); }
  function applyLevel() {
    const chat = M.level === 'chat';
    mega.hidden = chat; rail.hidden = !chat;
    root.classList.toggle('mg-chat', chat); root.classList.toggle('mg-cards', !chat);
    if (!chat) { hideOpts(); closeRail(); }
    if (window.__placeCallDock) window.__placeCallDock();
    syncRoom();
  }
  // Switching community through the same state the old switcher used, so drafts, members and call presence follow.
  function setCommunity(cid) {
    if (S.cid === cid) return;
    const inp = $('#input'); if (S.gid) S.drafts.set(S.gid, inp.value);
    inp.value = ''; S.cid = cid; S.gid = null; resetRoom(); S.membersKey = ''; refreshMembers();
  }
  function openRoomVia(gid) {
    M.opening = true;
    try {
      const g0 = S.gid, d0 = g0 ? S.drafts.get(g0) : undefined;
      openRoom(gid);
      // openRoom stores the composer text under the room that was current; if that was only a placeholder, put its draft back
      if (g0 && g0 !== gid) { if (d0 !== undefined) S.drafts.set(g0, d0); else if (S.drafts.get(g0) === '') S.drafts.delete(g0); }
    } finally { M.opening = false; }
  }

  function syncText() {
    const el = $('#m-sync'); if (!el || !S.snap) return;
    const st = $('#sync-status'), words = S.snap.revoked ? 'This device has been revoked.' : (st && !st.hidden ? st.textContent : '');
    el.textContent = words; el.hidden = !words; el.classList.toggle('bad', !!words);
  }
  function barHTML() {
    const cs = comms(), c = M.level === 'server' ? comm(M.cid) : null;
    const dot = $('#activity-dot') && !$('#activity-dot').hidden ? '<span class="mdot" title="new in the activity log"></span>' : '';
    const more = `<button class="mchip" data-m="more" type="button" aria-haspopup="menu">more${dot}</button>`;
    const jump = `<button class="mchip" data-m="jump" type="button">jump<span class="mhide-s">&nbsp;to</span> <span class="mtag mhide-s">ctrl k</span></button>`;
    if (!c) return `<div class="mbar"><span class="mmark">mega</span><span class="mtag" id="m-sync" hidden></span><span class="mgrow"></span><button class="mchip" data-m="paint" type="button" hidden>colours</button><button class="mchip" data-m="draw" type="button" aria-pressed="false" hidden>draw</button><button class="mchip" data-m="new" type="button">new</button><button class="mchip" data-m="join" type="button">join</button>${more}${jump}</div>`;
    const admin = c.role === 'admin' || c.role === 'owner', names = S.membersFor === c.id ? S.members.map(m => m.name) : [];
    return `<div class="mbar">${cs.length > 1 ? '<button class="mchip" data-m="back" type="button" aria-label="Back to all communities"><span aria-hidden="true">&larr;</span><span class="mhide-s"> back</span></button>' : ''}<span class="mmark">${esc(c.name)}</span><span class="mtag" id="m-sync" hidden></span><span class="mgrow"></span><button class="mchip" data-m="paint" type="button" hidden>colours</button><button class="mchip" data-m="draw" type="button" aria-pressed="false" hidden>draw</button>${admin ? '<button class="mchip mhide-s" data-m="invite" type="button">invite</button><button class="mchip mhide-s" data-m="room" type="button">new room</button>' : ''}<button class="mchip" data-m="people" type="button" aria-label="People">${names.length ? mfaces(names.slice(0, 3)) : ''}<span class="mhide-s">people</span></button>${more}${jump}</div>`;
  }
  function drawBar() {
    const html = barHTML(); let bar = $('.mbar', mega);
    if (bar && html !== M.barSig) { bar.outerHTML = html; }
    else if (!bar) mega.insertAdjacentHTML('afterbegin', html);
    M.barSig = html; syncText(); if (M.list) paintFollow(M.list.item, M.list.sq());
  }
  function notesHTML() {
    const sn = S.snap; if (!sn) return '';
    const rows = [
      ...sn.refused.map(j => `<div class="mnote">Could not join “${esc(j.name)}”: ${esc(j.reason)}. <button type="button" data-join="${esc(j.id)}">Dismiss</button></div>`),
      ...sn.joining.map(j => `<div class="mnote">Asked to join “${esc(j.name)}”. An admin has to let you in; this can take a while if nobody is online. <button type="button" data-join="${esc(j.id)}">Stop waiting</button></div>`),
    ];
    return rows.length ? `<div class="mnotes">${rows.join('')}</div>` : '';
  }
  function drawNotes() {
    const html = notesHTML(), old = $('.mnotes', mega);
    if (old) old.remove();
    if (html) mega.insertAdjacentHTML('beforeend', html);
  }

  function homeItems() { const nw = window.__megaNew && __megaNew.item(); return [...(nw ? [nw] : []), ...comms().map(commItem), youItem()]; }
  function showHome(startCid, startKey) {
    killList(); M.level = 'home'; M.opts = false; M.adm = false; applyLevel();
    drawBar(); M.sig = '';
    const items = homeItems(), want = startCid || store.get('last', null) || S.cid;
    const ki = startKey ? items.findIndex(x => x.key === startKey) : -1, start = ki >= 0 ? ki : Math.max(0, items.findIndex(x => x.cid === want));
    M.list = List(items, { start, enter: enterCommunity, onSettle: paintFollow });
    drawNotes(); homePresence(); homePreviews();
  }
  function showServer(cid, startKey) {
    const c = comm(cid); if (!c) { showHome(); return; }
    setCommunity(cid); store.set('last', cid);
    killList(); M.level = 'server'; M.cid = cid; M.opts = false; M.adm = false; applyLevel();
    drawBar();
    const items = roomItems(c);
    let start = startKey ? items.findIndex(x => x.key === startKey) : items.findIndex(x => x.unread > 0 && x.kind === 'text');
    if (start < 0) start = 0;
    M.list = List(items, { start, enter: enterRoom, join: joinVoice, onSettle: paintFollow });
    drawNotes(); loadPreviews(c);
  }
  const goHome = () => { const cs = comms(); if (cs.length === 1) showServer(cs[0].id, S.gid); else showHome(M.cid || S.cid); };

  // home -> a community: the card opens up to fill the room, then its rooms
  const enterCommunity = guard(async x => {
    if (x.kind === 'you') { $('#btn-settings').click(); return; }
    if (x.kind === 'news') return;   // its rows are the buttons
    if (calm() || !M.list) { showServer(x.cid); return; }
    const r = M.list.rectOf(M.list.cur), f = fly(r, fieldOf(x)), full = mega.getBoundingClientRect();
    await f.animate([{ left: r.left + 'px', top: r.top + 'px', width: r.width + 'px', height: r.height + 'px', borderRadius: sq() }, { left: full.left + 'px', top: '0px', width: full.width + 'px', height: full.height + 'px', borderRadius: '0px' }], { duration: 320, easing: 'cubic-bezier(.7,0,.2,1)', fill: 'forwards' }).finished;
    showServer(x.cid);
    await f.animate([{ opacity: 1 }, { opacity: 0 }], { duration: 200, fill: 'forwards' }).finished; f.remove();
  });
  function enterRoom(x) {
    if (x.locked) { toast('This room is still waiting for keys.'); return; }
    if (x.kind === 'voice') return;               // opening a voice card does nothing; its join button (or Enter) joins
    closeIntoDots(x);
  }
  // The only places that may join a call. (Also: a click on a voice dot.)
  function joinVoice(x, act) {
    try {
      if (x.here || act === 'leave') { if (x.here) window.__commonsCall.leave().catch(() => { }); return; }
      if (window.__callUI && window.__callUI.joinRoom) window.__callUI.joinRoom(x.gid, x.name); else toast('Calls are not available in this window.');
    } catch (er) { toast(er.message); }
  }

  // the cards close into dots on the left, and the chat opens beside them
  const closeIntoDots = guard(async x => {
    const from = M.list ? M.list.visible() : [];
    M.level = 'chat'; M.cid = x.cid; M.opts = x.kind === 'options' || x.kind === 'admin'; M.adm = x.kind === 'admin';
    if (!M.opts) openRoomVia(x.gid);
    drawRail(true); killList(); applyLevel(); showOpts();
    if (calm() || !from.length) { rail.classList.remove('arriving'); return; }
    $('#main').classList.add('mslide'); setTimeout(() => $('#main').classList.remove('mslide'), 700);
    const runs = from.map(o => {
      const dot = $(`.mitem[data-key="${CSS.escape(o.x.key)}"]`, rail); if (!dot) return Promise.resolve();
      dot.classList.add('hold');
      const d = dot.getBoundingClientRect(), r = o.r, f = fly(r, fieldOf(o.x));
      return f.animate([{ left: r.left + 'px', top: r.top + 'px', width: r.width + 'px', height: r.height + 'px', borderRadius: sq(), backgroundColor: fieldOf(o.x) }, { left: d.left + 'px', top: d.top + 'px', width: d.width + 'px', height: d.height + 'px', borderRadius: d.height > d.width ? '8px' : '50%', backgroundColor: dotOf(o.x) }], { duration: 460, delay: o.x.key === x.key ? 50 : 0, easing: 'cubic-bezier(.76,0,.18,1)', fill: 'forwards' }).finished.then(() => { dot.classList.remove('hold'); dot.style.animation = 'none'; f.remove(); });
    });
    await Promise.all(runs); rail.classList.remove('arriving');
  });
  // and the other way: the dots open back out into the cards
  const openFromDots = guard(async () => {
    const cid = M.cid, key = M.opts ? (M.adm ? 'adm:' : 'opt:') + cid : S.gid;
    if (calm()) { showServer(cid, key); return; }
    const dots = new Map($$('.mitem', rail).map(d => [d.dataset.key, d.getBoundingClientRect()]));
    showServer(cid, key); if (!M.list) return; M.list.el.style.visibility = 'hidden';
    await wait(20);
    const runs = M.list.visible().map(o => { const d = dots.get(o.x.key); if (!d) return Promise.resolve(null); const r = o.r, f = fly(d, fieldOf(o.x)); return f.animate([{ left: d.left + 'px', top: d.top + 'px', width: d.width + 'px', height: d.height + 'px', borderRadius: '50%', backgroundColor: dotOf(o.x) }, { left: r.left + 'px', top: r.top + 'px', width: r.width + 'px', height: r.height + 'px', borderRadius: sq(), backgroundColor: fieldOf(o.x) }], { duration: 380, easing: 'cubic-bezier(.3,1.1,.3,1)', fill: 'forwards' }).finished.then(() => f); });
    const fs = await Promise.all(runs); if (M.list) M.list.el.style.visibility = ''; fs.forEach(f => f && f.remove());
  });
  // straight into a chat from anywhere (jump to, search, the old paths): no flying
  function toChat(cid, gid) {
    setCommunity(cid);
    if (gid) openRoomVia(gid);
    killList(); M.level = 'chat'; M.cid = cid; M.opts = !gid; M.adm = false; drawRail(); applyLevel(); showOpts();
  }
  const toChatOpen = cid => { killList(); M.level = 'chat'; M.cid = cid; M.opts = false; M.adm = false; drawRail(); applyLevel(); showOpts(); };

  // ---------------------------------------------------------------- the dots
  function dotHTML(x, i) {
    const vo = x.kind === 'voice', active = x.kind === 'options' ? M.opts && !M.adm : x.kind === 'admin' ? M.adm : (!M.opts && x.gid === S.gid);
    const say = [x.name, vo ? 'voice room' : '', vo && x.people.length ? x.people.length + ' in the call' : '', x.unread && !active ? x.unread + ' unread' : '', x.at ? x.at + ' mentions of you' : '', x.locked ? 'waiting for keys' : ''].filter(Boolean).join(', ');
    const un = x.unread && !active && !vo;
    return `<button class="mitem ${vo ? 'voice' : ''} ${active ? 'active' : ''} ${un ? 'un' : ''} ${un && x.at ? 'at' : ''} ${vo && x.people.length ? 'live' : ''} ${vo && x.here ? 'here' : ''} ${x.kind === 'options' ? 'opt' : x.kind === 'admin' ? 'adm' : ''} ${x.locked ? 'pending' : ''}" type="button" data-key="${esc(x.key)}" aria-label="${esc(say)}"${active ? ' aria-current="true"' : ''} style="--c:${dotOf(x)};--mh:${markOf(x)};--m-cink:${bestInk(fieldOf(x))};--n:${i}">${x.at && !active ? `<span class="mbadge">${x.at}</span>` : ''}${vo ? `<span class="mvc">${x.people.length ? esc(x.people.map(p => p.me ? 'you' : p.name).join(', ')) : 'empty'}</span>` : ''}${ti(x.name, 'mlb', 'span')}${(vo ? x.people.length : un ? x.unread : 0) ? `<span class="mct">${vo ? x.people.length : x.unread}</span>` : ''}</button>`;
  }
  const railSig = c => JSON.stringify([c.id, S.gid, M.opts, M.adm, roomItems(c).map(x => x.sig), comms().map(o => o.id !== c.id && unreadIn(o) > 0)]);
  function drawRail(arriving) {
    const c = comm(M.cid); if (!c) return;
    const items = roomItems(c), others = comms().some(o => o.id !== c.id && unreadIn(o) > 0), many = comms().length > 1;
    M.railItems = items; M.sig = railSig(c);
    rail.className = arriving ? 'arriving' : ''; M.rail = false;
    syncRoom();
    rail.innerHTML = `<button class="mhomedot ${others ? 'un' : ''}" id="m-home" type="button" aria-label="${many ? 'All communities' : 'All rooms'}${others ? ', unread messages elsewhere' : ''}" data-name="${many ? 'all communities' : 'all rooms'}"><span class="mhl">${many ? 'all communities' : 'all rooms'}</span></button><div class="mitems" id="m-items">${items.map(dotHTML).join('')}</div>`;
  }
  function openRail() {
    if (M.rail || M.level !== 'chat') return; M.rail = true; hideTip(); if (window.__megaNew) __megaNew.railUsed();
    $$('.mitem', rail).forEach((b, i) => { const x = M.railItems[i]; if (x) b.style.cssText += ';' + cardVars(x, true); });
    rail.classList.add('open'); const a = $('.mitem.active', rail); if (a) setTimeout(() => a.scrollIntoView({ block: 'center', behavior: calm() ? 'auto' : 'smooth' }), 240);
  }
  function closeRail() { if (!M.rail) return; M.rail = false; clearTimeout(M.railT); rail.classList.remove('open'); $$('.mitem', rail).forEach(i => i.style.transform = ''); const it = $('#m-items'); if (it) it.scrollTop = 0; }
  let railRaf = 0;
  rail.addEventListener('wheel', e => { if (!M.rail) { e.preventDefault(); openRail(); } }, { passive: false });
  rail.addEventListener('mouseleave', () => { M.railT = setTimeout(closeRail, 380); hideTip(); });
  rail.addEventListener('mouseenter', () => clearTimeout(M.railT));
  rail.addEventListener('scroll', e => {
    const items = e.target; if (railRaf || !M.rail || items.id !== 'm-items') return;
    railRaf = requestAnimationFrame(() => { railRaf = 0; const b = items.getBoundingClientRect(), m = b.top + b.height / 2; $$('.mitem', items).forEach(i => { const r = i.getBoundingClientRect(), d = Math.min(1, Math.abs(r.top + r.height / 2 - m) / (b.height / 2)); i.style.transform = `scale(${(1 - .16 * d).toFixed(3)})`; }); });
  }, { passive: true, capture: true });
  rail.addEventListener('contextmenu', e => {
    const b = e.target.closest('.mitem'); if (!b) return;
    const x = (M.railItems || []).find(k => k.key === b.dataset.key);
    if (x && canPaint(x)) { e.preventDefault(); paint(x); }
  });
  rail.addEventListener('click', e => {
    if (e.target.closest('#m-home')) { goHomeFromChat(); return; }
    const b = e.target.closest('.mitem'); if (!b) return;
    const x = (M.railItems || []).find(k => k.key === b.dataset.key); if (x) switchChat(x, true);
  });
  rail.addEventListener('mouseover', e => {
    const b = e.target.closest('.mitem, #m-home'); if (!b || M.rail) return;
    const x = b.dataset.key ? (M.railItems || []).find(k => k.key === b.dataset.key) : null;
    showTip(b, x ? (x.kind === 'voice' && x.people.length ? `${x.name}, ${x.people.length} in voice` : x.locked ? `${x.name}, waiting for keys` : x.name) : b.dataset.name);
  });
  rail.addEventListener('mouseout', e => { if (!e.relatedTarget || !e.relatedTarget.closest || !e.relatedTarget.closest('.mitem, #m-home')) hideTip(); });
  function showTip(el, t) { tip.textContent = t; tip.hidden = false; const r = el.getBoundingClientRect(); tip.style.left = r.right + 12 + 'px'; tip.style.top = r.top + r.height / 2 - tip.offsetHeight / 2 + 'px'; }
  function hideTip() { tip.hidden = true; }
  // the home dot: all communities (with only one community, its rooms)
  function goHomeFromChat() { closeRail(); if (comms().length > 1) { M.cid = S.cid; showHome(M.cid); } else openFromDots(); }

  function switchChat(x, fromRail) {
    if (x.locked) { toast('This room is still waiting for keys.'); return; }
    if (x.kind === 'voice') { hideTip(); if (fromRail) closeRail(); joinVoice(x, x.here ? 'leave' : 'join'); return; }   // a click on a voice dot is an explicit join
    closeRail();
    if (x.kind === 'options' || x.kind === 'admin') { M.opts = true; M.adm = x.kind === 'admin'; showOpts(); }
    else { M.opts = false; M.adm = false; showOpts(); openRoomVia(x.gid); }
    drawRail();
  }
  const textRooms = () => { const c = comm(M.cid); return c ? roomItems(c).filter(x => x.kind === 'text' && !x.locked) : []; };

  // ---------------------------------------------------------------- the options room (client-only; same controls and ids as Settings)
  function hideOpts() { optsEl.hidden = true; optsEl.innerHTML = ''; }
  function optsHTML() {
    const wave = window.__waveDither ? window.__waveDither.palettes : [];
    const th = pref.get('theme') || '';
    return `<div class="mopts-in mrise">${ti('options', '', 'h1')}<p>Only on this device. Nobody else can see these.</p>
      <section><h3>alerts and sound</h3><label>Sound for new messages</label>${seg('snd', [['all', 'All messages'], ['mentions', 'Mentions and replies'], ['off', 'Off']], soundPref())}<p>Plays when mega is in the background or the message is in another room.</p></section>
      <section><h3>on the cards</h3><label class="check"><input type="checkbox" id="pres" ${window.__megaPres && !__megaPres.on ? '' : 'checked'}><span>Show where I am on the cards — friends who share a community see a small dot in your colour on the card you are looking at; the relay sees only that a small packet was sent</span></label></section>
      <section><h3>typing hints</h3><label class="check"><input type="checkbox" id="ty" ${S.snap.typing_enabled ? 'checked' : ''}><span>Send “typing…” hints — the relay can see that a message-sized packet was sent at that moment</span></label></section>
      ${mobileApp() ? '' : `<section><h3>large files</h3><label for="dl-dir">Large-file downloads folder</label><input type="text" id="dl-dir" value="${esc(S.snap.download_dir || '')}" spellcheck="false"><p>Written here as they arrive, one verified piece at a time. Leave empty for your usual Downloads folder. Each download still asks where to save it.</p></section>`}
    </div>`;
  }
  function showOpts() {
    syncRoom();
    if (!M.opts || M.level !== 'chat') { hideOpts(); return; }
    optsEl.innerHTML = M.adm && window.__megaAdmin ? __megaAdmin.html(M.cid) : optsHTML(); optsEl.hidden = false; optsEl.scrollTop = 0;
  }
  optsEl.addEventListener('click', e => {
    const b = e.target.closest('.seg button'); if (!b) return;
    const g = b.parentElement, id = g.dataset.for, v = b.dataset.v;
    $$('button', g).forEach(x => x.setAttribute('aria-checked', x === b));
    $('#' + id, optsEl).value = v;
    if (id === 'snd') pref.set('sound', v === 'all' ? '' : v);
    else if (id === 'th') { pref.set('theme', v); applyPrefs(); if (window.__waveDither) window.__waveDither.refresh(); const r = $('#wave-row', optsEl); if (r) r.hidden = v !== 'wave'; }
  });
  optsEl.addEventListener('change', async e => {
    try {
      if (e.target.id === 'pres') { if (window.__megaPres) __megaPres.set(e.target.checked); }
      else if (e.target.id === 'ty') { await rpc('set_typing_enabled', { on: e.target.checked }); tick(); }
      else if (e.target.id === 'dl-dir') { await rpc('set_download_dir', { dir: e.target.value }); tick(); }
      else if (e.target.id === 'wave-pal') { pref.set('wavePalette', e.target.value); applyPrefs(); if (window.__waveDither) window.__waveDither.refresh(); }
    } catch (er) { toast(er.message); }
  });
  optsEl.addEventListener('keydown', e => { if (e.key === 'Enter' && e.target.id === 'dl-dir') e.target.blur(); });


  // ---- following the snapshot: the owner's colours arrive, fade in, and are announced once by name
  const calmNow = () => typeof calm === 'function' && calm();
  function setShown(id, to, animate) {
    const from = SHOWN.get(id); GOAL.set(id, to); TW.delete(id);
    if (!from || !animate || calmNow()) { SHOWN.set(id, to); return false; }
    const t0 = performance.now(), tok = {}; TW.set(id, tok);
    // timers, not animation frames: a window in the background still ends on the right colour
    const step = () => {
      if (TW.get(id) !== tok) return;
      const k = Math.min(1, (performance.now() - t0) / 650), e = k * k * (3 - 2 * k);
      SHOWN.set(id, k >= 1 ? to : [mix(from[0], to[0], e), mix(from[1], to[1], e)]); repaintColours();
      if (k < 1) setTimeout(step, 33); else TW.delete(id);
    };
    setTimeout(step, 0); return true;
  }
  // A look changed by somebody else: the pattern fades over the old one, the backdrop strength eases to its new value.
  const DTW = new Map();
  function tweenDim(id, to) {
    const from = DIMS.has(id) ? DIMS.get(id) : 100, tok = {}; DTW.set(id, tok);
    if (calmNow() || from === to) { DIMS.set(id, to); repaintColours(); return; }
    const t0 = performance.now();
    const step = () => {
      if (DTW.get(id) !== tok) return;
      const k = Math.min(1, (performance.now() - t0) / 650), e = k * k * (3 - 2 * k);
      DIMS.set(id, k >= 1 ? to : Math.round(from + (to - from) * e)); repaintColours();
      if (k < 1) setTimeout(step, 33); else DTW.delete(id);
    };
    setTimeout(step, 0);
  }
  function syncLooks(c) {
    const now = Date.now(), names = [];
    for (const [id, name] of [[c.id, c.name], ...c.channels.map(ch => [ch.gid, ch.name])]) {
      const st = lookStored(c.id, id), s = JSON.stringify(st || null), was = LSEEN.get(id);
      LSEEN.set(id, s);
      if (was === undefined) { DIMS.set(id, dimOf(st || {})); continue; }
      if (was === s) continue;
      const h = LHOLD.get(id); if (h && h.until > now) continue;   // the owner is editing this one right now
      LHOLD.delete(id); names.push(name); tweenDim(id, dimOf(st || {})); repaintColours();
    }
    return names;
  }
  function syncColours() {
    const now = Date.now();
    for (const c of comms()) {
      const changed = [], lnames = syncLooks(c);
      for (const [id, gid, name] of [[c.id, null, c.name], ...c.channels.map(ch => [ch.gid, ch.gid, ch.name])]) {
        const to = targetOf(c.id, gid), goal = GOAL.get(id);
        if (!goal) { SHOWN.set(id, to); GOAL.set(id, to); continue; }
        if (goal[0] === to[0] && goal[1] === to[1]) continue;
        if ((HOLD.get(id) || 0) > now) continue;          // the owner is editing this one right now
        setShown(id, to, true); changed.push(name);
      }
      const ver = c.style ? c.style.ver : 0, was = SEEN.get(c.id);
      SEEN.set(c.id, ver);
      if (was !== undefined && ver > was && c.role !== 'owner' && (changed.length || lnames.length)) {
        const who = c.style && c.style.by ? c.style.by : 'the owner', list = a => (a.length > 2 ? a.length + ' rooms' : a.join(' and '));
        toast(`${who} changed the ${[changed.length ? 'colours of ' + list(changed) : '', lnames.length ? 'look of ' + list(lnames) : ''].filter(Boolean).join(' and the ')}`);
      }
    }
  }
  let rcRaf = 0;
  function repaintColours() {
    if (rcRaf) return;
    rcRaf = setTimeout(() => {
      rcRaf = 0; if (!M.started) return;
      const c = comm(M.cid);
      if (M.level === 'home') { if (M.list) M.list.repaint(homeItems()); }
      else if (M.level === 'server') { if (c && M.list) M.list.repaint(roomItems(c)); }
      else if (c) {
        const items = roomItems(c); M.railItems = items; M.sig = railSig(c);
        $$('.mitem', rail).forEach(b => { const x = items.find(k => k.key === b.dataset.key); if (x) { b.style.setProperty('--c', dotOf(x)); b.style.setProperty('--mh', markOf(x)); if (M.rail) b.style.cssText += ';' + cardVars(x, true); } });
        const it = items.find(x => x.gid === S.gid && x.kind === 'text');
        if (it && BD && BD.gid === it.gid && mainEl) {
          const hi = lift(hiOf(it), 4.8); root.style.setProperty('--mg-c', dotOf(it)); root.style.setProperty('--mg-hi', hi); root.style.setProperty('--mg-hi-ink', bestInk(hi));
          BD.f = fieldOf(it); BD.h = hiOf(it); bdLook(it); bdColours(BD); dirty(); if (window.__megaWall) __megaWall.recolour();
        }
      }
    }, 16);
  }

  // ---- the picker (owner only): square, hue, hex, presets, a warning when the two are too close, reset
  const PRESETS = DEF.slice(0, 8);
  const canPaint = x => !!x && x.kind !== 'options' && x.kind !== 'admin' && x.kind !== 'you' && (comm(x.cid || x.id) || {}).role === 'owner';
  const PT = { open: false, set() { } };
  const toHsv = h => { const [r, g, b] = rgb(h).map(v => v / 255), Mx = Math.max(r, g, b), d = Mx - Math.min(r, g, b); let hh = 0; if (d) hh = Mx === r ? ((g - b) / d + 6) % 6 : Mx === g ? (b - r) / d + 2 : (r - g) / d + 4; return [hh * 60, Mx ? d / Mx : 0, Mx]; };
  const fromHsv = (h, s, v) => { const f = n => { const k = (n + h / 60) % 6; return (v - v * s * Math.max(0, Math.min(k, 4 - k, 1))) * 255; }; return hex([f(5), f(3), f(1)]); };
  const cardLevel = () => M.level === 'home' || M.level === 'server';
  // The focused card is the only one whose wall is polled; the draw chip is there when this person may draw on it.
  function drawChip() {
    const b = $('[data-m="draw"]', mega); if (!b) return;
    const x = M.list && M.list.item, ok = !!(x && cardLevel() && window.__megaWall && __megaWall.drawable(x)), on = ok && __megaWall.isOn();
    b.hidden = !ok; b.setAttribute('aria-pressed', String(on)); b.textContent = on ? 'drawing' : 'draw';
  }
  function paintFollow(x, sq) {
    const b = $('[data-m="paint"]', mega); if (b) b.hidden = !(canPaint(x) && cardLevel());
    if (window.__megaWall) { if (x && cardLevel()) __megaWall.focus(x, sq || (M.list && M.list.sq())); else __megaWall.leave(); }
    drawChip();
    if (PT.open) { if (!x || (!canPaint(x) && x.kind !== 'options')) closePaint(); else PT.set(x); }
  }
  function closePaint() { const e = document.getElementById('mega-paint'); if (e) e.remove(); PT.open = false; PT.set = () => { }; }
  // sending: one request at a time, newest value per card, never on every pointer move
  const SAVE = new Map(); let saving = false;
  async function flushSaves() {
    if (saving) return; saving = true;
    try {
      while (SAVE.size) {
        const [id, o] = SAVE.entries().next().value; SAVE.delete(id);
        try {
          if (o.look) await rpc('set_look', { cid: o.cid, room: o.gid || '', ...o.look });
          else await rpc('set_style', { cid: o.cid, room: o.gid || '', main: o.main, hi: o.hi });
        } catch (er) { toast(er.message); }
        if (o.look) { const h = LHOLD.get(o.hid); if (h) h.until = Math.max(h.until, Date.now() + 4000); } else HOLD.set(id, Date.now() + 2500);
      }
    } finally { saving = false; }
  }
  const queueLook = (x, look) => { SAVE.set('L' + x.id, { cid: x.cid || x.id, gid: x.kind === 'community' ? '' : x.gid, look, hid: x.id }); flushSaves(); };
  const queueSave = (x, d) => { HOLD.set(x.id, Date.now() + 6000); SAVE.set(x.id, { cid: x.cid || x.id, gid: x.kind === 'community' ? '' : x.gid, main: d ? d[0] : '', hi: d ? d[1] : '' }); flushSaves(); };
  function paint(start) {
    if (PT.open) { closePaint(); return; }
    closePops();
    const e = document.createElement('div'); e.id = 'mega-paint'; e.setAttribute('role', 'dialog'); e.setAttribute('aria-label', 'Colours');
    e.innerHTML = `<div class="mp-head"><span id="mp-name"></span><button class="mchip" id="mp-done" type="button">done</button></div><p class="mtag" id="mp-note"></p><div class="mp-body">
      <div class="mp-wells"><button type="button" class="mp-well" data-w="0"><span></span>colour</button><button type="button" class="mp-well" data-w="1"><span></span>highlight</button></div>
      <div id="mp-sv" tabindex="0" role="slider" aria-label="Saturation and brightness"><span id="mp-svt"></span></div>
      <input id="mp-hue" type="range" min="0" max="359" step="1" aria-label="Hue">
      <label class="mp-hex">hex <input id="mp-hex" maxlength="7" spellcheck="false" autocomplete="off" aria-label="Hex colour"></label>
      <div class="mp-presets">${PRESETS.map((p, i) => `<button type="button" data-p="${i}" aria-label="Preset ${i + 1}" style="background:linear-gradient(135deg,${p[0]} 50%,${p[1]} 50%)"></button>`).join('')}</div>
      <div id="mp-looks"></div>
      <p class="mtag" id="mp-warn" role="status"></p><div class="mp-end"><button class="mchip" id="mp-reset" type="button">reset</button></div></div>`;
    document.body.append(e); PT.open = true;
    let w = 0, hsv = [0, 0, 0], target = start, timer = 0, ltimer = 0, tt = 0;
    const quietHold = new Map();
    const looksBox = $('#mp-looks', e), idOf = t => (t.kind === 'community' ? t.cid : t.gid);
    const sv = $('#mp-sv', e), hue = $('#mp-hue', e), hx = $('#mp-hex', e);
    const cur = () => (SHOWN.get(target.id) || themeOf(target)).slice();
    const show = () => {
      if (!canPaint(target)) return;
      const t = cur();
      sv.style.setProperty('--hc', fromHsv(hsv[0], 1, 1)); const th = $('#mp-svt', e); th.style.left = hsv[1] * 100 + '%'; th.style.top = (1 - hsv[2]) * 100 + '%'; th.style.background = t[w]; hue.value = Math.round(hsv[0]);
      if (document.activeElement !== hx) hx.value = t[w];
      $$('.mp-well', e).forEach(b => { b.setAttribute('aria-pressed', +b.dataset.w === w); $('span', b).style.background = t[+b.dataset.w]; });
      $('#mp-warn', e).textContent = contrast(t[0], t[1]) < 1.6 ? 'These two are too close, so the highlight will barely show.' : '';
    };
    // live on this screen at once; sent to everyone only when the person lets go
    // the choosers: card animation for any card, chat animation and backdrop strength for text rooms
    const fillLooks = () => {
      if (!canPaint(target)) { looksBox.innerHTML = ''; return; }
      const lk = lookOf(target.cid, idOf(target)), text = target.kind === 'text', t = cur();
      const tiles = (k, sel) => ['', ...FX.map((_, i) => i)].map(n => `<button type="button" class="mp-t" data-k="${k}" data-n="${n}" aria-pressed="${n === '' ? patOf(sel) == null : n === patOf(sel)}" aria-label="${n === '' ? 'automatic' : FXNAMES[n]}" style="background-image:${fxThumb(n === '' ? target.fxAuto : n, t[0], t[1])}"><i>${n === '' ? 'auto' : FXNAMES[n]}</i></button>`).join('');
      const d = dimOf(lk), qh = quietHold.get(idOf(target)), quiet = qh && qh.until > Date.now() ? qh.on : !!(lookStored(target.cid, idOf(target)) || {}).oracle_quiet;
      looksBox.innerHTML = `<p class="mp-lab" id="mp-l1">card animation</p><div class="mp-fx" role="group" aria-labelledby="mp-l1">${tiles('card', lk.card)}</div>`
        + (text ? `<p class="mp-lab" id="mp-l2">chat animation</p><div class="mp-fx" role="group" aria-labelledby="mp-l2">${tiles('chat', lk.chat)}</div>
          <label class="mp-dim"><span class="mp-lab">backdrop</span><input id="mp-dim" type="range" min="0" max="${DIM_MAX}" step="5" value="${d}" aria-label="Backdrop strength behind the chat"><output id="mp-dimv" for="mp-dim">${d === 0 ? 'off' : d + '%'}</output></label>
          <button type="button" id="mp-oracle" class="mp-oracle" aria-pressed="${quiet ? 'true' : 'false'}">${quiet ? 'oracle stays quiet here' : 'oracle can join in here'}</button>` : '');
    };
    const tint = () => { clearTimeout(tt); tt = setTimeout(() => { const t = cur(); $$('.mp-t', e).forEach(b => { b.style.backgroundImage = fxThumb(b.dataset.n === '' ? target.fxAuto : +b.dataset.n, t[0], t[1]); }); }, 120); };
    // a look is changed here at once, and sent only when committed (the same one-request-at-a-time queue as the colours)
    const editLook = (patch, commit) => {
      if (!canPaint(target)) return;
      const id = idOf(target), nl = { ...lookOf(target.cid, id), ...patch }, clean = {};
      for (const k of ['card', 'chat', 'dim']) if (Number.isFinite(nl[k])) clean[k] = nl[k];
      LHOLD.set(id, { look: clean, until: Date.now() + 6000 }); DIMS.set(id, dimOf(clean));
      repaintColours(); if (commit) queueLook(target, clean);
    };
    const apply = d => { setShown(target.id, d, false); HOLD.set(target.id, Date.now() + 6000); repaintColours(); show(); tint(); };
    const commit = () => { clearTimeout(timer); queueSave(target, cur()); };
    const later = () => { clearTimeout(timer); timer = setTimeout(commit, 450); };
    PT.set = x => {
      target = x; const ok = canPaint(x); e.classList.toggle('off', !ok);
      $('#mp-name').textContent = x.kind === 'options' ? 'options' : x.name;
      $('#mp-note').textContent = x.kind === 'options' ? 'The options room is private to each person, so it stays grey.' : 'Everyone in the community sees these colours. Only you, the owner, can change them.';
      if (ok) { hsv = toHsv(cur()[w]); show(); }
      fillLooks();
    };
    const edit = (i, v) => { const t = cur(); t[i] = v; apply(t); };
    const pick = ev => { const r = sv.getBoundingClientRect(); hsv[1] = Math.max(0, Math.min(1, (ev.clientX - r.left) / r.width)); hsv[2] = 1 - Math.max(0, Math.min(1, (ev.clientY - r.top) / r.height)); edit(w, fromHsv(...hsv)); };
    sv.onpointerdown = ev => { sv.setPointerCapture(ev.pointerId); pick(ev); sv.onpointermove = pick; };
    sv.onpointerup = () => { sv.onpointermove = null; commit(); };
    sv.onkeydown = ev => { const d = { ArrowLeft: [-.02, 0], ArrowRight: [.02, 0], ArrowUp: [0, .02], ArrowDown: [0, -.02] }[ev.key]; if (!d) return; ev.preventDefault(); ev.stopPropagation(); hsv[1] = Math.max(0, Math.min(1, hsv[1] + d[0])); hsv[2] = Math.max(0, Math.min(1, hsv[2] + d[1])); edit(w, fromHsv(...hsv)); later(); };
    hue.oninput = () => { hsv[0] = +hue.value; edit(w, fromHsv(...hsv)); }; hue.onchange = commit;
    hx.oninput = () => { let v = hx.value.trim(); if (v && v[0] !== '#') v = '#' + v; if (isHex(v)) { v = v.toLowerCase(); hsv = toHsv(v); edit(w, v); later(); } };
    hx.onblur = show;
    looksBox.addEventListener('click', async ev => {
      const b = ev.target.closest && ev.target.closest('#mp-oracle'); if (!b || b.disabled || target.kind !== 'text') return;
      const on = b.getAttribute('aria-pressed') !== 'true', t = target; b.disabled = true;
      quietHold.set(t.gid, { on, until: Date.now() + 15000 });
      try { await rpc('set_oracle_quiet', { cid: t.cid, room: t.gid, on }); b.setAttribute('aria-pressed', on ? 'true' : 'false'); b.textContent = on ? 'oracle stays quiet here' : 'oracle can join in here'; }
      catch (er) { quietHold.delete(t.gid); toast('Could not change that: ' + er.message); }
      b.disabled = false;
    });
    looksBox.addEventListener('input', ev => { if (ev.target.id !== 'mp-dim') return; const v = +ev.target.value; $('#mp-dimv', e).textContent = v === 0 ? 'off' : v + '%'; editLook({ dim: v }, false); });
    looksBox.addEventListener('change', ev => { if (ev.target.id !== 'mp-dim') return; const v = +ev.target.value; clearTimeout(ltimer); ltimer = setTimeout(() => editLook({ dim: v }, true), 350); });
    e.onclick = ev => {
      const tl = ev.target.closest('.mp-t'); if (tl) { editLook({ [tl.dataset.k]: tl.dataset.n === '' ? null : +tl.dataset.n }, true); $$('.mp-t[data-k="' + tl.dataset.k + '"]', e).forEach(b => b.setAttribute('aria-pressed', String(b === tl))); return; }
      const b = ev.target.closest('.mp-well'); if (b) { w = +b.dataset.w; hsv = toHsv(cur()[w]); show(); return; }
      const p = ev.target.closest('[data-p]'); if (p) { const d = PRESETS[+p.dataset.p].slice(); hsv = toHsv(d[w]); apply(d); commit(); return; }
      if (ev.target.id === 'mp-reset') { const d = DEF[H(target.gid || target.cid || target.id) % DEF.length].slice(); hsv = toHsv(d[w]); apply(d); clearTimeout(timer); queueSave(target, null); { const id = idOf(target); LHOLD.set(id, { look: {}, until: Date.now() + 6000 }); DIMS.set(id, 100); repaintColours(); queueLook(target, {}); fillLooks(); } return; }
      if (ev.target.id === 'mp-done') closePaint();
    };
    PT.set(start);
  }

  // ---------------------------------------------------------------- floats: menus, jump to
  function closePops() { const had = M.closers.length; M.closers.forEach(f => f()); M.closers = []; return had > 0; }
  function popAt(html, x, y) {
    closePops(); const p = document.createElement('div'); p.className = 'mpop'; p.innerHTML = html; document.body.append(p);
    p.style.left = Math.max(8, Math.min(x, innerWidth - p.offsetWidth - 8)) + 'px'; p.style.top = Math.max(8, Math.min(y, innerHeight - p.offsetHeight - 8)) + 'px';
    const onOut = e => { if (!p.contains(e.target)) closePops(); };
    setTimeout(() => document.addEventListener('pointerdown', onOut), 0);
    M.closers.push(() => { p.remove(); document.removeEventListener('pointerdown', onOut); });
    const f = $('button', p); if (f) f.focus({ preventScroll: true }); return p;
  }
  function menu(items, x, y) { const p = popAt(`<div class="mmenu" role="menu">${items.map((m, i) => `<button role="menuitem" type="button" data-m="${i}">${esc(m[0])}</button>`).join('')}</div>`, x, y); p.onclick = e => { const b = e.target.closest('[data-m]'); if (b) { closePops(); items[+b.dataset.m][1](); } }; }
  function moreMenu(btn) {
    const r = btn.getBoundingClientRect();
    // on a phone the colours and draw chips leave the bar (mega-design.css) and wait here, when they apply to the card in focus
    const phoneOnly = matchMedia('(max-width: 760px)').matches ? ['paint', 'draw'].map(k => $(`[data-m="${k}"]`, mega)).filter(b => b && !b.hidden).map(b => [b.textContent, () => b.click()]) : [];
    menu([...phoneOnly, ['settings', () => $('#btn-settings').click()], ['devices and recovery', () => $('#btn-devices').click()], ['activity log', () => $('#btn-activity').click()], ...(M.level === 'server' ? [['new community', () => newCommunityHere()], ['join with an invite password', () => joinCommunity()]] : [])], Math.min(r.left, innerWidth - 220), r.bottom + 6);
  }
  mega.addEventListener('click', e => {
    const j = e.target.closest('[data-join]');
    if (j) { rpc('cancel_join', { id: j.dataset.join }).then(() => tick()).catch(er => toast(er.message)); return; }
    const b = e.target.closest('[data-m]'); if (!b) return;
    const a = b.dataset.m;
    if (a === 'new') newCommunityHere(); else if (a === 'join') joinCommunity();
    else if (a === 'more') moreMenu(b); else if (a === 'jump') palette();
    else if (a === 'back') showHome(M.cid);
    else if (a === 'invite') $('#btn-invite').click(); else if (a === 'room') $('#add-room').click();
    else if (a === 'people') $('#btn-people').click();
    else if (a === 'paint') { if (M.list && canPaint(M.list.item)) paint(M.list.item); }
    else if (a === 'draw') { const x = M.list && M.list.item; if (x && window.__megaWall && __megaWall.drawable(x)) { closePaint(); __megaWall.focus(x, M.list.sq()); __megaWall.drawCard(); drawChip(); } }
  });

  // a community made from here becomes the one the person is looking at: show its rooms once the core lists it
  async function follow(p, before) {
    await p;
    const t0 = Date.now();
    // by what is new in the list, not by S.cid: a refresh that has not listed the community yet can put S.cid back
    for (let i = 0; i < 160; i++) {
      const fresh = comms().find(c => !before.has(c.id));
      if (fresh) { M.followMs = Date.now() - t0; if (M.level !== 'chat') showServer(fresh.id); return; }
      await wait(150);   // a busy machine can take many seconds to list a new community
    }
  }
  const newCommunityHere = () => { const before = new Set(comms().map(c => c.id)); return follow(newCommunity(), before); };
  window.__megaNewCommunity = newCommunityHere;

  function palette() {
    if ($('#mega-pal')) return; closePops();
    const all = [];
    for (const c of comms()) {
      const cx = { id: c.id, kind: 'community', theme: DEF[H(c.id) % DEF.length] };
      all.push({ x: cx, label: c.name, sub: 'community', run: () => showServer(c.id) });
      for (const it of roomItems(c)) {
        const sub = c.name;
        if (it.kind === 'voice') all.push({ x: it, label: it.name, sub, run: () => showServer(c.id, it.key) });
        else if (it.kind === 'admin') all.push({ x: it, label: 'server settings', sub, run: () => { toChat(c.id, null); M.adm = true; showOpts(); drawRail(); } });
        else if (it.kind === 'options') all.push({ x: it, label: 'options', sub, run: () => toChat(c.id, null) });
        else all.push({ x: it, label: it.name, sub: it.locked ? sub + ', waiting for keys' : sub, run: () => { if (it.locked) { showServer(c.id, it.key); toast('This room is still waiting for keys.'); } else toChat(c.id, it.gid); } });
      }
    }
    for (const [label, run] of [['settings', () => $('#btn-settings').click()], ['devices and recovery', () => $('#btn-devices').click()], ['activity log', () => $('#btn-activity').click()], ['new community', () => newCommunityHere()], ['join with an invite password', () => joinCommunity()]]) all.push({ x: { kind: 'action', id: 'a:' + label, theme: ['#efedf2', '#efedf2'] }, label, sub: 'action', run });
    const e = document.createElement('div'); e.id = 'mega-pal'; e.setAttribute('role', 'dialog'); e.setAttribute('aria-label', 'Jump to');
    e.innerHTML = '<input id="m-pq" placeholder="jump to" autocomplete="off" aria-label="Jump to a room"><ul id="m-pl"></ul>'; document.body.append(e); M.pal = e;
    const close = () => { e.remove(); M.pal = null; M.closers = M.closers.filter(f => f !== close); document.removeEventListener('pointerdown', out); };
    M.closers.push(close);
    const out = ev => { if (!e.contains(ev.target)) close(); }; setTimeout(() => document.addEventListener('pointerdown', out), 0);
    let sel = 0, res = all; const q = $('#m-pq', e);
    const draw = () => { $('#m-pl', e).innerHTML = res.slice(0, 9).map((o, i) => `<li class="${i === sel ? 'on' : ''}" data-i="${i}" style="--c:${o.x.kind === 'action' ? '#efedf2' : dotOf(o.x)}"><span class="mpip ${o.x.kind === 'voice' ? 'v' : o.x.kind === 'options' ? 'o' : o.x.kind === 'action' ? 'a' : ''}"></span>${esc(o.label)}<small>${esc(o.sub)}</small></li>`).join(''); };
    const go = o => { close(); if (o) o.run(); };
    q.oninput = () => { const t = q.value.toLowerCase(); res = all.filter(o => (o.label + ' ' + o.sub).toLowerCase().includes(t)); sel = 0; draw(); };
    q.onkeydown = ev => {
      if (ev.key === 'ArrowDown') { sel = Math.min(sel + 1, Math.min(res.length, 9) - 1); draw(); ev.preventDefault(); ev.stopPropagation(); }
      if (ev.key === 'ArrowUp') { sel = Math.max(sel - 1, 0); draw(); ev.preventDefault(); ev.stopPropagation(); }
      if (ev.key === 'Enter') { ev.preventDefault(); ev.stopPropagation(); go(res[sel]); }   // a voice room only gets its card focused
    };
    $('#m-pl', e).onclick = ev => { const li = ev.target.closest('li'); if (li) go(res[+li.dataset.i]); };
    draw(); q.focus();
  }

  // ---------------------------------------------------------------- keys: Esc steps back one level
  function modalOpen() { return !!document.querySelector('#modal-root > .scrim'); }
  function back(e) {
    if (modalOpen() || document.querySelector('.lightbox, .emojis, #call-pop')) return false;   // those answer Esc themselves
    const t = e.target, typing = t && /INPUT|TEXTAREA|SELECT/.test(t.tagName || '');
    if (typing && t.id === 'input' && (S.reply || S.edit || !$('#mention-pop').hidden)) return false;
    if (typing && t.id === 'search' && !$('#search-pop').hidden) return false;
    if (!$('#sw-menu').hidden) return false;
    if (M.pal) { M.pal.querySelector('input').blur(); closePops(); return true; }
    if (PT.open) { closePaint(); return true; }
    if (closePops()) return true;
    if (window.__megaWall && __megaWall.isOn()) { __megaWall.off(); return true; }
    if (S.people) { $('#btn-people').click(); return true; }
    if (M.rail) { closeRail(); return true; }
    if (M.level === 'chat') { if (typing) t.blur(); openFromDots(); return true; }
    if (M.level === 'server' && comms().length > 1) { showHome(M.cid); return true; }
    return true;
  }
  addEventListener('keydown', e => {
    if (!M.started) return;
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); e.stopPropagation(); if (M.pal) closePops(); else if (!modalOpen()) palette(); return; }
    if (e.key === 'Escape') { if (back(e)) { e.preventDefault(); e.stopPropagation(); } return; }
    if (modalOpen() || M.pal) return;
    const typing = /INPUT|SELECT|TEXTAREA/.test(document.activeElement?.tagName || '');
    if (e.altKey && M.level === 'chat' && /Arrow(Up|Down)/.test(e.key)) {
      e.preventDefault(); const t = textRooms(); if (!t.length) return;
      const i = M.opts ? -1 : t.findIndex(x => x.gid === S.gid), n = t[(i + (e.key === 'ArrowDown' ? 1 : -1) + t.length) % t.length];
      switchChat(n); return;
    }
    if (root.classList.contains('mg-drawing-card')) return;   // the pen has the card: no keyboard scrolling
    if (typing || !M.list || M.closers.length || M.busy) return;
    if (e.key === 'ArrowDown' || e.key === 'j') { e.preventDefault(); M.list.move(1); }
    else if (e.key === 'ArrowUp' || e.key === 'k') { e.preventDefault(); M.list.move(-1); }
    else if (e.key === 'Enter' && (document.activeElement === document.body || document.activeElement === M.list.el)) {
      e.preventDefault(); const x = M.list.item;
      if (x.kind === 'voice') { if (!x.locked && !x.here) joinVoice(x, 'join'); } else enterCurrent();   // Enter on a voice card is an explicit join
    }
  }, true);
  function enterCurrent() { const x = M.list.item; if (!x) return; if (x.kind === 'news') { __megaNew.openFirst(x); return; } if (M.level === 'home') enterCommunity(x); else enterRoom(x); }
  // phones: swipe in from the left edge to pop the dots out
  let tx = null;
  addEventListener('touchstart', e => { tx = M.level === 'chat' && e.touches[0].clientX < 30 ? e.touches[0].clientX : null; if (M.rail && !e.target.closest('#mega-rail')) closeRail(); }, { passive: true });
  addEventListener('touchmove', e => { if (tx != null && e.touches[0].clientX - tx > 34) { openRail(); tx = null; } }, { passive: true });

  // ---------------------------------------------------------------- the living room (phase 2)
  // Everything below works on the real stream (#stream, built by renderStream in app.js) without changing what it shows:
  // the backdrop is a canvas behind it, sizes/focus are styles on the message rows, arrivals and reactions are detected by
  // comparing what renderStream painted this time with what it painted last time. Message text is untrusted: it only ever
  // reaches the page through textContent here.
  const ST = { decodes: 0, ripples: 0, throws: 0, lands: 0, flights: 0, bdDraws: 0 };
  M.stats = ST;
  const still = () => calm() || root.classList.contains('fx-lite');
  const roomOn = () => root.classList.contains('mg-room');
  const lift = (c, r) => { let i = 0; while (contrast(c, '#050407') < r && i++ < 16) c = mix(c, '#ffffff', .12); return c; };
  const kidsIn = (st, lo, hi) => {   // the stream's children that touch [lo, hi]; they are in order, so find the first by bisection
    const k = st.children, n = k.length; let a = 0, b = n;
    while (a < b) { const m = (a + b) >> 1, e = k[m]; if (e.offsetTop + e.offsetHeight < lo) a = m + 1; else b = m; }
    const out = []; for (let i = a; i < n; i++) { const e = k[i]; if (e.offsetTop > hi) break; out.push(e); }
    return out;
  };

  // ---- the backdrop: the room's two colours as ascii, flowing around everything on the page
  const HEAT_FLOOR = .12;   // a quiet room still breathes: this much heat is the least it ever settles to
  const HEAT = new Map();   // gid -> heat, so a room you come back to is as lively as you left it
  let BD = null, mainEl = null;
  // The owner's backdrop strength scales how bright the glyphs are (100 is today's look exactly), how dense they get above that, and how much
  // a ripple or a busy room stirs it. At 0 nothing is drawn at all.
  const lvOf = a => (a.dim == null ? 100 : a.dim) / 100;
  function bdColours(a) {
    const v = '#050407', b = a.f, h = a.h, lv = lvOf(a);
    a.at = atlasOf(mix(v, b, Math.min(1, .2 * lv)), mix(v, h, Math.min(1, .3 * lv)), a.sw, a.sh, a.fs); a.atB = atlasOf(mix(v, b, Math.min(1, .5 * lv)), mix(v, lift(h, 3.4), Math.min(1, lv)), a.sw, a.sh, a.fs);
  }
  /** Take the owner's chat pattern and strength for this room (live: a change from the owner fades in). */
  function bdLook(it) {
    const a = BD; if (!a || !it || a.gid !== it.gid) return;
    const fxn = it.chatFx ?? it.fx, d = it.dim ?? 100;
    a.fxAuto = it.fxAuto;
    if (a.fx !== fxn) { a.fx = fxn; fade(a.c); }
    if (a.dim !== d) { a.dim = d; if (d === 0) a.g.clearRect(0, 0, a.c.width, a.c.height); bdColours(a); a.last = 0; if (window.__megaMusic) __megaMusic.sync(); }
  }
  function bdSize() {
    const a = BD; if (!a) return; const box = a.c.parentElement, w0 = box.clientWidth, h0 = box.clientHeight; if (!w0 || !h0) return;
    const dpr = Math.min(devicePixelRatio || 1, 2), small = w0 < 560; a.cw = small ? 8 : 10; a.chh = small ? 13 : 17; a.fs = Math.round((small ? 11 : 14) * dpr); a.sw = Math.round(a.cw * dpr); a.sh = Math.round(a.chh * dpr);
    a.cols = Math.ceil(w0 / a.cw); a.rows = Math.ceil(h0 / a.chh); a.c.width = a.cols * a.sw; a.c.height = a.rows * a.sh;
    a.mask = new Uint8Array(a.cols * a.rows); a.col = colFor(a.cols, H(a.gid)); bdColours(a); dirty();
  }
  // every rectangle the words occupy right now (the backdrop and the shared wall both paint around them)
  function eachRect(hit) {
    const rg = document.createRange(), text = e => { rg.selectNodeContents(e); for (const r of rg.getClientRects()) hit(r); }, box = e => hit(e.getBoundingClientRect());
    const pad = e => { const r = e.getBoundingClientRect(); hit({ left: r.left - 6, right: r.right + 6, top: r.top - 6, bottom: r.bottom + 6, width: r.width, height: r.height }); };
    const st = $('#stream');
    for (const el of kidsIn(st, st.scrollTop - 60, st.scrollTop + st.clientHeight + 60)) {
      if (el.classList.contains('msg')) {
        // only the words and pictures themselves: a row's cells are wider than what is in them, and clearing those left black boxes in empty space
        $$('.body,.who .name,.quote,time.when', el).forEach(text);
        $$('.who > :not(.name):not(.sr),.file,.react > *,.imgslot,.mediaslot', el).forEach(box);
      }
      else if (el.classList.contains('day')) $$('span,button', el).forEach(box);
      else if (el.classList.contains('sys')) text(el);
      else box(el);
    }
    $$('#chan-title,#chan-sub', mainEl).forEach(text);
    $$('#head .head-actions > *,#head #btn-rooms', mainEl).forEach(pad);
    $$('.slip,#mega-note,#typing,#mega-typing .mmascot,#mega-typing .mtwho:not(:empty),#mega-typing .mripple:not(:empty),#hint:not([hidden]),#jump:not([hidden]),#replybar:not([hidden]),#banner:not([hidden]),#upd:not([hidden])', mainEl).forEach(box);
  }
  function bdMask() {
    const a = BD; if (!a || !a.mask) return;
    const b = a.c.getBoundingClientRect(); if (!b.width) return; a.mask.fill(0);
    const kw = b.width / a.cols, kh = b.height / a.rows;
    const hit = r => {
      if (!r.width && !r.height) return;
      const x0 = Math.max(0, Math.floor((r.left - b.left) / kw) - 1), x1 = Math.min(a.cols, Math.ceil((r.right - b.left) / kw) + 1), y0 = Math.max(0, Math.floor((r.top - b.top) / kh)), y1 = Math.min(a.rows, Math.ceil((r.bottom - b.top) / kh));
      for (let y = y0; y < y1; y++) a.mask.fill(1, y * a.cols + x0, y * a.cols + Math.max(x0, x1));
    };
    eachRect(hit);
    if (window.__megaWall) { __megaWall.fillMask(a.mask, a.cols, a.rows, b); __megaWall.remask(); }   // the drawing is an obstacle too, and the drawing avoids the words
  }
  function bdDraw() {
    const a = BD; if (!a || !a.mask || !a.at) return;
    const { g, cols, rows, mask, sw, sh } = a; g.clearRect(0, 0, a.c.width, a.c.height);
    if (a.dim === 0) return;
    ST.bdDraws++;
    const gate = (.5 - .26 * a.heat) / Math.max(1, lvOf(a)), field = FX[a.fx], rips = a.rip, nr = rips.length;
    for (let y = 0, i = 0; y < rows; y++) for (let x = 0; x < cols; x++, i++) {
      if (mask[i]) continue;
      let v = field(x, y, a.t, a), lit = 0;
      for (let k = 0; k < nr; k++) { const q = rips[k], d = Math.hypot(x - q.x, (y - q.y) * 1.7) - q.r; if (d * d < q.band * 5) { const e = Math.exp(-d * d / q.band) * q.s; v += e; if (e > lit) lit = e; } }
      if (v < gate) continue;
      g.drawImage(lit > .2 ? a.atB : a.at, Math.min(9, v * 10 | 0) * sw, (v < .64 ? 0 : v < .86 ? 1 : 2) * sh, sw, sh, x * sw, y * sh, sw, sh);
    }
  }
  function bdLoop(now) {
    const a = BD; if (!a) return; a.raf = requestAnimationFrame(bdLoop);
    if (document.hidden || still() || !a.mask || a.dim === 0) return;
    if (now - a.last < (a.rip.length ? 40 : 100)) return;   // 25 fps while a ripple runs, a steady 10 otherwise: a quiet room still drifts
    const dt = Math.min(.25, (now - a.last) / 1000); a.last = now;
    a.heat = Math.max(HEAT_FLOOR, a.heat * Math.pow(.5, dt / 18)); a.t += dt * (.1 + a.heat * 1.5);
    if ((a.nextRip -= dt) <= 0) { a.nextRip = 14 + Math.random() * 14; idleRipple(a); }   // now and then a gentle ring from somewhere in the room
    a.rip.forEach(q => { q.r += q.v * dt; q.s *= Math.pow(q.fade, dt); }); a.rip = a.rip.filter(q => q.s > .04 && q.r < a.cols * 1.3);
    bdDraw();
  }
  function bdStart(item) {
    bdStop();
    const c = document.createElement('canvas'); c.id = 'mega-bd'; c.setAttribute('aria-hidden', 'true'); mainEl.prepend(c);
    const a = BD = { c, g: c.getContext('2d'), gid: item.gid, fx: item.chatFx ?? item.fx, fxAuto: item.fxAuto, dim: item.dim ?? 100, f: fieldOf(item), h: hiOf(item), heat: Math.max(HEAT_FLOOR, HEAT.get(item.gid) ?? 0), t: H(item.gid) % 90, rip: [], last: performance.now(), raf: 0, nextRip: 5 + Math.random() * 6 };
    a.ro = new ResizeObserver(() => { clearTimeout(a.rz); a.rz = setTimeout(() => { if (BD === a) { bdSize(); focusFrame(); } }, 120); }); a.ro.observe(mainEl);
    bdSize(); if (!still()) a.raf = requestAnimationFrame(bdLoop);
  }
  function bdStop() {
    const a = BD; if (!a) return; BD = null; HEAT.set(a.gid, a.heat);
    cancelAnimationFrame(a.raf); clearTimeout(a.rz); a.ro.disconnect(); a.c.remove();
  }
  const bdHeat = n => { if (BD) BD.heat = Math.min(1, BD.heat + n * lvOf(BD)); };
  function idleRipple(a) {
    if (!a.mask || a.dim === 0 || !a.cols) return;
    if (a.rip.length > 6) a.rip.shift();
    a.rip.push({ x: a.cols * (.15 + .7 * Math.random()), y: a.rows * (.2 + .6 * Math.random()), r: 0, s: .5 * lvOf(a), v: 26, band: 8, fade: .3 });
  }
  function ripple(cx, cy, s, ring) {
    const a = BD; if (!a || !a.mask || still() || a.dim === 0) return; const b = a.c.getBoundingClientRect(); if (!b.width) return;
    if (a.rip.length > 6) a.rip.shift();
    a.rip.push({ x: (cx - b.left) / (b.width / a.cols), y: (cy - b.top) / (b.height / a.rows), r: 0, s: s * lvOf(a), v: ring ? 20 : 44, band: ring ? 7 : 4, fade: ring ? .3 : .12 });
    ST.ripples++; a.last = 0;
  }

  // ---- when a room is on screen: its colours, the dark look, the backdrop
  function syncRoom() {
    if (!mainEl) mainEl = $('#main');
    root.classList.toggle('mg-opts', !!(M.started && M.level === 'chat' && M.opts));
    let item = null;
    if (M.started && M.level === 'chat' && !M.opts && S.gid) { const c = comm(M.cid); item = c && roomItems(c).find(x => x.gid === S.gid && x.kind === 'text'); }
    root.classList.toggle('mg-room', !!item);
    if (window.__megaNew) __megaNew.room(item ? item.gid : null);   // density for this room, and its header button
    if (!item) { bdStop(); if (window.__megaMusic) __megaMusic.sync(); if (window.__megaWall) __megaWall.close(); typingDraw([]); focusFrame(); return; }
    if (BD && BD.gid === item.gid) return;
    const hi = lift(hiOf(item), 4.8);
    root.style.setProperty('--mg-c', dotOf(item)); root.style.setProperty('--mg-hi', hi); root.style.setProperty('--mg-hi-ink', bestInk(hi));
    typingDraw([]); bdStart(item); if (window.__megaWall) __megaWall.open(item); if (window.__megaMusic) __megaMusic.sync();
  }

  // ---- size by length, and the centre focus while reading back
  function sizeTier(x) {
    if (!x || x.kind !== 'text' || x.deleted) return 0;
    const n = [...String(x.text || '')].length; if (!n) return 0;
    const t = n <= 4 ? 3 : n <= 12 ? 2 : n <= 18 ? 1 : 0;
    return Math.min(3, t + (!x.mine && mentionsMe(x.text) ? 1 : 0));
  }
  let touched = new Set();
  const atEnd = st => st.scrollHeight - st.scrollTop - st.clientHeight < 80;
  function focusFrame() {
    const st = $('#stream'); if (!st) return;
    if (!roomOn() || still() || atEnd(st)) { touched.forEach(e => { e.style.transform = ''; e.style.opacity = ''; }); touched.clear(); return; }
    const hh = st.clientHeight, top = st.scrollTop, zt = top + hh * .3, zb = top + hh * .7, reach = hh * .3;
    const sel = getSelection(), hasSel = !!sel && !sel.isCollapsed && sel.rangeCount > 0;
    const seen = new Set(), ops = [];
    for (const el of kidsIn(st, top - 60, top + hh + 60)) {
      if (!el.classList.contains('msg')) continue;
      const a = el.offsetTop, b = a + el.offsetHeight;
      let d = Math.min(1, Math.max(0, zt - b, a - zb) / reach);
      if (d && (hasSel && sel.containsNode(el, true))) d = 0;
      ops.push([el, d]); seen.add(el);
    }
    for (const [el, d] of ops) { el.style.transform = d ? `scale(${(1 - .2 * d).toFixed(3)})` : ''; el.style.opacity = d ? (1 - .55 * d).toFixed(3) : ''; if (d) touched.add(el); else touched.delete(el); }
    touched.forEach(e => { if (!seen.has(e)) { e.style.transform = ''; e.style.opacity = ''; touched.delete(e); } });   // scrolled far away: back to normal
  }
  let flushRaf = 0, flushT = 0;
  function dirty(trail) {
    if (!flushRaf) flushRaf = requestAnimationFrame(() => { flushRaf = 0; if (!roomOn()) return; focusFrame(); if (BD) { if (BD.dim === 0) { if (window.__megaWall) __megaWall.remask(); } else { bdMask(); if (still() || document.hidden) bdDraw(); } } });
    if (trail !== false) { clearTimeout(flushT); flushT = setTimeout(() => dirty(false), 300); }   // the rows ease into their new size, so look again once they have
  }
  document.addEventListener('selectionchange', () => { if (roomOn() && !atEnd($('#stream'))) dirty(false); });
  document.addEventListener('input', e => { if (e.target && e.target.id === 'input') dirty(); }, true);
  addEventListener('resize', () => dirty());
  $('#stream').addEventListener('scroll', () => {
    dirty();
    // something new arrived while reading back: the existing "back to latest" button must be there (app.js hides it under 260px)
    if (S.below > 0 && !S.pinned) $('#jump').hidden = false;
  }, { passive: true });

  // ---- ascii arrival: a new message resolves out of scrambled glyphs
  const SCR = ".:;-/!?'()";
  function decode(body) {
    const chars = [...body.textContent]; if (!chars.length || chars.length > 400) return;
    ST.decodes++;
    const ov = document.createElement('span'), real = document.createElement('span'), noise = document.createElement('span');
    ov.className = 'mdec'; real.className = 'mreal'; ov.setAttribute('aria-hidden', 'true'); noise.className = 'mdg'; ov.append(real, noise);
    body.classList.add('mdecoding'); body.append(ov);
    const t0 = performance.now(), D = 340;
    const done = () => { ov.remove(); body.classList.remove('mdecoding'); };
    const step = now => {
      if (!body.isConnected || document.hidden) { done(); return; }
      const k = Math.min(1, (now - t0) / D); if (k >= 1) { done(); return; }
      const n = Math.floor(k * chars.length); let g = '';
      for (let i = n; i < chars.length; i++) g += /\s/.test(chars[i]) ? chars[i] : SCR[Math.random() * SCR.length | 0];
      real.textContent = chars.slice(0, n).join(''); noise.textContent = g;   // text, never markup
      requestAnimationFrame(step);
    };
    real.textContent = ''; noise.textContent = chars.map(c => /\s/.test(c) ? c : SCR[Math.random() * SCR.length | 0]).join('');
    requestAnimationFrame(step);
  }

  // ---- what renderStream just painted, against what it painted last time
  const RM = { gid: null, ids: new Set(), rx: new Map(), arr: [] };
  let lastMine = null;
  document.addEventListener('click', e => {
    const b = e.target.closest && (e.target.closest('.emojis button') || e.target.closest('.react[data-e]'));
    if (b) { const r = b.getBoundingClientRect(); lastMine = { x: r.left + r.width / 2, y: r.top + r.height / 2, t: Date.now() }; }
  }, true);
  const rxMap = m => { const o = new Map(); for (const x of m.messages) if (x.reactions && x.reactions.length) o.set(String(x.id), new Map(x.reactions.map(r => [r.emoji, { n: r.count, mine: !!r.mine }]))); return o; };
  function burst(x, y, c) {
    if (!fxl.isConnected) return;
    const n = root.classList.contains('fx-lite') ? 10 : 24;
    for (let i = 0; i < n; i++) {
      const e = document.createElement('span'); e.className = 'mburst'; e.textContent = SCR[Math.random() * SCR.length | 0]; e.style.color = c; fxl.append(e);
      const ang = Math.random() * 6.283, d = 30 + Math.random() * 100, dx = x + Math.cos(ang) * d, dy = y + Math.sin(ang) * d * .7;
      e.animate([{ transform: `translate(${x}px,${y}px) scale(1.3)`, opacity: 1 }, { transform: `translate(${dx}px,${dy}px) scale(.8)`, opacity: 0 }], { duration: 380 + Math.random() * 300, easing: 'cubic-bezier(.1,.8,.3,1)' }).onfinish = () => e.remove();
    }
  }
  const countNow = (id, em) => { const x = S.msgs && S.msgs.messages.find(k => String(k.id) === id), r = x && x.reactions.find(k => k.emoji === em); return r ? r.count : 0; };
  const pillOf = (id, em) => $$('.react[data-e]', $('#stream')).find(b => b.dataset.id === id && b.dataset.e === em);
  // a reaction flies to its message and bursts there; the number on the pill only changes when it lands
  function throwReaction(id, em, prevN, mine) {
    const pill = pillOf(id, em), st = $('#stream'); if (!pill || !roomOn() || still() || document.hidden) return;
    const t = pill.getBoundingClientRect(), sr = st.getBoundingClientRect();
    if (t.bottom < sr.top || t.top > sr.bottom) return;
    ST.flights++;
    if (prevN > 0) $('.n', pill).textContent = String(prevN); else pill.classList.add('rhold');
    const cr = $('#composer').getBoundingClientRect();
    let x0 = sr.right - 30, y0 = sr.top + sr.height * (.2 + Math.random() * .5);
    if (mine) { if (lastMine && Date.now() - lastMine.t < 8000) { x0 = lastMine.x; y0 = lastMine.y; } else { x0 = cr.left + cr.width / 2; y0 = cr.top; } }
    const x1 = t.left + t.width / 2, y1 = t.top + t.height / 2, hi = getComputedStyle(mainEl).getPropertyValue('--mg-hi').trim() || '#efedf2';
    const f = document.createElement('span'); f.className = 'mthrow'; f.textContent = em; f.style.color = hi; fxl.append(f);
    const finish = () => {
      const p = pillOf(id, em); if (p) { p.classList.remove('rhold'); const n = $('.n', p); if (n) n.textContent = String(countNow(id, em)); p.classList.add('pop'); setTimeout(() => p.classList.remove('pop'), 400); }
    };
    const a = f.animate([{ transform: `translate(${x0}px,${y0}px) scale(.7) rotate(-30deg)` }, { transform: `translate(${(x0 + x1) / 2}px,${Math.max(sr.top + 40, Math.min(y0, y1) - 90)}px) scale(2.4) rotate(190deg)` }, { transform: `translate(${x1}px,${y1}px) scale(1) rotate(360deg)` }], { duration: 560, easing: 'ease-in' });
    a.onfinish = () => { f.remove(); burst(x1, y1, hi); ripple(x1, y1, 1.25, true); bdHeat(.12); ST.throws++; finish(); };
    a.oncancel = () => { f.remove(); finish(); };
    setTimeout(finish, 1500);   // however it ended, the pill shows the true count
  }

  // ---- the mascot peeking over the composer
  const BOT = ['......#.....', '......#.....', '...######...', '..########..', '.##########.', '.##e####e##.', '.##e####e##.', '.##########.', '.##########.', '..########..'], HA = ['...##...##..', '...##.......'], HB = ['...##...##..', '........##..'];
  const px = (rows, y0, kind) => rows.flatMap((r, y) => [...r].map((c, x) => c === kind ? `<rect x="${x}" y="${y + y0}" width="1.02" height="1.02"/>` : '')).join('');
  const MASCOT = `<svg viewBox="0 0 12 12" shape-rendering="crispEdges" aria-hidden="true"><g fill="currentColor">${px(BOT, 0, '#')}${px(BOT, 0, 'e')}</g><g class="eyes" fill="#101116">${px(BOT, 0, 'e')}</g><g class="ha" fill="currentColor">${px(HA, 10, '#')}</g><g class="hb" fill="currentColor">${px(HB, 10, '#')}</g></svg>`;
  let rippleT = 0, rippleP = 0, ripW = 0;
  function typingBox() {
    let t = $('#mega-typing'); if (t) return t;
    const slip = $('#composer .slip'); if (!slip) return null;
    t = document.createElement('div'); t.id = 'mega-typing'; t.setAttribute('aria-hidden', 'true'); t.innerHTML = '<div class="mtin"><span class="mmascots"></span><span class="mtwho"></span><span class="mripple"></span></div>'; slip.before(t); return t;
  }
  function typingDraw(names) {
    const t = typingBox(); if (!t) return;
    const box = $('.mmascots', t), rip = $('.mripple', t), want = roomOn() ? [...new Set(names)].slice(0, 6) : [];
    const key = want.join('\n'); if (key === typingDraw.key) return; typingDraw.key = key;
    const n = want.length, shown = want.slice(0, 3);
    $('.mtwho', t).textContent = !n ? '' : n === 1 ? want[0] + ' is typing' : (n <= 3 ? shown.slice(0, -1).join(', ') + ' and ' + shown[n - 1] : shown.join(', ') + ' and ' + (n - 3) + ' more') + ' are typing';
    $$('.mmascot', box).forEach(e => { if (!want.includes(e.dataset.who) && !e.dataset.out) { e.dataset.out = '1'; if (still()) e.remove(); else e.animate([{ transform: 'none' }, { transform: 'translateY(26px)' }], { duration: 200, easing: 'ease-in', fill: 'forwards' }).onfinish = () => e.remove(); } });
    for (const w of want) {
      if ($$('.mmascot', box).some(e => e.dataset.who === w && !e.dataset.out)) continue;
      const e = document.createElement('span'); e.className = 'mmascot'; e.dataset.who = w; e.style.color = faceColor(w); e.innerHTML = MASCOT; box.append(e);   // MASCOT is a constant; the name only goes into a data attribute
      if (!still()) e.animate([{ transform: 'translateY(30px)' }, { transform: 'none' }], { duration: 380, easing: 'cubic-bezier(.2,1.5,.3,1)' });
    }
    clearInterval(rippleT); rip.textContent = '';
    if (want.length && !still()) {
      ripW = 0;
      rippleT = setInterval(() => {
        if (document.hidden) return;
        if (!ripW) ripW = rip.clientWidth || 60;
        const n = Math.ceil(ripW / 7.4), G = ' .:-=:. '; let o = ''; for (let i = 0; i < n; i++) o += G[Math.floor((Math.sin(i * .34 - rippleP) + 1) * 3.99)];
        rip.textContent = o; rippleP += .42;
      }, 66);
    }
    if (want.length) bdHeat(.03);
    dirty(false);
  }

  const baseRender = window.renderStream;
  window.renderStream = function () {
    const first = !!S.first, gid = S.gid, was = S.painted;
    const r = baseRender.apply(this, arguments);
    try { afterPaint(first, gid, was); } catch (er) { console.error(er); }
    return r;
  };
  function afterPaint(first, gid, prevPainted) {
    const m = S.msgs; if (!m || S.painted === prevPainted) return;
    const st = $('#stream'), on = roomOn() && !document.hidden;
    const byId = new Map(m.messages.map(x => [String(x.id), x]));
    const els = $$('.msg[data-id]', st), ids = new Set(els.map(e => e.dataset.id));
    // sizes: set on every row the paint touched (a repainted row is a new element)
    for (const el of els) { const t = sizeTier(byId.get(el.dataset.id)), v = t ? String(t) : ''; if ((el.dataset.mz || '') !== v) { if (v) el.dataset.mz = v; else delete el.dataset.mz; } }
    const live = !first && RM.gid === gid && gid != null;
    const rx = rxMap(m), seen = RM.ids, prevRx = RM.rx;
    if (live && on && !still()) {
      const nowS = Date.now() / 1000, fresh = els.filter(e => !seen.has(e.dataset.id));
      const burstIn = fresh.length > 4;   // a catch-up, not conversation
      const sr = st.getBoundingClientRect();
      for (const el of burstIn ? [] : fresh) {
        const x = byId.get(el.dataset.id);
        if (!x) continue;                                       // a row still sending is handled below
        if (nowS - x.ts > 60 || x.deleted) continue;            // old history paged in
        if (x.mine) continue;                             // what you just wrote already rippled as "sending"
        RM.arr = RM.arr.filter(t => Date.now() - t < 1500); RM.arr.push(Date.now());
        const body = $('.body', el) || $('.txt', el);
        if (body) {
          const r = body.getBoundingClientRect();
          if (r.bottom > sr.top && r.top < sr.bottom) ripple(r.left + Math.min(r.width, 300) / 2, r.top + r.height / 2, mentionsMe(x.text) ? 1.1 : .72);
        }
        bdHeat(.24);
        if (x.kind === 'text' && x.text) {
          if (RM.arr.length <= 2 && $('.body', el)) decode($('.body', el));
          if (mentionsMe(x.text)) { el.classList.add('mland'); ST.lands++; setTimeout(() => el.classList.remove('mland'), 700); }
        }
      }
      // reactions that just appeared on messages that were already here
      let flights = 0;
      for (const [id, mm] of rx) {
        const prev = prevRx.get(id); if (!seen.has(id)) continue;
        for (const [em, r] of mm) {
          const p = prev && prev.get(em), pn = p ? p.n : 0;
          if (r.n > pn && flights < 3) { flights++; throwReaction(id, em, pn, r.mine && !(p && p.mine)); }
        }
      }
    }
    // a sending row ripples once
    if (live && on && !still()) for (const el of els) if (!seen.has(el.dataset.id) && el.classList.contains('sending')) { const b = $('.body', el); if (b) { const r = b.getBoundingClientRect(); ripple(r.left + Math.min(r.width, 300) / 2, r.top + r.height / 2, .5); bdHeat(.1); } }
    RM.gid = gid; RM.ids = ids; RM.rx = rx;
    typingDraw(m.typing || []);
    if (S.below > 0 && !S.pinned) $('#jump').hidden = false;
    dirty();
  }

  // ---------------------------------------------------------------- following the snapshot
  function render() {
    if (!S.snap) return;
    syncColours();
    if (!M.started) { start(); return; }
    if (M.busy) { M.dirty = true; return; }
    const cs = comms();
    if (M.level === 'home') {
      if (cs.length === 1) { showServer(cs[0].id, S.gid); return; }
      const next = homeItems();
      if (M.list && !M.list.refresh(next)) {
        // a card appeared or went (the "what's new" card): never while a finger or wheel is on the list; the focused card stays where it is
        if (window.__megaNew && __megaNew.hold()) { setTimeout(render, 450); return; }
        showHome(M.list.item && M.list.item.cid, M.list.item && M.list.item.key); if (window.__megaNew) __megaNew.still();
      }
      drawBar(); drawNotes(); return;
    }
    const c = comm(M.cid);
    if (!c) { M.cid = null; if (cs.length) goHome(); else showHome(); return; }
    if (M.level === 'server') {
      const next = roomItems(c);
      if (M.list && !M.list.refresh(next)) showServer(M.cid, M.list.item && M.list.item.key);
      drawBar(); drawNotes(); loadPreviews(c); return;
    }
    const sig = railSig(c);
    if (sig !== M.sig) { if (M.rail) { M.sig = sig; } else drawRail(); }
    if (M.adm && window.__megaAdmin) __megaAdmin.refresh();   // server settings follow the snapshot; the options room keeps its own state
  }
  function start() {
    M.started = true;
    // mega has one look: a theme chosen in the old app no longer applies.
    if (pref.get('theme')) { pref.set('theme', ''); applyPrefs(); }
    const cs = comms();
    if (cs.length === 1) showServer(cs[0].id); else showHome(store.get('last', null), 'news');
    document.fonts.load('500 14px "IBM Plex Mono"').catch(() => { }).then(() => { fontKey = 1; ACTIVE.forEach(a => { a.at = atlasOf(fieldOf(a.x), hiOf(a.x), a.sw, a.sh, a.fs); asciiDraw(a, clock); }); });
    document.fonts.ready.then(remaskAll);
    requestAnimationFrame(asciiTick);
  }
  M.typingDraw = typingDraw;
  M.eachRect = eachRect;
  window.__megaRender = render;
  window.__megaIdle = () => M.started && (M.level !== 'chat' || M.opts);
  // a room was opened by something other than mega (a search hit, the old rail's handlers)
  window.__megaOpened = gid => {
    if (M.opening || !M.started) return;
    const c = comms().find(o => o.channels.some(x => x.gid === gid)); if (!c) return;
    if (M.level === 'chat' && M.cid === c.id) { M.opts = false; M.adm = false; showOpts(); drawRail(); return; }
    toChatOpen(c.id);
  };
  M.bdApi = { bd: () => BD, heat: bdHeat, ripple, still };   // for mega-music.js
  M.eachRect = eachRect; M.relook = () => dirty(false);
  // what the wall module needs from the cards: ink for the marks, a repaint of the cards that show a given wall, the draw chip
  M.wallInk = x => { const f = fieldOf(x), hi = hiOf(x), ink = bestInk(f), alt = ink === '#0c0d12' ? '#ffffff' : '#0c0d12'; return [ink, contrast(hi, f) >= 2.4 ? hi : alt]; };
  const rwKeys = new Set(); let rwRaf = 0;
  M.rewall = key => {
    rwKeys.add(key); if (rwRaf) return;
    rwRaf = requestAnimationFrame(() => { rwRaf = 0; const ks = new Set(rwKeys); rwKeys.clear(); ACTIVE.forEach(a => { if (window.__megaWall && ks.has(__megaWall.keyOf(a.x))) { remask(a); asciiDraw(a, clock); } }); });
  };
  M.attachWalls = () => ACTIVE.forEach(a => { __megaWall.cardOn(a); remask(a); asciiDraw(a, clock); });
  M.drawChip = drawChip; M.fx = FX; M.fxNames = FXNAMES;
  M.bdInfo = () => (BD ? { gid: BD.gid, fx: BD.fx, fxAuto: BD.fxAuto, dim: BD.dim, heat: BD.heat, at: H(BD.at.toDataURL()), draws: ST.bdDraws } : null);
  window.__megaTheme = { colours(gid) { const c = comm(M.cid), x = c && roomItems(c).find(i => i.gid === gid && i.kind === 'text'); return x ? [lift(fieldOf(x), 5), lift(hiOf(x), 6)] : null; } };
  M.api = { faceColor, comms, comm, roomItems, dotOf, hiOf, fieldOf, lift, mix, contrast, bestInk, cardVars, ti, letters, toChat, openRail, themeOf, dirty, DEF };
  M.admExit = () => { M.adm = false; showOpts(); drawRail(); };   // mega-admin.js: no longer an admin, so back to the plain options room
  M.prev = PREV; M.cp = CP; M.shown = SHOWN; M.canPaint = canPaint;
  M.callTheme = gid => { const c = comms().find(x => x.channels.some(ch => ch.gid === gid)); return c ? targetOf(c.id, gid) : null; };
  M.inkOn = hex => { const n = parseInt(hex.slice(1), 16), f = v => { v /= 255; return v <= .03928 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4; }, l = .2126 * f(n >> 16) + .7152 * f((n >> 8) & 255) + .0722 * f(n & 255); return (l + .05) * (l + .05) > .0525 * 1.05 ? '#050407' : '#efedf2'; };
  window.__mega = M;
  if (S.snap && !$('#app').hidden) render();   // the first snapshot may have arrived while this file was still loading
})();
