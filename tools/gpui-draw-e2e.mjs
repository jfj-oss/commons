// Colours and the shared ASCII wall in the native window (app/gpui/src/paint.rs, wall.rs), against the web client. Alice is the native WINDOW
// (`mega-gpui`) in a private headless KWin, driven over its test port (uitest.rs); Bob is the real app/ui in headless Chrome for Testing.
// One local relay, throwaway accounts, silent. Nobody joins a real room.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin mega-gpui --bin commons-relay --bin dev_bridge
//   COMMONS_TEST_PORT_OFFSET=2300 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-draw-e2e.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, startChrome, openPage, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-draw-e2e');
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

try {
  await startServers(out, { livekit: false });
  const people = {}; let k = 0;
  const makeAccount = async name => { const port = P(27200 + k++); const b = startBridge(out, name, port); await sleep(1300); await rpc(port, 'create_account', { name: name[0].toUpperCase() + name.slice(1), passphrase: PASS, relay: REL }); people[name] = { name, port, account: (await rpc(port, 'snapshot')).me.account, bridge: b }; return people[name]; };
  const alice = await makeAccount('alice'), bob = await makeAccount('bob');
  // Alice makes the community and lets Bob in while her bridge is up; then the window takes over her folder
  const cid = (await rpc(alice.port, 'create_community', { name: 'Draw club' })).id;
  await rpc(alice.port, 'create_channel', { cid, name: 'sketches' });
  const link = (await rpc(alice.port, 'create_invite', { cid, uses: 5, ttl: 3600 })).link;
  await rpc(bob.port, 'join', { link });
  const ready = async p => until(async () => { const s = await rpc(p.port, 'snapshot'); const c = s.communities.find(c => c.id === cid); return c && c.channels.length >= 2 && c.channels.every(x => x.joined && x.status === 'ok') ? c : null; }, 90000, p.name + ' ready', 500);
  const ca = await ready(alice); await ready(bob);
  const room = ca.channels.find(x => x.name === 'sketches').gid;
  try { process.kill(-alice.bridge.pid, 'SIGTERM'); } catch { } await sleep(1800);

  const kwBusFile = path.join(out, 'kwin-bus');
  sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket wayland-draw-e2e --no-lockscreen --no-global-shortcuts --width 1600 --height 1100`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(out, 'kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, 'wayland-draw-e2e')), 20000, 'kwin socket');
  const shot = async name => { const f = path.join(shots, name + '.png'); spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); return f; };
  const wport = P(27310);
  sh(BIN, [], { ...cleanEnv(), WAYLAND_DISPLAY: 'wayland-draw-e2e', DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim(), COMMONS_DATA: path.join(out, 'data', 'alice'), COMMONS_RELAY: REL, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_UI_TEST: String(wport), MEGA_GPUI_SIZE: '1280x900', RUST_BACKTRACE: '1' }, path.join(out, 'alice-window.log'));
  const W = new Win(wport);
  await until(async () => { try { await W.connect(); return true; } catch { return false; } }, 30000, 'window test port');
  await until(async () => (await W.cmd('core', { method: 'snapshot' })).ok, 30000, 'window core unlocked');
  await sleep(2500);

  // Bob: the real web UI, at the community's room cards with the sketches card in focus (its wall is polled; the draw chip is there)
  const chrome = await startChrome(out, 'bob', P(29400), 500);
  bob.page = await openPage(chrome, `http://127.0.0.1:${bob.port}/`, 'bob', { before: "try { localStorage.setItem('commons.tour', '1'); } catch {}" });
  await bob.page.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 900, deviceScaleFactor: 1, mobile: false });
  await bob.page.waitFor(`!!window.__megaWall && !!S.gid`, 40000, 'bob app ready');
  const web = expr => bob.page.ev(`(async()=>{ return ${expr}; })()`);
  const key = async k => { const vk = { ArrowDown: 40, ArrowUp: 38, Enter: 13, Escape: 27 }[k]; await bob.page.send('Input.dispatchKeyEvent', { type: 'keyDown', key: k, code: k, windowsVirtualKeyCode: vk }); await bob.page.send('Input.dispatchKeyEvent', { type: 'keyUp', key: k, code: k, windowsVirtualKeyCode: vk }); await sleep(900); };
  await sleep(2500);
  // (with one community the web starts at its room cards)
  console.log('web level', await web(`__mega.level`));
  for (let i = 0; i < 8 && (await web(`__megaWall.state().gid`)) !== room; i++) { await key('ArrowDown'); await sleep(900); }
  const webOpen = await soft(until(async () => (await web(`__megaWall.state().gid`)) === room, 20000, 'bob has the sketches card focused', 400));
  check('0 the native window is Alice (owner), the web client is Bob (member), the sketches card is in focus on the web', !timedOut(webOpen), { webGid: await web(`__megaWall.state().gid`), mode: await web(`__megaWall.state().mode`) });

  // ---- the window: the community's room cards, the "sketches" card in focus
  await W.cmd('go', { name: 'Draw club' }); await sleep(1500);
  const nav = await W.cmd('nav');
  const idx = (nav.opens || []).findIndex(o => o.includes(room));
  for (let i = 0; i < idx; i++) { await W.cmd('navkey', { key: 'down' }); await sleep(250); }
  await sleep(1800);
  let w0 = await W.wall();
  check('1 the focused card of the window is the sketches room and its wall is Alice\'s to draw on', w0.key === room && w0.drawable && w0.owner && !w0.on, { key: w0.key === room, drawable: w0.drawable, owner: w0.owner });
  await shot('01-card-focused');

  // ---- A. native draws, the web sees it
  const spot = w0.spot;   // [x, y, w, h, scale] of the focused card in the window
  const stroke = (x1, y1, x2, y2, n = 20) => ({ cmd: 'pen', points: Array.from({ length: n + 1 }, (_, i) => [x1 + (x2 - x1) * i / n, y1 + (y2 - y1) * i / n]) });
  await W.cmd('draw', { on: true }); await sleep(400);
  check('A0 draw mode is on in the window', (await W.wall()).on === true);
  await W.cmd('tool', { brush: 1, col: 0 });
  const [sx, sy, sw, shh] = spot;
  await W.cmd('pen', stroke(sx + sw * .2, sy + shh * .3, sx + sw * .8, sy + shh * .3).points ? stroke(sx + sw * .2, sy + shh * .3, sx + sw * .8, sy + shh * .3) : {});
  await W.cmd('tool', { brush: 5, col: 4 });
  await W.cmd('pen', stroke(sx + sw * .2, sy + shh * .4, sx + sw * .8, sy + shh * .6));
  await sleep(300); await shot('02-native-drawing');
  const mine = (await W.wall()).cells.map(c => c.join(','));
  check('A1 the pen puts cells under the window\'s wall', mine.length > 40, mine.length);
  const seenOnWeb = await soft(until(async () => { const cs = await web(`__megaWall.cellsOf(${J(room)}).map(c => c.join(','))`); return mine.every(m => cs.includes(m)) ? cs : null; }, 30000, 'web sees the strokes', 500));
  check('A2 the web client shows the same cells (glyph and colour) as the window drew', !timedOut(seenOnWeb), { mine: mine.length, web: (await web(`__megaWall.cellsOf(${J(room)}).length`)) });
  await shot('03-after-send');
  const sent0 = (await W.wall()).sent;

  // ---- B. the web draws, the window sees it
  await web(`document.querySelector('[data-m="draw"]').click()`); await sleep(500);
  check('B0 the web client\'s draw chip turns drawing on', (await web(`__megaWall.state().on`)) === true);
  const geo = await web(`(() => { const g = __megaWall.state().geo; return { l: g.b.left + g.ox / g.u, t: g.b.top + g.oy / g.u, cw: g.cw / g.u, ch: g.ch / g.u }; })()`);
  const press = (x, y, type) => bob.page.send('Input.dispatchMouseEvent', { type, x, y, button: 'left', buttons: type === 'mouseReleased' ? 0 : 1, clickCount: 1 });
  const webCell = (cx, cy) => [geo.l + (cx + .5) * geo.cw, geo.t + (cy + .5) * geo.ch];
  await web(`document.querySelector('#mw-glyphs [data-g="11"]').click()`); await web(`document.querySelector('#mw-cols [data-c="2"]').click()`);
  let [px0, py0] = webCell(10, 40); await press(px0, py0, 'mouseMoved'); await press(px0, py0, 'mousePressed');
  for (let i = 1; i <= 30; i++) { const [x, y] = webCell(10 + i, 40 + (i >> 2)); await press(x, y, 'mouseMoved'); await sleep(25); }
  await press(px0 + 30 * geo.cw, py0, 'mouseReleased');
  const webCells = await web(`__megaWall.cellsOf(${J(room)}).filter(c => c[1] === 11).map(c => c.join(','))`);
  const winSees = await soft(until(async () => { const cs = (await W.wall()).cells.map(c => c.join(',')); return webCells.length > 20 && webCells.every(c => cs.includes(c)) ? cs : null; }, 40000, 'window sees the web strokes', 500));
  check('B1 the web client\'s strokes (red x) arrive in the window', !timedOut(winSees), { webCells: webCells.length, window: (await W.wall()).cells.length });
  await shot('04-web-drawn-seen-native');
  check('B2 the window kept its own strokes besides', (await W.wall()).cells.length >= mine.length + webCells.length - 3, (await W.wall()).cells.length);
  await web(`__megaWall.off()`);

  // ---- C. owner lock blocks a member
  await W.cmd('wall_do', { what: 'lock' });
  const lockedWeb = await soft(until(async () => (await web(`(S.snap.communities.find(c => c.id === ${J(cid)}).style || {}).locked`) || []).includes(room), 30000, 'lock reached the web', 500));
  check('C1 the owner\'s lock in the window reaches the web client', lockedWeb === true);
  const before = (await W.wall()).cells.length;
  const refused = await soft(rpc(bob.port, 'wall_draw', { gid: room, cells: [[3, 3, 1, 0], [4, 3, 1, 0]], share: true }));
  await sleep(2500);
  const afterLock = (await W.wall()).cells.map(c => c.join(','));
  check('C2 a member\'s stroke on the locked wall is refused by the core', timedOut(refused) && /lock/.test(refused.timeout), J(refused).slice(0, 120));
  check('C2b and the web client offers no draw chip on a locked wall', (await web(`document.querySelector('[data-m="draw"]').hidden`)) === true);
  check('C3 the window does not hold the refused cells', !afterLock.includes((3 * 0 + 3 * 120 + 3) + ',1,0') && afterLock.length === before, { before, after: afterLock.length });
  const owner2 = await W.cmd('pen', stroke(sx + sw * .3, sy + shh * .8, sx + sw * .6, sy + shh * .8, 6));
  check('C4 the owner can still draw while it is locked', owner2.pending > 0 || (await W.wall()).cells.length > before, owner2);
  await W.cmd('wall_do', { what: 'lock' });   // unlock (the same button)
  await until(async () => !((await web(`(S.snap.communities.find(c => c.id === ${J(cid)}).style || {}).locked`)) || []).includes(room), 30000, 'unlock reached the web', 500);
  check('C5 unlocking lets members draw again', true);

  // ---- D. a colour change by the native owner shows on the web
  await W.cmd('paint');
  const op = (await W.cmd('wall'));   // panel open (the cards level)
  await W.paint('hex', { value: '#2a9d8f' });
  await sleep(700); await shot('05-colours-panel');
  const colOnWeb = await soft(until(async () => (await web(`(((S.snap.communities.find(c => c.id === ${J(cid)}).style || {}).rooms || {})[${J(room)}] || {}).main`)) === '#2a9d8f', 40000, 'colour reached web', 500));
  check('D1 a colour changed in the native colours panel shows on the web client (same style document)', colOnWeb === true);
  await W.paint('tile', { which: 'card', n: 7 });
  const patOnWeb = await soft(until(async () => (await web(`(((S.snap.communities.find(c => c.id === ${J(cid)}).style || {}).looks || {})[${J(room)}] || {}).card`)) === 7, 40000, 'pattern reached web', 500));
  check('D2 a card animation chosen in the window (aurora) reaches the web client', patOnWeb === true);
  await W.paint('dim', { value: 55 });
  check('D3 the backdrop strength set in the window reaches the web client', await soft(until(async () => (await web(`(((S.snap.communities.find(c => c.id === ${J(cid)}).style || {}).looks || {})[${J(room)}] || {}).dim`)) === 55, 40000, 'dim reached web', 500)) === true);
  await W.cmd('paint');

  // ---- E. idle sends nothing
  const s1 = (await W.wall()).sent; await sleep(6000); const s2 = (await W.wall()).sent;
  check('E1 an idle window sends no wall events', s1 === s2, { s1, s2 });

  // ---- F. clear empties it for everybody
  await W.cmd('wall_do', { what: 'clear' });
  const cleared = await soft(until(async () => (await web(`__megaWall.cellsOf(${J(room)}).length`)) === 0 && (await W.wall()).cells.length === 0, 40000, 'wall cleared', 500));
  check('F1 the owner\'s clear empties the wall in the window and on the web client', !timedOut(cleared), { web: await web(`__megaWall.cellsOf(${J(room)}).length`), win: (await W.wall()).cells.length });
  await shot('06-cleared');
  await W.cmd('draw', { on: false });
} catch (e) { check('the run itself', false, String(e && e.stack || e).slice(0, 400)); }
fs.writeFileSync(path.join(out, 'results.json'), J(results, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED');
stopAll(); process.exit(failed ? 1 : 0);
