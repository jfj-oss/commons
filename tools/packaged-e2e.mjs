// The packaged Linux app, as a friend gets it: dist/Commons-linux-x86_64.AppImage on a clean Ubuntu in Docker, against the
// relay and LiveKit that deploy/go-online.sh starts, driven through WebKit's own WebDriver. The display is an Xvfb inside the
// container, so nothing can open on the desktop. The app's Call button hands off to a browser; xdg-open is replaced by a stub
// that records the link, and Chrome for Testing opens it headless with fake media.
//   docker build -t commons-e2e-ubuntu -f tools/e2e.Dockerfile tools
//   systemd-run --user --scope -q -p MemoryMax=6G node tools/packaged-e2e.mjs [outdir]
import fs from 'node:fs'; import path from 'node:path'; import { execFileSync } from 'node:child_process';
import { root, run, startChrome, openPage, sleep, stopAll, FAKE_DISPLAY } from './lib/env.mjs';
import { toneScript } from './lib/world.mjs';
import { startHost, forceRelay, relayPath, screenShot } from './lib/host.mjs';

const out = process.argv[2] || '/tmp/commons-packaged-e2e';
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const APP = process.env.COMMONS_APPIMAGE || path.join(root, 'dist/Commons-linux-x86_64.AppImage');
let failed = 0; const check = (n, ok, d = '') => { if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${n}${d ? '  [' + d + ']' : ''}`); };
const J = JSON.stringify;
async function until(fn, ms = 30000, what = 'condition') { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(300); } }

// A stub xdg-open: the link goes to a file instead of a browser window.
const bin = path.join(out, 'bin'); fs.mkdirSync(bin);
fs.writeFileSync(path.join(bin, 'xdg-open'), `#!/bin/sh\nprintf '%s\\n' "$1" >> "$COMMONS_TEST_URLS"\n`, { mode: 0o755 });

class App {
  constructor(name, port) { this.name = name; this.port = port; this.urls = path.join(out, name + '.urls'); }
  async start() {
    this.box = 'commons-e2e-' + this.name;
    const d = 90 + (this.port % 10), e = (k, v) => ['-e', `${k}=${v}`];
    run('docker', ['run', '--rm', '--name', this.box, '--network', 'host', '--memory=2g', '--user', `${process.getuid()}:${process.getgid()}`,
      ...e('HOME', '/tmp'), ...e('APPIMAGE_EXTRACT_AND_RUN', '1'), ...e('TAURI_WEBVIEW_AUTOMATION', 'true'), ...e('WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS', '1'), ...e('DISPLAY', ':' + d),
      ...e('COMMONS_DATA', path.join(out, this.name)), ...e('COMMONS_RELAY', ''), ...e('COMMONS_TEST_URLS', this.urls), ...e('PATH', bin + ':/usr/local/bin:/usr/bin:/bin'),
      '-v', `${out}:${out}`, '-v', `${APP}:/app/Commons.AppImage:ro`, 'commons-e2e-ubuntu',
      'sh', '-c', `Xvfb :${d} -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 & sleep 1; exec WebKitWebDriver --port=${this.port} --host=127.0.0.1`], {}, path.join(out, this.name + '-driver.log'));
    await until(() => fetch(`http://127.0.0.1:${this.port}/status`).then(r => r.ok), 20000, 'driver');
    const r = await this.wd('POST', '/session', { capabilities: { alwaysMatch: { browserName: 'wry', 'webkitgtk:browserOptions': { binary: '/app/Commons.AppImage', args: [] } } } });
    this.sid = r.sessionId; if (!this.sid) throw new Error('no session: ' + J(r));
  }
  async wd(method, p, body) {
    const r = await fetch(`http://127.0.0.1:${this.port}${p}`, { method, headers: { 'content-type': 'application/json' }, body: body ? J(body) : undefined, signal: AbortSignal.timeout(90000) });
    const j = await r.json(); if (j.value && j.value.error) throw new Error(`${this.name}: ${j.value.error}: ${j.value.message}`); return j.value;
  }
  // Runs in the app's page. rpc() there is the real Tauri path into the packaged core.
  async ev(js) {
    const r = await this.wd('POST', `/session/${this.sid}/execute/async`, { script: `const cb = arguments[arguments.length - 1]; (async () => { ${js} })().then(v => cb({ ok: v === undefined ? null : v }), e => cb({ err: String(e && e.message || e) }));`, args: [] });
    if (r.err) throw new Error(this.name + ': ' + r.err); return r.ok;
  }
  rpc(m, a = {}) { return this.ev(`return await rpc(${J(m)}, ${J(a)});`); }
  async firstRun(link, name) {
    await until(() => this.ev(`return !!document.querySelector('#lnk');`), 60000, this.name + ' first-run screen');
    await this.ev(`const set = (s, v) => { const e = document.querySelector(s); e.value = v; e.dispatchEvent(new Event('input', { bubbles: true })); };
      set('#lnk', ${J(link)}); await new Promise(r => setTimeout(r, 400));`);
    await this.shot(this.name + '-link-pasted');
    await this.ev(`const set = (s, v) => { const e = document.querySelector(s); e.value = v; e.dispatchEvent(new Event('input', { bubbles: true })); }; set('#nm', ${J(name)}); set('#pp', 'correct horse battery'); set('#pp2', 'correct horse battery');
      document.querySelector('#go').click();`);
    // the recovery key is shown once and has to be acknowledged
    await until(() => this.ev(`const m = [...document.querySelectorAll('.modal button, dialog button, .acts button')].pop(); if (document.querySelector('.key-code') && m) { m.click(); } return !document.querySelector('#app').hidden && !document.querySelector('.key-code');`), 40000, this.name + ' past recovery key');
  }
  async shot(name) { try { fs.writeFileSync(path.join(out, name + '.png'), Buffer.from(await this.wd('GET', `/session/${this.sid}/screenshot`), 'base64')); } catch (e) { console.log('no screenshot: ' + e.message); } }
  async stop() { try { await this.wd('DELETE', `/session/${this.sid}`); } catch { } try { execFileSync('docker', ['rm', '-f', this.box], { stdio: 'ignore' }); } catch { } }
}

const host = await startHost(out);
const alice = new App('alice', 26711), bob = new App('bob', 26712);
const PAYLOAD = Buffer.from(J({ relay: host.RELAY, secret: JSON.parse(Buffer.from(host.web.split('#relay/')[1], 'base64url')).secret })).toString('base64url');
try {
  await alice.start();
  check('P1 the AppImage starts and its web view answers', (await alice.ev(`return typeof rpc;`)) === 'function');
  const st = await alice.rpc('state');
  check('P2 a fresh install has no relay built in', st.exists === false && st.default_relay === '', J(st));
  await sleep(1500); await alice.shot('first-run');
  await alice.ev(`document.querySelector('#go').click(); await new Promise(r => setTimeout(r, 600));`);
  const noLink = await alice.ev(`return document.querySelector('#e').textContent;`);
  check('P3 no link and no address says what to do', /invite link|Display name|name|passphrase/i.test(noLink), noLink);
  await alice.ev(`document.querySelector('#go').disabled = false;`);
  await alice.firstRun(`commons://relay/${PAYLOAD}`, 'Alice');
  check('P4 account made from one pasted link', (await alice.rpc('snapshot')).me.name === 'Alice');
  const cid = (await alice.rpc('create_community', { name: 'Friends' })).id;
  const inv = (await alice.rpc('create_invite', { cid, uses: 1, ttl: 3600 })).link;
  await bob.start();
  await bob.firstRun(inv, 'Bob');
  let gid;
  await until(async () => { const s = await bob.rpc('snapshot'); const ch = s.communities.find(x => x.id === cid)?.channels.find(x => x.joined && x.status === 'ok'); gid = ch && ch.gid; return gid; }, 60000, 'bob in the room');
  check('P5 a friend gets in with nothing but the community invite', true);
  await until(async () => (await alice.rpc('snapshot')).communities[0].channels.some(x => x.joined && x.status === 'ok'), 30000, 'alice ready');
  await alice.rpc('send_text', { gid, text: 'hello from the packaged app' });
  await until(async () => (await bob.rpc('messages', { gid })).messages.some(m => m.text === 'hello from the packaged app'), 20000, 'chat');
  await until(() => bob.ev(`return document.body.textContent.includes('hello from the packaged app');`), 15000, 'message on screen').catch(async e => { console.log(await bob.ev(`return JSON.stringify({ gid: S.gid, cid: S.cid, vis: document.visibilityState, app: document.querySelector('#app').hidden, boot: document.querySelector('#boot').hidden, modal: !!document.querySelector('.modal'), text: document.body.innerText.slice(0, 400) });`)); throw e; });
  await bob.shot('chat');
  check('P6 chat arrives and is on screen', true);
  const blob = Buffer.alloc(200000, 9).toString('base64url');
  await bob.rpc('send_file', { gid, name: 'notes.bin', mime: 'application/octet-stream', data: blob, caption: '' });
  const m = await until(async () => (await alice.rpc('messages', { gid })).messages.find(x => x.kind === 'file'), 20000, 'file');
  check('P7 file arrives intact', (await alice.rpc('fetch_file', { gid, id: m.id })).data === blob, m.file?.name);

  // Call: press the real button. This web view has no WebRTC, so the panel offers the browser.
  const openCall = async a => {
    await until(() => a.ev(`const b = document.querySelector('#btn-call'); if (b && !b.hidden) { b.click(); return true; } return false;`), 20000, a.name + ' call button');
    await until(() => a.ev(`const b = document.querySelector('#c-browser'); if (b) { b.click(); return true; } return false;`), 15000, a.name + ' browser button');
    return until(() => fs.existsSync(a.urls) && fs.readFileSync(a.urls, 'utf8').trim().split('\n').pop(), 15000, a.name + ' hand-off link');
  };
  const panel = await (async () => { const u = await openCall(alice); return { u, text: await alice.ev(`return document.querySelector('#call-panel').innerText;`) }; })();
  await alice.shot('call-panel');
  check('P8 the panel explains calls open in the browser, and hands off', /open in your browser/i.test(panel.text) && /^http:\/\/127\.0\.0\.1:\d+\/callpage\.html#s=/.test(panel.u), panel.text.slice(0, 80));
  const urlB = await openCall(bob);
  const chrome = await startChrome(out, 'all', 26800, 500, ['--ignore-certificate-errors']);
  const A = await openPage(chrome, panel.u, 'alice', { before: toneScript(500) + forceRelay }), B = await openPage(chrome, urlB, 'bob', { before: toneScript(700) + forceRelay });
  for (const p of [A, B]) await p.waitFor(`!!window.__commonsCall`, 20000, 'call page');
  const call = (p, e) => p.ev(`(async()=>{ const c = window.__commonsCall; return ${e}; })()`);
  await call(A, `c.join(GID, NAME)`); await call(B, `c.join(GID, NAME)`);
  await until(async () => (await call(A, `c.room.remoteParticipants.size`)) === 1, 30000, 'call formed');
  check('P9 call from the packaged apps: frame encryption on at both ends', (await call(A, `c.room.isE2EEEnabled`)) && (await call(B, `c.room.isE2EEEnabled`)));
  const pa = await until(async () => { const v = await relayPath(A); return v.length ? v : null; }, 15000, 'stats');
  check('P10 media went through TURN over TLS on the one public port', pa.every(x => x === 'relay/tls'), pa.join());
  const devA = (await alice.rpc('snapshot')).me.device, devB = (await bob.rpc('snapshot')).me.device;
  const hears = async (p, dev, hz) => { for (let i = 0; i < 8; i++) { await sleep(900); const r = await call(p, `c.probeTone(${J(dev)}, ${hz}, 1500, 'microphone')`).catch(() => null); if (r && r.present && r.toneShare > 0.15) return true; } return false; };
  check('P11 bob hears alice', await hears(B, devA, 500));
  check('P12 alice hears bob', await hears(A, devB, 700));
  await A.ev(FAKE_DISPLAY); await A.ev(`window.__shareName = 'alice'`);
  await call(A, `c.startShare({ audio: false, profile: 'text' })`);
  const shot = await screenShot(B, devA); const near = (a, b) => a && a.every((x, i) => Math.abs(x - b[i]) < 60);
  check('P13 screen share decrypts to the right picture (synthetic screen)', shot.present && near(shot.red, [224, 32, 32]) && near(shot.green, [32, 192, 64]), J(shot));
  await call(A, `c.leave()`).catch(() => { }); await call(B, `c.leave()`).catch(() => { });
} catch (e) { failed++; console.log('FAIL  stopped early: ' + (e.stack || e)); }
await alice.stop(); await bob.stop();
host.offline(); host.stopTerminator(); stopAll();
console.log(failed ? `${failed} FAILED` : 'ALL PASSED');
process.exit(failed ? 1 : 0);
