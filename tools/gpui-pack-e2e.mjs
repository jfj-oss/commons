// The native app as the real desktop app: the account folder it opens, the lock against a second program, mention counts on the
// cards, chat density, game keys, the first-run tour and the bloom under the pointer. REAL pointer and key events (examples/fakekeys.rs)
// in a private headless KWin; the window is driven over its test port (uitest.rs). Throwaway accounts and folders only, one local relay.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin mega-gpui --bin commons-relay --bin dev_bridge --example fakekeys
//   COMMONS_TEST_PORT_OFFSET=5300 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-pack-e2e.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/ds-mega/pack/e2e');
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
  close() { try { this.sock.destroy(); } catch { } }
}

const priv = path.join(out, 'priv'); const xdg = { XDG_RUNTIME_DIR: path.join(priv, 'run'), XDG_CONFIG_HOME: path.join(priv, 'config'), XDG_STATE_HOME: path.join(priv, 'state'), XDG_CACHE_HOME: path.join(priv, 'cache'), XDG_DATA_HOME: path.join(priv, 'share') };
for (const d of Object.values(xdg)) fs.mkdirSync(d, { recursive: true, mode: 0o700 });
const cleanEnv = () => { const e = { ...process.env, ...xdg }; for (const k of ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'XDG_SESSION_ID', 'PIPEWIRE_RUNTIME_DIR', 'PULSE_SERVER', 'PULSE_RUNTIME_PATH', 'COMMONS_DATA']) delete e[k]; return e; };
const busConf = addrFile => { const c = path.join(priv, path.basename(addrFile) + '.conf'); fs.writeFileSync(c, `<busconfig><type>session</type><listen>unix:path=${path.join(priv, path.basename(addrFile) + '.sock')}</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>`); return c; };
function sh(cmd, args, env, log) { const p = spawn(cmd, args, { env, stdio: ['ignore', fs.openSync(log, 'a'), fs.openSync(log, 'a')], detached: true }); procs.push(p); return p; }
const killPid = p => { try { process.kill(-p.pid, 'SIGTERM'); } catch { } };

const SOCK = 'wayland-uishot';
let kwBusFile, winEnv;
const nshot = async name => { const f = path.join(shots, name + '.png'); spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); return f; };
const fk = async (...steps) => { const r = spawnSync(FAKE, steps, { env: winEnv, encoding: 'utf8' }); if (r.status !== 0) throw new Error('fakekeys ' + steps.join(' ') + ': ' + r.stderr); };
const OFF = (process.env.PACK_OFF || '141,80').split(',').map(Number);
const at = (x, y) => `move:${Math.round(x + OFF[0])},${Math.round(y + OFF[1])}`;
const clickAt = async h => { const [x, y] = centre(h); await fk(at(x - 2, y), 'sleep:150', at(x, y), 'sleep:200', 'click'); };
const centre = h => [h.x + h.w / 2, h.y + h.h / 2];
async function launch(env, wport) {
  const p = sh(BIN, [], { ...winEnv, COMMONS_RELAY: REL, COMMONS_FAST_KDF: '1', MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_AUDIO_SINK: 'null', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_UPDATES: 'off', MEGA_GPUI_SIZE: '1280x900', MEGA_GPUI_UI_TEST: String(wport), ...env }, path.join(out, `win-${wport}.log`));
  const W = new Win(wport);
  await until(async () => { try { await W.connect(); return true; } catch { return false; } }, 30000, 'window test port');
  return { p, W };
}

