'use strict';
// The Wave theme: a slow wave field drawn with four flat colours through an 8x8 ordered dither on 3 px cells. One fragment
// per cell, scaled up with nearest-neighbour. The dither is fixed to the screen; only the field moves. It stops when the
// window is hidden or unfocused and for reduced motion.
(function () {
  const PALETTES = [{"key":"blue","label":"Blue","dark":{"bg":"#171A21","wave0":"#12141A","wave1":"#1D2333","wave2":"#26375E","wave3":"#324B8A","accent":"#5782D9"},"light":{"bg":"#ECEEF8","wave0":"#E3E5F3","wave1":"#CBD0EB","wave2":"#A2ADDF","wave3":"#788BD3","accent":"#426BC9"}},{"key":"clay","label":"Clay","dark":{"bg":"#211A17","wave0":"#1A1412","wave1":"#33231D","wave2":"#5E3426","wave3":"#8A4B32","accent":"#D97657"},"light":{"bg":"#F8F1EC","wave0":"#F3EAE3","wave1":"#EBD8CB","wave2":"#DFB9A2","wave3":"#D39878","accent":"#C96542"}},{"key":"ember","label":"Ember","dark":{"bg":"#211817","wave0":"#1A1312","wave1":"#33201D","wave2":"#5E2B26","wave3":"#8A3C32","accent":"#D96057"},"light":{"bg":"#F8EFEC","wave0":"#F3E8E3","wave1":"#EBD3CB","wave2":"#DFAFA2","wave3":"#D38978","accent":"#C94F42"}},{"key":"amber","label":"Amber","dark":{"bg":"#211D17","wave0":"#1A1712","wave1":"#332A1D","wave2":"#5E4726","wave3":"#8A6832","accent":"#D9A057"},"light":{"bg":"#F8F5EC","wave0":"#F3EFE3","wave1":"#EBE2CB","wave2":"#DFCDA2","wave3":"#D3B678","accent":"#C99142"}},{"key":"gold","label":"Gold","dark":{"bg":"#212018","wave0":"#1A1913","wave1":"#33301F","wave2":"#5E562C","wave3":"#8A803B","accent":"#D9C464"},"light":{"bg":"#F8F8ED","wave0":"#F2F3E5","wave1":"#EBEBCE","wave2":"#DFDDA8","wave3":"#D3CE81","accent":"#C9B650"}},{"key":"moss","label":"Moss","dark":{"bg":"#1D211A","wave0":"#171A14","wave1":"#2A3323","wave2":"#495E34","wave3":"#678A48","accent":"#AAD978"},"light":{"bg":"#F2F8EF","wave0":"#EBF3E7","wave1":"#DBEBD3","wave2":"#C2DFB1","wave3":"#AAD38F","accent":"#96C964"}},{"key":"jade","label":"Jade","dark":{"bg":"#19211D","wave0":"#141A16","wave1":"#213329","wave2":"#315E44","wave3":"#448A63","accent":"#71D99B"},"light":{"bg":"#EEF8F4","wave0":"#E6F3EE","wave1":"#D1EBE0","wave2":"#AEDFC9","wave3":"#8AD3B0","accent":"#5DC98B"}},{"key":"teal","label":"Teal","dark":{"bg":"#182121","wave0":"#131A1A","wave1":"#203233","wave2":"#2E5D5E","wave3":"#3F878A","accent":"#6AD9D8"},"light":{"bg":"#EEF6F8","wave0":"#E5F0F3","wave1":"#D0E7EB","wave2":"#ABD8DF","wave3":"#86CBD3","accent":"#56C8C9"}},{"key":"indigo","label":"Indigo","dark":{"bg":"#191721","wave0":"#13121A","wave1":"#201D33","wave2":"#2D265E","wave3":"#3E328A","accent":"#6357D9"},"light":{"bg":"#EFECF8","wave0":"#E8E3F3","wave1":"#D3CBEB","wave2":"#B0A2DF","wave3":"#8B78D3","accent":"#5242C9"}},{"key":"violet","label":"Violet","dark":{"bg":"#1D1821","wave0":"#17131A","wave1":"#2A1F33","wave2":"#462C5E","wave3":"#663B8A","accent":"#9E64D9"},"light":{"bg":"#F5EDF8","wave0":"#EFE5F3","wave1":"#E1CEEB","wave2":"#CBA8DF","wave3":"#B381D3","accent":"#8E50C9"}},{"key":"rose","label":"Rose","dark":{"bg":"#21181C","wave0":"#1A1316","wave1":"#332029","wave2":"#5E2E45","wave3":"#8A3F61","accent":"#D96AA1"},"light":{"bg":"#F8EEF1","wave0":"#F3E5E9","wave1":"#EBD0D9","wave2":"#DFABBD","wave3":"#D386A3","accent":"#C9568C"}},{"key":"slate","label":"Slate","dark":{"bg":"#1E1E21","wave0":"#17181A","wave1":"#2B2D33","wave2":"#4A505E","wave3":"#6B748A","accent":"#ABBAD9"},"light":{"bg":"#F4F4F8","wave0":"#EDEEF3","wave1":"#E0E2EB","wave2":"#CACEDF","wave3":"#B3BAD3","accent":"#9AA8C9"}}];
  const CELL = 3, INTENSITY = 0.8, FPS = 30;
  const root = document.documentElement;
  const VS = 'attribute vec2 a; void main() { gl_Position = vec4(a, 0.0, 1.0); }';
  const FS = `precision highp float;
uniform float uH, uTime, uIntensity;
uniform vec3 uC0, uC1, uC2, uC3;
float field(vec2 p, float t) {
  vec2 q = p + 0.28 * vec2(sin(p.y * 2.7 + t * 0.19), cos(p.x * 2.1 - t * 0.15));
  q += 0.08 * vec2(sin(q.y * 6.3 - t * 0.31), cos(q.x * 5.7 + t * 0.27));
  float w = sin(q.x * 3.6 + q.y * 1.4 + t * 0.33)
          + 0.7 * sin(q.x * -2.0 + q.y * 3.4 - t * 0.25)
          + 0.45 * sin(length(q - vec2(0.8, 0.3)) * 6.5 - t * 0.45);
  float ridge = 1.0 - abs(sin(dot(q, vec2(1.3, -2.1)) * 2.4 + t * 0.22));
  float v = 0.5 + 0.5 * w / 2.15;
  v = v * 0.78 + ridge * ridge * 0.30;
  return clamp(smoothstep(0.12, 0.92, v), 0.0, 1.0);
}
float bayer2(vec2 a) { a = floor(a); return fract(a.x * 0.5 + a.y * a.y * 0.75); }
float bayer4(vec2 a) { return bayer2(a * 0.5) * 0.25 + bayer2(a); }
float bayer8(vec2 a) { return bayer4(a * 0.5) * 0.25 + bayer2(a); }
void main() {
  vec2 cell = vec2(floor(gl_FragCoord.x), uH - 1.0 - floor(gl_FragCoord.y));
  vec2 p = (cell + 0.5) / uH;
  float v = mix(0.5, field(p, uTime), uIntensity);
  float tone = clamp(floor(v * 3.0 + (bayer8(cell) - 0.5) * 0.95), 0.0, 3.0);
  gl_FragColor = vec4(tone < 0.5 ? uC0 : (tone < 1.5 ? uC1 : (tone < 2.5 ? uC2 : uC3)), 1.0);
}`;
  const rgb = h => [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16) / 255);
  const mix = (a, b, k) => '#' + rgb(a).map((x, i) => Math.round((x + (rgb(b)[i] - x) * k) * 255).toString(16).padStart(2, '0')).join('');
  const lum = h => { const [r, g, b] = rgb(h).map(c => c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4)); return 0.2126 * r + 0.7152 * g + 0.0722 * b; };
  const inkOn = h => ((lum(h) + 0.05) / 0.0549 >= 1.05 / (lum(h) + 0.05) ? '#10131a' : '#ffffff');
  const sysDark = matchMedia('(prefers-color-scheme: dark)'), calm = matchMedia('(prefers-reduced-motion: reduce)');
  const saved = () => { try { return localStorage.getItem('commons.wavePalette') || 'blue'; } catch { return 'blue'; } };
  const state = { force: null, time: 0, last: 0, raf: 0, on: false };
  let canvas = null, gl = null, U = null, painted = [];

  const active = () => root.dataset.theme === 'wave';
  const palette = () => PALETTES.find(p => p.key === ((state.force && state.force.palette) || saved())) || PALETTES[0];
  const mode = () => (state.force && state.force.mode) || (sysDark.matches ? 'dark' : 'light');

  function setup() {
    if (canvas) return !!gl;
    canvas = document.createElement('canvas'); canvas.id = 'wave-dither'; canvas.setAttribute('aria-hidden', 'true');
    document.body.prepend(canvas);
    gl = canvas.getContext('webgl', { antialias: false, alpha: false, depth: false, stencil: false, preserveDrawingBuffer: false });
    if (!gl) return false;
    const sh = (type, src) => { const s = gl.createShader(type); gl.shaderSource(s, src); gl.compileShader(s); return s; };
    const pr = gl.createProgram(); gl.attachShader(pr, sh(gl.VERTEX_SHADER, VS)); gl.attachShader(pr, sh(gl.FRAGMENT_SHADER, FS)); gl.linkProgram(pr); gl.useProgram(pr);
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    const a = gl.getAttribLocation(pr, 'a'); gl.enableVertexAttribArray(a); gl.vertexAttribPointer(a, 2, gl.FLOAT, false, 0, 0);
    U = n => gl.getUniformLocation(pr, n);
    return true;
  }
  function draw() {
    if (!gl) return;
    const w = Math.ceil(innerWidth / CELL), h = Math.ceil(innerHeight / CELL), c = palette()[mode()];
    if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; canvas.style.width = w * CELL + 'px'; canvas.style.height = h * CELL + 'px'; }
    gl.viewport(0, 0, w, h);
    gl.uniform1f(U('uH'), h); gl.uniform1f(U('uTime'), state.time); gl.uniform1f(U('uIntensity'), INTENSITY);
    ['uC0', 'uC1', 'uC2', 'uC3'].forEach((n, i) => gl.uniform3fv(U(n), rgb(c['wave' + i])));
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }
  function colours() {
    painted.forEach(k => root.style.removeProperty(k)); painted = [];
    if (!active()) { delete root.dataset.waveMode; return; }
    const m = mode(), c = palette()[m], w2 = c.wave2;
    root.dataset.waveMode = m;
    const set = {
      '--wall': c.bg, '--wall-2': c.wave0, '--page': c.bg, '--page-sunk': c.wave0, '--rail': c.bg, '--rail-floor': c.bg,
      '--card': mix(c.bg, w2, 0.14), '--raise': mix(c.bg, w2, 0.24), '--rail-2': mix(c.bg, w2, 0.2), '--rail-3': c.wave0,
      '--line': mix(c.bg, w2, 0.45), '--line-2': mix(c.bg, w2, 0.8), '--rail-line': mix(c.bg, w2, 0.45), '--glass-line': mix(c.bg, w2, 0.45),
      '--glass': c.bg + 'e0', '--glass-solid': c.bg,
      '--act': c.accent, '--act-ink': inkOn(c.accent), '--act-line': c.accent, '--act-soft': mix(c.bg, c.accent, 0.22),
      '--focus': c.accent, '--brass-line': c.accent, '--hover': c.accent + '14',
    };
    Object.entries(set).forEach(([k, v]) => { root.style.setProperty(k, v); painted.push(k); });
  }
  function stop() { cancelAnimationFrame(state.raf); state.raf = 0; state.on = false; }
  function frame(now) {
    state.raf = requestAnimationFrame(frame);
    if (now - state.last < 1000 / FPS - 1) return;
    if (state.last) state.time += (now - state.last) / 1000;
    state.last = now; draw();
  }
  function sync() {
    stop(); colours();
    if (!active() || !setup()) return;
    state.last = 0; draw();
    if (state.force && state.force.time != null) return;
    if (document.hidden || !document.hasFocus() || calm.matches || root.classList.contains('fx-lite')) return;
    state.on = true; state.raf = requestAnimationFrame(frame);
  }
  window.__waveDither = {
    palettes: PALETTES.map(p => [p.key, p.label]),
    refresh: sync,
    force(f) { state.force = f; if (f && f.time != null) state.time = f.time; sync(); },   // the tests freeze a frame here
  };
  const init = () => {
    new MutationObserver(sync).observe(root, { attributes: true, attributeFilter: ['data-theme'] });
    [sysDark, calm].forEach(m => m.addEventListener('change', sync));
    ['focus', 'blur', 'resize'].forEach(e => addEventListener(e, sync));
    document.addEventListener('visibilitychange', sync);
    sync();
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init); else init();
})();
