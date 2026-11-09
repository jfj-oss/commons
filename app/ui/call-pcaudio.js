'use strict';
// "Play PC audio into the call": send the sound of a browser tab, or of one app on a Linux computer, to everyone in the call.
// It is published exactly like "Play into the call" music (stereo 192 kbps, end-to-end encrypted, a picture on a canvas, the
// same 'music' / 'music-audio' tracks) and is the call's `play` state, so only one thing plays at a time and the existing Stop
// control ends it. All voice processing is off. The sender hears the sound locally the way they always did, and nothing here
// plays it a second time: the audio goes into Web Audio only to be published, never to the speakers.
//   Route 1, a browser tab (any OS with a Chromium browser): getDisplayMedia, keep its audio, drop its video.
//   Route 2, an app (Linux, PipeWire): the native core makes a virtual source and moves ONE stream onto it (pcaudio.rs).
(() => {
  const Call = window.CommonsCall, CallError = window.CommonsCallError;
  if (!Call) return;
  const SILENCE_MS = 5000;          // a tab that has made no sound this long is reported
  const GONE_STOP_S = 10;           // an app whose stream has been gone this long ends the share
  const DEVICE_WAIT_MS = 8000;      // the virtual input can take a moment to show up in the browser's device list
  const NO_SOUND = 'No sound is coming from that tab. Make sure it is playing, and that you ticked "Share tab audio" when you picked it. Some protected players (for example Spotify\'s web player) may not allow their sound to be captured.';
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const OFF = { echoCancellation: false, noiseSuppression: false, autoGainControl: false };

  const proto = Call.prototype;

  // PC audio adds a note to the playback state (shown under the title); everything else is the normal music state.
  const basePlaybackState = proto.playbackState;
  proto.playbackState = function () {
    const s = basePlaybackState.call(this);
    if (s && this.play && this.play.kind === 'pcaudio') { s.note = this.play.note || ''; s.level = this.play.level || 0; }
    return s;
  };

  // ------------------------------------------------------------------ what this device can do
  proto.pcAudioTabSupported = function () {
    const ua = navigator.userAgent || '';
    if (/iPhone|iPad|iPod/.test(ua)) return false;
    if (!navigator.mediaDevices || typeof navigator.mediaDevices.getDisplayMedia !== 'function') return false;
    return !!(navigator.userAgentData || /Chrom(e|ium)\//.test(ua));   // only Chromium browsers hand over a tab's audio
  };
  // Route 2 needs the native core on Linux with PipeWire's tools. Anything else says no (and the menu hides it).
  proto.pcAudioAppSupported = async function () {
    if (!/Linux/.test(navigator.platform || '') && !/Linux/.test(navigator.userAgent || '')) return false;
    try { const c = await this.rpc('pcaudio_caps', {}); return !!(c && c.supported); } catch { return false; }
  };
  proto.pcAudioStreams = async function () { return this.rpc('pcaudio_streams', {}); };

  // ------------------------------------------------------------------ shared: graph, picture, publishing
  proto._pcGraph = function (atrack) {
    const actx = new (window.AudioContext || window.webkitAudioContext)({ latencyHint: 'playback' });
    const src = actx.createMediaStreamSource(new MediaStream([atrack]));
    const gain = actx.createGain(); gain.gain.value = 1;
    const an = actx.createAnalyser(); an.fftSize = 1024;
    const dest = actx.createMediaStreamDestination();
    dest.channelCount = 2; dest.channelCountMode = 'explicit'; dest.channelInterpretation = 'speakers';
    // To the publisher and to the meter. NOT to actx.destination: the person already hears the sound where it plays.
    src.connect(gain); gain.connect(dest); src.connect(an);
    return { actx, gain, dest, an, src };
  };
  proto._pcPaint = function (canvas, st) {
    const secs = Math.floor((Date.now() - st.startedAt) / 1000);
    this._paintMusicCanvas(canvas, { title: st.title, artist: st.artist, albumTitle: '' }, null);
    const g = canvas.getContext('2d');
    g.fillStyle = '#9e8d70'; g.font = '22px sans-serif'; g.textAlign = 'left';
    g.fillText(st.sourceLabel + ' · ' + Math.floor(secs / 60) + ':' + String(secs % 60).padStart(2, '0'), 70 + 500 + 60, canvas.height - 120);
  };

  // The whole publishing path, given the captured audio track. `o`: { title, artist, sourceLabel, sources: [tracks to stop],
  // cleanup(), onTick(st) }. Returns the playback state. Everything is undone if any step fails.
  proto._pcPublish = async function (atrackIn, o) {
    if (!this.room) throw new CallError('idle', 'Join the call first.');
    if (this.sharing) throw new CallError('busy', 'Stop sharing your screen before playing sound into the call.');
    if (this.play) throw new CallError('busy', 'Something is already playing into the call. Stop it first.');
    const LK = this.LK;
    let graph = null, vpub = null, apub = null, vtrack = null, atrack = null, timer = null;
    const st = {
      title: o.title, artist: o.artist || '', kind: 'pcaudio', profile: 'music', paused: false, volume: 1, capture: 'canvas', width: 1280, height: 720,
      sourceLabel: o.sourceLabel, startedAt: Date.now(), note: '', level: 0, queue: null, coverUrl: '',
    };
    try {
      graph = this._pcGraph(atrackIn);
      await graph.actx.resume().catch(() => { });
      const canvas = document.createElement('canvas'); canvas.width = 1280; canvas.height = 720;
      this._pcPaint(canvas, st);
      const vstream = canvas.captureStream(2);
      vtrack = vstream.getVideoTracks()[0];
      if (vtrack) vtrack.contentHint = 'detail';
      atrack = graph.dest.stream.getAudioTracks()[0];
      if (atrack) atrack.contentHint = 'music';
      if (!vtrack) throw new CallError('play', 'The picture could not be captured as a video track.');
      if (!atrack) throw new CallError('play', 'The sound could not be captured as an audio track.');
      vpub = await this.room.localParticipant.publishTrack(vtrack, {
        name: 'music', source: LK.Track.Source.ScreenShare,
        screenShareEncoding: { maxBitrate: 600_000, maxFramerate: 2 },
        videoCodec: 'vp8', degradationPreference: 'maintain-framerate',
      });
      apub = await this.room.localParticipant.publishTrack(atrack, {
        name: 'music-audio', source: LK.Track.Source.ScreenShareAudio,
        audioPreset: { maxBitrate: 192_000, priority: 'high' },
        forceStereo: true, dtx: false, red: true,
      });
      for (const pub of [vpub, apub]) {
        if (!pub || !pub.isEncrypted) throw new CallError('no-e2ee', 'The sound was not encrypted, so it was stopped. Nothing was sent.');
      }
      Object.assign(st, { graph, actx: graph.actx, gain: graph.gain, dest: graph.dest, vpub, apub, vtrack, atrack, canvas });
      st.cleanup = () => { clearTimeout(timer); st.dead = true; try { o.cleanup && o.cleanup(); } catch { } for (const t of o.sources || []) { try { t.stop(); } catch { } } };
      this.play = st;
      // redrawn each second (a still picture would stop the video track from sending), and the sound level is read
      const buf = new Float32Array(graph.an.fftSize);
      let quietSince = Date.now(), heard = false, warned = false;
      const tick = () => {
        if (st.dead || this.play !== st) return;
        try {
          graph.an.getFloatTimeDomainData(buf);
          let peak = 0; for (let i = 0; i < buf.length; i++) { const a = Math.abs(buf[i]); if (a > peak) peak = a; }
          st.level = peak;
          if (peak > 0.0005) {
            heard = true; quietSince = Date.now();
            if (warned) { warned = false; st.note = ''; if (this.error && this.error.kind === 'pcaudio-silent') { this.error = null; } }
          } else if (o.warnSilence && !heard && !warned && Date.now() - quietSince > SILENCE_MS) {
            warned = true; st.note = NO_SOUND; this.fail('pcaudio-silent', NO_SOUND);
          }
          if (o.onTick) o.onTick(st);
          this._pcPaint(canvas, st);
        } catch { }
        timer = setTimeout(tick, 1000);
        this.changed();
      };
      timer = setTimeout(tick, 1000);
      this.log('playing ' + o.sourceLabel + ' "' + st.title + '" into the call');
      this.changed();
      return this.playbackState();
    } catch (e) {
      clearTimeout(timer);
      for (const pub of [apub, vpub]) { try { if (pub && pub.track) await this.room.localParticipant.unpublishTrack(pub.track); } catch { } }
      for (const t of [vtrack, atrack]) { try { t && t.stop(); } catch { } }
      if (graph) { try { graph.actx.close(); } catch { } }
      try { o.cleanup && o.cleanup(); } catch { }
      for (const t of o.sources || []) { try { t.stop(); } catch { } }
      throw e;
    }
  };

  // ------------------------------------------------------------------ route 1: a browser tab
  proto.startTabAudio = async function () {
    if (!this.room) throw new CallError('idle', 'Join the call first.');
    if (this.play) throw new CallError('busy', 'Something is already playing into the call. Stop it first.');
    if (!this.pcAudioTabSupported()) throw new CallError('unsupported', 'This browser cannot share a tab\'s sound. Use a Chromium-based browser (Chrome, Brave, Edge) on a computer.');
    let ms;
    try {
      ms = await navigator.mediaDevices.getDisplayMedia({
        video: true,
        audio: { ...OFF, channelCount: 2, suppressLocalAudioPlayback: false },   // the tab stays audible to the person sharing it
        // Only tabs are offered: the call's own tab and the whole screen/system sound would feed the call back into itself.
        selfBrowserSurface: 'exclude', monitorTypeSurfaces: 'exclude', systemAudio: 'exclude', surfaceSwitching: 'exclude', preferCurrentTab: false,
      });
    } catch (e) {
      const denied = e && (e.name === 'NotAllowedError' || e.name === 'AbortError');
      throw new CallError('pcaudio', denied ? 'Sharing the tab was cancelled.' : 'The browser could not share a tab: ' + ((e && e.message) || e));
    }
    const vts = ms.getVideoTracks(), ats = ms.getAudioTracks();
    const stopAll = () => { for (const t of ms.getTracks()) { try { t.stop(); } catch { } } };
    const surface = vts[0] && vts[0].getSettings ? vts[0].getSettings().displaySurface : '';
    if (surface && surface !== 'browser') { stopAll(); throw new CallError('pcaudio', 'That was a window or a screen, not a browser tab. Pick a tab, or the call itself would be sent back into the call.'); }
    if (!ats.length) { stopAll(); throw new CallError('pcaudio', 'No sound was shared. Pick the tab again and tick "Share tab audio" in the window that asks.'); }
    // The picture is not used: the call shows its own cover-style picture, like all music. Drop the tab's video.
    for (const v of vts) { try { v.stop(); } catch { } try { ms.removeTrack(v); } catch { } }
    const at = ats[0];
    try { await at.applyConstraints({ ...OFF }); } catch { }
    const st = await this._pcPublish(at, { title: 'Audio from a browser tab', artist: '', sourceLabel: 'Browser tab', sources: [at], warnSilence: true });
    // The person ends the share with the browser's own "Stop sharing" bar: end ours too.
    const mine = this.play;
    at.addEventListener('ended', () => { if (this.play === mine && !mine.dead) { this.fail('pcaudio', 'The tab stopped being shared, so the sound was stopped.'); this.stopPlayback(); } });
    return st;
  };

  // ------------------------------------------------------------------ route 2: an app on Linux
  proto._pcFindInput = async function (label) {
    const find = async () => {
      const all = (await navigator.mediaDevices.enumerateDevices()).filter(d => d.kind === 'audioinput');
      return { all, hit: all.find(d => d.label === label) };
    };
    let r = await find();
    if (!r.hit && r.all.length && r.all.every(d => !d.label)) {
      // labels stay hidden until the page has been given the microphone once (a call normally has it already)
      try { const t = await navigator.mediaDevices.getUserMedia({ audio: OFF }); t.getTracks().forEach(x => x.stop()); } catch { }
      r = await find();
    }
    const t0 = Date.now();
    while (!r.hit && Date.now() - t0 < DEVICE_WAIT_MS) { await sleep(400); r = await find(); }
    return r.hit || null;
  };
  proto.startAppAudio = async function (streamId) {
    if (!this.room) throw new CallError('idle', 'Join the call first.');
    if (this.play) throw new CallError('busy', 'Something is already playing into the call. Stop it first.');
    let started;
    try { started = await this.rpc('pcaudio_start', { id: Number(streamId) }); }
    catch (e) { throw new CallError('pcaudio', (e && e.message) || String(e)); }
    const undo = () => { this.rpc('pcaudio_stop', {}).catch(() => { }); };
    let track = null, poll = null, onHide = null;
    try {
      const label = started.label || 'Megatron PC audio';
      const dev = await this._pcFindInput(this.pcAudioLabelOverride || label);
      if (!dev) throw new CallError('pcaudio', 'The browser did not list the sound input "' + label + '". Close the picker and try again; if it keeps failing, the browser may be hiding virtual inputs.');
      const ms = await navigator.mediaDevices.getUserMedia({ video: false, audio: { deviceId: { exact: dev.deviceId }, ...OFF, channelCount: { ideal: 2 } } });
      track = ms.getAudioTracks()[0];
      if (!track) throw new CallError('pcaudio', 'The sound input could not be opened.');
      const set = track.getSettings ? track.getSettings() : {};
      if (set.echoCancellation || set.noiseSuppression || set.autoGainControl) { try { await track.applyConstraints({ ...OFF }); } catch { } }
      let goneFor = 0;
      const stop = async msg => { this.fail('pcaudio', msg); await this.stopPlayback(); };
      const o = {
        title: started.title || started.app || 'App audio', artist: started.title ? started.app : '', sourceLabel: started.app || 'App', sources: [track],
        cleanup: () => { clearInterval(poll); if (onHide) { window.removeEventListener('pagehide', onHide); onHide = null; } undo(); },
        onTick: null,
      };
      const st = await this._pcPublish(track, o);
      const mine = this.play;
      // Keeps the route alive (the native side undoes everything if this stops), follows the track title, and notices the end.
      poll = setInterval(async () => {
        if (!this.play || this.play !== mine) { clearInterval(poll); return; }
        let s; try { s = await this.rpc('pcaudio_status', {}); } catch { return; }
        if (!this.play || this.play !== mine) return;
        if (!s.active) { await stop(s.reason || 'The sound route ended, so sharing was stopped.'); return; }
        if (s.title && s.title !== mine.title) { mine.title = s.title; }
        if (s.app && s.app !== mine.artist) { mine.artist = s.app; }
        if (!s.alive) { goneFor = s.gone_secs || 0; mine.note = 'That app has stopped playing.'; if (goneFor >= GONE_STOP_S) await stop('That app stopped playing, so sharing its sound was stopped.'); }
        else if (mine.note === 'That app has stopped playing.') mine.note = '';
        mine.paused = false;
      }, 1000);
      onHide = () => undo();
      window.addEventListener('pagehide', onHide);
      track.addEventListener('ended', () => { if (this.play === mine) { this.fail('pcaudio', 'The sound input went away, so sharing was stopped.'); this.stopPlayback(); } });
      return st;
    } catch (e) {
      clearInterval(poll); if (onHide) window.removeEventListener('pagehide', onHide);
      try { track && track.stop(); } catch { }
      undo();
      const ce = e instanceof CallError ? e : new CallError('pcaudio', 'Could not send that app\'s sound: ' + ((e && e.message) || e));
      this.fail(ce.kind, ce.message);
      throw ce;
    }
  };

  // ------------------------------------------------------------------ the picker section in "Play into the call"
  const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  window.CommonsPcAudioUI = {
    async mount(host, call, hooks = {}) {
      if (!host) return;
      const tab = call.pcAudioTabSupported();
      const app = await call.pcAudioAppSupported();
      if (!tab && !app) { host.innerHTML = ''; host.hidden = true; return; }
      host.hidden = false;
      host.innerHTML = `<div class="pd-row"><b>Sound from this computer</b> <span class="muted small">send music or any sound to everyone in the call</span></div>
        <div class="pd-row">${tab ? '<button class="btn" id="pc-tab">A browser tab</button>' : ''}${app ? '<button class="btn" id="pc-app">An app on this computer</button>' : ''}<span id="pc-status" class="muted small"></span></div>
        <p class="muted small" id="pc-help">${tab ? 'Browser tab: pick the tab, and tick <b>Share tab audio</b>. Only that tab\'s sound is sent, never the call. ' : ''}${app ? 'App: pick the app that is playing, for example Spotify. You keep hearing it as usual.' : ''}</p>
        <div id="pc-list" class="pd-results"></div>`;
      const $ = s => host.querySelector(s), say = t => { $('#pc-status').textContent = t || ''; };
      const done = () => { say(''); hooks.close && hooks.close(); hooks.toast && hooks.toast('Playing into the call'); };
      if (tab) $('#pc-tab').onclick = async () => {
        $('#pc-tab').disabled = true; say('Waiting for you to pick a tab…');
        try { await call.startTabAudio(); done(); } catch (e) { say(e.message); } finally { const b = $('#pc-tab'); if (b) b.disabled = false; }
      };
      if (app) $('#pc-app').onclick = async () => {
        say('Looking for apps that are playing…'); $('#pc-list').innerHTML = '';
        try {
          const r = await call.pcAudioStreams();
          const list = r.streams || [];
          const hid = r.browser_hidden ? ' Browser sound is not listed here: use "A browser tab".' : '';
          say(list.length ? 'Pick the one to send.' + hid : 'No app is playing sound right now. Start the music, then try again.' + hid);
          $('#pc-list').innerHTML = list.map(s => `<button class="pd-item" data-id="${Number(s.id)}" title="${esc(s.app + ' — ' + s.title)}"><span class="nm">${esc(s.app)}</span><span class="sub">${esc(s.title)}${s.corked ? ' · paused' : ''}</span></button>`).join('');
          $('#pc-list').querySelectorAll('[data-id]').forEach(b => b.onclick = async () => {
            b.disabled = true; say('Starting…');
            try { await call.startAppAudio(Number(b.dataset.id)); done(); } catch (e) { say(e.message); b.disabled = false; }
          });
        } catch (e) { say(e.message); }
      };
    },
  };
})();
