// Encryption interop between the native app's call stack and the web client (CALLS-PLAN.md step 0).
// One local LiveKit room: Alice (and later Carol) are the real app/ui call.js in headless Chrome for Testing; Bob is
// app/gpui/examples/call_probe.rs, a Rust LiveKit participant with the app's own key and ICE rules, fed Bob's real
// call_state and call_token from his bridge. Silent: fake devices, muted Chromium, no audio device in Rust.
//   cargo build -p mega-gpui --example call_probe
//   COMMONS_TEST_PORT_OFFSET=400 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-call-interop.mjs <outdir>
// If any of checks 1 to 4 fails after an SDK upgrade, do not upgrade.
import fs from 'node:fs';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { root, startServers, startBridge, rpc, sleep, stopAll, procs, LK_PORT, RELAY_PORT, port as P } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-call-interop');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const PROBE = process.env.GPUI_CALL_PROBE || path.join(root, 'target/debug/examples/call_probe');
const TONE_MIN = 0.15, TONE_MAX = 0.02, RUST_HZ = 900;
const J = x => JSON.stringify(x);
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? '  [' + (typeof detail === 'string' ? detail : J(detail)) + ']' : ''}`); };
async function until(fn, ms, what) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(250); } }
const near = (px, want, tol = 12) => Array.isArray(px) && want.every((w, i) => Math.abs(px[i] - w) <= tol);

// One Rust participant. Each answer is one line of JSON carrying its command's id.
class Probe {
  constructor(name) {
    this.name = name; this.waiting = new Map(); this.n = 0; this.buf = '';
    this.proc = spawn(PROBE, [], { stdio: ['pipe', 'pipe', 'pipe'], env: { ...process.env, RUST_LOG: process.env.RUST_LOG || 'warn' }, detached: true });
    procs.push(this.proc);
    const log = fs.createWriteStream(path.join(out, `probe-${name}.log`));
    this.proc.stderr.pipe(log);
    this.proc.stdout.on('data', d => { this.buf += d; let i; while ((i = this.buf.indexOf('\n')) >= 0) { const l = this.buf.slice(0, i); this.buf = this.buf.slice(i + 1); let id = 0; try { id = JSON.parse(l).id ?? 0; } catch { } const w = this.waiting.get(id); if (w) { this.waiting.delete(id); w(l); } } });
    this.exited = new Promise(r => this.proc.on('exit', code => { this.code = code; for (const w of this.waiting.values()) w(J({ error: 'probe exited ' + code })); this.waiting.clear(); r(code); }));
  }
  // the setup line is answered with no id (0); every command gets its own
  line(obj, ms = 60000) {
    const id = obj.cmd ? ++this.n : 0;
    return new Promise((res, rej) => {
      const t = setTimeout(() => { this.waiting.delete(id); rej(new Error(`${this.name}: no answer to ${obj.cmd || 'setup'}`)); }, ms);
      this.waiting.set(id, l => { clearTimeout(t); try { res(JSON.parse(l)); } catch { res({ error: 'bad line', raw: l.slice(0, 200) }); } });
      this.proc.stdin.write(J(obj.cmd ? { ...obj, id } : obj) + '\n');
    });
  }
  cmd(cmd, args = {}, ms) { return this.line({ cmd, ...args }, ms); }
  async leave() { try { await this.cmd('leave', {}, 8000); } catch { } setTimeout(() => { try { process.kill(-this.proc.pid, 'SIGKILL'); } catch { } }, 1500); await Promise.race([this.exited, sleep(3000)]); }
}

const res = { checks: results };
let lk, world, probe, pump;
try {
  lk = await startServers(out, {});
  world = await makeWorld(out, ['alice', 'bob']);
  await openApps(world, ['alice']);
  const A = world.people.alice, B = world.people.bob;
  const page = p => p.page;
  const call = (p, expr) => page(p).ev(`(async()=>{ const c = window.__commonsCall; return ${expr}; })()`);
  await call(A, `c.join(S.gid, 'general')`);
  await until(() => call(A, `c.phase === 'connected'`), 20000, 'alice connected');
  await call(A, `c.setCamera(true)`);
  const bobState = () => rpc(B.port, 'call_state', { gid: world.gid });
  const setupFor = async () => { const st = await bobState(), tok = await rpc(B.port, 'call_token', { gid: world.gid }); return { url: tok.url, token: tok.token, turn_port: tok.turn_port ?? null, state: st, tone_hz: RUST_HZ }; };

  // what the browser hears and sees of the Rust participant
  const bdev = (await bobState()).me;
  const hearRust = (p, ms = 2500) => call(p, `(async () => { for (let i = 0; i < 40 && ![...c.tracks.values()].some(x => x.identity === ${J(bdev)} && x.source === 'microphone'); i++) await new Promise(r => setTimeout(r, 250)); return c.probeTone(${J(bdev)}, ${RUST_HZ}, ${ms}); })()`);
  const seeRust = (p, ms = 5000, source = 'camera') => call(p, `(async () => { let t; for (let i = 0; i < 40 && !t; i++) { t = [...c.tracks.values()].find(x => x.identity === ${J(bdev)} && x.source === ${J(source)}); if (!t) await new Promise(r => setTimeout(r, 250)); } if (!t) return { present: false };
    const v = t.track.attach(); v.muted = true; await v.play().catch(() => {}); for (let i = 0; i < 40 && !(v.videoWidth > 0); i++) await new Promise(r => setTimeout(r, 100));
    const q0 = v.getVideoPlaybackQuality().totalVideoFrames, t0 = v.currentTime; await new Promise(r => setTimeout(r, ${ms}));
    const cv = document.createElement('canvas'); cv.width = v.videoWidth || 1; cv.height = v.videoHeight || 1; const g = cv.getContext('2d'); g.drawImage(v, 0, 0); const px = [...g.getImageData(cv.width >> 2, cv.height >> 1, 1, 1).data.slice(0, 3)];
    const o = { present: true, w: v.videoWidth, h: v.videoHeight, advanced: +(v.currentTime - t0).toFixed(2), frames: v.getVideoPlaybackQuality().totalVideoFrames - q0, px }; t.track.detach(v); return o; })()`);
  const browserSide = p => call(p, `({ fails: [...c.decryptFail].filter(([k]) => k === ${J(bdev)}).map(([k, v]) => v.n).reduce((a, b) => a + b, 0), enc: [...c.tracks.values()].filter(x => x.identity === ${J(bdev)}).map(x => x.source + ':' + x.pub.isEncrypted), epoch: c.snapshot().encryption.epoch, keyIndex: c.snapshot().encryption.keyIndex })`);
  const rustHears = async (ms = 2500) => { const r = await probe.cmd('measure', { ms, listen: { [A.device]: A.tone } }); const t = r.tracks || {}; return { audio: t[A.device + '/Microphone'] || { present: false }, video: t[A.device + '/Camera'] || { video_frames: 0 }, e2ee: r.e2ee }; };
  const both = async (ms = 2500) => { const [r, b] = await Promise.all([rustHears(ms), hearRust(A, ms)]); return { rust: r, browser: b, ok: r.audio.toneShare > TONE_MIN && b.present && b.toneShare > TONE_MIN }; };
  // Bob's core is asked every 1.5 s, as the app's engine does, and the probe gets every answer
  const startPump = () => { let busy = false; pump = setInterval(async () => { if (busy || !probe) return; busy = true; try { await probe.cmd('keys', { state: await bobState() }); } catch { } busy = false; }, 1500); };
  const stopPump = () => clearInterval(pump);

  // ---------------------------------------------------------------- controls: each must fail (check 4)
  for (const [name, controls] of [['wrong own key', { bad_key: true }], ['sending index left at 0', { index0: true }], ['PBKDF2 (the SDK default)', { pbkdf2: true }]]) {
    probe = new Probe(name.split(' ')[0]);
    const ready = await probe.line({ ...(await setupFor()), controls });
    const fails0 = (await browserSide(A)).fails;
    await sleep(2500);
    const m = await both(2500);
    const fails1 = (await browserSide(A)).fails;
    res['control ' + name] = { ready: { ...ready, ice_seen: undefined }, m, decryptFailures: fails1 - fails0 };
    const silentToBrowser = !m.browser.present || m.browser.toneShare < TONE_MAX;
    if (controls.pbkdf2) check(`4c control "${name}": neither side hears the other`, silentToBrowser && !(m.rust.audio.toneShare > TONE_MAX), { browser: m.browser.toneShare, rust: m.rust.audio.toneShare });
    else check(`4${controls.bad_key ? 'a' : 'b'} control "${name}": the browser hears nothing from Rust, Rust still hears the browser`, silentToBrowser && m.rust.audio.toneShare > TONE_MIN && (controls.index0 || fails1 > fails0), { browser: m.browser.toneShare ?? 'absent', rust: m.rust.audio.toneShare, decryptFailures: fails1 - fails0 });
    await probe.leave(); probe = null; await sleep(1500);
  }

  // ---------------------------------------------------------------- the good run (checks 1 to 3, and patches L1 and L2)
  probe = new Probe('main');
  const ready = await probe.line(await setupFor());
  res.ready = ready;
  const st0 = await bobState();
  startPump();
  const fails0 = (await browserSide(A)).fails;
  await sleep(3000);
  const good = await both(2500);
  check('1 audio both ways: the browser hears the Rust tone and Rust hears the browser\'s', good.ok, { browser: good.browser.toneShare, rust: good.rust.audio.toneShare });
  const [rv1, bv] = await Promise.all([rustHears(5000), seeRust(A, 5000)]);
  check('2a video Rust to browser: frames keep coming for 5 s, centre colour within 12 of rgb(32,192,64)', bv.present && bv.advanced > 4 && bv.frames > 30 && near(bv.px, [32, 192, 64]), bv);
  check('2b video browser to Rust: 1280x720 frames keep coming for 5 s', rv1.video.video_frames > 50 && rv1.video.w === 1280 && rv1.video.h === 720, rv1.video);
  const bs = await browserSide(A), rs = await probe.cmd('status');
  const rustE2ee = Object.values(rs.e2ee || {});
  check('3 no decrypt errors on either side; both see the other\'s tracks as encrypted', bs.fails === fails0 && bs.enc.length >= 2 && bs.enc.every(x => x.endsWith(':true')) && rustE2ee.length > 0 && rustE2ee.every(s => s === 'Ok') && rs.events.filter(e => e.startsWith('subscribed')).every(e => e.endsWith('enc=Gcm')),
    { browserFailures: bs.fails - fails0, browserTracks: bs.enc, rustStates: rs.e2ee, rustSubscribed: rs.events.filter(e => e.startsWith('subscribed')) });
  check('L2 the Rust sender cryptors were born on the epoch\'s key index (no set_key_index by hand)', ready.sender_index_at_birth.length === 2 && ready.sender_index_at_birth.every(i => i === ready.index), { born: ready.sender_index_at_birth, index: ready.index });
  // patch W1: volume 0 then 1 on Alice's microphone. libwebrtc applies the gain in the playout mix, so the tap (a
  // NativeAudioStream, fed before the gain) is expected to stay loud; the receiver's playout energy is what must move.
  const v0 = await probe.cmd('volume', { identity: A.device, v: 0, hz: A.tone }), v1 = await probe.cmd('volume', { identity: A.device, v: 1, hz: A.tone });
  res.volume = { v0, v1 };
  check('W1 per-track volume: at 0 the playout path carries no energy, at 1 it does', v0.ok && v1.ok && v0.playout_energy >= 0 && v0.playout_energy < 1e-4 && v1.playout_energy > 1e-3,
    { playoutEnergy: [v0.playout_energy, v1.playout_energy], tapToneShare: [v0.tap && v0.tap.toneShare, v1.tap && v1.tap.toneShare] });
  res.ice_seen = ready.ice_seen;
  check('L1 the ICE hook saw the join response\'s list (logged; local LiveKit sends no TURN)', Array.isArray(ready.ice_seen), J(ready.ice_seen));

  // ---------------------------------------------------------------- 5. rotation: Carol joins the room mid-call
  const oldIdx = st0.epoch % 16, oldKeys = st0.keys;
  const C = { name: 'carol', port: P(27202), tone: 700, chrome: world.chrome };
  world.people.carol = C;
  startBridge(out, 'carol', C.port); await sleep(1200);
  await rpc(C.port, 'create_account', { name: 'Carol', passphrase: 'correct horse battery', relay: `http://127.0.0.1:${RELAY_PORT}` });
  const link = (await rpc(A.port, 'create_invite', { cid: world.cid, uses: 2, ttl: 3600 })).link;
  await rpc(C.port, 'join', { link });
  await until(async () => (await bobState()).epoch > st0.epoch && (await browserSide(A)).epoch > st0.epoch, 60000, 'epoch moves after carol joined');
  const tRot = Date.now();
  const st1 = await bobState(), newIdx = st1.epoch % 16;
  let rot = null;
  for (let i = 0; i < 6 && !(rot && rot.ok); i++) rot = await both(1500);
  const rotAt = (Date.now() - tRot) / 1000;
  const rs1 = await probe.cmd('status'), bs1 = await browserSide(A);
  check('5a rotation: within about 5 s both directions pass again on the new key index', rot.ok && rotAt <= 5 + 1.5 && rs1.sender_index.every(i => i === newIdx) && bs1.keyIndex === newIdx && newIdx !== oldIdx,
    { epoch: [st0.epoch, st1.epoch], index: [oldIdx, newIdx], seconds: rotAt, rustSenders: rs1.sender_index, browserIndex: bs1.keyIndex, browser: rot.browser.toneShare, rust: rot.rust.audio.toneShare });
  await until(async () => (await probe.cmd('status')).wipes >= 1, 8000, 'old slot wiped').catch(() => { });
  const holds = {}; for (const [d, k] of Object.entries(oldKeys)) holds[d.slice(0, 8)] = (await probe.cmd('holds', { identity: d, index: oldIdx, key: k })).holds;
  const holdsNew = (await probe.cmd('holds', { identity: A.device, index: newIdx, key: st1.keys[A.device] })).holds;
  check('5b after the 3 s grace the provider no longer holds any old key in the old slot, and holds the new ones', Object.values(holds).every(h => h === false) && holdsNew, { oldSlotStillHolds: holds, newKeyHeld: holdsNew });

  // ---------------------------------------------------------------- 6. removal: Carol in the call, then kicked; her client ignores it
  C.account = (await rpc(C.port, 'snapshot')).me.account; C.device = (await rpc(C.port, 'snapshot')).me.device;
  await openApps(world, ['carol']);
  await call(C, `c.join(S.gid, 'general')`);
  await until(() => call(C, `c.phase === 'connected'`), 20000, 'carol connected');
  // a modified client: it keeps its last keys after the removal, and plays everyone it is sent
  await call(C, `(() => { const orig = c.rpc; let cache = null; c.rpc = async (m, a) => { try { const r = await orig(m, a); if (m === 'call_state') cache = r; return r; } catch (e) { if (m === 'call_state' && cache) return cache; throw e; } }; c._testTrustAll = true; return true; })()`);
  await sleep(2500);
  let pre = null; for (let i = 0; i < 4 && !(pre && pre.toneShare > TONE_MIN); i++) pre = await hearRust(C, 1500);
  const eKick = (await bobState()).epoch;
  await rpc(A.port, 'kick', { cid: world.cid, account: C.account });
  await until(async () => (await bobState()).epoch > eKick, 60000, 'epoch moves after the kick');
  await until(async () => (await probe.cmd('status')).epoch > eKick, 10000, 'probe has the post-kick keys');
  await sleep(5500);   // the old slot is wiped after 3 s
  let after = null; for (let i = 0; i < 3; i++) { const r = await hearRust(C, 1500); if (!after || (r.present && r.toneShare > (after.toneShare || 0))) after = r; }
  let still = null; for (let i = 0; i < 4 && !(still && still.toneShare > TONE_MIN); i++) still = await hearRust(A, 1500);
  check('6 removal: the kicked member, keeping her old keys and trusting everyone, hears nothing from Rust (and heard it before)', pre && pre.toneShare > TONE_MIN && after && (!after.present || after.toneShare < TONE_MAX) && still.toneShare > TONE_MIN,
    { before: pre && pre.toneShare, after: after && (after.present ? after.toneShare : 'absent'), aliceStillHears: still.toneShare, carolEpoch: await call(C, `c.snapshot().encryption.epoch`).catch(() => null) });
  await call(C, `c.leave()`).catch(() => { });

  // ---------------------------------------------------------------- 7. simulcast from Rust: a screen-share-shaped track
  const sh = await probe.cmd('share', { w: 2560, h: 1440 });
  const sizeAt = async q => call(A, `(async () => { let t; for (let i = 0; i < 60 && !t; i++) { t = [...c.tracks.values()].find(x => x.identity === ${J(bdev)} && x.source === 'screen_share'); if (!t) await new Promise(r => setTimeout(r, 250)); } if (!t) return { present: false };
    c.setViewQuality(t.pub.trackSid, ${J(q)}); const v = t.track.attach(); v.muted = true; await v.play().catch(() => {});
    const want = ${q === 'low' ? 540 : 1440}; let seen = []; const t0 = Date.now(); while (Date.now() - t0 < 20000) { await new Promise(r => setTimeout(r, 500)); seen.push(v.videoHeight); if (v.videoHeight === want && seen.slice(-3).every(h => h === want)) break; }
    const o = { present: true, w: v.videoWidth, h: v.videoHeight, after: (Date.now() - t0) / 1000, enc: t.pub.isEncrypted }; t.track.detach(v); return o; })()`);
  const hi = await sizeAt('high'), lo = await sizeAt('low'), hi2 = await sizeAt('high');
  check('7 simulcast from Rust: the browser plays the share at 2560x1440 on HIGH and 960x540 on LOW, encrypted', sh.ok && sh.encrypted && hi.w === 2560 && hi.h === 1440 && lo.w === 960 && lo.h === 540 && hi2.h === 1440 && hi.enc, { share: sh, high: hi, low: lo, highAgain: hi2 });
  await probe.cmd('unshare');

  // ---------------------------------------------------------------- 8. the media server restarts
  await lk.stopLivekit();
  await sleep(2500);
  lk.startLivekit();
  const tBack = Date.now();
  let back = null;
  for (let i = 0; i < 20 && !(back && back.ok); i++) { back = await both(1500).catch(e => ({ ok: false, error: String(e) })); }
  const backAt = (Date.now() - tBack) / 1000;
  const rs8 = await probe.cmd('status');
  check('8 LiveKit restart: both directions pass again within 10 s of the server being back', back.ok && backAt <= 10 + 1.5, { seconds: backAt, browser: back.browser && back.browser.toneShare, rust: back.rust && back.rust.audio.toneShare, rustEvents: rs8.events.filter(e => !e.startsWith('subscribed')).slice(-8), republishedIndex: rs8.republished_index, senders: rs8.sender_index, index: rs8.index });
  res.restart = { seconds: backAt, rust: rs8 };
  stopPump();
  await probe.leave();
} catch (e) { res.error = String(e.stack || e); console.log('ERROR', res.error); failed++; }
res.failed = failed;
fs.writeFileSync(path.join(out, 'result.json'), JSON.stringify(res, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED', '->', path.join(out, 'result.json'));
stopAll(); process.exit(failed ? 1 : 0);
