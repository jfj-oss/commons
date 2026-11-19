// Profile pictures in the native app (app/gpui/src/avatars.rs) against the web client (app/ui/avatar.js, app.js `fillAvatars`).
// Nat is the native WINDOW (`mega-gpui`) in a private headless KWin, driven over its test port (app/gpui/src/uitest.rs); Alice and Bob
// are the real app/ui in headless Chrome for Testing. One local relay and LiveKit, throwaway accounts, silent (the window opens no
// audio device, Chrome has a fake microphone and no output). Nobody joins a real room.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin mega-gpui --bin commons-relay --bin dev_bridge
//   COMMONS_TEST_PORT_OFFSET=3900 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-avatars-e2e.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-avatars-e2e');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const shots = path.join(out, 'shots'); fs.mkdirSync(shots, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const PASS = 'correct horse battery';
const J = x => JSON.stringify(x);
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? '  [' + (typeof detail === 'string' ? detail : J(detail)).slice(0, 300) + ']' : ''}`); };
async function until(fn, ms, what, step = 250) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(step); } }
const soft = p => p.catch(e => ({ timeout: String(e.message || e) }));
const timedOut = v => v && typeof v === 'object' && typeof v.timeout === 'string';

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

// the pictures the native user chooses from: a 600x400 landscape (orange left, teal right, a white disc and a dark bar on the left edge) and a tall one
const pics = path.join(out, 'pics'); fs.mkdirSync(pics, { recursive: true });
spawnSync('python3', ['-c', `
from PIL import Image, ImageDraw
im = Image.new('RGB', (600, 400)); d = ImageDraw.Draw(im)
d.rectangle([0,0,299,399], fill=(230,120,30)); d.rectangle([300,0,599,399], fill=(30,150,160))
d.ellipse([220,120,380,280], fill=(255,255,255)); d.rectangle([0,0,40,399], fill=(20,20,40))
im.save('${pics}/landscape.png')
im = Image.new('RGB', (300, 500)); d = ImageDraw.Draw(im)
d.rectangle([0,0,299,249], fill=(120,40,200)); d.rectangle([0,250,299,499], fill=(240,220,60)); d.ellipse([100,200,200,300], fill=(255,255,255))
im.save('${pics}/tall.jpg', quality=92)
open('${pics}/notapic.txt','w').write('hello')
`]);

const res = { checks: results };
let nat, kw;
try {
  const lk = await startServers(out, {});
  const world = await makeWorld(out, ['alice', 'bob']);
  await openApps(world, ['alice', 'bob']);
  const A = world.people.alice, B = world.people.bob;
  const web = (p, expr) => p.page.ev(`(async()=>{ return ${expr}; })()`);
  const cid = world.cid;
  const porch = (await rpc(A.port, 'create_channel', { cid, name: 'porch', kind: 'voice' })).gid;
  const snapA = await rpc(A.port, 'snapshot');
  const chans = snapA.communities.find(c => c.id === cid).channels;
  const porchGid = chans.find(c => c.name === 'porch').gid;
  const text = chans.find(c => c.kind !== 'voice');
  A.account = snapA.me.account; B.account = (await rpc(B.port, 'snapshot')).me.account;

  const N = { port: P(27203) };
  const natBridge = startBridge(out, 'nat', N.port); await sleep(1200);
  await rpc(N.port, 'create_account', { name: 'Nat', passphrase: PASS, relay: `http://127.0.0.1:${RELAY_PORT}` });
  await rpc(N.port, 'join', { link: (await rpc(A.port, 'create_invite', { cid, uses: 4, ttl: 3600 })).link });
  await until(async () => { const s = await rpc(N.port, 'snapshot'); const c = s.communities.find(c => c.id === cid); return c && c.channels.some(ch => ch.name === 'porch' && ch.joined && ch.status === 'ok') && c.channels.some(ch => ch.gid === text.gid && ch.joined && ch.status === 'ok'); }, 90000, 'nat has the rooms');
  N.account = (await rpc(N.port, 'snapshot')).me.account;
  try { process.kill(-natBridge.pid, 'SIGTERM'); } catch { } await sleep(1500);

  const kwBusFile = path.join(out, 'kwin-bus');
  kw = sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket wayland-av --no-lockscreen --no-global-shortcuts --width 1280 --height 860`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(out, 'kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, 'wayland-av')), 20000, 'kwin socket');
  const shot = async name => { const f = path.join(shots, 'native-' + name + '.png'); const r = spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); return r.status === 0 ? f : null; };
  const wshot = async (p, name) => { const r = await p.page.send('Page.captureScreenshot', { format: 'png' }); fs.writeFileSync(path.join(shots, 'web-' + name + '.png'), Buffer.from(r.result.data, 'base64')); };
  const webKey = async (p, k, vk) => { await p.page.send('Input.dispatchKeyEvent', { type: 'keyDown', key: k, code: k, windowsVirtualKeyCode: vk }); await p.page.send('Input.dispatchKeyEvent', { type: 'keyUp', key: k, code: k, windowsVirtualKeyCode: vk }); await sleep(700); };
  // the web client in the same room as the window, for side-by-side shots
  const webRoom = async p => { await p.page.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 860, deviceScaleFactor: 1, mobile: false }); await web(p, `(() => { const L = __mega.list; L.go(L.items.findIndex(x => x.name === ${J(text.name)}), true); })()`); await sleep(1500); await webKey(p, 'Enter', 13); await sleep(1500); };
  const webPeople = async (p, on) => { await web(p, `(document.querySelector('#btn-people').click(), true)`); await sleep(900); };
  const winPort = P(27310);
  nat = sh(BIN, [], { ...cleanEnv(), WAYLAND_DISPLAY: 'wayland-av', DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim(), COMMONS_DATA: path.join(out, 'data/nat'), COMMONS_RELAY: `http://127.0.0.1:${RELAY_PORT}`,
    COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_UI_TEST: String(winPort), RUST_LOG: 'info' }, path.join(out, 'nat.log'));
  const W = new Win(winPort);
  await until(async () => { try { await W.connect(); return true; } catch { return false; } }, 30000, 'window test port');
  await until(async () => (await W.cmd('core', { method: 'snapshot' })).ok, 30000, 'window core unlocked');
  await sleep(3000);
  await until(async () => { const s = (await W.cmd('core', { method: 'snapshot' })).ok; const c = s && s.communities.find(c => c.id === cid); return c && c.channels.length >= 2; }, 30000, 'window has the community');
  check('0 the native window is up as Nat (private KWin, silent)', true, { nat: N.account.slice(0, 8) });

  const held = async () => (await W.cmd('avatars')).held || [];
  const heldFor = async acct => (await held()).find(h => h.account === acct);
  const versions = async () => rpc(A.port, 'avatar_versions');
  const clubName = snapA.communities.find(c => c.id === cid).name;

  // ---------------------------------------------------------------- A. a picture set on the web shows in the window
  // some words first, so message rows exist
  await rpc(B.port, 'send_text', { gid: text.gid, text: 'hello from Bob, before any picture' });
  await W.cmd('open', { name: text.name }); await sleep(2500);
  await shot('01-rows-no-pictures');
  const setOnWeb = (p, hue) => web(p, `(async () => {
    const c = document.createElement('canvas'); c.width = 500; c.height = 360; const g = c.getContext('2d');
    g.fillStyle = 'hsl(${hue},70%,45%)'; g.fillRect(0, 0, 500, 360); g.fillStyle = '#fff'; g.beginPath(); g.arc(250, 180, 90, 0, 7); g.fill(); g.fillStyle = '#111'; g.fillRect(0, 0, 60, 360);
    const blob = await new Promise(r => c.toBlob(r, 'image/png')); const file = new File([blob], 'p.png', { type: 'image/png' });
    const src = await CommonsAvatar.decode(file); const cv = document.createElement('canvas'); const cr = new CommonsAvatar.Cropper(cv, src);
    cr.setZoom(1.3); const o = await CommonsAvatar.encode(cr.output(), 'circle'); await saveOwnAvatar(o);
    return { size: o.blob.size, type: o.blob.type, shape: o.shape }; })()`);
  const wb = await setOnWeb(B, 25);
  check('A1 Bob sets a picture on the web (its own crop and encode)', wb.size > 0 && wb.size <= 40 * 1024, wb);
  const bobPic = await soft(until(async () => { const h = await heldFor(B.account); return h && h.blob === (await versions())[B.account] ? h : null; }, 40000, 'window holds bob\'s picture', 500));
  check('A2 the window downloads and decodes Bob\'s picture (same blob id as the core\'s version)', !timedOut(bobPic) && bobPic.shape === 'circle', bobPic);
  await sleep(800); await shot('02-rows-bob-picture');
  await W.cmd('panel', { which: 'people', name: clubName }); await sleep(900); await shot('03-people');
  await W.cmd('panel', { which: 'person', name: clubName, who: 'Bob' }); await sleep(900); await shot('04-person-card');
  await W.cmd('panel', { which: 'close' });
  await webRoom(B); await wshot(B, '02-rows-bob-picture');
  await webPeople(B); await wshot(B, '03-people'); await webPeople(B);

  // ---------------------------------------------------------------- B. the cropper, then upload from the window
  await W.cmd('settings'); await sleep(900); await shot('05-settings-no-picture');
  const bad = await W.cmd('avatar_open', { path: path.join(pics, 'notapic.txt') }); await sleep(1500);
  const dBad = await W.cmd('avatars');
  check('B1 a file that is not a picture is refused with a message, no cropper', !dBad.cropping && /not a picture/.test(dBad.note || ''), dBad.note);
  await W.cmd('avatar_open', { path: path.join(pics, 'landscape.png') });
  const dOpen = await soft(until(async () => { const d = await W.cmd('avatars'); return d.cropping ? d : null; }, 10000, 'cropper opens'));
  check('B2 choosing a picture opens the cropper with its true size', !timedOut(dOpen) && dOpen.cropping.w === 600 && dOpen.cropping.h === 400 && dOpen.cropping.zoom === 1, dOpen.cropping);
  await sleep(500); await shot('06-crop-default');
  const dCrop = await W.cmd('avatar_crop', { zoom: 1.6, x: 500, y: 0, circle: false });
  check('B3 zoom is kept within 1..4 and a drag past the edge is stopped by it (the picture always covers the stage)', Math.abs(dCrop.cropping.zoom - 1.6) < 1e-3 && dCrop.cropping.x > 0 && dCrop.cropping.x < 500 && dCrop.cropping.circle === false, dCrop.cropping);
  const dOver = await W.cmd('avatar_crop', { zoom: 9 });
  check('B3b a zoom of 9 is held at 4', dOver.cropping.zoom === 4, dOver.cropping.zoom);
  await W.cmd('avatar_crop', { zoom: 1.6, x: 60, y: 0, circle: false }); await sleep(500); await shot('07-crop-square-zoomed');
  await W.cmd('avatar_crop', { circle: true }); await sleep(400); await shot('08-crop-circle');
  await W.cmd('avatar_crop', { circle: false });
  await W.cmd('avatar_use');
  const nv = await soft(until(async () => (await versions())[N.account] || null, 40000, 'nat\'s picture on the relay', 400));
  check('B4 "Use picture" uploads: Nat\'s picture is in the core\'s versions as seen by Alice', !timedOut(nv), nv);
  const d = await soft(rpc(A.port, 'avatar_data', { account: N.account }));
  const png = path.join(out, 'nat-as-sent.bin'); fs.writeFileSync(png, Buffer.from(d.data.replace(/-/g, '+').replace(/_/g, '/'), 'base64'));
  const info = JSON.parse(spawnSync('python3', ['-c', `import json,sys;from PIL import Image;im=Image.open('${png}');print(json.dumps({'w':im.width,'h':im.height,'fmt':im.format}))`]).stdout.toString() || '{}');
  check('B5 what was sent: a 256x256 JPEG under 40 KB with shape "square", as the web would send', d.mime === 'image/jpeg' && d.shape === 'square' && d.size <= 40 * 1024 && info.w === 256 && info.h === 256 && info.fmt === 'JPEG', { mime: d.mime, shape: d.shape, size: d.size, info });
  const ownHeld = await soft(until(async () => { const h = await heldFor(N.account); return h && h.blob === nv ? h : null; }, 20000, 'window shows its own picture'));
  check('B6 the window shows the picture it just sent', !timedOut(ownHeld), ownHeld);
  await sleep(800); await shot('09-settings-with-picture');
  const webSees = await soft(until(async () => { await web(B, `refreshAvatars(true)`); return web(B, `(() => { const i = document.querySelector('.seal[data-av="${N.account}"] img.av-img'); return i && i.complete && i.naturalWidth > 0 ? { w: i.naturalWidth, cls: i.className } : null; })()`); }, 40000, 'web shows nat\'s picture', 1500));
  check('B7 Bob\'s web client shows Nat\'s picture (a 256 px image, square frame)', !timedOut(webSees) && webSees.w === 256 && /square/.test(webSees.cls), webSees);
  await wshot(B, '06-rows-after-native-picture');

  // a second, tall picture set by drop (same path), round, zoomed
  await W.cmd('avatar_open', { path: path.join(pics, 'tall.jpg') });
  await until(async () => (await W.cmd('avatars')).cropping, 10000, 'cropper 2');
  const d2 = await W.cmd('avatar_crop', { zoom: 2, circle: true });
  check('B8 a tall JPEG opens with the right size', d2.cropping.w === 300 && d2.cropping.h === 500, d2.cropping);
  await W.cmd('avatar_use');
  const nv2 = await soft(until(async () => { const v = (await versions())[N.account]; return v && v !== nv ? v : null; }, 40000, 'new version', 400));
  check('B9 choosing another picture replaces it (new blob id)', !timedOut(nv2), nv2);
  const d2d = await soft(rpc(A.port, 'avatar_data', { account: N.account }));
  check('B10 and the shape follows the choice (circle)', d2d.shape === 'circle');
  await until(async () => { const h = await heldFor(N.account); return h && h.blob === nv2 && h.shape === 'circle'; }, 20000, 'window refetched on version change');
  check('B11 the window refetches only on a new version (it holds the new blob, circle)', true);

  // ---------------------------------------------------------------- C. messages and calls show both
  await W.cmd('open', { name: text.name }); await sleep(800);
  await W.core('send_text', { gid: text.gid, text: 'hello from Nat, with a picture' });
  await rpc(B.port, 'send_text', { gid: text.gid, text: 'and Bob answers' });
  await sleep(3500); await shot('10-rows-both-pictures');
  await wshot(B, '10-rows-both-pictures');
  await webPeople(B); await wshot(B, '11-people-both'); await webPeople(B);
  await W.cmd('panel', { which: 'people', name: clubName }); await sleep(900); await shot('11-people-both'); await W.cmd('panel', { which: 'close' });
  await W.cmd('home'); await sleep(1200); await shot('12-home-you-card');
  // the call bar
  const joinWeb = async (p, gid, name) => { await web(p, `(window.__callUI.joinRoom(${J(gid)}, ${J(name)}), true)`); await p.page.waitFor(`window.__commonsCall.phase === 'connected' && window.__commonsCall.gid === ${J(gid)}`, 30000, p.name + ' connected'); };
  await joinWeb(B, porchGid, 'porch');
  await W.cmd('open', { name: text.name }); await sleep(500);
  await W.cmd('join', { name: 'porch' });
  const inCall = await soft(until(async () => { const d = await W.cmd('dump'); return d.call.phase === 'connected' && d.call.peers.length >= 2 ? d : null; }, 30000, 'window in the call'));
  check('C1 the window is in a call with Bob', !timedOut(inCall), inCall.call && inCall.call.peers);
  await sleep(1500); await shot('13-call-bar-faces');
  await wshot(B, '13-call-bar-web');
  await W.cmd('leave'); await web(B, `(window.__commonsCall.phase !== 'idle' ? window.__commonsCall.leave() : 0, true)`);

  // ---------------------------------------------------------------- D. remove
  await W.cmd('settings'); await sleep(700);
  await W.cmd('avatar_remove');
  const gone = await soft(until(async () => !(await versions())[N.account], 30000, 'removed', 400));
  check('D1 "Remove" clears the picture in the core (no version for Nat)', gone === true || !timedOut(gone));
  check('D2 the window is back to the letter', await soft(until(async () => !(await heldFor(N.account)), 15000, 'window dropped it', 300)) === true);
  const webGone = await soft(until(async () => { await web(B, `refreshAvatars(true)`); return web(B, `!document.querySelector('.seal[data-av="${N.account}"] img.av-img')`); }, 40000, 'web drops it', 1500));
  check('D3 Bob\'s web client shows Nat\'s seal again', webGone === true);
  await sleep(600); await shot('14-settings-removed');
  // Bob removes his on the web: the window drops it too
  await web(B, `removeOwnAvatar()`);
  check('D4 Bob removes his picture on the web and the window drops it', await soft(until(async () => !(await heldFor(B.account)), 40000, 'window dropped bob\'s', 500)) === true);

  await W.cmd('quit').catch(() => { });
} catch (e) { res.error = String(e.stack || e); console.log('ERROR', res.error); failed++; }
res.failed = failed;
fs.writeFileSync(path.join(out, 'result.json'), JSON.stringify(res, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED', '->', path.join(out, 'result.json'));
stopAll(); process.exit(failed ? 1 : 0);
