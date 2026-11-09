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
  ];
  const colFor = (n, seed) => Array.from({ length: Math.max(1, n) }, (_, i) => ({ v: .6 + nh(i, seed) * 1.5, o: nh(seed, i) * 60, l: 16 + nh(i + 9, seed) * 30 }));
  const MONO = '"IBM Plex Mono", ui-monospace, monospace';
  let fontKey = 0;
  const SNAP = new Map();
  function snapOf(x) {
    const f = fieldOf(x), hi = hiOf(x), key = x.id + f + hi + fontKey; if (SNAP.has(key)) return SNAP.get(key);
    const c = document.createElement('canvas'); c.width = 240; c.height = 96; const g = c.getContext('2d'); g.font = '500 11px ' + MONO; g.textAlign = 'center'; g.textBaseline = 'middle';
    const a = { cols: 34, rows: 8, col: colFor(34, H(x.id)) };
    for (let y = 0; y < 8; y++) for (let k = 0; k < 34; k++) { if (y > 3 && k < 27) continue; const v = FX[x.fx](k, y, 7.3 + H(x.id) % 40, a); if (v < .2) continue; g.fillStyle = mix(f, hi, v < .64 ? .3 : v < .85 ? .6 : 1); g.fillText(RAMP[Math.min(9, v * 10 | 0)], k * 7 + 3.5, y * 12 + 6); }
    const u = `url(${c.toDataURL('image/png')}) center/cover, ${f}`; SNAP.set(key, u); return u;
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
  const ACTIVE = new Set(); let clock = 0, energy = 0, lastT = 0, tickN = 0;
  function asciiOn(sq, x) {
    if (!sq.offsetWidth) return null;
    const c = document.createElement('canvas'); c.className = 'masc'; c.setAttribute('aria-hidden', 'true'); sq.prepend(c);
    const dpr = Math.min(devicePixelRatio || 1, 2), small = sq.offsetWidth < 560, cw = small ? 8 : 10, ch = small ? 13 : 17, fs = Math.round((small ? 11 : 14) * dpr), sw = Math.round(cw * dpr), sh = Math.round(ch * dpr);
    const cols = Math.ceil(sq.offsetWidth / cw), rows = Math.ceil(sq.offsetHeight / ch); c.width = cols * sw; c.height = rows * sh; c.style.width = cols * cw + 'px'; c.style.height = rows * ch + 'px';
    const live = x.kind === 'voice' && x.people.length, frozen = x.locked || (x.kind === 'voice' && !x.people.length);
    const a = { c, ctx: c.getContext('2d'), sq, x, cols, rows, cw, ch, sw, sh, fs, mask: new Uint8Array(cols * rows), field: FX[x.fx], hiT: frozen ? 2 : live ? .76 : .9 - .22 * Math.min(1, (x.act || 0) / 12), frozen, col: colFor(cols, H(x.id)), focus: false, mx: -999, my: -999, bloom: 0 };
    a.at = atlasOf(fieldOf(x), hiOf(x), sw, sh, fs); ACTIVE.add(a); remask(a); asciiDraw(a, clock); return a;
  }
  function asciiOff(a) { if (!a) return; ACTIVE.delete(a); a.c.remove(); }
  // the ascii flows around the words: every text box on the card is cut out of the pattern
  function remask(a) {
    const sr = a.sq.getBoundingClientRect(); if (!sr.width) return;
    const k = a.sq.offsetWidth / sr.width, settled = a.sq.closest('.mslot')?.classList.contains('settled'); a.mask.fill(0);
    const hit = r => { const x0 = Math.max(0, Math.floor((r.left - sr.left) * k / a.cw) - 1), x1 = Math.min(a.cols, Math.ceil((r.right - sr.left) * k / a.cw) + 1), y0 = Math.max(0, Math.floor((r.top - sr.top) * k / a.ch)), y1 = Math.min(a.rows, Math.ceil((r.bottom - sr.top) * k / a.ch)); for (let y = y0; y < y1; y++) a.mask.fill(1, y * a.cols + x0, y * a.cols + Math.max(x0, x1)); };
    const rg = document.createRange();
    $$('.mtop>span:not(.mat),.mti,.mnu' + (settled ? ',.mpreview' : ''), a.sq).forEach(e => { rg.selectNodeContents(e); [...rg.getClientRects()].forEach(hit); });
    $$('.mat' + (settled ? ',.mwho>span,.mvact button,.mfaces-row' : ''), a.sq).forEach(e => hit(e.getBoundingClientRect()));
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
  const M = { started: false, level: 'home', cid: null, list: null, rail: false, opts: false, busy: false, dirty: false, opening: false, sig: '', barSig: '', pal: null, closers: [] };
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
  const mk = (o, sigParts) => { o.theme = o.theme || DEF[H(o.id) % DEF.length]; o.sig = JSON.stringify(sigParts); return o; };

  function commItem(c) {
    const un = unreadIn(c), at = mentionsIn(c), voice = S.cid === c.id ? c.channels.filter(x => x.kind === 'voice' && (callPeople(x.gid) || []).length) : [];
    const first = voice[0], people = first ? callPeople(first.gid) : [];
    const names = c.channels.filter(x => x.kind !== 'voice').map(x => x.name);
    return mk({ key: 'c:' + c.id, id: c.id, cid: c.id, kind: 'community', name: c.name, unread: un, at, act: un, fx: [4, 3, 0, 5, 2][H(c.name) % 5], rooms: c.channels.length, names, voiceName: first && first.name, people: people.map(p => p.name) },
      [c.name, un, at, c.channels.length, names.slice(0, 4), first && first.name, people.map(p => p.name)]);
  }
  const youItem = () => mk({ key: 'you', id: 'you', kind: 'you', name: 'you', unread: 0, at: 0, act: 0, fx: 4, theme: ['#d9dce4', '#2b3bff'] }, ['you']);
  function roomItems(c) {
    const off = H(c.id), out = [];
    const room = ch => {
      const voice = ch.kind === 'voice', i = c.channels.indexOf(ch), people = voice ? (callPeople(ch.gid) || []) : [], pv = PREV.get(ch.gid) || {};
      const it = { key: ch.gid, id: ch.gid, gid: ch.gid, cid: c.id, kind: voice ? 'voice' : 'text', name: ch.name, unread: ch.unread || 0, at: S.mention.get(ch.gid) || 0, locked: !ch.joined, status: ch.joined && ch.status && ch.status !== 'ok' ? ch.status : '', people, here: voice && inCallHere(ch.gid), fx: voice ? 1 : [0, 3, 4, 5, 2][(off + i) % 5], pv };
      it.act = it.unread;
      return mk(it, [it.name, it.unread, it.at, it.locked, it.status, people.map(p => [p.name, !!p.speaking, !!p.muted, !!p.deafened, !!p.me]), it.here, pv.who, pv.text]);
    };
    c.channels.filter(x => x.kind !== 'voice').forEach(x => out.push(room(x)));
    c.channels.filter(x => x.kind === 'voice').forEach(x => out.push(room(x)));
    out.push(mk({ key: 'opt:' + c.id, id: 'opt:' + c.id, cid: c.id, kind: 'options', name: 'options', unread: 0, at: 0, act: 0, fx: 6 }, ['options']));
    return out;
  }

  // newest line of each readable room, for the cards; asks the core only when a room's count changed
  async function loadPreviews(c) {
    if (loadPreviews.busy) return; loadPreviews.busy = true;
    try {
      for (const ch of c.channels) {
        if (ch.kind === 'voice' || !ch.joined) continue;
        const sig = ch.unread + ':' + ch.status, p = PREV.get(ch.gid);
        if (p && p.sig === sig && Date.now() - p.t < 4000) continue;   // the snapshot has no "last activity", so look again every few seconds
        let n;
        try {
          const m = await rpc('messages', { gid: ch.gid, limit: 8 });
          const last = [...m.messages].reverse().find(x => !x.deleted && x.kind !== 'notice' && x.kind !== 'member');
          n = { sig, t: Date.now(), who: last ? last.name : '', text: last ? plain(last.text || (last.file ? last.file.name : '')) : '' };
        } catch { n = { sig, t: Date.now(), who: '', text: '' }; }
        const changed = !p || p.who !== n.who || p.text !== n.text; PREV.set(ch.gid, n);
        if (changed && M.level === 'server' && M.cid === c.id) render();
      }
    } finally { loadPreviews.busy = false; }
  }

  function cardHTML(x) {
    let mid = '', sub = '', num = '';
    if (x.kind === 'community') {
      sub = `${x.rooms} room${x.rooms === 1 ? '' : 's'}`; num = x.unread || '';
      if (x.voiceName && x.people.length) mid += `<div class="mfaces-row">${mfaces(x.people)}<span>${x.people.length} in ${esc(x.voiceName)}</span></div>`;
      if (x.names.length) mid += `<div class="mpreview">${esc(x.names.slice(0, 5).join(', '))}${x.names.length > 5 ? ', …' : ''}</div>`;
    } else if (x.kind === 'you') { sub = 'settings'; mid = '<div class="mpreview">your profile and account</div>'; }
    else if (x.kind === 'voice') {
      sub = 'voice'; num = x.people.length || '';
      mid = x.people.length ? `<div class="mwho">${x.people.map(p => `<span>${mface(p.name, p.speaking ? 'talk' : '')}${esc(p.me ? 'you' : p.name)}</span>`).join('')}</div>` : '<div class="mpreview">nobody here yet</div>';
      if (x.locked) mid = '<div class="mpreview">this device does not have the keys for this room yet</div>';
      else mid += `<div class="mvact"><button class="mjoin" data-act="${x.here ? 'leave' : 'join'}" type="button">${x.here ? 'leave' : 'join'}</button></div>`;
      if (x.locked) sub = 'waiting for keys';
    } else if (x.kind === 'options') { sub = 'only on this device'; mid = '<div class="mpreview">alerts, sound and look. Nobody else can see these.</div>'; }
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
    const scrubCls = x => `${x.unread ? 'un' : ''} ${x.kind === 'voice' ? 'v' : x.kind === 'options' ? 'o' : ''}`;
    sc.innerHTML = items.map((x, i) => `<button type="button" tabindex="-1" data-i="${i}" class="${scrubCls(x)}" style="--c:${dotOf(x)}"></button>`).join('');
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
    };
    el.addEventListener('scroll', () => {
      if (!raf) raf = requestAnimationFrame(frame);
      energy = 1;
      if (settled >= 0 && Math.abs(el.scrollTop - settledTop) > 3) { slots[settled]?.classList.remove('settled'); settled = -1; remaskAll(); }
      clearTimeout(t); t = setTimeout(settle, 150);
    }, { passive: true });
    const go = (i, now) => { i = Math.max(0, Math.min(items.length - 1, i)); el.scrollTo({ top: tops[i] - (el.clientHeight - sh) / 2, behavior: now || calm() ? 'auto' : 'smooth' }); };
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
      visible() { return slots.map((s, i) => ({ x: items[i], i, a: Math.abs(i - cur) })).filter(o => o.a <= 1).map(o => ({ ...o, r: $('.msq', slots[o.i]).getBoundingClientRect() })); },
      rectOf(i) { return $('.msq', slots[i]).getBoundingClientRect(); },
      destroy() { ro.disconnect(); clearTimeout(t); clearTimeout(rz); dropAscii(); el.remove(); sc.remove(); },
    };
  }

  // ---------------------------------------------------------------- levels
  const guard = fn => async (...a) => { if (M.busy) return; M.busy = true; try { await fn(...a); } finally { M.busy = false; if (M.dirty) { M.dirty = false; render(); } } };
  const sq = () => getComputedStyle(root).getPropertyValue('--m-sq');
  function fly(r, m) { const f = document.createElement('div'); f.className = 'mfly'; f.style.cssText = `left:${r.left}px;top:${r.top}px;width:${r.width}px;height:${r.height}px;--m-mesh:${m}`; fxl.append(f); return f; }
  function killList() { if (M.list) { M.list.destroy(); M.list = null; } mega.innerHTML = ''; hideTip(); closeRail(); }
  function applyLevel() {
    const chat = M.level === 'chat';
    mega.hidden = chat; rail.hidden = !chat;
    root.classList.toggle('mg-chat', chat); root.classList.toggle('mg-cards', !chat);
    if (!chat) { hideOpts(); closeRail(); }
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
    if (!c) return `<div class="mbar"><span class="mmark">mega</span><span class="mtag" id="m-sync" hidden></span><span class="mgrow"></span><button class="mchip" data-m="new" type="button">new</button><button class="mchip" data-m="join" type="button">join</button>${more}${jump}</div>`;
    const admin = c.role === 'admin' || c.role === 'owner', names = S.membersFor === c.id ? S.members.map(m => m.name) : [];
    return `<div class="mbar">${cs.length > 1 ? '<button class="mchip" data-m="back" type="button" aria-label="Back to all communities"><span aria-hidden="true">&larr;</span><span class="mhide-s"> back</span></button>' : ''}<span class="mmark">${esc(c.name)}</span><span class="mtag" id="m-sync" hidden></span><span class="mgrow"></span>${admin ? '<button class="mchip mhide-s" data-m="invite" type="button">invite</button><button class="mchip mhide-s" data-m="room" type="button">new room</button>' : ''}<button class="mchip" data-m="people" type="button" aria-label="People">${names.length ? mfaces(names.slice(0, 3)) : ''}<span class="mhide-s">people</span></button>${more}${jump}</div>`;
  }
  function drawBar() {
    const html = barHTML(); let bar = $('.mbar', mega);
    if (bar && html !== M.barSig) { bar.outerHTML = html; }
    else if (!bar) mega.insertAdjacentHTML('afterbegin', html);
    M.barSig = html; syncText();
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

  function homeItems() { return [...comms().map(commItem), youItem()]; }
  function showHome(startCid) {
    killList(); M.level = 'home'; M.opts = false; applyLevel();
    drawBar(); M.sig = '';
    const items = homeItems(), want = startCid || store.get('last', null) || S.cid;
    const start = Math.max(0, items.findIndex(x => x.cid === want));
    M.list = List(items, { start, enter: enterCommunity });
    drawNotes();
  }
  function showServer(cid, startKey) {
    const c = comm(cid); if (!c) { showHome(); return; }
    setCommunity(cid); store.set('last', cid);
    killList(); M.level = 'server'; M.cid = cid; M.opts = false; applyLevel();
    drawBar();
    const items = roomItems(c);
    let start = startKey ? items.findIndex(x => x.key === startKey) : items.findIndex(x => x.unread > 0 && x.kind === 'text');
    if (start < 0) start = 0;
    M.list = List(items, { start, enter: enterRoom, join: joinVoice });
    drawNotes(); loadPreviews(c);
  }
  const goHome = () => { const cs = comms(); if (cs.length === 1) showServer(cs[0].id, S.gid); else showHome(M.cid || S.cid); };

  // home -> a community: the card opens up to fill the room, then its rooms
  const enterCommunity = guard(async x => {
    if (x.kind === 'you') { $('#btn-settings').click(); return; }
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
    M.level = 'chat'; M.cid = x.cid; M.opts = x.kind === 'options';
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
    const cid = M.cid, key = M.opts ? 'opt:' + cid : S.gid;
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
    killList(); M.level = 'chat'; M.cid = cid; M.opts = !gid; drawRail(); applyLevel(); showOpts();
  }
  const toChatOpen = cid => { killList(); M.level = 'chat'; M.cid = cid; M.opts = false; drawRail(); applyLevel(); showOpts(); };

  // ---------------------------------------------------------------- the dots
  function dotHTML(x, i) {
    const vo = x.kind === 'voice', active = x.kind === 'options' ? M.opts : (!M.opts && x.gid === S.gid);
    const say = [x.name, vo ? 'voice room' : '', vo && x.people.length ? x.people.length + ' in the call' : '', x.unread && !active ? x.unread + ' unread' : '', x.at ? x.at + ' mentions of you' : '', x.locked ? 'waiting for keys' : ''].filter(Boolean).join(', ');
    const un = x.unread && !active && !vo;
    return `<button class="mitem ${vo ? 'voice' : ''} ${active ? 'active' : ''} ${un ? 'un' : ''} ${vo && x.people.length ? 'live' : ''} ${vo && x.here ? 'here' : ''} ${x.kind === 'options' ? 'opt' : ''} ${x.locked ? 'pending' : ''}" type="button" data-key="${esc(x.key)}" aria-label="${esc(say)}"${active ? ' aria-current="true"' : ''} style="--c:${dotOf(x)};--m-cink:${bestInk(fieldOf(x))};--n:${i}">${x.at && !active ? `<span class="mbadge">${x.at}</span>` : ''}${vo ? `<span class="mvc">${x.people.length ? esc(x.people.map(p => p.me ? 'you' : p.name).join(', ')) : 'empty'}</span>` : ''}${ti(x.name, 'mlb', 'span')}${(vo ? x.people.length : un ? x.unread : 0) ? `<span class="mct">${vo ? x.people.length : x.unread}</span>` : ''}</button>`;
  }
  const railSig = c => JSON.stringify([c.id, S.gid, M.opts, roomItems(c).map(x => x.sig), comms().map(o => o.id !== c.id && unreadIn(o) > 0)]);
  function drawRail(arriving) {
    const c = comm(M.cid); if (!c) return;
    const items = roomItems(c), others = comms().some(o => o.id !== c.id && unreadIn(o) > 0), many = comms().length > 1;
    M.railItems = items; M.sig = railSig(c);
    rail.className = arriving ? 'arriving' : ''; M.rail = false;
    rail.innerHTML = `<button class="mhomedot ${others ? 'un' : ''}" id="m-home" type="button" aria-label="${many ? 'All communities' : 'All rooms'}${others ? ', unread messages elsewhere' : ''}" data-name="${many ? 'all communities' : 'all rooms'}"><span class="mhl">${many ? 'all communities' : 'all rooms'}</span></button><div class="mitems" id="m-items">${items.map(dotHTML).join('')}</div>`;
  }
  function openRail() {
    if (M.rail || M.level !== 'chat') return; M.rail = true; hideTip();
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
    if (x.kind === 'options') { M.opts = true; showOpts(); }
    else { M.opts = false; showOpts(); openRoomVia(x.gid); }
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
      <section><h3>look</h3><label>Theme</label>${seg('th', [['', 'Match system'], ['light', 'Daylight'], ['solar', 'Solarized'], ['dark', 'After hours'], ['wave', 'Wave']], th)}
        <div id="wave-row" ${th === 'wave' ? '' : 'hidden'}><label for="wave-pal">Wave colours</label><select id="wave-pal">${wave.map(([k, l]) => `<option value="${k}"${(pref.get('wavePalette') || 'blue') === k ? ' selected' : ''}>${l}</option>`).join('')}</select></div>
        <label>Message font</label>${seg('rd', [['', 'Book'], ['plain', 'Plain']], pref.get('read') || '')}</section>
      <section><h3>typing hints</h3><label class="check"><input type="checkbox" id="ty" ${S.snap.typing_enabled ? 'checked' : ''}><span>Send “typing…” hints — the relay can see that a message-sized packet was sent at that moment</span></label></section>
      ${mobileApp() ? '' : `<section><h3>large files</h3><label for="dl-dir">Large-file downloads folder</label><input type="text" id="dl-dir" value="${esc(S.snap.download_dir || '')}" spellcheck="false"><p>Written here as they arrive, one verified piece at a time. Leave empty for your usual Downloads folder. Each download still asks where to save it.</p></section>`}
    </div>`;
  }
  function showOpts() {
    if (!M.opts || M.level !== 'chat') { hideOpts(); return; }
    optsEl.innerHTML = optsHTML(); optsEl.hidden = false; optsEl.scrollTop = 0;
  }
  optsEl.addEventListener('click', e => {
    const b = e.target.closest('.seg button'); if (!b) return;
    const g = b.parentElement, id = g.dataset.for, v = b.dataset.v;
    $$('button', g).forEach(x => x.setAttribute('aria-checked', x === b));
    $('#' + id, optsEl).value = v;
    if (id === 'snd') pref.set('sound', v === 'all' ? '' : v);
    else if (id === 'th') { pref.set('theme', v); applyPrefs(); if (window.__waveDither) window.__waveDither.refresh(); const r = $('#wave-row', optsEl); if (r) r.hidden = v !== 'wave'; }
    else if (id === 'rd') { pref.set('read', v); applyPrefs(); }
  });
  optsEl.addEventListener('change', async e => {
    try {
      if (e.target.id === 'ty') { await rpc('set_typing_enabled', { on: e.target.checked }); tick(); }
      else if (e.target.id === 'dl-dir') { await rpc('set_download_dir', { dir: e.target.value }); tick(); }
      else if (e.target.id === 'wave-pal') { pref.set('wavePalette', e.target.value); applyPrefs(); if (window.__waveDither) window.__waveDither.refresh(); }
    } catch (er) { toast(er.message); }
  });
  optsEl.addEventListener('keydown', e => { if (e.key === 'Enter' && e.target.id === 'dl-dir') e.target.blur(); });

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
    menu([['settings', () => $('#btn-settings').click()], ['devices and recovery', () => $('#btn-devices').click()], ['activity log', () => $('#btn-activity').click()], ...(M.level === 'server' ? [['new community', () => follow(newCommunity())], ['join with an invite link', () => joinCommunity()]] : [])], Math.min(r.left, innerWidth - 220), r.bottom + 6);
  }
  mega.addEventListener('click', e => {
    const j = e.target.closest('[data-join]');
    if (j) { rpc('cancel_join', { id: j.dataset.join }).then(() => tick()).catch(er => toast(er.message)); return; }
    const b = e.target.closest('[data-m]'); if (!b) return;
    const a = b.dataset.m;
    if (a === 'new') follow(newCommunity()); else if (a === 'join') joinCommunity();
    else if (a === 'more') moreMenu(b); else if (a === 'jump') palette();
    else if (a === 'back') showHome(M.cid);
    else if (a === 'invite') $('#btn-invite').click(); else if (a === 'room') $('#add-room').click();
    else if (a === 'people') $('#btn-people').click();
  });

  // a community made from here becomes the one the person is looking at: show its rooms once the core lists it
  async function follow(p) {
    await p;
    for (let i = 0; i < 24; i++) { if (S.cid && S.cid !== M.cid && comm(S.cid)) { if (M.level !== 'chat') showServer(S.cid); return; } await wait(150); }
  }
  window.__megaNewCommunity = () => follow(newCommunity());

  function palette() {
    if ($('#mega-pal')) return; closePops();
    const all = [];
    for (const c of comms()) {
      const cx = { id: c.id, kind: 'community', theme: DEF[H(c.id) % DEF.length] };
      all.push({ x: cx, label: c.name, sub: 'community', run: () => showServer(c.id) });
      for (const it of roomItems(c)) {
        const sub = c.name;
        if (it.kind === 'voice') all.push({ x: it, label: it.name, sub, run: () => showServer(c.id, it.key) });
        else if (it.kind === 'options') all.push({ x: it, label: 'options', sub, run: () => toChat(c.id, null) });
        else all.push({ x: it, label: it.name, sub: it.locked ? sub + ', waiting for keys' : sub, run: () => { if (it.locked) { showServer(c.id, it.key); toast('This room is still waiting for keys.'); } else toChat(c.id, it.gid); } });
      }
    }
    for (const [label, run] of [['settings', () => $('#btn-settings').click()], ['devices and recovery', () => $('#btn-devices').click()], ['activity log', () => $('#btn-activity').click()], ['new community', () => follow(newCommunity())], ['join with an invite link', () => joinCommunity()]]) all.push({ x: { kind: 'action', id: 'a:' + label, theme: ['#efedf2', '#efedf2'] }, label, sub: 'action', run });
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
    if (closePops()) return true;
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
    if (typing || !M.list || M.closers.length || M.busy) return;
    if (e.key === 'ArrowDown' || e.key === 'j') { e.preventDefault(); M.list.move(1); }
    else if (e.key === 'ArrowUp' || e.key === 'k') { e.preventDefault(); M.list.move(-1); }
    else if (e.key === 'Enter' && (document.activeElement === document.body || document.activeElement === M.list.el)) {
      e.preventDefault(); const x = M.list.item;
      if (x.kind === 'voice') { if (!x.locked && !x.here) joinVoice(x, 'join'); } else enterCurrent();   // Enter on a voice card is an explicit join
    }
  }, true);
  function enterCurrent() { const x = M.list.item; if (!x) return; if (M.level === 'home') enterCommunity(x); else enterRoom(x); }
  // phones: swipe in from the left edge to pop the dots out
  let tx = null;
  addEventListener('touchstart', e => { tx = M.level === 'chat' && e.touches[0].clientX < 30 ? e.touches[0].clientX : null; if (M.rail && !e.target.closest('#mega-rail')) closeRail(); }, { passive: true });
  addEventListener('touchmove', e => { if (tx != null && e.touches[0].clientX - tx > 34) { openRail(); tx = null; } }, { passive: true });

  // ---------------------------------------------------------------- following the snapshot
  function render() {
    if (!S.snap) return;
    if (!M.started) { start(); return; }
    if (M.busy) { M.dirty = true; return; }
    const cs = comms();
    if (M.level === 'home') {
      if (cs.length === 1) { showServer(cs[0].id, S.gid); return; }
      const next = homeItems();
      if (M.list && !M.list.refresh(next)) showHome(M.list.item && M.list.item.cid);
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
    if (M.opts) { /* the options room keeps its own state */ }
  }
  function start() {
    M.started = true;
    const cs = comms();
    if (cs.length === 1) showServer(cs[0].id); else showHome(store.get('last', null));
    document.fonts.load('500 14px "IBM Plex Mono"').catch(() => { }).then(() => { fontKey = 1; ACTIVE.forEach(a => { a.at = atlasOf(fieldOf(a.x), hiOf(a.x), a.sw, a.sh, a.fs); asciiDraw(a, clock); }); });
    document.fonts.ready.then(remaskAll);
    requestAnimationFrame(asciiTick);
  }
  window.__megaRender = render;
  window.__megaIdle = () => M.started && (M.level !== 'chat' || M.opts);
  // a room was opened by something other than mega (a search hit, the old rail's handlers)
  window.__megaOpened = gid => {
    if (M.opening || !M.started) return;
    const c = comms().find(o => o.channels.some(x => x.gid === gid)); if (!c) return;
    if (M.level === 'chat' && M.cid === c.id) { M.opts = false; showOpts(); drawRail(); return; }
    toChatOpen(c.id);
  };
  M.prev = PREV; window.__mega = M;
  if (S.snap && !$('#app').hidden) render();   // the first snapshot may have arrived while this file was still loading
})();
