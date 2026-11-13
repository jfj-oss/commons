'use strict';
// mega: listening along with a friend's Spotify song.
//
// A friend who shares what they listen to may share a Spotify song with its position (the core reads it from rune). A small note
// under their picture in People says so; its tooltip is the song. A click on it makes the SAME song play on THIS person's own
// player, in step, and follows pauses, seeks and new songs; the note is then filled, and a second click leaves. The player is
// picked by the core: this person's rune when one answers (its own engine, PC speakers or phone), otherwise their own Spotify
// account (a login kept only on this device, see crates/core/src/spotify.rs); with neither, a short notice says why. mega never
// captures or forwards Spotify audio. The note is the only control: there is no menu of players and no strip.
(() => {
  const L = { st: { state: 'off' }, timer: 0 };
  const esc = s => String(s == null ? '' : s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const say = m => { try { toast(m); } catch { } };
  const R = (m, a) => rpc(m, a || {});
  const phone = () => { try { return !!(typeof mobileApp === 'function' && mobileApp()); } catch { return false; } };
  const NOTE = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M6 12.2V3.4l7-1.6v8.6" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/><ellipse cx="4.2" cy="12.2" rx="2" ry="1.6" fill="currentColor"/><ellipse cx="11.2" cy="10.4" rx="2" ry="1.6" fill="currentColor"/></svg>';

  function css() {
    if (document.getElementById('mega-listen-css')) return;
    const s = document.createElement('style'); s.id = 'mega-listen-css';
    s.textContent = `
      .person:has(.ml-note) { padding-bottom: 20px; }
      .ml-note { position: absolute; left: 50%; top: calc(100% + 3px); transform: translateX(-50%); width: 20px; height: 16px; padding: 0; display: grid; place-items: center; border: 0; border-radius: 8px; background: none; color: var(--m-paper, var(--ink-2)); opacity: .8; cursor: pointer; -webkit-tap-highlight-color: transparent; }
      .ml-note::before { content: ""; position: absolute; inset: -6px -8px; }
      .ml-note svg { width: 13px; height: 13px; display: block; }
      .ml-note:hover, .ml-note:focus-visible, .ml-note.on { opacity: 1; }
      .ml-note.on { background: var(--m-speak, var(--ok)); color: #04130e; }
    `;
    document.head.appendChild(s);
  }
  css();

  // ------------------------------------------------------------- the note under a person's picture
  const songOf = l => (l.artist ? l.artist + ' \u2013 ' : '') + l.title;
  /** For one person's row in People: the note, when they share a Spotify song that can be joined. Filled while we listen along with them. */
  function noteHtml(m) {
    const sp = m && !m.me && m.listening && m.listening.spotify;
    if (!sp) return '';
    const on = L.st.state === 'on' && L.st.acct === m.account, song = songOf(m.listening);
    return `<button type="button" class="ml-note${on ? ' on' : ''}" data-ml="toggle" data-acct="${esc(m.account)}" aria-pressed="${on}" title="${esc(song)}" aria-label="${esc((on ? 'Stop listening along with ' : 'Listen along with ') + m.name + ': ' + song)}">${NOTE}</button>`;
  }
  const redraw = () => { try { if (S.people) renderPeople(); } catch { } };
  /** Why nothing here can play it: said once, in a notice. */
  function why(st) {
    if (phone()) return 'Could not listen along: no rune answers from this phone.';
    if (st.needs === 'login') return 'Could not listen along: rune is not answering and Spotify is not logged in (see Settings).';
    return 'Could not listen along: rune is not answering and Spotify is not set up (see Settings).';
  }
  async function join(acct) {
    try {
      // No target: the core takes this person's rune if one answers, otherwise their own Spotify.
      const st = await R('listen_join', { account: acct, target: '' });
      if (st.state === 'on') { L.st = st; start(); }
      else { say(why(st)); await R('listen_leave').catch(() => { }); L.st = { state: 'off' }; }
    } catch (e) { say(e.message); }
    redraw();
  }
  async function leave() {
    try { await R('listen_leave'); } catch { }
    L.st = { state: 'off' }; stop(); redraw();
  }
  async function poll() {
    try {
      const s = await R('listen_status');
      if (s && s.state === 'on') { if (JSON.stringify(s) !== JSON.stringify(L.st)) { const was = L.st; L.st = s; if (was.state !== 'on' || was.acct !== s.acct) redraw(); } return; }
      // it ended by itself (the friend stopped, changed to something that cannot be joined, the player went away): say why, once
      if (s && s.state === 'ended' && s.message) say(s.message);
      if (s && s.state !== 'off') await R('listen_leave').catch(() => { });
      L.st = { state: 'off' }; stop(); redraw();
    } catch { }
  }
  function start() { if (!L.timer) L.timer = setInterval(poll, 2000); }
  function stop() { clearInterval(L.timer); L.timer = 0; }

  function openLink(url) {
    if (!url) return;
    if (typeof T !== 'undefined' && T && T.core) R('open_link', { url }).then(r => { if (!r.opened) say('Could not start a browser'); }).catch(e => say(e.message));
    else window.open(url, '_blank', 'noopener');
  }

  // ------------------------------------------------------------- Settings
  /** The Spotify block in Settings: the client ID of the person's own Spotify app, and the login. */
  async function fillSettings(w) {
    const row = w.querySelector('#sp-row'); if (!row) return;
    if (phone()) { row.innerHTML = '<p class="muted small">Listening along on this phone plays through rune. Logging in to Spotify is not offered here.</p>'; return; }
    let cfg; try { cfg = await R('spotify_config'); } catch { return; }
    const redirect = location.origin + '/spotify/callback';
    row.innerHTML = `<label for="sp-id">Spotify app client ID (to listen along on Spotify)</label>
      <div style="display:flex;gap:8px"><input type="text" id="sp-id" value="${esc(cfg.client_id)}" spellcheck="false" autocomplete="off" style="flex:1"><button type="button" class="btn ghost" id="sp-save">Save</button></div>
      <p class="muted small">Make a free app at developer.spotify.com for mega, add <b>${esc(redirect)}</b> as its redirect address, and paste its client ID here. It has no secret. With rune you do not need this.</p>
      <div id="sp-state"></div>`;
    const draw = c => {
      const box = row.querySelector('#sp-state');
      box.innerHTML = !c.effective ? '<p class="muted small">Not set up: listening along plays through rune only.</p>'
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
    const b = e.target.closest('[data-ml="toggle"]'); if (!b) return;
    e.preventDefault(); e.stopPropagation();
    if (L.st.state === 'on' && L.st.acct === b.dataset.acct) leave(); else join(b.dataset.acct);
  });

  // pick up a session that is already running (the page was reloaded); one that is over is put away
  setTimeout(() => R('listen_status').then(s => { if (s && s.state === 'on') { L.st = s; start(); redraw(); } else if (s && s.state && s.state !== 'off') R('listen_leave').catch(() => { }); }).catch(() => { }), 1500);

  window.__megaListen = { noteHtml, fillSettings, state: () => L.st };
})();
