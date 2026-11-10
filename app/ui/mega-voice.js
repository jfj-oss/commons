/* Voice messages: record a short clip in the composer, play one on a message.
   A clip travels as an ordinary encrypted file (audio/webm or audio/mp4) with two extras in the message: its length
   and a 64-byte picture of the sound. An older app shows it as an audio file. Nothing here plays by itself, and
   nothing records without the bar that says so. Uses app.js globals: rpc, S, $, esc, icon, toast, fileBlob, b64u, tick, soon. */
(() => {
  'use strict';
  const MAX_MS = 5 * 60 * 1000, MIN_MS = 600, BARS = 64, HOLD_MS = 350, CANCEL_PX = 90, SAMPLE_MS = 50;
  const SPEEDS = [1, 1.5, 2];

  const fmtClock = ms => { const s = Math.max(0, Math.round(ms / 1000)); return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`; };
  const hexToBytes = h => { const o = []; for (let i = 0; i + 1 < (h || '').length; i += 2) o.push(parseInt(h.slice(i, i + 2), 16) || 0); return o; };
  const bytesToHex = a => a.map(b => b.toString(16).padStart(2, '0')).join('');

  // ---------- playback ----------
  const players = new Map();     // message id -> { audio, url, loading, failed }
  let speed = 1, playing = null; // the one clip that is playing right now

  const playType = mime => /^audio\/webm/.test(mime) ? 'audio/webm; codecs=opus' : /^audio\/ogg/.test(mime) ? 'audio/ogg; codecs=opus' : mime;
  const canPlay = mime => { try { return !!document.createElement('audio').canPlayType(playType(mime)); } catch { return false; } };

  function row(x) {
    const v = x.file.voice, id = esc(x.id), total = fmtClock(v.ms);
    const peaks = hexToBytes(v.peaks); while (peaks.length < BARS) peaks.push(-1);
    const bars = peaks.slice(0, BARS).map(p => `<i style="--h:${p < 0 ? .18 : (.14 + .86 * p / 255).toFixed(2)}"></i>`).join('');
    const dl = `<a href="#" class="dl vdl" data-id="${id}" title="Download ${esc(x.file.name)}" aria-label="Download this voice message">${icon('download')}</a>`;
    if (!canPlay(x.file.mime)) {
      return `<div class="vmsg nope" data-vid="${id}">${icon('wave', 'fi')}<span class="vnote">Voice message · ${total} · this browser can't play ${esc((x.file.mime.split('/')[1] || 'audio').toUpperCase())}</span>${dl}</div>`;
    }
    return `<div class="vmsg" data-vid="${id}" data-ms="${v.ms}" data-mime="${esc(x.file.mime)}">`
      + `<button type="button" class="vplay" aria-label="Play voice message" title="Play">${icon('play')}</button>`
      + `<div class="vwave" role="slider" tabindex="0" aria-label="Position in the voice message" aria-valuemin="0" aria-valuemax="${v.ms}" aria-valuenow="0" aria-valuetext="0:00 of ${total}"><div class="vbars">${bars}</div><div class="vbars vfill" aria-hidden="true">${bars}</div></div>`
      + `<span class="vtime"><span class="vnow">0:00</span> / <span class="vtot">${total}</span></span>`
      + `<button type="button" class="vspeed" aria-label="Playback speed" title="Playback speed">${speed}×</button>${dl}</div>`;
  }

  const views = id => [...document.querySelectorAll('.vmsg[data-vid]')].filter(e => e.dataset.vid === id);
  function paintView(el) {
    const id = el.dataset.vid, p = players.get(id), ms = +el.dataset.ms || 1;
    const t = p && p.audio ? p.audio.currentTime * 1000 : 0, frac = Math.min(1, t / ms);
    const on = !!(p && p.audio && !p.audio.paused && !p.audio.ended);
    el.style.setProperty('--p', (frac * 100).toFixed(2) + '%');
    el.classList.toggle('playing', on); el.classList.toggle('loading', !!(p && p.loading));
    const b = el.querySelector('.vplay');
    if (b) { b.innerHTML = icon(on ? 'pause' : 'play'); b.setAttribute('aria-label', on ? 'Pause voice message' : 'Play voice message'); b.title = on ? 'Pause' : 'Play'; }
    const now = el.querySelector('.vnow'); if (now) now.textContent = fmtClock(t);
    const w = el.querySelector('.vwave'); if (w) { w.setAttribute('aria-valuenow', Math.round(t)); w.setAttribute('aria-valuetext', `${fmtClock(t)} of ${fmtClock(ms)}`); }
    const sp = el.querySelector('.vspeed'); if (sp) sp.textContent = speed + '×';
    if (p && p.failed && !el.classList.contains('nope')) {
      el.classList.add('nope'); el.innerHTML = `${icon('wave', 'fi')}<span class="vnote">Voice message · ${fmtClock(ms)} · this browser can't play it</span><a href="#" class="dl vdl" data-id="${esc(id)}" title="Download" aria-label="Download this voice message">${icon('download')}</a>`;
    }
  }
  const paintAll = id => views(id).forEach(paintView);

  async function load(id, mime) {
    let p = players.get(id);
    if (p && p.audio) return p;
    if (p && p.loading) return p.loading;
    p = p || {}; players.set(id, p);
    p.loading = (async () => {
      paintAll(id);
      try {
        const f = await fileBlob(id);
        const a = new Audio(); a.preload = 'auto'; a.src = p.url = URL.createObjectURL(f.blob); a.playbackRate = speed;
        a.addEventListener('timeupdate', () => paintAll(id));
        a.addEventListener('play', () => paintAll(id));
        a.addEventListener('pause', () => paintAll(id));
        a.addEventListener('ended', () => { a.currentTime = 0; if (playing === id) playing = null; paintAll(id); });
        a.addEventListener('error', () => { p.failed = true; if (playing === id) playing = null; paintAll(id); });
        p.audio = a;
        // A recorded WebM has no length in its header, so the browser calls it endless and may refuse to seek: nudge it.
        await new Promise(res => { const t = setTimeout(res, 1500); a.addEventListener('loadedmetadata', () => { clearTimeout(t); res(); }, { once: true }); a.addEventListener('error', () => { clearTimeout(t); res(); }, { once: true }); });
        if (a.duration === Infinity) {
          await new Promise(res => { const t = setTimeout(res, 1500); a.addEventListener('timeupdate', () => { clearTimeout(t); res(); }, { once: true }); a.currentTime = 1e7; });
          a.currentTime = 0;
        }
      } catch (er) { p.failed = true; p.audio = null; }
      finally { p.loading = null; }
      paintAll(id);
      return p;
    })();
    return p.loading;
  }
  function pauseOthers(id) { if (playing && playing !== id) { const o = players.get(playing); if (o && o.audio) o.audio.pause(); } }
  async function toggle(id, mime) {
    const p = await load(id, mime); if (!p || !p.audio || p.failed) return;
    if (!p.audio.paused) { p.audio.pause(); return; }
    pauseOthers(id); playing = id; p.audio.playbackRate = speed;
    try { await p.audio.play(); } catch (er) { if (playing === id && p.audio.paused) playing = null; if (er && er.name === 'NotSupportedError') { p.failed = true; paintAll(id); } }   // a browser that wants a fresh tap just stays paused: the clip is loaded now, so the next tap plays at once
  }
  async function seekTo(id, mime, frac) {
    const p = await load(id, mime); if (!p || !p.audio || p.failed) return;
    const ms = +(views(id)[0] || {}).dataset?.ms || 0;
    const target = Math.max(0, Math.min(1, frac)) * ms / 1000;
    p.audio.currentTime = target; paintAll(id);
  }
  function setSpeed() {
    speed = SPEEDS[(SPEEDS.indexOf(speed) + 1) % SPEEDS.length];
    for (const p of players.values()) if (p.audio) p.audio.playbackRate = speed;
    document.querySelectorAll('.vmsg[data-vid]').forEach(paintView);
  }
  const fracAt = (w, e) => { const r = w.getBoundingClientRect(); return r.width ? (e.clientX - r.left) / r.width : 0; };

  let drag = null;
  document.addEventListener('pointerdown', e => {
    const w = e.target.closest && e.target.closest('.vmsg .vwave'); if (!w || e.button) return;
    const el = w.closest('.vmsg'); drag = { id: el.dataset.vid, mime: el.dataset.mime, w, moved: false, last: fracAt(w, e) };
    try { w.setPointerCapture(e.pointerId); } catch { }
    e.preventDefault();
  });
  document.addEventListener('pointermove', e => {
    if (!drag) return; drag.last = fracAt(drag.w, e); drag.moved = true;
    const el = drag.w.closest('.vmsg'); if (el) { el.style.setProperty('--p', (Math.max(0, Math.min(1, drag.last)) * 100).toFixed(2) + '%'); const ms = +el.dataset.ms; el.querySelector('.vnow').textContent = fmtClock(Math.max(0, Math.min(1, drag.last)) * ms); }
  });
  const endDrag = async e => {
    if (!drag) return; const d = drag; drag = null;
    await seekTo(d.id, d.mime, e && e.clientX != null && e.type !== 'pointercancel' ? fracAt(d.w, e) : d.last);
  };
  document.addEventListener('pointerup', endDrag);
  document.addEventListener('pointercancel', endDrag);
  document.addEventListener('click', e => {
    const el = e.target.closest && e.target.closest('.vmsg[data-vid]'); if (!el) return;
    if (e.target.closest('.vplay')) toggle(el.dataset.vid, el.dataset.mime);
    else if (e.target.closest('.vspeed')) setSpeed();
  });
  document.addEventListener('keydown', e => {
    const w = e.target.closest && e.target.closest('.vmsg .vwave'); if (!w) return;
    const el = w.closest('.vmsg'), ms = +el.dataset.ms, p = players.get(el.dataset.vid);
    const now = p && p.audio ? p.audio.currentTime * 1000 : 0;
    const step = { ArrowRight: 5000, ArrowUp: 5000, ArrowLeft: -5000, ArrowDown: -5000 }[e.key];
    if (step != null) { e.preventDefault(); seekTo(el.dataset.vid, el.dataset.mime, (now + step) / ms); }
    else if (e.key === ' ' || e.key === 'Enter') { e.preventDefault(); toggle(el.dataset.vid, el.dataset.mime); }
    else if (e.key === 'Home') { e.preventDefault(); seekTo(el.dataset.vid, el.dataset.mime, 0); }
  });

  // ---------- recording ----------
  const isSafari = /^((?!chrome|android|chromium).)*(safari|applewebkit)/i.test(navigator.userAgent);
  const pickType = () => {
    if (!window.MediaRecorder) return '';
    const t = ['audio/webm;codecs=opus', 'audio/mp4;codecs=mp4a.40.2', 'audio/mp4', 'audio/ogg;codecs=opus', 'audio/webm'];
    if (isSafari) t.unshift(...t.splice(1, 2));
    return t.find(m => { try { return MediaRecorder.isTypeSupported(m); } catch { return false; } }) || '';
  };
  // Where the browser has no MediaRecorder (some WebKitGTK builds), the clip is recorded as raw sound and written as a
  // 16 kHz mono WAV: bigger (about 2 MB a minute) but every browser plays it.
  const PCM_RATE = 16000;
  const canPcm = () => !!(window.AudioContext || window.webkitAudioContext);
  const useMR = () => !forcePcm && !!pickType();
  let forcePcm = false;
  function wavOf(chunks, fromRate) {
    let n = 0; for (const c of chunks) n += c.length;
    const all = new Float32Array(n); let o = 0; for (const c of chunks) { all.set(c, o); o += c.length; }
    const ratio = fromRate / PCM_RATE, len = Math.floor(n / ratio), pcm = new DataView(new ArrayBuffer(44 + len * 2));
    for (let i = 0; i < len; i++) {   // average each slice of the original, which is enough of a low-pass for speech
      const a = Math.floor(i * ratio), b = Math.max(a + 1, Math.floor((i + 1) * ratio)); let sum = 0; for (let j = a; j < b && j < n; j++) sum += all[j];
      pcm.setInt16(44 + i * 2, Math.max(-1, Math.min(1, sum / (b - a))) * 32767, true);
    }
    const w = (at, t) => { for (let i = 0; i < t.length; i++) pcm.setUint8(at + i, t.charCodeAt(i)); };
    w(0, 'RIFF'); pcm.setUint32(4, 36 + len * 2, true); w(8, 'WAVEfmt '); pcm.setUint32(16, 16, true); pcm.setUint16(20, 1, true); pcm.setUint16(22, 1, true);
    pcm.setUint32(24, PCM_RATE, true); pcm.setUint32(28, PCM_RATE * 2, true); pcm.setUint16(32, 2, true); pcm.setUint16(34, 16, true); w(36, 'data'); pcm.setUint32(40, len * 2, true);
    return new Blob([pcm], { type: 'audio/wav' });
  }
  const rec = { st: 'idle' };   // idle | starting | recording | ready ; mode: hold | lock
  let bar = null, canvas = null, timerEl = null, hintEl = null, live = null;

  function ensureBar() {
    if (bar) return;
    bar = document.createElement('div'); bar.id = 'vrec'; bar.className = 'vrec'; bar.hidden = true;
    bar.innerHTML = `<span class="vdot" aria-hidden="true"></span><span class="vtimer" aria-hidden="true">0:00</span><canvas class="vlive" aria-hidden="true" height="32"></canvas>`
      + `<span class="vhint"></span>`
      + `<button type="button" class="mini vcancel" aria-label="Cancel recording" title="Cancel">${icon('trash')}</button>`
      + `<button type="button" class="btn vsend" aria-label="Send voice message" title="Send">${icon('send')}<span>Send</span></button>`
      + `<span class="sr vsr" role="status" aria-live="polite"></span>`;
    $('#composer .slip').appendChild(bar);
    canvas = bar.querySelector('canvas'); timerEl = bar.querySelector('.vtimer'); hintEl = bar.querySelector('.vhint');
    bar.querySelector('.vcancel').onclick = () => finish(false);
    bar.querySelector('.vsend').onclick = () => finish(true);
  }
  const say = t => { const s = bar && bar.querySelector('.vsr'); if (s) s.textContent = t; };
  function show() {
    ensureBar(); const slip = $('#composer .slip');
    const on = rec.st !== 'idle';
    slip.classList.toggle('recording', on); bar.hidden = !on;
    bar.dataset.mode = rec.mode || ''; bar.dataset.state = rec.st; bar.classList.toggle('cancelling', !!rec.cancelling);
    $('#mic').classList.toggle('on', on);
    $('#mic').setAttribute('aria-label', on ? 'Recording a voice message' : 'Record a voice message');
    const full = rec.ms >= MAX_MS;
    hintEl.textContent = rec.st === 'starting' ? 'waiting for the microphone…' : full ? 'five minutes is the most — send or cancel'
      : rec.mode === 'hold' ? (rec.cancelling ? 'release to cancel' : 'release to send · slide away to cancel') : 'recording…';
  }
  function draw() {
    if (!canvas || canvas.hidden) return;
    const dpr = window.devicePixelRatio || 1, w = Math.max(40, Math.floor(canvas.clientWidth)), h = 32;
    if (canvas.width !== w * dpr) { canvas.width = w * dpr; canvas.height = h * dpr; }
    const g = canvas.getContext('2d'); g.setTransform(dpr, 0, 0, dpr, 0, 0); g.clearRect(0, 0, w, h);
    g.fillStyle = getComputedStyle(bar).color || '#fff';
    const step = 4, n = Math.floor(w / step), s = rec.samples || [];
    for (let i = 0; i < n; i++) {
      const v = s[s.length - n + i]; const a = v == null ? .06 : Math.max(.06, Math.min(1, Math.sqrt(v) * 1.15));
      const bh = Math.max(2, a * h); g.fillRect(i * step, (h - bh) / 2, 2, bh);
    }
  }
  function tickRec() {
    if (rec.st !== 'recording') return;
    rec.ms = Math.min(MAX_MS, performance.now() - rec.t0);
    if (live.an) { live.an.getFloatTimeDomainData(live.buf); let m = 0; for (const v of live.buf) { const a = Math.abs(v); if (a > m) m = a; } rec.samples.push(m); }
    else rec.samples.push(0);
    timerEl.textContent = fmtClock(rec.ms); draw();
    if (S.gid !== rec.gid) { toast('Recording stopped: you changed room.'); finish(false); return; }
    if (rec.ms >= MAX_MS) { stopRecorder(); rec.st = 'ready'; show(); say('Five minutes reached. Send or cancel.'); }
  }
  function releaseMic() {
    try { clearInterval(live && live.timer); } catch { }
    if (live) { try { live.stream.getTracks().forEach(t => t.stop()); } catch { } try { live.ctx && live.ctx.close(); } catch { } }
    live = null;
  }
  function stopRecorder() {
    clearInterval(live && live.timer);
    try { if (live && live.mr && live.mr.state !== 'inactive') live.mr.stop(); } catch { }
    try { if (live && live.pr) { live.pr.disconnect(); live.pr.onaudioprocess = null; } } catch { }
    if (live) { try { live.stream.getTracks().forEach(t => t.stop()); } catch { } }
  }
  async function begin(downAt) {
    if (rec.st !== 'idle') return;
    if (!S.gid) return toast('Open a room first.');
    if (S.edit) return;
    const type = useMR() ? pickType() : '';
    let ctx = null;   // made now, while the press still counts as a tap: Safari keeps a context made later muted
    try { const AC = window.AudioContext || window.webkitAudioContext; ctx = new AC(); if (ctx.state === 'suspended') ctx.resume().catch(() => { }); } catch { }
    Object.assign(rec, { st: 'starting', mode: 'pending', gid: S.gid, ms: 0, samples: [], cancelling: false, down: !!downAt, release: null, heldLong: false });
    show();
    let stream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } });
    } catch (er) {
      rec.st = 'idle'; show(); try { ctx && ctx.close(); } catch { }
      toast(er && /NotAllowed|Permission|Security/i.test(er.name + er.message) ? 'Microphone access is blocked. Allow it in your browser or system settings to send voice messages.' : 'No microphone was found.');
      return;
    }
    if (rec.st !== 'starting') { stream.getTracks().forEach(t => t.stop()); try { ctx && ctx.close(); } catch { } return; }   // cancelled while waiting
    live = { stream, chunks: [], ctx, rate: ctx ? ctx.sampleRate : 48000 };
    try {
      if (!ctx) throw 0;
      live.an = ctx.createAnalyser(); live.an.fftSize = 1024; live.src = ctx.createMediaStreamSource(stream); live.src.connect(live.an); live.buf = new Float32Array(live.an.fftSize);
    } catch { live.an = null; }
    if (type) {
      const mr = live.mr = new MediaRecorder(stream, Object.assign(type ? { mimeType: type } : {}, { audioBitsPerSecond: 28000 }));
      mr.ondataavailable = e => { if (e.data && e.data.size) live && live.chunks.push(e.data); };
      live.stopped = new Promise(res => { mr.onstop = res; });
      mr.start(1000);
    } else {
      // raw sound: a processor that keeps what it hears and plays nothing
      const pr = live.pr = ctx.createScriptProcessor(4096, 1, 1);
      pr.onaudioprocess = e => { if (live && live.pr) live.chunks.push(new Float32Array(e.inputBuffer.getChannelData(0))); };
      live.src.connect(pr); pr.connect(ctx.destination); live.stopped = Promise.resolve();
    }
    rec.t0 = performance.now(); rec.st = 'recording'; rec.type = type;
    // A prompt took the press away from the finger, or the button was let go meanwhile: stay recording, with buttons.
    const slow = downAt && performance.now() - downAt > 600;
    if (rec.release === 'up' && rec.heldLong) return void finish(!rec.cancelling);
    if (slow || rec.release === 'up') rec.mode = 'lock';
    else if (rec.heldLong) rec.mode = 'hold';
    live.timer = setInterval(tickRec, SAMPLE_MS);
    show(); say('Recording voice message'); draw();
  }
  async function finish(send) {
    if (rec.st === 'idle') return;
    if (rec.st === 'starting') { rec.st = 'idle'; show(); return; }
    const r = Object.assign({}, rec), l = live; rec.st = 'idle'; rec.mode = ''; rec.cancelling = false; show();
    stopRecorder(); if (l) await l.stopped; releaseMic();
    const ms = Math.round(Math.min(MAX_MS, r.ms || (performance.now() - r.t0)));
    if (!send) return;
    if (ms < MIN_MS) return toast('That was too short. Hold the button and talk, then let go.');
    const blob = r.type ? new Blob(l.chunks, { type: (l.chunks[0] && l.chunks[0].type) || r.type || 'audio/webm' }) : wavOf(l.chunks, l.rate);
    if (!blob.size) return toast('Nothing was recorded.');
    const ext = /wav/.test(blob.type) ? 'wav' : /mp4|aac/.test(blob.type) ? 'm4a' : /ogg/.test(blob.type) ? 'ogg' : 'webm';
    const d = new Date(), p2 = n => String(n).padStart(2, '0');
    const name = `Voice message ${d.getFullYear()}-${p2(d.getMonth() + 1)}-${p2(d.getDate())} ${p2(d.getHours())}.${p2(d.getMinutes())}.${p2(d.getSeconds())}.${ext}`;
    const peaks = bytesToHex(toPeaks(r.samples));
    toast('Sending voice message…');
    try {
      const buf = new Uint8Array(await blob.arrayBuffer());
      await rpc('send_file', { gid: r.gid, name, mime: blob.type.split(';')[0] || 'audio/webm', data: b64u(buf), caption: '', voice: { ms, peaks } });
      S.lastMsgs = ''; S.pinned = true; tick(); soon();
    } catch (er) { toast(er.message); }
  }
  // 64 bytes: the loudest moment in each slice of the clip, scaled so the loudest slice is 255.
  function toPeaks(s) {
    const n = s.length, out = [];
    if (!n) return new Array(BARS).fill(0);
    for (let i = 0; i < BARS; i++) { const a = Math.floor(i * n / BARS), b = Math.max(a + 1, Math.floor((i + 1) * n / BARS)); let m = 0; for (let j = a; j < b && j < n; j++) m = Math.max(m, s[j]); out.push(m); }
    const top = Math.max(...out, 1e-4);
    return out.map(v => Math.round(Math.sqrt(v / top) * 255));
  }

  function wireMic() {
    const mic = $('#mic'); if (!mic) return;
    if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia || !(pickType() || canPcm())) { mic.hidden = true; return; }
    let start = null;
    mic.addEventListener('pointerdown', e => {
      if (e.button || rec.st !== 'idle') return;
      e.preventDefault(); start = { x: e.clientX, y: e.clientY, t: performance.now(), id: e.pointerId };
      try { mic.setPointerCapture(e.pointerId); } catch { }
      begin(start.t);
      start.timer = setTimeout(() => { if (start && rec.st !== 'idle') { rec.heldLong = true; if (rec.st === 'recording' && rec.mode === 'pending') { rec.mode = 'hold'; show(); } } }, HOLD_MS);
    });
    mic.addEventListener('pointermove', e => {
      if (!start || rec.st === 'idle') return;
      const far = Math.hypot(e.clientX - start.x, e.clientY - start.y) > CANCEL_PX;
      if (far !== rec.cancelling) { rec.cancelling = far; show(); }
    });
    const up = e => {
      if (!start) return; const s = start; start = null; clearTimeout(s.timer); rec.down = false;
      try { mic.releasePointerCapture(s.id); } catch { }
      if (rec.st === 'idle') return;
      const held = rec.heldLong;
      if (e.type === 'pointercancel' && rec.st === 'starting') { rec.release = 'up'; rec.heldLong = false; return; }   // the permission prompt took the press
      if (!held) { rec.mode = 'lock'; rec.release = 'up'; rec.heldLong = false; show(); return; }                      // a click: keep recording with buttons
      if (rec.st === 'starting') { rec.release = 'up'; rec.heldLong = true; return; }
      finish(e.type !== 'pointercancel' && !rec.cancelling);
    };
    mic.addEventListener('pointerup', up); mic.addEventListener('pointercancel', up);
    mic.addEventListener('contextmenu', e => e.preventDefault());
    // The keyboard and screen readers click without a pointer (detail 0): that starts a recording with buttons, or sends it.
    mic.addEventListener('click', e => { if (e.detail !== 0) return; if (rec.st === 'idle') begin(0).then(() => { if (rec.st !== 'idle') { rec.mode = 'lock'; rec.down = false; show(); } }); else if (rec.st !== 'starting') finish(true); });
    document.addEventListener('keydown', e => { if (e.key === 'Escape' && rec.st !== 'idle') { e.preventDefault(); finish(false); } });
    window.addEventListener('blur', () => { if (rec.st === 'recording' && rec.mode !== 'lock') { rec.mode = 'lock'; show(); } });
    window.addEventListener('pagehide', () => { if (rec.st !== 'idle') finish(false); });
  }

  window.MegaVoice = { row, audio: id => (players.get(id) || {}).audio || null, hydrate: () => document.querySelectorAll('.vmsg[data-vid]').forEach(paintView), state: rec, pickType, set forcePcm(v) { forcePcm = !!v; } };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', wireMic); else wireMic();
})();
