'use strict';
// mega: the "what's new" card, unread marks on the dots (and the hint that teaches them), and chat density per room.
// Hooks into mega.js are small calls to window.__megaNew; everything else about the cards and the chat stays there.
//
// Rules this file keeps:
//  - Showing the card, scrolling past it or resting on it never marks anything read (the home level is idle: see __megaIdle).
//    Only choosing a row opens a room, and an open room is read the way it always was.
//  - Density is personal to this device: it lives in the `pref` store, never in the owner's style document.
(() => {
  const M = window.__mega; if (!M || !M.api) return;
  const A = M.api, root = document.documentElement;
  const mega = document.getElementById('mega');
  const N = {};

  // ---------------------------------------------------------------- 1. the "what's new" card
  const NEWS = new Map();   // gid -> { sig, who, text, ts }: the line worth showing for an unread room, and when the room last spoke
  const THEME = ['#ff8a5c', '#ffe45e'];
  let nbusy = false;
  const roomX = (cid, gid) => ({ kind: 'text', id: gid, cid, gid });
  function rowsNow() {
    const out = [];
    for (const c of A.comms()) for (const ch of c.channels) {
      if (ch.kind === 'voice' || !ch.joined || !(ch.unread > 0)) continue;
      const p = NEWS.get(ch.gid), x = roomX(c.id, ch.gid); x.theme = A.themeOf(x);
      out.push({ cid: c.id, gid: ch.gid, comm: c.name, room: ch.name, n: ch.unread, at: S.mention.get(ch.gid) || 0, ts: p ? p.ts : 0, who: p ? p.who : '', text: p ? p.text : '', col: A.dotOf(x) });
    }
    // mentions first, then the rest, each newest first (the snapshot has no "last activity"; it comes from the line fetched below)
    out.sort((a, b) => (b.at > 0) - (a.at > 0) || b.ts - a.ts);
    return out;
  }
  // the newest line of each unread room, for its row: asked for only when a room's counts changed, one request at a time
  async function fill(rows) {
    if (nbusy) return; nbusy = true; let changed = false;
    try {
      for (const r of rows) {
        const sig = r.n + ':' + r.at, p = NEWS.get(r.gid);
        if (p && p.sig === sig) continue;
        let n;
        try {
          const m = await rpc('messages', { gid: r.gid, limit: Math.min(80, r.n + 12) });
          const list = m.messages.filter(x => !x.deleted && x.kind !== 'notice' && x.kind !== 'member'), last = list[list.length - 1];
          const hit = r.at ? [...list].reverse().find(x => !x.mine && (mentionsMe(x.text) || repliesToMe(x, m.messages))) : null, pick = hit || last;
          n = { sig, who: pick ? pick.name : '', text: pick ? plain(pick.text || (pick.file ? pick.file.name : '')) : '', ts: last ? last.ts || 0 : 0 };
        } catch { n = { sig, who: '', text: '', ts: 0 }; }
        if (!p || p.who !== n.who || p.text !== n.text || p.ts !== n.ts) changed = true;
        NEWS.set(r.gid, n);
        if (changed && window.__megaRender) window.__megaRender();
      }
    } finally { nbusy = false; }
  }
  /** The card as a list item, or null when nothing is unread. */
  N.item = () => {
    if (!S.snap) return null;
    const rows = rowsNow(); if (!rows.length) return null;
    fill(rows);
    const un = rows.reduce((a, r) => a + r.n, 0), at = rows.reduce((a, r) => a + r.at, 0);
    return { key: 'news', id: 'news', kind: 'news', name: 'what’s new', unread: un, at, act: Math.min(un, 12), fx: 12, theme: THEME, locked: false, rows,
      sig: JSON.stringify([rows.map(r => [r.gid, r.n, r.at, r.who, r.text, r.comm, r.room, r.col])]) };
  };
  N.html = x => {
    const rows = x.rows.map(r => {
      const say = `${r.comm}, ${r.room}, ${r.n} unread${r.at ? ', ' + r.at + ' mention' + (r.at === 1 ? '' : 's') + ' of you' : ''}`;
      return `<button type="button" class="mnrow mfaces-row ${r.at ? 'at' : ''}" data-cid="${esc(r.cid)}" data-gid="${esc(r.gid)}" style="--c:${r.col}" aria-label="${esc(say)}"><span class="mnd" aria-hidden="true"></span>
        <span class="mnw"><span class="mnh"><span class="mnc">${esc(r.comm)}</span><span class="mnr">${esc(r.room)}</span></span><span class="mnp">${r.text ? `<b>${esc(r.who)}</b> ${esc(r.text)}` : '&nbsp;'}</span></span>
        <span class="mnn">${r.at ? `<i>@${r.at}</i>` : ''}<b>${r.n}</b></span></button>`;
    }).join('');
    const num = x.unread > 99 ? '99+' : String(x.unread);
    return `<div class="msq" style="${A.cardVars(x)}"><div class="mtop"><span>${x.rows.length} unread room${x.rows.length === 1 ? '' : 's'}</span>${x.at ? `<span class="mat" aria-label="${x.at} mention${x.at === 1 ? '' : 's'}">@${x.at}</span>` : ''}</div>
      <div class="mmid"><div class="mnews" role="list">${rows}</div></div>${A.ti(x.name)}<div class="mnu" aria-label="${x.unread}">${A.letters(num)}</div></div>`;
  };
  // straight into a room, at its first unread message (the room opens the way it always does, so the "n new" line is where it lands)
  N.open = (cid, gid) => {
    if (M.busy || M.level !== 'home') return;
    if (S.gid === gid) resetRoom();   // the same room again: start from the first unread once more
    A.toChat(cid, gid);
    if (!calm()) { const m = document.getElementById('main'); m.classList.add('mslide'); setTimeout(() => m.classList.remove('mslide'), 700); }
  };
  N.openFirst = x => { const r = x.rows && x.rows[0]; if (r) N.open(r.cid, r.gid); };
  mega.addEventListener('click', e => {
    const b = e.target.closest && e.target.closest('.mnrow'); if (!b) return;
    e.preventDefault(); e.stopPropagation(); N.open(b.dataset.cid, b.dataset.gid);
  }, true);
  // The list never rebuilds under a finger or a wheel: a card that appears or goes waits until the touch ends and the scroll has settled.
  let down = false, lastMove = 0;
  const touch = () => { lastMove = Date.now(); };
  mega.addEventListener('pointerdown', () => { down = true; touch(); }, true);
  addEventListener('pointerup', () => { down = false; touch(); }, true);
  addEventListener('pointercancel', () => { down = false; touch(); }, true);
  mega.addEventListener('wheel', touch, { passive: true, capture: true });
  mega.addEventListener('scroll', touch, { passive: true, capture: true });
  N.hold = () => down || Date.now() - lastMove < 700;
  // a rebuild keeps the focused card where it was and does not play the entrance again
  N.still = () => { if (M.list) M.list.el.classList.remove('mrise'); };

  // ---------------------------------------------------------------- 2. unread at a glance
  /** The ring colour for a dot: the room's highlight, lifted to read on the near-black rail, and apart from the dot's own fill. */
  N.mark = x => {
    try {
      const dot = A.dotOf(x); let m = A.lift(A.hiOf(x), 4.8);
      if (A.contrast(dot, m) < 1.9) m = A.mix(m, '#ffffff', .62);
      if (A.contrast(dot, m) < 1.9) m = A.mix(dot, '#ffffff', .75);
      return m;
    } catch { return '#ffffff'; }
  };
  // the one-time hint: what the dots are, and that scrolling (or swiping) on them opens the cards
  const hintKey = 'dotsHint';
  const hintState = () => { try { return JSON.parse(pref.get(hintKey) || '{}'); } catch { return {}; } };
  const hintSave = st => pref.set(hintKey, JSON.stringify(st));
  let hintEl = null, hintT = 0;
  const coarse = () => matchMedia('(pointer: coarse)').matches;
  function hideHint(forever) {
    clearTimeout(hintT);
    if (forever) hintSave({ used: 1 });
    if (hintEl) { const e = hintEl; hintEl = null; e.classList.remove('on'); setTimeout(() => e.remove(), 300); }
  }
  function showHint() {
    const st = hintState(); if (st.used || (st.n || 0) >= 3 || hintEl || !root.classList.contains('mg-chat') || M.rail) return;
    hintSave({ n: (st.n || 0) + 1 });
    const e = hintEl = document.createElement('div'); e.id = 'mega-hint'; e.setAttribute('role', 'note');
    e.innerHTML = `<span class="mh-dots" aria-hidden="true"><i></i><i></i><i></i></span><span class="mh-t">${coarse() ? 'these dots are your rooms. swipe up or down on them to open the cards.' : 'these dots are your rooms. scroll on them to open the cards.'}</span><button type="button" class="mh-x" aria-label="got it">got it</button>`;
    e.querySelector('.mh-x').onclick = () => hideHint(true);
    document.body.append(e); requestAnimationFrame(() => e.classList.add('on'));
    hintT = setTimeout(() => hideHint(false), 11000);
  }
  // opening the cards by wheel, swipe in or swipe along the dots is the lesson learnt
  N.railUsed = () => hideHint(true);
  // switching the chat font in Settings: the density stops change with it (Raster keeps to whole stops)
  new MutationObserver(() => { applyDensity(); closePop(); }).observe(root, { attributes: true, attributeFilter: ['data-mchat'] });
  let wasChat = false;
  new MutationObserver(() => {
    const chat = root.classList.contains('mg-chat');
    if (chat && !wasChat) setTimeout(showHint, 1600);
    if (!chat && hintEl) hideHint(false);
    wasChat = chat;
  }).observe(root, { attributes: true, attributeFilter: ['class'] });
  // by touch: dragging up or down along the dots opens them (the swipe in from the left edge still does)
  {
    let y0 = null;
    const rail = document.getElementById('mega-rail');
    rail.addEventListener('touchstart', e => { y0 = e.touches.length === 1 && !M.rail ? e.touches[0].clientY : null; }, { passive: true });
    rail.addEventListener('touchmove', e => { if (y0 != null && Math.abs(e.touches[0].clientY - y0) > 14) { y0 = null; A.openRail(); } }, { passive: true });
    rail.addEventListener('touchend', () => { y0 = null; }, { passive: true });
  }

  // ---------------------------------------------------------------- 3. density per room
  // The middle stop (0) is exactly today's look and sets nothing. Each stop is a size for ordinary messages, a size for each length
  // tier, and the space between rows. The Raster chat face is a pixel font, sharp only on its 12 px grid: five stops (mega-new.css).
  // Bellefair scales cleanly, so it gets half steps too: nine stops, the same ends (12 px and 48 px), sizes set here.
  const raster = () => root.dataset.mchat === 'raster';
  const step = () => (raster() ? 1 : 0.5);
  const NAMES = { '-2': 'tightest', '-1.5': 'tighter', '-1': 'tighter', '-0.5': 'a bit tighter', '0': 'as it was', '0.5': 'a bit larger', '1': 'larger', '1.5': 'larger', '2': 'largest' };
  // Bellefair, from -2 to 2 in half steps: [message, short, shorter, shortest, row gap, row padding] in px
  const FINE = [[12, 12, 24, 36, 2, 0], [15, 16, 28, 42, 3, 0], [18, 21, 34, 50, 4, 1], [21, 28, 40, 60, 6, 1], null,
    [28, 42, 56, 78, 10, 2], [33, 50, 68, 88, 12, 3], [40, 60, 82, 104, 15, 3], [48, 72, 96, 120, 18, 4]];
  const attr = n => (n < 0 ? 'm' : 'p') + Math.abs(n);
  const clamp = v => { v = Number(v); if (!Number.isFinite(v)) return 0; v = Math.max(-2, Math.min(2, Math.round(v / step()) * step())); return Object.is(v, -0) ? 0 : v; };
  const D = {
    all: () => pref.get('densAll') === '1',
    global: () => clamp(pref.get('dens')),
    own: gid => clamp(pref.get('dens.' + gid)),
    of: gid => (D.all() ? D.global() : D.own(gid)),
    gid: null,
  };
  N.density = gid => D.of(gid);
  function applyDensity() {
    const n = D.gid ? D.of(D.gid) : 0, was = root.dataset.mdens || '', wasFine = root.style.getPropertyValue('--dz0');
    const fine = !raster() && n ? FINE[(n + 2) * 2] : null;
    const want = n ? (fine ? 'f' + n : attr(n)) : '';
    if (want === was && !!fine === !!wasFine) return;
    const st = document.getElementById('stream'), pinned = st && st.scrollHeight - st.scrollTop - st.clientHeight < 80;
    // Bellefair's sizes ride on the root (no #stream rule sets them for an f… value), the Raster stops on their own CSS rules
    ['--dz0', '--dz1', '--dz2', '--dz3', '--dgap', '--dpad'].forEach((k, i) => (fine ? root.style.setProperty(k, fine[i] + 'px') : root.style.removeProperty(k)));
    if (want) root.dataset.mdens = want; else delete root.dataset.mdens;
    if (st && pinned) st.scrollTop = st.scrollHeight;
    if (M.relook) M.relook();   // the rows changed size: the focus ring, the backdrop mask and the ascii all look again
  }
  function headButton() {
    let b = document.getElementById('btn-density');
    if (!b) {
      const bar = document.querySelector('#head .head-actions'); if (!bar) return null;
      b = document.createElement('button'); b.id = 'btn-density'; b.type = 'button'; b.className = 'btn ghost'; b.innerHTML = '<span class="lbl">Aa</span>';
      b.title = 'How much chat fits on screen'; b.setAttribute('aria-label', 'Density'); b.setAttribute('aria-haspopup', 'dialog'); b.setAttribute('aria-expanded', 'false');
      b.onclick = () => (pop ? closePop() : openPop());
      bar.insertBefore(b, document.getElementById('btn-wall') || document.getElementById('btn-call') || document.getElementById('btn-invite') || bar.firstChild);
    }
    return b;
  }
  N.room = gid => {
    if (D.gid !== gid) closePop();   // another room: the menu belongs to the one it was opened for
    D.gid = gid;
    const b = headButton(); if (b) b.hidden = !gid;
    if (!gid) closePop();
    applyDensity(); syncPop();
  };
  let pop = null, popOut = null;
  function closePop() {
    if (!pop) return;
    pop.remove(); pop = null;
    document.removeEventListener('pointerdown', popOut, true); M.closers = M.closers.filter(f => f !== closePop);
    const b = document.getElementById('btn-density'); if (b) b.setAttribute('aria-expanded', 'false');
  }
  function syncPop() {
    if (!pop) return;
    const r = pop.querySelector('#md-r'), a = pop.querySelector('#md-all'), n = D.gid ? D.of(D.gid) : 0;
    if (document.activeElement !== r) r.value = String(n);
    a.checked = D.all(); r.setAttribute('aria-valuetext', NAMES[n]); pop.querySelector('#md-v').textContent = NAMES[n];
    pop.querySelector('.md-for').textContent = D.all() ? 'every room' : (roomName() || 'this room');
  }
  const roomName = () => { const c = A.comm(M.cid), ch = c && c.channels.find(x => x.gid === D.gid); return ch ? ch.name : ''; };
  function openPop() {
    const b = headButton(); if (!b || !D.gid) return;
    const p = pop = document.createElement('div'); p.id = 'mega-dens'; p.className = 'mpop'; p.setAttribute('role', 'dialog'); p.setAttribute('aria-label', 'Density');
    p.innerHTML = `<div class="md-h"><span>density</span><span class="md-for"></span></div>
      <input type="range" id="md-r" min="-2" max="2" step="${step()}" value="0" aria-label="How much chat fits on screen">
      <div class="md-s"><span>more on screen</span><span id="md-v"></span><span>larger</span></div>
      <label class="md-c"><input type="checkbox" id="md-all"><span>same in every room</span></label>`;
    document.body.append(p);
    const r = b.getBoundingClientRect();
    p.style.top = Math.min(innerHeight - p.offsetHeight - 8, r.bottom + 8) + 'px'; p.style.left = Math.max(8, Math.min(innerWidth - p.offsetWidth - 8, r.right - p.offsetWidth)) + 'px';
    b.setAttribute('aria-expanded', 'true');
    const slider = p.querySelector('#md-r'), all = p.querySelector('#md-all');
    slider.oninput = () => {
      const n = clamp(slider.value);
      if (D.all()) pref.set('dens', n ? String(n) : ''); else pref.set('dens.' + D.gid, n ? String(n) : '');
      applyDensity(); syncPop();
    };
    all.onchange = () => {
      if (all.checked) pref.set('dens', D.of(D.gid) ? String(D.of(D.gid)) : ''), pref.set('densAll', '1');   // this room's value becomes everyone's
      else { const n = D.global(); pref.set('densAll', ''); pref.set('dens.' + D.gid, n ? String(n) : ''); }   // this room keeps what it shows; the others go back to their own
      applyDensity(); syncPop();
    };
    popOut = e => { if (!p.contains(e.target) && !b.contains(e.target)) closePop(); };
    document.addEventListener('pointerdown', popOut, true); M.closers.push(closePop);   // Esc closes it first, through the same list as the other menus
    syncPop(); slider.focus({ preventScroll: true });
  }
  N.showHint = showHint;
  N.state = () => ({ gid: D.gid, level: D.gid ? D.of(D.gid) : 0, all: D.all(), global: D.global(), attr: root.dataset.mdens || '', pop: !!pop, hint: !!hintEl, hintState: hintState() });

  window.__megaNew = N;
  if (window.__megaRender) window.__megaRender();
})();
