// The phone gestures' arithmetic, without a browser: node --test tools/mega-touch.test.mjs
import test from 'node:test'; import assert from 'node:assert/strict'; import fs from 'node:fs';
(0, eval)(fs.readFileSync(new URL('../app/ui/mega-touch.js', import.meta.url), 'utf8'));
const T = globalThis.MegaTouch;
const close = (a, b, eps = 1e-6) => assert.ok(Math.abs(a - b) < eps, `${a} ~ ${b}`);

test('direction: nothing under 10 px, sideways only when clearly sideways', () => {
  assert.equal(T.lock(6, 4), null); assert.equal(T.lock(9, 9), null);
  assert.equal(T.lock(12, 2), 'h'); assert.equal(T.lock(-30, 10), 'h');
  assert.equal(T.lock(12, 11), 'v', 'a diagonal is the scroll');
  assert.equal(T.lock(3, 14), 'v'); assert.equal(T.lock(0, -40), 'v');
  assert.equal(T.lock(14, 10), 'h'); assert.equal(T.lock(13, 10), 'v');
});
test('velocity: over the last 80 ms, px/ms, zero with one point', () => {
  assert.equal(T.velocity([], 100), 0); assert.equal(T.velocity([{ t: 0, x: 0 }], 100), 0);
  close(T.velocity([{ t: 0, x: 0 }, { t: 50, x: 100 }], 50), 2);
  // an old slow start does not count: only the fast finish does
  const s = [{ t: 0, x: 0 }, { t: 300, x: 10 }, { t: 340, x: 50 }, { t: 380, x: 90 }];
  close(T.velocity(s, 380), 1);
  assert.ok(T.velocity([{ t: 0, x: 100 }, { t: 40, x: 20 }], 40) < 0, 'leftwards is negative');
});
test('a page swipe commits past a third of the width, or on a flick', () => {
  assert.equal(T.decide(100, 0, 390), false);
  assert.equal(T.decide(137, 0, 390), true);
  assert.equal(T.decide(50, .6, 390), true, 'a fast short flick');
  assert.equal(T.decide(20, 3, 390), false, 'but not a twitch');
  assert.equal(T.decide(50, -.9, 390), false, 'flicking back cancels');
});
test('the rubber band: full travel to the limit, then a third', () => {
  assert.equal(T.damp(30, 60), 30); assert.equal(T.damp(60, 60), 60); close(T.damp(160, 60), 90);
  assert.ok(T.damp(400, 40) < 160);
});
test('reply: the row follows to the mark, past it a third; the mark or a flick replies', () => {
  assert.equal(T.replyX(30), 30); assert.equal(T.replyX(56), 56); close(T.replyX(156), 56 + 35);
  assert.equal(T.replyDecide(55, 0), false); assert.equal(T.replyDecide(56, 0), true);
  assert.equal(T.replyDecide(30, .8), true); assert.equal(T.replyDecide(20, .8), false);
  assert.equal(T.handover(150, 390), false); assert.equal(T.handover(156, 390), true, 'two fifths across hands over to the page');
});
test('two taps: same row, within 320 ms and 30 px', () => {
  const a = { t: 1000, x: 100, y: 200, id: 'm1' };
  assert.equal(T.doubleTap(a, 1200, 110, 205, 'm1'), true);
  assert.equal(T.doubleTap(a, 1400, 100, 200, 'm1'), false, 'too late');
  assert.equal(T.doubleTap(a, 1200, 100, 200, 'm2'), false, 'another row');
  assert.equal(T.doubleTap(a, 1200, 140, 200, 'm1'), false, 'too far');
  assert.equal(T.doubleTap(null, 1200, 100, 200, 'm1'), false);
});
test('pinch: the point between the fingers stays put, scale clamps to 1..5', () => {
  // fingers 100 px apart around the picture's centre (200, 400), spread to 200 px: scale 2, no shift
  let z = T.pinch({ x: 150, y: 400 }, { x: 250, y: 400 }, { x: 100, y: 400 }, { x: 300, y: 400 }, 1, 0, 0, 200, 400);
  close(z.s, 2); close(z.tx, 0); close(z.ty, 0);
  // fingers off centre: the picture point under their middle (u = 100 right of centre) stays under the new middle
  z = T.pinch({ x: 250, y: 400 }, { x: 350, y: 400 }, { x: 200, y: 400 }, { x: 400, y: 400 }, 1, 0, 0, 200, 400);
  close(z.s, 2); close(z.tx, 300 - 200 - 100 * 2); close(z.ty, 0);
  z = T.pinch({ x: 100, y: 0 }, { x: 300, y: 0 }, { x: 190, y: 0 }, { x: 210, y: 0 }, 1, 0, 0, 200, 400);
  assert.equal(z.s, 1, 'never below 1');
  z = T.pinch({ x: 190, y: 0 }, { x: 210, y: 0 }, { x: 0, y: 0 }, { x: 390, y: 0 }, 1, 0, 0, 200, 400);
  assert.equal(z.s, 5, 'never above 5');
  // zooming in to 2.5 about a tap keeps the tapped point under the finger
  const q = T.zoomAt(300, 500, 1, 0, 0, 2.5, 200, 400); close(q.s, 2.5); close(q.tx, 100 - 100 * 2.5); close(q.ty, 100 - 100 * 2.5);
});
test('panning a zoomed picture never shows past its edges; a small picture stays centred', () => {
  let c = T.clampPan(500, -900, 2, 390, 300, 390, 844);
  close(c.tx, 195); close(c.ty, 0, 1e-6);   // 300 * 2 = 600 tall fits in 844: no vertical pan
  c = T.clampPan(0, 0, 1, 200, 100, 390, 844); close(c.tx, 0); close(c.ty, 0);
  c = T.clampPan(-5000, 5000, 3, 390, 844, 390, 844); close(c.tx, -390); close(c.ty, 844);
});
test('the slide to the edge takes between 120 and 260 ms', () => {
  assert.equal(T.slideMs(300, 0), 260); assert.equal(T.slideMs(300, 10), 120); close(T.slideMs(300, 1.5), 200);
});
