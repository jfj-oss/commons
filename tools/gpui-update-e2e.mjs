// The native app's self-update, end to end, on one Linux machine and nothing else: release side and app side.
//
//   1. a throwaway minisign key pair (and a second, WRONG one). Never the real key: nothing here reads it.
//   2. two REAL release builds of mega-gpui (scripts/build-linux-native.sh): 0.9.0 and 0.9.1, both made to trust the throwaway key
//      (MEGA_GPUI_UPDATE_PUBKEY at build time).
//   3. scripts/publish-update.sh publishes 0.9.1 into a scratch dist/ (test signer, prebuilt files, scratch mode): this is the
//      RELEASE side, producing the linux-gpui-x86_64 entry, the versioned file, and latest.json.
//   4. a local commons-relay serves that dist/ (the feed the apps read, /update/latest.json).
//   5. the 0.9.0 tarball is installed into a throwaway HOME with its own install.sh, and its mega-gpui is started in a
//      private headless KWin on a throwaway account. It finds 0.9.1, downloads, checks the signature, replaces itself, and
//      "Restart now" starts 0.9.1 (same program path, new process), which shows the release notes once.
//   6. a 0.9.2 signed with the WRONG key is refused and the installed program is left byte for byte as it was.
//   7. the tarball's uninstall.sh removes everything it put there (and the leftovers of the updates).
//   8. no-Vulkan: with no Vulkan driver visible, the app prints and writes the clear message and exits (no window).
//
// Silent and private: one local relay on 127.0.0.1 (COMMONS_TEST_PORT_OFFSET), a private KWin and D-Bus, a throwaway HOME and
// XDG folders (nothing of the user's desktop, bus, audio or account is seen), no sound.
//   COMMONS_TEST_PORT_OFFSET=5700 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-update-e2e.mjs [workdir]
// Needs the toolchain on PATH (cargo), target/release/commons-relay (cargo build --release -p commons-relay), and a dev_bridge
// (MEGA_TEST_BRIDGE, only to make the throwaway account). MEGA_UPDATE_E2E_REUSE=1 keeps builds already in <workdir>/build.
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import crypto from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { root, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';

const W = path.resolve(process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/ds-mega/ship/update-e2e'));
const reuse = process.env.MEGA_UPDATE_E2E_REUSE === '1';
if (!reuse) fs.rmSync(W, { recursive: true, force: true });
for (const d of ['keys', 'wrong', 'build', 'dist/update', 'shots', 'logs', 'home', 'priv']) fs.mkdirSync(path.join(W, d), { recursive: true });
const TARGET = process.env.CARGO_TARGET_DIR || path.join(root, 'target');
const RELAY = process.env.MEGA_TEST_RELAY_BIN || path.join(TARGET, 'release/commons-relay');
const BRIDGE = process.env.MEGA_TEST_BRIDGE || path.join(process.env.HOME, 'data/agent-tmp/ds-mega/ui/bin/dev_bridge');
const PASS = 'correct horse battery';
const J = x => JSON.stringify(x);
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? '  [' + (typeof detail === 'string' ? detail : J(detail)).slice(0, 300) + ']' : ''}`); };
async function until(fn, ms, what, step = 250) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(step); } }
const soft = p => p.catch(e => ({ timeout: String(e.message || e) }));
const sha = f => crypto.createHash('sha256').update(fs.readFileSync(f)).digest('hex');
const sh = (cmd, args, env, log, cwd) => { const p = spawn(cmd, args, { env, cwd, stdio: ['ignore', fs.openSync(log, 'a'), fs.openSync(log, 'a')], detached: true }); procs.push(p); return p; };
const stopPid = p => { try { process.kill(-p.pid, 'SIGTERM'); } catch { } };
const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };
const run = (cmd, args, env = {}, opts = {}) => spawnSync(cmd, args, { cwd: root, env: { ...process.env, ...env }, encoding: 'utf8', ...opts });

class Win {
  constructor(port) { this.port = port; this.n = 0; this.waiting = new Map(); this.buf = ''; }
  async connect() {
    this.sock = net.connect(this.port, '127.0.0.1');
    await new Promise((res, rej) => { this.sock.once('connect', res); this.sock.once('error', rej); });
    this.sock.on('data', d => { this.buf += d; let i; while ((i = this.buf.indexOf('\n')) >= 0) { const l = this.buf.slice(0, i); this.buf = this.buf.slice(i + 1); const j = JSON.parse(l); const w = this.waiting.get(j.id); if (w) { this.waiting.delete(j.id); w(j); } } });
    this.sock.on('error', () => { });
  }
  cmd(cmd, args = {}, ms = 20000) {
    const id = ++this.n;
    return new Promise((res, rej) => { const t = setTimeout(() => { this.waiting.delete(id); rej(new Error('no answer to ' + cmd)); }, ms); this.waiting.set(id, j => { clearTimeout(t); res(j); }); this.sock.write(J({ id, cmd, ...args }) + '\n'); });
  }
  close() { try { this.sock.destroy(); } catch { } }
}

// the environment of everything that touches a display: private XDG folders, never the user's bus or runtime dir
const xdg = { XDG_RUNTIME_DIR: path.join(W, 'priv/run'), XDG_CONFIG_HOME: path.join(W, 'priv/config'), XDG_STATE_HOME: path.join(W, 'priv/state'), XDG_CACHE_HOME: path.join(W, 'priv/cache') };
for (const d of Object.values(xdg)) fs.mkdirSync(d, { recursive: true, mode: 0o700 });
const cleanEnv = () => { const e = { ...process.env, ...xdg, HOME: path.join(W, 'home') }; for (const k of ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'XDG_SESSION_ID', 'XDG_DATA_HOME', 'PIPEWIRE_RUNTIME_DIR', 'PULSE_SERVER', 'PULSE_RUNTIME_PATH', 'MEGA_GPUI_VERSION', 'MEGA_GPUI_UPDATE_PUBKEY', 'COMMONS_DATA']) delete e[k]; return e; };
const busConf = addrFile => { const c = path.join(W, 'priv', path.basename(addrFile) + '.conf'); fs.writeFileSync(c, `<busconfig><type>session</type><listen>unix:path=${path.join(W, 'priv', path.basename(addrFile) + '.sock')}</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>`); return c; };

const res = { checks: results };
let kw, win, bridge, relay;
try {
  // ---- 1. keys
  const gen = d => { const r = run('cargo', ['run', '-q', '-p', 'commons-core', '--example', 'minisign-test-sign', '--', 'gen', path.join(W, d)]); if (r.status !== 0) throw new Error('key gen: ' + r.stderr); return r.stdout.trim(); };
  const PUB = gen('keys'); gen('wrong');
  fs.writeFileSync(path.join(W, 'keys/pub.b64'), PUB + '\n');
  check('1 throwaway key pairs made (the real key is never read)', PUB.length > 40);

  // ---- 2. two real release builds, trusting the throwaway key
  const OLD = '0.9.0', NEW = '0.9.1';
  for (const v of [OLD, NEW]) {
    const out = path.join(W, 'build', v);
    if (reuse && fs.existsSync(path.join(out, 'mega-gpui-linux-x86_64'))) continue;
    const t = Date.now();
    const r = run('scripts/build-linux-native.sh', [], { MEGA_GPUI_VERSION: v, MEGA_GPUI_UPDATE_PUBKEY: PUB, COMMONS_OUT: out, CARGO_BUILD_JOBS: process.env.CARGO_BUILD_JOBS || '6' }, { maxBuffer: 1 << 28 });
    fs.writeFileSync(path.join(W, 'logs', `build-${v}.log`), (r.stdout || '') + (r.stderr || ''));
    check(`2 release build ${v} (scripts/build-linux-native.sh)`, r.status === 0 && fs.existsSync(path.join(out, 'mega-linux-x86_64.tar.gz')) && fs.existsSync(path.join(out, 'mega-gpui-linux-x86_64')), `${((Date.now() - t) / 1000) | 0}s`);
    if (r.status !== 0) throw new Error('build failed: logs/build-' + v + '.log');
  }
  const oldBin = path.join(W, 'build', OLD, 'mega-gpui-linux-x86_64'), newBin = path.join(W, 'build', NEW, 'mega-gpui-linux-x86_64');
  check('2b the two builds differ (the version is built in)', sha(oldBin) !== sha(newBin));

  // ---- 3. the release side: publish-update.sh with the throwaway signer, nothing built, scratch mode
  const dist = path.join(W, 'dist');
  const pub = (ver, signer, prebuilt, notes, extra = []) => run('scripts/publish-update.sh', [ver, '--only', 'linux-native', '--notes', notes, ...extra], { COMMONS_UPDATE_SCRATCH: '1', COMMONS_UPDATE_DIST: dist, COMMONS_UPDATE_TEST_SIGNER: path.join(W, signer), COMMONS_UPDATE_PUBKEY_FILE: path.join(W, 'keys/pub.b64'), COMMONS_UPDATE_PREBUILT: prebuilt });
  const p1 = pub(NEW, 'keys', path.join(W, 'build', NEW), 'Faster start. The test notes for 0.9.1.');
  fs.writeFileSync(path.join(W, 'logs/publish-0.9.1.log'), (p1.stdout || '') + (p1.stderr || ''));
  check('3 scripts/publish-update.sh signs and publishes the native Linux build', p1.status === 0, (p1.stderr || '').slice(-300));
  const feed = JSON.parse(fs.readFileSync(path.join(dist, 'update/latest.json'), 'utf8'));
  const ent = feed.platforms && feed.platforms['linux-gpui-x86_64'];
  check('3b the feed announces 0.9.1 with a linux-gpui-x86_64 entry (url + signature), notes kept', feed.version === NEW && ent && ent.url === `update/Megatron_${NEW}_gpui-x86_64` && ent.signature.length > 100 && /test notes/.test(feed.notes), { version: feed.version, ent: ent && ent.url, plats: Object.keys(feed.platforms || {}) });
  check('3c the update file in dist/update is the built executable', fs.existsSync(path.join(dist, 'update', `Megatron_${NEW}_gpui-x86_64`)) && sha(path.join(dist, 'update', `Megatron_${NEW}_gpui-x86_64`)) === sha(newBin));
  check('3d the friends\' download (the tarball) is in dist/', fs.existsSync(path.join(dist, 'mega-linux-x86_64.tar.gz')));

  // ---- 4. a local relay serving it
  relay = sh(RELAY, ['--bind', `127.0.0.1:${RELAY_PORT}`, '--data', path.join(W, 'relay-data'), '--site', dist], { ...process.env, COMMONS_FAST_KDF: '1' }, path.join(W, 'logs/relay.log'));
  const REL = `http://127.0.0.1:${RELAY_PORT}`;
  await until(async () => (await fetch(REL + '/update/latest.json')).ok, 15000, 'relay up');
  const served = await (await fetch(REL + '/update/latest.json')).json();
  check('4 the relay serves the feed at /update/latest.json', served.version === NEW && !!served.platforms['linux-gpui-x86_64']);

  // ---- 5. install 0.9.0 the way a friend does
  const pkg = path.join(W, 'pkg-old'); fs.mkdirSync(pkg, { recursive: true });
  check('5 the 0.9.0 tarball unpacks', run('tar', ['-xzf', path.join(W, 'build', OLD, 'mega-linux-x86_64.tar.gz'), '-C', pkg]).status === 0);
  const home = path.join(W, 'home');
  const inst = run(path.join(pkg, 'mega-linux-x86_64/install.sh'), [], cleanEnv());
  const BIN = path.join(home, '.local/bin/mega-gpui');
  check('5b install.sh puts mega in the throwaway HOME (bin, desktop entry, icons)', inst.status === 0 && fs.existsSync(BIN) && fs.existsSync(path.join(home, '.local/share/applications/app.commons.chat.gpui.desktop')) && fs.existsSync(path.join(home, '.local/share/icons/hicolor/256x256/apps/app.commons.chat.gpui.png')), inst.stderr);
  const oldHash = sha(BIN);
  check('5c the installed program is the 0.9.0 build', oldHash === sha(oldBin));

  // the account: made on the relay by a bridge, which stops so the window can open the same folder
  const data = path.join(W, 'data/nat');
  const bp = P(27200);
  bridge = sh(BRIDGE, [String(bp), path.join(root, 'app/ui')], { ...process.env, COMMONS_DATA: data, COMMONS_RELAY: REL, COMMONS_FAST_KDF: '1' }, path.join(W, 'logs/bridge.log'));
  await until(async () => (await fetch(`http://127.0.0.1:${bp}/rpc`, { method: 'POST', body: J({ method: 'snapshot', args: {} }) })).status > 0, 15000, 'bridge');
  await rpc(bp, 'create_account', { name: 'Nat', passphrase: PASS, relay: REL });
  stopPid(bridge); await sleep(1500);

  // ---- the private display
  const kwBusFile = path.join(W, 'kwin-bus');
  kw = sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket wayland-upd --no-lockscreen --no-global-shortcuts --width 1280 --height 800`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(W, 'logs/kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, 'wayland-upd')), 20000, 'kwin socket');
  const shot = async name => { const f = path.join(W, 'shots', name + '.png'); const r = spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); return r.status === 0 ? f : null; };
  const winPort = P(27330);
  const appEnv = () => ({ ...cleanEnv(), WAYLAND_DISPLAY: 'wayland-upd', DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim(), COMMONS_DATA: data, COMMONS_RELAY: REL, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS,
    MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_AUDIO_SINK: 'null', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_UI_TEST: String(winPort), MEGA_GPUI_UPDATE_FIRST_CHECK_SECS: '3', MEGA_GPUI_NO_DIALOG: '1', MEGA_GPUI_NOTIFY_BUS: 'unix:path=/nonexistent' });
  const open = async () => { const w = new Win(winPort); await until(async () => { try { await w.connect(); return true; } catch { return false; } }, 40000, 'window test port'); await until(async () => (await w.cmd('screen')).screen === 'Main', 40000, 'unlocked'); return w; };

  win = sh(BIN, [], appEnv(), path.join(W, 'logs/app-0.9.0.log'));
  let w = await open();
  const u0 = await w.cmd('update');
  check('5d the installed 0.9.0 runs, trusts the feed, is a Binary target next to install.sh\'s file', u0.current === OLD && u0.enabled === true && u0.target.includes(BIN), u0);
  const pid0 = u0.pid;
  const avail = await soft(until(async () => { const u = await w.cmd('update'); return u.phase === 'available' ? u : null; }, 40000, 'update offered', 500));
  check('6 0.9.0 reads the relay\'s feed and offers 0.9.1', !avail.timeout && avail.detail === NEW, avail);
  await sleep(800); check('6b screenshot of the update offer', !!(await shot('1-offered')));

  await w.cmd('update_do', { what: 'install' });
  const ready = await soft(until(async () => { const u = await w.cmd('update'); return u.phase === 'ready' ? u : (u.phase === 'failed' ? (() => { throw new Error('failed: ' + u.detail); })() : null); }, 60000, 'installed', 300));
  check('7 it downloads, checks the signature and installs 0.9.1', !ready.timeout && ready.detail === NEW, ready);
  const newHash = sha(BIN);
  check('7b the installed file is now exactly the 0.9.1 build, and the old one is kept as mega-gpui.previous', newHash === sha(newBin) && fs.existsSync(BIN + '.previous') && sha(BIN + '.previous') === oldHash, { installed: newHash === sha(newBin), previous: fs.existsSync(BIN + '.previous') });
  check('7c the running 0.9.0 went on unharmed while its file was replaced', alive(pid0) && (await w.cmd('screen')).screen === 'Main');
  await sleep(800); check('7d screenshot of "installed, restart now"', !!(await shot('2-ready')));

  await w.cmd('update_do', { what: 'restart' });
  w.close();
  await until(() => !alive(pid0), 20000, 'old process gone');
  check('8 "Restart now": the old process ends', !alive(pid0));
  w = await open();
  const u1 = await until(async () => { const u = await w.cmd('update'); return u.current === NEW ? u : null; }, 40000, 'new version up');
  check('8b a new process runs the program in the same place as version 0.9.1', u1.current === NEW && u1.pid !== pid0 && u1.target.includes(BIN), { pid: u1.pid, was: pid0 });
  check('8c it shows what is new once (the notes from the feed), and counts this start as new', u1.whatsnew && u1.whatsnew.version === NEW && /test notes/.test(u1.whatsnew.notes) && u1.boot && u1.boot.version === NEW, u1);
  await sleep(800); check('8d screenshot of the what\'s-new card', !!(await shot('3-whatsnew')));
  const stable = await soft(until(async () => { const u = await w.cmd('update'); return u.boot ? null : u; }, 45000, 'start confirmed', 1000));
  check('8e after 20 seconds up the new version is confirmed (the previous one is no longer a fallback)', !stable.timeout, stable.timeout || '');
  await w.cmd('update_do', { what: 'dismiss' });
  check('8f dismissing the card clears it', !(await w.cmd('update')).whatsnew);
  w.close();
  const pid1 = u1.pid; stopPid(win); try { process.kill(pid1, 'SIGTERM'); } catch { } await until(() => !alive(pid1), 15000, 'stopped');

  // ---- 6. a build signed with the wrong key is refused, the program left alone
  const evil = path.join(W, 'build-evil'); fs.mkdirSync(evil, { recursive: true });
  fs.copyFileSync(newBin, path.join(evil, 'mega-gpui-linux-x86_64')); fs.appendFileSync(path.join(evil, 'mega-gpui-linux-x86_64'), 'EVIL');
  const p2 = pub('0.9.2', 'wrong', evil, 'Not from the owner.');
  check('9 (setup) a 0.9.2 signed with another key is published to the scratch feed (publish-update.sh itself refuses: key id check)', p2.status !== 0 && /do not trust/.test(p2.stderr || ''), (p2.stderr || '').slice(-200));
  // publish-update.sh refused, so build the feed by hand, as a hostile relay or a stolen account could
  const evilName = 'Megatron_0.9.2_gpui-x86_64';
  fs.copyFileSync(path.join(evil, 'mega-gpui-linux-x86_64'), path.join(dist, 'update', evilName));
  const sig = run('cargo', ['run', '-q', '-p', 'commons-core', '--example', 'minisign-test-sign', '--', 'sign', path.join(W, 'wrong'), path.join(dist, 'update', evilName), '0.9.2']).stdout.trim();
  const f2 = JSON.parse(fs.readFileSync(path.join(dist, 'update/latest.json'), 'utf8'));
  f2.version = '0.9.2'; f2.platforms['linux-gpui-x86_64'] = { url: 'update/' + evilName, signature: sig };
  fs.writeFileSync(path.join(dist, 'update/latest.json'), J(f2));
  const before = sha(BIN);
  win = sh(BIN, [], appEnv(), path.join(W, 'logs/app-0.9.1.log'));
  w = await open();
  const avail2 = await soft(until(async () => { const u = await w.cmd('update'); return u.phase === 'available' ? u : null; }, 40000, 'second offer', 500));
  check('9b the 0.9.1 app is offered the 0.9.2 the feed announces', !avail2.timeout && avail2.detail === '0.9.2', avail2);
  await w.cmd('update_do', { what: 'install' });
  const failedU = await soft(until(async () => { const u = await w.cmd('update'); return u.phase === 'failed' ? u : (u.phase === 'ready' ? { ready: true } : null); }, 60000, 'refused', 300));
  check('9c it refuses the file signed with the wrong key ("The update was not installed")', failedU.phase === 'failed' && /signature|key/i.test(failedU.detail), failedU);
  await sleep(600); await shot('4-refused');
  check('9d the installed program is byte for byte what it was, and no update file was left next to it', sha(BIN) === before && !fs.readdirSync(path.dirname(BIN)).some(f => f.includes('update-new')), fs.readdirSync(path.dirname(BIN)));
  const pidB = (await w.cmd('update')).pid; w.close(); stopPid(win); try { process.kill(pidB, 'SIGTERM'); } catch { } await until(() => !alive(pidB), 15000, 'stopped');

  // ---- 7. uninstall
  const un = run(path.join(pkg, 'mega-linux-x86_64/uninstall.sh'), [], cleanEnv());
  const left = run('find', [path.join(home, '.local/bin'), path.join(home, '.local/share/applications'), path.join(home, '.local/share/mega'), '-type', 'f']).stdout.trim();
  check('10 uninstall.sh removes the program, its .previous and update leftovers, the entry and the icons; the account folder stays', un.status === 0 && left === '' && fs.existsSync(data), { left, status: un.status });

  // ---- 8. no Vulkan
  // (uninstalled above: run the newest build directly)
  const nvBin = run(newBin, [], { ...appEnv(), VK_ICD_FILENAMES: '/nonexistent/none.json', VK_DRIVER_FILES: '/nonexistent/none.json', COMMONS_DATA: path.join(W, 'data/novk'), MEGA_GPUI_UI_TEST: '' }, { timeout: 25000 });
  const txt = path.join(W, 'data/novk/logs/no-window.txt');
  check('11 with no Vulkan driver the app says why and exits with status 1 (no window, no hang)', nvBin.status === 1 && /Vulkan/.test(nvBin.stderr) && /vulkaninfo/.test(nvBin.stderr), { status: nvBin.status, err: (nvBin.stderr || '').slice(0, 300) });
  check('11b ...and writes the same text to logs/no-window.txt', fs.existsSync(txt) && /Vulkan/.test(fs.readFileSync(txt, 'utf8')));
} catch (e) { res.error = String(e.stack || e); console.log('ERROR', res.error); failed++; }
res.failed = failed;
fs.writeFileSync(path.join(W, 'result.json'), J({ ...res, checks: results }));
for (const p of [win, bridge, relay, kw]) if (p) stopPid(p);
console.log(failed ? `${failed} FAILED` : 'ALL PASSED', '->', path.join(W, 'result.json'));
await sleep(500); stopAll(); process.exit(failed ? 1 : 0);
