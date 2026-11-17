// Selecting and copying message text in the native window (select.rs), with REAL pointer and key events (examples/fakekeys.rs) in a private headless KWin;
// the clipboard is read back from that compositor (wl-paste) and from the app. Was: The open room's chat screen (reading zone, header, column, day lines, owner tags, read ticks, composer), native window against the web client: side-by-side screenshots and scroll frame sequences. Alice is the native WINDOW
// (`mega-gpui`) in a private headless KWin, driven over its test port (uitest.rs); Bob is the real app/ui in headless Chrome for Testing.
// One local relay, throwaway accounts, silent. Nobody joins a real room.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin mega-gpui --bin commons-relay --bin dev_bridge
//   COMMONS_TEST_PORT_OFFSET=2700 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-chat-shots.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, startChrome, openPage, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-select-e2e');
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




const FAKE = process.env.MEGA_GPUI_FAKEKEYS || path.join(root, 'target/release/examples/fakekeys');
const SOCK = 'wayland-selshot';
try {
  await startServers(out, { livekit: false });
  const people = {}; let k = 0;
  const makeAccount = async name => { const port = P(27200 + k++); const b = startBridge(out, name, port); await sleep(1300); await rpc(port, 'create_account', { name: name[0].toUpperCase() + name.slice(1), passphrase: PASS }); const a = { port, bridge: b }; people[name] = a; return a; };
  const alice = await makeAccount('alice'), bob = await makeAccount('bob');
  const cid = (await rpc(alice.port, 'create_community', { name: 'Select club' })).id;
  for (const n of ['lounge', 'long']) await rpc(alice.port, 'create_channel', { cid, name: n });
  const link = (await rpc(alice.port, 'create_invite', { cid, uses: 5, ttl: 3600 })).link;
  await rpc(bob.port, 'join', { link });
  const ready = async p => until(async () => { const s = await rpc(p.port, 'snapshot'); const c = s.communities.find(c => c.id === cid); return c && c.channels.length >= 3 && c.channels.every(x => x.j !== undefined ? true : true) && c; }, 40000, 'community ready');
  const ca = await ready(alice); await ready(bob);
  const gid = n => ca.channels.find(x => x.name === n).gid;
  const talk = [['alice', 'Welcome to the club. Copy this sentence with the mouse.'], ['bob', 'Thanks, glad to be here.'], ['alice', 'Two lines of words that are long enough to wrap around the column when the window is narrow, so a drag can start in the middle of the first line and end in the second.'],
    ['bob', 'Second message from Bob.'], ['alice', 'Third message from Alice.'], ['bob', 'Short one.']];
  for (const [w, t] of talk) { await rpc(people[w].port, 'send_text', { gid: gid('lounge'), text: t }); await sleep(150); }
  const words = 'the quick brown fox jumps over a lazy dog while the room keeps its pattern and every line flows around words'.split(' ');
  let seed = 7; const rnd = () => (seed = (seed * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff;
  for (let i = 0; i < 80; i++) { const n = 4 + (rnd() * 20 | 0); const t = Array.from({ length: n }, () => words[rnd() * words.length | 0]).join(' '); await rpc(people[rnd() < .5 ? 'alice' : 'bob'].port, 'send_text', { gid: gid('long'), text: t.slice(0, 1).toUpperCase() + t.slice(1) }); }
  await sleep(2000);
  try { process.kill(-alice.bridge.pid, 'SIGTERM'); } catch { } await sleep(1800);

  const kwBusFile = path.join(out, 'kwin-bus');
  sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket ${SOCK} --no-lockscreen --no-global-shortcuts --width 1600 --height 1100`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(out, 'kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, SOCK)), 20000, 'kwin socket');
  const nshot = async name => { const f = path.join(shots, name + '.png'); spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); return f; };
  const wport = P(27310);
  const winEnv = { ...cleanEnv(), WAYLAND_DISPLAY: SOCK, DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim() };
  sh(BIN, [], { ...winEnv, COMMONS_DATA: path.join(out, 'data', 'alice'), COMMONS_RELAY: REL, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_UI_TEST: String(wport), MEGA_GPUI_SIZE: '1280x900', RUST_BACKTRACE: '1' }, path.join(out, 'alice-window.log'));
  const W = new Win(wport);
  await until(async () => { try { await W.connect(); return true; } catch { return false; } }, 30000, 'window test port');
  await until(async () => (await W.cmd('core', { method: 'snapshot' })).ok, 30000, 'window core unlocked');
  await sleep(2500);
  // real input: fakekeys steps against the private compositor
  const fk = async (...steps) => { const r = spawnSync(FAKE, steps, { env: winEnv, encoding: 'utf8' }); if (r.status !== 0) throw new Error('fakekeys ' + steps.join(' ') + ': ' + r.stderr); };
  const paste = () => spawnSync('wl-paste', ['--no-newline'], { env: winEnv, encoding: 'utf8', timeout: 4000 }).stdout || '';
  const sel = () => W.cmd('selection');
  await W.cmd('go', { name: 'Select club' }); await sleep(1500);
  await W.cmd('open', { name: 'lounge' }); await sleep(3500);
  let s = await sel();
  console.log('rows', J(s.rows).slice(0, 1200));
  // where the window sits on the screen: the window is the whole output unless the compositor decorates it; found from the first screenshot
  const OFF = (process.env.SELECT_OFF || '0,0').split(',').map(Number);
  const at = (x, y) => `move:${Math.round(x + OFF[0])},${Math.round(y + OFF[1])}`;
  const row = (rows, text) => rows.find(r => r.text.startsWith(text));
  const find = async t => { const r = row((await sel()).rows, t); if (!r) throw new Error('row not drawn: ' + t); return r; };
  const clear = async () => { await fk('key:escape'); await sleep(250); };

  // 1. drag inside one message: from the start of "Copy" to the end of "mouse"
  let r1 = await find('Welcome');
  await fk(at(r1.x + 1, r1.y + r1.h / 2), 'mousedown', at(r1.x + r1.w * .6, r1.y + r1.h / 2), 'mouseup'); await sleep(400);
  await nshot('1-drag-one');
  s = await sel();
  check('S1 a drag inside one message selects part of its words', s.selected.length > 5 && r1.text.includes(s.selected), s.selected);
  await fk('key:ctrl+c'); await sleep(500);
  const c1 = paste(); const a1 = (await W.cmd('clipboard')).text;
  check('S2 Ctrl+C puts the picked words on the clipboard (wl-paste in the private compositor)', c1 === s.selected && c1.length > 5, J({ c1, a1 }));
  check('S2b the app reads back the same text', a1 === s.selected, a1);

  // 2. across messages
  const rows = (await sel()).rows.sort((a, b) => a.y - b.y);
  const i0 = rows.findIndex(r => r.text.startsWith('Thanks')), ra = rows[i0], rb = rows[i0 + 2];
  await fk(at(ra.x + 20, ra.y + ra.h / 2), 'mousedown', at(rb.x + 40, rb.y + rb.h / 2), 'mouseup'); await sleep(400);
  await nshot('2-drag-across');
  s = await sel();
  const lines = s.selected.split('\n');
  check('S3 a drag across messages selects from one to the other', lines.length === 3 && /^Bob: /.test(lines[0]) && /^Alice: /.test(lines[1]) && /^Bob: /.test(lines[2]), s.selected);
  await fk('key:ctrl+c'); await sleep(500);
  check('S4 several messages copy as "Name: message" lines', paste() === s.selected, paste());

  // 3. double-click a word, triple-click a message
  await clear();
  let r3 = await find('Third');
  await fk(at(r3.x + 6, r3.y + r3.h / 2), 'dblclick'); await sleep(400);
  s = await sel();
  check('S5 double-click selects the word', s.selected === 'Third', s.selected);
  await nshot('3-word');
  await clear();
  await fk(at(r3.x + 6, r3.y + r3.h / 2), 'click', 'dblclick'); await sleep(400);
  s = await sel();
  check('S6 triple-click selects the message', s.selected === 'Third message from Alice.', s.selected);
  await nshot('4-message');

  // 4. Ctrl+A in the chat selects the room's text, nothing outside it
  await fk('key:ctrl+a'); await sleep(400);
  s = await sel();
  check('S7 Ctrl+A in the chat selects the conversation', s.selected.split('\n').length === 6 && s.selected.startsWith('Alice: Welcome') && s.selected.endsWith('Bob: Short one.'), s.selected);
  await nshot('5-all');

  // 5. right-click menu
  await clear();
  await fk(at(r3.x + 6, r3.y + r3.h / 2), 'dblclick'); await sleep(300);
  await fk('click'); await sleep(300);
  await W.cmd('clipboard');
  // a click on the empty part of the room clears the selection
  await fk(at(r3.x + r3.w + 200, r3.y + r3.h / 2), 'click'); await sleep(300);
  s = await sel();
  check('S8 a click on nothing clears the selection', s.selected === '', s.selected);

  // 6. a room scrolled back (the reading zone draws far rows smaller): select across rows, small and whole
  await W.cmd('open', { name: 'long' }); await sleep(3000);
  await W.cmd('scroll', { end: true }); await sleep(500);
  await W.cmd('scroll', { dy: 500 }); await sleep(900);
  const rd = await W.cmd('reading');
  const small = (await sel()).rows;
  const scaled = rd.rows.filter(x => x.k < 1);
  check('S9 the reading zone has scaled rows in view', scaled.length > 0, rd.rows.map(x => x.k));
  const sr = small.sort((a, b) => a.y - b.y);
  const top = sr.find(r => rd.rows.some(x => x.ix === r.ix && x.k < .95)) || sr[0];
  const bot = sr[sr.length - 1];
  await fk(at(top.x + 4, top.y + top.h / 2), 'mousedown', at(bot.x + 30, bot.y + bot.h / 2), 'mouseup'); await sleep(500);
  await nshot('6-scaled');
  s = await sel();
  const n = s.selected.split('\n').length;
  check('S10 a drag over rows the zone draws smaller selects them all', n >= 3 && n === small.filter(r => r.y >= top.y - 1 && r.y <= bot.y + 1).length, J({ n, drawn: small.length }));
  await fk('key:ctrl+c'); await sleep(500);
  check('S11 and copies them', paste() === s.selected, paste().slice(0, 80));
  // typing after a selection goes to the composer
  await fk('type:hi'); await sleep(400);
  s = await sel();
  check('S12 typing clears the selection', s.selected === '', s.selected);
  await nshot('7-typed');
} catch (e) { check('the run itself', false, String(e && e.stack || e).slice(0, 600)); }
fs.writeFileSync(path.join(out, 'results.json'), J(results, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED');
stopAll(); process.exit(failed ? 1 : 0);
