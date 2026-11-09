// The packaged Linux app, as a friend gets it: dist/Megatron-linux-x86_64.AppImage on a clean Ubuntu in Docker, against the
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
const APP = process.env.COMMONS_APPIMAGE || path.join(root, 'dist/Megatron-linux-x86_64.AppImage');
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
      '-v', `${out}:${out}`, '-v', `${APP}:/app/Megatron.AppImage:ro`, 'commons-e2e-ubuntu',
      'sh', '-c', `Xvfb :${d} -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 & sleep 1; exec WebKitWebDriver --port=${this.port} --host=127.0.0.1`], {}, path.join(out, this.name + '-driver.log'));
    this.display = ':' + d;
    await until(() => fetch(`http://127.0.0.1:${this.port}/status`).then(r => r.ok), 20000, 'driver');
    const r = await this.wd('POST', '/session', { capabilities: { alwaysMatch: { browserName: 'wry', 'webkitgtk:browserOptions': { binary: '/app/Megatron.AppImage', args: [] } } } });
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
  // Runs inside this person's container, on their virtual display.
  sh(cmd) { return execFileSync('docker', ['exec', '-e', 'DISPLAY=' + this.display, this.box, 'sh', '-c', cmd], { encoding: 'utf8', timeout: 20000 }); }
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
  const panel = await (async () => { const u = await openCall(alice); return { u, text: await alice.ev(`return document.querySelector('#call-dock').innerText;`) }; })();
  await alice.shot('call-dock');
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

  // ---- game keys, through the real path: a program runs, X sees the key, the app tells the tab over the loopback bridge
  await call(A, `c.stopShare()`).catch(() => { });
  const tone = async () => { const r = await call(B, `c.probeTone(${J(devA)}, 500, 900, 'microphone')`).catch(() => null); return !!(r && r.present && r.toneShare > 0.15); };
  const heardAgain = async ms => { const t = Date.now(); for (;;) { if (await tone()) return true; if (Date.now() - t > ms) return false; } };
  const silent = async ms => { const t = Date.now(); for (;;) { if (!(await tone())) return true; if (Date.now() - t > ms) return false; await sleep(200); } };
  const gameMuted = () => call(A, `c.gameMuted`);
  const peerSees = () => call(B, `(() => { const p = c.room.remoteParticipants.get(${J(devA)}); return !!p && !p.isMicrophoneEnabled; })()`);
  alice.sh(`cp /bin/sleep /tmp/mygame; (nohup /tmp/mygame 600 >/dev/null 2>&1 &) ; true`);
  await sleep(500);
  const programs = await alice.rpc('running_programs');
  check('G1 the picker lists the running program', programs.includes('mygame'), programs.length + ' programs');
  const info = await alice.rpc('game_keys_get');
  check('G2 the app says how it sees keys here', info.backend === 'X11' && info.hold === true, info.backend);
  await alice.ev(`document.querySelector('#btn-settings').click(); await new Promise(r => setTimeout(r, 300)); document.querySelector('#set-gamekeys').click(); await new Promise(r => setTimeout(r, 900));`);
  await alice.ev(`const a = document.querySelector('#gk-add'); a.value = 'mygame'; document.querySelector('#gk-addb').click(); await new Promise(r => setTimeout(r, 200));
    const b = document.querySelectorAll('.gk-key')[1]; b.click(); await new Promise(r => setTimeout(r, 100)); document.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyV', bubbles: true, cancelable: true })); await new Promise(r => setTimeout(r, 200));`);
  await alice.shot('game-keys');
  await alice.ev(`[...document.querySelectorAll('.gk-modal .acts button')].pop().click(); await new Promise(r => setTimeout(r, 500)); document.querySelector('.modal .acts button')?.click(); await new Promise(r => setTimeout(r, 300));`);
  const saved = await alice.rpc('game_keys_get');
  check('G3 settings page saves a game, a key and hold mode', saved.settings.rules.length === 1 && saved.settings.rules[0].process === 'mygame' && saved.settings.rules[0].key === 'KeyV' && saved.settings.rules[0].mode === 'hold', J(saved.settings));
  await until(() => A.ev(`(async()=>{const r=await rpc('game_mute_wait',{gid:GID,since:-1});return r.running})()`), 15000, 'helper running').catch(() => { });
  check('G4 before the key, alice is heard', await heardAgain(6000));
  await sleep(2500); // the process scan
  const t0 = Date.now();
  alice.sh('xdotool keydown v');
  await until(gameMuted, 3000, 'muted by key');
  check('G5 holding the key mutes her', true, (Date.now() - t0) + ' ms including docker exec');
  check('G6 friends hear nothing', await silent(3000));
  check('G7 friends just see a normal mute', await peerSees());
  check('G8 the call says so', /Muted by game key/.test(await A.ev(`document.querySelector('#call-dock').innerText`)));
  const up = Date.now();
  alice.sh('xdotool keyup v');
  await until(async () => !(await gameMuted()), 3000, 'released');
  const held = Date.now() - up;
  check('G9 letting go comes back, not before the delay', held >= 150, held + ' ms');
  check('G10 and she is heard again', await heardAgain(6000));

  await call(A, `c.setMuted(true)`);
  alice.sh('xdotool keydown v'); await until(gameMuted, 3000, 'muted by key'); alice.sh('xdotool keyup v');
  await until(async () => !(await gameMuted()), 3000, 'released'); await sleep(500);
  check('G11 a manual mute wins over the key letting go', (await call(A, `c.muted`)) && (await peerSees()));
  await call(A, `c.setMuted(false)`);
  check('G12 and unmuting by hand brings her back', await heardAgain(6000));

  // Press to toggle: the key flips it, and pressing Unmute beats it.
  await alice.rpc('game_keys_set', { rules: [{ process: 'mygame', key: 'KeyV', mode: 'toggle' }] });
  await sleep(4500); await sleep(2500);
  alice.sh('xdotool key --delay 40 v'); await until(gameMuted, 4000, 'toggled on'); await sleep(600);
  check('G12a toggle: one press mutes and it stays', await gameMuted());
  alice.sh('xdotool key --delay 40 v'); await until(async () => !(await gameMuted()), 3000, 'toggled off');
  alice.sh('xdotool key --delay 40 v'); await until(gameMuted, 3000, 'toggled on again');
  await A.ev(`document.querySelector('#c-mute').click()`);
  await until(async () => !(await gameMuted()), 3000, 'unmute beats the toggle');
  check('G12b pressing Unmute beats a toggled key', !(await call(A, `c.muted`)));
  await alice.rpc('game_keys_set', { rules: [{ process: 'mygame', key: 'KeyV', mode: 'hold' }] });
  await sleep(4500); await sleep(2500);

  alice.sh('xdotool keydown v'); await until(gameMuted, 3000, 'muted by key');
  alice.sh('for p in $(ps -eo pid,comm | awk \'$2=="mygame"{print $1}\'); do kill $p; done');
  await until(async () => !(await gameMuted()), 6000, 'game closed');
  check('G13 the game closing while the key is held gives the mic back', await heardAgain(6000));
  alice.sh('xdotool keyup v');
  alice.sh(`(nohup /tmp/mygame 600 >/dev/null 2>&1 &) ; true`); await sleep(2800);
  alice.sh('xdotool keydown v'); await until(gameMuted, 3000, 'muted by key');
  // The helper's other half: the whole app dies with the key down. The tab is on its own, and lets go.
  execFileSync('docker', ['rm', '-f', alice.box], { stdio: 'ignore' });
  await until(async () => !(await gameMuted()), 12000, 'app gone');
  check('G14 the app dying gives the mic back in the tab', await heardAgain(6000));
  await call(A, `c.leave()`).catch(() => { }); await call(B, `c.leave()`).catch(() => { });
} catch (e) { failed++; console.log('FAIL  stopped early: ' + (e.stack || e)); }
await alice.stop(); await bob.stop();
host.offline(); host.stopTerminator(); stopAll();
console.log(failed ? `${failed} FAILED` : 'ALL PASSED');
process.exit(failed ? 1 : 0);
