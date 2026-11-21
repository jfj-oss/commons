// Notifications, the chime, the taskbar and one window per account, in the native app, against people who use the web
// client's core (dev_bridge: the same core the web page talks to). Nat is the native WINDOW (`mega-gpui`) inside a private headless
// KWin, driven over its test port (app/gpui/src/uitest.rs); Alice and Bob are bridges.
//
// Silent and private: notifications go to a MOCK org.freedesktop.Notifications on its own private bus (tools/lib/notifmock.py),
// the chime is only recorded in a file (MEGA_GPUI_CHIME_FILE) and the sink is null (MEGA_GPUI_AUDIO_SINK=null), the ask for
// attention is recorded in a file (MEGA_GPUI_ATTENTION_FILE). KWin, the mock and the window run on private buses and a
// private XDG_RUNTIME_DIR/XDG_CONFIG_HOME/XDG_STATE_HOME: nothing of the user's desktop, bus or audio is seen or touched.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin mega-gpui --bin commons-relay --bin dev_bridge
//   COMMONS_TEST_PORT_OFFSET=4900 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-notify-e2e.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, rpc, sleep, stopAll, RELAY_PORT, port as P } from './lib/env.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-notify-e2e');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const shots = path.join(out, 'shots'); fs.mkdirSync(shots, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const PASS = 'correct horse battery';
const J = x => JSON.stringify(x);
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? '  [' + (typeof detail === 'string' ? detail : J(detail)).slice(0, 300) + ']' : ''}`); };
async function until(fn, ms, what, step = 250) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(step); } }
const soft = p => p.catch(e => ({ timeout: String(e.message || e) }));
const lines = f => { try { return fs.readFileSync(f, 'utf8').split('\n').filter(Boolean).map(l => JSON.parse(l)); } catch { return []; } };

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
  dump() { return this.cmd('dump'); }
  alerts() { return this.cmd('alerts'); }
}

const priv = path.join(out, 'priv'); const xdg = { XDG_RUNTIME_DIR: path.join(priv, 'run'), XDG_CONFIG_HOME: path.join(priv, 'config'), XDG_STATE_HOME: path.join(priv, 'state'), XDG_CACHE_HOME: path.join(priv, 'cache'), XDG_DATA_HOME: path.join(priv, 'share') };
for (const d of Object.values(xdg)) fs.mkdirSync(d, { recursive: true, mode: 0o700 });
const cleanEnv = () => { const e = { ...process.env, ...xdg }; for (const k of ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'XDG_SESSION_ID', 'PIPEWIRE_RUNTIME_DIR', 'PULSE_SERVER', 'PULSE_RUNTIME_PATH']) delete e[k]; return e; };
const busConf = addrFile => { const c = path.join(priv, path.basename(addrFile) + '.conf'); fs.writeFileSync(c, `<busconfig><type>session</type><listen>unix:path=${path.join(priv, path.basename(addrFile) + '.sock')}</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>`); return c; };
function sh(cmd, args, env, log, opts = {}) { const p = spawn(cmd, args, { env, stdio: opts.stdin ? ['pipe', fs.openSync(log, 'a'), fs.openSync(log, 'a')] : ['ignore', fs.openSync(log, 'a'), fs.openSync(log, 'a')], detached: true }); return p; }
const stopPid = p => { try { process.kill(-p.pid, 'SIGTERM'); } catch { } };

const res = { checks: results };
let nat, kw, mock;
try {
  await startServers(out, { livekit: false });
  const people = {}; let i = 0;
  for (const n of ['alice', 'bob']) { people[n] = { port: P(27200 + i++) }; startBridge(out, n, people[n].port); }
  await sleep(1500);
  const A = people.alice, B = people.bob;
  for (const [n, p] of Object.entries(people)) await rpc(p.port, 'create_account', { name: n[0].toUpperCase() + n.slice(1), passphrase: PASS, relay: `http://127.0.0.1:${RELAY_PORT}` });
  const cid = (await rpc(A.port, 'create_community', { name: 'Test club' })).id;
  const mk = async name => { const r = await rpc(A.port, 'create_channel', { cid, name, kind: 'text' }); return r.gid; };
  const invite = async () => (await rpc(A.port, 'create_invite', { cid, uses: 6, ttl: 3600 })).link;
  await rpc(B.port, 'join', { link: await invite() });
  const rooms = {};
  for (const n of ['lounge', 'hush', 'only', 'other', 'plain']) rooms[n] = await mk(n);

  // Nat's account, made by a bridge that then stops so the window can open the same folder
  const N = { port: P(27203) };
  const natBridge = startBridge(out, 'nat', N.port); await sleep(1200);
  await rpc(N.port, 'create_account', { name: 'Nat', passphrase: PASS, relay: `http://127.0.0.1:${RELAY_PORT}` });
  await rpc(N.port, 'join', { link: await invite() });
  await until(async () => { const s = await rpc(N.port, 'snapshot'); const c = s.communities.find(c => c.id === cid); return c && Object.keys(rooms).every(n => c.channels.some(ch => ch.name === n && ch.joined && ch.status === 'ok')); }, 60000, 'nat holds the rooms');
  try { process.kill(-natBridge.pid, 'SIGTERM'); } catch { } await sleep(1500);
  await until(async () => { const s = await rpc(B.port, 'snapshot'); const c = s.communities.find(c => c.id === cid); return c && Object.keys(rooms).every(n => c.channels.some(ch => ch.name === n && ch.joined)); }, 60000, 'bob holds the rooms');

  const mockLog = path.join(out, 'notif-mock.jsonl'), mockBusFile = path.join(out, 'notif-bus');
  mock = sh('dbus-run-session', [`--config-file=${busConf(mockBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${mockBusFile}; exec python3 ${path.join(root, 'tools/lib/notifmock.py')} ${mockLog} reply`], cleanEnv(), path.join(out, 'notif-mock.log'), { stdin: true });
  await until(() => fs.existsSync(mockBusFile) && fs.readFileSync(mockBusFile, 'utf8').trim(), 10000, 'mock bus');
  const mockSay = line => mock.stdin.write(line + '\n');
  const kwBusFile = path.join(out, 'kwin-bus');
  kw = sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket wayland-notify --no-lockscreen --no-global-shortcuts --width 1280 --height 800`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(out, 'kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, 'wayland-notify')), 20000, 'kwin socket');
  const shot = async name => { const f = path.join(shots, name + '.png'); const r = spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); return r.status === 0 ? f : null; };

  const chimeFile = path.join(out, 'chime.jsonl'), attFile = path.join(out, 'attention.jsonl');
  const winPort = P(27310);
  const natEnv = { ...cleanEnv(), WAYLAND_DISPLAY: 'wayland-notify', DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim(), COMMONS_DATA: path.join(out, 'data/nat'), COMMONS_RELAY: `http://127.0.0.1:${RELAY_PORT}`,
    COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_AUDIO_SINK: 'null', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_UI_TEST: String(winPort), MEGA_GPUI_CHIME_FILE: chimeFile, MEGA_GPUI_ATTENTION_FILE: attFile,
    MEGA_GPUI_NOTIFY_BUS: fs.readFileSync(mockBusFile, 'utf8').trim(), MEGA_GPUI_UPDATES: 'off', RUST_LOG: 'info' };
  nat = sh(BIN, [], natEnv, path.join(out, 'nat.log'));
  const W = new Win(winPort);
  await until(async () => { try { await W.connect(); return true; } catch { return false; } }, 30000, 'window test port');
  await until(async () => (await W.cmd('core', { method: 'snapshot' })).ok, 30000, 'window core unlocked');
  await until(async () => { const d = await W.dump(); return d.level; }, 10000, 'window up');
  await until(async () => { const s = (await W.cmd('core', { method: 'snapshot' })).ok; const c = s && s.communities.find(c => c.id === cid); return c && c.channels.length >= 6; }, 30000, 'window has the community');
  await sleep(2500);
  check('0 the native window is up (private KWin, mock notifications, chime and attention recorded)', true, { nat: nat.pid });
  const gidOf = n => rooms[n];
  const notifs = () => lines(mockLog), chimes = () => lines(chimeFile), atts = () => lines(attFile);
  const say = (from, room, text) => rpc(from.port, 'send_text', { gid: rooms[room], text });
  const setRule = async (room, kind, secs = 0) => { await W.cmd('room_menu', { name: room }); return W.cmd('room_pick', { kind, secs }); };
  const rules = async () => (await W.cmd('core', { method: 'notify_get' })).ok.rooms;
  const idle = async () => { await sleep(2600); };   // past the chime's two-second throttle

  // ---------------------------------------------------------------- A. a message while the window is not looked at
  await W.cmd('attend', { on: false });
  await W.cmd('home'); await sleep(800);
  let n0 = notifs().length, c0 = chimes().length;
  await say(B, 'lounge', 'good evening everyone');
  const gotA = await soft(until(() => notifs().length > n0, 25000, 'notification raised', 300));
  const nA = notifs()[n0];
  check('A1 a message while the window is not looked at raises a notification (captured on the mock service, nothing shown)', !gotA.timeout && /Bob/.test(nA.summary + nA.body) && nA.category === 'im.received', nA);
  check('A2 it offers Open and a quick reply', nA && nA.actions.includes('default') && nA.actions.includes('inline-reply'), nA && nA.actions);
  const chimeA = await soft(until(() => chimes().length > c0, 8000, 'chime due', 200));
  check('A3 the chime is due for it (recorded, never played: the sink is null)', !chimeA.timeout && chimes()[c0].tone === 'chime', chimes().slice(c0));
  const attA = atts().length;
  check('A4 a plain message does not ask the taskbar for attention (only a mention does)', attA === 0, atts());
  // click through
  const idA = nA.id;
  mockSay(`action ${idA} default`);
  const openedA = await soft(until(async () => { const d = await W.dump(); return d.level === 'Chat' && d.gid === gidOf('lounge') ? d : null; }, 10000, 'click opens the room', 200));
  check('A5 clicking the notification opens that room in the window', !openedA.timeout, openedA.timeout || openedA.gid);

  // ---------------------------------------------------------------- B. a mention while away: chime and taskbar
  await W.cmd('home'); await sleep(800); await idle();
  n0 = notifs().length; c0 = chimes().length;
  await say(B, 'other', 'hey @Nat are you there');
  await soft(until(() => atts().length > 0, 25000, 'attention asked', 300));
  check('B1 a mention while the window is not looked at asks the taskbar for attention', atts().length >= 1 && atts()[0].attention === true, atts());
  check('B2 ...raises a notification that says so', notifs().length > n0 && /mentioned you/.test(notifs()[n0].body), notifs().slice(n0).map(x => x.body));
  check('B3 ...and chimes', chimes().length > c0, chimes().slice(c0));
  const f1 = await shot('mention-away');
  check('B4 screenshot', !!f1, f1 || 'none');

  // ---------------------------------------------------------------- C. per-room mute from the room menu
  await W.cmd('home'); await sleep(500);
  const mRes = await setRule('hush', 'mute', 0);
  const rC = await soft(until(async () => { const r = await rules(); return r[gidOf('hush')] ? r : null; }, 8000, 'rule saved', 200));
  check('C1 the room menu mutes a room (the core holds the rule: the same rule the web writes)', !rC.timeout && rC[gidOf('hush')].kind === 'muted' && mRes.ok, rC);
  const f2 = await shot('rooms-muted');
  await idle();
  n0 = notifs().length; c0 = chimes().length; const a0 = atts().length;
  await say(B, 'hush', 'psst'); await say(B, 'hush', 'psst @Nat even with a mention');
  await sleep(9000);
  check('C2 a muted room raises no notification, even for a mention', notifs().length === n0, notifs().slice(n0).map(x => x.body));
  check('C3 ...no chime and no ask for attention', chimes().length === c0 && atts().length === a0, { chimes: chimes().slice(c0), atts: atts().slice(a0) });
  await setRule('hush', 'none');
  check('C4 unmuting takes the rule away', !(await rules())[gidOf('hush')]);

  // ---------------------------------------------------------------- D. mentions only
  await setRule('only', 'mentions');
  check('D1 the room menu sets mentions only', (await rules())[gidOf('only')]?.kind === 'mentions');
  await idle();
  n0 = notifs().length; c0 = chimes().length;
  await say(B, 'only', 'just chatting');
  await sleep(9000);
  check('D2 an ordinary message in a mentions-only room is silent', notifs().length === n0 && chimes().length === c0, { n: notifs().length - n0, c: chimes().length - c0 });
  await say(B, 'only', 'hello @Nat this is for you');
  const gotD = await soft(until(() => notifs().length > n0, 25000, 'mention in mentions-only room', 300));
  check('D3 a mention in it still notifies and chimes', !gotD.timeout && (await soft(until(() => chimes().length > c0, 8000, 'chime', 200))) === true, { n: notifs().slice(n0).map(x => x.body), c: chimes().length - c0 });
  const cards = await shot('rooms-rules');

  // ---------------------------------------------------------------- E. the sound setting
  await setRule('only', 'none');
  await W.cmd('pref', { sound: 'mentions' }); await idle();
  c0 = chimes().length;
  await say(B, 'plain', 'a plain message');
  await sleep(9000);
  check('E1 sound "mentions and replies" is silent for a plain message', chimes().length === c0, chimes().slice(c0));
  await W.cmd('pref', { sound: 'off' }); await idle();
  c0 = chimes().length; const na = atts().length;
  await say(B, 'plain', 'and now @Nat with sound off');
  await soft(until(() => atts().length > na, 25000, 'attention with sound off', 300));
  check('E2 sound "off" is silent even for a mention, but the taskbar is still asked', chimes().length === c0 && atts().length > na, { chimes: chimes().length - c0, atts: atts().length - na });
  await W.cmd('pref', { sound: 'all' });

  // ---------------------------------------------------------------- F. the window being looked at
  await W.cmd('attend', { on: true });
  await W.cmd('open', { name: 'lounge' }); await sleep(1500); await idle();
  c0 = chimes().length; const a1 = atts().length; n0 = notifs().length;
  await say(B, 'lounge', 'a message in the room you are reading');
  await sleep(6000);
  check('F1 a message in the room being read makes no chime, no notification and no ask for attention', chimes().length === c0 && notifs().length === n0 && atts().length === a1, { c: chimes().length - c0, n: notifs().length - n0 });
  await say(B, 'other', 'a message in another room');
  const gotF = await soft(until(() => chimes().length > c0, 20000, 'chime for another room', 300));
  check('F2 a message in another room does chime (the window is looked at: the chime is the only alert)', !gotF.timeout, chimes().slice(c0));
  check('F3 ...with no system notification and no ask for attention', notifs().length === n0 && atts().length === a1, { n: notifs().length - n0, a: atts().length - a1 });
  await idle();
  await say(B, 'plain', 'and a mention @Nat while looking');
  await sleep(9000);
  check('F4 a mention while the window is looked at chimes but does not ask the taskbar', atts().length === a1, atts().slice(a1));

  // ---------------------------------------------------------------- G. one window per account
  await W.cmd('attend', { on: false }); await W.cmd('home'); await sleep(800);
  const t0 = Date.now();
  const second = spawnSync(BIN, ['--open', gidOf('other')], { env: natEnv, timeout: 15000 });
  const secs = (Date.now() - t0) / 1000;
  check('G1 a second launch ends by itself, quickly, without error', second.status === 0 && secs < 6, { status: second.status, secs });
  const openedG = await soft(until(async () => { const d = await W.dump(); return d.level === 'Chat' && d.gid === gidOf('other') ? d : null; }, 10000, 'second launch opens the room', 200));
  check('G2 ...and the window that was open opened the room it was asked for', !openedG.timeout, openedG.timeout || openedG.gid);
  const second2 = spawnSync(BIN, [], { env: natEnv, timeout: 15000 });
  check('G3 a plain second launch also just ends', second2.status === 0);
  const dG = await W.dump();
  check('G4 the first window is still the only one, still working', (await W.cmd('callsnap')).windows === 1 && dG.level, { windows: (await W.cmd('callsnap')).windows });
  // another account folder is another window
  const lockFile = path.join(out, 'data/nat/instance.lock'), sockFile = path.join(out, 'data/nat/mega.sock');
  check('G5 the lock file and the socket are in the account folder', fs.existsSync(lockFile) && fs.existsSync(sockFile));

  // ---------------------------------------------------------------- H. settings
  await W.cmd('pref', { read_size: 'large' });
  await W.cmd('settings'); await sleep(1200);
  const f3 = await shot('settings-large');
  const dH = await W.alerts();
  check('H1 reading size and sound are chosen in Settings (screenshot) and kept', dH.read_size === 'Large' && dH.sound === 'All' && !!f3, dH);
  const dir = path.join(out, 'downloads'); fs.mkdirSync(dir, { recursive: true });
  const setDl = await W.cmd('core', { method: 'set_download_dir', args: { dir } });
  const snapH = (await W.cmd('core', { method: 'snapshot' })).ok;
  check('H2 the downloads folder is the core\'s own setting, shared with the web', setDl.ok && snapH.download_dir === dir, snapH.download_dir);
  await W.cmd('pref', { read_size: 'normal' });
  const f4 = await shot('settings-normal');
  check('H3 screenshots of the settings page at both sizes', !!f4);
} catch (e) { res.error = String(e.stack || e); console.log('ERROR', res.error); failed++; }
res.failed = failed;
fs.writeFileSync(path.join(out, 'result.json'), JSON.stringify(res, null, 1));
for (const p of [nat, mock, kw]) if (p) stopPid(p);
console.log(failed ? `${failed} FAILED` : 'ALL PASSED', '->', path.join(out, 'result.json'));
await sleep(500); stopAll(); process.exit(failed ? 1 : 0);
