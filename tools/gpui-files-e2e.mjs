// Files, pictures, video and sound in the native window (app/gpui/src/files.rs, xfer.rs, media.rs, playback.rs) against the web client
// (app/ui/app.js uploadFile/uploadLarge/downloadLarge, lightbox, mega-voice.js).
// Nat is the native WINDOW (`mega-gpui`) in a private headless KWin, driven over its test port (uitest.rs `files_*`) and with REAL key and
// pointer events (examples/fakekeys.rs); Alice and Bob are the real app/ui in headless Chrome for Testing. One local relay and LiveKit,
// throwaway accounts. SILENT: the window plays through MEGA_GPUI_AUDIO_SINK=null (decoded and paced, never played), Chrome has no output.
// A drop from a file manager cannot be made in a headless compositor, so it is simulated at the window's own handler (`files_send` is
// what GPUI's ExternalPaths drop calls); paste is real: the picture goes on the window's clipboard and Ctrl+V is a real key.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin mega-gpui --bin commons-relay --bin dev_bridge
//   cargo build -p mega-gpui --example fakekeys
//   COMMONS_TEST_PORT_OFFSET=4300 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-files-e2e.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import crypto from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-files-e2e');
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
spawnSync('python3', ['-c', `
from PIL import Image, ImageDraw
im = Image.new('RGB', (600, 400)); d = ImageDraw.Draw(im)
d.rectangle([0,0,299,399], fill=(230,120,30)); d.rectangle([300,0,599,399], fill=(30,150,160))
d.ellipse([220,120,380,280], fill=(255,255,255)); d.rectangle([0,0,40,399], fill=(20,20,40))
im.save('${fx}/landscape.png')
im = Image.new('RGB', (300, 500)); d = ImageDraw.Draw(im)
d.rectangle([0,0,299,249], fill=(120,40,200)); d.rectangle([0,250,299,499], fill=(240,220,60)); d.ellipse([100,200,200,300], fill=(255,255,255))
im.save('${fx}/tall.jpg', quality=92)
im = Image.new('RGB', (1800, 1000)); d = ImageDraw.Draw(im)
for i in range(18): d.rectangle([i*100,0,i*100+99,999], fill=(i*14, 255-i*14, 128))
d.ellipse([700,300,1100,700], fill=(255,255,255))
im.save('${fx}/wide.png')
open('${fx}/notes.txt','w').write('hello from a text file\\n' * 40)
import os
open('${fx}/big.bin','wb').write(os.urandom(50*1024*1024))
open('${fx}/big2.bin','wb').write(os.urandom(40*1024*1024))
`]);
ff('-f', 'lavfi', '-i', 'testsrc=duration=3:size=320x240:rate=25', '-f', 'lavfi', '-i', 'sine=frequency=440:duration=3', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-shortest', `${fx}/clip.mp4`);
ff('-f', 'lavfi', '-i', 'sine=frequency=523:duration=4', '-c:a', 'libmp3lame', `${fx}/tone.mp3`);
ff('-f', 'lavfi', '-i', 'testsrc2=duration=3:size=320x180:rate=25', '-f', 'lavfi', '-i', 'sine=frequency=330:duration=3', '-c:v', 'libvpx', '-b:v', '300k', '-c:a', 'libopus', '-shortest', `${fx}/webclip.webm`);
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
    MEGA_GPUI_UI_TEST: String(winPort), MEGA_GPUI_AUDIO_SINK: 'null', MEGA_GPUI_SIZE: '1280x860', RUST_LOG: 'info' }, path.join(out, 'nat.log'));
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

  // ---------------------------------------------------------------- A. native -> web
  // the drop overlay while files are over the room, beside the web's `dropping` composer
  await W.cmd('files_drag', { on: true }); await sleep(700); await shot('01-drop-overlay'); await W.cmd('files_drag', { on: false });
  await web(B, `(document.querySelector('#composer').classList.add('dropping'), true)`); await sleep(400); await wshot(B, '01-drop-overlay'); await web(B, `(document.querySelector('#composer').classList.remove('dropping'), true)`);

  // several files at once: two pictures and a text file
  await W.cmd('files_send', { paths: [path.join(fx, 'landscape.png'), path.join(fx, 'tall.jpg'), path.join(fx, 'notes.txt')] });
  const got1 = await soft(until(async () => { const l = await fileMsgs(A, N.account); return l.length >= 3 ? l : null; }, 60000, 'alice gets three files', 500));
  check('A1 three files dropped at once all arrive for Alice (names, in order)', !timedOut(got1) && got1.map(m => m.file.name).join() === 'landscape.png,tall.jpg,notes.txt', timedOut(got1) ? got1 : got1.map(m => m.file.name));
  if (!timedOut(got1)) {
    check('A2 types and sizes are what the web would send (image/png, image/jpeg, text/plain; no resizing)', got1[0].file.mime === 'image/png' && got1[1].file.mime === 'image/jpeg' && got1[2].file.mime === 'text/plain' && got1.every((m, i) => m.file.size === fs.statSync(path.join(fx, ['landscape.png', 'tall.jpg', 'notes.txt'][i])).size), got1.map(m => [m.file.mime, m.file.size]));
    const b0 = await getBytes(A, got1[0].id), b1 = await getBytes(A, got1[1].id);
    check('A3 the bytes are identical to the files (sha256)', sha(b0) === shaFile(path.join(fx, 'landscape.png')) && sha(b1) === shaFile(path.join(fx, 'tall.jpg')));
    const dom = await soft(until(async () => web(A, `(() => { const i = [...document.querySelectorAll('.imgslot img')].filter(x => x.complete && x.naturalWidth > 0); return i.length >= 2 ? i.map(x => [x.naturalWidth, x.naturalHeight]) : null; })()`), 30000, 'web draws the pictures', 700));
    check('A4 the web client shows both pictures inline at their own size (600x400, 300x500)', !timedOut(dom) && J(dom).includes('[600,400]') && J(dom).includes('[300,500]'), dom);
  }
  // the window shows its own sent pictures
  const picsHeld = await soft(until(async () => { const d = await D(); const r = d.pics.filter(p => p.state === 'ready'); return r.length >= 2 ? r : null; }, 30000, 'window decodes the pictures', 500));
  check('A5 the window shows the pictures it sent (decoded off the UI thread, own size kept)', !timedOut(picsHeld) && J(picsHeld.map(p => [p.w, p.h])).includes('[600,400]') && J(picsHeld.map(p => [p.w, p.h])).includes('[300,500]'), picsHeld);
  await sleep(800); await shot('02-pictures'); await webScroll(A); await sleep(500); await wshot(A, '02-pictures');

  // a picture pasted: real Ctrl+V with the picture on the clipboard
  await fk('move:640,420', 'click'); await sleep(400);   // a real press gives the window the serial Wayland wants before it may own the clipboard
  await W.cmd('files_clip_image', { path: path.join(fx, 'wide.png') }); await sleep(700);
  await fk('key:ctrl+v'); await sleep(2500);
  const pasted = await soft(until(async () => { const l = await fileMsgs(A, N.account); return l.find(m => /^Pasted image \d{4}-\d\d-\d\d \d\d\.\d\d\.\d\d\.png$/.test(m.file.name)) || null; }, 45000, 'alice gets the pasted picture', 500));
  check('A6 Ctrl+V with a picture on the clipboard sends it, named like the web names a pasted picture', !timedOut(pasted), timedOut(pasted) ? pasted : pasted.file.name);
  if (!timedOut(pasted)) {
    const pb = await getBytes(A, pasted.id);
    const dims = JSON.parse(spawnSync('python3', ['-c', `import io,json,sys;from PIL import Image;im=Image.open(io.BytesIO(sys.stdin.buffer.read()));print(json.dumps([im.width,im.height,im.format]))`], { input: pb }).stdout.toString() || '[]');
    check('A7 the pasted picture arrives whole (1800x1000 PNG, not resized or recompressed by the window)', J(dims) === '[1800,1000,"PNG"]' && pb.length === fs.statSync(path.join(fx, 'wide.png')).size, dims);
  }
  // text paste still works: nothing on the clipboard that is a picture is not taken
  await W.cmd('files_clip_text', { text: 'just words' }); await sleep(400);
  check('A6b text on the clipboard is not taken as a file: Ctrl+V leaves it to the box', (await W.cmd('files_paste')).took === false);

  // video and audio
  await W.cmd('files_send', { paths: [path.join(fx, 'clip.mp4'), path.join(fx, 'tone.mp3')] });
  const got2 = await soft(until(async () => { const l = (await fileMsgs(A, N.account)).filter(m => /^(clip\.mp4|tone\.mp3)$/.test(m.file.name)); return l.length >= 2 ? l : null; }, 60000, 'alice gets the video and the audio', 500));
  check('A8 a video and an audio file arrive with their types (video/mp4, audio/mpeg), bytes identical', !timedOut(got2) && got2.some(m => m.file.mime === 'video/mp4') && got2.some(m => m.file.mime === 'audio/mpeg')
    && sha(await getBytes(A, got2.find(m => m.file.name === 'clip.mp4').id)) === shaFile(path.join(fx, 'clip.mp4')), timedOut(got2) ? got2 : got2.map(m => m.file.mime));
  const webMedia = await soft(until(async () => web(A, `(() => { const v = document.querySelector('.mediaslot.video'), a = document.querySelector('.mediaslot.audio'); return v && a ? { video: !!v.querySelector('.media-play'), audio: !!a.querySelector('.media-play') } : null; })()`), 30000, 'web media slots', 700));
  check('A9 the web client lists them as playable video and audio (Play buttons)', !timedOut(webMedia) && webMedia.video && webMedia.audio, webMedia);
  // the window's poster frame for its own video
  const post = await soft(until(async () => { const d = await D(); const m = d.media.find(x => x.state === 'ready' && x.poster); return m || null; }, 40000, 'poster', 500));
  check('A10 the window reads the video\'s first frame (poster 320x240) and length (about 3 s)', !timedOut(post) && post.poster[0] === 320 && post.poster[1] === 240 && Math.abs(post.dur_ms - 3000) < 400 && post.video && post.audio, post);
  await sleep(700); await shot('03-video-audio'); await webScroll(A); await sleep(500); await wshot(A, '03-video-audio');

  // ---------------------------------------------------------------- B. large file from the window: progress, pause, resume, cancel
  await W.cmd('files_send', { paths: [path.join(fx, 'big.bin')] });
  const prog = await soft(until(async () => { const d = await D(); const x = d.xfers.find(x => x.name === 'big.bin'); return x && x.bytes > 4 * 1024 * 1024 ? x : null; }, 60000, 'upload progresses', 150));
  check('B1 a 50 MB file goes the large way: a transfer with progress appears (resumable upload, not one message)', !timedOut(prog) && prog.total === 50 * 1024 * 1024 && prog.kind === 'up', prog);
  await shot('04-upload-in-progress');
  await W.cmd('files_pause', { name: 'big.bin' });
  await sleep(2500);
  const p1 = (await D()).xfers.find(x => x.name === 'big.bin'); await sleep(2500); const p2 = (await D()).xfers.find(x => x.name === 'big.bin');
  check('B2 pause stops it (no more bytes move; state says paused/idle)', p1 && p2 && p1.paused && p2.paused && p1.bytes === p2.bytes, { p1: p1 && p1.bytes, p2: p2 && p2.bytes, idle: p2 && p2.idle });
  await shot('05-upload-paused');
  await W.cmd('files_pause', { name: 'big.bin' });
  const done = await soft(until(async () => { const l = (await fileMsgs(A, N.account)).find(m => m.file.name === 'big.bin'); return l || null; }, 120000, 'big file arrives', 800));
  check('B3 resume carries on and the file arrives for Alice as a large file of 50 MB', !timedOut(done) && done.file.large && done.file.size === 50 * 1024 * 1024, timedOut(done) ? done : done.file);
  if (!timedOut(done)) {
    const dl = path.join(out, 'alice-dl'); fs.mkdirSync(dl, { recursive: true });
    const st = await rpc(A.port, 'file_download_begin', { gid: text.gid, id: done.id, folder: dl });
    let r; do { r = await rpc(A.port, 'file_download_chunk', { gid: text.gid, id: done.id }); } while (!r.finished);
    check('B4 Alice downloads it from the web client\'s API and every byte matches (sha256)', shaFile(r.path) === shaFile(path.join(fx, 'big.bin')), r.path);
  }
  // cancel: the first press asks (something is already on the relay), the second removes it
  await W.cmd('files_send', { paths: [path.join(fx, 'big2.bin')] });
  await until(async () => { const x = (await D()).xfers.find(x => x.name === 'big2.bin'); return x && x.bytes > 2 * 1024 * 1024; }, 60000, 'second upload progresses', 150);
  await W.cmd('files_pause', { name: 'big2.bin' }); await sleep(1500);
  const ask = await W.cmd('files_cancel', { name: 'big2.bin' });
  check('B5 cancel with part of the file already sent asks first (and "keep it" goes on)', ask.asked === true && (await D()).xfers.find(x => x.name === 'big2.bin').confirm === true);
  await shot('06-cancel-asks');
  await W.cmd('files_keep', { name: 'big2.bin' });
  await W.cmd('files_cancel', { name: 'big2.bin', sure: true });
  const gone = await soft(until(async () => { const d = await D(); return !d.xfers.some(x => x.name === 'big2.bin') && d.toasts.includes('Upload cancelled.') ? true : null; }, 30000, 'cancelled'));
  const big2 = (await fileMsgs(A, N.account)).find(m => m.file.name === 'big2.bin');
  const tr = await W.core('file_transfers');
  check('B6 cancelling removes it: no message for Alice, nothing left in the core\'s transfers', gone === true && !big2 && !tr.uploads.some(u => u.name === 'big2.bin'), { tr: tr.uploads.map(u => u.name) });
  // resumable after the window is closed: the core keeps what is on the relay; the same file offered again carries on, a different one is refused
  await W.cmd('files_send', { paths: [path.join(fx, 'big2.bin')] });
  await until(async () => { const x = (await D()).xfers.find(x => x.name === 'big2.bin'); return x && x.bytes >= 8 * 1024 * 1024; }, 60000, 'third upload progresses', 150);
  await W.cmd('files_pause', { name: 'big2.bin' }); await until(async () => (await D()).xfers.find(x => x.name === 'big2.bin').idle, 20000, 'idle');
  try { process.kill(-nat.pid, 'SIGTERM'); } catch { } await sleep(2500);
  nat = startNat(); W = new Win(winPort);
  await until(async () => { try { await W.connect(); return true; } catch { return false; } }, 30000, 'window test port again');
  await until(async () => (await W.cmd('core', { method: 'snapshot' })).ok, 30000, 'window core unlocked again');
  await W.cmd('open', { name: text.name }); await sleep(3500);
  const parked = await soft(until(async () => { const d = await D(); return d.parked.uploads.find(u => u.name === 'big2.bin') || null; }, 40000, 'parked upload', 500));
  check('B7 after the window was closed mid-upload the transfer is found again in Transfers (paused, what is safe on the relay kept)', !timedOut(parked) && parked.paused === true && parked.uploaded >= 1, parked);
  await shot('06b-parked-upload');
  await W.cmd('files_resume', { name: 'big2.bin', path: path.join(fx, 'tall.jpg') }); await sleep(1500);
  const refused = (await D()).toasts.filter(t => /not big2.bin/.test(t));
  check('B8 a different file offered for the continue is refused ("That is tall.jpg, not big2.bin.") and nothing is sent', refused.length === 1 && !(await D()).xfers.some(x => x.name === 'big2.bin'), refused);
  await W.cmd('files_resume', { name: 'big2.bin', path: path.join(fx, 'big2.bin') });
  const fin = await soft(until(async () => (await fileMsgs(A, N.account)).find(m => m.file.name === 'big2.bin') || null, 120000, 'resumed upload arrives', 800));
  check('B9 the same file offered again carries on from the saved part and arrives for Alice', !timedOut(fin) && fin.file.large && fin.file.size === 40 * 1024 * 1024, timedOut(fin) ? fin : fin.file);
  if (!timedOut(fin)) {
    const dl2 = path.join(out, 'alice-dl2'); fs.mkdirSync(dl2, { recursive: true });
    await rpc(A.port, 'file_download_begin', { gid: text.gid, id: fin.id, folder: dl2 });
    let r2; do { r2 = await rpc(A.port, 'file_download_chunk', { gid: text.gid, id: fin.id }); } while (!r2.finished);
    check('B10 and its bytes are the file\'s (sha256)', shaFile(r2.path) === shaFile(path.join(fx, 'big2.bin')));
  }
  // a web web upload of the same size, for the beside shot
  await web(A, `(async () => { const f = new File([new Uint8Array(30 * 1024 * 1024).map((_, i) => (i * 31) & 255)], 'webbig.bin', { type: 'application/octet-stream' }); uploadFiles([f]); return true; })()`);
  await sleep(2500); await wshot(A, '04-upload-in-progress');

  // ---------------------------------------------------------------- C. web -> window
  await W.cmd('open', { name: text.name }); await sleep(500);
  const webSend = async (srcName, name, mime) => {
    const b = fs.readFileSync(path.join(fx, srcName)).toString('base64');
    return web(A, `(async () => { const bin = atob(${J(b)}); const u = new Uint8Array(bin.length); for (let i = 0; i < bin.length; i++) u[i] = bin.charCodeAt(i); await uploadFile(new File([u], ${J(name)}, { type: ${J(mime)} })); return true; })()`);
  };
  await webSend('wide.png', 'from-alice.png', 'image/png');
  await webSend('webclip.webm', 'from-alice.webm', 'video/webm');
  await webSend('tone.mp3', 'from-alice.mp3', 'audio/mpeg');
  // a voice message exactly as mega-voice.js sends it: the recording and its waveform
  await rpc(A.port, 'send_file', { gid: text.gid, name: 'Voice message 12.00.00.webm', mime: 'audio/webm', data: fs.readFileSync(path.join(fx, 'voice.webm')).toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, ''), caption: '', voice: { ms: 5000, peaks } });
  const nat1 = await soft(until(async () => { const d = await D(); return ['from-alice.png', 'from-alice.webm', 'from-alice.mp3'].every(n => d.msgs.some(m => m.name === n)) && d.msgs.some(m => m.voice) ? d : null; }, 60000, 'window lists the web files', 500));
  check('C1 the window receives the web client\'s picture, video, audio and voice message', !timedOut(nat1), timedOut(nat1) ? nat1 : nat1.msgs.map(m => m.name));
  const pic = await soft(until(async () => { const d = await D(); const m = d.msgs.find(x => x.name === 'from-alice.png'); const p = m && d.pics.find(x => x.id === m.id && x.state === 'ready'); return p || null; }, 40000, 'window decodes the web picture', 500));
  check('C2 the picture is decoded at its own size (1800x1000)', !timedOut(pic) && pic.w === 1800 && pic.h === 1000, pic);
  const vposter = await soft(until(async () => { const d = await D(); const m = d.msgs.find(x => x.name === 'from-alice.webm'); const p = m && d.media.find(x => x.id === m.id && x.state === 'ready' && x.poster); return p || null; }, 40000, 'poster of the web video', 500));
  check('C3 the web client\'s WebM video shows its first frame (320x180)', !timedOut(vposter) && vposter.poster[0] === 320 && vposter.poster[1] === 180, vposter);
  await W.cmd('scroll', { end: true }); await sleep(900); await shot('07-web-files-in-window'); await webScroll(A); await wshot(A, '07-web-files-in-window');

  // the lightbox, opened with a real click on the picture
  const hits0 = (await D()).hits; const pid = (await D()).msgs.find(m => m.name === 'from-alice.png').id;
  let hit = hits0['f-img:' + pid];
  for (let i = 0; i < 12 && !(hit && hit.y > 60 && hit.y + hit.h < 780); i++) { await W.cmd('scroll', { dy: !hit || hit.y < 60 ? 220 : -220 }); await sleep(500); hit = (await D()).hits['f-img:' + pid]; }
  check('C4 the picture\'s place on screen is known to the window (for a real click)', !!hit, hit);
  if (hit) {
    await fk(`move:${Math.round(hit.x + hit.w / 2)},${Math.round(hit.y + hit.h / 2)}`, 'click'); await sleep(900);
    const lb = (await D()).light;
    check('C5 a real click on the picture opens the lightbox (that picture; pictures of the room in order)', lb && lb.id === pid && lb.n >= 4, lb);
    await sleep(1200); await shot('08-lightbox');
    await W.cmd('files_open', { name: 'from-alice.png' });
    await web(A, `(() => { const i = [...document.querySelectorAll('.imgslot img')].pop(); i.click(); return true; })()`); await sleep(900); await wshot(A, '08-lightbox'); await web(A, `(document.querySelector('.lightbox').click(), true)`);
    const z0 = (await D()).light.zoom;
    await W.cmd('files_light', { do: 'in' }); await W.cmd('files_light', { do: 'in' }); await sleep(500);
    const z1 = (await D()).light.zoom;
    await shot('09-lightbox-zoomed');
    await fk('key:-'); await sleep(300);
    const z2 = (await D()).light.zoom;
    await fk('key:0'); await sleep(300);
    const z3 = (await D()).light.zoom;
    check('C6 zoom in grows the picture, - shrinks it and 0 puts it back (real keys)', z1 > z0 && z2 < z1 && z3 === 1, { z0, z1, z2, z3 });
    const at0 = (await D()).light.at;
    await fk('key:left'); await sleep(900);
    const l1 = (await D()).light;
    check('C7 the arrow keys move between the room\'s pictures', l1.at === at0 - 1, { at0, at1: l1.at });
    await fk('key:right'); await sleep(600);
    await fk('key:escape'); await sleep(600);
    check('C8 Esc closes the lightbox', (await D()).light === null);
    // a click on the dark closes it too
    await fk(`move:${Math.round(hit.x + hit.w / 2)},${Math.round(hit.y + hit.h / 2)}`, 'click'); await sleep(700);
    await fk('move:30,420', 'click'); await sleep(600);
    check('C9 a click on the dark outside the picture closes it (as the page\'s)', (await D()).light === null);
  }

  // audio plays: the window's clock moves, pause holds it, seek moves it, nothing is heard (null sink)
  await W.cmd('files_play', { name: 'from-alice.mp3' });
  const pl1 = await soft(until(async () => { const d = await D(); return d.play.playing && d.play.status && d.play.status.pos_ms > 500 ? d.play.status : null; }, 20000, 'audio plays', 200));
  check('C10 an audio file plays in the window (decrypted, ffmpeg-decoded, null sink): position moves in real time', !timedOut(pl1), pl1);
  await sleep(1200); const pl2 = (await D()).play.status;
  check('C11 the position advances at about real time (1.2 s of waiting moves it 0.9 to 1.7 s)', pl2 && pl2.pos_ms - pl1.pos_ms > 700 && pl2.pos_ms - pl1.pos_ms < 1900, { a: pl1.pos_ms, b: pl2.pos_ms });
  await shot('10-audio-playing');
  await W.cmd('files_play', { name: 'from-alice.mp3' }); await sleep(500);
  const held1 = (await D()).play; await sleep(1200); const held2 = (await D()).play;
  check('C12 pause stops it and remembers the place', !held1.playing && held1.pos['' + (await D()).msgs.find(m => m.name === 'from-alice.mp3').id] > 500 && J(held1.pos) === J(held2.pos), held1.pos);
  await W.cmd('files_seek', { name: 'from-alice.mp3', frac: 0.75 }); await sleep(300);
  const sk = (await D()).play.pos; const audioId = (await D()).msgs.find(m => m.name === 'from-alice.mp3').id;
  check('C13 seeking by the bar moves the place (75% of 4 s)', Math.abs(sk[audioId] - 3000) < 400, sk);
  await W.cmd('files_play', { name: 'from-alice.mp3' });
  const ended = await soft(until(async () => { const d = await D(); return !d.play.playing && d.play.pos[audioId] === 0 ? true : null; }, 15000, 'audio ends', 300));
  check('C14 played to the end it stops and goes back to the start', ended === true);
  // the voice message: waveform row, plays by its own length
  await W.cmd('scroll', { end: true }); await sleep(500);
  const voiceName = (await D()).msgs.find(m => m.voice).name;
  await W.cmd('files_play', { name: voiceName });
  const vp = await soft(until(async () => { const d = await D(); return d.play.playing && d.play.status && d.play.status.pos_ms > 400 ? d.play.status : null; }, 20000, 'voice plays', 200));
  check('C15 a voice message plays (Opus in WebM, as the web records it)', !timedOut(vp), vp);
  await sleep(600); await shot('11-voice-playing'); await webScroll(A); await wshot(A, '11-voice');
  await W.cmd('files_play', { name: voiceName });
  // video plays: frames reach the window
  await W.cmd('files_play', { name: 'from-alice.webm' });
  const vv = await soft(until(async () => { const d = await D(); return d.play.playing && d.play.status && d.play.status.frames > 5 ? d.play.status : null; }, 30000, 'video frames', 200));
  check('C16 a video plays in its row: frames are decoded, paced and shown (>5 frames in the first moments)', !timedOut(vv), vv);
  await sleep(500); await shot('12-video-playing');
  await W.cmd('files_play', { name: 'from-alice.webm' });

  // ---------------------------------------------------------------- D. saving
  const sv = path.join(out, 'nat-saved'); fs.mkdirSync(sv, { recursive: true });
  await W.cmd('files_download', { name: 'from-alice.png' }); await sleep(600);
  const card = (await D()).save;
  await shot('13-save-to');
  check('D1 Download opens the "save to" card for the file', card && card.name === 'from-alice.png', card);
  await W.cmd('files_save', { folder: sv });
  const saved1 = await soft(until(async () => fs.existsSync(path.join(sv, 'from-alice.png')) ? true : null, 20000, 'saved', 300));
  check('D2 a small file is saved into the chosen folder, bytes identical', saved1 === true && shaFile(path.join(sv, 'from-alice.png')) === shaFile(path.join(fx, 'wide.png')));
  // a large file from the web: not fetched until Download; progress; saved
  await web(A, `(async () => { const u = new Uint8Array(40 * 1024 * 1024); for (let i = 0; i < u.length; i += 4096) u[i] = (i / 4096) & 255; await uploadFile(new File([u], 'alice-large.bin', { type: 'application/octet-stream' })); return true; })()`);
  const lg = await soft(until(async () => { const d = await D(); return d.msgs.find(m => m.name === 'alice-large.bin' && m.large) || null; }, 120000, 'window lists the large file', 700));
  check('D3 a large file from the web shows as a file row (Download), nothing fetched yet', !timedOut(lg) && !(await D()).pics.some(p => p.id === lg.id), lg);
  await W.cmd('scroll', { end: true }); await sleep(700); await shot('14-large-file-row'); await webScroll(A); await wshot(A, '14-large-file-row');
  await W.cmd('files_download', { name: 'alice-large.bin' }); await sleep(500);
  await W.cmd('files_save', { folder: sv });
  const dprog = await soft(until(async () => { const d = await D(); const x = d.xfers.find(x => x.kind === 'down'); return x && x.bytes > 0 ? x : null; }, 40000, 'download progresses', 100));
  check('D4 the large download runs with progress (a transfer, bytes moving)', !timedOut(dprog), dprog);
  await shot('15-download-progress');
  const dd = await soft(until(async () => { const d = await D(); const m = d.msgs.find(m => m.name === 'alice-large.bin'); return m && m.download && m.download.finished ? m.download : null; }, 120000, 'download finishes', 500));
  const want = Buffer.alloc(40 * 1024 * 1024); for (let i = 0; i < want.length; i += 4096) want[i] = (i / 4096) & 255;
  check('D5 it finishes into the folder, byte for byte (sha256)', !timedOut(dd) && sha(fs.readFileSync(dd.path)) === sha(want) && path.dirname(dd.path) === sv, timedOut(dd) ? dd : dd.path);
  await sleep(1500); await shot('16-saved-row');

  // the earlier window files, for the record
  await W.cmd('scroll', { end: true }); await sleep(500);
  res.finalToasts = (await D()).toasts;
  await W.cmd('quit').catch(() => { });
} catch (e) { res.error = String(e.stack || e); console.log('ERROR', res.error); failed++; }
res.failed = failed;
fs.writeFileSync(path.join(out, 'result.json'), JSON.stringify(res, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED', '->', path.join(out, 'result.json'));
stopAll(); process.exit(failed ? 1 : 0);
