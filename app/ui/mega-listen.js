'use strict';
// mega: listening along with a friend's Spotify song.
//
// A friend who shares what they listen to may share a Spotify song with its position (the core reads it from rune). "Listen along"
// on their row in People makes the SAME song play on THIS person's own player, synced, and follows pauses, seeks and new songs:
// through their rune when one answers (its own engine, PC speakers or phone), otherwise through their own Spotify account (a login
// kept only on this device, see crates/core/src/spotify.rs). mega never captures or forwards Spotify audio. All the deciding and all
// the talking to rune and Spotify happens in the core; this file only shows the state and sends Join and Leave.
(() => {
  const L = { st: { state: 'off' }, timer: 0, menu: null, cfg: null };
  const esc = s => String(s == null ? '' : s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const say = m => { try { toast(m); } catch { } };
  const R = (m, a) => rpc(m, a || {});
  const phone = () => { try { return !!(typeof mobileApp === 'function' && mobileApp()); } catch { return false; } };

  function css() {
    if (document.getElementById('mega-listen-css')) return;
    const s = document.createElement('style'); s.id = 'mega-listen-css';
    s.textContent = `
      .ml-btn { margin-top: 4px; font: 600 12px var(--m-ui, system-ui); padding: 3px 11px; border-radius: 13px; border: 1px solid currentColor; background: none; color: inherit; cursor: pointer; opacity: .85; }
      .ml-btn:hover, .ml-btn:focus-visible { opacity: 1; }
      .ml-link { margin-left: 8px; font-size: 12px; color: inherit; }
      #ml-strip { position: fixed; z-index: 30; left: 50%; bottom: 16px; transform: translateX(-50%); max-width: min(92vw, 560px); display: flex; gap: 10px; align-items: center; padding: 8px 8px 8px 16px; border-radius: 22px; background: var(--m-glass, rgb(26 24 32 / .92)); color: var(--m-paper, #efedf2); border: 1px solid var(--m-line, rgb(255 255 255 / .14)); font: 14px/1.3 var(--m-ui, system-ui); box-shadow: 0 6px 24px rgb(0 0 0 / .35); }
      #ml-strip[hidden] { display: none; }
      #ml-strip .ml-t { min-width: 0; overflow-wrap: anywhere; }
      #ml-strip .ml-s { display: block; font-size: 12px; opacity: .72; }
      #ml-strip button { font: 600 13px var(--m-ui, system-ui); padding: 6px 14px; border-radius: 16px; border: 1px solid var(--m-line, rgb(255 255 255 / .25)); background: none; color: inherit; cursor: pointer; white-space: nowrap; }
      #ml-strip a { color: inherit; white-space: nowrap; font-size: 13px; }
      #ml-menu { position: fixed; z-index: 40; display: grid; gap: 4px; padding: 8px; border-radius: 14px; background: var(--m-glass, rgb(26 24 32 / .97)); color: var(--m-paper, #efedf2); border: 1px solid var(--m-line, rgb(255 255 255 / .2)); font: 14px var(--m-ui, system-ui); box-shadow: 0 6px 24px rgb(0 0 0 / .4); }
      #ml-menu button { text-align: left; font: inherit; padding: 7px 12px; border: 0; border-radius: 9px; background: none; color: inherit; cursor: pointer; }
      #ml-menu button:hover, #ml-menu button:focus-visible { background: rgb(255 255 255 / .1); }
    `;
    document.head.appendChild(s);
  }

  // ------------------------------------------------------------- the People row
  /** The Join button (or, with no way to play it here, a link to open the song in Spotify), for one person's row. */
  function joinHtml(m) {
    const sp = m && !m.me && m.listening && m.listening.spotify;
    if (!sp) return '';
    const on = L.st.state === 'on' && L.st.acct === m.account;
    if (on) return `<button type="button" class="ml-btn" data-ml="leave">Leave</button>`;
    const link = sp.link ? ` <a class="ml-link" data-ml="open" href="#" data-url="${esc(sp.link)}">Open in Spotify</a>` : '';
    return `<button type="button" class="ml-btn" data-ml="join" data-acct="${esc(m.account)}" data-n="${esc(m.name)}" title="Play the same song on your own player, in step with ${esc(m.name)}">Listen along</button>${link}`;
  }

  function openLink(url) {
    if (!url) return;
    if (typeof T !== 'undefined' && T && T.core) R('open_link', { url }).then(r => { if (!r.opened) say('Could not start a browser'); }).catch(e => say(e.message));
    else window.open(url, '_blank', 'noopener');
  }

  function closeMenu() { if (L.menu) { L.menu.remove(); L.menu = null; } }
  async function join(acct, name, anchor) {
    closeMenu();
    let t = null;
    try { t = await R('listen_targets'); } catch { }
    const choices = [];
    if (t) {
      for (const z of t.zones || []) choices.push({ label: 'On rune: ' + z.name, target: 'rune:' + z.id });
      if (t.spotify) choices.push({ label: 'On my Spotify', target: 'spotify' });
    }
    if (choices.length > 1 && anchor) {
      const menu = document.createElement('div'); menu.id = 'ml-menu'; menu.setAttribute('role', 'menu');
      menu.innerHTML = choices.map(c => `<button type="button" role="menuitem" data-target="${esc(c.target)}">${esc(c.label)}</button>`).join('');
      const r = anchor.getBoundingClientRect();
      menu.style.left = Math.max(8, Math.min(r.left, innerWidth - 260)) + 'px'; menu.style.top = Math.min(r.bottom + 4, innerHeight - 40 - choices.length * 34) + 'px';
      menu.addEventListener('click', e => { const b = e.target.closest('button[data-target]'); if (b) { closeMenu(); go(acct, b.dataset.target); } });
      document.body.appendChild(menu); L.menu = menu; const f = menu.querySelector('button'); if (f) f.focus();
      return;
    }
    go(acct, choices.length ? choices[0].target : '');
  }
  async function go(acct, target) {
    try {
      const st = await R('listen_join', { account: acct, target });
      L.st = st; render(); start();
      if (st.state === 'needs' && st.needs === 'login') say('Log in to Spotify in Settings to listen along here.');
    } catch (e) { say(e.message); }
  }
  async function leave() {
    try { await R('listen_leave'); } catch { }
    L.st = { state: 'off' }; render(); stop();
  }

  // ------------------------------------------------------------- the strip
  function strip() {
    let el = document.getElementById('ml-strip');
    if (!el) { css(); el = document.createElement('div'); el.id = 'ml-strip'; el.hidden = true; el.setAttribute('role', 'status'); document.body.appendChild(el); }
    return el;
  }
  function render() {
    const s = L.st, el = strip();
    if (!s || s.state === 'off') { el.hidden = true; el.innerHTML = ''; return; }
    const link = s.open_url ? `<a href="#" data-ml="open" data-url="${esc(s.open_url)}">Open in Spotify</a>` : '';
    let main = '', sub = '', btn = '<button type="button" data-ml="leave">Leave</button>';
    if (s.state === 'on') {
      main = `Listening along with ${esc(s.who)} on ${esc(s.target)}`;
      sub = esc(s.message || (s.paused ? 'Paused' : s.title));
    } else if (s.state === 'ended') {
      main = `Ended: listening along with ${esc(s.who)}`; sub = esc(s.message || ''); btn = '<button type="button" data-ml="dismiss">Close</button>';
    } else if (s.state === 'needs') {
      main = esc(s.message || 'Open the song in Spotify to listen.'); sub = esc(s.who ? s.who + ' is listening to ' + s.title : ''); btn = '<button type="button" data-ml="dismiss">Close</button>';
    }
    el.innerHTML = `<span class="ml-t">${main}<span class="ml-s">${sub}</span></span>${link}${btn}`;
    el.hidden = false;
  }
  async function poll() {
    try {
      const s = await R('listen_status');
      if (JSON.stringify(s) !== JSON.stringify(L.st)) { L.st = s; render(); try { if (S.people) renderPeople(); } catch { } }
      if (!s || s.state !== 'on') stop();
    } catch { }
  }
  function start() { if (!L.timer) L.timer = setInterval(poll, 2000); }
  function stop() { clearInterval(L.timer); L.timer = 0; }

  // ------------------------------------------------------------- Settings
  /** The Spotify block in Settings: the client ID of the person's own Spotify app, and the login. */
  async function fillSettings(w) {
    const row = w.querySelector('#sp-row'); if (!row) return;
    if (phone()) { row.innerHTML = '<p class="muted small">Listening along on this phone opens the song in Spotify. Logging in is not offered here.</p>'; return; }
    let cfg; try { cfg = await R('spotify_config'); } catch { return; }
    const redirect = location.origin + '/spotify/callback';
    row.innerHTML = `<label for="sp-id">Spotify app client ID (to listen along on Spotify)</label>
      <div style="display:flex;gap:8px"><input type="text" id="sp-id" value="${esc(cfg.client_id)}" spellcheck="false" autocomplete="off" style="flex:1"><button type="button" class="btn ghost" id="sp-save">Save</button></div>
      <p class="muted small">Make a free app at developer.spotify.com for mega, add <b>${esc(redirect)}</b> as its redirect address, and paste its client ID here. It has no secret. With rune you do not need this.</p>
      <div id="sp-state"></div>`;
    const draw = c => {
      const box = row.querySelector('#sp-state');
      box.innerHTML = !c.effective ? '<p class="muted small">Not set up: listening along will offer Open in Spotify.</p>'
        : c.logged_in ? '<p class="small">Logged in to Spotify on this device. <button type="button" class="btn ghost" id="sp-out">Log out</button></p>'
        : '<p class="small"><button type="button" class="btn ghost" id="sp-in">Log in to Spotify</button></p>';
    };
    draw(cfg);
    row.addEventListener('click', async e => {
      const id = e.target.id;
      try {
        if (id === 'sp-save') { await R('spotify_set_client_id', { id: row.querySelector('#sp-id').value }); say('Saved'); }
        else if (id === 'sp-out') { await R('spotify_logout'); }
        else if (id === 'sp-in') {
          const r = await R('spotify_login_start', { redirect });
          openLink(r.url);
          const t0 = Date.now(); const iv = setInterval(async () => { const c = await R('spotify_config').catch(() => null); if (c && c.logged_in) { clearInterval(iv); draw(c); say('Spotify is connected'); } else if (Date.now() - t0 > 600000 || !document.body.contains(row)) clearInterval(iv); }, 2000);
          return;
        } else return;
        draw(await R('spotify_config'));
      } catch (er) { say(er.message); }
    });
  }

  document.addEventListener('click', e => {
    const b = e.target.closest('[data-ml]'); if (!b) return;
    e.preventDefault();
    const a = b.dataset.ml;
    if (a === 'join') join(b.dataset.acct, b.dataset.n || '', b);
    else if (a === 'leave') leave();
    else if (a === 'dismiss') { R('listen_leave').catch(() => { }); L.st = { state: 'off' }; render(); }
    else if (a === 'open') openLink(b.dataset.url);
  });
  document.addEventListener('click', e => { if (L.menu && !e.target.closest('#ml-menu') && !e.target.closest('[data-ml="join"]')) closeMenu(); }, true);
  document.addEventListener('keydown', e => { if (e.key === 'Escape') closeMenu(); });

  // pick up a session that is already running (the page was reloaded)
  setTimeout(() => R('listen_status').then(s => { if (s && s.state && s.state !== 'off') { L.st = s; render(); if (s.state === 'on') start(); } }).catch(() => { }), 1500);

  window.__megaListen = { joinHtml, fillSettings, state: () => L.st };
})();
