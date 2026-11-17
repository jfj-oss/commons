// Real devices, no sound: the native call engine with the platform audio module (PipeWire, the real default
// microphone and speaker) against a web client in one local LiveKit room. NOTHING is played out loud: the web client's
// "microphone" is an oscillator at zero gain (digital silence) and its output is muted; what the native app hears is
// measured on the receiver's inbound-rtp energy, what the web client hears on the AnalyserNode of the received track.
// The native microphone is the real one (room noise is fine). Never changes the default devices or WirePlumber state.
// Needs a real PipeWire session and a microphone; skips (exit 0) when there is none.
//   cargo build --release -p mega-gpui
//   MEGA_GPUI_BIN=<target>/release/mega-gpui COMMONS_TEST_PORT_OFFSET=3300 node tools/gpui-realvoice.mjs <outdir> [prefs-json]
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn, execFileSync } from 'node:child_process';
import { root, startServers, startBridge, openPage, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/realvoice/run');
const PREFS = process.argv[3] ? JSON.parse(process.argv[3]) : null;   // written as gpui-call-prefs.json before the engine starts
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/release/mega-gpui');
const PASS = 'correct horse battery';
const J = x => JSON.stringify(x);
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? '  [' + (typeof detail === 'string' ? detail : J(detail)).slice(0, 700) + ']' : ''}`); };
async function until(fn, ms, what) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(250); } }
const pa = (...a) => { try { return execFileSync('pactl', a, { encoding: 'utf8' }); } catch { return ''; } };
// the streams one process (by pid) has open on the sound server: kind is 'source-outputs' (recording) or 'sink-inputs' (playing)
function streams(kind, pid) {
  const txt = pa('list', kind), res = [];
  for (const blk of txt.split(/\n(?=(?:Source Output|Sink Input) #)/)) if (new RegExp('application.process.id = "' + pid + '"').test(blk)) res.push({ target: (blk.match(/\t(?:Source|Sink): (\d+)/) || [])[1], corked: /Corked: yes/.test(blk), muted: /Mute: yes/.test(blk) });
  return res;
}
const silentMic = `(() => { navigator.mediaDevices.getUserMedia = async (c) => { if (c && c.audio && !c.video) { const ctx = (() => { try { return new AudioContext({ sinkId: { type: 'none' } }); } catch { return new AudioContext(); } })(); await ctx.resume().catch(() => {}); const o = ctx.createOscillator(); o.frequency.value = 440; const g = ctx.createGain(); g.gain.value = 0; const d = ctx.createMediaStreamDestination(); o.connect(g); g.connect(d); o.start(); return d.stream; } throw new Error('no video here'); }; })();`;
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
  if (!/\bSource\b|alsa_input|input/.test(pa('list', 'short', 'sources').replace(/\.monitor/g, '')) ) { console.log('SKIP no real microphone'); process.exit(0); }
  const srcMute = () => /yes/.test(pa('get-source-mute', '@DEFAULT_SOURCE@'));
  const mute0 = srcMute();
  const before = { source: pa('get-default-source').trim(), sink: pa('get-default-sink').trim() };
  lk = await startServers(out, {});
  world = await makeWorld(out, ['alice']);
  await openApps(world, ['alice'], { before: silentMic });
  const A = world.people.alice, gid = world.gid;
  const call = (p, expr) => p.page.ev(`(async()=>{ const c = window.__commonsCall; return ${expr}; })()`);
  const N = { name: 'nat', port: P(27203) };
  const natBridge = startBridge(out, 'nat', N.port); await sleep(1200);
  await rpc(N.port, 'create_account', { name: 'Nat', passphrase: PASS, relay: `http://127.0.0.1:${RELAY_PORT}` });
  await rpc(N.port, 'join', { link: (await rpc(A.port, 'create_invite', { cid: world.cid, uses: 4, ttl: 3600 })).link });
  await until(async () => { const s = await rpc(N.port, 'snapshot'); const c = s.communities.find(c => c.id === world.cid); return c && c.channels.some(ch => ch.gid === gid && ch.joined && ch.status === 'ok'); }, 60000, 'nat joined');
  const ns = await rpc(N.port, 'snapshot'); N.account = ns.me.account; N.device = ns.me.device;
  try { process.kill(-natBridge.pid, 'SIGTERM'); } catch { } await sleep(1500);
  const dataDir = path.join(out, 'data/nat');
  if (PREFS) fs.writeFileSync(path.join(dataDir, 'gpui-call-prefs.json'), J(PREFS));
  const testPort = P(27300);
  nat = new Native({ COMMONS_DATA: dataDir, COMMONS_RELAY: `http://127.0.0.1:${RELAY_PORT}`, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_TEST: String(testPort), MEGA_GPUI_NOTIFY_FILE: path.join(out, 'nat-notes.jsonl'), RUST_LOG: process.env.RUST_LOG || 'info' });
  const ready = await nat.ready; await nat.connect(testPort);
  const pid = nat.proc.pid;

  await call(A, `c.join(S.gid, 'general')`); await until(() => call(A, `c.phase === 'connected'`), 20000, 'alice connected');
  await nat.cmd('join', { gid, name: 'general' });
  const s1 = await until(async () => { const s = await nat.snap(); return s.phase === 'connected' && s.peers.filter(p => !p.me && p.member).length === 1 ? s : null; }, 25000, 'nat connected');
  await nat.cmd('meter', { on: true });
  await nat.cmd('devices');
  await sleep(3500);
  const mute1 = srcMute();
  const s2 = await nat.snap();
  console.log(J({ micId: s2.micId, outId: s2.outId, muted: s2.muted, gameMuted: s2.gameMuted, deafened: s2.deafened, pushToTalk: s2.pushToTalk, micName: s2.micName, outName: s2.outName, micLevel: s2.micLevel, micQuiet: s2.micQuiet, micSystemMuted: s2.micSystemMuted, nMic: s2.devices.mic.length, nOut: s2.devices.out.length }));
  const rec = streams('source-outputs', pid), play = streams('sink-inputs', pid);
  console.log('streams', J({ rec, play }));
  // direction 1: native microphone -> web. The microphone is the real one (room noise); the web side measures energy.
  const probe = await call(A, `(async () => { for (let i = 0; i < 40 && ![...c.tracks.values()].some(x => x.identity === ${J(N.device)} && x.source === 'microphone'); i++) await new Promise(r => setTimeout(r, 250)); return c.probeTone(${J(N.device)}, 1000, 3000); })()`);
  console.log('web hears native', J(probe));
  // direction 2: web -> native playout (the web side sends digital silence, so the energy is ~0 whatever the speaker does;
  // what this proves is that the playout path and its stream exist and that RTP arrives)
  const pl = await nat.cmd('plout', { identity: A.device, ms: 1500 }); console.log('native plout', J(pl));
  check('the engine published a microphone track the web client can see', probe.present, probe);
  check('native opened a capture stream on the sound server (the real microphone)', rec.length >= 1 && !rec[0].corked, rec);
  check('native opened a playback stream on the sound server', play.length >= 1, play);
  const sysMuted = mute0;
  if (sysMuted) {
    // the real microphone is switched off in the sound settings: it delivers digital silence, and the app must say so
    await sleep(3000); const s3 = await nat.snap();
    check('the system microphone is muted: the app reports it (micSystemMuted, micQuiet)', s3.micSystemMuted === true && s3.micQuiet === true, { micSystemMuted: s3.micSystemMuted, micQuiet: s3.micQuiet, micLevel: s3.micLevel });
  } else {
    check('the web client receives non-silent audio from the real microphone', probe.present && probe.rms > 1e-5, probe);
    check('the in-call meter moves with the real microphone', s2.micLevel > 0.0003 && !s2.micQuiet && s2.micSystemMuted === false, { micLevel: s2.micLevel, micQuiet: s2.micQuiet, micSystemMuted: s2.micSystemMuted });
  }
  check('the chosen devices have names and the list is the real one', s2.devices.mic.length >= 2 && s2.devices.out.length >= 2, { mic: s2.devices.mic.map(d => d.name), out: s2.devices.out.map(d => d.name) });
  if (PREFS && PREFS.micName) check('a remembered name is found again whatever its old index', s2.micName === PREFS.micName && s2.micId && s2.devices.mic.some(d => d.id === s2.micId && d.name === PREFS.micName), { micId: s2.micId, micName: s2.micName });
  else check('an old positional id (or none) means the system default, not a call error', s2.micName === null && s2.micId === null && s2.phase === 'connected', { micId: s2.micId, micName: s2.micName, phase: s2.phase });
  check('the microphone is not muted by the app (muted / gameMuted / deafened all false)', !s2.muted && !s2.gameMuted && !s2.deafened, { muted: s2.muted, gameMuted: s2.gameMuted, deafened: s2.deafened });
  const after = { source: pa('get-default-source').trim(), sink: pa('get-default-sink').trim() };
  check('the default sound devices were not changed', after.source === before.source && after.sink === before.sink, { before, after });
  res.snapshot = s2; res.probe = probe; res.plout = pl; res.streams = { rec, play };
  await nat.cmd('leave'); await sleep(2500); const mute2 = srcMute();
  // the settings meter outside a call: a parec recording of the chosen source, level only
  await nat.cmd('meter', { on: true }); await sleep(2500);
  const s4 = await nat.snap();
  const recOut = streams('source-outputs', 0).length;   // (other processes' streams are not ours; parec is a child, so look by name)
  const meterStream = /mega-mic-test/.test(pa('list', 'source-outputs'));
  await nat.cmd('meter', { on: false }); await sleep(1500);
  const meterGone = !/mega-mic-test/.test(pa('list', 'source-outputs'));
  check('outside a call the meter records the microphone only while the settings panel asks, then stops', meterStream && meterGone, { meterStream, meterGone, micLevel: s4.micLevel });
  if (!sysMuted) check('the meter outside a call moves with the real microphone', s4.micLevel > 0.0003, { micLevel: s4.micLevel });
  console.log('system mic mute before/during/after:', mute0, mute1, mute2);
  check('the app never changes the system microphone mute', mute0 === mute1 && mute1 === mute2, { mute0, mute1, mute2 });
  await nat.cmd('quit').catch(() => { });
  await Promise.race([new Promise(r => nat.proc.on('exit', r)), sleep(5000)]);
} catch (e) { res.error = String(e.stack || e); console.log('ERROR', res.error); failed++; }
res.failed = failed;
fs.writeFileSync(path.join(out, 'result.json'), JSON.stringify(res, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED', '->', path.join(out, 'result.json'));
stopAll(); process.exit(failed ? 1 : 0);