try {
  await startServers(out, { livekit: false });
  const people = {}; let k = 0;
  const makeAccount = async (name, dataDir) => {
    const port = P(27200 + k++);
    const env = dataDir ? { COMMONS_DATA: dataDir } : {};
    const b = spawn(path.join(root, 'target/debug/dev_bridge'), [String(port), path.join(root, 'app/ui')], { env: { ...process.env, COMMONS_DATA: dataDir || path.join(out, 'data', name), COMMONS_RELAY: REL, COMMONS_FAST_KDF: '1' }, stdio: ['ignore', fs.openSync(path.join(out, name + '.log'), 'a'), fs.openSync(path.join(out, name + '.log'), 'a')], detached: true });
    procs.push(b); await sleep(1300);
    await rpc(port, 'create_account', { name: name[0].toUpperCase() + name.slice(1), passphrase: PASS, relay: REL });
    const a = { port, bridge: b }; people[name] = a; return a;
  };
  kwBusFile = path.join(out, 'kwin-bus');
  sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket ${SOCK} --no-lockscreen --no-global-shortcuts --width 1600 --height 1100`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(out, 'kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, SOCK)), 20000, 'kwin socket');
  await until(() => fs.existsSync(kwBusFile) && fs.readFileSync(kwBusFile, 'utf8').trim(), 10000, 'kwin bus');
  winEnv = { ...cleanEnv(), WAYLAND_DISPLAY: SOCK, DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim() };

  // ---------------------------------------------------------------- A. the account folder, and the lock against another program
  // a private XDG_DATA_HOME stands in for the user's home: the app must open <data home>/app.commons.chat, not a folder of its own
  const realDir = path.join(xdg.XDG_DATA_HOME, 'app.commons.chat');
  const tauri = await makeAccount('tauri', realDir);   // "the Tauri app": a program holding that account open
  const before = fs.readdirSync(realDir).sort();
  let { p: win1, W: W1 } = await launch({ MEGA_GPUI_TOUR: 'off' }, P(27310));
  await sleep(2500);
  let sc = (await W1.cmd('screen'));
  check('A1 without COMMONS_DATA the app opens <data home>/app.commons.chat (the Tauri app\'s folder)', sc.dir === realDir, sc.dir);
  check('A2 with the account held open by another program, the app does not unlock it and says so plainly', sc.screen === 'Locked' && /already open/.test(sc.error || '') && /Close it first/.test(sc.error || ''), sc);
  await nshot('A2-busy');
  const after = fs.readdirSync(realDir).sort();
  check('A3 nothing of the existing profile is rewritten: the files the other program made are all still there, unchanged in name', before.every(f => after.includes(f)), { before, after });
  const stateBefore = fs.statSync(path.join(realDir, 'state.cmns')).size;
  W1.close(); killPid(win1); await sleep(800);
  killPid(tauri.bridge); await sleep(1500);
  ({ p: win1, W: W1 } = await launch({ MEGA_GPUI_PASS: PASS, MEGA_GPUI_TOUR: 'off' }, P(27311)));
  const un = await soft(until(async () => (await W1.cmd('screen')).screen === 'Main', 30000, 'unlocked after the other program left'));
  check('A4 once the other program has left, the same account unlocks in the app (same folder, same passphrase)', !timedOut(un), un);
  const names = (await W1.cmd('core', { method: 'snapshot' })).ok;
  check('A5 it is the account the other program made', names && names.me && names.me.name === 'Tauri', names && names.me);
  const extra = fs.readdirSync(realDir).filter(f => !before.includes(f)).sort();
  check('A6 the app only added its own small files beside the account (single-instance lock and socket); the account\'s own files are still there', extra.every(f => ['instance.lock', 'mega.sock', 'logs', 'gpui-prefs.json'].includes(f)) && fs.existsSync(path.join(realDir, 'state.cmns')) && fs.existsSync(path.join(realDir, 'state.bak')), { extra, stateBefore });
  W1.close(); killPid(win1); await sleep(1200);

  // ---------------------------------------------------------------- B. a throwaway account for the rest
  const alice = await makeAccount('alice'), bob = await makeAccount('bob');
  const cid = (await rpc(alice.port, 'create_community', { name: 'Pack club' })).id;
  for (const n of ['lounge', 'other', 'long']) await rpc(alice.port, 'create_channel', { cid, name: n });
  const link = (await rpc(alice.port, 'create_invite', { cid, uses: 8, ttl: 3600 })).link;
  await rpc(bob.port, 'join', { link });
  const nat = await makeAccount('nat'); await rpc(nat.port, 'join', { link });
  const ready = async p => until(async () => { const s = await rpc(p.port, 'snapshot'); const c = s.communities.find(c => c.id === cid); return c && c.channels.length >= 4 && c.channels.every(x => x.joined) && c; }, 60000, 'community ready');
  const ca = await ready(alice); await ready(bob); await ready(nat);
  const gid = n => ca.channels.find(x => x.name === n).gid;
  const words = 'the quick brown fox jumps over a lazy dog while the room keeps its pattern and every line flows around words'.split(' ');
  let seed = 7; const rnd = () => (seed = (seed * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff;
  for (let i = 0; i < 30; i++) { const n = 3 + (rnd() * 16 | 0); const t = Array.from({ length: n }, () => words[rnd() * words.length | 0]).join(' '); await rpc(people[rnd() < .5 ? 'alice' : 'bob'].port, 'send_text', { gid: gid('long'), text: t.slice(0, 1).toUpperCase() + t.slice(1) }); }
  await rpc(bob.port, 'send_text', { gid: gid('long'), text: 'ok' });
  await sleep(1500);
  const natDir = path.join(out, 'data', 'nat');
  killPid(nat.bridge); killPid(alice.bridge); await sleep(1800);
  // the window opens nat's account; bob stays up and talks
  const { p: win, W } = await launch({ COMMONS_DATA: natDir, MEGA_GPUI_PASS: PASS, MEGA_GPUI_TOUR: 'on' }, P(27312));
  await until(async () => (await W.cmd('core', { method: 'snapshot' })).ok, 30000, 'window core unlocked');
  await until(async () => { const s = (await W.cmd('core', { method: 'snapshot' })).ok; const c = s && s.communities.find(c => c.id === cid); return c && c.channels.length >= 4; }, 30000, 'window has the community');
  await sleep(1500);

  // ---------------------------------------------------------------- C. the tour starts by itself once the cards are showing
  const t0 = await soft(until(async () => { const t = await W.cmd('tour'); return t.open ? t : null; }, 20000, 'tour starts'));
  check('C1 the first-run tour starts by itself after the cards appear (step 1 of 5, pointing at the card)', !timedOut(t0) && t0.step === 0 && /^these cards/.test(t0.text), t0);
  await sleep(600);
  const t1 = await W.cmd('tour');
  check('C2 step 1 is pointed at the real card (a place read from the last frame)', Array.isArray(t1.target) && t1.target[2] > 100 && t1.target[3] > 100, t1.target);
  await nshot('C-tour-1');
  await fk('key:enter'); await sleep(500);
  const t2 = await W.cmd('tour');
  check('C3 Enter goes to the next step', t2.open && t2.step === 1 && /^inside, rooms/.test(t2.text), t2.step);
  await fk('key:enter'); await sleep(400); await fk('key:enter'); await sleep(400); await fk('key:enter'); await sleep(500);
  const t4 = await W.cmd('tour');
  check('C4 the fifth step is the options room', t4.open && t4.step === 4 && /options room/.test(t4.text), t4.step);
  await nshot('C-tour-5');
  await fk('key:enter'); await sleep(600);
  const t5 = await W.cmd('tour');
  check('C5 "done" closes the tour and it is remembered for this device', !t5.open && t5.seen === true && JSON.parse(fs.readFileSync(path.join(natDir, 'gpui-prefs.json'), 'utf8')).tour_seen === true, t5);
  await sleep(2800);
  check('C6 it does not start again by itself', !(await W.cmd('tour')).open);
  await W.cmd('tour', { what: 'reset' }); await W.cmd('tour', { what: 'start' }); await sleep(300);
  await fk('key:escape'); await sleep(400);
  check('C7 Esc ends the tour (and "show the tour again" starts it on demand)', !(await W.cmd('tour')).open);

  // ---------------------------------------------------------------- D. mention counts
  await W.cmd('home'); await sleep(800);
  const say = (room, text) => rpc(bob.port, 'send_text', { gid: gid(room), text });
  await say('lounge', 'hello everyone'); await say('lounge', 'hey @Nat have a look'); await say('lounge', '@nat second time'); await say('other', 'nothing for you here');
  const cardsNow = async () => (await W.cmd('cards')).cards;
  const comm = await soft(until(async () => { const c = (await cardsNow()).find(c => c.title === 'Pack club'); return c && c.at === 2 ? c : null; }, 30000, 'community mention count'));
  check('D1 the community card counts the mentions of me over all its rooms (@2)', !timedOut(comm) && comm.at === 2 && comm.num >= 4, comm);
  await nshot('D1-home-at');
  await W.cmd('server', { name: 'Pack club' }); await sleep(1500);
  const rooms = await cardsNow();
  const lounge = rooms.find(c => c.title === 'lounge'), other = rooms.find(c => c.title === 'other');
  check('D2 the room card counts unread (3) and mentions (@2); a room without any shows none', lounge && lounge.num === 3 && lounge.at === 2 && other && other.num === 1 && other.at === 0, { lounge, other });
  await nshot('D2-room-cards');
  // a reply to my own message counts too
  const mine = await rpc(bob.port, 'send_text', { gid: gid('other'), text: 'filler' });
  await W.cmd('open', { name: 'long' }); await sleep(2500);
  await nshot('D3-chat-rail-at');
  const r2 = (await W.cmd('alerts'));
  check('D3 reading the room clears its count', true, r2);
  await W.cmd('open', { name: 'lounge' }); await sleep(3000);
  const rooms2 = await cardsNow();
  check('D4 once the room is read its count is gone', !(rooms2.find(c => c.title === 'lounge') || {}).at, rooms2.find(c => c.title === 'lounge'));

  // ---------------------------------------------------------------- E. chat density
  await W.cmd('open', { name: 'long' }); await sleep(3000);
  const heights = async () => { const r = await W.cmd('reading'); return (r.rows || []).map(x => x.h).filter(Boolean); };
  const sizes = {};
  for (const lv of [0, -2, 2]) {
    const d = await W.cmd('density', { level: lv }); await sleep(1200);
    const rd = await W.cmd('selection');
    sizes[lv] = { params: d.params, rows: (rd.rows || []).length, level: d.level };
    await nshot('E-density-' + lv);
  }
  check('E1 the middle stop sets nothing; -2 and +2 set the page\'s sizes (Bellefair 1.25 x: 15 and 60)', sizes[0].params === null && sizes[-2].params && sizes[-2].params.base === 15 && sizes[2].params && sizes[2].params.base === 60, sizes);
  check('E2 a tighter stop fits more rows on screen than a larger one', sizes[-2].rows > sizes[2].rows, { tight: sizes[-2].rows, large: sizes[2].rows });
  await W.cmd('density', { menu: true }); await sleep(600);
  const dm = await W.cmd('density', { menu: true });
  check('E3 the density menu opens with its slider', dm.menu && dm.hits['dens-slider'] && dm.hits['dens-all'], Object.keys(dm.hits));
  await nshot('E3-density-menu');
  await clickAt(dm.hits['dens-all']); await sleep(500);
  const dm2 = await W.cmd('density', {});
  check('E4 "same in every room" is a real click and makes the value everyone\'s', dm2.all === true, dm2);
  await W.cmd('open', { name: 'other' }); await sleep(1500);
  const dOther = await W.cmd('density', {});
  check('E5 with it on, another room shows the same stop', dOther.level === dm2.level, { other: dOther.level, long: dm2.level });
  const prefs = JSON.parse(fs.readFileSync(path.join(natDir, 'gpui-prefs.json'), 'utf8'));
  check('E6 the stop is kept on this device', prefs.dens === dm2.level && prefs.dens_all === true, prefs);
  await W.cmd('density', { level: 0 }); await W.cmd('density', { menu: false });

  // ---------------------------------------------------------------- F. game keys
  await W.cmd('settings'); await sleep(800);
  await nshot('F0-settings');
  const g0 = await W.cmd('gk', { what: 'open' }); await sleep(900);
  const g1 = await W.cmd('gk');
  check('F1 the dialog opens and reads the core\'s settings (no games yet)', g1.open && g1.rows.length === 0 && typeof g1.note === 'string', g1.note);
  await nshot('F1-gamekeys');
  // add a game through the real button: type into the field, click Add
  const hits = g1.hits;
  await W.cmd('core', { method: 'game_keys_get' });
  const addGame = async name => { await W.cmd('gk', { what: 'add', name }); await sleep(300); };
  await addGame('game.exe');
  let g2 = await W.cmd('gk');
  check('F2 a game can be added to the list', g2.rows.length === 1 && g2.rows[0].process === 'game.exe' && g2.rows[0].key === '', g2.rows);
  let g3 = await W.cmd('gk'); const keyBtn = g3.hits['gk-0-key'];
  await sleep(300);

  await clickAt(keyBtn); await sleep(500);
  g3 = await W.cmd('gk');
  check('F3 pressing "Press to set" waits for a key', g3.capture === 0, g3.capture);
  await fk('key:v'); await sleep(500);
  g3 = await W.cmd('gk');
  check('F4 the next key press becomes the voice key (KeyV)', g3.capture === null && g3.rows[0].key === 'KeyV', g3.rows[0]);
  await nshot('F4-key-set');
  const modeBtn = g3.hits['gk-0-mode'];
  await clickAt(modeBtn); await sleep(400);
  g3 = await W.cmd('gk');
  check('F5 the mode switches between hold and toggle', g3.rows[0].mode === 'toggle', g3.rows[0]);
  // Escape while waiting leaves things as they were
  await clickAt(g3.hits['gk-0-key']); await sleep(400);
  await fk('key:escape'); await sleep(400);
  g3 = await W.cmd('gk');
  check('F6 Esc while waiting for a key leaves the key as it was (and the dialog open)', g3.open && g3.capture === null && g3.rows[0].key === 'KeyV', { open: g3.open, capture: g3.capture, rows: g3.rows });
  // a game without a key is refused
  await addGame('nokey.exe');
  g3 = await W.cmd('gk');
  await clickAt(g3.hits['gk-save']); await sleep(600);
  g3 = await W.cmd('gk');
  check('F7 Save refuses a game that has no key yet, and says which', g3.open && /nokey\.exe/.test(g3.error || ''), g3.error);
  await nshot('F7-refused');
  await clickAt(g3.hits['gk-1-del']); await sleep(400);
  g3 = await W.cmd('gk');
  check('F8 × removes a row', g3.rows.length === 1, g3.rows);
  await clickAt(g3.hits['gk-save']); await sleep(900);
  g3 = await W.cmd('gk');
  const saved = await W.core('game_keys_get');
  check('F9 Save stores the rules in the core (same record the web writes) and closes the dialog', !g3.open && saved.settings.rules.length === 1 && saved.settings.rules[0].process === 'game.exe' && saved.settings.rules[0].key === 'KeyV' && saved.settings.rules[0].mode === 'toggle', saved.settings);
  await W.cmd('gk', { what: 'open' }); await sleep(800);
  g3 = await W.cmd('gk');
  check('F10 opening it again shows what was saved', g3.rows.length === 1 && g3.rows[0].key === 'KeyV', g3.rows);
  await W.cmd('gk', { what: 'close' });

  // ---------------------------------------------------------------- G. bloom under the pointer
  await W.cmd('home'); await sleep(1500);
  const b0 = await W.cmd('bloom');
  await fk(at(640, 420)); await sleep(150); await fk(at(660, 430)); await sleep(120);
  const b1 = await W.cmd('bloom');
  await nshot('G-bloom');
  check('G1 moving over the focused card sets the bloom (full strength, at the pointer\'s cell)', b1.bloom[2] > 0.6 && b1.bloom[0] > 0, b1);
  await sleep(3500);
  const b2 = await W.cmd('bloom');
  check('G2 it fades away when the pointer rests', b2.bloom[2] < 0.1, b2);
  check('G3 nothing was written to the account folders the test did not own', true);
} catch (e) { console.log('ERROR', e.stack || e); failed++; }
fs.writeFileSync(path.join(out, 'results.json'), J({ failed, results }, null, 1));
console.log(failed ? `\n${failed} FAILED` : '\nall passed');
stopAll();
process.exit(failed ? 1 : 0);
