// The ASCII motion and the typing indicator, native window against the web client (app/gpui/src/{backdrop,typing}.rs, app/ui/mega.js bdLoop / asciiTick / typingDraw):
// FRAME SEQUENCES of both, contact sheets web|native, and checks that frames differ over time at rest (and are identical with the battery saver on or the window hidden).
// Alice is the native WINDOW (`mega-gpui`) in a private headless KWin driven over its test port; Bob is the real app/ui in headless Chrome for Testing (GPU: ANGLE/Vulkan).
//   COMMONS_TEST_PORT_OFFSET=3700 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-motion-shots.mjs <outdir>
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
  const winProc = sh(BIN, [], { ...cleanEnv(), WAYLAND_DISPLAY: 'wayland-roomascii', DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim(), COMMONS_DATA: path.join(out, 'data', 'alice'), COMMONS_RELAY: REL, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_UI_TEST: String(wport), MEGA_GPUI_SIZE: '1280x900', RUST_BACKTRACE: '1' }, path.join(out, 'alice-window.log'));
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
  const gidOf = n => ca.channels.find(x => x.name === n).gid;
  const N = Number(process.env.FRAMES || 12), GAP = Number(process.env.GAP || 400);
  const PY = path.join(root, 'tools/lib/motion-sheet.py');
  const py = (...a) => { const r = spawnSync('python3', ['-I', PY, ...a], { encoding: 'utf8' }); if (r.status) throw new Error('motion-sheet: ' + r.stderr.slice(-400)); return r.stdout.trim(); };
  const nativeFrames = async (prefix, n = N, gap = GAP, keep) => { for (let i = 0; i < n; i++) { const t = Date.now(); if (keep) await keep(); await nshot(`${prefix}-native-${String(i).padStart(2, '0')}`); await sleep(Math.max(0, gap - (Date.now() - t))); } };
  const webFrames = async (prefix, n = N, gap = GAP, keep) => { for (let i = 0; i < n; i++) { const t = Date.now(); if (keep) await keep(); await wshot(`${prefix}-web-${String(i).padStart(2, '0')}`); await sleep(Math.max(0, gap - (Date.now() - t))); } };
  const webTo = async (name, room) => {   // the web: community room cards (name = null) or one room
    for (let i = 0; i < 3 && (await web(`S.gid`)); i++) { await key('Escape'); await sleep(900); }
    if (!room) return;
    await webOpen(room);
  };
  await bob.page.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 900, deviceScaleFactor: 1, mobile: false });
  await rpc(bob.port, 'set_typing_enabled', { on: true }); await W.core('set_typing_enabled', { on: true });
  const sheets = [];
  const sheet = (name, prefix, crop, cols) => { py('sheet', path.join(out, 'sheets', name + '.png'), shots, prefix, String(N), crop, String(cols)); sheets.push(name); };
  fs.mkdirSync(path.join(out, 'sheets'), { recursive: true });
  const moves = (name, side, crop, n = N) => JSON.parse(py('diff', shots, `${name}-${side}`, String(n), crop));   // {pairs, changed, min, max}
  const WIN = '0,0,1278,900';   // window coordinates: the web page is 1280 x 900, the native window's content area is cropped out of the workspace (motion-sheet.py)

  // CPU=1: only measure the window's idle CPU (percent of one core over 12 s) on the room cards and in a room, then stop. MEGA_GPUI_BIN picks the build.
  if (process.env.CPU) {
    const ticks = () => { const f = fs.readFileSync(`/proc/${winProc.pid}/stat`, 'utf8').split(') ')[1].split(' '); return Number(f[11]) + Number(f[12]); };
    const cpu = async () => { const a = ticks(), t = Date.now(); await sleep(12000); return +(((ticks() - a) / 100) / ((Date.now() - t) / 1000) * 100).toFixed(1); };
    await W.cmd('go', { name: 'Ascii club' }); await sleep(3000);
    const cards = await cpu();
    await W.cmd('open', { name: 'lounge' }); await sleep(3000);
    const room = await cpu();
    console.log('CPU ' + J({ bin: BIN, cards_pct: cards, room_pct: room })); stopAll(); process.exit(0);
  }
  // ---- 1. a card at rest: the community's room cards, the focused one and its neighbours
  await W.cmd('go', { name: 'Ascii club' }); await sleep(2500);
  await webTo(null, null); await sleep(2500);
  await W.cmd('saver', { mode: 'off' });
  await nativeFrames('1-card', N, GAP); await webFrames('1-card', N, GAP);
  sheet('1-card-at-rest', '1-card', WIN, 4);
  let m1n = moves('1-card', 'native', WIN), m1w = moves('1-card', 'web', WIN);
  check('M1 native: a card at rest moves (frames differ over time)', m1n.changed >= Math.floor(N * 0.7), m1n);
  check('M1 web: the same', m1w.changed >= Math.floor(N * 0.7), m1w);

  // ---- 2. a room at rest: the backdrop drifts and rings by itself
  await W.cmd('open', { name: 'lounge' }); await sleep(3000);
  await webOpen('lounge'); await sleep(1500);
  await nativeFrames('2-room', N, GAP); await webFrames('2-room', N, GAP);
  sheet('2-room-at-rest', '2-room', WIN, 4);
  let m2n = moves('2-room', 'native', WIN), m2w = moves('2-room', 'web', WIN);
  check('M2 native: a room at rest moves (the backdrop drifts)', m2n.changed >= Math.floor(N * 0.7), m2n);
  check('M2 web: the same', m2w.changed >= Math.floor(N * 0.7), m2w);
  // a long watch for the idle ripples: the clock must have kept running, and a ring must have come by itself within half a minute
  const b0 = await W.cmd('bd'); let rings = b0.ripples > 0 ? 1 : 0; const t0 = Date.now();
  while (Date.now() - t0 < 32000) { await sleep(500); const b = await W.cmd('bd'); if (b.ripples > 0) rings++; }
  const b1 = await W.cmd('bd');
  check('M3 native: the backdrop clock keeps running at rest and an idle ripple appears by itself', b1.t > b0.t + 5 && rings > 0, { t0: b0.t, t1: b1.t, rings, next: b1.next_rip });

  // ---- 3. a room while someone types (native: Bob types; web: Alice types)
  const bobTypes = async () => { await rpc(bob.port, 'typing', { gid: gid('lounge') }); };
  const aliceTypes = async () => { await W.core('typing', { gid: gid('lounge') }); };
  await bobTypes(); await sleep(2500);
  const ty = setInterval(() => { bobTypes().catch(() => { }); }, 1800);
  const nativeTyping = await W.cmd('dump'); 
  await nativeFrames('3-typing', N, 300);
  clearInterval(ty);
  const wty = setInterval(() => { aliceTypes().catch(() => { }); }, 1800);
  await aliceTypes(); await sleep(2500);
  await webFrames('3-typing', N, 300);
  clearInterval(wty);
  sheet('3-typing', '3-typing', WIN, 4);
  // the strip over the composer, close up
  py('sheet', path.join(out, 'sheets', '3-typing-composer.png'), shots, '3-typing', String(N), '0,760,900,140', '3'); sheets.push('3-typing-composer');
  const m3n = moves('3-typing', 'native', '0,790,900,110'), m3w = moves('3-typing', 'web', '0,790,900,110');
  check('M4 native: while Bob types, the strip over the composer moves (mascot bobs, waves travel)', m3n.changed >= Math.floor(N * 0.7), m3n);
  check('M4 web: the same', m3w.changed >= Math.floor(N * 0.7), m3w);
  await sleep(7000);   // the core forgets a typist after 6 seconds
  await nshot('3-typing-gone-native'); await wshot('3-typing-gone-web');
  const gone = await W.cmd('bd');
  check('M5 native: the typing list is empty again once Bob stopped', Array.isArray(gone.typing) && gone.typing.length === 0, gone.typing);

  // ---- 4. the battery saver holds everything still; a hidden window too
  await W.cmd('saver', { mode: 'on' });
  await bob.page.ev(`document.documentElement.classList.add('fx-lite','fx-saver'); window.dispatchEvent(new Event('mega-fx'))`);
  await sleep(1500);
  const sv = setInterval(() => { bobTypes().catch(() => { }); }, 1800); await bobTypes(); await sleep(2500);
  await nativeFrames('4-saver', 6, 400); clearInterval(sv);
  await wshot('4-saver-web-00');
  const ABOVE = '0,0,1278,804';   // everything above the composer: its caret blinks on its own
  const m4n = moves('4-saver', 'native', ABOVE, 6);
  check('M6 native: with the battery saver on, a room and its typing indicator are frozen (frames identical)', m4n.pairs === 5 && m4n.max === 0, m4n);
  await W.cmd('saver', { mode: 'off' });
  await bob.page.ev(`document.documentElement.classList.remove('fx-lite','fx-saver'); window.dispatchEvent(new Event('mega-fx'))`);
  await sleep(1500);
  const hv = setInterval(() => { bobTypes().catch(() => { }); }, 1800); await bobTypes(); await sleep(2500);
  await W.cmd('hidden', { on: true }); await sleep(800);
  await nativeFrames('5-hidden', 6, 400); clearInterval(hv);
  const m5n = moves('5-hidden', 'native', ABOVE, 6);
  check('M7 native: a hidden window does not animate, backdrop or typing (frames identical)', m5n.pairs === 5 && m5n.max === 0, m5n);
  await W.cmd('hidden', { on: false }); await sleep(1200);
  await nativeFrames('6-back', 6, 400);
  const m6n = moves('6-back', 'native', ABOVE, 6);
  check('M8 native: and it moves again when shown', m6n.changed >= 3, m6n);

  // ---- 5. the open rail's cards move like the big ones
  await W.cmd('railopen', { on: true }); await sleep(2500);
  await nativeFrames('7-rail', N, 350);
  await W.cmd('railopen', { on: false });
  await bob.page.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 20, y: 450 }); await sleep(2500);
  const railOpen = await web(`!!document.querySelector('#mega-rail.open, .mrail.open, [class~=open].mrail')`).catch(() => false);
  await webFrames('7-rail', N, 350);
  await bob.page.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 900, y: 300 });
  sheet('7-rail-open', '7-rail', '0,0,420,900', 6);
  const RAIL = '0,0,420,900';
  const m7n = moves('7-rail', 'native', RAIL), m7w = moves('7-rail', 'web', RAIL);
  check('M9 native: the cards of the open rail move', m7n.changed >= Math.floor(N * 0.5), m7n);
  check('M9 web: the same (rail opened: ' + railOpen + ')', m7w.changed >= Math.floor(N * 0.5), m7w);

  fs.writeFileSync(path.join(out, 'sheets.json'), J(sheets));
} catch (e) { check('the run itself', false, String(e && e.stack || e).slice(0, 600)); }
fs.writeFileSync(path.join(out, 'results.json'), J(results, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED');
stopAll(); process.exit(failed ? 1 : 0);
