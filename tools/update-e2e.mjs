// Signed updates, end to end on loopback: the packaged Linux app (AppImage) at version N, built with a throwaway public
// key, on a clean Ubuntu in Docker (virtual display, no window on the desktop), against a throwaway relay that serves
// version N+1 published by scripts/publish-update.sh. Checks that the app finds, verifies and installs N+1 and restarts
// into it; that a tampered file, a wrong signature, a signature for another version, and a manifest pointing at another
// server are all REFUSED with the installed app left untouched; that nothing is offered or installed while a call runs;
// and that the relay's minimum version reaches the app.
//
//   docker build -t commons-e2e-ubuntu -f tools/e2e.Dockerfile tools
//   cargo build -j2 -p commons-relay
//   systemd-run --user --scope -q -p MemoryMax=6G node tools/update-e2e.mjs <workdir>
// <workdir> must hold keys/test.key(.pub|.password), keys/wrong.key(.password) (throwaway keys from `tauri signer
// generate --ci`) and n/Megatron-linux-x86_64.AppImage built as version N with keys/test.key.pub (docs/UPDATES.md has the
// commands). Version N+1 is built and published into <workdir>/published the first time, and reused after that.
import fs from 'node:fs'; import path from 'node:path'; import http from 'node:http'; import crypto from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { root, run, sleep, stopAll } from './lib/env.mjs';

