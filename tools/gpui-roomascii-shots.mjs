// The open room's ASCII backdrop, native window against the web client (app/gpui/src/backdrop.rs, app/ui/mega.js bdLoop): side-by-side screenshots. Alice is the native WINDOW
// (`mega-gpui`) in a private headless KWin, driven over its test port (uitest.rs); Bob is the real app/ui in headless Chrome for Testing.
// One local relay, throwaway accounts, silent. Nobody joins a real room.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin mega-gpui --bin commons-relay --bin dev_bridge
//   COMMONS_TEST_PORT_OFFSET=2700 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-roomascii-shots.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, startChrome, openPage, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-roomascii-shots');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const shots = path.join(out, 'shots'); fs.mkdirSync(shots, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const PASS = 'correct horse battery';
const REL = `http://127.0.0.1:${RELAY_PORT}`;
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
  wall() { return this.cmd('wall'); }
  paint(what, args = {}) { return this.cmd('paint_do', { what, ...args }); }
}

const priv = path.join(out, 'priv'); const xdg = { XDG_RUNTIME_DIR: path.join(priv, 'run'), XDG_CONFIG_HOME: path.join(priv, 'config'), XDG_STATE_HOME: path.join(priv, 'state'), XDG_CACHE_HOME: path.join(priv, 'cache'), XDG_DATA_HOME: path.join(priv, 'data') };
for (const d of Object.values(xdg)) fs.mkdirSync(d, { recursive: true, mode: 0o700 });
const cleanEnv = () => { const e = { ...process.env, ...xdg }; for (const k of ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'XDG_SESSION_ID', 'PIPEWIRE_RUNTIME_DIR', 'PULSE_SERVER', 'PULSE_RUNTIME_PATH']) delete e[k]; return e; };
const busConf = (addrFile) => { const c = path.join(priv, path.basename(addrFile) + '.conf'); fs.writeFileSync(c, `<busconfig><type>session</type><listen>unix:path=${path.join(priv, path.basename(addrFile) + '.sock')}</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>`); return c; };
function sh(cmd, args, env, log) { const p = spawn(cmd, args, { env, stdio: ['ignore', fs.openSync(log, 'a'), fs.openSync(log, 'a')], detached: true }); procs.push(p); return p; }


