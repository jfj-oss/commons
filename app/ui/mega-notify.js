'use strict';
// mega: notifications, the page's half.
//
// The background part (the bridge, or the desktop app's core) looks at the rooms itself and shows an operating system
// notification for a new message when no mega window is being looked at (crates/core/src/notify.rs). This file is what the
// page adds:
//  * a heartbeat every few seconds, from a worker so that a hidden tab does not slow it down: "I am here, and I am being
//    looked at / not". While it is fresh and the page is hidden, the page shows the notification itself (the Web
//    Notification API, so a click lands in the right tab) from the notes the heartbeat's answer carries; the background part
//    stays silent. Without the heartbeat for ten seconds, or without the browser's permission, the background part shows it.
//  * the rules, read from and written to the background part (they live in the data folder, so both agree): the global
//    mode and the "show message text" switch (Settings), and a mute per room (right-click or long-press on a room).
//  * the muted mark on a room's dot and card, and the sound's respect for the same mutes (app.js `news`).
(() => {
  const N = { mode: 'all', show_text: false, rooms: {}, skew: 0, sig: '', subs: [], loaded: false };
  const nowS = () => Date.now() / 1000 + N.skew;
  const rpc_ = (m, a) => rpc(m, a || {});

  // ---------------------------------------------------------------- the rules
  function apply(s) {
    if (!s || typeof s !== 'object') return;
    const sig = JSON.stringify([s.mode, s.show_text, s.rooms]);
    N.mode = s.mode || 'all'; N.show_text = !!s.show_text; N.rooms = s.rooms || {}; N.skew = (s.now || 0) - Date.now() / 1000;
    const first = !N.loaded; N.loaded = true;
    if (sig !== N.sig || first) { N.sig = sig; paintMarks(); N.subs.forEach(f => { try { f(); } catch { } }); }
  }
  const rule = gid => { const r = N.rooms[gid]; if (!r) return null; if (r.kind === 'muted' && r.until && r.until <= nowS()) return null; return r; };
  const state = gid => { const r = rule(gid); return r ? (r.kind === 'muted' ? 'muted' : 'mentions') : ''; };
  // May a message in this room make a sound? (Notifications have their own global mode; a room's rule applies to both.)
  const soundOk = (gid, named) => { const r = rule(gid); if (!r) return true; return r.kind === 'muted' ? false : !!named; };

  // ---------------------------------------------------------------- the marks
  const BELL = '<svg viewBox="0 0 20 20" aria-hidden="true" focusable="false"><path d="M5 14V9a5 5 0 0 1 10 0v5l1.5 1.5h-13zM8.5 17.5h3"/><path class="sl" d="M3 3l14 14"/></svg>';
  const markFor = x => {
    if (!x || x.kind !== 'text' || !x.gid) return '';
    const s = state(x.gid); if (!s) return '';
    return s === 'muted' ? `<span class="mmute" title="Muted on this device" role="img" aria-label="muted">${BELL}</span>` : '<span class="mmute at" title="Only mentions and replies" role="img" aria-label="mentions only">@</span>';
  };
  // In place, so the marks follow a change at once even while a sidebar or a list is open.
  function paintMarks() {
    const M = window.__mega; if (!M || !M.started) return;
    const put = (host, x) => {
      const have = host.querySelector(':scope > .mmute');
      const want = markFor(x);
      if (!want) { if (have) have.remove(); host.classList.remove('is-muted'); return; }
      const t = document.createElement('template'); t.innerHTML = want;
      if (have) have.replaceWith(t.content.firstChild); else host.append(t.content.firstChild);
      host.classList.add('is-muted');
    };
    document.querySelectorAll('#mega-rail .mitem[data-key]').forEach(b => put(b, (M.railItems || []).find(k => k.key === b.dataset.key))) ;
    const items = M.list && M.list.items;
    if (items) document.querySelectorAll('#mega .mslot[data-i]').forEach(s => { const q = s.querySelector('.msq'); if (q) put(q, items[+s.dataset.i]); });
    document.querySelectorAll('.room[data-g]').forEach(b => put(b, { kind: 'text', gid: b.dataset.g })) ;
  }

  // ---------------------------------------------------------------- opening a room from a notification
  function openRoomAnywhere(gid) {
    try {
      const c = S.snap && S.snap.communities.find(k => k.channels.some(x => x.gid === gid)); if (!c) return;
      const M = window.__mega;
      if (M && M.started && M.api) M.api.toChat(c.id, gid);
      else { if (S.cid !== c.id) { S.cid = c.id; S.gid = null; resetRoom(); S.membersKey = ''; refreshMembers(); } openRoom(gid); }
    } catch { }
  }

  // ---------------------------------------------------------------- showing, and the heartbeat
  const attended = () => document.visibilityState === 'visible' && document.hasFocus();
  const canWeb = () => !window.__TAURI__ && typeof Notification !== 'undefined' && Notification.permission === 'granted';
  const shown = [];
  function show(n) {
    if (!n || attended()) return;
    try {
      const x = new Notification(String(n.title || 'mega'), { body: String(n.body || ''), tag: 'mega-' + n.gid, renotify: true, icon: 'favicon-32.png' });
      x.onclick = () => { try { window.focus(); x.close(); } catch { } openRoomAnywhere(n.gid); };
      shown.push(n); if (shown.length > 50) shown.shift();
    } catch { }
  }
  let beating = false;
  async function beat() {
    if (beating || !S.snap) return;
    beating = true;
    try {
      const web = canWeb();
      const r = await rpc_('notify_beat', { attended: attended(), web });
      apply(r.settings);
      if (r.open) openRoomAnywhere(r.open);
      if (web && r.notes) r.notes.forEach(show);
    } catch { } finally { beating = false; }
  }
  function clock() {
    let w = null;
    try {
      w = new Worker(URL.createObjectURL(new Blob(['setInterval(function(){postMessage(0)},3000)'], { type: 'text/javascript' })));
      w.onmessage = beat; w.onerror = () => { w = null; setInterval(beat, 3000); };
    } catch { setInterval(beat, 3000); }
  }
  clock();
  ['visibilitychange', 'focus', 'blur', 'pageshow'].forEach(ev => (ev === 'visibilitychange' ? document : window).addEventListener(ev, () => setTimeout(beat, 0)));
  setTimeout(beat, 300);
  // a first look at the rules as soon as the app is up (the heartbeat above waits for the first snapshot)
  const early = setInterval(() => { if (S.snap) { clearInterval(early); beat(); } }, 250);

  async function ask() {
    if (typeof Notification === 'undefined' || window.__TAURI__) return 'unsupported';
    if (Notification.permission === 'default') { try { await Notification.requestPermission(); } catch { } }
    beat(); return Notification.permission;
  }

  // ---------------------------------------------------------------- Settings
  const permLine = () => {
    if (window.__TAURI__) return '';
    if (typeof Notification === 'undefined') return '<p class="muted small">This browser cannot show its own notifications; mega\'s background part shows them through your system instead.</p>';
    const p = Notification.permission;
    if (p === 'granted') return '<p class="muted small">Browser notifications are allowed: with mega open in the background, a click on one takes you to the room.</p>';
    if (p === 'denied') return '<p class="muted small">This browser blocks mega\'s own notifications. Your system\'s still work.</p>';
    return '<div class="ntf-ask"><button type="button" class="btn ghost slim" id="ntf-allow">Allow notifications in this browser</button></div><p class="muted small">With mega open in the background, a click on a notification then takes you to the room. Without it, your system shows them.</p>';
  };
  const body = () => `<label>Notifications</label>${seg('ntf', [['all', 'All messages'], ['mentions', 'Only @mentions and replies'], ['off', 'Off']], N.mode)}
  <label class="check"><input type="checkbox" id="ntf-text" ${N.show_text ? 'checked' : ''}><span>Show message text in notifications</span></label>
  <p class="muted small">Shown even when mega is closed, as long as it runs in the background. Off by default: a notification then says only who wrote and where. Mute a single room by right-clicking it (touch: hold it). Do Not Disturb on your computer is respected. Nothing goes through anyone's servers.</p>
  <div id="ntf-perm">${permLine()}</div>`;
  const settingsHTML = () => `<h3 class="set-h">Notifications</h3>${body()}`;
  const optsHTML = () => `<section><h3>notifications</h3>${body()}</section>`;
  // the options room applies a change at once
  async function set(o) { try { apply(await rpc_('notify_set', o)); } catch (e) { toast('Could not change that: ' + ((e && e.message) || e)); } }
  async function save(w) {
    const mode = $('#ntf', w), txt = $('#ntf-text', w); if (!mode || !txt) return;
    if (mode.value !== N.mode || txt.checked !== N.show_text) apply(await rpc_('notify_set', { mode: mode.value, show_text: txt.checked }));
  }
  document.addEventListener('click', e => {
    if (!e.target.closest('#ntf-allow')) return;
    ask().then(() => { const box = $('#ntf-perm'); if (box) box.innerHTML = permLine(); });
  });

  // ---------------------------------------------------------------- the room menu
  let menuEl = null, menuClose = null;
  function closeMenu() { if (menuClose) menuClose(); }
  const OPTIONS = [['mute', 3600, 'Mute for 1 hour', '1h'], ['mute', 28800, 'Mute for 8 hours', '8h'], ['mute', 0, 'Mute until I unmute', 'forever'], ['mentions', 0, 'Mentions only', 'mentions']];
  function openMenu(t, px, py) {
    closeMenu();
    const cur = rule(t.gid), curKey = !cur ? '' : cur.kind === 'muted' ? (cur.until ? '' : 'forever') : 'mentions';
    const m = document.createElement('div'); m.className = 'nm-menu'; m.setAttribute('role', 'menu'); m.setAttribute('aria-label', 'Notifications for ' + t.name);
    m.innerHTML = `<div class="nm-h">${esc(t.name)}</div>` +
      OPTIONS.map(([kind, secs, label, key]) => `<button type="button" role="menuitemradio" aria-checked="${curKey === key}" data-kind="${kind}" data-secs="${secs}">${esc(label)}</button>`).join('') +
      (cur ? '<button type="button" role="menuitem" data-kind="none" class="nm-un">Unmute</button>' : '') +
      (t.paint ? '<hr><button type="button" role="menuitem" data-kind="paint">Colours…</button>' : '');
    if (cur && cur.kind === 'muted' && cur.until) m.insertAdjacentHTML('beforeend', `<div class="nm-n">Muted until ${esc(new Date(cur.until * 1000).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }))}</div>`);
    document.body.append(m); menuEl = m;
    const r = m.getBoundingClientRect();
    m.style.left = Math.max(8, Math.min(px, innerWidth - r.width - 8)) + 'px';
    m.style.top = Math.max(8, Math.min(py, innerHeight - r.height - 8)) + 'px';
    const btns = [...m.querySelectorAll('button')]; (btns.find(b => b.getAttribute('aria-checked') === 'true') || btns[0]).focus();
    const onDown = ev => { if (!m.contains(ev.target)) closeMenu(); };
    const onKey = ev => {
      if (ev.key === 'Escape') { ev.preventDefault(); closeMenu(); return; }
      if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') { ev.preventDefault(); const i = btns.indexOf(document.activeElement); btns[(i + (ev.key === 'ArrowDown' ? 1 : btns.length - 1)) % btns.length].focus(); }
    };
    setTimeout(() => { document.addEventListener('pointerdown', onDown, true); }, 0);
    document.addEventListener('keydown', onKey, true); window.addEventListener('blur', closeMenu); window.addEventListener('resize', closeMenu);
    menuClose = () => { m.remove(); menuEl = null; menuClose = null; document.removeEventListener('pointerdown', onDown, true); document.removeEventListener('keydown', onKey, true); window.removeEventListener('blur', closeMenu); window.removeEventListener('resize', closeMenu); };
    m.addEventListener('click', async ev => {
      const b = ev.target.closest('button'); if (!b) return;
      const kind = b.dataset.kind; closeMenu();
      if (kind === 'paint') { const x = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: px, clientY: py }); x.__nm = true; t.el.dispatchEvent(x); return; }
      try { apply(await rpc_('notify_room', { gid: t.gid, kind, secs: +b.dataset.secs || 0 })); } catch (e) { toast('Could not change that: ' + ((e && e.message) || e)); }
    });
  }
  // The room under an element: a dot in mega's sidebar, a card, or a row of the old room list.
  function target(el) {
    if (!el || !el.closest) return null;
    const M = window.__mega, canPaint = x => !!(M && M.canPaint && M.canPaint(x));
    const dot = el.closest('#mega-rail .mitem[data-key]');
    if (dot && M) { const x = (M.railItems || []).find(k => k.key === dot.dataset.key); return x && x.kind === 'text' ? { gid: x.gid, name: x.name, paint: canPaint(x), el: dot } : null; }
    const slot = el.closest('#mega .mslot[data-i]');
    if (slot && M && M.list) { const x = M.list.items[+slot.dataset.i]; return x && x.kind === 'text' ? { gid: x.gid, name: x.name, paint: false, el: slot } : null; }
    const row = el.closest('.room[data-g]');
    if (row && S.snap) { const gid = row.dataset.g; for (const c of S.snap.communities) { const ch = c.channels.find(k => k.gid === gid); if (ch && ch.kind !== 'voice') return { gid, name: ch.name, paint: false, el: row }; } }
    return null;
  }
  document.addEventListener('contextmenu', e => {
    if (e.__nm) return;
    const t = target(e.target); if (!t) return;
    e.preventDefault(); e.stopPropagation(); openMenu(t, e.clientX, e.clientY);
  }, true);
  // Touch: hold a room. A phone's browser may also send a contextmenu event for the same hold; the menu is already open then.
  let hold = null, held = 0;
  document.addEventListener('pointerdown', e => {
    if (e.pointerType !== 'touch' && e.pointerType !== 'pen') return;
    const t = target(e.target); if (!t) return;
    const sx = e.clientX, sy = e.clientY;
    clearTimeout(hold);
    const move = ev => { if (Math.abs(ev.clientX - sx) > 10 || Math.abs(ev.clientY - sy) > 10) stop(); };
    const stop = () => { clearTimeout(hold); hold = null; document.removeEventListener('pointermove', move, true); document.removeEventListener('pointerup', stop, true); document.removeEventListener('pointercancel', stop, true); };
    document.addEventListener('pointermove', move, true); document.addEventListener('pointerup', stop, true); document.addEventListener('pointercancel', stop, true);
    hold = setTimeout(() => { stop(); held = Date.now(); if (!menuEl) openMenu(t, sx, sy); }, 550);
  }, true);
  // The click that ends a hold must not also open the room.
  document.addEventListener('click', e => { if (held && Date.now() - held < 900 && !e.target.closest('.nm-menu')) { held = 0; e.preventDefault(); e.stopImmediatePropagation(); } }, true);

  window.__megaNotify = {
    state, soundOk, mark: markFor, settingsHTML, optsHTML, set, save, ask, apply, beat, open: openRoomAnywhere,
    on: f => N.subs.push(f), get rules() { return N; }, get shown() { return shown; },
  };
})();
