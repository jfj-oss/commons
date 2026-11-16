// Step 5 screen share and one app's sound, from the native engine (CALLS-PLAN.md sections 6 and 7), against the web client.
// One local LiveKit room: Alice is the real app/ui call.js in headless Chrome for Testing; Nat is `mega-gpui --call-headless`
// (no audio device of its own: MEGA_GPUI_CALL_AUDIO=none). The portal needs a person to pick a screen, so the pictures come
// from the engine's test-only synthetic source (a scrolling 1440p text page) which goes through the same fit, convert and
// publish path; the portal itself is not run here (its no-session-bus error path is). The shared "apps" are two paplay
// processes playing different tones into a null sink of a PRIVATE PipeWire (own XDG_RUNTIME_DIR, with a hardware-free
// WirePlumber so streams link). Nothing is audible, no window opens on the desktop.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --bin dev_bridge
//   COMMONS_TEST_PORT_OFFSET=500 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-call-share.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn } from 'node:child_process';
import { root, startServers, startBridge, rpc, sleep, stopAll, procs, run, wav, RELAY_PORT, port as P } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-call-share');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const PASS = 'correct horse battery', HZ_A = 600, HZ_B = 1500, TONE_MIN = 0.15, TONE_MAX = 0.02;
const J = x => JSON.stringify(x);
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? '  [' + (typeof detail === 'string' ? detail : J(detail)) + ']' : ''}`); };
async function until(fn, ms, what) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(250); } }
const soft = p => p.catch(e => ({ timeout: String(e.message || e) }));

// ------------------------------------------------------------------ the private PipeWire (no hardware, ever)
const RT = path.join(out, 'pw-runtime'), CFG = path.join(out, 'pw-config');
fs.mkdirSync(RT, { recursive: true }); fs.chmodSync(RT, 0o700);
fs.mkdirSync(path.join(CFG, 'wireplumber/wireplumber.conf.d'), { recursive: true });
fs.writeFileSync(path.join(CFG, 'wireplumber/wireplumber.conf.d/90-nohw.conf'), 'wireplumber.profiles = {\n  main = {\n    hardware.audio = disabled\n    hardware.bluetooth = disabled\n    hardware.video-capture = disabled\n  }\n}\n');
const pwEnv = { ...process.env, XDG_RUNTIME_DIR: RT, PIPEWIRE_RUNTIME_DIR: RT, PULSE_SERVER: `unix:${RT}/pulse/native`, XDG_CONFIG_HOME: CFG };
delete pwEnv.DBUS_SESSION_BUS_ADDRESS; delete pwEnv.DISPLAY; delete pwEnv.WAYLAND_DISPLAY;   // no bus: nothing here can reach the desktop's audio or portals
run('pipewire', [], pwEnv, path.join(out, 'pipewire.log'));
run('pipewire-pulse', [], pwEnv, path.join(out, 'pipewire-pulse.log'));
await until(() => fs.existsSync(path.join(RT, 'pipewire-0')) && fs.existsSync(path.join(RT, 'pulse/native')), 15000, 'private pipewire up');
run('wireplumber', [], pwEnv, path.join(out, 'wireplumber.log'));
await sleep(1500);
const pactl = args => new Promise((res, rej) => { const p = spawn('pactl', args, { env: pwEnv, stdio: ['ignore', 'pipe', 'pipe'] }); let o = '', e = ''; p.stdout.on('data', d => o += d); p.stderr.on('data', d => e += d); p.on('close', c => c === 0 ? res(o.trim()) : rej(new Error(`pactl ${args.join(' ')}: ${e || o}`))); });
await pactl(['load-module', 'module-null-sink', 'sink_name=appsink', 'sink_properties=device.description=SharedAppsSink']);
await sleep(500);
// two "apps": each plays its own tone for 10 minutes into the null sink (nothing audible)
const toneA = path.join(out, 'a.wav'), toneB = path.join(out, 'b.wav'); wav(toneA, HZ_A, 600); wav(toneB, HZ_B, 600);
const app = (name, media, file) => run('paplay', ['--device=appsink', `--client-name=${name}`, `--stream-name=${media}`, file], pwEnv, path.join(out, `${name}.log`));
const appA = app('AlphaApp', 'Alpha song', toneA), appB = app('BetaApp', 'Beta video', toneB);

// ------------------------------------------------------------------ the native engine's test port
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
  const ns = await rpc(N.port, 'snapshot'); N.device = ns.me.device;
  try { process.kill(-natBridge.pid, 'SIGTERM'); } catch { } await sleep(1500);

  const testPort = P(27300);
  nat = new Native({ COMMONS_DATA: path.join(out, 'data/nat'), COMMONS_RELAY: `http://127.0.0.1:${RELAY_PORT}`, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_TEST: String(testPort), MEGA_GPUI_CALL_AUDIO: 'none',
    MEGA_GPUI_NOTIFY_FILE: path.join(out, 'nat-notes.jsonl'), RUST_LOG: process.env.RUST_LOG || 'info', ...pwEnv });
  const ready = await nat.ready;
  await nat.connect(testPort);
  check('0 the headless engine unlocked Nat\'s account', ready.ready && ready.device === N.device);

  // what Alice sees of Nat
  const natTrack = (source, want = true) => call(A, `(async () => { for (let i = 0; i < 40; i++) { const t = [...c.tracks.values()].find(x => x.identity === ${J(N.device)} && x.source === ${J(source)}); if (t) return { present: true, name: t.pub.trackName, enc: t.pub.isEncrypted, sid: t.pub.trackSid }; if (!${want}) { await new Promise(r => setTimeout(r, 100)); continue; } await new Promise(r => setTimeout(r, 250)); } return { present: false }; })()`);
  const gone = async source => { for (let i = 0; i < 40; i++) { const r = await natTrack(source, false).catch(() => ({ present: true })); if (!r.present) return true; await sleep(250); } return false; };
  // the picture at one quality: size once it settles, then frames per second over 4 s
  const view = (q, want) => call(A, `(async () => { const sleep = ms => new Promise(r => setTimeout(r, ms));
    let t; for (let i = 0; i < 60 && !t; i++) { t = [...c.tracks.values()].find(x => x.identity === ${J(N.device)} && x.source === 'screen_share'); if (!t) await sleep(250); } if (!t) return { present: false };
    c.setViewQuality(t.pub.trackSid, ${J(q)}); const v = t.track.attach(); v.muted = true; await v.play().catch(() => {});
    const stats = async () => { const r = await t.track.receiver.getStats(); let o = null; r.forEach(s => { if (s.type === 'inbound-rtp' && s.kind === 'video') o = s; }); return o; };
    let s = null; const t0 = Date.now(); while (Date.now() - t0 < 25000) { await sleep(500); s = await stats(); if (s && s.frameHeight === ${want}) break; }
    await sleep(2000); const a = await stats(); await sleep(4000); const b = await stats(); const vw = b && b.frameWidth, vh = b && b.frameHeight; t.track.detach(v);
    return { present: true, w: vw, h: vh, fps: a && b ? (b.framesDecoded - a.framesDecoded) / 4 : -1, enc: t.pub.isEncrypted, sid: t.pub.trackSid, settle: (Date.now() - t0) / 1000 }; })()`);
  // a viewer's frame rate depends on the machine's load, so a low sample is taken again (up to 3 times) before it counts
  const viewBest = async (q, want, min) => { let best = null, n = 0; for (; n < 3; n++) { const r = await view(q, want); if (!best || r.fps > best.fps) best = r; if (best.fps >= min && best.h === want) break; await sleep(2000); } return { ...best, attempts: n + 1 }; };
  const tone = (hz, source = 'screen_share_audio', ms = 1500) => call(A, `c.probeTone(${J(N.device)}, ${hz}, ${ms}, ${J(source)})`);
  const toneOk = async hz => { let r; for (let i = 0; i < 6; i++) { r = await tone(hz); if (r.present && r.toneShare > TONE_MIN) return r; await sleep(500); } return r; };
  const layers = async () => { const e0 = await nat.cmd('share_stats'); await sleep(4000); const e1 = await nat.cmd('share_stats'); return (e1.layers || []).map(l => { const b = (e0.layers || []).find(x => x.rid === l.rid) || { encoded: 0 }; return { rid: l.rid, size: l.w + 'x' + l.h, encodedFps: (l.encoded - b.encoded) / 4, limit: l.limit, target: Math.round(l.target) }; }); };
  const apps = async () => (await nat.cmd('share_apps')).apps || [];

  await nat.cmd('join', { gid, name: 'general' });
  const s1 = await soft(until(async () => { const s = await nat.snap(); return s.phase === 'connected' && s.peers.filter(p => !p.me && p.member).length === 1 ? s : null; }, 20000, 'nat connected'));
  check('1 Nat joins the call and Alice sees him', s1.phase === 'connected' && s1.encryption.active, { phase: s1.phase });

  // ------------------------------------------------------------ the list of apps
  let last = null;
  const list = await until(async () => { const l = last = await apps(); return l.length >= 2 ? l : null; }, 12000, 'two apps listed').catch(e => { console.log('last list', J(last)); throw e; });
  const la = list.find(a => a.app === 'AlphaApp'), lb = list.find(a => a.app === 'BetaApp');
  check('2 the app list shows each playing app with its name, pid and media name', la && lb && la.media === 'Alpha song' && lb.media === 'Beta video' && la.pid === appA.pid && lb.pid === appB.pid && la.sink === 'appsink' && !la.browser, list.map(a => ({ id: a.id, app: a.app, pid: a.pid, media: a.media, sink: a.sink })));

  // ------------------------------------------------------------ sharp text
  await nat.cmd('share', { profile: 'text', source: 'test', w: 2560, h: 1440 });
  const live = await soft(until(async () => { const s = await nat.snap(); return s.share.state === 'live' && s.share.sid ? s : null; }, 15000, 'share live'));
  check('3 the snapshot says the share is live, encrypted, 2560x1440, text profile', live.share && live.share.state === 'live' && live.share.encrypted && live.share.width === 2560 && live.share.height === 1440 && live.share.profile === 'text' && live.share.source === 'test', live.share || live);
  const tr = await natTrack('screen_share');
  check('4 Alice receives the share as a screen-share track, encrypted', tr.present && tr.enc, tr);
  const hi = await viewBest('high', 1440, 20), lo = await view('low', 540), hi2 = await view('high', 1440);
  check('5 sharp text: Alice gets 2560x1440 on HIGH at about 30 fps, and 960x540 on LOW', hi.w === 2560 && hi.h === 1440 && hi.fps >= 20 && lo.w === 960 && lo.h === 540 && hi2.h === 1440, { high: hi, low: lo, highAgain: hi2 });
  const sidText = live.share.sid;

  // ------------------------------------------------------------ switching the source
  await nat.cmd('share_switch', { source: 'test', w: 2560, h: 1440, speed: 5 });
  await sleep(2500);
  const sSame = await nat.snap();
  check('6 switching to a source of the same size keeps the same track (no republish)', sSame.share.state === 'live' && sSame.share.sid === sidText && !sSame.share.switching, { sid: sSame.share.sid, was: sidText });
  await nat.cmd('share_switch', { source: 'test', w: 1920, h: 1080 });
  const sNew = await soft(until(async () => { const s = await nat.snap(); return s.share.sid && s.share.sid !== sidText && s.share.width === 1920 ? s : null; }, 15000, 'republished at 1080p'));
  const hi3 = await viewBest('high', 1080, 15), lo3 = await view('low', 540);
  check('7 switching to a 1920x1080 source republishes and Alice gets 1920x1080 HIGH and 960x540 LOW', sNew.share && sNew.share.height === 1080 && hi3.w === 1920 && hi3.h === 1080 && lo3.h === 540 && lo3.w === 960, { share: sNew.share, high: hi3, low: lo3 });

  // ------------------------------------------------------------ the other profiles
  await nat.cmd('share_switch', { source: 'test', w: 2560, h: 1440 });
  await sleep(1500);
  await nat.cmd('share_profile', { profile: 'motion' });
  const sMo = await soft(until(async () => { const s = await nat.snap(); return s.share.state === 'live' && s.share.profile === 'motion' && s.share.height === 1080 ? s : null; }, 15000, 'motion live'));
  const [mHi, mEnc] = await Promise.all([viewBest('high', 1080, 15), (async () => { await sleep(6000); return layers(); })()]); const mLo = await view('low', 540);
  check('8 smooth motion: a 1440p source is fitted to 1920x1080, 30 fps, plus the 540p layer', sMo.share && mHi.w === 1920 && mHi.h === 1080 && mHi.fps >= 15 && mLo.w === 960 && mLo.h === 540, { share: sMo.share, high: mHi, low: mLo, senderLayers: mEnc });
  await nat.cmd('share_profile', { profile: 'motion60' });
  const s60 = await soft(until(async () => { const s = await nat.snap(); return s.share.state === 'live' && s.share.profile === 'motion60' ? s : null; }, 15000, 'motion60 live'));
  const fHi = await viewBest('high', 1080, 35);
  check('9 fast motion: 1920x1080 and the frame rate Alice sees is well above 30 fps (reported)', s60.share && fHi.h === 1080 && fHi.fps > 30, { sentFps: s60.share && s60.share.fps, viewerFps: fHi.fps });
  // the encoder's own frame rate per layer on the sender (outbound-rtp, over 4 s) with a viewer on the 540p layer, once with the
  // heavy scrolling page and once with a calm one (reported only: a full-screen scroll of dense text costs more bits than
  // 350 to 600 kbit/s allow, so the encoder drops frames of the small layer)
  await call(A, `(async () => { const t = [...c.tracks.values()].find(x => x.identity === ${J(N.device)} && x.source === 'screen_share'); c.setViewQuality(t.pub.trackSid, 'low'); const v = t.track.attach(); v.muted = true; await v.play().catch(() => {}); window.__lowView = v; })()`);
  await sleep(3000);
  const busy = await layers();
  await nat.cmd('share_switch', { source: 'test', w: 2560, h: 1440, speed: 0 }); await sleep(3000);
  const calm = await layers();
  check('9b the encoder runs both layers: the full picture and the 540p layer (reported: encoded frames per second each, busy page then calm page)', busy.length === 2 && busy.some(l => l.size === '1920x1080') && busy.some(l => l.size === '960x540'), { busy, calm });
  await call(A, `(() => { const t = [...c.tracks.values()].find(x => x.identity === ${J(N.device)} && x.source === 'screen_share'); t.track.detach(window.__lowView); })()`);
  await nat.cmd('share_switch', { source: 'test', w: 2560, h: 1440 }); await sleep(1500);
  await nat.cmd('share_profile', { profile: 'text' });
  await until(async () => { const s = await nat.snap(); return s.share.state === 'live' && s.share.profile === 'text' && s.share.height === 1440 ? s : null; }, 15000, 'text live again');

  // ------------------------------------------------------------ one app's sound, beside the share
  await nat.cmd('share_audio', { stream: la.id });
  const aud = await soft(until(async () => { const s = await nat.snap(); return s.share.audio && s.share.audio.sid ? s : null; }, 10000, 'audio published'));
  const aTrack = await natTrack('screen_share_audio');
  const heardA = await toneOk(HZ_A), notB = await tone(HZ_B);
  check('10 the chosen app\'s sound arrives as a second, encrypted audio track named music-audio', aud.share && aud.share.audio && aud.share.audio.app === 'AlphaApp' && aud.share.audio.encrypted && aTrack.present && aTrack.name === 'music-audio' && aTrack.enc, { audio: aud.share && aud.share.audio, track: aTrack });
  check('11 Alice hears that app\'s tone and not the other app\'s', heardA.present && heardA.toneShare > TONE_MIN && notB.present && notB.toneShare < TONE_MAX, { appA600: heardA.toneShare, appB1500: notB.toneShare });
  const lvl = await nat.snap();
  check('12 the meter level moves and the screen share carried on beside it', lvl.share.audio && lvl.share.audio.level > 0 && !lvl.share.audio.silent && lvl.share.state === 'live', lvl.share.audio);
  await nat.cmd('share_audio', { stream: lb.id });
  const heardB = await toneOk(HZ_B), notA = await tone(HZ_A);
  check('13 choosing the other app moves the track over to its sound', heardB.present && heardB.toneShare > TONE_MIN && notA.present && notA.toneShare < TONE_MAX, { appB1500: heardB.toneShare, appA600: notA.toneShare });
  await nat.cmd('share_audio_stop');
  const aGone = await gone('screen_share_audio'); const sAfter = await nat.snap();
  check('14 stopping the sound removes the track and leaves the share running', aGone && !sAfter.share.audio && sAfter.share.state === 'live');
  await nat.cmd('share_stop');
  check('15 stopping the share removes the tile for Alice and the snapshot is idle', await gone('screen_share') && (await nat.snap()).share.state === 'idle');

  // ------------------------------------------------------------ audio attached to a share
  await nat.cmd('share', { profile: 'text', source: 'test', audio: la.id });
  await until(async () => { const s = await nat.snap(); return s.share.state === 'live' && s.share.audio && s.share.audio.sid ? s : null; }, 15000, 'share with audio live');
  const both = [(await natTrack('screen_share')).present, (await natTrack('screen_share_audio')).present];
  const heardAtt = await toneOk(HZ_A);
  check('16 a share started with an app\'s sound carries both tracks', both[0] && both[1] && heardAtt.toneShare > TONE_MIN, { tracks: both, toneShare: heardAtt.toneShare });
  await nat.cmd('share_stop');
  const bothGone = (await gone('screen_share')) && (await gone('screen_share_audio'));
  check('17 stopping that share stops its sound too', bothGone && !(await nat.snap()).share.audio);

  // ------------------------------------------------------------ the system ending it
  await nat.cmd('share', { profile: 'text', source: 'test' });
  await until(async () => { const s = await nat.snap(); return s.share.state === 'live' ? s : null; }, 15000, 'live before end');
  await nat.cmd('share_end');
  const sEnd = await soft(until(async () => { const s = await nat.snap(); return s.share.ended ? s : null; }, 8000, 'ended event'));
  check('18 the system ending the share raises an event (reason system) and the tile goes', sEnd.share && sEnd.share.ended && sEnd.share.ended.what === 'screen' && sEnd.share.ended.reason === 'system' && sEnd.share.state === 'idle' && await gone('screen_share'), sEnd.share && sEnd.share.ended);

  // ------------------------------------------------------------ the app closing its stream
  await nat.cmd('share_audio', { stream: la.id });
  await until(async () => { const s = await nat.snap(); return s.share.audio && s.share.audio.sid ? s : null; }, 10000, 'audio again');
  try { process.kill(-appA.pid, 'SIGKILL'); } catch { try { process.kill(appA.pid, 'SIGKILL'); } catch { } }
  const sGone = await soft(until(async () => { const s = await nat.snap(); return s.share.ended && s.share.ended.what === 'audio' ? s : null; }, 10000, 'app gone event'));
  check('19 the app closing its sound raises an event (reason app-gone) and the track goes', sGone.share && sGone.share.ended && sGone.share.ended.reason === 'app-gone' && !sGone.share.audio && await gone('screen_share_audio'), sGone.share && sGone.share.ended);

  // ------------------------------------------------------------ the portal, where there is none
  await nat.cmd('share', { profile: 'text', source: 'portal' });
  const sPortal = await soft(until(async () => { const s = await nat.snap(); return s.share.ended && s.share.ended.what === 'screen' && s.share.ended.seq > (sEnd.share.ended.seq) ? s : null; }, 10000, 'portal error'));
  check('20 the portal source with no desktop session says so, plainly, and nothing is published', sPortal.share && sPortal.share.ended && sPortal.share.ended.reason === 'error' && /portal|bus/i.test(sPortal.share.ended.msg) && sPortal.share.state === 'idle' && !(await natTrack('screen_share', false)).present, sPortal.share && sPortal.share.ended);

  // ------------------------------------------------------------ the media server restarts mid-share
  await nat.cmd('share', { profile: 'text', source: 'test', audio: lb.id });
  const before = await until(async () => { const s = await nat.snap(); return s.share.state === 'live' && s.share.audio && s.share.audio.sid ? s : null; }, 15000, 'live before restart');
  await lk.stopLivekit(); await sleep(2500); lk.startLivekit();
  const tBack = Date.now();
  const back = await soft(until(async () => { const s = await nat.snap(); return s.phase === 'connected' && s.share.state === 'live' && s.share.audio && s.share.audio.sid && s.share.sid !== before.share.sid && s.share.audio.sid !== before.share.audio.sid ? s : null; }, 60000, 'share back after restart'));
  const backTone = back.share ? await toneOk(HZ_B) : {};
  const backTrack = back.share ? await natTrack('screen_share', true) : {};
  check('22 after the media server restarts, the same capture and sound are published again into the new session', back.share && back.share.state === 'live' && backTrack.present && backTone.toneShare > TONE_MIN, { seconds: (Date.now() - tBack) / 1000, share: back.share && { sid: back.share.sid, audio: back.share.audio && back.share.audio.sid }, tone: backTone.toneShare, events: back.events && back.events.slice(-4) });
  await nat.cmd('share_stop');

  // ------------------------------------------------------------ leaving ends everything
  await nat.cmd('share', { profile: 'motion', source: 'test', audio: lb.id });
  await until(async () => { const s = await nat.snap(); return s.share.state === 'live' && s.share.audio && s.share.audio.sid ? s : null; }, 15000, 'live for leave');
  await nat.cmd('leave');
  await sleep(2500);
  const sLeft = await nat.snap();
  check('21 leaving the call ends the share and its sound', sLeft.phase === 'idle' && sLeft.share.state === 'idle' && !sLeft.share.audio, { phase: sLeft.phase, share: sLeft.share });

  await nat.cmd('quit').catch(() => { });
  await Promise.race([new Promise(r => nat.proc.on('exit', r)), sleep(5000)]);
} catch (e) { res.error = String(e.stack || e); console.log('ERROR', res.error); failed++; }
res.failed = failed;
fs.writeFileSync(path.join(out, 'result.json'), JSON.stringify(res, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED', '->', path.join(out, 'result.json'));
stopAll(); process.exit(failed ? 1 : 0);
