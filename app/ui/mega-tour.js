/* mega: the first-run tour. Five short steps, shown once per device after the cards first appear,
   and again from the options room ("show the tour again"). Self-contained: it watches the page, it hooks nothing in mega.js.
   A step points at the real element when that is on screen; otherwise (say, the dots while you are on the cards) the note stands alone. */
(() => {
  'use strict';
  const KEY = 'tour';
  const STEPS = [
    { sel: () => first('#mega .mslot .mcard'), text: 'these cards are your communities. scroll to move through them, click one to go in.' },
    { sel: () => first('#mega .mslot .mcard', 1), text: 'inside, rooms are cards too. a voice room joins with one click, and the call dock opens on the right.' },
    { sel: () => first('#mega-rail:not([hidden])'), text: 'in a chat, the dots on the left are your rooms. hover or scroll on them to switch.' },
    { sel: () => first('#mega [data-m="draw"]:not([hidden])'), text: 'draw lets everyone doodle on the room\'s backdrop. you find it in the bar at the top.' },
    { sel: () => first('#mega-rail .mitem.opt'), text: 'the options room has your settings, and a link to send friends who want to get mega.' },
  ];
  const still = () => matchMedia('(prefers-reduced-motion: reduce)').matches;
  const seen = () => !!pref.get(KEY);
  function first(q, n = 0) {
    const l = [...document.querySelectorAll(q)].filter(e => { const r = e.getBoundingClientRect(); return r.width > 4 && r.height > 4; });
    return l[n] || l[0] || null;
  }
  let T = null;   // the open tour

  function end(done) {
    if (!T) return;
    const t = T; T = null;
    removeEventListener('keydown', t.keys, true); removeEventListener('resize', t.place);
    t.root.classList.remove('on'); setTimeout(() => t.root.remove(), still() ? 0 : 220);
    if (done) pref.set(KEY, '1');
    try { if (t.back && t.back.isConnected) t.back.focus({ preventScroll: true }); } catch { }
  }
  function start() {
    if (T) return;
    const root = document.createElement('div'); root.id = 'mega-tour';
    root.innerHTML = '<div class="mt-ring" aria-hidden="true"></div><div class="mt-box" role="dialog" aria-label="quick tour"><p class="mt-n" aria-hidden="true"></p><p class="mt-t"></p><div class="mt-a"><button type="button" class="mchip mt-skip">skip</button><button type="button" class="mchip mt-next"></button></div></div><div class="mt-live" role="status" aria-live="polite"></div>';
    document.body.append(root);
    const ring = root.querySelector('.mt-ring'), box = root.querySelector('.mt-box'), live = root.querySelector('.mt-live');
    const nEl = root.querySelector('.mt-n'), tEl = root.querySelector('.mt-t'), next = root.querySelector('.mt-next'), skip = root.querySelector('.mt-skip');
    let i = 0;
    function place() {
      const el = STEPS[i].sel(), narrow = innerWidth <= 760;
      if (!el) { ring.style.opacity = '0'; box.style.cssText = ''; box.classList.add('mid'); return; }
      box.classList.remove('mid');
      const r = el.getBoundingClientRect(), pad = 6;
      ring.style.cssText = `opacity:1;left:${r.left - pad}px;top:${r.top - pad}px;width:${r.width + pad * 2}px;height:${r.height + pad * 2}px`;
      if (narrow) { box.style.cssText = `left:12px;right:12px;width:auto;${r.top > innerHeight / 2 ? 'top:calc(12px + env(safe-area-inset-top, 0px));bottom:auto' : 'bottom:calc(12px + env(safe-area-inset-bottom, 0px));top:auto'}`; return; }
      const w = Math.min(320, innerWidth - 24), bh = box.offsetHeight || 140;
      let left = r.right + 16, top = r.top;
      if (left + w > innerWidth - 12) { left = Math.min(Math.max(12, r.left), innerWidth - w - 12); top = r.bottom + 16; if (top + bh > innerHeight - 12) top = Math.max(12, r.top - bh - 16); }
      top = Math.min(Math.max(12, top), innerHeight - bh - 12);
      box.style.cssText = `left:${left}px;top:${top}px;width:${w}px`;
    }
    function show() {
      const last = i === STEPS.length - 1;
      nEl.textContent = `${i + 1} of ${STEPS.length}`; tEl.textContent = STEPS[i].text; next.textContent = last ? 'done' : 'next';
      skip.hidden = last; live.textContent = `step ${i + 1} of ${STEPS.length}: ${STEPS[i].text}`;
      place(); next.focus({ preventScroll: true });
    }
    const go = () => { if (i >= STEPS.length - 1) end(true); else { i++; show(); } };
    next.onclick = go; skip.onclick = () => end(true);
    const keys = e => {
      if (e.key === 'Escape') { e.preventDefault(); e.stopImmediatePropagation(); end(true); }
      else if (e.key === 'Enter' && !(e.target && e.target.closest && e.target.closest('.mt-skip'))) { e.preventDefault(); e.stopImmediatePropagation(); go(); }
    };
    addEventListener('keydown', keys, true); addEventListener('resize', place);
    T = { root, keys, place, back: document.activeElement };
    show(); requestAnimationFrame(() => root.classList.add('on'));
  }

  // the first time: once the cards are on screen and nothing else is in the way
  let calm = 0;
  setInterval(() => {
    // the options room gets its button
    const o = document.querySelector('#mega-opts:not([hidden]) .mopts-in');
    if (o && !o.querySelector('#m-tour-again') && o.querySelector('#ty')) {
      const s = document.createElement('section');
      s.innerHTML = '<h3>quick tour</h3><p>A short look at the cards, the dots and the call dock.</p><button type="button" class="mchip" id="m-tour-again">show the tour again</button>';
      s.querySelector('button').onclick = () => start();
      o.append(s);
    }
    if (T || seen()) { calm = 0; return; }
    const ok = document.documentElement.classList.contains('mega') && first('#mega:not([hidden]) .mcard') && !document.querySelector('#modal-root > .scrim, #mega-pal, .mpop, #mega-hint') && document.visibilityState === 'visible';
    calm = ok ? calm + 1 : 0;
    if (calm >= 2) { calm = 0; start(); }
  }, 700);
  window.__megaTour = { start, end, state: () => ({ open: !!T, seen: seen() }) };
})();
