// The open room's chat screen (reading zone, header, column, day lines, owner tags, read ticks, composer), native window against the web client: side-by-side screenshots and scroll frame sequences. Alice is the native WINDOW
// (`mega-gpui`) in a private headless KWin, driven over its test port (uitest.rs); Bob is the real app/ui in headless Chrome for Testing.
// One local relay, throwaway accounts, silent. Nobody joins a real room.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin mega-gpui --bin commons-relay --bin dev_bridge
//   COMMONS_TEST_PORT_OFFSET=2700 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-chat-shots.mjs <outdir>
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



try {
  await startServers(out, { livekit: false });
  const people = {}; let k = 0;
  const makeAccount = async name => { const port = P(27200 + k++); const b = startBridge(out, name, port); await sleep(1300); await rpc(port, 'create_account', { name: name[0].toUpperCase() + name.slice(1), passphrase: PASS, relay: REL }); people[name] = { name, port, account: (await rpc(port, 'snapshot')).me.account, bridge: b }; return people[name]; };
  const alice = await makeAccount('alice'), bob = await makeAccount('bob');
  const cid = (await rpc(alice.port, 'create_community', { name: 'Ascii club' })).id;
  for (const n of ['lounge', 'news', 'long']) await rpc(alice.port, 'create_channel', { cid, name: n });
  const link = (await rpc(alice.port, 'create_invite', { cid, uses: 5, ttl: 3600 })).link;
  await rpc(bob.port, 'join', { link });
  const ready = async p => until(async () => { const s = await rpc(p.port, 'snapshot'); const c = s.communities.find(c => c.id === cid); return c && c.channels.length >= 4 && c.channels.every(x => x.joined && x.status === 'ok') ? c : null; }, 90000, p.name + ' ready', 500);
  const ca = await ready(alice); await ready(bob);
  const gid = n => ca.channels.find(x => x.name === n).gid;
  const talk = [['alice', 'Welcome to the club. This room has the default look.'], ['bob', 'Thanks, glad to be here.'], ['alice', 'The backdrop is the room\'s own ASCII, dim, and it flows around every word.'],
    ['bob', 'It moves a little when somebody writes, then settles.'], ['alice', 'Right: a ring leaves the new message and the room warms up for a while.'], ['bob', 'And with the battery saver on it holds still.'],
    ['alice', 'Short one.'], ['bob', 'A longer line to see the pattern wrap around a wide block of words, which is what the page does too.']];
  for (const [w, t] of talk) { await rpc(people[w].port, 'send_text', { gid: gid('lounge'), text: t }); await sleep(120); }
  // "news": nobody has opened it yet, so both see a "N new" line
  for (const [w, t] of [['alice', 'Anything new today?'], ['bob', 'Not really, the same as yesterday.'], ['alice', 'Hello Bob, a fresh one.'], ['bob', 'Hi!'], ['alice', 'Ok.'], ['bob', 'Alright then, see you tomorrow at the usual time?']]) { await rpc(people[w].port, 'send_text', { gid: gid('news'), text: t }); await sleep(150); }
  // "long": 300 messages of every length, both writers
  const words = 'the quick brown fox jumps over a lazy dog while the room keeps its pattern and every line flows around words'.split(' ');
  let seed = 7; const rnd = () => (seed = (seed * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff;
  for (let i = 0; i < 300; i++) {
    const n = rnd() < .2 ? 1 + (rnd() * 3 | 0) : rnd() < .5 ? 4 + (rnd() * 8 | 0) : 10 + (rnd() * 26 | 0);
    const t = Array.from({ length: n }, () => words[rnd() * words.length | 0]).join(' ');
    await rpc(people[rnd() < .5 ? 'alice' : 'bob'].port, 'send_text', { gid: gid('long'), text: t.slice(0, 1).toUpperCase() + t.slice(1) + (i % 17 === 0 ? ' @Alice hello' : '') });
  }
  await sleep(2500);
  try { process.kill(-alice.bridge.pid, 'SIGTERM'); } catch { } await sleep(1800);

  const kwBusFile = path.join(out, 'kwin-bus');
  sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket wayland-roomascii --no-lockscreen --no-global-shortcuts --width 1600 --height 1100`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(out, 'kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, 'wayland-roomascii')), 20000, 'kwin socket');
  const nshot = async name => { const f = path.join(shots, name + '.png'); spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); return f; };
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
  const webOpen = async name => {
    for (let i = 0; i < 3 && (await web(`S.gid`)); i++) { await key('Escape'); await sleep(900); }
    await web(`(() => { const L = __mega.list; L.go(L.items.findIndex(x => x.name === ${J(name)}), true); })()`);
    await until(async () => (await web(`__mega.list.item.name`)) === name && (await web(`__mega.list.settled`)), 15000, 'web card in focus: ' + name, 300); await sleep(600);
    await key('Enter'); await sleep(3500);
  };
  const wheelWeb = async dy => bob.page.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: 640, y: 450, deltaX: 0, deltaY: dy });
  const pairs = [];
  await W.cmd('go', { name: 'Ascii club' }); await sleep(1500);
  // the web first (it reads Alice's messages, so Alice's own rows get the double check), then the window
  const pair = async (nm, room, extra) => {
    await webOpen(room); await sleep(1200);
    if (extra) await extra('web');
    await wshot(nm + '-web');
    await W.cmd('open', { name: room }); await sleep(3000);
    if (extra) await extra('native');
    await nshot(nm + '-native'); pairs.push(nm);
  };
  await pair('1-rest', 'lounge');
  await pair('2-new-line', 'news');
  // reading zone: a long room scrolled back
  const frames = async (tag, dy, n) => {
    for (let i = 0; i < n; i++) {
      if (tag === 'web') await wheelWeb(dy); else await W.cmd('scroll', { dy });
      await sleep(tag === 'web' ? 500 : 250);
      await (tag === 'web' ? wshot : nshot)(`seq-${String(i).padStart(2, '0')}-${tag}`);
    }
  };
  await pair('3-long-rest', 'long');
  await pair('4-long-scrolled', 'long', async tag => { if (tag === 'web') { await wheelWeb(-1400); await sleep(900); } else { await W.cmd('scroll', { dy: 1400 }); await sleep(600); } });
  // the frame sequence: both start at the end of the long room and scroll back in steps
  await webOpen('long'); await W.cmd('open', { name: 'long' }); await sleep(2500);
  await W.cmd('scroll', { end: true });
  await frames('web', -160, 9); await frames('native', 160, 9);
  const rd = await W.cmd('reading'); console.log('reading', J(rd).slice(0, 1500));
  check('Z1 the zone scales rows away from the middle and leaves the middle whole', rd.rows && rd.rows.some(r => r.k === 1) && rd.rows.some(r => r.k < 1 && r.k >= .8), rd.rows);
  fs.writeFileSync(path.join(out, 'pairs.json'), J(pairs));
  // scroll frame times on the 200-row window of the long room: the window is started again with the automatic scroll (bench.rs), no test port
  try { process.kill(-winProc.pid, 'SIGTERM'); } catch { } await sleep(2500);
  const marks = path.join(out, 'marks.jsonl'); fs.rmSync(marks, { force: true });
  sh(BIN, [], { ...cleanEnv(), WAYLAND_DISPLAY: 'wayland-roomascii', DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim(), COMMONS_DATA: path.join(out, 'data', 'alice'), COMMONS_RELAY: REL, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_SIZE: '1280x900', MEGA_GPUI_OPEN: 'long', MEGA_GPUI_SCROLL: '600', MEGA_BENCH_OUT: marks }, path.join(out, 'bench-window.log'));
  const done = await until(() => { try { return fs.readFileSync(marks, 'utf8').split('\n').filter(Boolean).map(l => JSON.parse(l)).find(m => m.mark === 'scroll_done'); } catch { return null; } }, 90000, 'scroll bench');
  console.log('scroll frames', J(done.summary));
  check('F1 scrolling the long room: p95 <= 17 ms and no frame over 20 ms', done.summary.p95 <= 17 && done.summary.over_20ms === 0, done.summary);
} catch (e) { check('the run itself', false, String(e && e.stack || e).slice(0, 400)); }
fs.writeFileSync(path.join(out, 'results.json'), J(results, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED');
stopAll(); process.exit(failed ? 1 : 0);
