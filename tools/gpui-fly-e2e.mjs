// The card flight between levels in the native window (app/gpui/src/flight.rs) against the web (mega.js enterCommunity / closeIntoDots / openFromDots). Nat is the native window in a
// private headless KWin driven over its test port (uitest.rs voice_*) and with real pointer events; Alice and Bob are the real app/ui in
// headless Chrome. The window records a SYNTHETIC microphone (MEGA_GPUI_MIC=synthetic): the user's real mic is never opened. SILENT.
//   COMMONS_TEST_PORT_OFFSET=5900 MEGA_GPUI_BIN=<mega-gpui> systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-fly-e2e.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import crypto from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-fly-e2e');
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
    MEGA_GPUI_UI_TEST: String(winPort), MEGA_GPUI_AUDIO_SINK: 'null', MEGA_GPUI_MIC: 'synthetic', MEGA_GPUI_FLY_SLOW: '8', MEGA_GPUI_SIZE: '1280x860', RUST_LOG: 'info' }, path.join(out, 'nat.log'));
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

  // ---------------------------------------------------------------- the flights, slowed 8x so frames can be caught
  const NAV = () => W.cmd('nav'), FLY = () => W.cmd('fly');
  const frames = async (tag, n, gap) => { const out = []; for (let i = 0; i < n; i++) { out.push(await FLY()); await shot(`${tag}-${i}`); await sleep(gap); } return out; };
  // 1. home -> community: the card grows to fill the column
  await W.cmd('home'); await sleep(2500);
  const nv = await NAV(); console.log('nav', J(nv).slice(0, 400));
  await shot('40-home');
  const ix = Math.max(0, (nv.cards || []).findIndex(c => c.title === 'Test club'));
  await W.cmd('navclick', { i: ix }); await sleep(1500);
  await W.cmd('navkey', { key: 'enter' });
  const f1 = await frames('41-expand', 5, 350);
  const sizes = f1.filter(f => f.active && f.frame).map(f => f.frame[0].rect[2]);
  check('F1 opening a community starts an Expand flight whose box grows (width samples rise)', f1[0].kind === 'Expand' && sizes.length >= 3 && sizes[sizes.length - 1] > sizes[0] * 1.2, { kinds: f1.map(f => f.kind), sizes });
  await sleep(3500);
  check('F2 the flight ends and the level is the community (no overlay left)', (await FLY()).active === false);
  await shot('42-after-expand');
  // 2. a room card -> chat: the cards fly into the rail's dots
  const nv2 = await NAV(); const rx = (nv2.cards || []).findIndex(c => c.title === text.name && !c.voice);
  await W.cmd('navclick', { i: Math.max(0, rx) }); await sleep(1500);
  await W.cmd('navkey', { key: 'enter' });
  const f2 = await frames('43-close', 5, 350);
  const ws = f2.filter(f => f.active && f.frame).map(f => f.frame[0].rect[2]);
  check('F3 opening a room starts a Close flight whose boxes shrink toward the dots', f2[0].kind === 'Close' && ws.length >= 3 && ws[ws.length - 1] < ws[0] * 0.6, { kinds: f2.map(f => f.kind), ws, items: f2[0].items });
  await sleep(3500);
  check('F4 it ends and the chat is open', (await FLY()).active === false && (await W.cmd('core', { method: 'snapshot' })).ok);
  await shot('44-chat');
  // 3. back: the dots open into the cards
  await W.cmd('server', { name: 'Test club' });
  const f3 = await frames('45-open', 5, 350);
  const os = f3.filter(f => f.active && f.frame).map(f => f.frame[0].rect[2]);
  check('F5 leaving the room starts an Open flight whose boxes grow out of the dots', f3[0].kind === 'Open' && os.length >= 3 && os[os.length - 1] > os[0] * 3, { kinds: f3.map(f => f.kind), os });
  await sleep(3500); await shot('46-cards-again');
  check('F6 afterwards the cards are back and no flight remains', (await FLY()).active === false);
  // 4. saver on: no flights at all
  await W.cmd('saver', { mode: 'on' });
  await W.cmd('navclick', { i: Math.max(0, rx) }); await sleep(1200); await W.cmd('navkey', { key: 'enter' }); await sleep(300);
  check('F7 with the battery saver on there is no flight', (await FLY()).active === false);

  await W.cmd('quit').catch(() => { });
} catch (e) { res.error = String(e.stack || e); console.log('ERROR', res.error); failed++; }
res.failed = failed;
fs.writeFileSync(path.join(out, 'result.json'), JSON.stringify(res, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED', '->', path.join(out, 'result.json'));
stopAll(); process.exit(failed ? 1 : 0);
