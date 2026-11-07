// The mic's two voices, without a browser: node --test tools/mic-gate.test.mjs
import test from 'node:test'; import assert from 'node:assert/strict'; import fs from 'node:fs';
(0, eval)(fs.readFileSync(new URL('../app/ui/mic-gate.js', import.meta.url), 'utf8'));
const G = globalThis.CommonsMicGate;
const gate = async (ms = 0) => { const sent = []; const g = new G(async on => { sent.push(on); if (ms) await new Promise(r => setTimeout(r, ms)); }); await g.open(); return { g, sent }; };
const micOn = sent => sent[sent.length - 1];

test('a game key mutes, and letting go comes back', async () => {
  const { g, sent } = await gate();
  await g.setGame(true); assert.equal(micOn(sent), false);
  await g.setGame(false); assert.equal(micOn(sent), true);
});
test('a manual mute outlives a game key', async () => {
  const { g, sent } = await gate();
  await g.setManual(true); await g.setGame(true); await g.setGame(false);
  assert.equal(micOn(sent), false, 'still muted: the person said so');
});
test('muting by hand during a game key stays after it', async () => {
  const { g, sent } = await gate();
  await g.setGame(true); await g.setManual(true); await g.setGame(false);
  assert.equal(micOn(sent), false);
});
test('unmuting by hand while a key is held keeps the mic off until it lets go', async () => {
  const { g, sent } = await gate();
  await g.setManual(true); await g.setGame(true); await g.setManual(false);
  assert.equal(micOn(sent), false);
  await g.setGame(false); assert.equal(micOn(sent), true);
});
test('losing the helper goes back to what the person had set', async () => {
  for (const manual of [false, true]) {
    const { g, sent } = await gate();
    await g.setManual(manual); await g.setGame(true); await g.lost();
    assert.equal(micOn(sent), !manual);
  }
});
test('a fast press and release ends on the last wish, in order', async () => {
  const { g, sent } = await gate(5);
  g.setGame(true); g.setGame(false); g.setGame(true); await g.setGame(false);
  await g.flush();
  assert.equal(micOn(sent), true); assert.equal(g.off, false);
  assert.ok(sent.length <= 4);
});
test('no publishing before the mic exists or after leaving', async () => {
  const sent = []; const g = new G(async on => sent.push(on));
  await g.setGame(true); assert.deepEqual(sent, []);
  await g.open(); assert.deepEqual(sent, [false], 'joins muted if the key is down');
  g.close(); await g.setManual(false); assert.deepEqual(sent, [false]);
});
