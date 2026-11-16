// The native call engine against the web client (CALLS-PLAN.md step 2). One local LiveKit room: Alice and Bob are the
// real app/ui call.js in headless Chrome for Testing; Nat is `mega-gpui --call-headless`, the app's own engine with the
// core linked in, driven over its test port (calls/testhook.rs). Silent: fake devices, muted Chromium, and Nat opens no
// audio device (MEGA_GPUI_CALL_AUDIO=none: its microphone is a 900 Hz sine and hearing is measured on taps).
//   cargo build -p mega-gpui
//   COMMONS_TEST_PORT_OFFSET=400 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-call-e2e.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn } from 'node:child_process';
import { root, startServers, startBridge, openPage, rpc, sleep, stopAll, procs, operatorToken, LK_PORT, RELAY_PORT, port as P } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-call-e2e');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const UMD = fs.readFileSync(path.join(root, 'app/ui/vendor/livekit-client.umd.js'), 'utf8');
const TONE_MIN = 0.15, TONE_MAX = 0.02, NAT_HZ = 900, PASS = 'correct horse battery';
const J = x => JSON.stringify(x);
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? '  [' + (typeof detail === 'string' ? detail : J(detail)) + ']' : ''}`); };
async function until(fn, ms, what) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(250); } }
const soft = p => p.catch(e => ({ timeout: String(e.message || e) }));

// The native engine's test port: one JSON line per command, answered with the same id.
class Native {
  constructor(env) {
    this.proc = spawn(BIN, ['--call-headless'], { env: { ...process.env, ...env }, stdio: ['ignore', 'pipe', 'pipe'], detached: true });
    procs.push(this.proc);
    this.proc.stderr.pipe(fs.createWriteStream(path.join(out, 'nat.log')));
    this.ready = new Promise((res, rej) => { let b = ''; this.proc.stdout.on('data', d => { b += d; const i = b.indexOf('\n'); if (i >= 0) { try { res(JSON.parse(b.slice(0, i))); } catch (e) { rej(e); } } }); this.proc.on('exit', c => rej(new Error('mega-gpui exited ' + c))); });
    this.n = 0; this.waiting = new Map(); this.buf = '';
  }
  async connect(port) {
    this.sock = net.connect(port, '127.0.0.1');
    await new Promise((res, rej) => { this.sock.once('connect', res); this.sock.once('error', rej); });
    this.sock.on('data', d => { this.buf += d; let i; while ((i = this.buf.indexOf('\n')) >= 0) { const l = this.buf.slice(0, i); this.buf = this.buf.slice(i + 1); const j = JSON.parse(l); const w = this.waiting.get(j.id); if (w) { this.waiting.delete(j.id); w(j); } } });
  }
  cmd(cmd, args = {}, ms = 30000) {
    const id = ++this.n;
    return new Promise((res, rej) => { const t = setTimeout(() => { this.waiting.delete(id); rej(new Error('no answer to ' + cmd)); }, ms); this.waiting.set(id, j => { clearTimeout(t); res(j); }); this.sock.write(J({ cmd, id, ...args }) + '\n'); });
  }
  snap() { return this.cmd('snapshot'); }
}

const res = { checks: results };
let lk, world, nat;
try {
  lk = await startServers(out, {});
  world = await makeWorld(out, ['alice', 'bob', 'carol']);
  await openApps(world, ['alice', 'bob']);
  const A = world.people.alice, B = world.people.bob, C = world.people.carol;
  const call = (p, expr) => p.page.ev(`(async()=>{ const c = window.__commonsCall; return ${expr}; })()`);
  const gid = world.gid;

  // Nat's account is made by a bridge, which then stops so the native app can open the same folder
  const N = { name: 'nat', port: P(27203) };
  const natBridge = startBridge(out, 'nat', N.port); await sleep(1200);
  await rpc(N.port, 'create_account', { name: 'Nat', passphrase: PASS, relay: `http://127.0.0.1:${RELAY_PORT}` });
  await rpc(N.port, 'join', { link: (await rpc(A.port, 'create_invite', { cid: world.cid, uses: 4, ttl: 3600 })).link });
  await until(async () => { const s = await rpc(N.port, 'snapshot'); const c = s.communities.find(c => c.id === world.cid); return c && c.channels.some(ch => ch.gid === gid && ch.joined && ch.status === 'ok'); }, 60000, 'nat joined');
  const ns = await rpc(N.port, 'snapshot'); N.account = ns.me.account; N.device = ns.me.device;
  try { process.kill(-natBridge.pid, 'SIGTERM'); } catch { } await sleep(1500);
  const testPort = P(27300);
  nat = new Native({ COMMONS_DATA: path.join(out, 'data/nat'), COMMONS_RELAY: `http://127.0.0.1:${RELAY_PORT}`, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_CALL_TONE: String(NAT_HZ), MEGA_GPUI_CALL_TEST: String(testPort), MEGA_GPUI_NOTIFY_FILE: path.join(out, 'nat-notes.jsonl'), RUST_LOG: process.env.RUST_LOG || 'info' });
  const ready = await nat.ready;
  await nat.connect(testPort);
  check('0 the headless engine unlocked Nat\'s account', ready.ready && ready.device === N.device, J({ device: (ready.device || '').slice(0, 8) }));

  const natHears = async (p, ms = 1500) => nat.cmd('hear', { identity: p.device, hz: p.tone, ms });
  const hearsNat = (p, ms = 1500) => call(p, `(async () => { for (let i = 0; i < 40 && ![...c.tracks.values()].some(x => x.identity === ${J(N.device)} && x.source === 'microphone'); i++) await new Promise(r => setTimeout(r, 250)); return c.probeTone(${J(N.device)}, ${NAT_HZ}, ${ms}); })()`);
  const pass = async (fn, n = 6) => { let r; for (let i = 0; i < n; i++) { r = await fn(); if (r.present && r.toneShare > TONE_MIN) return r; await sleep(500); } return r; };
  const silent = async (fn) => { let worst = null; for (let i = 0; i < 2; i++) { const r = await fn(); if (!worst || (r.present && r.toneShare > (worst.toneShare || 0))) worst = r; } return worst; };
  const ok = r => r && r.present && r.toneShare > TONE_MIN;
  const quiet = r => !r || !r.present || r.toneShare < TONE_MAX;
  const presence = async () => ((await rpc(A.port, 'call_presence', { cid: world.cid })).rooms[gid] || []).find(p => p.account === N.account);
  const fourWays = async () => { const [a, b, na, nb] = await Promise.all([hearsNat(A), hearsNat(B), natHears(A), natHears(B)]); return { aliceHearsNat: a.toneShare, bobHearsNat: b.toneShare, natHearsAlice: na.toneShare, natHearsBob: nb.toneShare, ok: ok(a) && ok(b) && ok(na) && ok(nb) }; };

  // ---------------------------------------------------------------- A. join, hear, leave, join again
  for (const p of [A, B]) { await call(p, `c.join(S.gid, 'general')`); await until(() => call(p, `c.phase === 'connected'`), 20000, p.name + ' connected'); }
  const t0 = Date.now();
  await nat.cmd('join', { gid, name: 'general' });
  const s1 = await soft(until(async () => { const s = await nat.snap(); return s.phase === 'connected' && s.peers.filter(p => !p.me && p.member).length === 2 ? s : null; }, 20000, 'nat connected'));
  check('A1 the engine joins: connected, encryption on, both browsers seen as members', s1.phase === 'connected' && s1.encryption.active && s1.encryption.senderIndex.length >= 1 && s1.encryption.senderIndex.every(i => i === s1.encryption.keyIndex), { seconds: (Date.now() - t0) / 1000, enc: s1.encryption, peers: (s1.peers || []).map(p => p.name + (p.member ? '' : '?')) });
  const ways1 = await fourWays();
  check('A2 hear and be heard: Nat hears Alice and Bob, both hear Nat', ways1.ok, ways1);
  const p1 = await soft(until(async () => { const p = await presence(); return p && p.speaking && !p.muted ? p : null; }, 15000, 'presence speaking'));
  await nat.cmd('mute', { on: true });
  const p2 = await soft(until(async () => { const p = await presence(); return p && p.muted && !p.speaking ? p : null; }, 10000, 'presence muted'));
  await nat.cmd('deafen', { on: true });
  const p3 = await soft(until(async () => { const p = await presence(); return p && p.deafened ? p : null; }, 10000, 'presence deafened'));
  await nat.cmd('deafen', { on: false }); await nat.cmd('mute', { on: false });
  const p4 = await soft(until(async () => { const p = await presence(); return p && !p.deafened && !p.muted ? p : null; }, 10000, 'presence back'));
  check('A3 the relay\'s call_presence shows Nat speaking, then muted, then deafened, then back', p1.speaking && p2.muted && p3.deafened && p4.account === N.account, { speaking: p1, muted: p2, deafened: p3, back: p4 });
  check('C1 call_active is true in the core during the call', (await nat.cmd('call_active')).active);
  await nat.cmd('leave');
  const left = await soft(until(async () => !(await call(A, `[...c.room.remoteParticipants.keys()].includes(${J(N.device)})`)) && (await nat.snap()).phase === 'idle', 8000, 'nat gone'));
  check('A4 leave: Nat is gone from the room and the engine is idle', left === true, left);
  const tLeft = Date.now();
  await nat.cmd('join', { gid, name: 'general' });
  await soft(until(async () => (await nat.snap()).phase === 'connected', 20000, 'nat back'));
  const ways2 = await fourWays();
  check('A5 join again: all four directions work again', ways2.ok, ways2);
  res.leftAt = tLeft;

  // ---------------------------------------------------------------- B. who may be heard
  const roomName = (await rpc(A.port, 'call_token', { gid })).room;
  const spy = await openPage(A.chrome, `http://127.0.0.1:${A.port}/vendor/README.txt`, 'spy'); await spy.ev(UMD);
  await spy.ev(`(async()=>{ const room = new LivekitClient.Room(); await room.connect('ws://127.0.0.1:${LK_PORT}', ${J(operatorToken({ room: roomName, roomJoin: true, canPublish: true, canSubscribe: true }, 'operator-spy'))}); await room.localParticipant.setMicrophoneEnabled(true); window.__r = room; return true; })()`);
  const carolTok = await rpc(C.port, 'call_token', { gid });
  const plain = await openPage(A.chrome, `http://127.0.0.1:${A.port}/vendor/README.txt`, 'plain'); await plain.ev(UMD);
  await plain.ev(`(async()=>{ const room = new LivekitClient.Room(); await room.connect(${J(carolTok.url)}, ${J(carolTok.token)}); await room.localParticipant.setMicrophoneEnabled(true); window.__r = room; return true; })()`);
  const sb = await soft(until(async () => { const s = await nat.snap(); return s.blocked.length >= 2 ? s : null; }, 15000, 'two blocked'));
  await sleep(2000);
  const sb2 = await nat.snap();
  const spyB = (sb2.blocked || []).find(b => b.identity === 'operator-spy'), plainB = (sb2.blocked || []).find(b => b.identity === C.device);
  const subscribedTo = id => (sb2.subscribed || []).some(k => k.startsWith(id + '/'));
  check('B1 a device that is not a member (an operator\'s token) is never subscribed and is listed as blocked', spyB && /not a current member/.test(spyB.why) && !subscribedTo('operator-spy'), { blocked: sb2.blocked, subscribed: sb2.subscribed });
  check('B2 a member\'s unencrypted publication is never subscribed and is listed as blocked', plainB && /not marked as encrypted/.test(plainB.why) && !subscribedTo(C.device), plainB || sb2.blocked);
  for (const p of [spy, plain]) await p.ev(`window.__r.disconnect().then(() => true)`).catch(() => { });

  // ---------------------------------------------------------------- D. rotation, through the engine
  const e0 = (await nat.snap()).encryption;
  const E = { port: P(27204) }; startBridge(out, 'erin', E.port); await sleep(1200);
  await rpc(E.port, 'create_account', { name: 'Erin', passphrase: PASS, relay: `http://127.0.0.1:${RELAY_PORT}` });
  await rpc(E.port, 'join', { link: (await rpc(A.port, 'create_invite', { cid: world.cid, uses: 2, ttl: 3600 })).link });
  const tRot0 = Date.now();
  const e1 = await soft(until(async () => { const s = await nat.snap(); return s.encryption.epoch > e0.epoch && (await call(A, `c.snapshot().encryption.epoch`)) === s.encryption.epoch ? s.encryption : null; }, 60000, 'epoch moves'));
  const tRot = Date.now();
  let rot = null; for (let i = 0; i < 4 && !(rot && rot.ok); i++) { const [a, n] = await Promise.all([hearsNat(A), natHears(A)]); rot = { aliceHearsNat: a.toneShare, natHearsAlice: n.toneShare, ok: ok(a) && ok(n) }; }
  const rotAt = (Date.now() - tRot) / 1000;
  await sleep(3500);
  const e2 = (await nat.snap()).encryption;
  check('D1 rotation: a member joining moves the epoch; within about 5 s both ways pass on the new index; the old slot is wiped after 3 s', rot.ok && rotAt <= 6.5 && e2.keyIndex === e1.epoch % 16 && e2.senderIndex.every(i => i === e2.keyIndex) && e2.wipes >= 1 && e2.rotations >= 1,
    { epochs: [e0.epoch, e1.epoch], seconds: rotAt, sinceJoinCommit: (tRot - tRot0) / 1000, after: e2, ...rot });

  // ---------------------------------------------------------------- E. removal, through the engine
  await call(B, `(() => { const orig = c.rpc; let cache = null; c.rpc = async (m, a) => { try { const r = await orig(m, a); if (m === 'call_state') cache = r; return r; } catch (e) { if (m === 'call_state' && cache) return cache; throw e; } }; c._testTrustAll = true; return true; })()`);
  const preKick = await pass(() => hearsNat(B));
  const eK = (await nat.snap()).encryption.epoch;
  await rpc(A.port, 'kick', { cid: world.cid, account: B.account });
  await soft(until(async () => (await nat.snap()).encryption.epoch > eK, 60000, 'epoch after the kick'));
  await sleep(5500);
  const bobAfter = await silent(() => hearsNat(B));
  const aliceStill = await pass(() => hearsNat(A));
  const sk = await nat.snap();
  const bobBlocked = sk.blocked.filter(b => b.identity === B.device);
  check('E1 removal: the kicked member (keeping old keys, trusting everyone) hears nothing from Nat; Alice still does', ok(preKick) && quiet(bobAfter) && ok(aliceStill), { before: preKick.toneShare, after: bobAfter && (bobAfter.present ? bobAfter.toneShare : 'absent'), aliceStill: aliceStill.toneShare });
  check('E2 the engine stops playing the removed member and lists her tracks as blocked', bobBlocked.length >= 1 && bobBlocked.every(b => /not a current member/.test(b.why)) && !(sk.subscribed || []).some(k => k.startsWith(B.device + '/')), { blocked: bobBlocked, subscribed: sk.subscribed });
  await call(B, `c.leave()`).catch(() => { });

  // ---------------------------------------------------------------- F. game keys and the gate
  await nat.cmd('game', { muted: true });
  const g1 = await soft(until(async () => { const s = await nat.snap(); return s.gameMuted ? s : null; }, 5000, 'game muted'));
  await sleep(1200);
  const gOff = await silent(() => hearsNat(A));
  await nat.cmd('game', { muted: false });
  const gBack = await pass(() => hearsNat(A));
  await nat.cmd('mute', { on: true }); await nat.cmd('game', { muted: true }); await nat.cmd('game', { muted: false });
  await sleep(1500);
  const gManual = await silent(() => hearsNat(A));
  const sManual = await nat.snap();
  await nat.cmd('mute', { on: false });
  const gEnd = await pass(() => hearsNat(A));
  await nat.cmd('game', { muted: null });
  check('F1 a simulated game key mutes and releases the microphone', g1.gameMuted && quiet(gOff) && ok(gBack), { during: gOff && gOff.toneShare, after: gBack.toneShare });
  check('F2 a game key letting go does not undo a manual mute', quiet(gManual) && sManual.muted && !sManual.gameMuted && ok(gEnd), { whileManual: gManual && gManual.toneShare, snapshot: { muted: sManual.muted, gameMuted: sManual.gameMuted }, unmuted: gEnd.toneShare });

  // ---------------------------------------------------------------- C2. call_active after the earlier leave
  // (the core counts a call as running until CALL_QUIET, 20 s, after the last call_state; checked on a leave below)

  // ---------------------------------------------------------------- G. the media server restarts
  const twoWays = async () => { const [a, n] = await Promise.all([hearsNat(A), natHears(A)]); return { aliceHearsNat: a.toneShare, natHearsAlice: n.toneShare, ok: ok(a) && ok(n) }; };
  await lk.stopLivekit(); await sleep(2500);
  const gPhase = (await nat.snap()).phase;
  lk.startLivekit();
  const tBack = Date.now();
  let back = null; for (let i = 0; i < 20 && !(back && back.ok); i++) back = await twoWays().catch(e => ({ ok: false, e: String(e) }));
  const backAt = (Date.now() - tBack) / 1000;
  const sG = await nat.snap();
  check('G1 server restart: the engine notices, and both ways pass again within 10 s of the server being back', back.ok && backAt <= 11.5 && ['reconnecting', 'connected'].includes(gPhase), { seconds: backAt, phaseDuring: gPhase, ...back, events: sG.events.slice(-6), senderIndex: sG.encryption.senderIndex });

  // ---------------------------------------------------------------- H. a 20 s outage
  await lk.stopLivekit();
  const during = [];
  for (let i = 0; i < 8; i++) { await sleep(2500); const s = await nat.snap(); during.push(s.phase + '/' + s.attempt); }
  lk.startLivekit();
  const tBack2 = Date.now();
  let back2 = null; for (let i = 0; i < 30 && !(back2 && back2.ok); i++) back2 = await twoWays().catch(e => ({ ok: false, e: String(e) }));
  const back2At = (Date.now() - tBack2) / 1000;
  check('H1 a 20 s outage: the engine keeps trying, then recovers by itself', back2.ok && during.every(x => x.startsWith('reconnecting')) && during.some(x => !x.endsWith('/0')), { during, secondsAfterBack: back2At, ...back2, events: (await nat.snap()).events.slice(-5) });

  // ---------------------------------------------------------------- I. removal while the media server is down
  await lk.stopLivekit();
  await sleep(1500);
  await rpc(A.port, 'kick', { cid: world.cid, account: N.account });
  const sI = await soft(until(async () => { const s = await nat.snap(); return s.phase === 'error' ? s : null; }, 30000, 'nat removed'));
  check('I1 removal during an outage ends the call with "removed"', sI.phase === 'error' && sI.error && sI.error.kind === 'removed', { phase: sI.phase, error: sI.error, events: (sI.events || []).slice(-4) });
  lk.startLivekit();
  const tEnd = Date.now();
  const inactive = await soft(until(async () => !(await nat.cmd('call_active')).active, 40000, 'call_active false'));
  check('C2 call_active goes false after the call ends (the core\'s CALL_QUIET is 20 s, not the plan\'s 5 s)', inactive === true && (Date.now() - tEnd) / 1000 <= 25, { seconds: (Date.now() - tEnd) / 1000 });
  await nat.cmd('quit').catch(() => { });
  await Promise.race([new Promise(r => nat.proc.on('exit', r)), sleep(5000)]);
} catch (e) { res.error = String(e.stack || e); console.log('ERROR', res.error); failed++; }
res.failed = failed;
fs.writeFileSync(path.join(out, 'result.json'), JSON.stringify(res, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED', '->', path.join(out, 'result.json'));
stopAll(); process.exit(failed ? 1 : 0);