const SAVER = process.env.SAVER || '';
try {
  await startServers(out, { livekit: false });
  const people = {}; let k = 0;
  const makeAccount = async name => { const port = P(27200 + k++); const b = startBridge(out, name, port); await sleep(1300); await rpc(port, 'create_account', { name: name[0].toUpperCase() + name.slice(1), passphrase: PASS, relay: REL }); people[name] = { name, port, account: (await rpc(port, 'snapshot')).me.account, bridge: b }; return people[name]; };
  const alice = await makeAccount('alice'), bob = await makeAccount('bob');
  const cid = (await rpc(alice.port, 'create_community', { name: 'Ascii club' })).id;
  for (const n of ['lounge', 'aurora', 'sketch']) await rpc(alice.port, 'create_channel', { cid, name: n });
  const link = (await rpc(alice.port, 'create_invite', { cid, uses: 5, ttl: 3600 })).link;
  await rpc(bob.port, 'join', { link });
  const ready = async p => until(async () => { const s = await rpc(p.port, 'snapshot'); const c = s.communities.find(c => c.id === cid); return c && c.channels.length >= 4 && c.channels.every(x => x.joined && x.status === 'ok') ? c : null; }, 90000, p.name + ' ready', 500);
  const ca = await ready(alice); await ready(bob);
  const gid = n => ca.channels.find(x => x.name === n).gid;
  const talk = [['alice', 'Welcome to the club. This room has the default look.'], ['bob', 'Thanks, glad to be here.'], ['alice', 'The backdrop is the room\'s own ASCII, dim, and it flows around every word.'],
    ['bob', 'It moves a little when somebody writes, then settles.'], ['alice', 'Right: a ring leaves the new message and the room warms up for a while.'], ['bob', 'And with the battery saver on it holds still.'],
    ['alice', 'Short one.'], ['bob', 'A longer line to see the pattern wrap around a wide block of words, which is what the page does too.']];
  for (const r of ['lounge', 'aurora', 'sketch']) for (const [w, t] of talk) { await rpc(people[w].port, 'send_text', { gid: gid(r), text: t }); await sleep(120); }
  // the owner's look for "aurora": chat pattern 7 (aurora), own backdrop colours, strength 130
  await rpc(alice.port, 'set_look', { cid, room: gid('aurora'), chat: 7, dim: 130, back: { main: '#0b3d6e', hi: '#45f0d0' } });
  // a drawing on "sketch"
  const cells = []; for (let i = 0; i < 46; i++) cells.push([14 + i, 14 + (i >> 3), 1, 0]); for (let i = 0; i < 30; i++) cells.push([24 + i, 24 - (i >> 1), 5, 2]); for (let i = 0; i < 16; i++) cells.push([60 + (i % 4), 6 + (i >> 2), 11, 3]);
  await rpc(alice.port, 'wall_draw', { gid: gid('sketch'), cells, share: true });
  await sleep(2500);
  try { process.kill(-alice.bridge.pid, 'SIGTERM'); } catch { } await sleep(1800);

  const kwBusFile = path.join(out, 'kwin-bus');
  sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket wayland-roomascii --no-lockscreen --no-global-shortcuts --width 1600 --height 1100`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(out, 'kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, 'wayland-roomascii')), 20000, 'kwin socket');
  const nshot = async name => { const f = path.join(shots, name + '.png'); spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); return f; };
  if (SAVER) fs.writeFileSync(path.join(out, 'data', 'alice', 'gpui-prefs.json'), J({ saver: 'on' }));
  const wport = P(27310);
  sh(BIN, [], { ...cleanEnv(), WAYLAND_DISPLAY: 'wayland-roomascii', DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim(), COMMONS_DATA: path.join(out, 'data', 'alice'), COMMONS_RELAY: REL, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_UI_TEST: String(wport), MEGA_GPUI_SIZE: '1280x900', RUST_BACKTRACE: '1' }, path.join(out, 'alice-window.log'));
  const W = new Win(wport);
  await until(async () => { try { await W.connect(); return true; } catch { return false; } }, 30000, 'window test port');
  await until(async () => (await W.cmd('core', { method: 'snapshot' })).ok, 30000, 'window core unlocked');
  await sleep(2500);

  const chrome = await startChrome(out, 'bob', P(29400), 500);
  bob.page = await openPage(chrome, `http://127.0.0.1:${bob.port}/`, 'bob', { before: "try { localStorage.setItem('commons.tour', '1'); } catch {}" });
  await bob.page.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 900, deviceScaleFactor: 1, mobile: false });
  await bob.page.waitFor(`!!window.__megaWall && !!S.gid`, 40000, 'bob app ready');
  const web = expr => bob.page.ev(`(async()=>{ return ${expr}; })()`);
  const wshot = async name => { const r = await bob.page.send('Page.captureScreenshot', { format: 'png' }); fs.writeFileSync(path.join(shots, name + '.png'), Buffer.from(r.result.data, 'base64')); };
  const key = async k => { const vk = { ArrowDown: 40, ArrowUp: 38, Enter: 13, Escape: 27 }[k]; await bob.page.send('Input.dispatchKeyEvent', { type: 'keyDown', key: k, code: k, windowsVirtualKeyCode: vk }); await bob.page.send('Input.dispatchKeyEvent', { type: 'keyUp', key: k, code: k, windowsVirtualKeyCode: vk }); await sleep(700); };
  await sleep(2500);
  // the web: open a room by walking the cards (the community's room cards come first, then Enter)
  const webOpen = async name => {
    for (let i = 0; i < 3 && (await web(`S.gid`)); i++) { await key('Escape'); await sleep(900); }
    await web(`(() => { const L = __mega.list; L.go(L.items.findIndex(x => x.name === ${J(name)}), true); })()`);
    await until(async () => (await web(`__mega.list.item.name`)) === name && (await web(`__mega.list.settled`)), 15000, 'web card in focus: ' + name, 300); await sleep(600);
    await key('Enter'); await sleep(3500);
  };
  const rooms = { lounge: 'lounge', aurora: 'aurora', sketch: 'sketch' };
  const pairs = [];
  const pair = async (nm, room, extra) => {
    await W.cmd('open', { name: room }); await sleep(3000);
    if (extra) await extra('native');
    await nshot(nm + '-native');
    await webOpen(room);
    if (extra) await extra('web');
    await wshot(nm + '-web'); pairs.push(nm);
  };
  if (SAVER) await bob.page.ev(`document.documentElement.classList.add('fx-lite')`);
  await W.cmd('go', { name: 'Ascii club' }); await sleep(1500);
  // the web starts at the community's room cards (one community)
  console.log('web level', await web(`__mega.level`), await web(`__mega.list && __mega.list.item && __mega.list.item.name`));
  if (SAVER) { await pair('5-battery-saver', 'aurora'); await pair('5b-battery-saver-default', 'lounge'); fs.writeFileSync(path.join(out, 'pairs.json'), J(pairs)); stopAll(); process.exit(0); }
  await pair('1-rest-default', 'lounge');
  await pair('3-owner-pattern-colours', 'aurora');
  await pair('4-drawing', 'sketch');
  // 2: right after a new message: the ripple (native: the window's own, web: Bob writes, so Alice's web ... here Bob's own page ripples at "sending")
  await W.cmd('open', { name: 'lounge' }); await sleep(2500);
  const before = await W.cmd('bd');
  await W.cmd('stir'); await sleep(450); await nshot('2-ripple-native');
  const mid = await W.cmd('bd');
  await sleep(1200); const later = await W.cmd('bd');
  check('R1 a new message starts a ripple and warms the room', before.live === false && mid.ripples >= 1 && mid.heat > before.heat, { before, mid });
  await sleep(40000); const settled = await W.cmd('bd');
  check('R2 the ripple dies and the room settles to rest (no more frames asked for)', settled.live === false && settled.ripples === 0, settled);
  const t1 = (await W.cmd('bd')).t; await sleep(3000); const t2 = (await W.cmd('bd')).t;
  check('R3 at rest the backdrop clock does not move', t1 === t2, { t1, t2 });
  await webOpen('lounge');
  await rpc(alice.port, 'snapshot').catch(() => {});
  await rpc(bob.port, 'send_text', { gid: gid('lounge'), text: 'a brand new message to ripple around' });
  await sleep(500); await wshot('2-ripple-web');
  fs.writeFileSync(path.join(out, 'pairs.json'), J(pairs));
} catch (e) { check('the run itself', false, String(e && e.stack || e).slice(0, 400)); }
fs.writeFileSync(path.join(out, 'results.json'), J(results, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED');
stopAll(); process.exit(failed ? 1 : 0);
