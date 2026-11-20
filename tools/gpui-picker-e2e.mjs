// The colour picker in the native window (app/gpui/src/paint.rs), driven by REAL pointer and key events (examples/fakekeys.rs) in a private
// headless KWin, against the web client: drag the square and the hue strip, type a hex, arrows, presets, Escape, reset, the too-close warning,
// and the picker opened from the community settings and from the chat. Alice is the native window (owner); Bob is the real app/ui (member).
// A web Alice (same account, before the window takes her folder) takes the web picker's screenshots for the pairs.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin mega-gpui --bin commons-relay --bin dev_bridge --example fakekeys
//   COMMONS_TEST_PORT_OFFSET=4500 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-picker-e2e.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, startChrome, openPage, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/ds-mega/picker/e2e');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const shots = path.join(out, 'shots'); fs.mkdirSync(shots, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const FAKE = process.env.MEGA_GPUI_FAKEKEYS || path.join(root, 'target/debug/examples/fakekeys');
const PASS = 'correct horse battery';
const REL = `http://127.0.0.1:${RELAY_PORT}`;
const J = x => JSON.stringify(x);
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? '  [' + (typeof detail === 'string' ? detail : J(detail)).slice(0, 300) + ']' : ''}`); };
async function until(fn, ms, what, step = 250) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(step); } }
const soft = p => p.catch(e => ({ timeout: String(e.message || e) }));
const timedOut = v => v && typeof v === 'object' && typeof v.timeout === 'string';

// the page's maths, for the expected numbers
const hsv2hex = (h, s, v) => { const f = n => { const k = (n + h / 60) % 6; return (v - v * s * Math.max(0, Math.min(k, 4 - k, 1))) * 255; }; return '#' + [f(5), f(3), f(1)].map(x => Math.round(Math.max(0, Math.min(255, x))).toString(16).padStart(2, '0')).join(''); };
const near = (a, b, e) => Math.abs(a - b) <= e;
const dist = (a, b) => [1, 3, 5].reduce((s, i) => s + Math.abs(parseInt(a.slice(i, i + 2), 16) - parseInt(b.slice(i, i + 2), 16)), 0);

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
  paint(what, args = {}) { return this.cmd('paint_do', { what, ...args }); }
  state() { return this.cmd('paint_state'); }
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
  const cid = (await rpc(alice.port, 'create_community', { name: 'Paint club' })).id;
  await rpc(alice.port, 'create_channel', { cid, name: 'sketches' });
  const link = (await rpc(alice.port, 'create_invite', { cid, uses: 5, ttl: 3600 })).link;
  await rpc(bob.port, 'join', { link });
  const ready = async p => until(async () => { const s = await rpc(p.port, 'snapshot'); const c = s.communities.find(c => c.id === cid); return c && c.channels.length >= 2 && c.channels.every(x => x.joined && x.status === 'ok') ? c : null; }, 90000, p.name + ' ready', 500);
  const ca = await ready(alice); await ready(bob);
  const room = ca.channels.find(x => x.name === 'sketches').gid;

  // ---- the page's own picker (Alice on the web), for the screenshot pairs
  const wkey = async (p, k) => { const vk = { ArrowDown: 40, ArrowUp: 38, Enter: 13, Escape: 27 }[k]; await p.page.send('Input.dispatchKeyEvent', { type: 'keyDown', key: k, code: k, windowsVirtualKeyCode: vk }); await p.page.send('Input.dispatchKeyEvent', { type: 'keyUp', key: k, code: k, windowsVirtualKeyCode: vk }); await sleep(900); };
  const wshot = async (p, name) => { const r = await p.page.send('Page.captureScreenshot', { format: 'png' }); fs.writeFileSync(path.join(shots, 'web-' + name + '.png'), Buffer.from(r.result.data, 'base64')); };
  const focusRoom = async p => { await p.page.waitFor(`!!window.__megaWall && !!S.gid`, 40000, p.name + ' app ready'); await sleep(2000); for (let i = 0; i < 8 && (await p.page.ev(`__megaWall.state().gid`)) !== room; i++) await wkey(p, 'ArrowDown'); };
  const chromeA = await startChrome(out, 'alice', P(29410), 500);
  alice.page = await openPage(chromeA, `http://127.0.0.1:${alice.port}/`, 'alice', { before: "try { localStorage.setItem('commons.tour', '1'); } catch {}" });
  await alice.page.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 900, deviceScaleFactor: 1, mobile: false });
  await focusRoom(alice);
  await alice.page.ev(`document.querySelector('[data-m="paint"]').click()`); await sleep(900);
  await wshot(alice, '01-picker-default');
  await alice.page.ev(`(() => { const h = document.querySelector('#mp-hex'); h.value = '#2a9d8f'; h.dispatchEvent(new Event('input')); })()`); await sleep(1200);
  await wshot(alice, '02-picker-teal');
  await alice.page.ev(`(() => { const h = document.querySelector('#mp-hex'); h.value = '#8a8a8a'; h.dispatchEvent(new Event('input')); document.querySelector('.mp-well[data-w="1"]').click(); })()`); await sleep(400);
  await alice.page.ev(`(() => { const h = document.querySelector('#mp-hex'); h.value = '#8c8c8c'; h.dispatchEvent(new Event('input')); })()`); await sleep(1200);
  await wshot(alice, '03-picker-too-close');
  await alice.page.ev(`document.querySelector('#mp-reset').click()`); await sleep(1500);
  try { process.kill(-alice.bridge.pid, 'SIGTERM'); } catch { } await sleep(1800);

  const kwBusFile = path.join(out, 'kwin-bus');
  sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket wayland-pickshot --no-lockscreen --no-global-shortcuts --width 1280 --height 900`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(out, 'kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, 'wayland-pickshot')), 20000, 'kwin socket');
  const winEnv = { ...cleanEnv(), WAYLAND_DISPLAY: 'wayland-pickshot', DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim() };
  const shot = async name => { const f = path.join(shots, 'native-' + name + '.png'); spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); return f; };
  const fk = async (...steps) => { const r = spawnSync(FAKE, steps, { env: winEnv, encoding: 'utf8' }); if (r.status !== 0) throw new Error('fakekeys ' + steps.join(' ') + ': ' + r.stderr); };
  const wport = P(27310);
  sh(BIN, [], { ...winEnv, COMMONS_DATA: path.join(out, 'data', 'alice'), COMMONS_RELAY: REL, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_UI_TEST: String(wport), MEGA_GPUI_SIZE: '1280x900', RUST_BACKTRACE: '1' }, path.join(out, 'alice-window.log'));
  const W = new Win(wport);
  await until(async () => { try { await W.connect(); return true; } catch { return false; } }, 30000, 'window test port');
  await until(async () => (await W.cmd('core', { method: 'snapshot' })).ok, 30000, 'window core unlocked');
  await sleep(2500);

  const chrome = await startChrome(out, 'bob', P(29400), 500);
  bob.page = await openPage(chrome, `http://127.0.0.1:${bob.port}/`, 'bob', { before: "try { localStorage.setItem('commons.tour', '1'); } catch {}" });
  await bob.page.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 900, deviceScaleFactor: 1, mobile: false });
  await focusRoom(bob);
  const web = expr => bob.page.ev(`(async()=>{ return ${expr}; })()`);
  const webMain = () => web(`(((S.snap.communities.find(c => c.id === ${J(cid)}).style || {}).rooms || {})[${J(room)}] || {}).main || ''`);
  const webHi = () => web(`(((S.snap.communities.find(c => c.id === ${J(cid)}).style || {}).rooms || {})[${J(room)}] || {}).hi || ''`);
  const webCommMain = () => web(`(((S.snap.communities.find(c => c.id === ${J(cid)}).style || {}).community || {}).main) || ''`);
  const webIs = async (fn, want, what) => soft(until(async () => (await fn()) === want, 40000, what, 400));
  check('0 the web member sees the sketches card; the window is Alice (owner)', (await web(`__megaWall.state().gid`)) === room);

  // ---- the window at the sketches card, picker open
  await W.cmd('go', { name: 'Paint club' }); await sleep(1500);
  const nav = await W.cmd('nav');
  const idx = (nav.opens || []).findIndex(o => o.includes(room));
  for (let i = 0; i < idx; i++) { await W.cmd('navkey', { key: 'down' }); await sleep(250); }
  await sleep(1800);
  await W.cmd('paint'); await sleep(800);
  let st = await W.state();
  check('1 the picker opens on the focused card, with its square and hue strip', st.open && !st.external && st.sv[2] > 100 && st.hue[2] > 100, { target: st.target, sv: st.sv });
  const orig = st.cur.slice();
  const webOrig = await webMain();
  await shot('01-picker-default'); await wshot(bob, '01-card-default');

  // ---- A. drag the square with the real pointer
  const sv = st.sv; const OFF = (process.env.PICK_OFF || "-20,-20").split(",").map(Number);   // where the window sits on the screen (found from the first screenshot against the reported bounds)
  const at = (x, y) => `move:${Math.round(x + OFF[0])},${Math.round(y + OFF[1])}`;
  // one fakekeys process holds the button (KWin lets go of a client's buttons when it exits), so the state is read mid-drag
  const fkAsync = (...steps) => new Promise(res => { const c = spawn(FAKE, steps, { env: winEnv, stdio: 'ignore' }); c.on('exit', res); });
  const drag = fkAsync(at(sv[0] + sv[2] * .3, sv[1] + sv[3] * .3), 'mousedown', at(sv[0] + sv[2] * .55, sv[1] + sv[3] * .4), at(sv[0] + sv[2] * .8, sv[1] + sv[3] * .3), 'sleep:3500', 'mouseup');
  await sleep(2200);
  st = await W.state();
  const wantA = hsv2hex(st.hsv[0], .8, .7);
  check('A1 dragging the square: hsv follows the pointer (s .8, v .7)', near(st.hsv[1], .8, .02) && near(st.hsv[2], .7, .02), st.hsv);
  check('A2 live preview: the card colour is the picked colour while the button is still down', dist(st.cur[0], wantA) <= 6, { cur: st.cur[0], want: wantA });
  check('A3 ... and nothing has been sent to the web yet (commit on release)', (await webMain()) === webOrig, await webMain());
  await shot('02-dragging-square');
  await drag; await sleep(700);
  const a = await W.state();
  check('A4 on release the exact colour reaches the web client', (await webIs(webMain, a.cur[0], 'square colour on web')) === true, { native: a.cur[0], web: await webMain() });
  await wshot(bob, '02-card-after-square');

  // ---- B. drag the hue strip
  const hu = st.hue;
  await fk(at(hu[0] + hu[2] * .1, hu[1] + hu[3] / 2), 'mousedown', at(hu[0] + hu[2] * .35, hu[1] + hu[3] / 2), at(hu[0] + hu[2] * .5, hu[1] + hu[3] / 2), 'mouseup');
  await sleep(600);
  const b = await W.state();
  check('B1 dragging the hue strip: hue is half way round (180)', near(b.hsv[0], 180, 3) && near(b.hsv[1], .8, .02), b.hsv);
  check('B2 the hue colour reaches the web client exactly', (await webIs(webMain, b.cur[0], 'hue colour on web')) === true, { native: b.cur[0], web: await webMain() });

  // ---- C. type a hex (real keys into the hex box)
  const hexY = hu[1] + hu[3] + 16 + 22, hexX = hu[0] + hu[2] * .5;
  await fk(at(hexX, hexY), 'click', 'key:ctrl+a', 'type:#2a9d8f'); await sleep(1500);
  const c = await W.state();
  check('C1 typing #2a9d8f sets the colour', c.cur[0] === '#2a9d8f', c.cur);
  check('C2 the web client sees exactly #2a9d8f', (await webIs(webMain, '#2a9d8f', 'typed hex on web')) === true, await webMain());
  await fk('key:ctrl+a', 'type:#abc'); await sleep(1500);
  const c3 = await W.state();
  check('C3 a three-digit #abc is accepted (#aabbcc)', c3.cur[0] === '#aabbcc', c3.cur);
  await fk('key:ctrl+a', 'type:#zz12'); await sleep(900);
  check('C4 an invalid hex changes nothing', (await W.state()).cur[0] === '#aabbcc');
  await shot('03-hex-typed');
  await fk('key:ctrl+a', 'type:#2a9d8f'); await sleep(1200);

  // ---- D. arrows nudge the square (after it was pressed)
  await fk(at(sv[0] + sv[2] * .5, sv[1] + sv[3] * .5), 'mousedown', 'mouseup'); await sleep(900);
  const d0 = await W.state();
  await fk('key:left', 'key:left', 'key:left', 'key:up'); await sleep(1500);
  const d1 = await W.state();
  check('D1 arrows nudge the square: three left (-.06), one up (+.02)', near(d0.hsv[1] - d1.hsv[1], .06, .005) && near(d1.hsv[2] - d0.hsv[2], .02, .005), { d0: d0.hsv, d1: d1.hsv });
  check('D2 the nudged colour is committed and the web sees it', (await webIs(webMain, d1.cur[0], 'arrow colour on web')) === true, { native: d1.cur[0], web: await webMain() });

  // ---- E. Escape cancels back to the colour the panel opened with
  await fk(at(sv[0] + sv[2] * .9, sv[1] + sv[3] * .9), 'mousedown', 'mouseup'); await sleep(800);
  await fk('key:escape'); await sleep(900);
  const e = await W.state();
  check('E1 Escape closes the picker', e.open === false);
  check('E2 ... and puts the colours back as they were when it opened', e.cur[0] === orig[0] && e.cur[1] === orig[1], { now: e.cur, orig });
  check('E3 the web client is back to the original too', (await webIs(webMain, webOrig, 'escape on web')) === true, await webMain());

  // ---- F. the too-close warning, presets, reset
  await W.cmd('paint'); await sleep(600);
  await W.paint('hex', { value: '#4b6b8a' }); await sleep(300);
  await W.paint('well', { n: 1 }); await W.paint('hex', { value: '#4d6d8c' }); await sleep(900);
  const f1 = await W.state();
  check('F1 two colours too close (contrast < 1.6) show the warning', f1.warn === true && f1.cur[0] === '#4b6b8a' && f1.cur[1] === '#4d6d8c', f1);
  await shot('04-too-close'); await wshot(bob, '04-card-too-close');
  await W.paint('well', { n: 0 });
  const pr = await W.paint('preset', { n: 2 }); await sleep(700);
  const want = await web(`__mega.api.DEF.slice(0, 8)[2]`);
  check('F2 a preset sets both colours and clears the warning', pr.cur[0] === want[0] && pr.cur[1] === want[1] && !pr.warn, { cur: pr.cur, want });
  check('F3 the preset reaches the web client', (await webIs(webMain, want[0], 'preset on web')) === true && (await webHi()) === want[1]);
  await W.paint('reset'); await sleep(900);
  const rs = await W.state();
  check('F4 reset: both colours go back to the default', rs.cur[0] === orig[0] && rs.cur[1] === orig[1], { cur: rs.cur, orig });
  check('F5 reset reaches the web client (no custom colours)', (await webIs(webMain, '', 'reset on web')) === true && (await webHi()) === '');
  await W.cmd('paint');

  // ---- G. the community settings: custom colours for a room, and for the community
  await W.cmd('panel', { which: 'manage', name: 'Paint club' }); await sleep(900);
  await shot('05-settings');
  const g0 = await W.cmd('paint_for', { cid, room });
  await sleep(700);
  check('G1 "custom" in the community settings opens the picker for that room, over the settings', g0.open && g0.external && g0.target === 'sketches', g0);
  await shot('06-settings-picker');
  await W.paint('hex', { value: '#c1440e' }); await sleep(1500);
  check('G2 a hex typed there reaches the web as the room\'s colour', (await webIs(webMain, '#c1440e', 'settings room colour on web')) === true, await webMain());
  await fk('key:escape'); await sleep(900);
  check('G3 Escape there cancels back and closes', (await W.state()).open === false && (await webIs(webMain, '', 'settings escape on web')) === true);
  const g1 = await W.cmd('paint_for', { cid, room: '' }); await sleep(500);
  await W.paint('hex', { value: '#10a050' }); await sleep(1500);
  check('G4 the community\'s own colours can be picked custom too', g1.target === 'Paint club' && (await webIs(webCommMain, '#10a050', 'community colour on web')) === true, await webCommMain());
  await W.paint('reset'); await sleep(900);
  await W.cmd('paint');
  await W.cmd('panel', { which: 'close', name: 'Paint club' });

  // ---- H. in the chat room
  await W.cmd('open', { name: 'sketches' }); await sleep(2500);
  const h0 = await W.cmd('paint_for', { cid, room }); await sleep(600);
  check('H1 the chat room opens the same picker', h0.open && h0.external && h0.target === 'sketches', h0);
  await W.paint('hex', { value: '#7a3bd6' }); await sleep(1500);
  await shot('07-chat-picker');
  check('H2 a colour picked in the chat reaches the web', (await webIs(webMain, '#7a3bd6', 'chat colour on web')) === true, await webMain());
  await W.paint('reset'); await sleep(600); await W.cmd('paint');
  await W.cmd('home');
} catch (e) { check('the run itself', false, String(e && e.stack || e).slice(0, 400)); }
fs.writeFileSync(path.join(out, 'results.json'), J(results, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED');
stopAll(); process.exit(failed ? 1 : 0);