const W = path.resolve(process.argv[2] || '');
if (!process.argv[2] || !fs.existsSync(path.join(W, 'n/Megatron-linux-x86_64.AppImage'))) { console.error('usage: node tools/update-e2e.mjs <workdir> (see the top of this file)'); process.exit(2); }
const N = '0.1.901', N1 = '0.1.902';
const out = path.join(W, 'run'); fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const site = path.join(out, 'site'), upd = path.join(site, 'update');
const RELAY_PORT = 26801, EVIL_PORT = 26802, DRIVER = 26811, RELAY = `http://127.0.0.1:${RELAY_PORT}`;
const CLI = path.join(root, 'tools/node_modules/.bin/tauri');
let failed = 0; const check = (n, ok, d = '') => { if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${n}${d ? '  [' + d + ']' : ''}`); };
const J = JSON.stringify;
const sha = f => crypto.createHash('sha256').update(fs.readFileSync(f)).digest('hex');
async function until(fn, ms = 30000, what = 'condition') { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(300); } }

// ---- version N+1, published by the real script (Linux only, scratch mode: the repository's version is not touched)
const pub = path.join(W, 'published');
if (!fs.existsSync(path.join(pub, 'update/latest.json')) || JSON.parse(fs.readFileSync(path.join(pub, 'update/latest.json'))).version !== N1) {
  console.log('publishing', N1, 'with scripts/publish-update.sh (a Docker build; log in', path.join(W, 'publish.log') + ')');
  fs.rmSync(pub, { recursive: true, force: true });
  execFileSync(path.join(root, 'scripts/publish-update.sh'), [N1, '--only', 'linux', '--notes', 'Test release: the update path, end to end.'], {
    env: { ...process.env, COMMONS_UPDATE_SCRATCH: '1', COMMONS_UPDATE_DIST: pub, COMMONS_UPDATE_KEY: path.join(W, 'keys/test.key'), COMMONS_UPDATE_PUBKEY_FILE: path.join(W, 'keys/test.key.pub') },
    stdio: ['ignore', fs.openSync(path.join(W, 'publish.log'), 'w'), fs.openSync(path.join(W, 'publish.log'), 'a')], timeout: 3600e3 });
}
const good = JSON.parse(fs.readFileSync(path.join(pub, 'update/latest.json')));
const goodFile = path.join(pub, 'update', path.basename(good.platforms['linux-x86_64'].url));
check('U0 publish-update.sh wrote a manifest with a relative file address, the notes, and a signature', good.version === N1 && good.platforms['linux-x86_64'].url === `update/Commons_${N1}_x86_64.AppImage` && /^Test release/.test(good.notes) && good.platforms['linux-x86_64'].signature.length > 100 && fs.existsSync(path.join(pub, 'Megatron-linux-x86_64.AppImage')), J(good).slice(0, 200));

// ---- the relay (fresh code: /update/ and --min-app-version), and a server that is not the relay
fs.mkdirSync(site, { recursive: true });
run(path.join(root, 'target/debug/commons-relay'), ['--bind', `127.0.0.1:${RELAY_PORT}`, '--data', path.join(out, 'relay'), '--site', site, '--min-app-version', N1], {}, path.join(out, 'relay.log'));
let evilHits = 0;
const evil = http.createServer((q, r) => { evilHits++; r.writeHead(200, { 'content-type': 'application/octet-stream' }); r.end(fs.readFileSync(goodFile)); }).listen(EVIL_PORT, '127.0.0.1');
await until(() => fetch(RELAY + '/health').then(r => r.ok), 15000, 'relay');
const setManifest = m => { fs.mkdirSync(upd, { recursive: true }); fs.writeFileSync(path.join(upd, 'latest.json'), J(m)); };
const putFile = (name, bytes) => { fs.mkdirSync(upd, { recursive: true }); fs.writeFileSync(path.join(upd, name), bytes); };

// ---- version N, installed where a friend would keep it (writable, so the updater can replace it)
const inst = path.join(out, 'install'); fs.mkdirSync(inst);
const APP = path.join(inst, 'Megatron.AppImage');
fs.copyFileSync(path.join(W, 'n/Megatron-linux-x86_64.AppImage'), APP); fs.chmodSync(APP, 0o755);
const shaN = sha(APP), shaN1 = sha(goodFile);
check('U0 versions N and N+1 are different files', shaN !== shaN1);

const box = 'commons-update-e2e';
const dsh = cmd => execFileSync('docker', ['exec', box, 'sh', '-c', cmd], { encoding: 'utf8', timeout: 20000 });
class App {
  async session() {
    const r = await this.wd('POST', '/session', { capabilities: { alwaysMatch: { browserName: 'wry', 'webkitgtk:browserOptions': { binary: APP, args: [] } } } });
    this.sid = r.sessionId; if (!this.sid) throw new Error('no session: ' + J(r));
  }
  async wd(method, p, body) {
    const r = await fetch(`http://127.0.0.1:${DRIVER}${p}`, { method, headers: { 'content-type': 'application/json' }, body: body ? J(body) : undefined, signal: AbortSignal.timeout(90000) });
    const j = await r.json(); if (j.value && j.value.error) throw new Error(`${j.value.error}: ${j.value.message}`); return j.value;
  }
  async ev(js) {
    const r = await this.wd('POST', `/session/${this.sid}/execute/async`, { script: `const cb = arguments[arguments.length - 1]; (async () => { ${js} })().then(v => cb({ ok: v === undefined ? null : v }), e => cb({ err: String(e && e.message || e) }));`, args: [] });
    if (r.err) throw new Error(r.err); return r.ok;
  }
  rpc(m, a = {}) { return this.ev(`return await rpc(${J(m)}, ${J(a)});`); }
  inv(c) { return this.ev(`return await window.__TAURI__.core.invoke(${J(c)});`); }
  async shot(name) { try { fs.writeFileSync(path.join(out, name + '.png'), Buffer.from(await this.wd('GET', `/session/${this.sid}/screenshot`), 'base64')); } catch (e) { console.log('no screenshot: ' + e.message); } }
  // The app's own process (its name, however it was started), and with `all` also the AppImage runtime around it.
  pids(all = false) { return dsh(`ps -eo pid=,comm=,args=`).trim().split('\n').map(l => l.trim().split(/\s+/)).filter(([, c, ...r]) => c === 'commons-app' || (all && r.join(' ').includes('Megatron.AppImage'))).map(([p]) => +p); }
  // The update checker, driven by hand instead of waiting hours: look now, and let the offer show (not awaited, it is a dialog).
  async look() { return this.ev(`const U = window.__commonsUpdate; U.later = 0; U.offer = null; await U.check(); setTimeout(() => U.step(), 0); return { offer: U.offer, error: U.error };`); }
  dialog() { return this.ev(`const m = document.querySelector('.modal'); return m ? { title: m.querySelector('h2').textContent, text: m.querySelector('.mbody').innerText, err: m.querySelector('#m-err').textContent } : null;`); }
  press(label) { return this.ev(`const b = [...document.querySelectorAll('.modal .acts button')].find(b => b.textContent === ${J(label)}); if (!b) throw new Error('no button ' + ${J(label)}); b.click(); return true;`); }
}

