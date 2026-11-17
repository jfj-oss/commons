// Step 3 voice, real devices (CALLS-PLAN.md section 4): the engine's PlatformAudio device handling against a PRIVATE
// PipeWire in its own XDG_RUNTIME_DIR (never the desktop's). One local LiveKit room: Alice is the real app/ui call.js
// in headless Chrome for Testing; Nat is `mega-gpui --call-headless` in platform-audio mode, its microphone the monitor
// of a null sink that a test process is feeding a 900 Hz tone, its speaker another null sink (silent). No audible sound
// is ever produced: null sinks discard everything.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --bin dev_bridge
//   COMMONS_TEST_PORT_OFFSET=400 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-call-voice.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn } from 'node:child_process';
import { root, startServers, startBridge, rpc, sleep, stopAll, procs, run, wav, RELAY_PORT, port as P } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-call-voice');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const NAT_HZ = 900, PASS = 'correct horse battery';
const J = x => JSON.stringify(x);
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? '  [' + (typeof detail === 'string' ? detail : J(detail)) + ']' : ''}`); };
async function until(fn, ms, what) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(250); } }
const soft = p => p.catch(e => ({ timeout: String(e.message || e) }));

// ------------------------------------------------------------------ the private PipeWire
const RT = path.join(out, 'pw-runtime');
fs.mkdirSync(RT, { recursive: true }); fs.chmodSync(RT, 0o700);
const pwEnv = { ...process.env, XDG_RUNTIME_DIR: RT, PIPEWIRE_RUNTIME_DIR: RT, PULSE_SERVER: `unix:${RT}/pulse/native` };
run('pipewire', [], pwEnv, path.join(out, 'pipewire.log'));
run('pipewire-pulse', [], pwEnv, path.join(out, 'pipewire-pulse.log'));
await until(() => fs.existsSync(path.join(RT, 'pipewire-0')) && fs.existsSync(path.join(RT, 'pulse/native')), 15000, 'private pipewire up');
await sleep(800);
const pactl = args => new Promise((res, rej) => { const p = spawn('pactl', args, { env: pwEnv, stdio: ['ignore', 'pipe', 'pipe'] }); let o = '', e = ''; p.stdout.on('data', d => o += d); p.stderr.on('data', d => e += d); p.on('close', c => c === 0 ? res(o.trim()) : rej(new Error(`pactl ${args.join(' ')}: ${e || o}`))); });
await pactl(['load-module', 'module-null-sink', 'sink_name=vtest_a', 'sink_properties=device.description=CommonsTestA']);
await pactl(['load-module', 'module-null-sink', 'sink_name=vtest_b', 'sink_properties=device.description=CommonsTestB']);
// WebRTC's PulseAudio ADM hides monitor sources, so each monitor is re-exposed as a plain source (a "microphone").
await pactl(['load-module', 'module-remap-source', 'master=vtest_a.monitor', 'source_name=vtest_a_mic', 'source_properties=device.description=CommonsTestMicA']);
await pactl(['load-module', 'module-remap-source', 'master=vtest_b.monitor', 'source_name=vtest_b_mic', 'source_properties=device.description=CommonsTestMicB']);
// no session manager runs in this private PipeWire, so there is no hardware and no default device to point anywhere;
// the engine is told which devices to use explicitly below, before it joins.
// the "microphone": a 900 Hz tone fed into vtest_a, picked up on vtest_a.monitor, re-exposed as vtest_a_mic
const toneFile = path.join(out, 'tone900.wav'); wav(toneFile, NAT_HZ, 180);
run('paplay', ['--device=vtest_a', toneFile], pwEnv, path.join(out, 'tone.log'));

// ------------------------------------------------------------------ the world
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
  world = await makeWorld(out, ['alice']);
  await openApps(world, ['alice']);
  const A = world.people.alice;
  const call = (p, expr) => p.page.ev(`(async()=>{ const c = window.__commonsCall; return ${expr}; })()`);
  const gid = world.gid;
  await call(A, `c.join(S.gid, 'general')`);
  await until(() => call(A, `c.phase === 'connected'`), 20000, 'alice connected');

  // Nat's account is made by a bridge, which then stops so the native app can open the same folder
  const N = { name: 'nat', port: P(27203) };
  const natBridge = startBridge(out, 'nat', N.port); await sleep(1200);
  await rpc(N.port, 'create_account', { name: 'Nat', passphrase: PASS, relay: `http://127.0.0.1:${RELAY_PORT}` });
  await rpc(N.port, 'join', { link: (await rpc(A.port, 'create_invite', { cid: world.cid, uses: 4, ttl: 3600 })).link });
  await until(async () => { const s = await rpc(N.port, 'snapshot'); const c = s.communities.find(c => c.id === world.cid); return c && c.channels.some(ch => ch.gid === gid && ch.joined && ch.status === 'ok'); }, 60000, 'nat joined');
  const ns = await rpc(N.port, 'snapshot'); N.account = ns.me.account; N.device = ns.me.device;
  try { process.kill(-natBridge.pid, 'SIGTERM'); } catch { } await sleep(1500);

  const testPort = P(27300);
  nat = new Native({ COMMONS_DATA: path.join(out, 'data/nat'), COMMONS_RELAY: `http://127.0.0.1:${RELAY_PORT}`, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_TEST: String(testPort), MEGA_GPUI_NOTIFY_FILE: path.join(out, 'nat-notes.jsonl'), RUST_LOG: process.env.RUST_LOG || 'info', ...pwEnv });
  const ready = await nat.ready;
  await nat.connect(testPort);
  check('0 the headless engine unlocked Nat\'s account', ready.ready && ready.device === N.device, J({ device: (ready.device || '').slice(0, 8) }));

  // devices first: pick the null sinks, then tell the engine before it joins (there is no default device in this
  // private PipeWire, and no hardware to fall back on).
  const dev = await nat.cmd('devices');
  const byName = (list, s) => list.find(d => (d.name || '').toLowerCase().includes(s.toLowerCase()));
  const micA = byName(dev.mic || [], 'Commonstestmica'), micB = byName(dev.mic || [], 'Commonstestmicb');
  const outA = byName(dev.out || [], 'Commonstesta'), outB = byName(dev.out || [], 'Commonstestb');
  check('V7 device list is non-empty and holds the null sinks (inputs and outputs)', (dev.mic || []).length >= 2 && (dev.out || []).length >= 2 && micA && micB && outA && outB, { mic: (dev.mic || []).map(d => `${d.name}#${d.id}`), out: (dev.out || []).map(d => `${d.name}#${d.id}`), outB: outB && outB.id, outA: outA && outA.id });
  await nat.cmd('mic', { device: micA.id });
  await nat.cmd('out', { device: outA.id });

  await nat.cmd('join', { gid, name: 'general' });
  const s1 = await soft(until(async () => { const s = await nat.snap(); return s.phase === 'connected' && s.peers.filter(p => !p.me && p.member).length === 1 ? s : null; }, 20000, 'nat connected'));
  check('V5 the engine joins in platform-audio mode', s1.phase === 'connected' && s1.encryption.active, { phase: s1.phase, enc: s1.encryption.active });

  // (No audible assertion here: this private PipeWire runs without a session manager, so its streams never link and
  // nothing plays anywhere. Hearing and per-person volume are covered by the silent e2e and the human echo check.)
  const healthy = s => s.phase === 'connected' && s.encryption.active && s.attempt === 0;
  const noDrop = (s, id, key) => healthy(s) && s[key] === id;

  // switch the speaker mid-call: applied, no drop
  await nat.cmd('out', { device: outB.id });
  const sOut = await soft(until(async () => { const s = await nat.snap(); return s.outId === outB.id ? s : null; }, 8000, 'out switched'));
  check('V8 switching the speaker mid-call is applied and does not drop the call', noDrop(sOut, outB.id, 'outId'), { phase: sOut.phase, outId: sOut.outId, events: sOut.events });

  // switch the microphone mid-call: applied, no drop
  await nat.cmd('mic', { device: micB.id });
  const sMic = await soft(until(async () => { const s = await nat.snap(); return s.micId === micB.id ? s : null; }, 8000, 'mic switched'));
  check('V9 switching the microphone mid-call is applied and does not drop the call', noDrop(sMic, micB.id, 'micId'), { phase: sMic.phase, micId: sMic.micId, events: sMic.events });

  // let the device-module switches finish (or be abandoned by the engine's timeout) before the next step
  await sleep(6000);

  // V11 the choice is remembered by NAME (ids are positions that move), and a microphone that sends only silence is reported
  // (this private PipeWire has no session manager, so the device module's calls are slow here and hold the microphone slot; the level reader waits its turn)
  const sQuiet = await soft(until(async () => { const s = await nat.snap(); return s.micQuiet ? s : null; }, 45000, 'mic quiet')).then(r => r.timeout ? nat.snap() : r);
  const prefs = JSON.parse(fs.readFileSync(path.join(out, 'data/nat/gpui-call-prefs.json'), 'utf8'));
  check('V11 the microphone is remembered by name and a silent one is reported (micQuiet), without dropping the call', sQuiet.micName === micB.name && prefs.micName === micB.name && sQuiet.micQuiet === true && sQuiet.micLevel < 0.0003 && healthy(sQuiet), { micName: sQuiet.micName, saved: prefs.micName, micQuiet: sQuiet.micQuiet, micLevel: sQuiet.micLevel, muted: sQuiet.muted, deafened: sQuiet.deafened, gameMuted: sQuiet.gameMuted, phase: sQuiet.phase });

  // a processing switch republishes the mic (new SID) without rejoining
  const sidBefore = (await nat.cmd('mic_options')).sid;
  await nat.cmd('processing', { noise: false });
  const sProc = await until(async () => { const s = await nat.snap(); return !s.noiseSuppression && s.phase === 'connected' ? s : null; }, 8000, 'noise off').catch(async e => { const s = await nat.snap(); console.log('NOISE-OFF STUCK', J({ phase: s.phase, error: s.error, events: s.events, noise: s.noiseSuppression })); throw e; });
  const moProc = await nat.cmd('mic_options');
  check('V10 a processing switch republishes the mic without a rejoin', healthy(sProc) && !sProc.noiseSuppression && moProc.sid && moProc.sid !== sidBefore, { sidBefore, sidAfter: moProc.sid, phase: sProc.phase, noiseSuppression: sProc.noiseSuppression, error: sProc.error, events: sProc.events });

  await nat.cmd('quit').catch(() => { });
  await Promise.race([new Promise(r => nat.proc.on('exit', r)), sleep(5000)]);
} catch (e) { res.error = String(e.stack || e); console.log('ERROR', res.error); failed++; }
res.failed = failed;
fs.writeFileSync(path.join(out, 'result.json'), JSON.stringify(res, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED', '->', path.join(out, 'result.json'));
stopAll(); process.exit(failed ? 1 : 0);
