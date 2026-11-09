'use strict';
// Calls in the app: while you are in one, a compact dock sits at the bottom of the sidebar (a slim bar at the bottom of
// the chat on a phone), with the status, a lock that explains what the call server can see, and one row of icon buttons.
// The people are the ones under the room in the rail already. Screen shares, cameras and played videos still open large
// in the main area. The device settings and "play into the call" pickers keep their dialogs.
// Depends on app.js globals (S, rpc, esc, toast) and call.js.
(function () {
  if (!window.CommonsCall) return;
  const call = new CommonsCall({ rpc });
  // own icons, same stroke style as the app's sprite (ids are prefixed c- so they cannot clash)
  if (!document.getElementById('call-sprite')) {
    document.body.insertAdjacentHTML('beforeend', `<svg id="call-sprite" width="0" height="0" aria-hidden="true" focusable="false"><g fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
      <symbol id="ci-mic" viewBox="0 0 20 20"><rect x="7.5" y="2.5" width="5" height="9" rx="2.5"/><path d="M4.5 9.5a5.5 5.5 0 0 0 11 0M10 15v2.5"/></symbol>
      <symbol id="ci-micoff" viewBox="0 0 20 20"><path d="M3 3l14 14M7.5 7.5v1a2.5 2.5 0 0 0 4 2M12.5 8V5a2.5 2.5 0 0 0-5-.2M4.5 9.5a5.5 5.5 0 0 0 8.4 4.7M15.5 9.5c0 .8-.2 1.5-.5 2.2M10 15v2.5"/></symbol>
      <symbol id="ci-phones" viewBox="0 0 20 20"><path d="M3.5 12V10a6.5 6.5 0 0 1 13 0v2"/><rect x="2.5" y="11.5" width="3.5" height="5" rx="1.2"/><rect x="14" y="11.5" width="3.5" height="5" rx="1.2"/></symbol>
      <symbol id="ci-screen" viewBox="0 0 20 20"><rect x="2.5" y="3.5" width="15" height="10" rx="1.6"/><path d="M7 17h6M10 13.5V17"/></symbol>
      <symbol id="ci-cam" viewBox="0 0 20 20"><rect x="2.5" y="5.5" width="10.5" height="9" rx="1.8"/><path d="m13 9 4.5-2.5v7L13 11"/></symbol>
      <symbol id="ci-leave" viewBox="0 0 20 20"><path d="M3.5 12.5c4-3.5 9-3.5 13 0l-1.2 2.2-2.8-.9-.3-2c-1.5-.5-3-.5-4.4 0l-.3 2-2.8.9z"/></symbol>
      <symbol id="ci-gear" viewBox="0 0 20 20"><path d="M3 6h7.5M15.5 6H17M3 14h1.5M9.5 14H17"/><circle cx="13" cy="6" r="2.3"/><circle cx="7" cy="14" r="2.3"/></symbol>
      <symbol id="ci-wave" viewBox="0 0 20 20"><path d="M3 10v0M6 7v6M9 4.5v11M12 7v6M15 8.5v3M18 10v0"/></symbol>
      <symbol id="ci-film" viewBox="0 0 20 20"><rect x="2.5" y="4" width="15" height="12" rx="1.6"/><path d="M2.5 8h15M6.5 4v4M10 4v4M13.5 4v4"/></symbol>
      <symbol id="ci-lock" viewBox="0 0 20 20"><rect x="4.5" y="9" width="11" height="8" rx="2"/><path d="M7 9V6.5a3 3 0 0 1 6 0V9"/></symbol>
      <symbol id="ci-note" viewBox="0 0 20 20"><path d="M7.5 15.5V5l8-1.8v10.3"/><circle cx="5.2" cy="15.5" r="2.3"/><circle cx="13.2" cy="13.5" r="2.3"/></symbol>
      <symbol id="ci-play" viewBox="0 0 20 20"><path d="M7 4.5l8 5.5-8 5.5z"/></symbol>
      <symbol id="ci-pause" viewBox="0 0 20 20"><path d="M7 4.5v11M13 4.5v11"/></symbol>
      <symbol id="ci-stop" viewBox="0 0 20 20"><rect x="5.5" y="5.5" width="9" height="9" rx="1"/></symbol>
      <symbol id="ci-prev" viewBox="0 0 20 20"><path d="M6 4.5v11M15 5l-7 5 7 5z"/></symbol>
      <symbol id="ci-next" viewBox="0 0 20 20"><path d="M14 4.5v11M5 5l7 5-7 5z"/></symbol>
    </g></svg>`);
  }
  const ic = n => `<svg class="ic"><use href="#${n}"/></svg>`;
  // Handing a call to the browser is a desktop escape hatch (WebKitGTK without WebRTC). A phone has no second
  // browser to hand it to, and Safari is already the engine underneath.
  const desktopApp = () => !!window.__TAURI__ && window.__commonsPlatform !== 'mobile';
  window.__commonsCall = call;               // used by the headless tests
  const $q = (s, r = document) => r.querySelector(s);
  let devices = null, showSettings = false;
  let playDlg = null, playItems = null, playBusy = false;
  let runeItems = null, runeBusy = false;
  let qcSession = null;   // Jellyfin Quick Connect while a code is on screen: { secret, code, busy, error, timer }
  let dock = null, stage = null, settingsDlg = null, pop = null;
  const fmtTime = s => { s = Math.max(0, Math.floor(Number(s) || 0)); const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), x = s % 60; return (h ? h + ':' + String(m).padStart(2, '0') : String(m)) + ':' + String(x).padStart(2, '0'); };
  // Video tiles live in their own host that no render ever rewrites. Each tile (one per video track: a camera and a screen
  // share of the same person are two tiles) is created once, updated in place, and removed when its track is muted or gone.
  // Moving or re-creating a <video> on every state update made the pictures flicker.
  const tiles = new Map();
  const isPhone = () => window.matchMedia && window.matchMedia('(max-width: 760px)').matches;

  function currentRoom() {
    try {
      const c = S.snap && S.snap.communities.find(x => x.id === S.cid);
      const ch = c && c.channels.find(x => x.gid === S.gid);
      return ch ? { gid: ch.gid, name: ch.name, community: c.name, joined: ch.joined } : null;
    } catch { return null; }
  }

  // ---- the room header's Call button: it joins this room's call (there is no panel to open any more) ----
  function ensureButton() {
    const bar = $q('.head-actions');
    if (!bar || $q('#btn-call')) return;
    const b = document.createElement('button'); b.id = 'btn-call'; b.className = 'btn ghost'; b.innerHTML = ic('ci-wave') + '<span class="lbl">Call</span>'; b.title = 'Voice and screen share in this room (end-to-end encrypted)';
    b.onclick = async () => {
      const room = currentRoom(); if (!room) return;
      if (call.gid === room.gid && call.phase !== 'idle' && call.phase !== 'error') return;   // already here; the dock is up
      try { await call.join(room.gid, room.name); } catch { }
      render();
    };
    bar.insertBefore(b, $q('#btn-invite', bar) || bar.firstChild);
  }
  function updateButton() {
    const b = $q('#btn-call'); if (!b) return;
    const room = currentRoom();
    b.hidden = !room || !room.joined;
    const live = call.phase === 'connected' || call.phase === 'reconnecting' || call.phase === 'connecting';
    b.classList.toggle('live', live);
    b.innerHTML = ic('ci-wave') + '<span class="lbl">' + (live ? (call.gid === (room && room.gid) ? 'In call' : 'In call elsewhere') : 'Call') + '</span>';
  }

  // ---------------------------------------------------------------- the dock, in the rail (or the chat on a phone)
  function ensureDock() {
    if (dock) return dock;
    dock = document.getElementById('call-dock');
    if (!dock) {
      dock = document.createElement('div'); dock.id = 'call-dock'; dock.hidden = true;
      (document.getElementById('rail') || document.body).appendChild(dock);
    }
    placeDock();
    wireDock();
    return dock;
  }
  // The rail is a drawer on a phone, so the same dock lives in the chat there: always reachable, no drawer needed.
  function placeDock() {
    if (!dock) return;
    const rail = document.getElementById('rail'), main = document.getElementById('main');
    // mega's card screens cover the chat on a phone, so there the dock floats on its own above them.
    const root = document.documentElement, mega = root.classList.contains('mega');
    const cards = mega && (!isPhone() || root.classList.contains('mg-cards'));   // on a computer mega keeps it as a strip on the right, everywhere
    root.classList.toggle('mg-dock', mega && !isPhone() && !dock.hidden);
    if (mega && window.__mega && __mega.callTheme && call && call.gid) {
      const t = __mega.callTheme(call.gid);
      if (t) { dock.style.setProperty('--dk-main', t[0]); dock.style.setProperty('--dk-hi', t[1]); dock.style.setProperty('--dk-ink', __mega.inkOn(t[0])); dock.style.setProperty('--dk-hink', __mega.inkOn(t[1])); }
    }
    if (isPhone() && main && !cards) {
      if (dock.parentElement !== main) main.insertBefore(dock, document.getElementById('composer') || null);
      dock.classList.remove('standalone');
    } else if (rail && !cards) {
      if (dock.parentElement !== rail) rail.insertBefore(dock, document.getElementById('sync-status') || null);
      dock.classList.remove('standalone');
    } else {
      if (dock.parentElement !== document.body) document.body.appendChild(dock);
      dock.classList.add('standalone');   // the stand-alone browser call page has no rail or chat
    }
  }
  window.__placeCallDock = placeDock;
  // A phone has a whole row of controls beside the words, so say the shorter thing there.
  const statusWords = s => s.phase === 'connecting' ? 'Connecting…' : s.phase === 'reconnecting' ? 'Reconnecting…' : s.phase === 'error' ? 'Call ended' : isPhone() ? 'Connected' : 'Voice connected';
  function dockButtons(s, sup) {
    const muted = s.muted || s.gameMuted;
    const btn = (id, icon, label, on, extra = '') => `<button type="button" class="cd-b${on ? ' on' : ''}" id="${id}" title="${esc(label)}" aria-label="${esc(label)}" aria-pressed="${on ? 'true' : 'false'}"${extra}>${ic(icon)}</button>`;
    const shareLabel = s.sharing ? 'Stop sharing your screen' : sup.canShareScreen ? 'Share your screen' : 'This browser or webview cannot share a screen';
    const playLabel = s.playback ? 'A video is already playing; stop it first' : s.sharing ? 'Stop sharing your screen first' : 'Play a video into the call';
    return `<div class="cd-acts">
      ${btn('c-mute', muted ? 'ci-micoff' : 'ci-mic', muted ? (s.gameMuted && !s.muted ? 'Muted by game key: unmute' : 'Unmute') : 'Mute', muted)}
      ${btn('c-deaf', 'ci-phones', s.deafened ? 'Undeafen' : 'Deafen', s.deafened)}
      ${btn('c-share', 'ci-screen', shareLabel, s.sharing, sup.canShareScreen ? '' : ' disabled')}
      ${btn('c-playopen', 'ci-film', playLabel, false, s.playback || s.sharing ? ' disabled' : '')}
      ${btn('c-cam', 'ci-cam', s.camera ? 'Turn the camera off' : 'Turn the camera on', s.camera)}
      ${btn('c-set', 'ci-gear', 'Call settings', showSettings)}
      ${s.ptt ? `<button type="button" class="cd-b${s.pttHeld ? ' on' : ''}" id="c-ptt" title="Hold to talk (or hold your push-to-talk key)" aria-label="Hold to talk">${ic('ci-mic')}</button>` : ''}
      <button type="button" class="cd-b leave" id="c-leave" title="Leave the call" aria-label="Leave the call">${ic('ci-leave')}</button>
    </div>`;
  }
  function dockHtml(s, sup) {
    const room = currentRoom();
    const name = s.roomName || (room && room.name) || 'Call';
    const dot = s.phase === 'connected' ? 'ok' : s.phase === 'error' ? 'bad' : 'wait';
    let notes = '';
    if (s.error) notes += `<div class="cd-err" role="status">${esc(s.error.msg)}</div>`;
    else if (!sup.ok) notes += `<div class="cd-err">Calls are not available in this window.</div>`;
    if (!sup.ok && desktopApp() && room) notes += `<div class="cd-row"><button type="button" class="btn ghost" id="c-browser">Open this call in my browser</button></div>`;
    if (s.gameMuted) notes += `<div class="cd-note">Muted by game key${s.gameName ? ' (' + esc(s.gameName) + ')' : ''}. Friends see you as muted.</div>`;
    else if (s.gameError) notes += `<div class="cd-note">Game keys are not working: ${esc(s.gameError)}</div>`;
    if (s.blocked.length) notes += `<div class="cd-note bad">Blocked ${s.blocked.length} stream(s) that are not from a current member or not encrypted.</div>`;
    return `<div class="cd-top"><span class="cd-dot ${dot}"></span><span class="cd-phase">${esc(statusWords(s))}</span><span class="cd-dot-sep">·</span><span class="cd-room" title="${esc(name)}">${esc(name)}</span>${encBadge(s)}</div>${notes}${dockButtons(s, sup)}`;
  }
  function renderDock(s, sup) {
    ensureDock();
    placeDock();
    const inCall = s.phase !== 'idle';
    dock.hidden = !inCall;
    if (!inCall) { closePop(); if (showSettings) { showSettings = false; renderSettings(s); } return; }
    const html = dockHtml(s, sup);
    if (dock.__html !== html) { dock.innerHTML = html; dock.__html = html; }
  }
  let dockWired = false, swallowClick = false;
  function wireDock() {
    if (dockWired) return; dockWired = true;
    dock.addEventListener('click', e => {
      const b = e.target.closest('button'); if (!b || !dock.contains(b)) return;
      switch (b.id) {
        case 'c-mute':
          if (call.gameMuted && !call.muted) call.clearGameKey();
          else { const m = !call.muted; call.setMuted(m); if (!m) call.clearGameKey(); }
          break;
        case 'c-deaf': call.setDeafened(!call.deafened); break;
        case 'c-share': call.sharing ? call.stopShare().catch(() => { }) : call.startShare().catch(() => { }); break;
        case 'c-playopen': openPlayDialog(); break;
        case 'c-cam': call.setCamera(!call.camera); break;
        case 'c-set':
          showSettings = !showSettings;
          if (showSettings) {
            ensureSettings();
            call.listDevices().then(d => { devices = d; render(); }).catch(() => render());
          }
          render();
          break;
        case 'c-leave': leaveRoom(); break;
        case 'c-enc': openExplainer(b); break;
        case 'c-browser': openInBrowser(); break;
      }
    });
    // Push to talk: hold the button (the keyboard key works the same way).
    dock.addEventListener('pointerdown', e => { const p = e.target.closest('#c-ptt'); if (p) { e.preventDefault(); call.pttButton(true); } });
    for (const ev of ['pointerup', 'pointercancel', 'pointerleave']) dock.addEventListener(ev, () => call.pttButton(false));
  }
  function leaveRoom() {
    closePlayDialog(); cancelQuickConnect(); closePop();
    showSettings = false;
    call.leave();
    render();
  }
  async function openInBrowser() {
    const room = currentRoom(); if (!room) return;
    try {
      const r = await rpc('open_call_in_browser', { gid: room.gid, name: room.name });
      if (r.opened) toast('Opened in your browser');
      call.fail(r.opened ? 'handoff' : 'browser', (r.opened ? 'The call should now be open in your browser. If nothing appeared, open' : 'Could not start a browser automatically. Open') + ' this link in Chrome, Brave, Edge or Firefox on this computer: ' + r.url);
    } catch (e) { call.fail('browser', e.message); }
  }

  function encBadge(s) {
    const e = s.encryption;
    if (s.phase === 'idle') return '';
    if (e.active) return `<button type="button" class="call-enc ok" id="c-enc" title="Encrypted. What the call server can see" aria-label="Encrypted. What the call server can see">${ic('ci-lock')}<span class="sr">Encrypted</span></button>`;
    return `<span class="call-enc bad" title="This call's media is not encrypted">NOT encrypted</span>`;
  }

  // ---------------------------------------------------------------- the big picture, in the main area
  function ensureStage() {
    if (stage) return stage;
    stage = document.getElementById('call-stage');
    if (!stage) {
      stage = document.createElement('div'); stage.id = 'call-stage'; stage.hidden = true;
      const main = document.getElementById('main');
      if (main) main.insertBefore(stage, document.getElementById('stream-wrap') || null);
      else document.body.appendChild(stage);
    }
    stage.innerHTML = '<div id="c-shares" hidden></div><div id="c-play" hidden></div>';
    if (!document.getElementById('main')) stage.classList.add('standalone');
    wireStage();
    return stage;
  }
  function renderStage(s) {
    ensureStage();
    const show = s.phase !== 'idle' && (s.shares.length > 0 || !!s.playback);
    stage.classList.toggle('standalone', !document.getElementById('main'));
    stage.hidden = !show;
    if (!show) {
      if (tiles.size) syncTiles([]);
      const pl = $q('#c-play', stage);
      if (pl) { pl.hidden = true; if (pl.__html) { pl.innerHTML = ''; pl.__html = ''; } }
      return;
    }
    syncTiles(s.shares);   // first: a focused quality picker must never keep a stopped video on screen
    renderPlay(s.playback);
  }
  function renderPlay(p) {
    const el = $q('#c-play', stage); if (!el) return;
    if (!p) { el.hidden = true; if (el.__html) { el.innerHTML = ''; el.__html = ''; } return; }
    el.hidden = false;
    if (el.contains(document.activeElement) && document.activeElement.matches('input')) return;   // do not fight a drag
    const html = playBar(p);
    if (el.__html !== html) { el.innerHTML = html; el.__html = html; }
  }
  let stageWired = false;
  function wireStage() {
    if (stageWired) return; stageWired = true;
    const on = (id, fn) => { const el = $q('#' + id, stage); if (el) el.onclick = fn; };
    stage.addEventListener('click', e => {
      const b = e.target.closest('button'); if (!b || !stage.contains(b)) return;
      if (b.id === 'c-playtoggle') call.togglePlayback();
      else if (b.id === 'c-playstop') call.stopPlayback();
      else if (b.id === 'c-prev') call.runePrev();
      else if (b.id === 'c-next') call.runeNext();
    });
    stage.addEventListener('input', e => {
      const t = e.target;
      if (t.id === 'c-pvol') call.setPlaybackVolume(Number(t.value));
    });
    stage.addEventListener('change', e => {
      const t = e.target;
      if (t.id === 'c-seek') call.seekPlayback(Number(t.value));
    });
  }
  function playBar(p) {
    const dur = Math.max(1, Math.round(p.duration || 0));
    const pc = p.kind === 'pcaudio';   // sound from this computer: nothing to pause, skip or seek
    const music = p.kind === 'rune' || pc;
    const skip = music && !pc ? `<button class="btn ghost" id="c-prev" title="Previous track" ${p.hasPrev || p.position > 3 ? '' : 'disabled'}>${ic('ci-prev')}</button>` : '';
    const next = music && !pc ? `<button class="btn ghost" id="c-next" title="Next track" ${p.hasNext ? '' : 'disabled'}>${ic('ci-next')}</button>` : '';
    const queue = music && !pc && p.queueLength > 1 ? `<span class="small muted" title="${esc(p.queueName)}">${p.queueIndex + 1} / ${p.queueLength}</span>` : '';
    return `<div class="cplay"><div class="row"><span class="ttl">${ic(music ? 'ci-note' : 'ci-film')}<b>${esc(p.title)}</b>${p.artist ? `<span class="muted small">· ${esc(p.artist)}</span>` : ''}</span><span class="muted small">${esc(p.profile)}${p.paused ? ' · paused' : ''}</span></div>
      ${pc ? (p.note ? `<div class="row"><span class="small" style="color:var(--warn,#c8a96b)" id="c-pcnote">${esc(p.note)}</span></div>` : '') : `<div class="row">${skip}<button class="btn ghost" id="c-playtoggle" title="${p.paused ? 'Play' : 'Pause'}">${ic(p.paused ? 'ci-play' : 'ci-pause')}</button>${next}
      <input type="range" id="c-seek" min="0" max="${dur}" step="1" value="${Math.min(dur, Math.round(p.position || 0))}" aria-label="Seek">
      <span class="time small muted">${fmtTime(p.position)} / ${fmtTime(p.duration)}</span></div>`}
      <div class="row"><span class="small muted">Volume</span><input type="range" id="c-pvol" min="0" max="1" step="0.05" value="${p.volume}" aria-label="${music ? 'Music' : 'Movie'} volume">${queue}<span class="grow" style="flex:1"></span><button class="btn danger" id="c-playstop">${ic('ci-stop')}Stop</button></div></div>`;
  }
  function syncTiles(shares) {
    const host = stage && $q('#c-shares', stage); if (!host) return;
    const want = new Set(shares.map(x => x.key));
    for (const [k, t] of [...tiles]) if (!want.has(k)) { try { t.track.detach(t.video); } catch { } t.el.remove(); tiles.delete(k); }
    shares.forEach((sh, i) => {
      let t = tiles.get(sh.key);
      if (!t) t = makeTile(sh);
      else if (t.track !== sh.track) { try { t.track.detach(t.video); } catch { } sh.track.attach(t.video); t.track = sh.track; t.video.play().catch(() => { }); }
      const label = `${sh.name} · ${sh.movie ? 'movie' : sh.music ? 'music' : sh.source === 'camera' ? 'camera' : 'screen'}`;
      if (t.label.textContent !== label) t.label.textContent = label;
      if (t.sel && document.activeElement !== t.sel && t.sel.value !== sh.quality) t.sel.value = sh.quality;
      if (host.children[i] !== t.el) host.insertBefore(t.el, host.children[i] || null);   // only new or reordered tiles move
    });
    host.hidden = shares.length === 0;
  }
  function makeTile(sh) {
    const el = document.createElement('div'); el.className = 'ctile' + (sh.local ? ' local' : ''); el.dataset.key = sh.key; el.dataset.source = sh.source;
    const video = document.createElement('video'); video.autoplay = true; video.playsInline = true; video.muted = true;
    sh.track.attach(video);
    const bar = document.createElement('div'); bar.className = 'bar';
    bar.innerHTML = '<span class="grow"></span>' + (sh.local ? '' : '<select aria-label="Quality" title="Picture quality"><option value="auto">Quality: auto</option><option value="high">High</option><option value="medium">Medium</option><option value="low">Low</option></select>') + '<button>Fullscreen</button>';
    const sel = bar.querySelector('select'); if (sel) { sel.value = sh.quality; sel.onchange = () => call.setViewQuality(sh.sid, sel.value); }
    bar.querySelector('button').onclick = () => (video.requestFullscreen || video.webkitRequestFullscreen).call(video);
    el.append(video, bar);
    const t = { el, video, track: sh.track, label: bar.querySelector('.grow'), sel };
    tiles.set(sh.key, t);
    video.play().catch(() => { });
    return t;
  }

  // ---------------------------------------------------------------- call settings, as a dialog
  function ensureSettings() {
    if (settingsDlg) return settingsDlg;
    settingsDlg = document.createElement('div'); settingsDlg.id = 'call-settings'; settingsDlg.hidden = true;
    settingsDlg.innerHTML = `<div class="cs" role="dialog" aria-label="Call settings"><div class="ch"><h3>Call settings</h3><button type="button" class="mini" id="cs-close" title="Close" aria-label="Close">×</button></div><div class="cs-body" id="c-set-body"></div></div>`;
    document.body.appendChild(settingsDlg);
    settingsDlg.addEventListener('click', e => {
      if (e.target === settingsDlg || e.target.closest('#cs-close')) { showSettings = false; render(); }
    });
    return settingsDlg;
  }
  function renderSettings(s) {
    ensureSettings();
    settingsDlg.hidden = !showSettings;
    if (!showSettings) return;
    const body = $q('#c-set-body', settingsDlg);
    if (settingsDlg.contains(document.activeElement) && document.activeElement.matches('input,select,textarea')) return;   // do not fight typing
    const html = settingsBody(s);
    if (body.__html !== html) { body.innerHTML = html; body.__html = html; wireSettings(); }
  }
  function settingsBody(s) {
    const sup = CommonsCall.support();
    const d = devices || { mic: [], out: [], cam: [] };
    return `<div class="cset">${devSelect('c-mic', d.mic, s.prefs.micId, 'Microphone')}${sup.outputSelect ? devSelect('c-out', d.out, s.prefs.outId, 'Speakers') : '<span class="full muted small">This web view cannot pick an output device; the system default is used.</span>'}${devSelect('c-camsel', d.cam, s.prefs.camId, 'Camera')}
      <label class="full"><input type="checkbox" id="c-ns" ${s.prefs.noiseSuppression ? 'checked' : ''}> Noise suppression and echo cancellation (the browser's own)</label>
      <label class="full"><input type="checkbox" id="c-pttc" ${s.prefs.ptt ? 'checked' : ''}> Push to talk (hold <b>${esc(s.pttKey.replace('Key', '').replace('Backquote', '` (backtick)'))}</b> while this window is focused) <button type="button" class="mini" id="c-pttkey">Change key</button></label>
      <label class="full"><input type="checkbox" id="c-sa" ${s.prefs.shareAudio ? 'checked' : ''}> Include audio when sharing a screen or window, where the system allows it</label>
      <label for="c-camprof">Camera quality</label><select id="c-camprof">${Object.entries(call.cameraProfiles()).map(([k, v]) => `<option value="${k}" ${(s.prefs.cameraProfile || '720p') === k ? 'selected' : ''}>${esc(v.label)}</option>`).join('')}</select>
      <label for="c-prof">Share as</label><select id="c-prof">${Object.entries(call.shareProfiles()).map(([k, v]) => `<option value="${k}" ${s.prefs.shareProfile === k ? 'selected' : ''}>${esc(v.label)}</option>`).join('')}</select>
      ${window.__commonsGameKeys ? '<span></span><button type="button" class="btn ghost" id="c-gamekeys">Game keys…</button>' : ''}
      <div class="full jfset"><b>Jellyfin library</b> <span class="muted small">(for playing films and episodes into the call)</span></div>
      <label for="c-jf-url">Address</label><input type="text" id="c-jf-url" placeholder="http://192.168.1.10:8096" spellcheck="false" value="${esc(call.jellyfinConfig().url)}">
      <span></span><button type="button" class="btn ghost" id="c-jfsave">Save address</button>
      ${jellyfinAuthMarkup()}
      <div class="full muted small">Kept on this device only: never synced and never sent to the relay. The address and sign-in go straight to your Jellyfin server from this page. Jellyfin must allow this page's origin (CORS).</div>
      <div class="full jfset"><b>rune music</b> <span class="muted small">(for playing your own local music into the call)</span></div>
      <label for="c-rune-url">Address</label><input type="text" id="c-rune-url" placeholder="http://127.0.0.1:4545" spellcheck="false" value="${esc(call.runeConfig().url)}">
      <span></span><button type="button" class="btn ghost" id="c-runesave">Save rune address</button>
      <div class="full muted small">rune sends only your local library, never Spotify. Its <code>corsOrigins</code> must list this page's origin, and for another device the address must be reachable from it.</div></div>`;
  }
  function devSelect(id, list, sel, label) {
    return `<label for="${id}">${label}</label><select id="${id}"><option value="">System default</option>${list.map(d => `<option value="${esc(d.deviceId)}" ${d.deviceId === sel ? 'selected' : ''}>${esc(d.label || 'Device')}</option>`).join('')}</select>`;
  }
  function wireSettings() {
    const root = $q('#c-set-body', settingsDlg); if (!root) return;
    const $ = s => root.querySelector(s);
    const on = (id, fn) => { const el = $('#' + id); if (el) el.onclick = fn; };
    const sel = (id, kind) => { const el = $('#' + id); if (el) el.onchange = () => call.pickDevice(kind, el.value); };
    sel('c-mic', 'audioinput'); sel('c-out', 'audiooutput'); sel('c-camsel', 'videoinput');
    const ns = $('#c-ns'); if (ns) ns.onchange = () => call.setNoiseSuppression(ns.checked);
    const pc = $('#c-pttc'); if (pc) pc.onchange = () => call.setPtt(pc.checked);
    const sa = $('#c-sa'); if (sa) sa.onchange = () => { call.prefs.shareAudio = sa.checked; call.savePrefs(); };
    const cq = $('#c-camprof'); if (cq) cq.onchange = async () => { call.prefs.cameraProfile = cq.value; call.savePrefs(); if (call.camera) { await call.setCamera(false); await call.setCamera(true); } };
    const pr = $('#c-prof'); if (pr) pr.onchange = () => { call.prefs.shareProfile = pr.value; call.savePrefs(); };
    const gk = $('#c-gamekeys'); if (gk) gk.onclick = () => { if (window.__commonsGameKeys) window.__commonsGameKeys(); };
    const jfs = $('#c-jfsave');
    if (jfs) jfs.onclick = () => { call.saveJellyfinUrl($('#c-jf-url').value); toast('Jellyfin address saved on this device'); render(); };
    const jfqc = $('#c-jfqc'); if (jfqc) jfqc.onclick = () => startQuickConnect();
    const jfcancel = $('#c-jfcancel'); if (jfcancel) jfcancel.onclick = () => cancelQuickConnect('Quick Connect cancelled');
    const jfso = $('#c-jfsignout'); if (jfso) jfso.onclick = () => { call.jellyfinSignOut(); toast('Signed out of Jellyfin on this device'); render(); };
    const jfkey = $('#c-jfsavekey');
    if (jfkey) jfkey.onclick = () => {
      const k = $('#c-jf-key').value;
      call.saveJellyfinConfig($('#c-jf-url').value, k);
      toast(k ? 'API key saved on this device' : 'API key cleared');
      render();
    };
    const runes = $('#c-runesave');
    if (runes) runes.onclick = () => { call.saveRuneConfig($('#c-rune-url').value); toast('rune address saved on this device'); render(); };
    const pk = $('#c-pttkey'); if (pk) pk.onclick = () => { pk.textContent = 'Press a key…'; const h = e => { e.preventDefault(); document.removeEventListener('keydown', h, true); pk.textContent = 'Change key'; call.setPttKey(e.code); }; document.addEventListener('keydown', h, true); };
  }

  // ---------------------------------------------------------------- small popovers: per-person volume, the explainer
  function showPop(anchor, html, kind) {
    closePop();
    pop = document.createElement('div'); pop.id = 'call-pop'; pop.className = 'call-pop ' + kind; pop.dataset.kind = kind; pop.setAttribute('role', 'dialog'); pop.innerHTML = html;
    document.body.appendChild(pop);
    const r = anchor.getBoundingClientRect(), pr = pop.getBoundingClientRect();
    let left = Math.min(Math.max(8, r.left + r.width / 2 - pr.width / 2), Math.max(8, window.innerWidth - pr.width - 8));
    let top = r.top - pr.height - 8;
    if (top < 8) top = Math.min(r.bottom + 8, Math.max(8, window.innerHeight - pr.height - 8));
    pop.style.left = left + 'px'; pop.style.top = top + 'px';
    return pop;
  }
  function closePop() { if (pop) { pop.remove(); pop = null; } }
  document.addEventListener('pointerdown', e => { if (pop && !pop.contains(e.target) && !(e.target.closest && e.target.closest('#c-enc'))) closePop(); }, true);
  document.addEventListener('keydown', e => { if (e.key === 'Escape' && pop) closePop(); });
  // Right-click or long-press a person under a room: their volume, and a way to silence them on this device only.
  function openVolumePop(row) {
    const account = row.dataset.account || '';
    const name = row.dataset.name || (row.querySelector('.live-n') || {}).textContent || 'This person';
    const s = call.snapshot();
    const peer = s.peers.find(p => p.account === account);
    const id = peer && peer.identity;
    const vol = id ? call.volumeOf(id) : 1;
    const p = showPop(row, `<div class="cp-name">${esc(name)}</div>
      <label class="cp-vol"><span>Volume</span><input type="range" min="0" max="1" step="0.05" value="${vol}" aria-label="Volume for ${esc(name)}" ${id ? '' : 'disabled'}></label>
      <button type="button" class="btn ghost" id="cp-mute" ${id ? '' : 'disabled'}>${vol === 0 ? 'Unmute for me' : 'Mute for me'}</button>
      ${id ? '' : '<p class="small muted">Join the call to change how they sound.</p>'}`, 'vol');
    const r = p.querySelector('input'), b = p.querySelector('#cp-mute');
    r.oninput = () => { const v = Number(r.value); call.setVolume(id, v); b.textContent = v === 0 ? 'Unmute for me' : 'Mute for me'; };
    b.onclick = () => { const to = Number(r.value) === 0 ? 1 : 0; r.value = String(to); call.setVolume(id, to); b.textContent = to === 0 ? 'Unmute for me' : 'Mute for me'; };
  }
  function explainHtml(s) {
    const e = s.encryption;
    return `<b>What the call server can see</b>
      ${e.active ? `<p>Every audio and video frame is encrypted on this device before it is sent. Frame cipher: AES-GCM; backend: ${esc(e.backend)}; key epoch ${e.epoch ?? '?'}.</p>` : '<p class="bad">This call\'s media is NOT encrypted.</p>'}
      <p>Your IP address, when you join and leave, how much you send, and who is speaking. It cannot hear or see anything.</p>
      <p class="muted small">Keys: ${e.devices ?? '?'} device keys from the room, epoch ${e.epoch ?? '?'}, rotated ${e.rotations} time(s) this call.</p>`;
  }
  function openExplainer(anchor) {
    const was = pop && pop.dataset.kind === 'explain';
    closePop();
    if (was) return;
    showPop(anchor, explainHtml(call.snapshot()), 'explain');
  }
  // The rail is re-rendered by app.js; this one listener stays on the container.
  function wireRail() {
    const rooms = document.getElementById('rooms');
    if (!rooms || rooms.__callPopWired) return;
    rooms.__callPopWired = true;
    rooms.addEventListener('contextmenu', e => {
      const row = e.target.closest('.live-p[data-account]'); if (!row) return;
      e.preventDefault();
      openVolumePop(row);
    });
    let timer = null, held = false, sx = 0, sy = 0;
    const stop = () => { if (timer) { clearTimeout(timer); timer = null; } };
    rooms.addEventListener('pointerdown', e => {
      if (e.button !== 0) return;
      const row = e.target.closest('.live-p[data-account]'); if (!row) return;
      sx = e.clientX; sy = e.clientY; stop();
      timer = setTimeout(() => { timer = null; held = true; openVolumePop(row); setTimeout(() => { held = false; }, 800); }, 500);
    });
    rooms.addEventListener('pointerup', stop);
    rooms.addEventListener('pointercancel', stop);
    rooms.addEventListener('pointerleave', stop);
    rooms.addEventListener('pointermove', e => { if (timer && (Math.abs(e.clientX - sx) > 10 || Math.abs(e.clientY - sy) > 10)) stop(); }, true);
    // A long press must not also join the call: the rail's own click handler is stopped before it runs.
    rooms.addEventListener('click', e => {
      if (!held) return;
      e.preventDefault(); e.stopImmediatePropagation();
    }, true);
  }

  // ---------------------------------------------------------------- play into the call: the picker dialog
  function ensurePlayDialog() {
    if (playDlg) return playDlg;
    playDlg = document.createElement('div'); playDlg.id = 'play-dialog'; playDlg.hidden = true;
    playDlg.innerHTML = `<div class="pd" role="dialog" aria-label="Play into the call">
      <div class="ch"><h3>Play into the call</h3><button class="mini" id="pd-close" title="Close">×</button></div>
      <div class="pd-body">
        <div class="pd-row"><label class="btn ghost pd-pick">${ic('ci-film')}Choose a video file<input type="file" id="pd-file" accept="video/*" hidden></label>
          <select id="pd-profile" aria-label="Quality">${Object.entries(call.playbackProfiles()).map(([k, v]) => `<option value="${k}" ${call.prefs.playProfile === k ? 'selected' : ''}>${esc(v.label)}</option>`).join('')}</select>
          <span id="pd-status" class="muted small"></span></div>
        <p class="muted small">The video is streamed from your device straight into the call, encrypted like everything else. Everyone sees and hears it, and you control it.</p>
        <div class="pd-pc" id="pd-pc" hidden></div>
        <div class="pd-rune">
          <div class="pd-row"><b>Music from rune</b> <span class="muted small">your own local library, played as a queue</span></div>
          <div class="pd-row"><input type="search" id="pd-rq" placeholder="Search your local music in rune" autocomplete="off"><button class="btn" id="pd-rgo">Search</button></div>
          <div id="pd-rnote" class="muted small"></div>
          <div id="pd-rresults" class="pd-results"></div>
        </div>
        <div class="pd-jf">
          <div class="pd-row"><b>Jellyfin</b> <span class="muted small">films and episodes from your library</span></div>
          <div class="pd-row"><input type="search" id="pd-q" placeholder="Search your Jellyfin library" autocomplete="off"><button class="btn" id="pd-go">Search</button></div>
          <div id="pd-jfnote" class="muted small"></div>
          <div id="pd-results" class="pd-results"></div>
        </div>
      </div></div>`;
    document.body.appendChild(playDlg);
    const $ = s => playDlg.querySelector(s);
    $('#pd-close').onclick = () => closePlayDialog();
    playDlg.addEventListener('click', e => { if (e.target === playDlg) closePlayDialog(); });
    $('#pd-q').addEventListener('keydown', e => { if (e.key === 'Enter') { e.preventDefault(); searchJellyfin(); } });
    $('#pd-go').onclick = () => searchJellyfin();
    $('#pd-rq').addEventListener('keydown', e => { if (e.key === 'Enter') { e.preventDefault(); searchRune(); } });
    $('#pd-rgo').onclick = () => searchRune();
    $('#pd-profile').onchange = () => { call.prefs.playProfile = $('#pd-profile').value; call.savePrefs(); };
    $('#pd-file').onchange = async e => {
      const f = e.target.files && e.target.files[0]; if (!f) return;
      $('#pd-status').textContent = 'Opening ' + f.name + '…';
      try { await call.startFilePlayback(f, { profile: $('#pd-profile').value }); closePlayDialog(); toast('Playing into the call'); }
      catch (er) { $('#pd-status').textContent = er.message; }
      e.target.value = '';
    };
    return playDlg;
  }
  function openPlayDialog() {
    ensurePlayDialog();
    playDlg.hidden = false;
    const jf = call.jellyfinConfig(), note = playDlg.querySelector('#pd-jfnote');
    if (!jf.url || !call.jellyfinCredential()) { note.innerHTML = 'Jellyfin is not set up on this device. Add its address and sign in under <b>Settings</b> in the call\'s settings.'; playDlg.querySelector('#pd-results').innerHTML = ''; }
    else if (!playItems) searchJellyfin();
    if (window.CommonsPcAudioUI) window.CommonsPcAudioUI.mount(playDlg.querySelector('#pd-pc'), call, { toast, close: closePlayDialog });
    const rnote = playDlg.querySelector('#pd-rnote');
    if (rnote && !rnote.textContent) rnote.textContent = 'rune at ' + call.runeConfig().url + ' — search tracks, albums and playlists from your own library.';
  }
  function closePlayDialog() { if (playDlg) playDlg.hidden = true; }
  function runeResultButton(kind, i, cover, title, sub) {
    const label = kind.charAt(0).toUpperCase() + kind.slice(1);
    return `<button class="pd-item" data-kind="${kind}" data-i="${i}" title="${esc(title)}"><span class="pos">${cover ? `<img src="${esc(cover)}" alt="" loading="lazy" referrerpolicy="no-referrer">` : `<span class="noposter">${ic('ci-note')}</span>`}</span><span class="nm">${esc(title)}</span><span class="sub">${esc([label, sub].filter(Boolean).join(' · '))}</span></button>`;
  }
  async function searchRune() {
    const $ = s => playDlg.querySelector(s);
    if (runeBusy) return;
    runeBusy = true; $('#pd-rgo').disabled = true; $('#pd-rnote').textContent = 'Searching rune…'; $('#pd-rresults').innerHTML = '';
    try {
      runeItems = await call.runeSearch($('#pd-rq').value);
      const n = runeItems.tracks.length + runeItems.albums.length + runeItems.playlists.length;
      $('#pd-rnote').textContent = n ? n + ' found. Pick one to play into the call (Spotify tracks are left out).' : 'Nothing found in your local library.';
      $('#pd-rresults').innerHTML = [
        ...runeItems.albums.map((it, i) => runeResultButton('album', i, it.coverUrl, it.title, [it.artist, it.year].filter(Boolean).join(' · '))),
        ...runeItems.playlists.map((it, i) => runeResultButton('playlist', i, it.coverUrl, it.name, it.trackCount ? it.trackCount + ' tracks' : '')),
        ...runeItems.tracks.map((it, i) => runeResultButton('track', i, it.coverUrl, it.title, it.artist)),
      ].join('');
      $('#pd-rresults').querySelectorAll('[data-kind]').forEach(b => b.onclick = async () => {
        const kind = b.dataset.kind, it = (runeItems[kind + 's'] || [])[Number(b.dataset.i)];
        if (!it) return;
        b.disabled = true; $('#pd-rnote').textContent = 'Opening ' + (it.title || it.name) + '…';
        try {
          await call.startRunePlayback(kind === 'track' ? { type: 'track', track: it } : { type: kind, id: it.id, title: it.title || it.name });
          closePlayDialog(); toast('Playing into the call');
        } catch (er) { $('#pd-rnote').textContent = er.message; b.disabled = false; }
      });
    } catch (er) { $('#pd-rnote').textContent = er.message; }
    finally { runeBusy = false; if (playDlg) $('#pd-rgo').disabled = false; }
  }
  async function searchJellyfin() {
    const $ = s => playDlg.querySelector(s);
    const jf = call.jellyfinConfig();
    if (!jf.url || !call.jellyfinCredential()) { $('#pd-jfnote').innerHTML = 'Add your Jellyfin address and sign in under <b>Settings</b> first.'; return; }
    if (playBusy) return;
    playBusy = true; $('#pd-go').disabled = true; $('#pd-jfnote').textContent = 'Searching…'; $('#pd-results').innerHTML = '';
    try {
      playItems = await call.jellyfinSearch($('#pd-q').value);
      $('#pd-jfnote').textContent = playItems.length ? playItems.length + ' found. Pick one to play into the call.' : 'Nothing found.';
      $('#pd-results').innerHTML = playItems.map((it, i) => `<button class="pd-item" data-i="${i}" title="${esc(it.name)}"><span class="pos">${it.hasImage ? `<img src="${esc(call.jellyfinImageUrl(it))}" alt="" loading="lazy" referrerpolicy="no-referrer">` : `<span class="noposter">${ic('ci-film')}</span>`}</span><span class="nm">${esc(it.name)}</span><span class="sub">${esc([it.series, it.year].filter(Boolean).join(' · '))}</span></button>`).join('');
      $('#pd-results').querySelectorAll('[data-i]').forEach(b => b.onclick = async () => {
        const it = playItems[Number(b.dataset.i)]; if (!it) return;
        b.disabled = true; $('#pd-jfnote').textContent = 'Opening ' + it.name + '…';
        try { await call.startJellyfinPlayback(it, { profile: $('#pd-profile').value }); closePlayDialog(); toast('Playing into the call'); }
        catch (er) { $('#pd-jfnote').textContent = er.message; b.disabled = false; }
      });
    } catch (er) { $('#pd-jfnote').textContent = er.message; }
    finally { playBusy = false; if (playDlg) $('#pd-go').disabled = false; }
  }

  // ---------------------------------------------------------------- Jellyfin sign-in (inside the settings dialog)
  function jellyfinAuthMarkup() {
    const jf = call.jellyfinConfig();
    if (qcSession) {
      return `<div class="full qc">
        <div class="qc-code" id="c-qc-code">${esc(qcSession.code || '…')}</div>
        <div class="small">On your Jellyfin server open <b>Settings → Quick Connect</b>, choose this device and enter the code above.</div>
        <div class="small ${qcSession.error ? 'w' : 'muted'}" id="c-qc-status">${qcSession.error ? esc(qcSession.error) : 'Waiting for you to approve it…'}</div>
        <button type="button" class="btn ghost" id="c-jfcancel">Cancel</button></div>`;
    }
    if (call.jellyfinCredential()) {
      const who = jf.userName || (jf.userId ? 'this Jellyfin user' : 'an API key');
      return `<div class="full qc-signed"><span class="ok">Signed in</span> <span class="muted small">as ${esc(who)}</span> <button type="button" class="btn ghost" id="c-jfsignout">Sign out</button></div>`;
    }
    if (!jf.url) return '<div class="full muted small">Add the address above, then sign in.</div>';
    return `<span></span><button type="button" class="btn" id="c-jfqc">Sign in with Quick Connect</button>
      <details class="full adv"><summary class="small muted">Advanced: paste an API key instead</summary>
        <div class="pd-row"><input type="password" id="c-jf-key" placeholder="Dashboard → API Keys" autocomplete="off"><button type="button" class="btn ghost" id="c-jfsavekey">Use API key</button></div></details>`;
  }
  function cancelQuickConnect(msg) {
    const s = qcSession;
    if (!s) return;
    qcSession = null;
    if (s.timer) clearTimeout(s.timer);
    if (msg) toast(msg);
    if (showSettings) render();
  }
  async function startQuickConnect() {
    if (qcSession) return;
    const btn = $q('#c-jfqc', settingsDlg);
    if (btn) { btn.disabled = true; btn.textContent = 'Starting…'; }
    try {
      const s = await call.jellyfinQuickConnectInitiate();
      qcSession = { secret: s.secret, code: s.code, error: '', timer: null, startedAt: Date.now() };
      render();
      pollSoon();
    } catch (er) {
      if (btn) { btn.disabled = false; btn.textContent = 'Sign in with Quick Connect'; }
      toast(er.message);
    }
  }
  function pollSoon() {
    const s = qcSession; if (!s) return;
    s.timer = setTimeout(() => pollQuickConnect(s), 2000);
  }
  async function pollQuickConnect(s) {
    if (qcSession !== s) return;
    s.timer = null;
    try {
      const r = await call.jellyfinQuickConnectPoll(s.secret);
      if (qcSession !== s) return;
      if (r && r.authenticated) {
        qcSession = null;
        toast('Signed in to Jellyfin' + (r.userName ? ' as ' + r.userName : ''));
        render();
        return;
      }
    } catch (er) {
      if (qcSession !== s) return;
      qcSession = null;
      toast(er.message);
      render();
      return;
    }
    if (Date.now() - s.startedAt > 120000) { cancelQuickConnect('The Quick Connect code expired. Start again.'); return; }
    pollSoon();
  }

  // ---------------------------------------------------------------- one render
  function render() {
    updateButton();
    let s;
    try { s = call.snapshot(); } catch { return; }
    const sup = CommonsCall.support();
    renderDock(s, sup);
    renderStage(s);
    renderSettings(s);
    if (window.__railCallChanged) window.__railCallChanged();
    wireRail();
    if (window.__commonsFillAvatars) window.__commonsFillAvatars(dock);
  }

  call.addEventListener('change', render);
  setInterval(() => { ensureButton(); render(); if (window.__commonsRefreshAvatars) window.__commonsRefreshAvatars(); }, 700);
  if (window.matchMedia) {
    const mq = window.matchMedia('(max-width: 760px)');
    if (mq.addEventListener) mq.addEventListener('change', () => { placeDock(); render(); });
  }
  window.addEventListener('resize', () => { placeDock(); });
  ensureDock(); ensureStage();
  // Used by the rail's call rows and by the stand-alone browser call page. A voice room click joins its call; if this
  // device is already in that call, nothing is torn down and set up again.
  window.__callUI = {
    open() {
      ensureDock(); ensureStage();
      // The stand-alone call page exists only to run this one call in a real browser tab, so it joins by itself.
      if (!document.getElementById('rail')) { const room = currentRoom(); if (room && call.phase === 'idle') call.join(room.gid, room.name).catch(() => { }); }
      render();
    },
    render,
    joinRoom(gid, name) {
      if (call.gid === gid && call.phase !== 'idle') { render(); return; }
      call.join(gid, name).catch(() => { });
      render();
    },
  };
})();
