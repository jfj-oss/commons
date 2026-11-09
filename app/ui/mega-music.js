// mega: the chat backdrop moves to music.
//
// 1. Music playing in the call you are in (someone plays rune or a video into it): the published audio track is read by a Web Audio
//    analyser, and its bass and beat onsets feed the backdrop's heat and gentle ripples. The analyser is a dead end: the track goes
//    into the analyser and nowhere else. It is never connected to any output, so what anyone hears, how loud and how processed, is
//    exactly as before. The audio is already decrypted on this device and nothing about it leaves it. Brave randomises analyser values a
//    little, so only smoothed relative energy is used, quantised coarsely, never absolute thresholds.
// 2. Someone in the room shares "listening to" and there is no call audio: the app knows only a title, so the backdrop breathes slowly
//    and a small quiet note says what is playing. Never synced to anything.
//
// The owner's strength still rules (0: nothing at all), reduced motion and html.fx-lite mean no pulsing, a hidden tab does nothing, and
// when the music stops, the call ends or the room closes the analyser is disconnected and its AudioContext closed.
(() => {
  'use strict';
  const FFT = 1024, TICK_MS = 72;                     // about 14 analyses a second, under the 15 allowed
  const BASS = [1, 5], BODY = [5, 64];                // bins of 47 Hz at 48 kHz: roughly 50-235 Hz and 235-3000 Hz
  const q32 = x => Math.round(x * 32) / 32;           // coarse on purpose
  const clamp = (x, a, b) => Math.max(a, Math.min(b, x));
  const ST = { ctxOpen: 0, ctxMade: 0, ctxClosed: 0, analyses: 0, onsets: 0, breaths: 0, mode: '' };
  const api = () => window.__mega && window.__mega.bdApi;

  let mode = '', cur = null, watch = 0, timer = 0, sub = null, noteEl = null, srcOverride = null, lastKey = '';

  const mean = (a, [lo, hi]) => { let s = 0; for (let i = lo; i < hi; i++) s += a[i]; return s / ((hi - lo) * 255); };

  function style() {
    if (document.getElementById('mega-music-css')) return;
    const s = document.createElement('style'); s.id = 'mega-music-css';
    s.textContent = '#mega-note{position:absolute;right:14px;top:58px;max-width:min(42%,320px);z-index:1;pointer-events:none;font-size:11px;line-height:1.3;letter-spacing:.01em;color:var(--mg-hi,#aaa);opacity:.62;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;text-align:right}'
      + '@media(max-width:560px){#mega-note{right:10px;top:50px;max-width:56%;font-size:10.5px}}'
      + 'html.fx-lite #mega-note{opacity:.55}';
    document.head.appendChild(s);
  }

  // ---- what is playing
  function callSources() {
    if (srcOverride) return srcOverride();
    const c = window.__commonsCall, bd = api() && api().bd();
    if (!c || !bd || c.gid !== bd.gid || c.phase !== 'connected' || typeof c.musicSources !== 'function') return [];
    try { return c.musicSources(); } catch { return []; }
  }
  function listening() {
    const m = typeof S !== 'undefined' && S.members, bd = api() && api().bd(); if (!m || !m.length || !bd) return null;
    const mine = m.filter(x => x.listening && x.listening.title);
    const x = mine.find(y => !y.me) || mine[0]; return x ? { who: x.me ? '' : x.name, ...x.listening } : null;
  }

  // ---- the analyser (case 1)
  function stopAnalyser() {
    clearInterval(timer); timer = 0;
    const c = cur; cur = null; if (!c) return;
    for (const n of c.nodes) { try { n.disconnect(); } catch { } }
    try { const p = c.ctx.close(); if (p && p.catch) p.catch(() => { }); } catch { }
    ST.ctxOpen--; ST.ctxClosed++;
  }
  function startAnalyser(srcs, key) {
    const AC = window.AudioContext || window.webkitAudioContext; if (!AC) return false;
    const nodes = []; let ctx;
    try {
      ctx = new AC({ latencyHint: 'playback' }); ST.ctxMade++; ST.ctxOpen++;
      const an = ctx.createAnalyser(); an.fftSize = FFT; an.smoothingTimeConstant = .6; nodes.push(an);
      for (const s of srcs) { const n = ctx.createMediaStreamSource(new MediaStream([s.track])); n.connect(an); nodes.push(n); }   // into the analyser only
      try { ctx.resume && ctx.resume(); } catch { }
      cur = { ctx, an, nodes, key, buf: new Uint8Array(an.frequencyBinCount), fast: 0, slow: 0, lastOn: 0, lastRip: 0, n: 0 };
    } catch { cur = { ctx, nodes }; stopAnalyser(); return false; }
    timer = setInterval(analyse, TICK_MS);
    return true;
  }
  function analyse() {
    const c = cur, A = api(), bd = A && A.bd(); if (!c) return;
    if (!bd || document.hidden || bd.dim === 0 || A.still()) { sync(); return; }
    c.an.getByteFrequencyData(c.buf); ST.analyses++;
    const bass = q32(mean(c.buf, BASS)), body = q32(mean(c.buf, BODY));
    const e = bass * .75 + body * .25;
    c.fast += (e - c.fast) * .5; c.slow += (e - c.slow) * .05;
    const now = performance.now(), present = clamp((c.slow - .02) * 4, 0, 1);       // a quiet or paused track stirs nothing
    if (present <= 0) return;
    const rel = clamp((c.fast - c.slow) / (c.slow + .08), 0, 1.5);
    const target = .16 + .3 * present + .14 * Math.min(1, rel);
    if (bd.heat < target) A.heat(.01);
    if (c.fast > c.slow * 1.3 + .03 && c.fast > .1 && now - c.lastOn > 280) {
      c.lastOn = now; ST.onsets++;
      if (now - c.lastRip > 500) {
        c.lastRip = now; c.n++;
        const b = bd.c.getBoundingClientRect(), u = (c.n * 0.6180339) % 1, v = (c.n * 0.7548777) % 1;
        A.ripple(b.left + b.width * (.15 + .7 * u), b.top + b.height * (.25 + .5 * v), .2 + .22 * Math.min(1, rel), true);
      }
    }
  }

  // ---- the slow breath (case 2)
  function breathe() {
    const A = api(), bd = A && A.bd(); if (!bd || document.hidden || bd.dim === 0 || A.still()) { sync(); return; }
    const t = performance.now(); ST.breaths++;
    const v = .5 + .5 * Math.sin(t / 4800 * Math.PI * 2);
    if (bd.heat < .18 + .12 * v) A.heat(.005);
    if (!bd.breathAt || t - bd.breathAt > 11000) {
      bd.breathAt = t; const b = bd.c.getBoundingClientRect();
      A.ripple(b.left + b.width * .5, b.top + b.height * .55, .16, true);
    }
  }

  // ---- the note
  function note(l) {
    const bd = api() && api().bd(), host = document.getElementById('main');
    if (!l || !bd || !host || bd.dim === 0) { if (noteEl) { noteEl.remove(); noteEl = null; if (window.__mega) __mega.relook(); } return; }
    style();
    if (!noteEl) { noteEl = document.createElement('div'); noteEl.id = 'mega-note'; noteEl.setAttribute('aria-hidden', 'true'); host.appendChild(noteEl); }
    const t = '♪ ' + l.title + (l.artist ? ' · ' + l.artist : '');
    if (noteEl.textContent !== t) { noteEl.textContent = t; noteEl.title = (l.who ? l.who + ' is listening to ' : 'listening to ') + l.title; if (window.__mega) __mega.relook(); }
  }

  function setMode(m) { mode = m; ST.mode = m; }

  /** Decide what the backdrop is doing right now, and start or stop to match. Cheap; safe to call often. */
  function sync() {
    const A = api(), bd = A && A.bd();
    if (!bd || document.hidden) { stopAll(); return; }
    if (!watch) watch = setInterval(sync, 2000);
    if (!sub && window.__commonsCall) { sub = () => sync(); window.__commonsCall.addEventListener('change', sub); }
    const live = bd.dim !== 0 && !A.still();
    const srcs = live ? callSources() : [], key = srcs.map(s => s.key + ':' + s.track.id).join('|');
    const l = bd.dim === 0 ? null : listening();
    if (srcs.length) {
      clearInterval(breathTimer); breathTimer = 0; note(null);
      if (mode !== 'music' || !cur || cur.key !== key) { stopAnalyser(); setMode(startAnalyser(srcs, key) ? 'music' : ''); }
      return;
    }
    stopAnalyser();
    note(l);
    if (l && live) { if (!breathTimer) breathTimer = setInterval(breathe, 450); setMode('breath'); }
    else { clearInterval(breathTimer); breathTimer = 0; setMode(''); }
  }
  let breathTimer = 0;
  function stopAll() {
    stopAnalyser(); clearInterval(breathTimer); breathTimer = 0; clearInterval(watch); watch = 0; setMode('');
    if (noteEl) { noteEl.remove(); noteEl = null; }
    if (sub && window.__commonsCall) { try { window.__commonsCall.removeEventListener('change', sub); } catch { } }
    sub = null;
  }

  document.addEventListener('visibilitychange', sync);
  window.addEventListener('pagehide', stopAll);
  window.__megaMusic = {
    sync, stats: ST,
    state: () => ({ ...ST, hasCtx: !!cur, note: noteEl ? noteEl.textContent : '' }),
    /** for tests only: replace where the music tracks come from ([{ key, track }]) */
    _sources: f => { srcOverride = f; sync(); },
  };
})();
