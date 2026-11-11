// End-to-end call tests: real relay (with call tokens), real LiveKit server, real Megatron clients (Rust core),
// real call engine in Chrome for Testing with FAKE media devices only. Everything on 127.0.0.1.
//   systemd-run --user --scope -q -p MemoryMax=8G node tools/call-e2e.mjs [outdir]
// CALL_E2E_ONLY=video runs only section J (video tiles in the call panel, about a minute).
import fs from 'node:fs';
import path from 'node:path';
import { startServers, startBridge, startChrome, openPage, rpc, sleep, stopAll, lkAdmin, operatorToken, FAKE_DISPLAY, RELAY_PORT, LK_PORT, port } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';
const UMD = fs.readFileSync(path.join(new URL('..', import.meta.url).pathname, 'app/ui/vendor/livekit-client.umd.js'), 'utf8');

const out = process.argv[2] || '/tmp/commons-call-e2e';
fs.rmSync(out, { recursive: true, force: true });
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? '  [' + detail + ']' : ''}`); };
const J = x => JSON.stringify(x);
async function until(fn, ms = 20000, what = 'condition') { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(250); } }

const TONE_MIN = 0.15, TONE_MAX = 0.02;    // share of spectral energy at the tone's frequency: heard vs not heard

let world, lk;
try {
  lk = await startServers(out, { relayTtl: 900 });
  const names = ['alice', 'bob', 'carol', 'dave'];
  world = await makeWorld(out, names);
  await openApps(world, names);
  const P = n => world.people[n];
  const page = n => P(n).page;
  const call = (n, expr) => page(n).ev(`(async()=>{ const c = window.__commonsCall; return ${expr}; })()`);
  const dev = n => P(n).device;
  const hear = async (listener, speaker, ms = 1500, source = 'microphone') => {
    await until(() => call(listener, `[...c.tracks.values()].some(x => x.identity === ${J(dev(speaker))} && x.source === ${J(source)})`), 20000, `${listener} has ${speaker}'s ${source}`).catch(() => { });
    await sleep(800);
    return call(listener, `c.probeTone(${J(dev(speaker))}, ${P(speaker).tone}, ${ms}, ${J(source)})`);
  };
  const hears = async (l, s, src) => { for (let i = 0; i < 6; i++) { const r = await hear(l, s, 1500, src); if (r.present && r.toneShare > TONE_MIN) return true; await sleep(1000); } return false; };
  const notHeard = async (l, s) => { const r = await hear(l, s, 1500); return { present: r.present, toneShare: r.toneShare, ok: !!r.present && r.toneShare < TONE_MAX }; };
  const room = await rpc(P('alice').port, 'call_token', { gid: world.gid });
  const roomName = room.room;

  // ---------------------------------------------------------------- J. video tiles in the real call (runs first, on a fresh call)
  // Bugs from real use: (1) with camera or screen share on, the videos flickered: every state update rebuilt the call UI and moved
  // the <video> elements into new tiles; (2) a camera switched off left a black box (the SDK mutes a camera, it does not unpublish it).
  // The stage is watched from inside the page: DOM removals/additions of tiles and videos, media events, and a 50 ms sampler that
  // counts moments where a shown video is not playing.
  const WATCH = `(() => {
    const panel = document.querySelector('#call-stage'); const st = window.__vt = { added: 0, removed: 0, events: {}, samples: 0, blank: 0, n: 0 };
    const count = (nodes, k) => { for (const n of nodes) if (n.nodeType === 1) st[k] += (n.matches('.ctile, video') ? 1 : 0) + n.querySelectorAll('.ctile, video').length; };
    new MutationObserver(ms => { for (const m of ms) { count(m.addedNodes, 'added'); count(m.removedNodes, 'removed'); } }).observe(panel, { childList: true, subtree: true });
    for (const ev of ['emptied', 'loadstart', 'pause', 'abort']) panel.addEventListener(ev, e => { if (e.target.tagName === 'VIDEO') st.events[ev] = (st.events[ev] || 0) + 1; }, true);
    setInterval(() => { for (const v of panel.querySelectorAll('.ctile video')) { st.samples++; if (!(v.isConnected && v.readyState >= 2 && !v.paused && v.videoWidth > 0)) st.blank++; } }, 50);
    window.__vtReset = () => { st.added = st.removed = st.samples = st.blank = 0; st.events = {}; };
    return true; })()`;
  const tilesOf = n => page(n).ev(`[...document.querySelectorAll('#call-stage .ctile')].map(d => { const v = d.querySelector('video'); if (v && !v.__vid) v.__vid = ++window.__vt.n; return { key: d.dataset.key, label: (d.querySelector('.bar .grow') || d).innerText, id: v ? v.__vid : 0, w: v ? v.videoWidth : 0, playing: !!v && !v.paused && v.readyState >= 2 }; })`);
  const isCam = t => /camera/.test(t.label), isScreen = t => /screen/.test(t.label);
  {
    for (const n of ['alice', 'bob']) { await page(n).ev(FAKE_DISPLAY); await page(n).ev(`window.__shareName = ${J(n)}`); }
    await call('alice', `c.join(S.gid, 'general')`); await call('bob', `c.join(S.gid, 'general')`);
    for (const n of ['alice', 'bob']) { await page(n).ev(`(window.__callUI.open(), true)`); await page(n).ev(WATCH); }
    await until(async () => (await call('bob', `c.room.remoteParticipants.size`)) === 1, 15000, 'J: bob sees alice');
    await call('alice', `c.setCamera(true)`); await call('alice', `c.startShare({ audio: false, profile: 'motion' })`);
    const ready = async () => { const b = await tilesOf('bob'), a = await tilesOf('alice'); return b.length === 2 && b.every(t => t.w > 0 && t.playing) && a.length === 2 && a.every(t => t.w > 0) ? { a, b } : null; };
    const t0 = await until(ready, 30000, 'J: both tiles show on both sides').catch(async () => ({ a: await tilesOf('alice'), b: await tilesOf('bob'), bad: true }));
    check('J1 camera and screen share each get their own tile, on both sides', !t0.bad && t0.b.filter(isCam).length === 1 && t0.b.filter(isScreen).length === 1 && new Set(t0.b.map(t => t.id)).size === 2, J(t0));
    await sleep(1500);
    for (const n of ['alice', 'bob']) await page(n).ev(`(window.__vtReset(), true)`);
    // unrelated state updates while both videos run: mute and deafen, a volume change, plain change events (speaking, polls)
    for (let i = 0; i < 4; i++) {
      await call('bob', `c.setMuted(true)`); await call('alice', `c.setDeafened(true)`); await sleep(400);
      await call('bob', `c.setMuted(false)`); await call('alice', `c.setDeafened(false)`); await call('alice', `c.setMuted(false)`);
      await call('bob', `c.setVolume(${J(dev('alice'))}, ${0.5 + i / 10})`); await call('bob', `(c.changed(), true)`); await sleep(600);
    }
    await sleep(2500);
    const steady = {}; for (const n of ['alice', 'bob']) steady[n] = { vt: await page(n).ev(`({ ...window.__vt, ratio: window.__vt.blank / (window.__vt.samples || 1) })`), tiles: await tilesOf(n) };
    const calm = n => { const s = steady[n]; const before = n === 'bob' ? t0.b : t0.a; return s.vt.added === 0 && s.vt.removed === 0 && !Object.keys(s.vt.events).length && s.vt.ratio < 0.02 && s.vt.samples > 100 && J(s.tiles.map(t => t.id).sort()) === J(before.map(t => t.id).sort()); };
    check('J2 no flicker: state updates leave the video tiles and elements in place, and they keep playing (viewer)', calm('bob'), J(steady.bob.vt));
    check('J3 no flicker on the sender\'s own previews either', calm('alice'), J(steady.alice.vt));
    // camera off: the SDK only mutes it; the tile must go away on both sides, and the screen share must not be touched
    const screenId = { a: t0.a.find(isScreen)?.id, b: t0.b.find(isScreen)?.id };
    await call('alice', `c.setCamera(false)`);
    const camGone = n => until(async () => { const t = await tilesOf(n); return t.length === 1 && isScreen(t[0]) ? t : null; }, 8000, n + ' camera tile gone').catch(async () => tilesOf(n));
    const offB = await camGone('bob'), offA = await camGone('alice');
    check('J4 camera off: its tile goes away for the viewer (no black box)', offB.length === 1 && isScreen(offB[0]), J(offB));
    check('J5 camera off: your own camera preview goes away too', offA.length === 1 && isScreen(offA[0]), J(offA));
    check('J6 the screen share tile stays the same element while the camera goes', offB[0]?.id === screenId.b && offA[0]?.id === screenId.a && offB[0]?.playing, J({ screenId, offB, offA }));
    await call('alice', `c.setCamera(true)`);
    const back = await until(async () => { const t = await tilesOf('bob'); return t.length === 2 && t.every(x => x.w > 0 && x.playing) ? t : null; }, 20000, 'camera back').catch(async () => tilesOf('bob'));
    check('J7 camera on again: the tile comes back with pictures', back.length === 2 && back.filter(isCam).length === 1 && back.every(x => x.w > 0 && x.playing), J(back));
    // ---- K. the camera stack (mega phase 4): all tiles in one column, dragging reorders for everyone in the call
    {
      // a tab in the background does not poll the live channel (as in the real app): the person who is meant to see a change is brought to the front
      const front = async n => { await page(n).send('Page.bringToFront'); await sleep(300); };
      const shotOf = async (n, name) => { try { await page(n).send('Page.bringToFront'); await sleep(300); const r = await page(n).send('Page.captureScreenshot', { format: 'png' }); fs.mkdirSync(out, { recursive: true }); fs.writeFileSync(path.join(out, name + '.png'), Buffer.from(r.result.data, 'base64')); } catch { } };
      const deck = (n, expr) => page(n).ev(`(() => { const d = window.__megaDeck; return ${expr}; })()`);
      const ord = n => deck(n, `d.order()`);
      const visual = n => page(n).ev(`[...document.querySelectorAll('#c-shares .ctile')].sort((a, b) => (+a.style.order || 0) - (+b.style.order || 0)).map(e => e.dataset.key.slice(e.dataset.key.indexOf('/') + 1))`);
      const sentDeck = n => page(n).ev(`(window.__megaLive.stats.kinds['deck.m'] || 0) + (window.__megaLive.stats.kinds['deck.q'] || 0)`);
      // a scripted drag of the first tile to the bottom and back to the middle over `ms`, with a move every ~40 ms
      const drag = (n, ms, hold) => page(n).ev(`(async () => {
        const tiles = [...document.querySelectorAll('#c-shares .ctile')].sort((a, b) => (+a.style.order || 0) - (+b.style.order || 0));
        const el = tiles[0], r = el.getBoundingClientRect(), last = tiles[tiles.length - 1].getBoundingClientRect();
        const x = r.left + r.width / 2, y0 = r.top + r.height / 3, y1 = last.bottom - 6, id = 11, ev = (t, y) => new PointerEvent(t, { clientX: x, clientY: y, pointerId: id, bubbles: true, cancelable: true, button: 0, pointerType: 'mouse' });
        el.dispatchEvent(ev('pointerdown', y0));
        const steps = Math.max(2, Math.round(${ms} / 40));
        for (let i = 1; i <= steps; i++) { window.dispatchEvent(ev('pointermove', y0 + (y1 - y0) * (i / steps))); await new Promise(r => setTimeout(r, 40)); }
        ${hold ? 'return true;' : `window.dispatchEvent(ev('pointerup', y1)); return true;`} })()`);
      const upN = n => page(n).ev(`(window.dispatchEvent(new PointerEvent('pointerup', { pointerId: 11, bubbles: true, button: 0 })), true)`);
      const same = (a, b) => J(a) === J(b);
      await front('bob');
      await call('alice', `c.setCamera(true)`);
      await until(async () => (await tilesOf('bob')).length === 2 && (await tilesOf('alice')).length === 2, 20000, 'K: two tiles on both sides').catch(() => { });
      await until(async () => (await deck('alice', `d.tiles().length`)) === 2 && (await deck('bob', `d.tiles().length`)) === 2, 10000, 'K: both decks know both tiles');
      check('K1 the stack is open on the left for people in the call, and the dots slide right to make room', await page('bob').ev(`(() => { const s = document.querySelector('#call-stage').getBoundingClientRect(), shown = e => e && !e.hidden && e.getBoundingClientRect().width > 0, r = [document.querySelector('#mega-rail'), document.querySelector('#mega')].find(shown); return document.documentElement.classList.contains('deck-open') && s.left <= 1 && s.width > 120 && !!r && Math.abs(r.getBoundingClientRect().left - s.width) < 3; })()`), J(await page('bob').ev(`[document.querySelector('#call-stage').getBoundingClientRect().width, [document.querySelector('#mega-rail'), document.querySelector('#mega')].map(e => e && !e.hidden ? e.getBoundingClientRect().left : null)]`)));
      await sleep(600);
      const rects = n => page(n).ev(`[...document.querySelectorAll('#c-shares .ctile')].sort((a, b) => (+a.style.order || 0) - (+b.style.order || 0)).map(e => { const r = e.getBoundingClientRect(); return [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)]; })`);
      const stacked = r => r.length >= 2 && r.every(x => x[3] >= 90 && x[2] >= 120) && r.every((x, i) => i === 0 || x[1] >= r[i - 1][1] + r[i - 1][3] - 1);
      check('K1b the tiles are stacked top to bottom, none over another, each a real picture size', stacked(await rects('bob')), J(await rects('bob')));
      const base = await ord('bob');
      check('K2 both sides start from a valid order with every tile once', same([...(await ord('alice'))].sort(), [...base].sort()) && base.length === 2 && same(await visual('bob'), base), J([await ord('alice'), base]));
      // a 3 second drag by alice: the order changes for bob as well, with at most about 10 events
      const e0 = await sentDeck('alice');
      await drag('alice', 3000, false);
      await sleep(1500);
      const e1 = await sentDeck('alice');
      const aOrd = await ord('alice');
      const moved = !same(aOrd, base);
      await until(async () => same(await ord('bob'), aOrd), 8000, 'bob follows alice\'s order').catch(() => { });
      check('K3 dragging a tile reorders it for the other person in the call, and their tiles really moved', moved && same(await ord('bob'), aOrd) && same(await visual('bob'), aOrd) && same(await visual('alice'), aOrd), J([base, aOrd, await ord('bob'), await visual('bob')]));
      check('K4 a whole 3 second drag is at most about 10 live events (not one per pointer move)', e1 - e0 >= 2 && e1 - e0 <= 10, `${e1 - e0} events for ~75 pointer moves`);
      const idle0 = await sentDeck('alice'); await sleep(3500);
      check('K5 nothing is sent while idle', (await sentDeck('alice')) === idle0 && (await sentDeck('bob')) <= 12);
      await shotOf('alice', 'K-stack-alice'); await shotOf('bob', 'K-stack-bob');
      // a person who is not in the call cannot reorder anything, however well-formed the message and however high the stamp
      const callGid = world.gid, rev = [...aOrd].reverse();
      await rpc(P('carol').port, 'live_send', { gid: callGid, kind: 'deck.m', data: J({ s: '', o: rev, n: 99999, b: '', e: 1 }) });
      await sleep(3500);
      check('K6 an order from a member who is not in the call is ignored', same(await ord('bob'), aOrd) && same(await ord('alice'), aOrd), J([await ord('bob'), rev]));
      // junk from someone who is in the call: unknown tiles, duplicates, wrong types never hide or duplicate a tile
      const known = aOrd.slice();
      await rpc(P('alice').port, 'live_send', { gid: callGid, kind: 'deck.m', data: J({ s: '', o: ['nope', known[0], known[0], 'x'.repeat(10)], n: 100000, b: '', e: 1 }) });
      await sleep(1200);
      await rpc(P('alice').port, 'live_send', { gid: callGid, kind: 'deck.m', data: '{"o": 5}' });
      await sleep(3000);
      const bAfter = await ord('bob'), vAfter = await visual('bob');
      check('K7 unknown, repeated and malformed entries never hide or duplicate a tile', same([...bAfter].sort(), [...known].sort()) && bAfter.length === known.length && same(vAfter, bAfter) && (await tilesOf('bob')).length === 2, J([bAfter, vAfter]));
      // first grab holds the tile; it expires by itself after about 3 s of silence
      await drag('alice', 400, true);   // grabbed and moved, button still down, then silence
      await until(async () => (await deck('bob', `d.holds().length`)) === 1, 8000, 'bob sees alice\'s hold').catch(() => { });
      const marked = await page('bob').ev(`(() => { const e = document.querySelector('#c-shares .ctile.held'); return e ? e.dataset.holder : null; })()`);
      const b0 = await ord('bob');
      await drag('bob', 400, false);   // bob tries to take the same (first) tile
      await sleep(600);
      check('K8 while alice holds a tile bob cannot take it, and her name is on it', marked === 'Alice' && same(await ord('bob'), b0), J([marked, b0, await ord('bob')]));
      await sleep(3600);
      const expired = await deck('bob', `d.holds().length`);
      check('K9 the hold expires by itself after about 3 s of silence', expired === 0 && (await page('bob').ev(`!document.querySelector('#c-shares .ctile.held')`)), J(await deck('bob', `d.holds()`)));
      await upN('alice');
      await sleep(800);
      // a tile leaving: the order stays valid on both sides, and the remaining tile is kept
      const camKeep = (await visual('bob'))[0];
      await call('alice', `c.stopShare()`);
      await until(async () => (await tilesOf('bob')).length === 1 && (await tilesOf('alice')).length === 1, 10000, 'K: one tile left').catch(() => { });
      await sleep(1200);
      const [lb, la] = [await ord('bob'), await ord('alice')];
      check('K10 a reorder survives a tile leaving: one tile left on both sides, each exactly once', lb.length === 1 && la.length === 1 && same(await visual('bob'), lb) && (await tilesOf('bob')).length === 1, J([lb, la]));
      // the hide control only changes my own view
      await page('bob').ev(`document.querySelector('#mega-deckhide').click()`); await sleep(900);
      check('K11 hide collapses my stack and slides everything back, without touching anyone else\'s', await page('bob').ev(`!document.documentElement.classList.contains('deck-open') && getComputedStyle(document.querySelector('#call-stage')).display === 'none' && !document.querySelector('#mega-deckshow').hidden`) && await page('alice').ev(`document.documentElement.classList.contains('deck-open')`));
      await page('bob').ev(`document.querySelector('#mega-deckshow').click()`); await sleep(900);
      check('K12 show brings it back', await page('bob').ev(`document.documentElement.classList.contains('deck-open') && document.querySelector('#mega-deckshow').hidden`));
      await call('alice', `c.startShare({ audio: false, profile: 'motion' })`);
      await until(async () => (await tilesOf('bob')).length === 2, 20000, 'K: share back').catch(() => { });
      // a phone (390 px): the same stack as a scrollable strip across the top, and nothing slides
      await page('bob').send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true }); await sleep(1500);
      const ph = await page('bob').ev(`(() => { const s = document.querySelector('#call-stage').getBoundingClientRect(), t = [...document.querySelectorAll('#c-shares .ctile')].sort((a, b) => (+a.style.order || 0) - (+b.style.order || 0)).map(e => { const r = e.getBoundingClientRect(); return [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)]; }), r = document.querySelector('#mega-rail'), cs = getComputedStyle(document.querySelector('#call-stage')); return { stage: [Math.round(s.left), Math.round(s.top), Math.round(s.width), Math.round(s.height)], tiles: t, railLeft: r && !r.hidden ? Math.round(r.getBoundingClientRect().left) : null, overflowX: cs.overflowX, hscroll: document.documentElement.scrollWidth > innerWidth + 1, deckw: getComputedStyle(document.documentElement).getPropertyValue('--m-deckw') }; })()`);
      await shotOf('bob', 'K-stack-phone');
      const dock = await page('bob').ev(`(() => { const b = document.querySelector('#call-dock .cd-acts .cd-b:last-of-type'), r = b && b.getBoundingClientRect(); if (!r) return null; const e = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2); return { h: r.height, top: r.top, bottom: r.bottom, vh: innerHeight, hit: !!(e && e.closest('#call-dock')), by: e ? (e.id || e.className || e.tagName) : null }; })()`);
      check('K14 on a phone the call controls are on screen and can be pressed, on the cards too', !!dock && dock.hit && dock.h >= 20 && dock.top >= 0 && dock.bottom <= dock.vh, J(dock));
      check('K13 on a phone the stack is a scrollable strip across the top, tiles side by side, and the dots do not slide', ph.stage[0] <= 1 && ph.stage[1] <= 1 && ph.stage[2] >= 380 && ph.stage[3] >= 80 && ph.stage[3] <= 140 && ph.tiles.length === 2 && ph.tiles[1][0] >= ph.tiles[0][0] + ph.tiles[0][2] - 1 && ph.tiles.every(t => t[3] >= 60) && (ph.railLeft === null || ph.railLeft <= 1) && ph.overflowX === 'auto' && !ph.hscroll, J(ph));
      await page('bob').send('Emulation.clearDeviceMetricsOverride'); await sleep(1200);
      const strip = await page('bob').ev(`(() => { window.dispatchEvent(new Event('resize')); const d = document.querySelector('#call-dock').getBoundingClientRect(), m = document.querySelector('#mega').getBoundingClientRect(), b = document.querySelector('#call-dock .cd-acts .cd-b:last-of-type').getBoundingClientRect(), e = document.elementFromPoint(b.left + b.width / 2, b.top + b.height / 2); return { left: d.left, right: d.right, w: d.width, h: d.height, vw: innerWidth, megaRight: m.right, hit: !!(e && e.closest('#call-dock')), by: e ? (e.id || e.className || e.tagName) : null }; })()`);
      await shotOf('bob', 'K-strip');
      check('K15 on a computer the call controls are a vertical strip on the right and the app makes room', strip.right >= strip.vw - 16 && strip.w <= 72 && strip.h > strip.w * 2 && strip.megaRight <= strip.left + 1 && strip.hit, J(strip));
    }
    await call('alice', `c.stopShare()`);
    const noShare = await until(async () => { const t = await tilesOf('bob'); return t.length === 1 && isCam(t[0]) ? t : null; }, 8000, 'share gone').catch(async () => tilesOf('bob'));
    check('J8 stopping the share removes only the screen tile', noShare.length === 1 && isCam(noShare[0]) && noShare[0].id === back.find(isCam)?.id, J(noShare));
    await call('alice', `c.setCamera(false)`);
    for (const n of ['alice', 'bob']) await call(n, `c.leave()`);
    await sleep(1000);
  }
  if (process.env.CALL_E2E_ONLY === 'video') throw Object.assign(new Error('CALL_E2E_ONLY=video: the other sections are skipped'), { only: true });

  // ---------------------------------------------------------------- 0. what the relay says about call support
  const sup = await page('alice').ev(`CommonsCall.support()`);
  check('browser reports frame-encryption support', sup.ok && sup.e2ee === 'insertable-streams', J(sup.e2ee));

  // ---------------------------------------------------------------- A. two clients
  await call('alice', `c.join(S.gid, 'general')`);
  await call('bob', `c.join(S.gid, 'general')`);
  await until(async () => (await call('alice', `c.room.remoteParticipants.size`)) === 1, 15000, 'alice sees bob');
  check('A1 two clients connect and both report frame encryption active', (await call('alice', `c.room.isE2EEEnabled`)) && (await call('bob', `c.room.isE2EEEnabled`)));
  const ea = await call('alice', `c.snapshot().encryption`), eb = await call('bob', `c.snapshot().encryption`);
  check('A2 both derive the same MLS epoch/key index', ea.epoch === eb.epoch && ea.keyIndex === eb.keyIndex, `epoch ${ea.epoch}`);
  check('A3 bob hears alice (tone share)', await hears('bob', 'alice'), J(await hear('bob', 'alice')));
  check('A4 alice hears bob (tone share)', await hears('alice', 'bob'), J(await hear('alice', 'bob')));
  await until(async () => (await call('alice', `c.speaking.has(${J(dev('bob'))})`)), 8000, 'speaking indicator').then(() => check('A5 "who is speaking" reaches the other side', true), () => check('A5 "who is speaking" reaches the other side', false));

  // what the media server (and its operator) can see
  const parts = (await lkAdmin('ListParticipants', { room: roomName })).participants || [];
  const rooms = (await lkAdmin('ListRooms', {})).rooms || [];
  check('A6 server room name is not the group id, and carries no names', rooms.length === 1 && rooms[0].name === roomName && roomName !== world.gid && !JSON.stringify(rooms).includes('general') && !JSON.stringify(rooms).includes('Test club'), rooms.map(r => r.name.slice(0, 12)).join());
  check('A7 server participants are bare device ids with no display names', parts.length === 2 && parts.every(p => /^[0-9a-f]{64}$/.test(p.identity) && !p.name), parts.map(p => p.identity.slice(0, 8) + ':' + J(p.name)).join());
  check('A8 tracks are marked encrypted (AES-GCM) on the server', parts.every(p => (p.tracks || []).length > 0 && p.tracks.every(t => t.encryption === 'GCM' || t.encryption === 1)), J(parts.map(p => (p.tracks || []).map(t => t.encryption))));

  // ---------------------------------------------------------------- B. three clients
  await call('carol', `c.join(S.gid, 'general')`);
  await until(async () => (await call('alice', `c.room.remoteParticipants.size`)) === 2, 15000, 'three in room');
  const mesh = {};
  for (const l of ['alice', 'bob', 'carol']) for (const s of ['alice', 'bob', 'carol']) if (l !== s) mesh[l + '<-' + s] = await hears(l, s);
  check('B1 three clients: everyone hears everyone (6 pairs)', Object.values(mesh).every(Boolean), J(mesh));

  // ---------------------------------------------------------------- C. controls
  await call('alice', `c.setMuted(true)`); await sleep(1200);
  const muted = await hear('bob', 'alice');
  check('C1 mute silences you for others', !(muted.toneShare > TONE_MAX) || muted.rms < 0.002, J(muted));
  await call('alice', `c.setMuted(false)`); await sleep(1200);
  check('C2 unmute brings you back', await hears('bob', 'alice'));
  await call('bob', `c.setDeafened(true)`);
  const deaf = await call('bob', `({ muted: c.muted, els: [...c.audioEls.values()].every(e => e.muted), snap: c.snapshot().deafened })`);
  check('C3 deafen mutes you and all playback', deaf.muted && deaf.els && deaf.snap, J(deaf));
  await call('bob', `c.setDeafened(false)`); await call('bob', `c.setMuted(false)`);
  await call('bob', `c.setVolume(${J(dev('alice'))}, 0.3)`);
  const vol = await call('bob', `({ el: [...c.audioEls.entries()].filter(([k]) => k.startsWith(${J(dev('alice'))})).map(([, e]) => e.volume)[0], saved: JSON.parse(localStorage.getItem('commons.call.volumes'))[ ${J(P('alice').account)} ] })`);
  check('C4 per-person volume applies and is remembered', Math.abs(vol.el - 0.3) < 0.01 && vol.saved === 0.3, J(vol));
  // push to talk: carol turns it on (starts muted), holds the key via real key events
  await call('carol', `c.setPtt(true)`); await sleep(500);
  const pttBefore = await call('carol', `c.muted`);
  await page('carol').send('Input.dispatchKeyEvent', { type: 'keyDown', code: 'Backquote', key: '`', windowsVirtualKeyCode: 192 });
  await sleep(1500);
  const pttHeard = await hears('alice', 'carol');
  await page('carol').send('Input.dispatchKeyEvent', { type: 'keyUp', code: 'Backquote', key: '`', windowsVirtualKeyCode: 192 });
  await sleep(1500);
  const pttAfter = await call('carol', `c.muted`);
  check('C5 push to talk: muted until the key is held, heard while held, muted again after', pttBefore === true && pttHeard && pttAfter === true, J({ pttBefore, pttHeard, pttAfter }));
  await call('carol', `c.setPtt(false)`); await call('carol', `c.setMuted(false)`); await sleep(800);
  const devs = await call('alice', `c.listDevices()`);
  check('C6 device lists come from the (fake) devices', devs.mic.length > 0 && devs.cam.length > 0, J({ mic: devs.mic.length, out: devs.out.length, cam: devs.cam.length }));
  let devErr = null; try { await call('alice', `c.pickDevice('audioinput', ${J(devs.mic[0].deviceId)})`); await call('alice', `c.setNoiseSuppression(false)`); await call('alice', `c.setNoiseSuppression(true)`); } catch (e) { devErr = e.message; }
  check('C7 picking a microphone and toggling noise suppression works while live', !devErr && await hears('bob', 'alice'), devErr || '');

  // ---------------------------------------------------------------- D. screen share (synthetic frames, never a real screen)
  for (const n of ['alice', 'bob']) { await page(n).ev(FAKE_DISPLAY); await page(n).ev(`window.__shareName = ${J(n)}`); }
  await call('alice', `c.startShare({ audio: true, profile: 'text' })`);
  const viewShot = async (viewer, sharer) => page(viewer).ev(`(async()=>{
      const c = window.__commonsCall; let t; for (let i = 0; i < 100 && !t; i++) { t = [...c.tracks.values()].find(x => x.identity === ${J(dev(sharer))} && x.source === 'screen_share'); if (!t) await new Promise(r => setTimeout(r, 200)); } if (!t) return { present: false };
      const v = document.createElement('video'); v.muted = true; v.autoplay = true; t.track.attach(v); document.body.appendChild(v); v.style.cssText = 'position:fixed;left:-9999px';
      const t0 = Date.now(); while (!(v.videoWidth > 0) && Date.now() - t0 < 10000) await new Promise(r => setTimeout(r, 100));
      await new Promise(r => setTimeout(r, 1500));
      const cv = document.createElement('canvas'); cv.width = v.videoWidth; cv.height = v.videoHeight; const g = cv.getContext('2d'); g.drawImage(v, 0, 0);
      const px = (fx, fy) => Array.from(g.getImageData(Math.round(fx * cv.width), Math.round(fy * cv.height), 1, 1).data.slice(0, 3));
      const r = { present: true, w: v.videoWidth, h: v.videoHeight, red: px(.19, .28), green: px(.51, .28), blue: px(.82, .28) }; v.remove(); return r; })()`);
  const shot = await viewShot('bob', 'alice');
  const near = (a, b) => a.every((x, i) => Math.abs(x - b[i]) < 60);
  check('D1 screen share arrives and decodes (pattern blocks have the right colours)', shot.present && near(shot.red, [224, 32, 32]) && near(shot.green, [32, 192, 64]) && near(shot.blue, [32, 64, 224]), J(shot));
  check('D2 a second viewer gets it too', (await viewShot('carol', 'alice')).present);
  check('D3 screen audio (a 1 kHz tone) arrives', await (async () => { const r = await page('bob').ev(`(async()=>{ const c = window.__commonsCall; let t; for (let i = 0; i < 100 && !t; i++) { t = [...c.tracks.values()].find(x => x.identity === ${J(dev('alice'))} && x.source === 'screen_share_audio'); if (!t) await new Promise(r => setTimeout(r, 200)); } if (!t) return { present: false, note: 'no screen audio track' }; return c.probeTone(${J(dev('alice'))}, 1000, 1500, 'screen_share_audio'); })()`); results[results.length - 1].detail = J(r); return r.present && r.toneShare > TONE_MIN; })());
  await call('bob', `c.startShare({ audio: false, profile: 'motion' })`);
  await until(async () => (await call('carol', `[...c.tracks.values()].filter(x => x.source === 'screen_share').length`)) === 2, 15000, 'two shares');
  check('D4 several sharers: carol sees both alice and bob sharing', true);
  const bobShot = await viewShot('alice', 'bob');
  check('D5 bob\'s share (smooth profile) decodes for alice', bobShot.present && near(bobShot.red, [224, 32, 32]), J(bobShot));
  // viewer quality choice
  const qualityH = async (viewer, sharer) => page(viewer).ev(`(async()=>{ const c = window.__commonsCall; const t = [...c.tracks.values()].find(x => x.identity === ${J(dev(sharer))} && x.source === 'screen_share'); const v = document.createElement('video'); v.muted = true; t.track.attach(v); document.body.appendChild(v); v.style.cssText='position:fixed;left:-9999px'; await new Promise(r => setTimeout(r, 4000)); const h = v.videoHeight; v.remove(); return h; })()`);
  const hHigh = await (async () => { await call('carol', `c.setViewQuality(${J(await call('carol', `[...c.tracks.values()].find(x => x.identity === ${J(dev('alice'))} && x.source === 'screen_share').pub.trackSid`))}, 'high')`); return qualityH('carol', 'alice'); })();
  const hLow = await (async () => { await call('carol', `c.setViewQuality(${J(await call('carol', `[...c.tracks.values()].find(x => x.identity === ${J(dev('alice'))} && x.source === 'screen_share').pub.trackSid`))}, 'low')`); await sleep(2500); return qualityH('carol', 'alice'); })();
  check('D6 viewer can pick a lower quality (resolution drops)', hLow > 0 && hLow < hHigh, `high ${hHigh}p, low ${hLow}p`);
  const fsr = await page('carol').send('Runtime.evaluate', { userGesture: true, awaitPromise: true, returnByValue: true, expression: `(async()=>{ const c = window.__commonsCall; const t = [...c.tracks.values()].find(x => x.source === 'screen_share'); const v = document.createElement('video'); v.muted = true; t.track.attach(v); document.body.appendChild(v); await v.play().catch(()=>{}); try { await v.requestFullscreen(); } catch (e) { return 'err ' + e.message; } const ok = document.fullscreenElement === v; await document.exitFullscreen().catch(()=>{}); v.remove(); return ok; })()` });
  const fs1 = fsr.result.result.value;
  check('D7 fullscreen request on a share tile is issued (headless Chromium has no window and denies the grant: actual fullscreen UNVERIFIED here)', fs1 === true || /not granted|Permissions/.test(String(fs1)), String(fs1));
  await call('alice', `c.setCamera(true)`);
  const camOk = await until(async () => page('bob').ev(`(async()=>{ const c = window.__commonsCall; const t = [...c.tracks.values()].find(x => x.identity === ${J(dev('alice'))} && x.source === 'camera'); if (!t) return false; const v = document.createElement('video'); v.muted = true; t.track.attach(v); document.body.appendChild(v); await new Promise(r => setTimeout(r, 2000)); const w = v.videoWidth; v.remove(); return w > 0; })()`), 20000, 'camera').catch(() => false);
  check('D8 camera (fake device) arrives and decodes', camOk);
  const camOut = await page('alice').ev(`(()=>{ const c = window.__commonsCall, p = c.room.localParticipant.getTrackPublication('camera'), t = p.track, enc = t.sender.getParameters().encodings; const set = t.mediaStreamTrack.getSettings(); return { w: set.width, h: set.height, fps: set.frameRate, top: Math.max(...enc.map(e => e.maxBitrate || 0)), layers: enc.length }; })()`);
  check('D8b the camera captures 720p and may send about 2.5 Mbps (it was 360p at 450 kbps)', camOut.w >= 1280 && camOut.h >= 720 && camOut.top >= 2_000_000 && camOut.layers >= 2, J(camOut));
  const shareOut = await page('alice').ev(`(()=>{ const c = window.__commonsCall, p = c.room.localParticipant.getTrackPublication('screen_share'), enc = p.track.sender.getParameters().encodings; return { top: Math.max(...enc.map(e => e.maxBitrate || 0)), layers: enc.length, pref: p.track.sender.getParameters().degradationPreference || null }; })()`);
  check('D8c a screen share may send at least 5 Mbps (it was 2 to 2.5 Mbps)', shareOut.top >= 5_000_000 && shareOut.layers >= 2, J(shareOut));
  await call('alice', `c.setCamera(false)`);
  await call('alice', `c.stopShare()`); await call('bob', `c.stopShare()`);
  await sleep(1000);

  // ---------------------------------------------------------------- E. what the media server sees (operator joins as a participant)
  // An operator can mint tokens, so it can join the room with no key. It records the frames the server forwards, and a plaintext
  // control publisher of its own tone shows what an unencrypted call would look like.
  const spy = await openPage(P('alice').chrome, `http://127.0.0.1:${P('alice').port}/vendor/README.txt`, 'spy');
  await spy.ev(UMD);   // evaluated through the debugger: the page's CSP forbids eval, the test does not need to
  const spyTok = operatorToken({ room: roomName, roomJoin: true, canPublish: true, canSubscribe: true }, 'operator-spy');
  const plainTok = operatorToken({ room: roomName, roomJoin: true, canPublish: true, canSubscribe: true }, 'operator-plain');
  const spyRes = await spy.ev(`(async()=>{
    // capture every encoded frame the server forwards to us (what is on the wire after SRTP), per remote track
    const Orig = window.RTCPeerConnection; window.__frames = {}; window.__decoded = {};
    window.RTCPeerConnection = function (cfg, ...r) { cfg = Object.assign({}, cfg, { encodedInsertableStreams: true }); const pc = new Orig(cfg, ...r); const origSetCfg = pc.setConfiguration.bind(pc); pc.setConfiguration = c => origSetCfg(Object.assign({}, c, { encodedInsertableStreams: true }));
      pc.addEventListener('track', ev => { if (ev.track.kind !== 'audio') return; const key = ev.track.id; window.__frames[key] = []; let es; try { es = ev.receiver.createEncodedStreams(); } catch (e) { (window.__capErr = window.__capErr || []).push(String(e)); return; } const { readable, writable } = es;
        readable.pipeThrough(new TransformStream({ transform(f, ctl) { if (window.__frames[key].length < 150) window.__frames[key].push(Array.from(new Uint8Array(f.data))); ctl.enqueue(f); } })).pipeTo(writable); });
      return pc; };
    window.RTCPeerConnection.prototype = Orig.prototype;
    const room = new LivekitClient.Room({ adaptiveStream: false }); window.__room = room;
    room.on('trackSubscribed', (t, pub, p) => { window.__sub = window.__sub || []; window.__sub.push({ who: p.identity, src: pub.source, enc: pub.isEncrypted, trackId: t.mediaStreamTrack.id }); const a = t.attach(); document.body.appendChild(a); });
    await room.connect('ws://127.0.0.1:${LK_PORT}', ${J(spyTok)}, { autoSubscribe: true });
    return true; })()`);
  // the plaintext control: an unencrypted publisher with the same kind of tone (500 Hz, from alice's browser's fake microphone)
  const ctl = await openPage(P('alice').chrome, `http://127.0.0.1:${P('alice').port}/vendor/README.txt`, 'plain');
  await ctl.ev(UMD);
  await ctl.ev(`(async()=>{ const room = new LivekitClient.Room(); await room.connect('ws://127.0.0.1:${LK_PORT}', ${J(plainTok)}); await room.localParticipant.setMicrophoneEnabled(true, undefined, { red: false }); window.__r = room; return true; })()`);
  await sleep(6000);
  const sub = await spy.ev(`window.__sub`);
  console.log('spy capture errors:', J(await spy.ev(`({ errs: window.__capErr || [], has: typeof RTCRtpReceiver.prototype.createEncodedStreams, script: typeof RTCRtpScriptTransform, ua: navigator.userAgent })`)));
  const analysis = await spy.ev(`(async()=>{
    const frames = window.__frames; const out = [];
    const eq = (a, b) => { let n = 0, m = Math.min(a.length, b.length); for (let i = 0; i < m; i++) if (a[i] === b[i]) n++; return m ? n / m : 0; };
    for (const [id, fr] of Object.entries(frames)) {
      const sub = window.__sub.find(s => s.trackId === id) || {};
      const big = fr.filter(f => f.length > 40); const body = big.map(f => f.slice(1, f.length - 14));
      let same = 0, cnt = 0, bs = 0; for (let i = 1; i < big.length; i++) { same += eq(big[i], big[i - 1]); bs += eq(body[i], body[i - 1]); cnt++; }
      // decode the forwarded frames with a real Opus decoder and measure the tone
      let toneShare = null, derr = 0;
      try {
        const pcm = []; const dec = new AudioDecoder({ output: d => { const a = new Float32Array(d.numberOfFrames); d.copyTo(a, { planeIndex: 0, format: 'f32-planar' }); pcm.push(a); d.close(); }, error: () => derr++ });
        dec.configure({ codec: 'opus', sampleRate: 48000, numberOfChannels: 1 });
        let ts = 0; for (const f of fr) { try { dec.decode(new EncodedAudioChunk({ type: 'key', timestamp: ts, data: new Uint8Array(f) })); } catch { derr++; } ts += 20000; }
        await dec.flush().catch(() => derr++);
        const x = new Float32Array(pcm.reduce((n, a) => n + a.length, 0)); let o = 0; for (const a of pcm) { x.set(a, o); o += a.length; }
        const N = Math.min(x.length, 48000); const hz = 500; let re = 0, im = 0, tot = 0; for (let i = 0; i < N; i++) { re += x[i] * Math.cos(2 * Math.PI * hz * i / 48000); im += x[i] * Math.sin(2 * Math.PI * hz * i / 48000); tot += x[i] * x[i]; }
        toneShare = tot > 0 ? (2 * (re * re + im * im) / N) / tot : 0;
        if (!(N > 4000)) toneShare = null;
      } catch (e) { toneShare = 'ERR ' + e.message; }
      out.push({ who: sub.who, src: sub.src, markedEncrypted: sub.enc, frames: fr.length, bytesEqualBetweenFrames: +(cnt ? same / cnt : 0).toFixed(4), bodyEqual: +(cnt ? bs / cnt : 0).toFixed(4), opusToneShare500: toneShare, decodeErrors: derr, sizes: fr.slice(0, 5).map(f => f.length) });
    }
    return out; })()`);
  console.log('operator view of forwarded audio frames:'); for (const a of analysis) console.log('   ', J(a));
  const plain = analysis.filter(a => a.who === 'operator-plain'), enc = analysis.filter(a => a.who && a.who !== 'operator-plain');
  check('E1 operator sees our tracks as encrypted ones', enc.length >= 3 && enc.every(a => a.markedEncrypted), `${enc.length} encrypted audio tracks from ${new Set(enc.map(a => a.who)).size} members`);
  const toneOf = a => typeof a.opusToneShare500 === 'number' ? a.opusToneShare500 : null;
  const plainTone = plain.map(toneOf).filter(x => x !== null)[0];
  check('E2 control: a plaintext Opus frame stream decodes to the 500 Hz tone at the operator', plainTone !== undefined && plainTone > 0.3, `tone share ${plainTone}`);
  const encTones = enc.map(toneOf).filter(x => x !== null);
  const aliceEnc = enc.find(a => a.who === dev('alice')); const aliceShare = aliceEnc ? toneOf(aliceEnc) : null;
  check('E3 the same tone inside our E2EE frames is NOT decodable at the operator (alice tone 500 Hz)', aliceShare !== null && aliceShare < 0.05, `alice share ${aliceShare}; all ${J(encTones)}`);
  check('E4 forwarded E2EE frame bodies are not predictable from one frame to the next (ciphertext: ~0.4% equal bytes)', enc.length >= 3 && enc.every(a => a.frames >= 50 && a.bodyEqual < 0.02), J(enc.map(a => a.bodyEqual)) + ' vs plaintext frames ' + J(plain.map(a => a.bodyEqual)) + ' (the unencrypted 1-byte header and 14-byte IV/trailer are excluded)');
  const spyHear = await spy.ev(`(async()=>{ const el = [...document.querySelectorAll('audio')]; const res = []; for (const a of el) { const ctx = (() => { try { return new AudioContext({ sinkId: { type: 'none' } }); } catch { return new AudioContext(); } })(); await ctx.resume(); const s = ctx.createMediaStreamSource(a.srcObject); const an = ctx.createAnalyser(); an.fftSize = 8192; s.connect(an); const buf = new Float32Array(an.frequencyBinCount); let best = 0; for (let k = 0; k < 12; k++) { await new Promise(r => setTimeout(r, 100)); an.getFloatFrequencyData(buf); let tot = 0; for (const v of buf) tot += Math.pow(10, v / 10); const bin = Math.round(500 / (ctx.sampleRate / 8192)); let pk = 0; for (let i = bin - 2; i <= bin + 2; i++) pk = Math.max(pk, Math.pow(10, buf[i] / 10)); best = Math.max(best, pk / tot); } res.push(+best.toFixed(3)); await ctx.close(); } return res; })()`);
  console.log('operator playback tone shares (plain control among them):', J(spyHear));
  await spy.ev(`window.__room.disconnect().then(() => true)`); await ctl.ev(`window.__r.disconnect().then(() => true)`);

  // a member-like client that does NOT have the key (wrong keys, right everything else)
  const wk = await openPage(P('dave').chrome, `http://127.0.0.1:${P('dave').port}/`, 'wrongkey');
  await wk.waitFor(`!!window.__commonsCall && typeof S !== 'undefined' && !!S.gid`, 30000);
  const realState = await rpc(P('alice').port, 'call_state', { gid: world.gid });
  const wrongKeys = Object.fromEntries(Object.keys(realState.keys).map(k => [k, Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString('base64url')]));
  const wrongTok = await rpc(P('dave').port, 'call_token', { gid: world.gid });   // a valid token: the operator's view, minus the key
  const wrongTone = await wk.ev(`(async()=>{
    const orig = window.__commonsCall.rpc;
    const c = new CommonsCall({ rpc: async (m, a) => m === 'call_state' ? Object.assign({}, ${J(realState)}, { keys: ${J(wrongKeys)}, me: ${J(dev('dave'))} }) : orig(m, a) });
    window.__wk = c; await c.join(S.gid, 'general'); await new Promise(r => setTimeout(r, 5000));
    const res = {}; for (const id of [${J(dev('alice'))}, ${J(dev('bob'))}]) res[id.slice(0, 8)] = await c.probeTone(id, 500, 1200).catch(e => ({ err: e.message }));
    const snap = c.snapshot(); const fails = [...c.decryptFail.entries()].map(([k, v]) => [k.slice(0, 8), v.n]);
    return { res, fails, e2ee: c.room.isE2EEEnabled }; })()`);
  console.log('wrong-key client:', J(wrongTone));
  const wkShares = Object.values(wrongTone.res).map(r => r.present ? r.toneShare : 0);
  check('E5 a client in the room with the wrong keys hears no tone from anyone', wkShares.every(s => s < TONE_MAX) && wrongTone.fails.length > 0, `tone shares ${J(wkShares)}, decrypt failures ${J(wrongTone.fails)}`);
  await wk.ev(`window.__wk.leave().then(() => true)`);

  // ---------------------------------------------------------------- F. rotation on join, then kick
  const e0 = (await call('alice', `c.snapshot().encryption`));
  await call('dave', `c.join(S.gid, 'general')`);
  // erin joins the community mid-call: an add commit
  const erinPort = port(27210); startBridge(out, 'erin', erinPort); await sleep(1200);
  await rpc(erinPort, 'create_account', { name: 'Erin', passphrase: 'correct horse battery', relay: `http://127.0.0.1:${RELAY_PORT}` });
  const link = (await rpc(P('alice').port, 'create_invite', { cid: world.cid, uses: 2, ttl: 3600 })).link;
  await rpc(erinPort, 'join', { link });
  await until(async () => (await call('alice', `c.snapshot().encryption.epoch`)) > e0.epoch, 60000, 'epoch moves after erin joined');
  const e1 = await call('alice', `c.snapshot().encryption`);
  await until(async () => (await call('bob', `c.snapshot().encryption.epoch`)) === e1.epoch, 20000, 'bob follows');
  check('F1 a new member joining changes the epoch and the call keys rotate (no one drops out)', e1.epoch > e0.epoch && e1.rotations > e0.rotations, `epoch ${e0.epoch} to ${e1.epoch}`);
  await sleep(2500);
  check('F2 after the join-rotation everyone still hears everyone (alice-bob, bob-carol, carol-dave)', await hears('bob', 'alice') && await hears('carol', 'bob') && await hears('dave', 'carol'));
  const eRem = (await call('alice', `c.snapshot().encryption`)).epoch;

  // carol turns into a "modified client": ignores the removal and keeps using the keys it has. bob trusts all identities, so only
  // cryptography (not the membership filter) decides what he can hear.
  await call('carol', `(() => { const orig = c.rpc; let cache = null; c.rpc = async (m, a) => { try { const r = await orig(m, a); if (m === 'call_state') cache = r; return r; } catch (e) { if (m === 'call_state' && cache) return cache; throw e; } }; return true; })()`);
  await call('bob', `(c._testTrustAll = true)`);
  const preKick = { bobHearsCarol: await hears('bob', 'carol'), carolHearsBob: await hears('carol', 'bob') };
  await rpc(P('alice').port, 'kick', { cid: world.cid, account: P('carol').account });
  await until(async () => (await call('alice', `c.snapshot().encryption.epoch`)) > eRem, 60000, 'epoch moves after kick');
  const eAfter = (await call('alice', `c.snapshot().encryption`));
  await until(async () => (await call('bob', `c.snapshot().encryption.epoch`)) === eAfter.epoch, 20000, 'bob follows kick');
  await sleep(5500);  // old key slot is wiped after 3 s
  const post = { aliceBob: await hears('alice', 'bob'), bobAlice: await hears('bob', 'alice'), daveBob: await hears('dave', 'bob'), bobHearsCarol: await hears('bob', 'carol'), carolHearsBob: await hears('carol', 'bob'), carolHearsAlice: await hears('carol', 'alice'), aliceHearsCarol: await hears('alice', 'carol') };
  const carolFails = await call('carol', `[...c.decryptFail.entries()].map(([k, v]) => v.n).reduce((a, b) => a + b, 0)`);
  const bobFails = await call('bob', `(c.decryptFail.get(${J(dev('carol'))}) || { n: 0 }).n`);
  check('F3 positive control: remaining members still hear each other after the kick rotation', post.aliceBob && post.bobAlice && post.daveBob, J(post));
  check('F4 before the kick carol and bob heard each other', preKick.bobHearsCarol && preKick.carolHearsBob, J(preKick));
  const nh = { carolBob: await notHeard('carol', 'bob'), carolAlice: await notHeard('carol', 'alice'), bobCarol: await notHeard('bob', 'carol') };
  check('F5 after the kick carol (holding the old keys, tracks present) cannot decode bob or alice', nh.carolBob.ok && nh.carolAlice.ok, J({ nh, carolDecryptFailures: carolFails }));
  check('F6 after the kick bob (trusting her identity, track present) cannot decode carol\'s frames either', nh.bobCarol.ok, J({ nh: nh.bobCarol, bobDecryptFailuresForCarol: bobFails }));
  const blockedOnAlice = await call('alice', `c.snapshot().blocked.length`);
  check('F7 alice (normal client) refuses to play carol\'s tracks: not a member any more', blockedOnAlice > 0 && !post.aliceHearsCarol, `blocked ${blockedOnAlice}`);
  let carolTokenErr = null; try { await rpc(P('carol').port, 'call_token', { gid: world.gid }); } catch (e) { carolTokenErr = e.message; }
  let carolStateErr = null; try { await rpc(P('carol').port, 'call_state', { gid: world.gid }); } catch (e) { carolStateErr = e.message; }
  check('F8 the removed member cannot get a call token or keys for later calls', !!carolTokenErr && !!carolStateErr, `${carolTokenErr} | ${carolStateErr}`);
  await call('bob', `(c._testTrustAll = false)`);

  // ---------------------------------------------------------------- G. reconnect
  // (a) the media server restarts: every client's connection is cut; they must rejoin by themselves with fresh tokens and keys
  await lk.stopLivekit();
  await sleep(2500);
  const duringDrop = await call('alice', `c.phase`);
  lk.startLivekit();
  await until(async () => (await Promise.all(['alice', 'bob', 'dave'].map(n => call(n, `c.phase === 'connected' && c.room && c.snapshot().peers.filter(p => p.member && !p.me).length === 2`)))).every(Boolean), 90000, 'all rejoin').catch(async e => { for (const n of ['alice', 'bob', 'dave']) console.log('rejoin state', n, J(await call(n, `({ phase: c.phase, remote: c.room ? c.room.remoteParticipants.size : null, err: c.error, ev: c.events.slice(-6).map(e => e.msg) })`))); throw e; });
  check('G1 server restart: clients report the drop, then rejoin by themselves', duringDrop === 'reconnecting' || duringDrop === 'connected', `phase during drop ${duringDrop}`);
  await sleep(3500);
  const g2 = {}; for (const [l, sp] of [['bob', 'alice'], ['alice', 'dave'], ['dave', 'bob']]) { for (let i = 0; i < 4 && !g2[l + '<-' + sp]; i++) g2[l + '<-' + sp] = await hears(l, sp); }
  check('G2 after the rejoin everyone hears everyone again (up to ~60 s per pair)', Object.values(g2).every(Boolean), J(g2));
  if (!Object.values(g2).every(Boolean)) for (const n of ['alice', 'bob', 'dave']) console.log('G2 diag', n, J(await call(n, `({ phase: c.phase, e2ee: c.room.isE2EEEnabled, key: c.keyState, blocked: c.snapshot().blocked, fails: [...c.decryptFail].map(([k, v]) => [k.slice(0, 6), v.n, v.msg]), tracks: [...c.tracks.values()].map(t => [t.identity.slice(0, 6), t.source, t.pub.isEncrypted]), remote: [...c.room.remoteParticipants.values()].map(p => [p.identity.slice(0, 6), [...p.trackPublications.values()].map(x => [x.source, x.isSubscribed, x.isEncrypted, x.isMuted])]), ev: c.events.slice(-5).map(e => e.msg), muted: c.muted, deaf: c.deafened, els: [...c.audioEls.values()].map(e => [e.paused, e.muted, e.volume, e.srcObject && e.srcObject.getAudioTracks().map(t => [t.readyState, t.muted, t.enabled])]), stats: await Promise.all([...c.tracks.values()].map(async t => { const st = await t.track.receiver.getStats(); let o = null; st.forEach(r => { if (r.type === 'inbound-rtp') o = { who: t.identity.slice(0, 6), pk: r.packetsReceived, en: +(r.totalAudioEnergy || 0).toFixed(3), lvl: r.audioLevel, conc: r.concealedSamples, tot: r.totalSamplesReceived, dec: r.framesDecoded }; }); return o; })), mic: (() => { const p = c.room.localParticipant.getTrackPublication('microphone'); return p ? [p.isMuted, p.track && p.track.mediaStreamTrack.readyState, p.track && p.track.mediaStreamTrack.muted, p.isEncrypted] : null; })() })`)));
  const evs = await call('alice', `c.snapshot().events.map(e => e.msg)`);
  check('G3 the rejoin is logged for the user', evs.some(m => /rejoined automatically|reconnected/.test(m)), J(evs.slice(-4)));
  // (b) a signalling reconnect inside the live session (SDK-level resume)
  await call('bob', `c.room.simulateScenario('signal-reconnect')`).catch(() => { });
  await sleep(4000);
  check('G4 signal-reconnect scenario: bob stays in the call and keeps hearing alice', (await call('bob', `c.phase`)) === 'connected' && await hears('bob', 'alice'));

  // ---------------------------------------------------------------- H. refuses to start without frame encryption
  const nofe = await openPage(P('dave').chrome, `http://127.0.0.1:${P('dave').port}/`, 'noframeenc', { before: `delete RTCRtpSender.prototype.createEncodedStreams; delete RTCRtpReceiver.prototype.createEncodedStreams; delete window.RTCRtpScriptTransform;` });
  await nofe.waitFor(`!!window.__commonsCall && typeof S !== 'undefined' && !!S.gid`, 30000);
  const before = ((await lkAdmin('ListParticipants', { room: roomName })).participants || []).length;
  const refuse = await nofe.ev(`(async()=>{ try { await __commonsCall.join(S.gid, 'general'); return { joined: true }; } catch (e) { return { joined: false, kind: e.kind, msg: e.message, phase: __commonsCall.phase, support: CommonsCall.support() }; } })()`);
  const after = ((await lkAdmin('ListParticipants', { room: roomName })).participants || []).length;
  check('H1 without frame encryption the call refuses to start, says why, and never connects', refuse.joined === false && refuse.kind === 'unsupported' && /encrypt/i.test(refuse.msg) && before === after, J({ kind: refuse.kind, msg: refuse.msg, participantsBefore: before, participantsAfter: after }));

  // ---------------------------------------------------------------- I. the real UI
  const ui = page('dave');
  await call('dave', `c.leave()`);
  await ui.ev(`window.__callUI.joinRoom(S.gid, 'general')`);   // a text room's call, joined as the rail's Join does (the Call button is gone)
  await ui.waitFor(`!document.querySelector('#call-dock').hidden`, 8000, 'call dock');
  await ui.waitFor(`!!document.querySelector('#call-dock .call-enc.ok')`, 20000, 'encrypted badge');
  // The people are the ones under the room in the rail, not a list in the dock.
  const peersShown = await until(async () => { const n = await ui.ev(`document.querySelectorAll('#rooms .room-live .live-p').length`); return n >= 3 ? n : 0; }, 20000, 'people under the room').catch(() => 0);
  await ui.ev(`document.querySelector('#c-set').click()`); await sleep(800);
  const settingsOpen = await ui.ev(`!document.querySelector('#call-settings').hidden && !!document.querySelector('#c-jf-url')`);
  const badge = await ui.ev(`document.querySelector('#call-dock .call-enc').innerText`);
  check('I1 UI: the Call button joins, the rail dock shows Encrypted and Settings, and the people under the room are listed', /Encrypted/.test(badge) && peersShown >= 3 && settingsOpen, `${badge}; ${peersShown} people in the rail; settings ${settingsOpen}`);
  await ui.ev(`document.querySelector('#cs-close').click()`); await sleep(300);
  await ui.send('Page.bringToFront'); await sleep(250);
  const shotRes = await ui.send('Page.captureScreenshot', { format: 'png' });
  fs.writeFileSync(path.join(out, 'call-dock.png'), Buffer.from(shotRes.result.data, 'base64'));
  await ui.ev(`document.querySelector('#c-leave')?.click()`);
  await sleep(1500);

  // ---------------------------------------------------------------- RG. ringing a voice room: one rings, the others see it and join, a stale ring rings nobody
  {
    for (const n of ['alice', 'bob', 'carol', 'dave']) { await call(n, `c.phase === 'idle' ? true : c.leave()`).catch(() => { }); await page(n).ev(`(pref.set('sound', 'off'), true)`); }   // silent: no ring tone in any test
    const vg = (await page('alice').ev(`rpc('create_channel', { cid: S.cid, name: 'lounge', kind: 'voice' })`)).id;
    for (const n of ['bob', 'dave']) await until(() => page(n).ev(`!!S.snap.communities.flatMap(c => c.channels).find(x => x.gid === ${J(vg)} && x.joined)`), 40000, n + ' has the voice room');
    const cards = n => page(n).ev(`[...document.querySelectorAll('#mega-ring .rg-card')].map(c => c.innerText.replace(/\\s+/g, ' ').trim())`);
    await call('alice', `c.join(${J(vg)}, 'lounge')`); await until(() => call('alice', `c.phase === 'connected'`), 30000, 'alice in the voice room');
    // a stale ring (a device catching up on an old log) rings nobody, even though it is a perfectly good envelope from a member
    await page('alice').ev(`rpc('debug_ring', { gid: ${J(vg)}, age: 100 })`);
    await sleep(3500);
    const stale = [await cards('bob'), await cards('dave')];
    check('RG1 a stale ring (older than the allowed age) rings nobody', stale.every(c => c.length === 0), J(stale));
    // the real thing, from the ring button's own code path
    const dockHas = await page('alice').ev(`(__callUI.render(), document.querySelector('#call-dock').innerHTML.includes('id="c-ring"'))`);
    check('RG2 the dock has a ring button inside a voice room call, and a text room has no Call button', dockHas && await page('alice').ev(`!document.getElementById('btn-call')`));
    await page('alice').ev(`__megaRing.ringAll()`);
    const seen = {};
    for (const n of ['bob', 'dave']) seen[n] = await until(async () => { const c = await cards(n); return c.length === 1 ? c : null; }, 20000, n + ' sees the ring').catch(() => null);
    check('RG3 everyone in the community who is not in the call sees who is calling and from which room', ['bob', 'dave'].every(n => seen[n] && /alice/i.test(seen[n][0]) && /lounge/.test(seen[n][0])), J(seen));
    check('RG4 the caller is not rung', (await cards('alice')).length === 0);
    // joining from the card is the normal join
    await page('bob').ev(`document.querySelector('#mega-ring .rg-b.yes').click()`);
    await until(() => call('bob', `c.phase === 'connected' && c.gid === ${J(vg)}`), 30000, 'bob joined from the ring');
    check('RG5 join on the card joins the voice room through the normal encrypted join', await call('bob', `c.room.isE2EEEnabled`) && (await cards('bob')).length === 0);
    await page('dave').ev(`document.querySelector('#mega-ring .rg-b.no').click()`);
    await sleep(500);
    check('RG6 decline closes the card', (await cards('dave')).length === 0);
    check('RG6b a member removed earlier (carol) is not rung and sees no card', (await cards('carol')).length === 0);
    // one person from their row: only they are rung
    const daveAcct = await page('dave').ev(`S.snap.me.account`);
    await page('alice').ev(`__megaRing.ringOne(${J(daveAcct)}, 'Dave')`);
    const one = await until(async () => { const c = await cards('dave'); return c.length === 1 ? c : null; }, 20000, 'dave is rung directly').catch(() => null);
    await sleep(1500);
    check('RG7 ringing one person rings only them (bob, in the call, is not rung), and says it is for them', one && /calling you/.test(one[0]) && (await cards('bob')).length === 0, J({ one, bob: await cards('bob') }));
    // a ring that is taken back, and one that is not answered, end
    await page('alice').ev(`__megaRing.ringOne(${J(daveAcct)}, 'Dave')`);   // second press stops it
    await until(async () => (await cards('dave')).length === 0, 20000, 'dave\'s ring is taken back');
    check('RG8 the caller taking a ring back ends it', true);
    await page('dave').ev(`rpc('ring_send', { gid: ${J(vg)}, to: '' })`).catch(() => { });   // dave is not in the call; the room is still ringable by a member
    const aliceSees = await until(async () => { const c = await cards('alice'); return c.length === 0 ? null : c; }, 3000).catch(() => null);
    check('RG9 a member who is already in the call is not rung for it', !aliceSees, J(aliceSees));
    for (const n of ['alice', 'bob']) await call(n, `c.leave()`).catch(() => { });
    await sleep(1500);
  }

  // ---------------------------------------------------------------- DC. calling one person directly: a hidden two-person community, rung and joined through the normal paths
  {
    for (const n of ['alice', 'bob', 'dave']) { await call(n, `c.phase === 'idle' ? true : c.leave()`).catch(() => { }); await page(n).ev(`(pref.set('sound', 'off'), true)`); }
    await sleep(1500);
    const cards = n => page(n).ev(`[...document.querySelectorAll('#mega-ring .rg-card')].map(c => c.innerText.replace(/\\s+/g, ' ').trim())`);
    const bobAcct = P('bob').account;
    const commsBefore = await page('alice').ev(`S.snap.communities.length`);
    // alice calls bob from the code behind the phone on his row in the People list
    check('DC0 the People list has a call button on other people\'s rows', await page('alice').ev(`(document.getElementById('btn-people').click(), 0)`) === 0 && await until(() => page('alice').ev(`document.querySelectorAll('#people .dr-row').length >= 2`), 15000, 'dr-row').then(() => true, () => false));
    await page('alice').ev(`(document.getElementById('btn-people').click(), 0)`);
    const started = await page('alice').ev(`__megaDirect.call(${J(bobAcct)}, 'Bob')`);
    const dr = await until(() => page('alice').ev(`S.snap.direct.length === 1 && S.snap.direct[0].room`), 40000, 'alice has the direct room').catch(() => null);
    check('DC1 alice has a direct community with one voice room, named after bob', !!started && !!dr && await page('alice').ev(`S.snap.direct[0].name === 'Bob' && S.snap.direct[0].channels.length === 1 && S.snap.direct[0].channels[0].kind === 'voice'`));
    await until(() => call('alice', `c.phase === 'connected' && c.gid === ${J(dr)}`), 40000, 'alice in the direct call').catch(() => { });
    check('DC2 the caller is in the call in that room, which the call strip names after bob', await call('alice', `c.phase === 'connected' && c.gid === ${J(dr)} && c.roomName === 'Bob'`), await call('alice', `JSON.stringify({ phase: c.phase, gid: c.gid, name: c.roomName })`).catch(() => ''));
    const seen = await until(async () => { const c = await cards('bob'); return c.length === 1 ? c : null; }, 60000, 'bob sees the ring').catch(() => null);
    check('DC3 bob is rung: the normal ring card, from alice, for him, saying direct call and not the community', !!seen && /alice/i.test(seen[0]) && /calling you/.test(seen[0]) && /direct call/.test(seen[0]) && !/Test club/.test(seen[0]), J(seen));
    check('DC4 nobody else is rung or sees anything: dave has no card, no direct room and no join in progress', (await cards('dave')).length === 0 && await page('dave').ev(`(S.snap.direct || []).length === 0 && !JSON.stringify(S.snap).includes(${J(dr)})`), J(await cards('dave')));
    await page('bob').ev(`document.querySelector('#mega-ring .rg-b.yes').click()`);
    await until(() => call('bob', `c.phase === 'connected' && c.gid === ${J(dr)}`), 40000, 'bob joined from the ring').catch(() => { });
    check('DC5 bob joins from the card through the normal join, and the call is named after alice', await call('bob', `c.phase === 'connected' && c.gid === ${J(dr)} && c.roomName === 'Alice'`) && (await cards('bob')).length === 0);
    // bob joining moves the room's key epoch on; alice takes the new epoch at her next sync, a moment later
    await until(async () => (await call('alice', `c.snapshot().encryption.epoch`)) === (await call('bob', `c.snapshot().encryption.epoch`)), 15000, 'both sides on the same key epoch').catch(() => { });
    const ea2 = await call('alice', `c.snapshot().encryption`), eb2 = await call('bob', `c.snapshot().encryption`);
    check('DC6 the call uses end-to-end frame encryption on both sides, with the same key epoch', (await call('alice', `c.room.isE2EEEnabled`)) && (await call('bob', `c.room.isE2EEEnabled`)) && ea2.active && eb2.active && ea2.epoch === eb2.epoch, J({ ea2, eb2 }));
    check('DC7 bob hears alice, through the encryption', await hears('bob', 'alice'));
    check('DC8 alice hears bob, through the encryption', await hears('alice', 'bob'));
    const droom = (await rpc(P('alice').port, 'call_token', { gid: dr })).room;
    const dparts = (await lkAdmin('ListParticipants', { room: droom })).participants || [];
    check('DC9 at the media server: two bare device ids, no names, a room name that is not the group id, every track marked encrypted', dparts.length === 2 && dparts.every(p => /^[0-9a-f]{64}$/.test(p.identity) && !p.name && (p.tracks || []).length > 0 && p.tracks.every(t => t.encryption === 'GCM' || t.encryption === 1)) && droom !== dr && !/Alice|Bob|direct/i.test(droom), J({ droom: droom.slice(0, 12), parts: dparts.map(p => [p.identity.slice(0, 8), p.name, (p.tracks || []).map(t => t.encryption)]) }));
    check('DC10 it is in no list: not among the communities (alice, bob, dave), not in the rooms, not in the page text', await Promise.all(['alice', 'bob', 'dave'].map(n => page(n).ev(`S.snap.communities.length === ${commsBefore} && !JSON.stringify(S.snap.communities).includes(${J(dr)}) && ![...document.querySelectorAll('#rooms .room')].some(r => r.dataset.g === ${J(dr)}) && !/Direct call [(]open in a newer/i.test(document.body.innerText)`))).then(a => a.every(Boolean)));
    // the same two call again: found, not made again, and rung straight away
    for (const n of ['alice', 'bob']) await call(n, `c.leave()`).catch(() => { });
    await sleep(1500);
    await page('bob').ev(`__megaDirect.call(${J(P('alice').account)}, 'Alice')`);
    const again = await until(async () => { const c = await cards('alice'); return c.length === 1 ? c : null; }, 60000, 'alice is rung by bob').catch(() => null);
    check('DC11 calling again finds the same community (still one) and rings the other person at once', !!again && /bob/i.test(again[0]) && /direct call/.test(again[0]) && await page('alice').ev(`S.snap.direct.length === 1 && S.snap.direct[0].room === ${J(dr)}`), J(again));
    await page('alice').ev(`document.querySelector('#mega-ring .rg-b.no')?.click()`).catch(() => { });
    for (const n of ['alice', 'bob']) await call(n, `c.phase === 'idle' ? true : c.leave()`).catch(() => { });
    await page('alice').ev(`(document.getElementById('people').hidden || document.getElementById('btn-people').click(), 0)`).catch(() => { });
    await sleep(1500);
  }

  // ---------------------------------------------------------------- R. voice reliability: joining, healing, nothing left behind
  for (const n of ['alice', 'bob', 'dave']) await call(n, `c.phase === 'idle' ? true : c.leave()`).catch(() => { });
  await sleep(1500);
  const inCall = async n => until(() => call(n, `c.phase === 'connected'`), 30000, n + ' connected');
  const countTokens = n => call(n, `(() => { const o = c.__rpc0 = c.__rpc0 || c.rpc; c.__tok = 0; c.rpc = async (m, a) => { if (m === 'call_token') c.__tok++; return o.call(c, m, a); }; return true; })()`);
  // R1: the others are heard while our own microphone is still being set up (the delay after joining)
  await call('alice', `c.join(S.gid, 'general')`); await inCall('alice');
  await call('bob', `(() => { c._publishMic = (orig => async on => { await new Promise(r => setTimeout(r, 2500)); return orig(on); })(c._publishMic.bind(c)); return true; })()`);
  const r1 = await call('bob', `(async () => { const t0 = Date.now(); const joined = c.join(S.gid, 'general'); let at = null; while (Date.now() - t0 < 2300) { if ([...c.tracks.values()].some(x => x.identity === ${J(dev('alice'))} && x.source === 'microphone')) { at = Date.now() - t0; break; } await new Promise(r => setTimeout(r, 20)); } await joined; delete c._publishMic; return { at }; })()`);
  check('R1 others are subscribed before our own microphone has finished publishing', r1.at !== null && r1.at < 2300, J(r1));
  check('R2 bob hears alice after that join', await hears('bob', 'alice'));
  check('R3 alice hears bob once his microphone is up', await hears('alice', 'bob'));
  await call('bob', `c.leave()`); await call('alice', `c.leave()`); await sleep(1200);
  // R4: a join that starts while the server is down is retried without the user, reusing one token
  await countTokens('bob');
  await lk.stopLivekit(); await sleep(400);
  await page('bob').ev(`(window.__j = window.__commonsCall.join(S.gid, 'general').then(() => 'ok', e => 'err ' + e.message), true)`);
  await sleep(2600);
  const midPhase = await call('bob', `c.phase`);
  lk.startLivekit();
  const joinRes = await page('bob').ev(`Promise.race([window.__j, new Promise(r => setTimeout(() => r('timeout'), 25000))])`);
  const r4 = await call('bob', `({ phase: c.phase, tok: c.__tok, attempt: c.attempt, err: c.error, ev: c.events.filter(e => /could not connect/.test(e.msg)).length })`);
  check('R4 joining while the server restarts: retried, connected, without the user', joinRes === 'ok' && r4.phase === 'connected' && r4.ev > 0, J({ joinRes, midPhase, r4 }));
  check('R5 those retries used a single call token', r4.tok === 1, J(r4));
  await call('alice', `c.join(S.gid, 'general')`); await inCall('alice');
  await until(async () => (await call('bob', `c.room && c.room.remoteParticipants.size`)) === 1 && (await call('alice', `c.room && c.room.remoteParticipants.size`)) === 1, 20000, 'R: both in').catch(() => { });
  check('R6 after that join both hear each other', await hears('bob', 'alice') && await hears('alice', 'bob'));
  // R7: a short signal drop is resumed in place: same room object, no rejoin
  await call('bob', `(window.__r0 = c.room, window.__rj0 = c.events.filter(e => /rejoined automatically/.test(e.msg)).length, true)`);
  await call('bob', `c.room.simulateScenario('signal-reconnect').then(() => true)`); await sleep(3000);
  const r7 = await call('bob', `({ same: c.room === window.__r0, phase: c.phase, rejoined: c.events.filter(e => /rejoined automatically/.test(e.msg)).length > window.__rj0 })`);
  check('R7 a short signal drop heals in place, no full rejoin', r7.same && r7.phase === 'connected' && !r7.rejoined && await hears('bob', 'alice'), J(r7));
  // Stuck media path: the SDK thinks it is still resuming, the media path is down, the call rebuilds itself after ~10 s of polls
  await call('bob', `(() => { window.__gcs = c.room.engine.pcManager.publisher.getConnectionState; c.room.engine.pcManager.publisher.getConnectionState = () => 'failed'; window.__rejoined0 = c.events.filter(e => /rebuilding the call/.test(e.msg)).length; return true; })()`);
  const r10 = await until(async () => (await call('bob', `c.events.filter(e => /rebuilding the call/.test(e.msg)).length`)) > 0 && (await call('bob', `c.phase`)) === 'connected', 25000, 'R10 rebuilt').catch(() => null);
  await until(() => call('bob', `c.room && c.room.isE2EEEnabled`), 10000, 'R10 e2ee').catch(() => { });
  check('R10 a media path that stays down rebuilds the call by itself, encrypted', r10 && await call('bob', `c.room.isE2EEEnabled`) && await hears('bob', 'alice'), J(r10));
  // R8: leaving while a rejoin is in progress must not leave a call behind; R9: the rejoin retries reuse the token
  await countTokens('alice');
  await lk.stopLivekit();
  await until(() => call('bob', `c.phase === 'reconnecting'`), 12000, 'bob reconnecting').catch(() => { });
  await sleep(1500);
  await call('bob', `c.leave()`);
  await sleep(4000);
  lk.startLivekit();
  await until(() => call('alice', `c.phase === 'connected' && !!c.room`), 45000, 'R8: alice rejoined');
  await sleep(4000);
  const r8 = await call('bob', `({ phase: c.phase, room: !!c.room })`);
  const present = ((await lkAdmin('ListParticipants', { room: roomName })).participants || []).map(p => p.identity.slice(0, 8));
  check('R8 leaving during a rejoin stays left: no call appears afterwards', r8.phase === 'idle' && !r8.room && !present.includes(dev('bob').slice(0, 8)), J({ r8, present }));
  const aTok = await call('alice', `c.__tok`);
  check('R9 an outage of several seconds costs at most two call tokens (the relay limits them)', aTok <= 2, String(aTok));
  await call('alice', `c.leave()`);
} catch (e) { if (e.only) console.log(e.message); else { console.error('TEST RUN ABORTED:', e); failed++; results.push({ name: 'run', ok: false, detail: String(e.stack || e) }); } }
finally {
  fs.writeFileSync(path.join(out, 'results.json'), JSON.stringify(results, null, 1));
  if (world) for (const n of Object.keys(world.people)) { const p = world.people[n]; if (p.page && p.page.console.length) fs.writeFileSync(path.join(out, `console-${n}.txt`), p.page.console.join('\n')); }
  stopAll();
  console.log(`\n${results.filter(r => r.ok).length} passed, ${failed} failed. Output in ${out}`);
  process.exitCode = failed ? 1 : 0;
}
