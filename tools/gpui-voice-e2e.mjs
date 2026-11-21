// Voice clips in the native window (app/gpui/src/voice.rs) against the web client (app/ui/mega-voice.js). Nat is the native window in a
// private headless KWin driven over its test port (uitest.rs voice_*) and with real pointer events; Alice and Bob are the real app/ui in
// headless Chrome. The window records a SYNTHETIC microphone (MEGA_GPUI_MIC=synthetic): the user's real mic is never opened. SILENT.
//   COMMONS_TEST_PORT_OFFSET=5900 MEGA_GPUI_BIN=<mega-gpui> systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-voice-e2e.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import crypto from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-voice-e2e');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const shots = path.join(out, 'shots'); fs.mkdirSync(shots, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const FAKE = process.env.MEGA_GPUI_FAKEKEYS || path.join(root, 'target/debug/examples/fakekeys');
const PASS = 'correct horse battery';
const J = x => JSON.stringify(x);
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? '  [' + (typeof detail === 'string' ? detail : J(detail)).slice(0, 300) + ']' : ''}`); };
async function until(fn, ms, what, step = 250) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(step); } }
const soft = p => p.catch(e => ({ timeout: String(e.message || e) }));
const timedOut = v => v && typeof v === 'object' && typeof v.timeout === 'string';
const sha = b => crypto.createHash('sha256').update(b).digest('hex');
const shaFile = f => sha(fs.readFileSync(f));

class Win {
  constructor(port) { this.port = port; this.n = 0; this.waiting = new Map(); this.buf = ''; }
  async connect() {
    this.sock = net.connect(this.port, '127.0.0.1');
    await new Promise((res, rej) => { this.sock.once('connect', res); this.sock.once('error', rej); });
    this.sock.on('data', d => { this.buf += d; let i; while ((i = this.buf.indexOf('\n')) >= 0) { const l = this.buf.slice(0, i); this.buf = this.buf.slice(i + 1); const j = JSON.parse(l); const w = this.waiting.get(j.id); if (w) { this.waiting.delete(j.id); w(j); } } });
  }
  cmd(cmd, args = {}, ms = 20000) {
    const id = ++this.n;
    return new Promise((res, rej) => { const t = setTimeout(() => { this.waiting.delete(id); rej(new Error('no answer to ' + cmd)); }, ms); this.waiting.set(id, j => { clearTimeout(t); res(j); }); this.sock.write(J({ id, cmd, ...args }) + '\n'); });
  }
  core(method, args = {}) { return this.cmd('core', { method, args }).then(r => { if (r.error) throw new Error(r.error); return r.ok; }); }
}

const priv = path.join(out, 'priv'); const xdg = { XDG_RUNTIME_DIR: path.join(priv, 'run'), XDG_CONFIG_HOME: path.join(priv, 'config'), XDG_STATE_HOME: path.join(priv, 'state'), XDG_CACHE_HOME: path.join(priv, 'cache'), XDG_DATA_HOME: path.join(priv, 'data') };
for (const d of Object.values(xdg)) fs.mkdirSync(d, { recursive: true, mode: 0o700 });
const cleanEnv = () => { const e = { ...process.env, ...xdg }; for (const k of ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'XDG_SESSION_ID', 'PIPEWIRE_RUNTIME_DIR', 'PULSE_SERVER', 'PULSE_RUNTIME_PATH']) delete e[k]; return e; };
const busConf = (addrFile) => { const c = path.join(priv, path.basename(addrFile) + '.conf'); fs.writeFileSync(c, `<busconfig><type>session</type><listen>unix:path=${path.join(priv, path.basename(addrFile) + '.sock')}</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>`); return c; };
function sh(cmd, args, env, log) { const p = spawn(cmd, args, { env, stdio: ['ignore', fs.openSync(log, 'a'), fs.openSync(log, 'a')], detached: true }); procs.push(p); return p; }

// ---- the files the people send ----
const fx = path.join(out, 'files'); fs.mkdirSync(fx, { recursive: true });
const ff = (...a) => spawnSync('ffmpeg', ['-v', 'error', '-y', ...a], { encoding: 'utf8' });
ff('-f', 'lavfi', '-i', 'sine=frequency=262:duration=5', '-c:a', 'libopus', '-f', 'webm', `${fx}/voice.webm`);
console.log('fixtures:', fs.readdirSync(fx).map(f => f + ':' + fs.statSync(path.join(fx, f)).size).join(' '));
const peaks = Array.from({ length: 64 }, (_, i) => Math.round(255 * Math.abs(Math.sin(i / 5)) * (0.3 + 0.7 * (i % 7) / 6))).map(v => v.toString(16).padStart(2, '0')).join('');

const res = { checks: results };
let nat, kw;
try {
  const lk = await startServers(out, {});
  const world = await makeWorld(out, ['alice', 'bob']);
  await openApps(world, ['alice', 'bob']);
  const A = world.people.alice, B = world.people.bob;
  const web = (p, expr) => p.page.ev(`(async()=>{ return ${expr}; })()`);
  const cid = world.cid;
  const snapA = await rpc(A.port, 'snapshot');
  const chans = snapA.communities.find(c => c.id === cid).channels;
  const text = chans.find(c => c.kind !== 'voice');
  A.account = snapA.me.account; B.account = (await rpc(B.port, 'snapshot')).me.account;

  const N = { port: P(27203) };
  const natBridge = startBridge(out, 'nat', N.port); await sleep(1200);
  await rpc(N.port, 'create_account', { name: 'Nat', passphrase: PASS, relay: `http://127.0.0.1:${RELAY_PORT}` });
  await rpc(N.port, 'join', { link: (await rpc(A.port, 'create_invite', { cid, uses: 4, ttl: 3600 })).link });
  await until(async () => { const s = await rpc(N.port, 'snapshot'); const c = s.communities.find(c => c.id === cid); return c && c.channels.some(ch => ch.gid === text.gid && ch.joined && ch.status === 'ok'); }, 90000, 'nat has the room');
  N.account = (await rpc(N.port, 'snapshot')).me.account;
  try { process.kill(-natBridge.pid, 'SIGTERM'); } catch { } await sleep(1500);

  const kwBusFile = path.join(out, 'kwin-bus');
  kw = sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket wayland-selshot --no-lockscreen --no-global-shortcuts --width 1280 --height 860`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(out, 'kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, 'wayland-selshot')), 20000, 'kwin socket');
  const winEnv = { ...cleanEnv(), WAYLAND_DISPLAY: 'wayland-selshot', DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim() };
  const shot = async name => { const f = path.join(shots, 'native-' + name + '.png'); const r = spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); return r.status === 0 ? f : null; };
  const wshot = async (p, name) => { const r = await p.page.send('Page.captureScreenshot', { format: 'png' }); fs.writeFileSync(path.join(shots, 'web-' + name + '.png'), Buffer.from(r.result.data, 'base64')); };
  const fk = async (...steps) => { const r = spawnSync(FAKE, steps, { env: winEnv, encoding: 'utf8' }); if (r.status !== 0) throw new Error('fakekeys ' + steps.join(' ') + ': ' + r.stderr); };
  const winPort = P(27310);
  const startNat = () => sh(BIN, [], { ...winEnv, COMMONS_DATA: path.join(out, 'data/nat'), COMMONS_RELAY: `http://127.0.0.1:${RELAY_PORT}`, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_SHORTCUTS: 'off',
    MEGA_GPUI_UI_TEST: String(winPort), MEGA_GPUI_AUDIO_SINK: 'null', MEGA_GPUI_MIC: 'synthetic', MEGA_GPUI_SIZE: '1280x860', RUST_LOG: 'info' }, path.join(out, 'nat.log'));
  nat = startNat();
  let W = new Win(winPort);
  await until(async () => { try { await W.connect(); return true; } catch { return false; } }, 30000, 'window test port');
  await until(async () => (await W.cmd('core', { method: 'snapshot' })).ok, 30000, 'window core unlocked');
  await sleep(3000);
  await until(async () => { const s = (await W.cmd('core', { method: 'snapshot' })).ok; const c = s && s.communities.find(c => c.id === cid); return c && c.channels.length >= 1; }, 30000, 'window has the community');
  check('0 the native window is up as Nat (private KWin, silent sink)', true, { nat: N.account.slice(0, 8) });

  const D = () => W.cmd('files_dump');
  const msgsOf = async (p, n = 60) => (await rpc(p.port, 'messages', { gid: text.gid, limit: n })).messages;
  const fileMsgs = async (p, from) => (await msgsOf(p)).filter(m => m.kind === 'file' && (!from || m.sender === from));
  const getBytes = async (p, id) => { const f = await rpc(p.port, 'file_open', { gid: text.gid, id }); const parts = []; for (let at = 0; at < f.size; at += f.chunk) parts.push(Buffer.from((await rpc(p.port, 'file_read', { gid: text.gid, id, offset: at })).data.replace(/-/g, '+').replace(/_/g, '/'), 'base64')); return Buffer.concat(parts); };
  const webRoom = async p => { await p.page.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 860, deviceScaleFactor: 1, mobile: false }); await web(p, `(() => { const L = __mega.list; L.go(L.items.findIndex(x => x.name === ${J(text.name)}), true); })()`); await sleep(1500); await p.page.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 }); await p.page.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 }); await sleep(1500); };
  const webScroll = p => web(p, `(() => { const s = document.querySelector('#stream'); s.scrollTop = s.scrollHeight; return true; })()`);
  await W.cmd('open', { name: text.name }); await sleep(2500);
  await webRoom(A); await webRoom(B);
  await rpc(B.port, 'send_text', { gid: text.gid, text: 'hello, here come the files' }); await sleep(2000);
  await shot('00-room'); await wshot(B, '00-room');

  // ---------------------------------------------------------------- native records, the web plays
  const wavInfo = b => ({ riff: b.slice(0, 4).toString(), rate: b.readUInt32LE(24), ch: b.readUInt16LE(22), bits: b.readUInt16LE(34), data: b.readUInt32LE(40) });
  const voiceMsgs = async p => (await msgsOf(p)).filter(m => m.kind === 'file' && m.file && m.file.voice);
  // 1. hold: press, wait past the hold time, release = send
  const r0 = await W.cmd('voice_dump');
  check('V0 the window has a synthetic mic and is not recording', r0.synthetic === true && r0.recording === false, r0);
  await W.cmd('voice_press', { x: 700, y: 800 }); await sleep(300);
  const r1 = await W.cmd('voice_dump');
  check('V1 pressing the mic button starts recording (pending)', r1.recording === true, r1);
  await sleep(1500); await shot('20-recording-hold');
  const r2 = await W.cmd('voice_dump');
  check('V2 held past 350 ms it is in hold mode, the clock runs, slices of sound arrive', r2.mode === 'hold' && r2.ms > 1500 && r2.slices > 25, r2);
  await W.cmd('voice_move', { x: 700, y: 600 }); await sleep(300);
  const rc = await W.cmd('voice_dump'); await shot('21-recording-cancelling');
  check('V3 sliding 90 px away marks it as cancelling', rc.cancelling === true, rc);
  await W.cmd('voice_move', { x: 705, y: 805 }); await sleep(200);
  await W.cmd('voice_release');
  await sleep(500);
  const got = await soft(until(async () => { const l = await voiceMsgs(A); return l.length ? l : null; }, 60000, 'alice gets the clip', 500));
  check('V4 releasing sends the clip: Alice receives a voice message from Nat', !timedOut(got) && got[0].sender === N.account, timedOut(got) ? got : got[0].file);
  let clip = null;
  if (!timedOut(got)) {
    clip = got[0]; const f = clip.file;
    check('V5 it is audio/wav named like the web names a clip, about 2 s long', f.mime === 'audio/wav' && /^Voice message \d{4}-\d\d-\d\d \d\d\.\d\d\.\d\d\.wav$/.test(f.name) && f.voice.ms > 1500 && f.voice.ms < 4500, f);
    const bytes = await getBytes(A, clip.id); const wi = wavInfo(bytes);
    check('V6 the file is a 16 kHz mono 16-bit WAV (what the page writes where it has no MediaRecorder) of the stated length', wi.riff === 'RIFF' && wi.rate === 16000 && wi.ch === 1 && wi.bits === 16 && Math.abs(wi.data / 32000 * 1000 - f.voice.ms) < 150, wi);
    const pk = Buffer.from(f.voice.peaks, 'hex');
    check('V7 64 waveform bytes, the loudest 255, not flat', pk.length === 64 && Math.max(...pk) === 255 && Math.min(...pk) < 200, f.voice.peaks);
    await webScroll(A); await sleep(800); await wshot(A, '20-native-clip-on-web');
    const row = await soft(until(async () => web(A, `(() => { const e = document.querySelector('.vmsg[data-vid=${J(clip.id)}]'); return e ? { nope: e.classList.contains('nope'), bars: e.querySelectorAll('.vbars i').length, ms: e.dataset.ms } : null; })()`), 20000, 'web row', 500));
    check('V8 the web draws it as a playable voice row with 64 bars', !timedOut(row) && !row.nope && row.bars >= 64, row);
    await web(A, `(document.querySelector('.vmsg[data-vid=${J(clip.id)}] .vplay').click(), true)`);
    const pl = await soft(until(async () => web(A, `(() => { const a = MegaVoice.audio(${J(clip.id)}); return a && a.currentTime > 0.3 && !a.paused ? { t: a.currentTime, dur: a.duration } : null; })()`), 20000, 'web plays', 300));
    check('V9 the web client plays the native clip (audio element advances)', !timedOut(pl), pl);
    await wshot(A, '21-native-clip-playing');
    await web(A, `(document.querySelector('.vmsg[data-vid=${J(clip.id)}] .vplay').click(), true)`);
  }
  // 2. tap = lock mode with buttons; the native row shows its own clip
  const before = (await voiceMsgs(A)).length;
  await W.cmd('voice_press', { x: 700, y: 800 }); await sleep(120); await W.cmd('voice_release'); await sleep(1400);
  const t1 = await W.cmd('voice_dump'); await shot('22-recording-lock');
  check('V10 a quick tap keeps recording with buttons (lock mode)', t1.recording && t1.mode === 'lock', t1);
  await W.cmd('voice_send');
  const got2 = await soft(until(async () => { const l = await voiceMsgs(A); return l.length > before ? l : null; }, 60000, 'second clip', 500));
  check('V11 Send in lock mode sends it (a second clip arrives)', !timedOut(got2), timedOut(got2) ? got2 : got2.length);
  // 3. cancel sends nothing; too short is refused
  await W.cmd('voice_press', { x: 700, y: 800 }); await sleep(120); await W.cmd('voice_release'); await sleep(900);
  await W.cmd('voice_cancel'); await sleep(1500);
  check('V12 cancel sends nothing', (await voiceMsgs(A)).length === before + 1, (await voiceMsgs(A)).length);
  await W.cmd('voice_press', { x: 700, y: 800 }); await sleep(100); await W.cmd('voice_release'); await sleep(100); await W.cmd('voice_send'); await sleep(1500);
  const dd = await W.cmd('voice_dump');
  check('V13 under 0.6 s is "too short", nothing sent', dd.toasts.some(t => /too short/.test(t)) && (await voiceMsgs(A)).length === before + 1, dd.toasts);
  // 4. the window lists its own clip as a waveform row and plays it
  const own = await soft(until(async () => { const d = await D(); return d.msgs.filter(m => m.voice && m.mine).length >= 2 ? d : null; }, 30000, 'own clips', 500));
  check('V14 the window shows its own clips as voice rows', !timedOut(own), timedOut(own) ? own : own.msgs.filter(m => m.voice).length);
  await W.cmd('scroll', { end: true }); await sleep(800); await shot('23-own-voice-rows');

  // ---------------------------------------------------------------- the web records, the window shows the waveform
  const webClip = fs.readFileSync(path.join(fx, 'voice.webm')).toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  await rpc(B.port, 'send_file', { gid: text.gid, name: 'Voice message 12.00.00.webm', mime: 'audio/webm', data: webClip, caption: '', voice: { ms: 5000, peaks } });
  const fromWeb = await soft(until(async () => { const d = await D(); return d.msgs.some(m => m.voice === 5000 && !m.mine) ? d : null; }, 60000, 'window gets the web clip', 500));
  check('V15 a web-recorded clip shows in the window as a voice row of 5 s', !timedOut(fromWeb), timedOut(fromWeb) ? fromWeb : '');
  await W.cmd('scroll', { end: true }); await sleep(800); await shot('24-web-clip-in-native'); await webScroll(B); await wshot(B, '24-web-clip');
  const webRowName = (await D()).msgs.filter(m => m.voice === 5000).pop().name;
  await W.cmd('files_play', { name: webRowName });
  const wp = await soft(until(async () => { const d = await D(); return d.play.playing && d.play.status && d.play.status.pos_ms > 400 ? d.play.status : null; }, 20000, 'plays', 200));
  check('V16 the window plays the web clip', !timedOut(wp), wp);
  await W.cmd('files_play', { name: webRowName });

  // ---------------------------------------------------------------- emoji sequences: picker (tones, flags, families) and rendering
  const E = () => W.cmd('emoji');
  const FAMILY = '\u{1F468}\u200D\u{1F469}\u200D\u{1F467}', JP = '\u{1F1EF}\u{1F1F5}', THUMB_MID = '\u{1F44D}\u{1F3FD}', TECH_DARK = '\u{1F469}\u{1F3FF}\u200D\u{1F4BB}';
  await W.cmd('emoji_do', { what: 'open' }); await sleep(600);
  await W.cmd('emoji_do', { what: 'query', q: 'flag japan' }); await sleep(400);
  check('E1 searching "flag japan" finds the flag', (await E()).first === JP, (await E()).first);
  await W.cmd('emoji_do', { what: 'query', q: 'family man woman girl' }); await sleep(400);
  check('E2 searching a family finds the ZWJ sequence', (await E()).first === FAMILY, (await E()).first);
  await W.cmd('emoji_do', { what: 'query', q: '' }); await W.cmd('emoji_do', { what: 'tone', n: 3 }); await sleep(500);
  await shot('30-picker-tone'); 
  await W.cmd('emoji_do', { what: 'query', q: 'thumbs up' }); await sleep(400);
  check('E3 with tone 3 chosen, "thumbs up" shows the medium tone', (await E()).first === THUMB_MID && (await E()).tone === 3, (await E()).first);
  await W.cmd('emoji_do', { what: 'query', q: 'woman technologist' }); await W.cmd('emoji_do', { what: 'tone', n: 5 }); await sleep(400);
  check('E4 a profession sequence takes the tone inside the ZWJ sequence', (await E()).first === TECH_DARK, (await E()).first);
  await W.cmd('emoji_do', { what: 'tone', n: 0 }); await W.cmd('emoji_do', { what: 'query', q: 'flag' }); await sleep(500); await shot('31-picker-flags');
  await W.cmd('emoji_do', { what: 'close' });
  const msg = `flags ${JP} family ${FAMILY} thumbs ${THUMB_MID} tech ${TECH_DARK}`;
  await W.core('send_text', { gid: text.gid, text: msg });
  await W.cmd('scroll', { end: true }); await sleep(1500); await shot('32-emoji-native');
  await webScroll(B); await sleep(1000); await wshot(B, '32-emoji-web');
  const bGot = (await msgsOf(B)).find(m => m.text === msg);
  check('E5 the web client receives the sequences intact (code points identical)', !!bGot);

  res.finalToasts = (await D()).toasts;
  await W.cmd('quit').catch(() => { });
} catch (e) { res.error = String(e.stack || e); console.log('ERROR', res.error); failed++; }
res.failed = failed;
fs.writeFileSync(path.join(out, 'result.json'), JSON.stringify(res, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED', '->', path.join(out, 'result.json'));
stopAll(); process.exit(failed ? 1 : 0);
