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
    </g></svg>`);
  }
  const ic = n => `<svg class="ic"><use href="#${n}"/></svg>`;
  window.__commonsCall = call;               // used by the headless tests
  const $q = (s, r = document) => r.querySelector(s);
  const hue = s => { let h = 0; for (const c of s || 'x') h = (h * 31 + c.charCodeAt(0)) % 360; return h; };
  let panel = null, open = false, showSettings = false, devices = null;
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
    document.body.appendChild(panel);
  }

  function encBadge(s) {
    const e = s.encryption;
    if (s.phase === 'idle') return '';
    if (e.active) return `<span class="call-enc ok" title="Every audio and video frame is encrypted on your device with keys from this room's MLS group before it is sent. Frame cipher: AES-GCM; backend: ${esc(e.backend)}.">${ic('i-lock')}Encrypted · key ${e.epoch ?? '?'}</span>`;
    return `<span class="call-enc bad">NOT encrypted</span>`;
  }

  function peerRow(p) {
    const tags = [];
    if (p.me) tags.push('you');
    if (p.muted) tags.push('muted');
    if (!p.member) tags.push('<span class="w">not in this room: blocked</span>');
    if (p.cantDecrypt) tags.push('<span class="w">cannot decrypt (keys out of sync)</span>');
    return `<div class="cp ${p.speaking ? 'speaking' : ''}" data-id="${esc(p.identity)}">
      <div class="av" style="background:hsl(${hue(p.account || p.identity)} 50% 38%)">${esc((p.name || '?').slice(0, 1).toUpperCase())}</div>
      <div><div class="nm">${esc(p.name)}</div><div class="tags">${tags.join(' · ') || (p.speaking ? 'speaking' : '&nbsp;')}</div></div>
      ${p.me ? '' : `<input type="range" min="0" max="1" step="0.05" value="${p.volume}" aria-label="Volume for ${esc(p.name)}" data-vol="${esc(p.identity)}">`}</div>`;
  }

  function devSelect(id, list, sel, label) {
    return `<label for="${id}">${label}</label><select id="${id}"><option value="">System default</option>${list.map(d => `<option value="${esc(d.deviceId)}" ${d.deviceId === sel ? 'selected' : ''}>${esc(d.label || 'Device')}</option>`).join('')}</select>`;
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
    if (panel.contains(document.activeElement) && document.activeElement.matches('input[type=range],select')) return;   // do not fight a drag in progress
    let h = `<div class="ch"><h3>${esc(inThis || s.phase !== 'idle' ? (s.roomName || 'Call') : (room ? room.name : 'Call'))}</h3>${encBadge(s)}<button class="mini" id="c-close" title="Hide (the call keeps running)">×</button></div>`;
    if (!sup.ok) {
      const why = `<details class="cnote"><summary>Why?</summary>${sup.reasons.map(esc).join('<br>')}</details>`;
      h += window.__TAURI__
        ? `<div class="cnote"><b>On this computer, calls open in your browser.</b> The app window cannot do encrypted calls here, so the call runs in a browser tab: same room, same end-to-end encryption. Use Chrome, Brave, Edge or Firefox. If you are asked for the microphone, allow it. Close the tab to leave the call.</div>
           ${room ? '<div class="cbar"><button class="btn" id="c-browser">Open this call in my browser</button></div>' : ''}${why}`
        : `<div class="cerr"><b>Calls are not available in this window.</b><br>${sup.reasons.map(esc).join('<br>')}</div><div class="cnote">Commons will never fall back to an unencrypted call. Try a current Chromium-based browser or Firefox.</div>`;
      panel.innerHTML = h; wireBasic(); return;
    }
    if (s.phase === 'idle' || (s.phase === 'error' && !inThis && !s.gid)) {
      h += `${s.error ? `<div class="cerr">${esc(s.error.msg)}</div>` : ''}
        <div class="cinfo">${room ? `Join the voice room for <b>${esc(room.name)}</b>. Anyone in this room can join; media is encrypted with keys from the room's group and the call server only forwards ciphertext.` : 'Open a room first.'}</div>
        <div class="cbar"><button class="btn" id="c-join" ${room ? '' : 'disabled'}>Join voice</button></div>`;
      panel.innerHTML = h; wireBasic(); return;
    }
    if (s.error) h += `<div class="cerr">${esc(s.error.msg)}</div>`;
    if (s.phase === 'connecting') h += '<div class="cinfo">Connecting…</div>';
    if (s.phase === 'reconnecting') h += `<div class="cinfo">Connection lost${s.attempt ? ' (attempt ' + s.attempt + ')' : ''}. Reconnecting; your place in the call is kept if it comes back quickly.</div>`;
    if (s.blocked.length) h += `<div class="cerr">Blocked ${s.blocked.length} stream(s) that are not from a current member or not encrypted. They are not played.</div>`;
    h += `<div class="cshares" id="c-shares"></div>`;
    h += `<div class="cpeople">${s.peers.map(peerRow).join('')}</div>`;
    const ptt = s.ptt;
    h += `<div class="cbar">
      <button class="btn ${s.muted ? 'on' : 'ghost'}" id="c-mute">${ic(s.muted ? 'ci-micoff' : 'ci-mic')}${s.muted ? 'Unmute' : 'Mute'}</button>
      <button class="btn ${s.deafened ? 'on' : 'ghost'}" id="c-deaf">${ic('ci-phones')}${s.deafened ? 'Undeafen' : 'Deafen'}</button>
      ${ptt ? `<button class="btn ${s.pttHeld ? '' : 'ghost'}" id="c-ptt">Hold to talk</button>` : ''}
      <button class="btn ${s.sharing ? 'on' : 'ghost'}" id="c-share" ${sup.canShareScreen ? '' : 'disabled title="This browser or webview cannot share a screen"'}>${ic('ci-screen')}${s.sharing ? 'Stop sharing' : 'Share screen'}</button>
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
        <label for="c-prof">Share as</label><select id="c-prof">${Object.entries(call.shareProfiles()).map(([k, v]) => `<option value="${k}" ${s.prefs.shareProfile === k ? 'selected' : ''}>${esc(v.label)}</option>`).join('')}</select></div>`;
    }
    h += `<div class="cnote">Call server sees: your IP address, when you join and leave, how much you send, and who is speaking. It cannot hear or see anything. Keys: ${s.encryption.devices ?? '?'} device keys from the room, epoch ${s.encryption.epoch ?? '?'}, rotated ${s.encryption.rotations} time(s) this call.</div>`;
    panel.classList.toggle('wide', s.shares.length > 0);
    panel.innerHTML = h;
    syncTiles(s);
    wireBasic(s);
  }

  function syncTiles(s) {
    const host = $q('#c-shares'); if (!host) return;
    const want = new Set(s.shares.map(x => x.key));
    for (const [k, t] of [...tiles]) if (!want.has(k)) { try { t.track.detach(t.video); } catch { } tiles.delete(k); }
    for (const sh of s.shares) {
      let t = tiles.get(sh.key);
      if (!t) {
        const video = document.createElement('video'); video.autoplay = true; video.playsInline = true; video.muted = true;
        sh.track.attach(video); t = { video, track: sh.track }; tiles.set(sh.key, t);
      }
      const d = document.createElement('div'); d.className = 'ctile'; d.dataset.key = sh.key;
      d.appendChild(t.video);
      const bar = document.createElement('div'); bar.className = 'bar';
      const label = sh.source === 'camera' ? 'camera' : 'screen';
      bar.innerHTML = `<span class="grow">${esc(sh.name)} · ${label}</span>` + (sh.local ? '' : `<select data-q="${esc(sh.sid)}" aria-label="Quality"><option value="auto">Auto</option><option value="high">High</option><option value="medium">Medium</option><option value="low">Low</option></select>`) + `<button data-fs="${esc(sh.key)}">Fullscreen</button>`;
      const sel = bar.querySelector('select'); if (sel) sel.value = sh.quality;
      d.appendChild(bar); host.appendChild(d); t.video.play().catch(() => { });
    }
  }

  function wireBasic(s) {
    const on = (id, fn) => { const el = $q('#' + id, panel); if (el) el.onclick = fn; };
    on('c-close', () => { open = false; render(); });
    on('c-join', async () => { const room = currentRoom(); if (!room) return; try { await call.join(room.gid, room.name); } catch { } render(); });
    on('c-browser', async () => { const room = currentRoom(); try { const r = await rpc('open_call_in_browser', { gid: room.gid, name: room.name }); if (r.opened) toast('Opened in your browser'); else call.fail('browser', 'Could not start a browser automatically. Open this link in Chrome, Edge, Brave, Chromium or Firefox on this computer: ' + r.url);  } catch (e) { call.fail('browser', e.message); } });
    on('c-leave', () => call.leave());
    on('c-mute', () => call.setMuted(!call.muted));
    on('c-deaf', () => call.setDeafened(!call.deafened));
    on('c-share', async () => { try { call.sharing ? await call.stopShare() : await call.startShare(); } catch { } });
    on('c-cam', () => call.setCamera(!call.camera));
    on('c-set', async () => { showSettings = !showSettings; if (showSettings) devices = await call.listDevices(); render(); });
    const p = $q('#c-ptt', panel); if (p) { p.onmousedown = () => call.pttButton(true); p.onmouseup = p.onmouseleave = () => call.pttButton(false); }
    panel.querySelectorAll('[data-vol]').forEach(r => r.oninput = () => call.setVolume(r.dataset.vol, Number(r.value)));
    panel.querySelectorAll('[data-q]').forEach(sel => sel.onchange = () => call.setViewQuality(sel.dataset.q, sel.value));
    panel.querySelectorAll('[data-fs]').forEach(b => b.onclick = () => { const t = tiles.get(b.dataset.fs); if (t) (t.video.requestFullscreen || t.video.webkitRequestFullscreen).call(t.video); });
    const sel = (id, kind) => { const el = $q('#' + id, panel); if (el) el.onchange = () => call.pickDevice(kind, el.value); };
    sel('c-mic', 'audioinput'); sel('c-out', 'audiooutput'); sel('c-camsel', 'videoinput');
    const ns = $q('#c-ns', panel); if (ns) ns.onchange = () => call.setNoiseSuppression(ns.checked);
    const pc = $q('#c-pttc', panel); if (pc) pc.onchange = () => call.setPtt(pc.checked);
    const sa = $q('#c-sa', panel); if (sa) sa.onchange = () => { call.prefs.shareAudio = sa.checked; call.savePrefs(); };
    const pr = $q('#c-prof', panel); if (pr) pr.onchange = () => { call.prefs.shareProfile = pr.value; call.savePrefs(); };
    const pk = $q('#c-pttkey', panel); if (pk) pk.onclick = () => { pk.textContent = 'Press a key…'; const h = e => { e.preventDefault(); document.removeEventListener('keydown', h, true); call.setPttKey(e.code); }; document.addEventListener('keydown', h, true); };
  }

  call.addEventListener('change', () => { if (call.phase === 'connected' || call.phase === 'error' || call.phase === 'idle' || call.phase === 'reconnecting' || call.phase === 'connecting') render(); });
  setInterval(() => { ensureButton(); updateButton(); }, 700);
  ensurePanel();
  window.__callUI = { open() { open = true; ensurePanel(); render(); }, render };
})();
