/* Dithered waves, after rune's "Dither" ambient: broad soft swells roll slowly down the sidebar and are
   ordered-dithered with a blue-noise threshold map into two tones of dark dots (2 CSS px per dot), over the rail art.
   The same wave field continues across the wall as a barely-there shimmer of warm amber dots on the swell crests,
   kept out of the header and the message column so the small grey text keeps WCAG AA. One clock and one coordinate
   system (window x and y, in units of the rail's width), so the swells run on across the seam in lockstep.
   Kept cheap: the wave terms are worked out once per row and once per column, and the per-dot work is table
   lookups (sine, tone curve), so a frame costs a few milliseconds. About 15 fps, paused while the window is hidden
   or in the background, one still frame under reduced motion.
   Both themes: dark dots over After hours, light sepia ink over Daylight; it restarts when the theme changes. */
(function () {
  const root = document.documentElement;
  // Linux's WebKitGTK draws blur, blends and constantly changing canvases on the CPU (badly so on NVIDIA and at 4K):
  // there only the small rail animates (the wall's big canvas is drawn once per resize) and fresco.css drops the
  // live blur and blend
  const LITE = /Linux/.test(navigator.userAgent) && !/Android|Chrome|Firefox/.test(navigator.userAgent);
  if (LITE) root.classList.add('fx-lite');
  const sysDark = matchMedia('(prefers-color-scheme: dark)');
  const isDark = () => root.dataset.theme === 'dark' || (root.dataset.theme !== 'light' && sysDark.matches);
  const DOT = 2;                  // CSS px per dot
  const FPS = 15;
  // rail tones: [r, g, b, alpha 0..255]. After hours dots are darker than the wall; Daylight dots are light sepia ink.
  const DARK_PAL = {
    mid: [22, 8, 3, 120],       // swell bodies
    light: [12, 4, 1, 185],     // swell cores, darkest
    glow: [214, 140, 92, 60],   // the leading edge of each crest
    amber: [228, 170, 90, 30],  // the wall: sparse, faint warm amber dots on the swells only
  };
  const LIGHT_PAL = {
    mid: [168, 140, 100, 92],
    light: [130, 104, 74, 130],
    glow: [208, 166, 110, 52],
    amber: [150, 120, 80, 28],
  };
  const MASK = { light: 0.30, mid: 0.70 };
  const TONE = { lo: 0.38, hi: 0.9 };
  const BASE = 0.24;              // share of rail dots dithered even between waves
  const GLOW_MAX = 0.32;
  const WALL_MAX = 0.20;          // at most this share of dots lit on a crest
  // waves: wavelength in rail widths and downward speed for the two swell layers
  const WK1 = 2 * Math.PI / 0.95, WS1 = WK1 * 0.06, WK2 = 2 * Math.PI / 1.6, WS2 = WK2 * 0.045;
  const pack = c => ((c[3] << 24) | (c[2] << 16) | (c[1] << 8) | c[0]) >>> 0;
  const ss = (a, b, x) => { const t = Math.min(1, Math.max(0, (x - a) / (b - a))); return t * t * (3 - 2 * t); };

  // the wave field at (x, y) in rail widths from the window's top-left, time t in seconds
  function field(px, py, t) {
    const ph1 = py * WK1 - t * WS1 + 0.9 * Math.sin(px * 4.1 + t * 0.21) + 0.45 * Math.sin(px * 9.3 - t * 0.13 + 1.7);
    const ph2 = py * WK2 - t * WS2 + 1.1 * Math.sin(px * 3.2 - t * 0.17 + 2.4);
    const s1 = 0.5 + 0.5 * Math.sin(ph1), s2 = 0.5 + 0.5 * Math.sin(ph2);
    return { f: 1.15 * ss(0.32, 0.95, 0.62 * s1 + 0.38 * s2), edge: Math.max(0, Math.cos(ph1)) * (0.5 + 0.5 * s2) };
  }

  // tables: sine over one turn, and the tone curves over the swell mix u = 0.62 s1 + 0.38 s2 in 0..1
  const SN = 4096, SIN = new Float32Array(SN + 1), TWO_PI = 2 * Math.PI;
  for (let k = 0; k <= SN; k++) SIN[k] = Math.sin(k / SN * TWO_PI);
  const sinT = a => { let x = a / TWO_PI; x -= Math.floor(x); return SIN[(x * SN) | 0]; };
  const UN = 1024, DL = new Float32Array(UN + 1), DM = new Float32Array(UN + 1), WL = new Float32Array(UN + 1);
  for (let k = 0; k <= UN; k++) {
    const f = 1.15 * ss(0.32, 0.95, k / UN), tau = ss(TONE.lo, TONE.hi, f);
    DL[k] = Math.pow(tau, 1.6) * MASK.light; DM[k] = Math.min(1 - DL[k], BASE + (1 - BASE) * Math.pow(tau, 0.6) * MASK.mid);
    WL[k] = ss(0.55, 1.05, f) * WALL_MAX;
  }
  const GL_ = new Float32Array(UN + 1); for (let k = 0; k <= UN; k++) GL_[k] = ss(0.55, 0.98, k / UN) * GLOW_MAX;
  // per-column and per-row wave terms for one canvas
  function terms(x0, y0, W, H, U, t, C1, C2, R1, R2) {
    for (let i = 0; i < W; i++) {
      const px = (x0 + (i + 0.5) * DOT) / U;
      C1[i] = 0.9 * Math.sin(px * 4.1 + t * 0.21) + 0.45 * Math.sin(px * 9.3 - t * 0.13 + 1.7);
      C2[i] = 1.1 * Math.sin(px * 3.2 - t * 0.17 + 2.4);
    }
    for (let j = 0; j < H; j++) { const py = (y0 + (j + 0.5) * DOT) / U; R1[j] = py * WK1 - t * WS1; R2[j] = py * WK2 - t * WS2; }
  }
  let stop = null;
  function start() {
    const rail = document.getElementById('rail'), main = document.getElementById('main'); if (!rail || !main) return;
    const mk = (cls, parent) => { const c = document.createElement('canvas'); c.className = cls; c.setAttribute('aria-hidden', 'true'); parent.prepend(c); return c; };
    const rc = mk('rail-dither', rail), wc = mk('wall-dither', main);
    const rx = rc.getContext('2d'), wx = wc.getContext('2d');
    const P = isDark() ? DARK_PAL : LIGHT_PAL;
    const PX_MID = pack(P.mid), PX_LIGHT = pack(P.light), PX_GLOW = pack(P.glow), PX_AMBER = pack(P.amber);
    let alive = true, noise = null;
    const R = { W: 0, H: 0, img: null, x0: 0 }, M = { W: 0, H: 0, img: null, x0: 0, y0: 0, c0: 0, c1: 0, top: 0 };
    let U = 268;                    // the rail's width in CSS px: the unit of the shared field
    const nimg = new Image();
    nimg.onload = () => {
      if (!alive) return;            // stopped (the theme changed) before the noise map arrived
      const c = document.createElement('canvas'); c.width = c.height = 64; const x = c.getContext('2d');
      x.drawImage(nimg, 0, 0); const d = x.getImageData(0, 0, 64, 64).data;
      noise = new Float32Array(64 * 64); for (let i = 0; i < noise.length; i++) noise[i] = (d[i * 4] + 0.5) / 256;
      size(); frame(performance.now()); if (!still) loop();
    };
    nimg.src = 'fresco/bluenoise64.png';

    function size() {
      const r = rail.getBoundingClientRect(), m = main.getBoundingClientRect();
      U = r.width > 40 ? r.width : 268;
      R.W = Math.max(1, Math.ceil(r.width / DOT)); R.H = Math.max(1, Math.ceil(r.height / DOT)); R.x0 = r.left; R.y0 = r.top;
      rc.width = R.W; rc.height = R.H; R.img = rx.createImageData(R.W, R.H); R.px = new Uint32Array(R.img.data.buffer);
      R.C1 = new Float32Array(R.W); R.C2 = new Float32Array(R.W); R.R1 = new Float32Array(R.H); R.R2 = new Float32Array(R.H);
      M.W = Math.max(1, Math.ceil(m.width / DOT)); M.H = Math.max(1, Math.ceil(m.height / DOT)); M.x0 = m.left; M.y0 = m.top;
      wc.width = M.W; wc.height = M.H; M.img = wx.createImageData(M.W, M.H); M.px = new Uint32Array(M.img.data.buffer);
      M.C1 = new Float32Array(M.W); M.C2 = new Float32Array(M.W); M.R1 = new Float32Array(M.H); M.R2 = new Float32Array(M.H);
      // keep the wall's dots out of the message column and the header (soft 40 px edges)
      // the column is the stream's content box narrowed to --col and centred (as style.css lays out #stream > *)
      const stream = document.getElementById('stream');
      let c0 = 0, c1 = m.width;
      if (stream) {
        const probe = document.createElement('div');
        probe.style.cssText = 'position:absolute;visibility:hidden;height:0;width:var(--col)';
        stream.appendChild(probe); const colW = probe.offsetWidth; probe.remove();
        const sr = stream.getBoundingClientRect(), cs = getComputedStyle(stream);
        const l = sr.left + parseFloat(cs.paddingLeft), r = sr.right - parseFloat(cs.paddingRight);
        const w = Math.min(colW || (r - l), r - l);
        c0 = (l + r) / 2 - w / 2 - m.left; c1 = (l + r) / 2 + w / 2 - m.left;
      }
      const head = document.getElementById('head');
      M.c0 = c0; M.c1 = c1; M.top = head ? head.getBoundingClientRect().bottom - m.top : 64;
    }
    function frame(ms) {
      if (!noise || !R.img || !M.img) return;
      const t = ms / 1000;
      // the rail
      terms(R.x0, R.y0, R.W, R.H, U, t, R.C1, R.C2, R.R1, R.R2);
      const rp = R.px;
      for (let j = 0; j < R.H; j++) {
        const r1 = R.R1[j], r2 = R.R2[j], row = j * R.W, nrow = (j & 63) * 64;
        for (let i = 0; i < R.W; i++) {
          const ph1 = r1 + R.C1[i], s1 = 0.5 + 0.5 * sinT(ph1), s2 = 0.5 + 0.5 * sinT(r2 + R.C2[i]);
          const u = ((0.62 * s1 + 0.38 * s2) * UN) | 0;
          const th = noise[nrow + (i & 63)], dL = DL[u];
          let c = 0;
          if (th < dL) c = PX_LIGHT;
          else if (th < dL + DM[u]) c = PX_MID;
          else {
            const e = Math.max(0, sinT(ph1 + Math.PI / 2)) * (0.5 + 0.5 * s2);   // the crest's leading edge, as field()
            if (1 - th < GL_[(e * UN) | 0]) c = PX_GLOW;
          }
          rp[row + i] = c;
        }
      }
      rx.putImageData(R.img, 0, 0);
      if (LITE && wallDone) return;
      wallDone = true;
      // the wall: only crests, only outside the column and below the header; most dots are skipped early
      terms(M.x0, M.y0, M.W, M.H, U, t, M.C1, M.C2, M.R1, M.R2);
      const mp = M.px; mp.fill(0);
      for (let j = 0; j < M.H; j++) {
        const y = (j + 0.5) * DOT; const vy = ss(M.top, M.top + 40, y); if (vy <= 0) continue;
        const r1 = M.R1[j], r2 = M.R2[j], row = j * M.W, nrow = (j & 63) * 64;
        for (let i = 0; i < M.W; i++) {
          const th = noise[nrow + (i & 63)];
          if (th > WALL_MAX) continue;                          // can never light: skip the field
          const x = (i + 0.5) * DOT;
          const vx = Math.max(ss(M.c0, M.c0 - 40, x), ss(M.c1, M.c1 + 40, x)); if (vx <= 0) continue;
          const s1 = 0.5 + 0.5 * sinT(r1 + M.C1[i]), s2 = 0.5 + 0.5 * sinT(r2 + M.C2[i]);
          if (th < WL[((0.62 * s1 + 0.38 * s2) * UN) | 0] * vx * vy) mp[row + i] = PX_AMBER;
        }
      }
      wx.putImageData(M.img, 0, 0);
    }
    const still = matchMedia('(prefers-reduced-motion: reduce)').matches;
    let wallDone = false;           // LITE: the wall is drawn once until the next resize
    let last = 0, raf = 0;
    function loop() { if (!alive) return; raf = requestAnimationFrame(ms => { if (ms - last >= 1000 / FPS) { last = ms; frame(ms); } loop(); }); }
    // paused while the window is hidden or another app has focus: nobody is watching it then
    const onVis = () => { cancelAnimationFrame(raf); if (!document.hidden && document.hasFocus() && !still && noise) loop(); };
    document.addEventListener('visibilitychange', onVis); window.addEventListener('focus', onVis); window.addEventListener('blur', onVis);
    const ro = new ResizeObserver(() => { size(); wallDone = false; frame(performance.now()); }); ro.observe(rail); ro.observe(main);
    stop = () => { alive = false; cancelAnimationFrame(raf); document.removeEventListener('visibilitychange', onVis); window.removeEventListener('focus', onVis); window.removeEventListener('blur', onVis); ro.disconnect(); rc.remove(); wc.remove(); stop = null; };
  }
  function sync() { if (stop) stop(); start(); }
  function init() {
    sync();
    sysDark.addEventListener('change', sync);
    new MutationObserver(sync).observe(root, { attributes: true, attributeFilter: ['data-theme'] });
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init); else init();
})();
