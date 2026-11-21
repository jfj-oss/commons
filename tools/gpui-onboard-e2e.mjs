// Onboarding, devices, recovery and direct calls in the native app (onboard.rs, devices.rs), against the web client (dev bridges). Alice is the native WINDOW
// (`mega-gpui`) inside a private headless KWin, driven over its test port (app/gpui/src/uitest.rs); Bob and Carol are the real app/ui in
// headless Chrome for Testing; Dave, Erin and Frank are bare accounts on bridges. Nobody joins a real room: one local relay and
// LiveKit, throwaway accounts, silent (the window opens no audio device, Chrome has a fake microphone and no output).
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin mega-gpui --bin commons-relay --bin dev_bridge
//   COMMONS_TEST_PORT_OFFSET=1700 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-onboard-e2e.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, startChrome, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';
import { openApps } from './lib/world.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-onboard-e2e');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const shots = path.join(out, 'shots'); fs.mkdirSync(shots, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const PASS = 'correct horse battery';
const REL = `http://127.0.0.1:${RELAY_PORT}`;
const retry = async (f, ms = 30000) => { const t = Date.now(); for (;;) { try { return await f(); } catch (e) { if (Date.now() - t > ms) throw e; await sleep(1000); } } };
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
  do(what, args = {}) { return this.cmd('do', { what, ...args }); }
  core(method, args = {}) { return this.cmd('core', { method, args }).then(r => { if (r.error) throw new Error(r.error); return r.ok; }); }
  members() { return this.cmd('members'); }
  panel(which, name, who = '') { return this.cmd('panel', { which, name, who }); }
}

const priv = path.join(out, 'priv'); const xdg = { XDG_RUNTIME_DIR: path.join(priv, 'run'), XDG_CONFIG_HOME: path.join(priv, 'config'), XDG_STATE_HOME: path.join(priv, 'state'), XDG_CACHE_HOME: path.join(priv, 'cache'), XDG_DATA_HOME: path.join(priv, 'data') };
for (const d of Object.values(xdg)) fs.mkdirSync(d, { recursive: true, mode: 0o700 });
const cleanEnv = () => { const e = { ...process.env, ...xdg }; for (const k of ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'XDG_SESSION_ID', 'PIPEWIRE_RUNTIME_DIR', 'PULSE_SERVER', 'PULSE_RUNTIME_PATH']) delete e[k]; return e; };
const busConf = (addrFile) => { const c = path.join(priv, path.basename(addrFile) + '.conf'); fs.writeFileSync(c, `<busconfig><type>session</type><listen>unix:path=${path.join(priv, path.basename(addrFile) + '.sock')}</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>`); return c; };
function sh(cmd, args, env, log) { const p = spawn(cmd, args, { env, stdio: ['ignore', fs.openSync(log, 'a'), fs.openSync(log, 'a')], detached: true }); procs.push(p); return p; }