const PASS = 'correct horse battery';
const link = 'commons://relay/' + Buffer.from(J({ relay: RELAY, secret: '' })).toString('base64url');
const a = new App();
try {
  run('docker', ['run', '--rm', '--init', '--name', box, '--network', 'host', '--memory=2g', '--user', `${process.getuid()}:${process.getgid()}`,
    '-e', 'HOME=/tmp', '-e', 'APPIMAGE_EXTRACT_AND_RUN=1', '-e', 'TAURI_WEBVIEW_AUTOMATION=true', '-e', 'WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1', '-e', 'DISPLAY=:97',
    '-e', 'COMMONS_DATA=' + path.join(out, 'data'), '-e', 'COMMONS_RELAY=', '-v', `${out}:${out}`, 'commons-e2e-ubuntu',
    'sh', '-c', `Xvfb :97 -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 & sleep 1; WebKitWebDriver --port=${DRIVER} --host=127.0.0.1 & wait`], {}, path.join(out, 'driver.log'));
  await until(() => fetch(`http://127.0.0.1:${DRIVER}/status`).then(r => r.ok), 30000, 'driver');
  await a.session();
  await until(() => a.ev(`return !!document.querySelector('#lnk') && !!window.__commonsUpdate;`), 60000, 'first-run screen');
  const st0 = await a.inv('update_status');
  check('U1 the installed app is version N', st0.current === N, J(st0));
  // account, a community (so there is a room to "call" in)
  await a.ev(`const set = (s, v) => { const e = document.querySelector(s); e.value = v; e.dispatchEvent(new Event('input', { bubbles: true })); };
    set('#lnk', ${J(link)}); await new Promise(r => setTimeout(r, 400)); set('#nm', 'Alice'); set('#pp', ${J(PASS)}); set('#pp2', ${J(PASS)}); document.querySelector('#go').click();`);
  await until(() => a.ev(`const m = [...document.querySelectorAll('.modal .acts button')].pop(); if (document.querySelector('.key-code') && m) m.click(); return !document.querySelector('#app').hidden && !document.querySelector('.key-code');`), 60000, 'account made');
  const cid = (await a.rpc('create_community', { name: 'Friends' })).id;
  const gid = await until(() => a.ev(`const s = await rpc('snapshot'); const c = s.communities.find(x => x.id === ${J(cid)}); return c && c.channels[0] && c.channels[0].joined && c.channels[0].gid;`), 30000, 'room');

  // U2: nothing published yet: a quiet "no", not an error
  let r = await a.look();
  check('U2 a relay with nothing published offers nothing and shows no error', !r.offer && !r.error && !(await a.dialog()), J(r));

  // U3: the file was tampered with on the server (same length, a few bytes changed)
  const bytes = fs.readFileSync(goodFile); const bad = Buffer.from(bytes); for (let i = 0; i < 64; i++) bad[(bytes.length >> 1) + i] ^= 0x5a;
  putFile(`Commons_${N1}_tampered.AppImage`, bad);
  const lin = good.platforms['linux-x86_64'];
  setManifest({ ...good, platforms: { 'linux-x86_64': { ...lin, url: `update/Commons_${N1}_tampered.AppImage` } } });
  r = await a.look();
  check('U3a the update is found, with its notes', r.offer && r.offer.version === N1 && /Test release/.test(r.offer.notes || ''), J(r));
  check('U3b the relay\'s minimum version reaches the app: N is told it must update', r.offer && r.offer.required === true && r.offer.minimum === N1 && await a.ev(`const b = document.querySelector('#upd'); return !b.hidden && b.classList.contains('need') && b.textContent.includes(${J(N1)});`));
  await until(() => a.dialog().then(d => d && d.title === 'Update available'), 15000, 'offer dialog');
  const d0 = await a.dialog();
  check('U3c the offer asks first: version, Install now and Later', /Version 0\.1\.902 of Megatron is available \(you have 0\.1\.901\)\. Install now\?/.test(d0.text) && await a.ev(`return [...document.querySelectorAll('.modal .acts button')].map(b => b.textContent).join('|') === 'Later|Install now';`), d0.text.slice(0, 160));
  await a.shot('u3-offer');
  await a.press('Install now');
  const e3 = await until(() => a.dialog().then(d => d && d.err), 120000, 'install result');
  check('U3d a tampered file is REFUSED: signature check failed', /signature check/i.test(e3) && /NOT installed/.test(e3), e3);
  check('U3e ...and the installed app is untouched', sha(APP) === shaN);
  await a.shot('u3-tampered-refused');
  await a.press('Later');
  check('U3f "Later" closes it, and it stays closed', await until(async () => !(await a.dialog()), 5000, 'closed').then(async () => { await sleep(6000); return !(await a.dialog()); }));

  // U4: the right file with a signature from another key
  putFile(path.basename(goodFile), fs.readFileSync(goodFile));
  const wrongCopy = path.join(out, 'wrongsig.AppImage'); fs.copyFileSync(goodFile, wrongCopy);
  execFileSync(CLI, ['signer', 'sign', '--app-version', N1, wrongCopy], { env: { ...process.env, TAURI_SIGNING_PRIVATE_KEY_PATH: path.join(W, 'keys/wrong.key'), TAURI_SIGNING_PRIVATE_KEY_PASSWORD: '' }, stdio: 'ignore' });
  setManifest({ ...good, platforms: { 'linux-x86_64': { ...lin, signature: fs.readFileSync(wrongCopy + '.sig', 'utf8') } } });
  r = await a.look();
  await until(() => a.dialog().then(d => d && d.title === 'Update available'), 15000, 'offer dialog');
  await a.press('Install now');
  const e4 = await until(() => a.dialog().then(d => d && d.err), 120000, 'install result');
  check('U4 a signature made with another key is REFUSED, app untouched', /signature check/i.test(e4) && sha(APP) === shaN, e4);
  await a.press('Later');

  // U5: a genuine file and signature, announced as a newer version than it was signed for
  setManifest({ ...good, version: '0.1.903' });
  r = await a.look();
  await until(() => a.dialog().then(d => d && d.title === 'Update available'), 15000, 'offer dialog');
  await a.press('Install now');
  const e5 = await until(() => a.dialog().then(d => d && d.err), 120000, 'install result');
  check('U5 a signature for another version than announced is REFUSED, app untouched', /different version/i.test(e5) && sha(APP) === shaN, e5);
  await a.press('Later');

  // U6: the manifest on the relay points at another server
  setManifest({ ...good, platforms: { 'linux-x86_64': { ...lin, url: `http://127.0.0.1:${EVIL_PORT}/update/Commons_${N1}_x86_64.AppImage` } } });
  r = await a.look();
  await sleep(6000);
  check('U6 a manifest pointing at another server is REFUSED before anything is fetched', !r.offer && /another server/.test(r.error) && !(await a.dialog()) && evilHits === 0 && sha(APP) === shaN, J(r) + ' hits=' + evilHits);

  // U7: during a call nothing is offered or installed; it waits until the call has ended
  setManifest(good);
  await a.ev(`window.__fakeCall = setInterval(() => rpc('call_state', { gid: ${J(gid)} }).catch(() => { }), 1500);`);   // what call.js does while a call runs
  await sleep(2000);
  r = await a.look();
  const st7 = await a.inv('update_status');
  const inst7 = await a.inv('update_install');
  await sleep(12000);
  check('U7a during a call the update is found but NOT offered', r.offer && r.offer.version === N1 && st7.in_call === true && !(await a.dialog()), J(st7));
  check('U7b ...and an install asked for anyway waits for the call', inst7.waiting_for_call === true && sha(APP) === shaN, J(inst7));
  await a.ev(`clearInterval(window.__fakeCall);`);
  const ended = Date.now();
  await until(() => a.dialog().then(d => d && d.title === 'Update available'), 60000, 'offer after the call');
  const waited = (Date.now() - ended) / 1000;
  check('U7c once the call has ended, the offer comes', waited >= 15 && waited < 45, `${waited.toFixed(1)} s after the call`);

  // U8: install: download, verify, replace the AppImage, restart into N+1
  const before = a.pids();
  await a.press('Install now');
  await until(() => sha(APP) === shaN1, 180000, 'AppImage replaced');
  check('U8a the AppImage on disk is now exactly the published N+1', sha(APP) === shaN1);
  const after = await until(() => { const p = a.pids(); return p.length && p.every(x => !before.includes(x)) && p; }, 60000, 'restarted process')
    .catch(e => { console.log('processes:\n' + dsh('ps -eo pid,ppid,etimes,args')); throw e; });
  check('U8b the app restarted itself as a new process', after.length >= 1, `before ${before} after ${after}`);
  const perm = (fs.statSync(APP).mode & 0o777).toString(8);
  check('U8c the updated AppImage is still executable', perm === '755', perm);

  // U9: the restarted app is N+1 with the account intact. (Its window is not under the test driver: stop it, open again.)
  for (const p of a.pids(true)) { try { dsh(`kill ${p}`); } catch { } }
  await until(() => !a.pids(true).length, 20000, 'restarted app stopped');
  try { await a.wd('DELETE', `/session/${a.sid}`); } catch { }
  await a.session();
  await until(() => a.ev(`return !!document.querySelector('#pp');`), 60000, 'unlock screen');
  const st9 = await a.inv('update_status');
  check('U9a after the update the app is version N+1', st9.current === N1, J(st9));
  await a.ev(`document.querySelector('#pp').value = ${J(PASS)}; document.querySelector('#go').click();`);
  await until(() => a.ev(`return !document.querySelector('#app').hidden;`), 60000, 'unlocked');
  const snap = await a.rpc('snapshot');
  check('U9b the account and its community came through the update', snap.me.name === 'Alice' && snap.communities.some(c => c.name === 'Friends'));
  const c9 = await a.inv('update_check');
  check('U9c and now there is nothing newer, and no warning about the minimum', c9.available === false && !c9.required, J(c9));
  await a.shot('u9-updated');
} catch (e) { failed++; console.log('FAIL  stopped early: ' + (e.stack || e)); try { console.log(J(await a.ev(`return window.__commonsUpdate && window.__commonsUpdate.log`))); } catch { } }
try { execFileSync('docker', ['rm', '-f', box], { stdio: 'ignore' }); } catch { }
evil.close(); stopAll();
console.log(failed ? `${failed} FAILED` : 'ALL PASSED');
process.exit(failed ? 1 : 0);
