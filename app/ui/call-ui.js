'use strict';
// Voice room panel. Depends on app.js globals (S, rpc, esc, toast) and call.js. Adds a "Call" button to the room header
// and one floating panel; nothing else in the page is touched.
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
      <symbol id="ci-note" viewBox="0 0 20 20"><path d="M7.5 15.5V5l8-1.8v10.3"/><circle cx="5.2" cy="15.5" r="2.3"/><circle cx="13.2" cy="13.5" r="2.3"/></symbol>
      <symbol id="ci-play" viewBox="0 0 20 20"><path d="M7 4.5l8 5.5-8 5.5z"/></symbol>
      <symbol id="ci-pause" viewBox="0 0 20 20"><path d="M7 4.5v11M13 4.5v11"/></symbol>
      <symbol id="ci-stop" viewBox="0 0 20 20"><rect x="5.5" y="5.5" width="9" height="9" rx="1"/></symbol>
      <symbol id="ci-prev" viewBox="0 0 20 20"><path d="M6 4.5v11M15 5l-7 5 7 5z"/></symbol>
      <symbol id="ci-next" viewBox="0 0 20 20"><path d="M14 4.5v11M5 5l7 5-7 5z"/></symbol>
    </g></svg>`);
  }
  const ic = n => `<svg class="ic"><use href="#${n}"/></svg>`;
  window.__commonsCall = call;               // used by the headless tests
  const $q = (s, r = document) => r.querySelector(s);
  const hue = s => { let h = 0; for (const c of s || 'x') h = (h * 31 + c.charCodeAt(0)) % 360; return h; };
  let panel = null, open = false, showSettings = false, devices = null;
  let playDlg = null, playItems = null, playBusy = false;
  let runeItems = null, runeBusy = false;
  let qcSession = null;   // Jellyfin Quick Connect while a code is on screen: { secret, code, busy, error, timer }
  const fmtTime = s => { s = Math.max(0, Math.floor(Number(s) || 0)); const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), x = s % 60; return (h ? h + ':' + String(m).padStart(2, '0') : String(m)) + ':' + String(x).padStart(2, '0'); };
  // Video tiles live in their own host that no render ever rewrites. Each tile (one per video track: a camera and a screen
  // share of the same person are two tiles) is created once, updated in place, and removed when its track is muted or gone.
  // Moving or re-creating a <video> on every state update made the pictures flicker.
  const tiles = new Map();

  function currentRoom() {
    try {
      const c = S.snap && S.snap.communities.find(x => x.id === S.cid);
      const ch = c && c.channels.find(x => x.gid === S.gid);
      return ch ? { gid: ch.gid, name: ch.name, community: c.name, joined: ch.joined } : null;
    } catch { return null; }
  }

  function ensureButton() {
    const bar = $q('.head-actions');
    if (!bar || $q('#btn-call')) return;
    const b = document.createElement('button'); b.id = 'btn-call'; b.className = 'btn ghost'; b.innerHTML = ic('ci-wave') + '<span class="lbl">Call</span>'; b.title = 'Voice and screen share in this room (end-to-end encrypted)';
    b.onclick = () => { open = !open; if (open) ensurePanel(); render(); };
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

  function ensurePanel() {
    if (panel) return;
    panel = document.createElement('div'); panel.id = 'call-panel'; panel.setAttribute('role', 'dialog'); panel.setAttribute('aria-label', 'Voice room'); panel.hidden = true;
    panel.innerHTML = '<div class="cpart" id="c-top"></div><div class="cshares" id="c-shares" hidden></div><div class="cpart" id="c-bottom"></div>';
    document.body.appendChild(panel);
  }

  function encBadge(s) {
    const e = s.encryption;
    if (s.phase === 'idle') return '';
    if (e.active) return `<span class="call-enc ok" title="Every audio and video frame is encrypted on your device with keys from this room's MLS group before it is sent. Frame cipher: AES-GCM; backend: ${esc(e.backend)}; key epoch ${e.epoch ?? '?'}.">${ic('i-lock')}Encrypted</span>`;
    return `<span class="call-enc bad">NOT encrypted</span>`;
  }

  function peerRow(p) {
    const tags = [];
    if (p.me) tags.push('you');
    if (p.muted) tags.push('muted');
    if (!p.member) tags.push('<span class="w">not in this room: blocked</span>');
    if (p.cantDecrypt) tags.push('<span class="w">cannot decrypt (keys out of sync)</span>');
    return `<div class="cp ${p.speaking ? 'speaking' : ''}" data-id="${esc(p.identity)}">
      ${typeof seal === 'function' ? `<div class="av sealed">${seal(p.account || p.identity, p.name)}</div>` : `<div class="av" style="background:hsl(${hue(p.account || p.identity)} 50% 38%)">${esc((p.name || '?').slice(0, 1).toUpperCase())}</div>`}
      <div><div class="nm">${esc(p.name)}</div><div class="tags">${tags.join(' · ') || (p.speaking ? 'speaking' : '&nbsp;')}</div></div>
      ${p.me ? '' : `<input type="range" min="0" max="1" step="0.05" value="${p.volume}" aria-label="Volume for ${esc(p.name)}" data-vol="${esc(p.identity)}">`}</div>`;
  }

  function devSelect(id, list, sel, label) {
    return `<label for="${id}">${label}</label><select id="${id}"><option value="">System default</option>${list.map(d => `<option value="${esc(d.deviceId)}" ${d.deviceId === sel ? 'selected' : ''}>${esc(d.label || 'Device')}</option>`).join('')}</select>`;
  }

  // The sender's controls for a movie playing into the call. Viewers get the picture and sound as a normal share tile; only
  // the sender controls it, as asked.
  function playBar(p) {
    const dur = Math.max(1, Math.round(p.duration || 0));
    const music = p.kind === 'rune';
    const skip = music ? `<button class="btn ghost" id="c-prev" title="Previous track" ${p.hasPrev || p.position > 3 ? '' : 'disabled'}>${ic('ci-prev')}</button>` : '';
    const next = music ? `<button class="btn ghost" id="c-next" title="Next track" ${p.hasNext ? '' : 'disabled'}>${ic('ci-next')}</button>` : '';
    const queue = music && p.queueLength > 1 ? `<span class="small muted" title="${esc(p.queueName)}">${p.queueIndex + 1} / ${p.queueLength}</span>` : '';
    return `<div class="cplay"><div class="row"><span class="ttl">${ic(music ? 'ci-note' : 'ci-film')}<b>${esc(p.title)}</b>${p.artist ? `<span class="muted small">· ${esc(p.artist)}</span>` : ''}</span><span class="muted small">${esc(p.profile)}${p.paused ? ' · paused' : ''}</span></div>
      <div class="row">${skip}<button class="btn ghost" id="c-playtoggle" title="${p.paused ? 'Play' : 'Pause'}">${ic(p.paused ? 'ci-play' : 'ci-pause')}</button>${next}
      <input type="range" id="c-seek" min="0" max="${dur}" step="1" value="${Math.min(dur, Math.round(p.position || 0))}" aria-label="Seek">
      <span class="time small muted">${fmtTime(p.position)} / ${fmtTime(p.duration)}</span></div>
      <div class="row"><span class="small muted">Volume</span><input type="range" id="c-pvol" min="0" max="1" step="0.05" value="${p.volume}" aria-label="${music ? 'Music' : 'Movie'} volume">${queue}<span class="grow" style="flex:1"></span><button class="btn danger" id="c-playstop">${ic('ci-stop')}Stop</button></div></div>`;
  }

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
    if (!jf.url || !call.jellyfinCredential()) { note.innerHTML = 'Jellyfin is not set up on this device. Add its address and sign in under <b>Settings</b> in the call panel.'; playDlg.querySelector('#pd-results').innerHTML = ''; }
    else if (!playItems) searchJellyfin();
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

  function render() {
    updateButton();
    if (!panel) return;
    panel.hidden = !open;
    if (!open) return;
    const s = call.snapshot();
    const sup = CommonsCall.support();
    const room = currentRoom();
    const inThis = room && s.gid === room.gid && s.phase !== 'idle';
    syncTiles(s.shares);   // before the guard below, so a focused quality picker never keeps a stopped video on screen
    if (panel.contains(document.activeElement) && document.activeElement.matches('input,select,textarea')) return;   // do not fight a drag or typing in progress
    let h = `<div class="ch"><h3>${esc(inThis || s.phase !== 'idle' ? (s.roomName || 'Call') : (room ? room.name : 'Call'))}</h3>${encBadge(s)}<button class="mini" id="c-close" title="Hide (the call keeps running)">×</button></div>`;
    if (!sup.ok) {
      const why = `<details class="cnote"><summary>Why?</summary>${sup.reasons.map(esc).join('<br>')}</details>`;
      h += (s.error ? `<div class="${s.error.kind === 'handoff' ? 'cinfo' : 'cerr'} wrapall">${esc(s.error.msg)}</div>` : '') + (window.__TAURI__
        ? `<div class="cnote"><b>On this computer, calls open in your browser.</b> The app window cannot do encrypted calls here, so the call runs in a browser tab: same room, same end-to-end encryption. Use Chrome, Brave, Edge or Firefox. If you are asked for the microphone, allow it. Close the tab to leave the call.</div>
           ${room ? '<div class="cbar"><button class="btn" id="c-browser">Open this call in my browser</button></div>' : ''}${why}`
        : `<div class="cerr"><b>Calls are not available in this window.</b><br>${sup.reasons.map(esc).join('<br>')}</div><div class="cnote">Megatron will never fall back to an unencrypted call. Try a current Chromium-based browser or Firefox.</div>`);
      paint(h, ''); return;
    }
    if (s.phase === 'idle' || (s.phase === 'error' && !inThis && !s.gid)) {
      h += `${s.error ? `<div class="${s.error.kind === 'handoff' ? 'cinfo' : 'cerr'} wrapall">${esc(s.error.msg)}</div>` : ''}
        <div class="cinfo">${room ? `Join the voice room for <b>${esc(room.name)}</b>. Anyone in this room can join; media is encrypted with keys from the room's group and the call server only forwards ciphertext.` : 'Open a room first.'}</div>
        <div class="cbar"><button class="btn" id="c-join" ${room ? '' : 'disabled'}>Join voice</button>${window.__TAURI__ && room ? '<button class="btn ghost" id="c-browser" title="If the microphone or screen share does not work in this window">Open in my browser</button>' : ''}</div>`;
      paint(h, ''); return;
    }
    if (s.error) h += `<div class="cerr">${esc(s.error.msg)}</div>`;
    if (s.phase === 'connecting') h += '<div class="cinfo">Connecting…</div>';
    if (s.phase === 'reconnecting') h += `<div class="cinfo">Connection lost${s.attempt ? ' (attempt ' + s.attempt + ')' : ''}. Reconnecting; your place in the call is kept if it comes back quickly.</div>`;
    if (s.blocked.length) h += `<div class="cerr">Blocked ${s.blocked.length} stream(s) that are not from a current member or not encrypted. They are not played.</div>`;
    if (s.gameMuted) h += `<div class="cinfo" id="c-gamekey">Muted by game key${s.gameName ? ' (' + esc(s.gameName) + ')' : ''}. Friends see you as muted.</div>`;
    else if (s.gameError) h += `<div class="cinfo wrapall">Game keys are not working: ${esc(s.gameError)}</div>`;
    const top = h; h = '';
    h += `<div class="cpeople">${s.peers.map(peerRow).join('')}</div>`;
    if (s.playback) h += playBar(s.playback);
    const ptt = s.ptt;
    h += `<div class="cbar">
      <button class="btn ${s.muted || s.gameMuted ? 'on' : 'ghost'}" id="c-mute">${ic(s.muted || s.gameMuted ? 'ci-micoff' : 'ci-mic')}${s.muted ? 'Unmute' : s.gameMuted ? 'Muted by game key' : 'Mute'}</button>
      <button class="btn ${s.deafened ? 'on' : 'ghost'}" id="c-deaf">${ic('ci-phones')}${s.deafened ? 'Undeafen' : 'Deafen'}</button>
      ${ptt ? `<button class="btn ${s.pttHeld ? '' : 'ghost'}" id="c-ptt">Hold to talk</button>` : ''}
      <button class="btn ${s.sharing ? 'on' : 'ghost'}" id="c-share" ${sup.canShareScreen ? '' : 'disabled title="This browser or webview cannot share a screen"'}>${ic('ci-screen')}${s.sharing ? 'Stop sharing' : 'Share screen'}</button>
      ${s.playback ? '' : `<button class="btn ghost" id="c-playopen" ${s.sharing ? 'disabled title="Stop sharing your screen first"' : ''}>${ic('ci-film')}Play video</button>`}
      <button class="btn ${s.camera ? 'on' : 'ghost'}" id="c-cam">${ic('ci-cam')}${s.camera ? 'Camera off' : 'Camera'}</button>
      <button class="btn ghost" id="c-set">${ic('ci-gear')}Settings</button>
      <span class="grow" style="flex:1"></span>
      <button class="btn danger" id="c-leave">${ic('ci-leave')}Leave</button></div>`;
    if (showSettings) {
      const d = devices || { mic: [], out: [], cam: [] };
      h += `<div class="cset">${devSelect('c-mic', d.mic, s.prefs.micId, 'Microphone')}${sup.outputSelect ? devSelect('c-out', d.out, s.prefs.outId, 'Speakers') : '<span class="full muted small">This web view cannot pick an output device; the system default is used.</span>'}${devSelect('c-camsel', d.cam, s.prefs.camId, 'Camera')}
        <label class="full"><input type="checkbox" id="c-ns" ${s.prefs.noiseSuppression ? 'checked' : ''}> Noise suppression and echo cancellation (the browser's own)</label>
        <label class="full"><input type="checkbox" id="c-pttc" ${s.prefs.ptt ? 'checked' : ''}> Push to talk (hold <b>${esc(s.pttKey.replace('Key', '').replace('Backquote', '` (backtick)'))}</b> while this window is focused) <button class="mini" id="c-pttkey">Change key</button></label>
        <label class="full"><input type="checkbox" id="c-sa" ${s.prefs.shareAudio ? 'checked' : ''}> Include audio when sharing a screen or window, where the system allows it</label>
        <label for="c-prof">Share as</label><select id="c-prof">${Object.entries(call.shareProfiles()).map(([k, v]) => `<option value="${k}" ${s.prefs.shareProfile === k ? 'selected' : ''}>${esc(v.label)}</option>`).join('')}</select>
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
    h += `<details class="cnote"><summary>What the call server can see</summary>Your IP address, when you join and leave, how much you send, and who is speaking. It cannot hear or see anything. Keys: ${s.encryption.devices ?? '?'} device keys from the room, epoch ${s.encryption.epoch ?? '?'}, rotated ${s.encryption.rotations} time(s) this call.</details>`;
    paint(top, h);
  }

  // Writes the two text parts of the panel, each only when its markup changed; the tile host between them is never touched.
  function paint(top, bottom) {
    for (const [id, html] of [['c-top', top], ['c-bottom', bottom]]) { const el = $q('#' + id, panel); if (el.__html !== html) { el.innerHTML = html; el.__html = html; } }
    wireBasic();
    // Call tiles use the same seal as the rest of the app; let profile pictures take its place when there are any.
    if (window.__commonsFillAvatars) window.__commonsFillAvatars(panel);
  }

  function syncTiles(shares) {
    const host = $q('#c-shares', panel); if (!host) return;
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
    panel.classList.toggle('wide', shares.length > 0);
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

  // Jellyfin sign-in inside Settings. Quick Connect is the normal path: Jellyfin shows a code, the user approves it there,
  // and the poll below picks up the access token. A pasted API key stays available under Advanced.
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
    if (panel && !panel.hidden) render();
  }
  async function startQuickConnect() {
    if (qcSession) return;
    const btn = $q('#c-jfqc', panel);
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

  function wireBasic() {
    const on = (id, fn) => { const el = $q('#' + id, panel); if (el) el.onclick = fn; };
    on('c-close', () => { open = false; closePlayDialog(); cancelQuickConnect(); render(); });
    on('c-join', async () => { const room = currentRoom(); if (!room) return; try { await call.join(room.gid, room.name); } catch { } render(); });
    on('c-browser', async () => { const room = currentRoom(); try { const r = await rpc('open_call_in_browser', { gid: room.gid, name: room.name }); if (r.opened) toast('Opened in your browser'); call.fail(r.opened ? 'handoff' : 'browser', (r.opened ? 'The call should now be open in your browser. If nothing appeared, open' : 'Could not start a browser automatically. Open') + ' this link in Chrome, Brave, Edge or Firefox on this computer: ' + r.url);  } catch (e) { call.fail('browser', e.message); } });
    on('c-leave', () => { closePlayDialog(); cancelQuickConnect(); call.leave(); });
    on('c-mute', () => { if (call.gameMuted && !call.muted) call.clearGameKey(); else { const m = !call.muted; call.setMuted(m); if (!m) call.clearGameKey(); } });
    on('c-deaf', () => call.setDeafened(!call.deafened));
    on('c-share', async () => { try { call.sharing ? await call.stopShare() : await call.startShare(); } catch { } });
    on('c-playopen', () => openPlayDialog());
    on('c-playtoggle', () => call.togglePlayback());
    on('c-playstop', () => call.stopPlayback());
    const sk = $q('#c-seek', panel); if (sk) sk.onchange = () => call.seekPlayback(Number(sk.value));
    const pv = $q('#c-pvol', panel); if (pv) pv.oninput = () => call.setPlaybackVolume(Number(pv.value));
    on('c-cam', () => call.setCamera(!call.camera));
    on('c-set', async () => { showSettings = !showSettings; if (showSettings) devices = await call.listDevices(); render(); });
    const p = $q('#c-ptt', panel); if (p) { p.onmousedown = () => call.pttButton(true); p.onmouseup = p.onmouseleave = () => call.pttButton(false); }
    panel.querySelectorAll('[data-vol]').forEach(r => r.oninput = () => call.setVolume(r.dataset.vol, Number(r.value)));
    const sel = (id, kind) => { const el = $q('#' + id, panel); if (el) el.onchange = () => call.pickDevice(kind, el.value); };
    sel('c-mic', 'audioinput'); sel('c-out', 'audiooutput'); sel('c-camsel', 'videoinput');
    const ns = $q('#c-ns', panel); if (ns) ns.onchange = () => call.setNoiseSuppression(ns.checked);
    const pc = $q('#c-pttc', panel); if (pc) pc.onchange = () => call.setPtt(pc.checked);
    const sa = $q('#c-sa', panel); if (sa) sa.onchange = () => { call.prefs.shareAudio = sa.checked; call.savePrefs(); };
    const pr = $q('#c-prof', panel); if (pr) pr.onchange = () => { call.prefs.shareProfile = pr.value; call.savePrefs(); };
    const jfs = $q('#c-jfsave', panel);
    if (jfs) jfs.onclick = () => { call.saveJellyfinUrl($q('#c-jf-url', panel).value); toast('Jellyfin address saved on this device'); render(); };
    const jfqc = $q('#c-jfqc', panel); if (jfqc) jfqc.onclick = () => startQuickConnect();
    const jfcancel = $q('#c-jfcancel', panel); if (jfcancel) jfcancel.onclick = () => cancelQuickConnect('Quick Connect cancelled');
    const jfso = $q('#c-jfsignout', panel); if (jfso) jfso.onclick = () => { call.jellyfinSignOut(); toast('Signed out of Jellyfin on this device'); render(); };
    const jfkey = $q('#c-jfsavekey', panel);
    if (jfkey) jfkey.onclick = () => {
      const k = $q('#c-jf-key', panel).value;
      call.saveJellyfinConfig($q('#c-jf-url', panel).value, k);
      toast(k ? 'API key saved on this device' : 'API key cleared');
      render();
    };
    const runes = $q('#c-runesave', panel);
    if (runes) runes.onclick = () => { call.saveRuneConfig($q('#c-rune-url', panel).value); toast('rune address saved on this device'); render(); };
    on('c-prev', () => call.runePrev());
    on('c-next', () => call.runeNext());
    const pk = $q('#c-pttkey', panel); if (pk) pk.onclick = () => { pk.textContent = 'Press a key…'; const h = e => { e.preventDefault(); document.removeEventListener('keydown', h, true); pk.textContent = 'Change key'; call.setPttKey(e.code); }; document.addEventListener('keydown', h, true); };
  }

  call.addEventListener('change', () => { if (call.phase === 'connected' || call.phase === 'error' || call.phase === 'idle' || call.phase === 'reconnecting' || call.phase === 'connecting') render(); });
  setInterval(() => { ensureButton(); updateButton(); if (window.__commonsRefreshAvatars) window.__commonsRefreshAvatars(); }, 700);
  ensurePanel();
  // Also used by the rail's call strip: open the panel and join this room's call. If this device is already in that
  // call (its own row was clicked), just reveal the panel rather than leaving and joining again.
  window.__callUI = {
    open() { open = true; ensurePanel(); render(); },
    render,
    joinRoom(gid, name) {
      open = true; ensurePanel();
      if (call.gid === gid && call.phase !== 'idle') { render(); return; }
      call.join(gid, name).catch(() => { });
      render();
    },
  };
})();