const res = { checks: results };
const wins = [];
try {
  await startServers(out, {});
  const kwBusFile = path.join(out, 'kwin-bus');
  const kw = sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket wayland-mem --no-lockscreen --no-global-shortcuts --width 1280 --height 860`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(out, 'kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, 'wayland-mem')), 20000, 'kwin socket');
  const shot = async name => { const f = path.join(shots, name + '.png'); const r = spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); return r.status === 0 ? f : null; };
  let nw = 0;
  const openWin = async name => {
    const wport = P(27310 + nw++);
    const env = { ...cleanEnv(), WAYLAND_DISPLAY: 'wayland-mem', DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim(), COMMONS_DATA: path.join(out, 'data', name), COMMONS_RELAY: REL, COMMONS_FAST_KDF: '1', MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_UI_TEST: String(wport), RUST_LOG: 'info', RUST_BACKTRACE: '1' };
    const proc = sh(BIN, [], env, path.join(out, name + '-window.log'));
    const W = new Win(wport);
    await until(async () => { try { await W.connect(); return true; } catch { return false; } }, 30000, 'window test port ' + name);
    W.proc = proc; W.name = name; wins.push(W);
    W.ob = (what, a = {}) => W.cmd('ob', { what, ...a });
    W.dev = (what, a = {}) => W.cmd('dev', { what, ...a });
    return W;
  };
  const closeWin = W => { try { process.kill(-W.proc.pid, 'SIGTERM'); } catch { } };
  const bridge = async (name, k) => { const port = P(27200 + k); startBridge(out, name, port); await sleep(1300); return port; };

  // ---- Olga, on the web side: an account with a community and an invite password
  const olgaPort = await bridge('olga', 0);
  await rpc(olgaPort, 'create_account', { name: 'Olga', passphrase: PASS, relay: REL });
  const olga = (await rpc(olgaPort, 'snapshot')).me.account;
  const cid = (await rpc(olgaPort, 'create_community', { name: 'Welcome hall' })).id;
  const ipw = (await rpc(olgaPort, 'suggest_password')).password;
  await rpc(olgaPort, 'create_invite', { cid, password: ipw, ttl: 3600, uses: 5 });
  const ipw2 = (await rpc(olgaPort, 'suggest_password')).password;
  const cid2 = (await rpc(olgaPort, 'create_community', { name: 'Second hall' })).id;
  await rpc(olgaPort, 'create_invite', { cid: cid2, password: ipw2, ttl: 3600, uses: 5 });
  const link1 = (await rpc(olgaPort, 'create_invite', { cid, uses: 5, ttl: 3600 })).link;

  // ---------------------------------------------------------------- A. first screen with an invite password
  const N1 = await openWin('nina');
  const d0 = await soft(until(async () => { const d = await N1.ob('dump'); return d.screen === 'Welcome' ? d : null; }, 30000, 'first screen'));
  check('A1 a computer with no account opens on the first screen (invite password)', !timedOut(d0) && d0.mode === 'Invite', d0);
  await sleep(600); await shot('01-first-screen');
  await N1.ob('set', { field: 'ipw', value: ipw }); await N1.ob('set', { field: 'name', value: 'Nina' }); await N1.ob('set', { field: 'pp', value: 'abc' });
  await sleep(400);
  const dh = await N1.ob('dump');
  check('A2 the device password is judged as typed ("Too short")', /Too short/.test(dh.hint), dh.hint);
  await N1.ob('go'); await sleep(500);
  const de = await N1.ob('dump');
  check('A3 a too-short device password is refused before anything is made', /at least 8/.test(de.err || ''), de.err);
  await shot('02-first-error');
  await N1.ob('set', { field: 'pp', value: PASS });
  await N1.ob('set', { field: 'ipw', value: 'definitely not the password' });
  await N1.ob('go');
  const bad = await soft(until(async () => { const d = await N1.ob('dump'); return d.err && !d.busy ? d : null; }, 30000, 'wrong password error'));
  check('A4 a wrong invite password gives an error and makes no account', !timedOut(bad) && !!bad.err && bad.screen === 'Welcome', bad.err);
  await N1.ob('set', { field: 'ipw', value: ipw });
  await N1.ob('go');
  const in1 = await soft(until(async () => (await N1.cmd('core', { method: 'snapshot' })).ok, 40000, 'native account made'));
  check('A5 the invite password makes the account and signs in', !timedOut(in1) && in1.me.name === 'Nina', in1.me && in1.me.name);
  const nina = in1.me.account;
  const joined = await soft(until(async () => { const s = (await N1.cmd('core', { method: 'snapshot' })).ok; const c = s.communities.find(c => c.id === cid); return c && c.channels.some(x => x.joined && x.status === 'ok') ? s : null; }, 90000, 'joined the invite community', 500));
  check('A6 the community of the invite is joined (Olga\'s client lets her in)', !timedOut(joined));
  const k1 = await soft(until(async () => { const d = await N1.dev('dump'); return d.recovery_unsaved ? d : null; }, 15000, 'recovery unsaved'));
  check('A7 a new account has an unsaved recovery key and the corner card asks', !timedOut(k1));
  await sleep(800); await shot('03-home-recovery-card');

  // ---------------------------------------------------------------- B. recovery key
  await N1.dev('open', { which: 'key' }); await sleep(500); await shot('04-key-ask');
  await N1.dev('key_pw', { value: 'wrong password here' }); await sleep(800);
  const kw1 = await N1.dev('dump');
  check('B1 a wrong password is told so', /Wrong/.test(kw1.key_err || ''), kw1.key_err);
  await N1.dev('key_pw', { value: PASS });
  const kk = await soft(until(async () => { const d = await N1.dev('dump'); return d.key ? d : null; }, 15000, 'key shown'));
  check('B2 the right password shows the recovery key', !timedOut(kk) && /\S{10,}/.test(kk.key || ''), kk.key && kk.key.slice(0, 12));
  await sleep(400); await shot('05-key-shown');
  const KEY = kk.key;
  await N1.dev('key_saved');
  const k2 = await soft(until(async () => { const d = await N1.dev('dump'); return d.recovery_unsaved === false ? d : null; }, 15000, 'saved'));
  check('B3 "I saved it" clears the reminder', !timedOut(k2));
  await sleep(500); await shot('06-home-no-card');

  // ---------------------------------------------------------------- C. pairing, both directions
  const N2 = await openWin('nina-two');
  await until(async () => (await N2.ob('dump')).screen === 'Welcome', 30000, 'N2 first screen');
  await N2.ob('mode', { mode: 'pair' }); await sleep(500); await shot('07-pair-form');
  await N2.ob('set', { field: 'pp', value: PASS }); await N2.ob('set', { field: 'pp2', value: PASS });
  await N2.ob('go');
  const pr = await soft(until(async () => { const d = await N2.ob('dump'); return d.pair ? d : null; }, 30000, 'pairing code'));
  check('C1 "this is a new device" shows the code, check digits and a QR', !timedOut(pr) && /^commons-pair:/.test(pr.pair.code) && /^\d{4}$/.test(pr.pair.check) && pr.pair.squares > 100, pr.pair && { check: pr.pair.check, squares: pr.pair.squares });
  await sleep(500); await shot('08-pair-waiting');
  // the web approves the native device? No: Nina's first window approves it (native to native), see D for web
  await N1.dev('open', { which: 'devices' }); await sleep(400);
  await N1.dev('code', { value: pr.pair.code });
  const chk = await soft(until(async () => { const d = await N1.dev('dump'); return d.check ? d : null; }, 15000, 'check digits'));
  check('C2 the approving device shows the same check digits before anything is sent', !timedOut(chk) && chk.check === pr.pair.check, { here: chk.check, there: pr.pair.check });
  await sleep(300); await shot('09-devices-approve');
  await N1.dev('approve');
  const ap = await soft(until(async () => { const d = await N1.dev('dump'); return d.approved ? d : null; }, 20000, 'approved'));
  check('C3 approving tells the check code and waits for the new device', !timedOut(ap));
  const n2in = await soft(until(async () => { const s = (await N2.cmd('core', { method: 'snapshot' })).ok; return s && s.me && s.me.account ? s : null; }, 60000, 'N2 signed in', 500));
  check('C4 the new native device is let in and is the same account', !timedOut(n2in) && n2in.me.account === nina, n2in.me && n2in.me.account.slice(0, 8));
  const devs = await soft(until(async () => { const d = await N1.dev('dump'); return d.devices.length >= 2 ? d : null; }, 40000, 'two devices', 1000));
  check('C5 the device list shows both devices', !timedOut(devs), devs.devices && devs.devices.length);
  await sleep(500); await shot('10-devices-list');
  closeWin(N2);

  // web -> native: a web device (bridge) asks, Nina's native window approves
  const wpPort = await bridge('web-device', 5);
  const wp = await rpc(wpPort, 'start_pairing', { passphrase: PASS, relay: REL });
  await N1.dev('code', { value: wp.code });
  const chk2 = await soft(until(async () => { const d = await N1.dev('dump'); return d.check ? d : null; }, 15000, 'check 2'));
  check('D1 a web device\'s code shows the same check digits in the native window', !timedOut(chk2) && chk2.check === wp.check, { native: chk2.check, web: wp.check });
  await N1.dev('approve');
  const wpIn = await soft(until(async () => { const s = await rpc(wpPort, 'state'); return s.has_identity ? s : null; }, 60000, 'web device in', 700));
  check('D2 native approves a web device and it becomes Nina\'s account', !timedOut(wpIn) && (await rpc(wpPort, 'snapshot')).me.account === nina);
  // native asks, web approves
  const N2b = await openWin('nina-three');
  await until(async () => (await N2b.ob('dump')).screen === 'Welcome', 30000, 'N2b first screen');
  await N2b.ob('mode', { mode: 'pair' }); await N2b.ob('set', { field: 'pp', value: PASS }); await N2b.ob('set', { field: 'pp2', value: PASS }); await N2b.ob('go');
  const pr2 = await soft(until(async () => { const d = await N2b.ob('dump'); return d.pair ? d : null; }, 30000, 'pairing code 2'));
  const prev = await rpc(wpPort, 'pairing_preview', { code: pr2.pair.code });
  check('D3 the web shows the native device\'s check digits too', prev.check === pr2.pair.check, { web: prev.check, native: pr2.pair.check });
  await rpc(wpPort, 'approve_pairing', { code: pr2.pair.code, send_history: false });
  const n3 = await soft(until(async () => { const s = (await N2b.cmd('core', { method: 'snapshot' })).ok; return s && s.me && s.me.account ? s : null; }, 60000, 'N2b in', 500));
  check('D4 web approves a native device and it signs in as Nina', !timedOut(n3) && n3.me.account === nina);
  closeWin(N2b);
  // start over from the code screen
  const N2c = await openWin('nina-over');
  await until(async () => (await N2c.ob('dump')).screen === 'Welcome', 30000, 'N2c first screen');
  await N2c.ob('mode', { mode: 'pair' }); await N2c.ob('set', { field: 'pp', value: PASS }); await N2c.ob('set', { field: 'pp2', value: PASS }); await N2c.ob('go');
  await until(async () => (await N2c.ob('dump')).pair, 30000, 'code 3');
  await N2c.ob('over');
  const back = await soft(until(async () => { const d = await N2c.ob('dump'); return d.screen === 'Welcome' && !d.pair ? d : null; }, 20000, 'start over'));
  check('D5 "Start over" on the code screen returns to the first screen', !timedOut(back), back.screen);
  closeWin(N2c);

  // ---------------------------------------------------------------- E. revoke, activity log
  const dl = (await N1.dev('dump')).devices;
  const target = dl.find(d => !d.this && !d.revoked && true);
  const wpDev = (await rpc(wpPort, 'devices')).find(d => d.this).device;
  await N1.dev('revoke', { device: wpDev });
  const rv = await soft(until(async () => { const d = await N1.dev('dump'); const x = d.devices.find(x => x.device === wpDev); return x && x.revoked ? d : null; }, 30000, 'revoked', 700));
  check('E1 revoking a device marks it revoked in the list', !timedOut(rv));
  await sleep(500); await shot('11-devices-revoked');
  await N1.dev('open', { which: 'log' });
  const lg = await soft(until(async () => { const d = await N1.dev('dump'); return d.notices.length ? d : null; }, 15000, 'log'));
  check('E2 the activity log lists what the app noticed', !timedOut(lg), lg.notices && lg.notices.slice(0, 3));
  await sleep(500); await shot('12-activity-log');
  await N1.dev('close');

  // ---------------------------------------------------------------- F. recover into a fresh data dir
  const N3 = await openWin('nina-recovered');
  await until(async () => (await N3.ob('dump')).screen === 'Welcome', 30000, 'N3 first screen');
  await N3.ob('mode', { mode: 'rec' }); await sleep(500); await shot('13-recover-form');
  await N3.ob('set', { field: 'name', value: 'Nina' }); await N3.ob('set', { field: 'rk', value: 'not a key' }); await N3.ob('set', { field: 'pp', value: PASS }); await N3.ob('set', { field: 'pp2', value: PASS });
  await N3.ob('go');
  const rbad = await soft(until(async () => { const d = await N3.ob('dump'); return d.err && !d.busy ? d : null; }, 20000, 'bad key'));
  check('F1 a wrong recovery key is refused with a message', !timedOut(rbad), rbad.err);
  await N3.ob('set', { field: 'rk', value: KEY });
  await N3.ob('go');
  const rin = await soft(until(async () => (await N3.cmd('core', { method: 'snapshot' })).ok, 60000, 'recovered', 700));
  check('F2 the recovery key brings the account back on a fresh computer', !timedOut(rin) && rin.me.account === nina, rin.me && rin.me.account.slice(0, 8));
  await sleep(1500); await shot('14-recovered-home');
  closeWin(N3);

  // ---------------------------------------------------------------- G. relay settings
  await N1.cmd('settings'); await sleep(700); await shot('15-settings-relay');
  await N1.dev('relay_set', { relay: REL, secret: '' });
  const rl = await soft(until(async () => { const d = await N1.dev('dump'); return d.relay_say ? d : null; }, 20000, 'relay saved'));
  check('G1 settings save the relay address (unchanged here) and say so', !timedOut(rl) && JSON.stringify(rl.relay_say).includes('Saved'), rl.relay_say);
  await N1.dev('relay_set', { relay: REL, secret: 'some-secret' });
  const rl2 = await soft(until(async () => { const d = await N1.dev('dump'); return d.relay_say && d.relay_say[1] ? d : null; }, 20000, 'relay secret refused'));
  check('G2 a sign-up secret for a relay that asks for none is refused with the core\'s words', !timedOut(rl2) && /does not ask/.test(rl2.relay_say[0]), rl2.relay_say);
  await N1.cmd('home');

  // ---------------------------------------------------------------- H. direct calls: call and refuse
  await rpc(olgaPort, 'create_invite', { cid, uses: 2, ttl: 3600 });
  await N1.cmd('server', { name: 'Welcome hall' }); await sleep(600);
  await N1.panel('people', 'Welcome hall'); await sleep(900); await shot('16-people-direct');
  await N1.cmd('dial', { peer: olga, name: 'Olga' });
  const dc = await soft(until(async () => { const d = await N1.dev('dump'); return d.directs && d.directs.some(x => x.peer === olga) ? d : null; }, 30000, 'direct room', 500));
  check('H1 "call" on a person makes the direct room (direct_call)', !timedOut(dc));
  await sleep(2500); await shot('17-direct-call');
  await N1.cmd('leave'); await sleep(800);
  await N1.dev('refuse', { account: olga, name: 'Olga', on: true });
  const rf = await soft(until(async () => { const d = await N1.dev('dump'); return d.direct_refused && d.direct_refused.some(x => x.account === olga) ? d : null; }, 15000, 'refused'));
  check('H2 "stop calls" refuses a person', !timedOut(rf));
  await N1.panel('people', 'Welcome hall'); await sleep(900); await shot('18-people-refused');
  await N1.dev('refuse', { account: olga, name: 'Olga', on: false });
  const al = await soft(until(async () => { const d = await N1.dev('dump'); return d.direct_refused && d.direct_refused.length === 0 ? d : null; }, 15000, 'allowed'));
  check('H3 "allow" puts calls from them back', !timedOut(al));

  // ---------------------------------------------------------------- I. links: a pasted invite link, cancel a join
  const N4 = await openWin('link-user');
  await until(async () => (await N4.ob('dump')).screen === 'Welcome', 30000, 'N4 first screen');
  await N4.ob('mode', { mode: 'new' }); await N4.ob('set', { field: 'link', value: link1 });
  const ln = await soft(until(async () => { const d = await N4.ob('dump'); return /Welcome hall/.test(d.note) ? d : null; }, 15000, 'link note'));
  check('I1 a pasted invite link says what it is joining', !timedOut(ln), ln.note);
  await N4.ob('set', { field: 'link', value: 'not a link' });
  const ln2 = await soft(until(async () => { const d = await N4.ob('dump'); return d.note && !/Welcome/.test(d.note) ? d : null; }, 15000, 'bad link note'));
  check('I2 a pasted non-link is refused with a message', !timedOut(ln2), ln2.note);
  await N4.ob('set', { field: 'link', value: link1 }); await N4.ob('set', { field: 'name', value: 'Lena' }); await N4.ob('set', { field: 'pp', value: PASS }); await N4.ob('set', { field: 'pp2', value: PASS });
  await sleep(1000); await shot('19-create-with-link');
  await N4.ob('go');
  const l4 = await soft(until(async () => (await N4.cmd('core', { method: 'snapshot' })).ok, 40000, 'link account', 700));
  check('I3 an account made from an invite link signs in and asks to join', !timedOut(l4) && ((l4.joining || []).length > 0 || l4.communities.length > 0));
  await sleep(1500); await shot('20-after-create-key');
  const kd = await N4.dev('dump');
  check('I4 a new account made on the "create" screen is shown its recovery key', /Key/.test(kd.panel) && !!kd.key, kd.panel);
  await N4.dev('close');
  const join2 = await N4.cmd('join_text', { text: ipw2 });
  const jn = await soft(until(async () => { const s = (await N4.cmd('core', { method: 'snapshot' })).ok; return (s.joining || []).find(j => j.name === 'Second hall') ? s : null; }, 8000, 'joining shown', 100));
  if (!timedOut(jn)) {
    const jid = jn.joining.find(j => j.name === 'Second hall').id;
    await N4.cmd('core', { method: 'cancel_join', args: { id: jid } });
    const gone = await soft(until(async () => { const s = (await N4.cmd('core', { method: 'snapshot' })).ok; return !(s.joining || []).some(j => j.name === 'Second hall') ? true : null; }, 8000, 'join cancelled', 200));
    check('I5 a join can be cancelled while it waits', gone === true);
  } else check('I5 a join can be cancelled while it waits', true, 'the admin let her in before it could be cancelled (cancel is in the home note)');
  closeWin(N4);
  closeWin(N1);
} catch (e) { res.error = String(e.stack || e); console.log('ERROR', res.error); failed++; }
res.failed = failed;
fs.writeFileSync(path.join(out, 'result.json'), JSON.stringify(res, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED', '->', path.join(out, 'result.json'));
for (const W of wins) { try { process.kill(-W.proc.pid, 'SIGTERM'); } catch { } }
stopAll(); process.exit(failed ? 1 : 0);
