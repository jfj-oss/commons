// The phone layout (app/gpui/src/phone.rs) on a desktop: the native window with MEGA_PHONE_PREVIEW=1 (390 x 844) in a private headless KWin,
// screenshots of the cards, a community's rooms, a room, the room menu and settings. One local relay, throwaway accounts, silent.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin mega-gpui --bin commons-relay --bin dev_bridge
//   COMMONS_TEST_PORT_OFFSET=2700 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-phone-shots.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/mega-ios/shots');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const shots = path.join(out, 'shots'); fs.mkdirSync(shots, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const PASS = 'correct horse battery';
const REL = `http://127.0.0.1:${RELAY_PORT}`;
const J = x => JSON.stringify(x);
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? '  [' + (typeof detail === 'string' ? detail : J(detail)).slice(0, 300) + ']' : ''}`); };
async function until(fn, ms, what, step = 250) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(step); } }

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
  const port = P(27200); const b = startBridge(out, 'alice', port); await sleep(1300);
  await rpc(port, 'create_account', { name: 'Alice', passphrase: PASS, relay: REL });
  const cid = (await rpc(port, 'create_community', { name: 'Ascii club' })).id;
  const cid2 = (await rpc(port, 'create_community', { name: 'Book group' })).id;
  for (const n of ['lounge', 'news']) await rpc(port, 'create_channel', { cid, name: n });
  await rpc(port, 'create_channel', { cid: cid2, name: 'reading' });
  const ready = async () => until(async () => { const s = await rpc(port, 'snapshot'); const c = s.communities.find(c => c.id === cid); return c && c.channels.length >= 3 && c.channels.every(x => x.joined && x.status === 'ok') ? c : null; }, 90000, 'ready', 500);
  const ca = await ready(); const gid = n => ca.channels.find(x => x.name === n).gid;
  const talk = ['Welcome to the club. This room has the default look.', 'Thanks, glad to be here.', 'The backdrop is the room\'s own ASCII, dim, and it flows around every word.', 'Short one.', 'A longer line to see how the text wraps in a narrow column, which is what a phone has, with a few more words to fill the line.'];
  for (const t of talk) { await rpc(port, 'send_text', { gid: gid('lounge'), text: t }); await sleep(150); }
  await sleep(2000);
  try { process.kill(-b.pid, 'SIGTERM'); } catch { } await sleep(1800);

  const kwBusFile = path.join(out, 'kwin-bus');
  sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket wayland-phone --no-lockscreen --no-global-shortcuts --width 700 --height 1000`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(out, 'kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, 'wayland-phone')), 20000, 'kwin socket');
  const nshot = async name => { const f = path.join(shots, name + '.png'); spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); spawnSync('python3', ['-c', 'import sys;from PIL import Image;i=Image.open(sys.argv[1]);i.crop((150,70,550,930)).save(sys.argv[1])', f]); return f; };
  const wport = P(27310);
  const winProc = sh(BIN, [], { ...cleanEnv(), WAYLAND_DISPLAY: 'wayland-phone', DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim(), COMMONS_DATA: path.join(out, 'data', 'alice'), COMMONS_RELAY: REL, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_UI_TEST: String(wport), MEGA_PHONE_PREVIEW: '1', RUST_BACKTRACE: '1' }, path.join(out, 'window.log'));
  const W = new Win(wport);
  await until(async () => { try { await W.connect(); return true; } catch { return false; } }, 30000, 'window test port');
  await until(async () => (await W.cmd('core', { method: 'snapshot' })).ok, 30000, 'window core unlocked');
  await sleep(3000);
  await nshot('1-home'); await W.cmd('go', { name: 'Ascii club' }); await sleep(2500); await nshot('2-rooms');
  await W.cmd('open', { name: 'lounge' }); await sleep(3500); await nshot('3-room');
  await W.cmd('scroll', { dy: 200 }); await sleep(800); await nshot('4-room-scrolled');
  await W.cmd('home'); await sleep(1500); await W.cmd('go', { name: 'Ascii club' }); await sleep(2000);
  await W.cmd('room_menu', { name: 'lounge' }); await sleep(1000); await nshot('5-room-menu');
  await W.cmd('settings'); await sleep(1500); await nshot('6-settings');
  check('shots taken', fs.readdirSync(shots).length >= 6, fs.readdirSync(shots));
  try { process.kill(-winProc.pid, 'SIGTERM'); } catch { }
} catch (e) { check('the run itself', false, String(e && e.stack || e).slice(0, 400)); }
fs.writeFileSync(path.join(out, 'results.json'), J(results, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED');
stopAll(); process.exit(failed ? 1 : 0);
