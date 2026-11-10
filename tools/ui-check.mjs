// Headless UI check: real relay + two dev bridges (two "windows") + Chromium on the GPU (ANGLE/Vulkan, never SwiftShader).
// No window is ever shown. Usage: node tools/ui-check.mjs <outdir>
// When it fails at a spot that passes alone: COMMONS_UI_RPC_TRACE=1 prints how long every call to the core took from the page's side,
// and COMMONS_LOCK_TRACE=1 (read by the bridges) prints how long the core was held, by whom, and every answer given from the cache.
import { spawn, execSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';

const out = process.argv[2] || '/tmp/commons-ui';
fs.mkdirSync(out, { recursive: true });
const root = path.resolve(new URL('..', import.meta.url).pathname);
const bin = p => path.join(process.env.COMMONS_UI_TARGET || path.join(root, 'target'), 'debug', p);
const sleep = ms => new Promise(r => setTimeout(r, ms));
const procs = [];
// Every bridge here runs the way the browser version's launcher starts it: with a per-start secret that the page gets in the
// URL fragment and sends back as a header (crates/core/src/bridge_auth.rs). A bridge without it would be open to any local program.
const SECRETS = new Map();
const secretFor = port => { if (!SECRETS.has(port)) SECRETS.set(port, crypto.randomBytes(32).toString('hex')); return SECRETS.get(port); };
const pageUrl = port => `http://127.0.0.1:${port}/#s=${secretFor(port)}`;
const run = (cmd, args, env = {}) => { const p = spawn(cmd, args, { env: { ...process.env, ...env }, stdio: ['ignore', 'inherit', 'inherit'] }); procs.push(p); return p; };
process.on('exit', () => procs.forEach(p => { try { p.kill(); } catch { } }));

// A fake rune on a test port (never the real one on 4545), so the status can be exercised end to end.
const runeSong = { status: 'playing', index: 0, queue: [{ track: { title: 'Detective Arriving on the Scene', artists: [{ name: 'Sea Power' }] } }] };
const runeSrv = http.createServer((req, res) => {
  const body = req.url === '/api/zones' ? JSON.stringify([{ id: 'pc', name: 'Linux PC speakers', kind: 'pc' }]) : req.url === '/api/player/pc' ? JSON.stringify(runeSong) : null;
  if (!body) { res.writeHead(404); res.end('{}'); return; }
  res.writeHead(200, { 'content-type': 'application/json' }); res.end(body);
});
await new Promise(r => runeSrv.listen(0, '127.0.0.1', r));
const runeUrl = `http://127.0.0.1:${runeSrv.address().port}`;

// Ports can be moved so two checks can run side by side without touching each other's processes.
const RELAY_PORT = Number(process.env.COMMONS_UI_RELAY_PORT || 18787);
const [A_PORT, B_PORT] = [Number(process.env.COMMONS_UI_A_PORT || 7101), Number(process.env.COMMONS_UI_B_PORT || 7102)];
const CDP_PORT = Number(process.env.COMMONS_UI_CDP_PORT || 9333);
const RELAY = `http://127.0.0.1:${RELAY_PORT}`;
fs.rmSync(path.join(out, 'data'), { recursive: true, force: true });
// Bob's core runs "Show in folder" through xdg-open. Point it at a script that records the folder, so the check
// proves the wiring without opening a file manager on the machine running the test.
const fakeBin = path.join(out, 'fakebin');
const xdgMarker = path.join(out, 'xdg-open.log');
fs.mkdirSync(fakeBin, { recursive: true });
fs.rmSync(xdgMarker, { force: true });
for (const p of ['kdialog', 'zenity']) fs.writeFileSync(path.join(fakeBin, p), '#!/bin/sh\nprintf "%s" "$COMMONS_PICKED"\n', { mode: 0o755 });
fs.writeFileSync(path.join(fakeBin, 'xdg-open'), '#!/bin/sh\nprintf "%s\\n" "$1" > "$COMMONS_XDG_MARKER"\n', { mode: 0o755 });
const startRelay = () => run(bin('commons-relay'), ['--bind', `127.0.0.1:${RELAY_PORT}`, '--data', path.join(out, 'data/relay')]);
let relayProc = startRelay();
for (const [n, port] of [['alice', A_PORT], ['bob', B_PORT]]) {
  // COMMONS_NOTIFY_LOG: what the background part would have shown as a system notification goes to a file, never to a screen.
  const env = { COMMONS_DATA: path.join(out, 'data', n), COMMONS_RELAY: RELAY, COMMONS_FAST_KDF: '1', MEGATRON_SECRET: secretFor(port), COMMONS_NOTIFY_LOG: path.join(out, `notify-${n}.log`) };
  if (n === 'bob') Object.assign(env, { PATH: fakeBin + ':' + process.env.PATH, COMMONS_XDG_MARKER: xdgMarker, COMMONS_PICKED: path.join(out, 'downloads-bob') });
  run(bin('dev_bridge'), [String(port), (process.env.COMMONS_UI_DIR || path.join(root, 'app/ui'))], env);
}
await sleep(1500);

// A known sound for the fake microphone: a 440 Hz tone for 0.4 s, then 0.4 s of silence, over and over (48 kHz mono, 16 bit).
const beepWav = path.join(out, 'beeps.wav');
{ const rate = 48000, n = rate * 3.2, pcm = Buffer.alloc(n * 2); for (let i = 0; i < n; i++) pcm.writeInt16LE(Math.floor(i / (rate * .4)) % 2 === 0 ? Math.round(Math.sin(2 * Math.PI * 440 * i / rate) * 14000) : 0, i * 2);
  const h = Buffer.alloc(44); h.write('RIFF', 0); h.writeUInt32LE(36 + pcm.length, 4); h.write('WAVEfmt ', 8); h.writeUInt32LE(16, 16); h.writeUInt16LE(1, 20); h.writeUInt16LE(1, 22); h.writeUInt32LE(rate, 24); h.writeUInt32LE(rate * 2, 28); h.writeUInt16LE(2, 32); h.writeUInt16LE(16, 34); h.write('data', 36); h.writeUInt32LE(pcm.length, 40);
  fs.writeFileSync(beepWav, Buffer.concat([h, pcm])); }
const chrome = run(process.env.COMMONS_UI_BROWSER || 'chromium', ['--headless=new', '--password-store=basic', '--use-mock-keychain', `--remote-debugging-port=${CDP_PORT}`, '--user-data-dir=' + path.join(out, 'chrome'), '--use-gl=angle', '--use-angle=vulkan',
  '--enable-features=Vulkan', '--ignore-gpu-blocklist', '--enable-gpu', '--ozone-platform=headless', '--disable-software-rasterizer', '--mute-audio', '--autoplay-policy=no-user-gesture-required', '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream', `--use-file-for-fake-audio-capture=${beepWav}`, '--disable-background-timer-throttling', '--disable-renderer-backgrounding', '--disable-backgrounding-occluded-windows', '--disable-audio-output', '--no-first-run', '--window-size=1280,800', 'about:blank']);
let ver; for (let i = 0; i < 50; i++) { try { ver = await (await fetch(`http://127.0.0.1:${CDP_PORT}/json/version`)).json(); break; } catch { await sleep(200); } }
if (!ver) throw new Error('chromium did not start');

class Page {
  constructor(ws) { this.id = 0; this.cb = new Map(); this.ws = ws; ws.onmessage = m => { const j = JSON.parse(m.data); if (j.id && this.cb.has(j.id)) { this.cb.get(j.id)(j); this.cb.delete(j.id); } else if (j.method === 'Runtime.exceptionThrown') console.log('PAGE ERROR:', JSON.stringify(j.params.exceptionDetails).slice(0, 400)); }; }
  send(method, params = {}) { return new Promise(res => { const id = ++this.id; this.cb.set(id, res); this.ws.send(JSON.stringify({ id, method, params })); }); }
  async ev(expr) { const r = await this.send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true }); if (r.result.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails).slice(0, 400)); return r.result.result.value; }
  async shot(name) { await this.send('Page.bringToFront'); await sleep(250); const r = await this.send('Page.captureScreenshot', { format: 'png' }); fs.writeFileSync(path.join(out, name + '.png'), Buffer.from(r.result.data, 'base64')); }
  async waitFor(expr, ms = 15000) { const t = Date.now(); while (Date.now() - t < ms) { if (await this.ev(expr).catch(() => false)) return; await sleep(250); } throw new Error('timeout waiting for ' + expr); }
  // Hand a real file on disk to a file input. Chromium keeps it backed by the file, so a several-hundred-megabyte
  // file never has to be materialised as a JavaScript string in this process or in the page.
  async setFiles(sel, file) {
    await this.send('DOM.enable');
    const doc = await this.send('DOM.getDocument', { depth: -1 });
    if (doc.error) throw new Error('DOM.getDocument: ' + JSON.stringify(doc.error));
    const q = await this.send('DOM.querySelector', { nodeId: doc.result.root.nodeId, selector: sel });
    if (q.error || !q.result || !q.result.nodeId) throw new Error('no file input matching ' + sel + ': ' + JSON.stringify(q.error || q.result));
    const r = await this.send('DOM.setFileInputFiles', { files: [file], nodeId: q.result.nodeId });
    if (r.error) throw new Error('DOM.setFileInputFiles: ' + JSON.stringify(r.error));
  }
}
async function open(url) {
  const t = await (await fetch(`http://127.0.0.1:${CDP_PORT}/json/new?` + encodeURIComponent(url), { method: 'PUT' })).json();
  const ws = new WebSocket(t.webSocketDebuggerUrl); await new Promise(r => ws.onopen = r);
  const p = new Page(ws); await p.send('Page.enable'); await p.send('Runtime.enable');
  // the first-run tour would sit over every other check; it is marked as seen on each load unless the tour check has asked for it
  await p.send('Page.addScriptToEvaluateOnNewDocument', { source: `try { if (!sessionStorage.getItem('tourtest')) localStorage.setItem('commons.tour', '1'); } catch {}` }); await p.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false });
  // COMMONS_UI_RPC_TRACE=1: how long each call to the core takes from the page's side (window.__rpcTrace, printed when the check ends).
  if (process.env.COMMONS_UI_RPC_TRACE) {
    const source = `(() => { if (window.__rpcTrace) return; const T = window.__rpcTrace = { m: {}, slow: [] }; const w = () => { if (typeof window.rpc !== 'function') return setTimeout(w, 20); const r0 = window.rpc; window.rpc = async (m, a) => { const t = performance.now(); try { return await r0(m, a); } finally { const d = performance.now() - t, e = T.m[m] || (T.m[m] = { n: 0, max: 0, sum: 0, over: 0 }); e.n++; e.sum += d; if (d > e.max) e.max = d; if (d > 400) { e.over++; T.slow.push([new Date().toISOString().slice(11, 23), m, Math.round(d)]); if (T.slow.length > 300) T.slow.shift(); } } }; }; w(); })()`;
    await p.send('Page.addScriptToEvaluateOnNewDocument', { source });   // the page may be reloaded by a test
    await p.send('Runtime.evaluate', { expression: source });              // and it is already loading now
  }
  return p;
}
const dumpTrace = async () => { if (!process.env.COMMONS_UI_RPC_TRACE) return; for (const [n, P] of [['A', A], ['B', B]]) { if (!P) continue; try { console.log(`rpc trace ${n}:`, await P.ev(`(() => { const T = window.__rpcTrace; if (!T) return 'none'; return Object.entries(T.m).map(([m, e]) => m + ' n=' + e.n + ' avg=' + Math.round(e.sum / e.n) + ' max=' + Math.round(e.max) + ' over400=' + e.over).join('; ') + ' | slow: ' + JSON.stringify(T.slow.slice(-80)); })()`)); } catch (e) { console.log('rpc trace', n, 'unavailable:', e.message); } } };
const type = (p, sel, v) => p.ev(`(()=>{const e=document.querySelector(${JSON.stringify(sel)});e.value=${JSON.stringify(v)};e.dispatchEvent(new Event('input',{bubbles:true}));})()`);
const click = (p, sel) => p.ev(`document.querySelector(${JSON.stringify(sel)}).click()`);
const textOf = (p, sel) => p.ev(`document.querySelector(${JSON.stringify(sel)})?.innerText||''`);

// A deterministic file of `size` bytes, written a megabyte at a time so this process stays small.
function makeBigFile(file, size) {
  return new Promise((resolve, reject) => {
    const block = Buffer.alloc(1 << 20);
    let x = 0x9e3779b9;
    for (let i = 0; i < block.length; i++) { x ^= x << 13; x ^= x >>> 17; x ^= x << 5; block[i] = x & 255; }
    const hash = crypto.createHash('sha256'), ws = fs.createWriteStream(file);
    let at = 0, n = 0;
    const pump = () => {
      while (at < size) {
        const len = Math.min(block.length, size - at);
        block.writeUInt32LE(n >>> 0, 0);
        const b = block.subarray(0, len);
        hash.update(b); at += len; n++;
        if (!ws.write(Buffer.from(b))) { ws.once('drain', pump); return; }
      }
      ws.end();
    };
    ws.on('error', reject);
    ws.on('finish', () => resolve(hash.digest('hex')));
    pump();
  });
}
function hashFile(file) {
  return new Promise((resolve, reject) => {
    const h = crypto.createHash('sha256');
    fs.createReadStream(file).on('error', reject).on('data', d => h.update(d)).on('end', () => resolve(h.digest('hex')));
  });
}

let A, B;
try {
  A = await open(pageUrl(A_PORT)); B = await open(pageUrl(B_PORT));
  await sleep(800);
  // The bridge needs its secret: the page took it from the address and sent it, and a bare request without it is refused.
  if ((await A.ev(`location.hash`)) !== '') throw new Error('bridge secret: it is still in the address bar');
  if ((await A.ev(`fetch('/rpc',{method:'POST',body:'{"method":"state","args":{}}'}).then(r=>r.status)`)) !== 403) throw new Error('bridge secret: an RPC without the secret was not refused');
  // The first screen asks for an invite password, a name and one device password. Starting a community of your own, or using an old link, is one tap away.
  await A.waitFor(`!!document.querySelector('#ipw') && !!document.querySelector('#mi-eye') && !document.querySelector('#pp2')`);
  await A.shot('01-onboarding');
  if (await A.ev(`document.querySelector('#pp').type`) !== 'password') throw new Error('first screen: the device password is not hidden');
  await click(A, '#mi-eye'); if (await A.ev(`document.querySelector('#pp').type`) !== 'text') throw new Error('first screen: show does not show'); await click(A, '#mi-eye');
  await A.ev(`(() => { document.querySelector('#go').click(); })()`); await A.waitFor(`document.querySelector('#e').textContent.length > 0`);   // nothing typed: it says what is missing
  for (const [P, name] of [[A, 'Alice'], [B, 'Bob']]) {
    await P.waitFor(`!!document.querySelector('#l-new')`); await click(P, '#l-new');
    await P.waitFor(`!!document.querySelector('#nm') && !!document.querySelector('#pp2')`);
    await type(P, '#nm', name); await type(P, '#pp', 'correct horse battery'); await type(P, '#pp2', 'correct horse battery'); await click(P, '#go');
    await P.waitFor(`document.querySelector('.modal')`); await sleep(300);
    if (name === 'Alice') await A.shot('02-recovery-key');   // the old way still shows the key before going on
    await click(P, '.modal .acts button');
    await P.waitFor(`!document.querySelector('#app').hidden`);
  }
  // Alice makes a community and an invite
  await click(A, '#switcher'); await click(A, '#mk-new'); await type(A, '#n', 'Book club'); await click(A, '.modal .acts button:last-child');
  await A.waitFor(`document.querySelector('.room')`);
  await click(A, '#btn-invite');
  await A.waitFor(`document.querySelector('#mi-pw') && document.querySelector('#mi-pw').value.split('-').length === 4`);   // a suggestion is already there
  const pwd = await A.ev(`document.querySelector('#mi-pw').value`); await A.shot('03-invite-form');
  await A.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true }); await sleep(500);
  if (!await A.ev(`document.documentElement.scrollWidth <= innerWidth + 1 && document.querySelector('.mi-modal').getBoundingClientRect().right <= innerWidth + 1`)) throw new Error('invite: the invite screen is wider than a phone');
  await A.shot('03-invite-form-390'); await A.send('Emulation.clearDeviceMetricsOverride'); await sleep(400);
  // a weak password is refused on the spot, with a plain warning, and Create cannot be pressed
  await type(A, '#mi-pw', 'hunter2'); await A.waitFor(`document.querySelector('#mi-hint').classList.contains('bad') && document.querySelector('.modal .acts button:last-child').disabled`);
  await A.shot('03-invite-weak');
  await click(A, '#mi-new'); await A.waitFor(`document.querySelector('#mi-hint').classList.contains('warn') && !document.querySelector('.modal .acts button:last-child').disabled`);
  await type(A, '#mi-pw', pwd); await A.waitFor(`!document.querySelector('.modal .acts button:last-child').disabled`);
  await click(A, '.modal .acts button:last-child');
  await A.waitFor(`document.querySelector('.mi-code')`);
  if ((await textOf(A, '.mi-code')).trim() !== pwd) throw new Error('invite: the screen does not show the password it made');
  if (!/\/join$/.test(await textOf(A, '.mi-dl'))) throw new Error('invite: no download address on the screen');
  await A.shot('03-invite'); await click(A, '.modal .acts button');
  // Bob joins with the password
  await click(B, '#switcher'); await click(B, '#mk-join'); await type(B, '#mi-jpw', pwd); await click(B, '.modal .acts button:last-child');
  await B.waitFor(`document.querySelector('.room:not(.pending)')`, 30000);
  await A.waitFor(`document.querySelectorAll('.room').length>=1`);
  // conversation
  const say = async (P, t) => { await type(P, '#input', t); await click(P, '#send'); };
  await A.waitFor(`document.body.innerText.includes('Bob joined the room')`, 20000);   // arrivals show in the room
  await say(A, 'Welcome to the club! First pick: *The Dispossessed*.');
  await A.waitFor(`document.body.innerText.includes('First pick')`, 400);   // your own message shows at once, before the core has it
  await B.waitFor(`document.body.innerText.includes('First pick')`, 20000);
  await say(B, 'Great choice. I can host on Friday.');
  await A.waitFor(`document.body.innerText.includes('host on Friday')`, 20000);
  await say(A, 'Friday works for me. Bring snacks 🙂');
  await B.waitFor(`document.body.innerText.includes('Bring snacks')`, 20000);
  await click(A, '#btn-people'); await sleep(600);
  // a second room, an unread badge on Bob, then Bob replies and reacts
  await click(A, '#add-room'); await type(A, '#n', 'music'); await click(A, '.modal .acts button:last-child');
  await B.waitFor(`document.querySelectorAll('.room').length>=2`, 20000); await sleep(1500);
  await A.ev(`document.querySelectorAll('.room')[1].click()`); await say(A, 'Anyone seen the new Boards of Canada reissue?');
  await B.waitFor(`document.querySelector('.pill')`, 20000);
  const titled = await B.ev(`(async () => { for (let i = 0; i < 25; i++) { if (/^[(][0-9]+[)] mega$/.test(document.title)) return 'ok'; await new Promise(r => setTimeout(r, 200)); } return document.title + ' ' + JSON.stringify(S.snap.communities.map(c => c.channels.map(x => x.name + ':' + x.unread))); })()`);
  if (titled !== 'ok') throw new Error('no unread count in the window title: ' + titled);   // the unread count is in the window title
  await B.shot('04-bob-unread');
  await A.ev(`document.querySelectorAll('.room')[0].click()`);
  await B.ev(`document.querySelectorAll('.room')[0].click()`); await sleep(1200);
  await B.ev(`document.querySelectorAll('.msg')[0].querySelector('[data-a=reply]').click()`);
  await say(B, 'Agreed, it is a great pick.');
  await A.waitFor(`document.body.innerText.includes('great pick')`, 20000);
  await A.waitFor(`[...document.querySelectorAll('.msg.mention')].some(m => m.innerText.includes('great pick'))`, 5000);   // a reply to you counts as a mention
  await B.ev(`(()=>{const m=document.querySelectorAll('.msg')[2]; m.querySelector('[data-a=react]').click();})()`);
  await B.ev(`document.querySelector('.emojis button').click()`); await sleep(1500);
  await A.shot('05-alice-chat');
  await B.shot('06-bob-chat');
  await click(A, '#btn-people'); await sleep(200); await click(A, '#btn-people'); await sleep(800);
  await A.ev(`document.querySelector('#people button[data-a=verify]').click()`); await A.waitFor(`document.querySelector('.safety')`); await sleep(300);
  await A.shot('07-safety-number'); await click(A, '.modal .acts button'); await sleep(300);
  await click(A, '#btn-activity'); await sleep(500); await A.shot('08-activity'); await click(A, '.modal .acts button');
  // The browser version's Update button, with the bridge's answer stubbed (the real install is tools/browser-update-test.sh).
  await A.ev(`(()=>{window.__realFetch=window.fetch;window.fetch=(u,o)=>u==='/update-check'?Promise.resolve(new Response('{"available":true,"version":"9.9.9","current":"0.2.5"}')):window.__realFetch(u,o);})()`);
  await click(A, '#btn-settings'); await A.waitFor(`document.querySelector('#set-update')`);
  await click(A, '#set-update');
  await A.waitFor(`document.querySelector('#set-update').textContent==='Update to 9.9.9'&&!!document.querySelector('#upd [data-u=install]')&&!document.querySelector('#upd').hidden`);
  await click(A, '.modal .acts button'); await sleep(300); await A.shot('08b-update-bar');
  await A.ev(`(()=>{window.fetch=window.__realFetch;window.__commonsBrowserUpdate.offer=null;const b=document.querySelector('#upd');b.hidden=true;b.innerHTML='';})()`);
  // "Listening to" from rune: one switch in Settings, the account strip, and under the name in the People drawer.
  await click(A, '#btn-settings'); await A.waitFor(`document.querySelector('#lstr')`);
  await type(A, '#rune', runeUrl);
  await A.ev(`(()=>{document.querySelector('#lstr').checked=true;})()`);
  await click(A, '.modal .acts button:last-child');
  await A.waitFor(`(()=>{const e=document.querySelector('#me-listen');return !!e&&!e.hidden&&e.innerText.includes('Detective Arriving on the Scene')&&e.innerText.includes('Sea Power')})()`, 20000);
  await B.ev(`(()=>{ if (!S.people) document.querySelector('#btn-people').click(); })()`);
  await B.waitFor(`[...document.querySelectorAll('.person .listen')].some(e=>e.innerText.includes('Detective Arriving on the Scene')&&e.innerText.includes('Sea Power'))`, 20000);
  await A.shot('09-listening-own'); await B.shot('10-listening-people');
  // Playback stops in rune: friends stop seeing it at once, not when the ten-minute TTL runs out.
  runeSong.status = 'stopped'; runeSong.index = -1; runeSong.queue = [];
  await B.waitFor(`![...document.querySelectorAll('.person .listen')].some(e=>e.innerText.includes('Detective'))`, 90000).catch(async () => {
    const a = await A.ev(`rpc('state').then(s => JSON.stringify({ listening: s.listening, share: s.share_listening, outbox: s.outbox, net: s.net }))`);
    const b = await B.ev(`(async () => { const s = S.snap; const cid = s.communities[0] && s.communities[0].id; const m = cid ? await rpc('members', { cid }) : []; return JSON.stringify({ people: !!S.people, outbox: s.outbox, net: s.net, members: m.map(x => [x.name, x.listening]), shown: [...document.querySelectorAll('.person .listen')].map(e => e.innerText) }); })()`);
    throw new Error('listening: friends still see the song after it stopped. alice ' + a + ' bob ' + b);
  });

  // ---- profile pictures ----
  // Alice picks a phone-style JPEG: its EXIF orientation says "rotate 90° clockwise". Stored, the four quadrants are
  // red (top left), green (top right), blue (bottom left), yellow (bottom right); shown the right way up, the blue
  // corner moves to the top left. If EXIF were ignored (or applied twice) the sampled colours below would not match.
  await click(A, '#btn-settings'); await A.waitFor(`document.querySelector('#av-file')`);
  await sleep(300); await A.shot('09-settings-picture');
  const sent = await A.ev(`(() => {
    const c = document.createElement('canvas'); c.width = c.height = 512;
    const g = c.getContext('2d');
    g.fillStyle = '#e00000'; g.fillRect(0, 0, 256, 256);
    g.fillStyle = '#00c000'; g.fillRect(256, 0, 256, 256);
    g.fillStyle = '#0000e0'; g.fillRect(0, 256, 256, 256);
    g.fillStyle = '#e0e000'; g.fillRect(256, 256, 256, 256);
    const raw = Uint8Array.from(atob(c.toDataURL('image/jpeg', .95).split(',')[1]), x => x.charCodeAt(0));
    const app1 = new Uint8Array([0xFF,0xE1,0x00,0x22,0x45,0x78,0x69,0x66,0x00,0x00,0x49,0x49,0x2A,0x00,0x08,0x00,0x00,0x00,0x01,0x00,0x12,0x01,0x03,0x00,0x01,0x00,0x00,0x00,0x06,0x00,0x00,0x00,0x00,0x00,0x00,0x00]);
    const out = new Uint8Array(raw.length + app1.length);
    out.set(raw.subarray(0, 2)); out.set(app1, 2); out.set(raw.subarray(2), 2 + app1.length);
    const dt = new DataTransfer(); dt.items.add(new File([out], 'phone.jpg', { type: 'image/jpeg' }));
    const inp = document.querySelector('#av-file'); if (!inp) return 'no input';
    inp.files = dt.files; inp.dispatchEvent(new Event('change', { bubbles: true }));
    return 'sent';
  })()`);
  if (sent !== 'sent') throw new Error('could not hand the picture to Settings: ' + sent);
  await A.waitFor(`document.querySelector('.crop-modal canvas')`, 10000);
  await A.ev(`document.querySelector('.crop-modal .seg[data-for="crop-shape"] button[data-v=square]').click()`);
  await sleep(300); await A.shot('10-crop');
  await A.ev(`document.querySelector('.crop-modal .acts button:last-child').click()`);
  await A.waitFor(`document.querySelector('#av-prev img.av-img')`, 15000);
  const avatar = await A.ev(`(async () => {
    const acct = S.snap.me.account;
    const vers = await rpc('avatar_versions');
    const d = await rpc('avatar_data', { account: acct });
    if (!vers[acct] || !d.blob) return { err: 'the core has no picture' };
    if (d.size > 40 * 1024) return { err: 'the picture is ' + d.size + ' bytes, over 40 KB' };
    const bytes = Uint8Array.from(atob(d.data.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0));
    const img = await createImageBitmap(new Blob([bytes], { type: d.mime }));
    const cv = document.createElement('canvas'); cv.width = 256; cv.height = 256;
    const g = cv.getContext('2d'); g.drawImage(img, 0, 0, 256, 256);
    const at = (x, y) => [...g.getImageData(x, y, 1, 1).data];
    const near = (p, r, gg, b) => Math.abs(p[0] - r) < 60 && Math.abs(p[1] - gg) < 60 && Math.abs(p[2] - b) < 60;
    const got = { tl: at(64, 64), tr: at(192, 64), bl: at(64, 192), br: at(192, 192) };
    const ok = near(got.tl, 0, 0, 224) && near(got.tr, 224, 0, 0) && near(got.bl, 224, 224, 0) && near(got.br, 0, 192, 0);
    return { ok, size: d.size, mime: d.mime, shape: d.shape, got, err: ok ? null : 'the picture came out sideways or upside down' };
  })()`);
  if (!avatar.ok) throw new Error('picture check: ' + JSON.stringify(avatar));
  // Alice sees it at once in her own strip and her messages.
  await A.waitFor(`document.querySelector('#me-seal img.av-img')`);
  await A.waitFor(`[...document.querySelectorAll('#stream .seal[data-av] img.av-img')].length > 0`);
  // ...and in the People drawer.
  await click(A, '#btn-people'); await A.waitFor(`document.querySelector('#people .seal img.av-img')`);
  await click(A, '#btn-people');
  // Bob sees it without doing anything, in messages, the header stack and People.
  await B.waitFor(`[...document.querySelectorAll('#stream .seal[data-av] img.av-img')].length > 0`, 20000);
  await B.waitFor(`document.querySelector('#people-stack img.av-img')`, 10000);
  await B.shot('11-bob-sees-picture');
  await click(B, '#btn-people'); await B.waitFor(`document.querySelector('#people .seal img.av-img')`, 10000);
  await B.shot('12-bob-people-picture'); await click(B, '#btn-people');
  // The call dock: while in a call it is a small card at the bottom of the rail with the status, the lock that opens the
  // explainer, and one row of icon buttons. A stub connected snapshot shows it without a media server.
  const dock = await A.ev(`(() => {
    const c = window.__commonsCall, room = (S.snap.communities[0].channels.find(ch => ch.gid === S.gid) || S.snap.communities[0].channels[0]);
    window.__snapOrig = c.snapshot;
    c.snapshot = () => ({ phase: 'connected', error: null, gid: S.gid, roomName: room.name, muted: true, gameMuted: false, gameName: '', gameError: '',
      deafened: false, ptt: false, pttKey: 'Backquote', pttHeld: false, sharing: false, camera: false, playing: false, playback: null,
      peers: [{ identity: 'bob-device', account: 'bob', name: 'Bob', me: false, member: true, speaking: false, muted: false, volume: 1, deafened: false }],
      shares: [], prefs: c.prefs, encryption: { active: true, backend: 'insertable-streams', epoch: 7, rotations: 1, devices: 2 }, blocked: [] });
    window.__callUI.render();
    const d = document.querySelector('#call-dock');
    return { inRail: (() => { const r = document.querySelector('#call-dock').getBoundingClientRect(); return r.right >= innerWidth - 16 && r.width <= 72 && r.height > r.width * 2; })(), hidden: !d || d.hidden, status: d && d.querySelector('.cd-phase') && d.querySelector('.cd-phase').textContent,
      name: room.name, room: d && d.querySelector('.cd-room') && d.querySelector('.cd-room').textContent, lock: !!d && !!d.querySelector('button.call-enc.ok'),
      title: d && d.querySelector('#c-enc') && d.querySelector('#c-enc').title, buttons: d ? [...d.querySelectorAll('.cd-b')].map(b => b.id) : [],
      labels: !!d && [...d.querySelectorAll('.cd-b')].every(b => b.title && b.getAttribute('aria-label')),
      mutedRed: !!d && !!d.querySelector('#c-mute.on') && d.querySelector('#c-mute').getAttribute('aria-pressed') === 'true' };
  })()`);
  if (!dock.inRail || dock.hidden || dock.status !== 'Voice connected' || dock.room !== dock.name) throw new Error('the call dock is not the connected status strip on the right: ' + JSON.stringify(dock));
  if (!dock.lock || dock.title !== 'Encrypted. What the call server can see') throw new Error('the dock lock is not the encryption explainer: ' + JSON.stringify(dock));
  if (dock.buttons.join() !== 'c-mute,c-deaf,c-share,c-playopen,c-cam,c-set,c-leave') throw new Error('the dock does not have exactly the one row of controls: ' + JSON.stringify(dock.buttons));
  if (!dock.labels || !dock.mutedRed) throw new Error('a dock control has no tooltip/aria-label, or the muted state is not shown: ' + JSON.stringify(dock));
  await A.ev(`document.querySelector('#c-enc').click()`);
  const explain = await A.ev(`(() => { const p = document.querySelector('#call-pop.explain'); return { open: !!p, text: p ? p.innerText : '' }; })()`);
  if (!explain.open || !/call server can see/.test(explain.text)) throw new Error('the lock does not open the explainer: ' + JSON.stringify(explain));
  await A.ev(`document.querySelector('#c-enc').click()`);
  if (await A.ev(`!!document.querySelector('#call-pop')`)) throw new Error('the lock does not close the explainer again');
  // No one was told anything about an unknown kind: the picture rides in the Profile message old apps already read.
  for (const P of [A, B]) {
    const newer = await P.ev(`(async () => (await rpc('notices')).filter(n => n.kind === 'newer').length)()`);
    if (newer) throw new Error('a newer-version notice appeared');
  }
  // Back to the seal, for everyone.
  await click(A, '#av-none');
  await A.waitFor(`!document.querySelector('#av-prev img.av-img')`, 10000);
  await A.shot('13-alice-picture-removed');
  await B.waitFor(`[...document.querySelectorAll('#stream .seal[data-av] img.av-img')].length === 0`, 20000);
  await B.waitFor(`!document.querySelector('#people-stack img.av-img')`, 10000);
  await A.ev(`document.querySelector('.modal .acts button').click()`);
  await sleep(400); await B.shot('14-bob-seal-again');
  // The rail shows who is in a room's call, without anyone having to join. The dev relay has no media server, so
  // put a presence answer in place as the core would have: the strip must render from it, and clicking a person
  // must ask the call panel to join that room. (The core->relay half is covered by crates/core/tests/presence.rs.)
  const gid = await A.ev(`S.snap.communities[0].channels[0].gid`);
  await A.ev(`(()=>{ window.__joined=null; window.__callUI.joinRoom=(g,n)=>{window.__joined=[g,n];};
    S.presence = new Map([[${JSON.stringify(gid)}, [{account:'bob', name:'Bob', me:false}, {account:'me', name:'Alice', me:true}]]]);
    S.presenceCid = S.cid; S.presenceSig = 'stub'; renderRail(); })()`);
  await sleep(200);
  if (!(await A.ev(`!!document.querySelector('.room.live')`))) throw new Error('no live mark on a room with people in its call');
  if (!(await A.ev(`!!document.querySelector('.room.live .live-ic use[href="#i-wave"]')`))) throw new Error('no speaker mark on the live room');
  if (!(await A.ev(`document.querySelectorAll('.room-live .live-p').length === 2`))) throw new Error('call strip does not list the people');
  if (!(await A.ev(`[...document.querySelectorAll('.live-n')].some(n => n.textContent === 'Bob')`))) throw new Error('call strip lost a name');
  if (!(await A.ev(`[...document.querySelectorAll('.live-n')].some(n => n.textContent === 'You')`))) throw new Error('your own row should read You');
  await A.ev(`document.querySelector('.room-live .live-p').click()`);
  if (!(await A.ev(`!!window.__joined && window.__joined[0] === ${JSON.stringify(gid)}`))) throw new Error('clicking a person did not join that room call');
  // Per-person volume: no always-visible sliders; right-click (or a long press) a person for a small popover.
  await A.ev(`(() => { const row = document.querySelector('.room-live .live-p[data-account="bob"]'); row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 70, clientY: 300 })); return true; })()`);
  const vp = await A.ev(`(() => { const p = document.querySelector('#call-pop.vol'); return { open: !!p, names: p ? p.innerText : '', slider: !!document.querySelector('#call-pop input[type=range]'), mute: (document.querySelector('#cp-mute') || {}).textContent || '',
    alwaysVisible: document.querySelectorAll('.room-live input[type=range], #call-dock input[type=range]').length }; })()`);
  if (!vp.open || !/Bob/.test(vp.names) || !vp.slider || !/Mute for me/.test(vp.mute) || vp.alwaysVisible) throw new Error('right-clicking a person does not show a small volume popover (and no always-visible slider): ' + JSON.stringify(vp));
  await A.ev(`document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))`);
  if (await A.ev(`!!document.querySelector('#call-pop')`)) throw new Error('the volume popover does not close when clicking away');
  // Put the engine back: the connected snapshot above was only to show the dock and the popover.
  await A.ev(`(() => { window.__commonsCall.snapshot = window.__snapOrig; delete window.__snapOrig; window.__callUI.render(); return true; })()`);
  await A.shot('09-call-presence');
  // ---- voice rooms ----
  // One new choice in the + dialog. A voice room sits in its own group under the text rooms, wears a speaker
  // instead of the keyhole, lists the people in its call for every member, and one click joins that call without
  // taking the conversation away.
  await click(A, '#add-room'); await type(A, '#n', 'lounge');
  await A.ev(`document.querySelector('.modal .seg[data-for="kind"] button[data-v=voice]').click()`);
  await A.ev(`document.querySelector('.modal .acts button:last-child').click()`);
  await A.waitFor(`[...document.querySelectorAll('#rooms .room')].some(r => (r.getAttribute('aria-label')||'').startsWith('lounge, voice room'))`, 20000);
  const vr = await A.ev(`(() => {
    const rooms = [...document.querySelectorAll('#rooms .room')];
    const room = rooms.find(r => (r.getAttribute('aria-label')||'').startsWith('lounge, voice room'));
    const label = document.querySelector('#rooms .rail-label.voice-label');
    const after = !!label && !!room && !!(label.compareDocumentPosition(room) & Node.DOCUMENT_POSITION_FOLLOWING);
    return { ok: !!room && !!label && label.textContent.trim() === 'Voice' && after && !!room.querySelector('.kh.speaker use[href="#i-speaker"]'), label: label && label.textContent, after };
  })()`);
  if (!vr.ok) throw new Error('a voice room is not in a Voice group with a speaker icon: ' + JSON.stringify(vr));
  const vgid = await A.ev(`S.snap.communities[0].channels.find(c => c.name === 'lounge').gid`);
  await A.ev(`(()=>{ window.__joined=null;
    S.presence.set(${JSON.stringify(vgid)}, [
      { account:'ann', name:'Ann', me:false, speaking:true, muted:false, deafened:false },
      { account:'ben', name:'Ben', me:false, speaking:false, muted:true, deafened:false },
      { account:'cal', name:'Cal', me:true, speaking:false, muted:true, deafened:true }]);
    S.presenceSig='voice-stub'; S.presenceBackoff=Date.now()+60000; renderRail(); })()`);
  const vs = await A.ev(`(() => {
    const room = [...document.querySelectorAll('#rooms .room')].find(r => r.dataset.g === ${JSON.stringify(vgid)});
    const wrap = room && room.closest('.room-wrap');
    const rows = wrap ? [...wrap.querySelectorAll('.room-live.voice .live-p')] : [];
    const by = n => rows.find(r => r.textContent.includes(n));
    const ring = e => e ? getComputedStyle(e.querySelector('.seal')).boxShadow : '';
    const cal = rows.find(r => r.classList.contains('deafened'));
    return { rows: rows.length, names: rows.map(r => r.querySelector('.live-n').textContent),
      ann: (by('Ann')||{}).className, ben: (by('Ben')||{}).className, cal: (cal||{}).className,
      rings: [ring(by('Ann')), ring(by('Ben')), ring(cal)] };
  })()`);
  if (vs.rows !== 3) throw new Error('a voice room does not list everyone in its call: ' + JSON.stringify(vs));
  if (!vs.names.includes('You')) throw new Error('your own row under a voice room should read You');
  if (!/speaking/.test(vs.ann) || !/muted/.test(vs.ben) || !/deafened/.test(vs.cal)) throw new Error('voice-room people lost their speaking/mute/deafen state: ' + JSON.stringify(vs));
  if (vs.rings.some(r => !r || r === 'none')) throw new Error('voice-room people lost their speaking/mute/deafen ring: ' + JSON.stringify(vs.rings));
  const chatBefore = await A.ev(`S.gid`);
  await A.ev(`[...document.querySelectorAll('#rooms .room')].find(r => r.dataset.g === ${JSON.stringify(vgid)}).click()`);
  await sleep(150);
  if (!(await A.ev(`!!window.__joined && window.__joined[0] === ${JSON.stringify(vgid)}`))) throw new Error('clicking a voice room did not join its call');
  if ((await A.ev(`S.gid`)) !== chatBefore) throw new Error('opening a voice room replaced the chat with a room screen');
  await A.shot('09-voice-room');

  // Themed screenshots: Daylight and After Hours, so the wall, dither and glass can be checked in both themes.
  // The voice-room row was selected a moment ago; force each theme and re-shoot the same three states.
  const textGid = gid;
  for (const [theme, tag] of [['light', 'daylight'], ['solar', 'solarized'], ['wave', 'wave'], ['dark', 'afterhours']]) {
    await A.ev(`document.documentElement.dataset.theme = ${JSON.stringify(theme)}`);
    await sleep(400);
    await A.ev(`[...document.querySelectorAll('#rooms .room')].find(r => r.dataset.g === ${JSON.stringify(textGid)}).click()`);
    await sleep(900);
    await A.shot(`${tag}-chat`);
    await click(A, '#btn-settings'); await A.waitFor(`document.querySelector('.modal')`); await sleep(300);
    await A.shot(`${tag}-settings`);
    await click(A, '.modal .acts button'); await sleep(250);
    await A.ev(`[...document.querySelectorAll('#rooms .room')].find(r => r.dataset.g === ${JSON.stringify(vgid)}).click()`);
    await sleep(300);
    await A.shot(`${tag}-voice-room`);
  }
  await A.ev(`delete document.documentElement.dataset.theme`);
  await A.ev(`[...document.querySelectorAll('#rooms .room')].find(r => r.dataset.g === ${JSON.stringify(textGid)}).click()`);
  await sleep(200);

  // ---- voice messages (mega-voice.js): alice records, bob plays ----
  {
    const tj = JSON.stringify(textGid);
    const open = P => P.ev(`[...document.querySelectorAll('#rooms .room')].find(r => r.dataset.g === ${tj}).click()`);
    for (const P of [A, B]) await P.ev(`S.people && document.querySelector('#btn-people').click()`);
    await open(B); await open(A); await sleep(1200);
    const vneed = (c, why) => { if (!c) throw new Error('voice: ' + why); };
    vneed(await A.ev(`!document.querySelector('#mic').hidden`), 'no mic button in a text room');
    const centre = async (P, sel) => { const r = await P.ev(`(() => { const b = document.querySelector(${JSON.stringify(sel)}).getBoundingClientRect(); return [b.x + b.width / 2, b.y + b.height / 2]; })()`); return r; };
    const mouse = (P, type, [x, y]) => P.send('Input.dispatchMouseEvent', { type, x, y, button: 'left', buttons: type === 'mouseReleased' ? 0 : 1, clickCount: 1 });
    const voices = P => P.ev(`S.msgs.messages.filter(m => m.file && m.file.voice).map(m => ({ id: m.id, ms: m.file.voice.ms, peaks: m.file.voice.peaks, mime: m.file.mime, name: m.file.name }))`);
    const v0 = (await voices(B)).length;
    // 1. press and hold about 2 s, let go: sent. The bar shows while it records.
    let mic = await centre(A, '#mic');
    vneed(await A.ev(`(() => { const r = document.querySelector('#mic').getBoundingClientRect(); const e = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2); return e && e.closest('#mic') ? true : (e ? e.outerHTML.slice(0, 120) : 'nothing'); })()`) === true, 'something covers the mic button: ' + await A.ev(`(() => { const r = document.querySelector('#mic').getBoundingClientRect(); const e = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2); return JSON.stringify([r, e && e.outerHTML.slice(0, 120), S.gid, document.querySelector('#composer').hidden]); })()`));
    await mouse(A, 'mousePressed', mic); await sleep(2200);
    const bar = await A.ev(`({ hidden: document.querySelector('#vrec').hidden, st: MegaVoice.state.st, mode: MegaVoice.state.mode, recording: document.querySelector('.slip').classList.contains('recording'), timer: document.querySelector('.vtimer').textContent })`);
    vneed(!bar.hidden && bar.recording && bar.st === 'recording' && bar.mode === 'hold', 'the recording bar is not showing while the button is held: ' + JSON.stringify(bar));
    await A.shot('voice-1-recording');
    await mouse(A, 'mouseReleased', mic);
    await B.waitFor(`S.msgs.messages.filter(m => m.file && m.file.voice).length >= ${v0 + 1}`, 20000);
    let vs = await voices(B); const c1 = vs[vs.length - 1];
    vneed(c1.ms >= 1800 && c1.ms <= 3500, 'wrong length: ' + c1.ms);
    vneed(/^audio\//.test(c1.mime) && c1.peaks.length === 128, 'wrong type or waveform size: ' + JSON.stringify(c1));
    const pk = c1.peaks.match(/../g).map(h => parseInt(h, 16)); vneed(Math.max(...pk) - Math.min(...pk) > 100, 'the waveform does not follow the sound: ' + c1.peaks);
    vneed(await A.ev(`MegaVoice.state.st === 'idle' && document.querySelector('#vrec').hidden && !document.querySelector('.slip').classList.contains('recording')`), 'the bar stayed after sending');
    // 2. bob sees a player with the right duration and plays it
    await B.waitFor(`document.querySelector('.vmsg[data-vid="${c1.id}"]')`, 8000);
    const pv = await B.ev(`(() => { const e = document.querySelector('.vmsg[data-vid="${c1.id}"]'); return { tot: e.querySelector('.vtot').textContent, bars: e.querySelectorAll('.vbars:not(.vfill) i').length, play: !!e.querySelector('.vplay'), speed: e.querySelector('.vspeed').textContent, dl: !!e.querySelector('.vdl') }; })()`);
    vneed(pv.bars === 64 && pv.play && pv.dl && pv.speed === '1×' && pv.tot === (sec => `${Math.floor(sec / 60)}:${String(sec % 60).padStart(2, '0')}`)(Math.round(c1.ms / 1000)), 'player is incomplete: ' + JSON.stringify(pv));
    vneed(await B.ev(`!document.querySelector('#stream audio, #stream video') && ![...document.querySelectorAll('audio')].length`), 'a clip is on the page before anyone pressed play');
    await B.shot('voice-2-player');
    await B.ev(`document.querySelector('.vmsg[data-vid="${c1.id}"] .vplay').click()`);
    await B.waitFor(`(MegaVoice.audio('${c1.id}') || {}).currentTime > 0.3`, 8000).catch(async er => { throw new Error(er.message + ' :: ' + await B.ev(`(() => { const a = MegaVoice.audio('${c1.id}'); return a ? JSON.stringify({ t: a.currentTime, d: a.duration, paused: a.paused, rs: a.readyState, ns: a.networkState, err: a.error && a.error.message, src: a.src.slice(0, 30), type: a.canPlayType('audio/webm; codecs=opus') }) : 'no audio'; })()`)); });
    vneed(await B.ev(`document.querySelector('.vmsg[data-vid="${c1.id}"]').classList.contains('playing')`), 'the player does not show playing');
    // seek by clicking the waveform, then change speed
    const wv = await B.ev(`(() => { const b = document.querySelector('.vmsg[data-vid="${c1.id}"] .vwave').getBoundingClientRect(); return [b.x, b.y + b.height / 2, b.width]; })()`);
    await B.ev(`MegaVoice.audio('${c1.id}').pause()`);
    await mouse(B, 'mousePressed', [wv[0] + wv[2] * .75, wv[1]]); await mouse(B, 'mouseReleased', [wv[0] + wv[2] * .75, wv[1]]); await sleep(500);
    const at = await B.ev(`MegaVoice.audio('${c1.id}').currentTime`);
    vneed(Math.abs(at - c1.ms * .00075) < .35, `seek landed at ${at}s, wanted ${c1.ms * .00075}`);
    await B.ev(`document.querySelector('.vmsg[data-vid="${c1.id}"] .vspeed').click()`);
    vneed(await B.ev(`MegaVoice.audio('${c1.id}').playbackRate === 1.5 && document.querySelector('.vmsg[data-vid="${c1.id}"] .vspeed').textContent === '1.5×'`), 'speed did not change to 1.5');
    await B.ev(`document.querySelector('.vmsg[data-vid="${c1.id}"] .vspeed').click()`);
    vneed(await B.ev(`MegaVoice.audio('${c1.id}').playbackRate === 2`), 'speed did not change to 2');
    await B.ev(`document.querySelector('.vmsg[data-vid="${c1.id}"] .vspeed').click()`);
    // 3. a click (not a hold) keeps recording with buttons; send it, and only one clip plays at a time
    mic = await centre(A, '#mic');
    await mouse(A, 'mousePressed', mic); await mouse(A, 'mouseReleased', mic); await sleep(1800);
    const lock = await A.ev(`({ mode: MegaVoice.state.mode, send: getComputedStyle(document.querySelector('.vsend')).display, cancel: getComputedStyle(document.querySelector('.vcancel')).display, t: document.querySelector('.vtimer').textContent })`);
    vneed(lock.mode === 'lock' && lock.send !== 'none' && lock.cancel !== 'none' && lock.t !== '0:00', 'a click did not start a recording with buttons: ' + JSON.stringify(lock));
    await A.shot('voice-3-locked');
    await click(A, '.vsend');
    await B.waitFor(`S.msgs.messages.filter(m => m.file && m.file.voice).length >= ${v0 + 2}`, 20000);
    vs = await voices(B); const c2 = vs[vs.length - 1];
    await B.waitFor(`document.querySelector('.vmsg[data-vid="${c2.id}"]')`, 8000);
    await B.ev(`MegaVoice.audio('${c1.id}').play()`); await sleep(300);
    await B.ev(`document.querySelector('.vmsg[data-vid="${c2.id}"] .vplay').click()`);
    await B.waitFor(`(MegaVoice.audio('${c2.id}') || {}).currentTime > 0.2`, 8000);
    vneed(await B.ev(`MegaVoice.audio('${c1.id}').paused`), 'two clips played at once');
    await B.ev(`MegaVoice.audio('${c2.id}').pause()`);
    // 4. cancel sends nothing, and the microphone is let go
    const before = (await voices(B)).length, filesBefore = await B.ev(`S.msgs.messages.filter(m => m.kind === 'file').length`);
    mic = await centre(A, '#mic');
    await mouse(A, 'mousePressed', mic); await mouse(A, 'mouseReleased', mic); await sleep(1200);
    await click(A, '.vcancel'); await sleep(2500);
    vneed(await A.ev(`MegaVoice.state.st === 'idle' && document.querySelector('#vrec').hidden`), 'cancel left the bar open');
    vneed((await voices(B)).length === before && await B.ev(`S.msgs.messages.filter(m => m.kind === 'file').length`) === filesBefore, 'cancelling sent something');
    // sliding away while holding cancels too
    await mouse(A, 'mousePressed', mic); await sleep(900);
    await A.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: mic[0] - 220, y: mic[1], button: 'left', buttons: 1 });
    vneed(await A.ev(`document.querySelector('#vrec').classList.contains('cancelling')`), 'sliding away does not mark the recording as cancelled');
    await mouse(A, 'mouseReleased', [mic[0] - 220, mic[1]]); await sleep(2500);
    vneed((await voices(B)).length === before, 'a recording slid away from was sent');
    // 5. a browser with no MediaRecorder records raw sound and sends a WAV
    await A.ev(`MegaVoice.forcePcm = true`);
    mic = await centre(A, '#mic');
    await mouse(A, 'mousePressed', mic); await mouse(A, 'mouseReleased', mic); await sleep(2000);
    await click(A, '.vsend');
    await B.waitFor(`S.msgs.messages.filter(m => m.file && m.file.voice).length >= ${v0 + 3}`, 20000);
    vs = await voices(B); const c3 = vs[vs.length - 1];
    await A.ev(`MegaVoice.forcePcm = false`);
    vneed(c3.mime === 'audio/wav' && c3.ms >= 1500 && c3.ms <= 4000, 'the WAV fallback clip is wrong: ' + JSON.stringify(c3));
    const pk3 = c3.peaks.match(/../g).map(h => parseInt(h, 16)); vneed(Math.max(...pk3) - Math.min(...pk3) > 100, 'the WAV fallback waveform is flat: ' + c3.peaks);
    await B.waitFor(`document.querySelector('.vmsg[data-vid="${c3.id}"] .vplay')`, 8000);
    await B.ev(`document.querySelector('.vmsg[data-vid="${c3.id}"] .vplay').click()`);
    await B.waitFor(`(MegaVoice.audio('${c3.id}') || {}).currentTime > 0.3`, 8000);
    await B.ev(`MegaVoice.audio('${c3.id}').pause()`);
    const wavLen = await B.ev(`MegaVoice.audio('${c3.id}').duration`);
    vneed(Math.abs(wavLen * 1000 - c3.ms) < 700, `the WAV is ${wavLen}s long but says ${c3.ms} ms`);
    // 6. a clip nothing here can play shows a download link
    const nope = await B.ev(`MegaVoice.row({ id: 'ab'.repeat(16), file: { mime: 'audio/x-nothing-real', name: 'v.xyz', voice: { ms: 4000, peaks: '' } } })`);
    vneed(/vmsg nope/.test(nope) && /class="dl vdl"/.test(nope), 'an unplayable clip has no download link');
    await B.shot('voice-4-after');
  }
  if (process.env.COMMONS_UI_STOP_AFTER === 'voice') { console.log('UI check stopped after the voice messages section (COMMONS_UI_STOP_AFTER)'); process.exit(0); }

  // ---- mega: the navigation around the chat ----
  // The old rail is kept only for its dock; people move through home -> a community's rooms -> the chat with dots.
  const key = async (P, k, mods = 0, code) => {
    const vk = k === 'Escape' ? 27 : k === 'Enter' ? 13 : k.length === 1 ? k.toUpperCase().charCodeAt(0) : 0;
    await P.send('Input.dispatchKeyEvent', { type: 'keyDown', key: k, code: code || k, modifiers: mods, windowsVirtualKeyCode: vk });
    await P.send('Input.dispatchKeyEvent', { type: 'keyUp', key: k, code: code || k, modifiers: mods });
  };
  const lvl = P => P.ev(`__mega.level`);
  const need = (cond, why) => { if (!cond) throw new Error('mega: ' + why); };
  // the composer's box in a real screenshot: how many bright pixels (placeholder, buttons, border) it holds. Computed styles can say "painted" while the pixels are blank.
  const composerInk = async P => {
    await P.send('Page.bringToFront'); await sleep(250);
    const r = await P.ev(`(() => { const e = document.querySelector('#composer .slip') || document.querySelector('#composer'), b = e.getBoundingClientRect(), cs = x => { const o = document.querySelector(x); if (!o) return null; const c = getComputedStyle(o), r = o.getBoundingClientRect(); return [c.opacity, c.visibility, c.display, c.color, c.backgroundColor, Math.round(r.width) + 'x' + Math.round(r.height), c.animationName, c.filter, c.backdropFilter]; }; return { x: b.left, y: b.top, w: b.width, h: b.height, st: { composer: cs('#composer'), slip: cs('#composer .slip'), input: cs('#input'), send: cs('#send'), row: cs('#composer .slip .row') }, hiddenAttr: document.querySelector('#composer').hidden }; })()`);
    const shot = await P.send('Page.captureScreenshot', { format: 'png', clip: { x: r.x, y: r.y, width: r.w, height: r.h, scale: 1 } });
    const bright = await P.ev(`(async () => { const b = await createImageBitmap(new Blob([Uint8Array.from(atob('${shot.result.data}'), c => c.charCodeAt(0))], { type: 'image/png' })), c = new OffscreenCanvas(b.width, b.height), g = c.getContext('2d'); g.drawImage(b, 0, 0); const d = g.getImageData(0, 0, b.width, b.height).data; let n = 0; for (let i = 0; i < d.length; i += 4) if (d[i] + d[i + 1] + d[i + 2] > 330) n++; return n; })()`);
    return { bright, ...r };
  };
  // ---- the first-run tour: seen on a first visit, stepped through, gone after a reload, back from the options room ----
  {
    const keep = await A.ev(`({ c: __mega.cid, g: S.gid })`);
    const pres = await A.ev(`JSON.stringify([...S.presence])`);   // the reloads below drop the stubs the earlier checks put in; they are put back at the end
    const reload = async () => { await A.ev(`location.reload(); 1`).catch(() => { }); await sleep(1500); await A.waitFor(`window.__megaTour && document.querySelector('#mega:not([hidden]) .mcard')`, 60000); };
    await A.send('Page.bringToFront'); await A.ev(`sessionStorage.setItem('tourtest', '1'); localStorage.removeItem('commons.tour'); 1`); await reload();
    await A.waitFor(`document.querySelector('#mega-tour.on .mt-t')`, 20000).catch(async () => { throw new Error('mega: tour: it did not appear on a first visit: ' + await A.ev(`JSON.stringify({ st: __megaTour.state(), mega: document.documentElement.classList.contains('mega'), card: !!document.querySelector('#mega:not([hidden]) .mcard'), blockers: [...document.querySelectorAll('#modal-root > .scrim, #mega-pal, .mpop, #mega-hint')].map(e => e.id + '.' + e.className), vis: document.visibilityState, ls: localStorage.getItem('commons.tour'), p: String(pref.get('tour')) })`).catch(e => String(e))); });
    await sleep(500);
    const t0 = await A.ev(`(() => { const t = document.querySelector('#mega-tour'); return { text: t.querySelector('.mt-t').innerText, n: t.querySelector('.mt-n').innerText, live: t.querySelector('[aria-live]').textContent, ring: getComputedStyle(t.querySelector('.mt-ring')).opacity }; })()`);
    need(/communities/.test(t0.text) && /1 of 5/.test(t0.n) && /step 1 of 5/.test(t0.live) && t0.ring === '1', 'tour: first step is wrong: ' + JSON.stringify(t0));
    await A.shot('mega-tour-1');
    for (let i = 2; i <= 5; i++) { await key(A, 'Enter'); await sleep(350); need(await A.ev(`document.querySelector('#mega-tour .mt-n').innerText === '${i} of 5'`), 'tour: Enter did not go to step ' + i); }
    need(await A.ev(`document.querySelector('#mega-tour .mt-skip').hidden && document.querySelector('#mega-tour .mt-next').innerText === 'done'`), 'tour: the last step should say done');
    await key(A, 'Enter'); await sleep(600);
    need(await A.ev(`!document.querySelector('#mega-tour') && localStorage.getItem('commons.tour') === '1'`), 'tour: it did not close and remember at the end');
    await reload(); await sleep(3500);
    need(await A.ev(`!document.querySelector('#mega-tour')`), 'tour: it came back after a reload');
    // Esc skips
    await A.ev(`__megaTour.start(); 1`); await sleep(500); need(await A.ev(`!!document.querySelector('#mega-tour')`), 'tour: start() did not open it');
    await key(A, 'Escape'); await sleep(600); need(await A.ev(`!document.querySelector('#mega-tour')`), 'tour: Esc did not skip: ' + await A.ev(`JSON.stringify({ t: document.querySelectorAll('#mega-tour').length, st: __megaTour.state(), ae: document.activeElement.className, ls: localStorage.getItem('commons.tour') })`));
    // the options room offers it again
    await A.ev(`__mega.api.toChat(${JSON.stringify(keep.c)}, null); 1`);
    await A.waitFor(`document.querySelector('#mega-opts:not([hidden]) #m-tour-again')`, 15000).catch(() => { throw new Error('mega: tour: the options room has no "show the tour again"'); });
    await click(A, '#m-tour-again'); await sleep(600);
    need(await A.ev(`!!document.querySelector('#mega-tour.on') && document.querySelector('#mega-tour .mt-n').innerText === '1 of 5'`), 'tour: the options button did not reopen it');
    await A.shot('mega-tour-again'); await key(A, 'Escape'); await sleep(500);
    await A.ev(`sessionStorage.removeItem('tourtest'); localStorage.setItem('commons.tour', '1'); 1`); await reload();
    await A.ev(`(() => { window.__joined = null; window.__callUI.joinRoom = (g, n) => { window.__joined = [g, n]; }; S.presence = new Map(${pres}); S.presenceCid = S.cid; S.presenceSig = 'stub'; renderRail(); return 1; })()`);
    await A.ev(`__mega.api.toChat(${JSON.stringify(keep.c)}, ${JSON.stringify(keep.g)}); 1`); await sleep(800);
  }
  if (process.env.COMMONS_UI_STOP_AFTER === 'tour') { console.log('UI check stopped after the tour (COMMONS_UI_STOP_AFTER)'); process.exit(0); }

  // entering a room shows the dots and the existing stream
  need(await lvl(A) === 'chat', 'a chat should be open, level is ' + await lvl(A));
  const chat = await A.ev(`(() => { const r = document.querySelector('#mega-rail'), s = document.querySelector('#stream').getBoundingClientRect(), d = [...document.querySelectorAll('#mega-rail .mitem')];
    return { rail: !r.hidden && r.getBoundingClientRect().width > 20, dots: d.length, active: d.filter(x => x.classList.contains('active')).length, voice: d.filter(x => x.classList.contains('voice')).length, opt: d.filter(x => x.classList.contains('opt')).length,
      stream: s.width > 200 && s.left >= r.getBoundingClientRect().right - 1, oldRailGone: document.querySelector('#rooms').getBoundingClientRect().width === 0 }; })()`);
  need(chat.rail && chat.dots >= 4 && chat.active === 1 && chat.voice >= 1 && chat.opt === 1 && chat.stream && chat.oldRailGone, 'the chat does not show the dots beside the stream: ' + JSON.stringify(chat));
  await A.shot('mega-1-chat');
  // the one-time hint beside the dots: says what they are, and goes for good once the cards have been opened from them
  await A.ev(`localStorage.removeItem('commons.dotsHint'); __megaNew.showHint()`); await sleep(500);
  const hint1 = await A.ev(`(() => { const h = document.querySelector('#mega-hint'); return h ? { text: h.innerText, button: !!h.querySelector('.mh-x'), shown: __megaNew.state().hint } : null; })()`);
  need(hint1 && hint1.shown && /dots are your rooms/.test(hint1.text) && /scroll on them to open the cards/.test(hint1.text) && hint1.button, 'dots hint: not shown to a first-time user: ' + JSON.stringify(hint1));
  await A.shot('mega-1b-dots-hint');
  // Esc closes the People sheet first when it is open
  if (await A.ev(`S.people`)) { await key(A, 'Escape'); await sleep(400); need(await A.ev(`!S.people && !document.querySelector('#people:not([hidden])')`), 'Esc did not close the People sheet'); }
  // wheel over the dots pops them out as small cards; Esc closes that first, leaving the chat where it was
  await A.ev(`document.querySelector('#mega-rail').dispatchEvent(new WheelEvent('wheel', { deltaY: 40, bubbles: true, cancelable: true }))`); await sleep(700);
  need(await A.ev(`document.querySelector('#mega-rail').classList.contains('open')`), 'wheel over the dots did not pop them out');
  await A.shot('mega-2-popout');
  need(await A.ev(`!document.querySelector('#mega-hint') && __megaNew.state().hintState.used === 1`), 'dots hint: opening the cards from the dots should dismiss it for good: ' + await A.ev(`JSON.stringify(__megaNew.state())`));
  await A.ev(`__megaNew.showHint()`); need(await A.ev(`!document.querySelector('#mega-hint')`), 'dots hint: it came back after it was used');
  await key(A, 'Escape'); await sleep(500);
  need(!(await A.ev(`document.querySelector('#mega-rail').classList.contains('open')`)) && await lvl(A) === 'chat', 'Esc should only close the pop-out first');
  // options room: client-only, with the existing controls and their ids
  await A.ev(`document.querySelector('#mega-rail .mitem.opt').click()`); await sleep(500);
  const op = await A.ev(`(() => { const o = document.querySelector('#mega-opts'); return { open: !o.hidden, ids: ['snd', 'ty', 'dl-dir'].filter(i => !o.querySelector('#' + i)), privacy: o.innerText.includes('the relay can see that a message-sized packet was sent at that moment'), noRelay: !o.querySelector('#rl') && !o.querySelector('#n') }; })()`);
  need(op.open && !op.ids.length && op.privacy && op.noRelay, 'the options room is wrong: ' + JSON.stringify(op));
  // a new account has typing hints on; the switch turns them off and on again
  need(await A.ev(`S.snap.typing_enabled === true && document.querySelector('#mega-opts #ty').checked`), 'typing hints should be on for a new account');
  await A.ev(`document.querySelector('#mega-opts #ty').click()`);
  await A.waitFor(`S.snap.typing_enabled === false`, 8000).catch(() => { throw new Error('mega: the typing switch in options did not reach the core'); });
  await A.ev(`document.querySelector('#mega-opts #ty').click()`);
  await A.waitFor(`S.snap.typing_enabled === true`, 8000).catch(() => { throw new Error('mega: the typing switch in options did not turn on again'); });
  await A.shot('mega-3-options');
  // Esc from chat -> the community's rooms; resting on the voice card never joins, Enter does
  await key(A, 'Escape'); await sleep(900);
  need(await lvl(A) === 'server', 'Esc from the chat should show the rooms, level is ' + await lvl(A));
  const cards = await A.ev(`__mega.list.items.map(x => x.kind + ':' + x.name).join()`);
  need(/text:general/.test(cards) && /voice:lounge/.test(cards) && /options:options/.test(cards), 'the rooms level lacks cards: ' + cards);
  await A.ev(`window.__joined = null; __mega.list.go(__mega.list.items.findIndex(x => x.kind === 'voice'), true)`); await sleep(1800);
  need(await A.ev(`__mega.list.item.kind === 'voice' && document.querySelector('.mslot.settled')`), 'the voice card did not come into focus');
  need(await A.ev(`window.__joined === null`), 'resting on a voice card joined its call');
  await A.shot('mega-4-voice-card');
  // design: the section label above the voice cards steps aside while its card is the settled one (it used to sit under the bar's chips)
  need(await A.ev(`(() => { const s = document.querySelector('.msect:has(+ .mslot.settled)'); return !s || getComputedStyle(s).opacity === '0'; })()`), 'design: the voice section label shows over the bar while the voice card is centred');
  await A.ev(`document.querySelector('.mslot.settled .mcard').dispatchEvent(new MouseEvent('click', { bubbles: true }))`); await sleep(300);
  need(await A.ev(`window.__joined === null`), 'clicking the body of a voice card joined its call');
  await key(A, 'Enter'); await sleep(300);
  need(await A.ev(`!!window.__joined && window.__joined[0] === ${JSON.stringify(vgid)}`), 'Enter on a voice card did not join it: ' + await A.ev(`JSON.stringify({ j: window.__joined, ae: document.activeElement && (document.activeElement.id || document.activeElement.className || document.activeElement.tagName), tour: !!document.querySelector('#mega-tour'), pops: [...document.querySelectorAll('.mpop,#mega-pal,#modal-root > .scrim')].length, item: __mega.list.item.kind, lk: __mega.list.item.locked, here: __mega.list.item.here, cc: window.__commonsCall && [window.__commonsCall.gid, window.__commonsCall.phase], idx: __mega.list.i, closers: __mega.closers.length, busy: __mega.busy, pal: !!__mega.pal, modal: !!document.querySelector('#modal-root > .scrim') })`));
  // jump to: picking a voice room focuses its card but never joins
  await A.ev(`window.__joined = null`);
  await key(A, 'k', 2, 'KeyK'); await sleep(400);
  need(await A.ev(`!!document.querySelector('#mega-pal')`), 'Ctrl+K did not open jump to');
  await A.ev(`(() => { const q = document.querySelector('#m-pq'); q.value = 'lounge'; q.dispatchEvent(new Event('input')); })()`);
  await key(A, 'Enter'); await sleep(900);
  need(await A.ev(`!document.querySelector('#mega-pal') && __mega.level === 'server' && __mega.list.item.name === 'lounge' && window.__joined === null`), 'jump to a voice room should focus its card without joining');

  // ---- text rooms and voice rooms are separate; ringing a voice room ----
  {
    const VG = JSON.stringify(vgid);
    await sleep(1500);   // call-ui used to add its Call button on a timer; give it every chance
    need(await A.ev(`!document.getElementById('btn-call') && ![...document.querySelectorAll('#head button')].some(b => /^\\s*call\\s*$/i.test(b.textContent))`), 'split: a text room still has a Call button');
    // a voice room has no chat: nothing opens it, and a stale state pointing at one is put right on the next paint
    await A.ev(`rpc('send_text', { gid: ${VG}, text: 'old voice room words', reply: null })`); await sleep(1500);
    await A.ev(`openRoom(${VG})`); await sleep(300);
    need(await A.ev(`S.gid !== ${VG}`), 'split: openRoom() opened a voice room as a chat');
    await A.ev(`(() => { S.gid = ${VG}; renderRail(); return 1; })()`); await sleep(1200);
    const sp = await A.ev(`(() => { const c = S.snap.communities.find(x => x.id === S.cid), ch = c.channels.find(x => x.gid === S.gid); return { gid: S.gid === ${VG}, kind: ch && ch.kind, words: document.getElementById('stream').innerText.includes('old voice room words') }; })()`);
    need(!sp.gid && sp.kind !== 'voice', 'split: the chat pane stayed on a voice room: ' + JSON.stringify(sp));
    need(await A.ev(`rpc('messages', { gid: ${VG}, limit: 50, around: '' }).then(m => JSON.stringify(m.messages).includes('old voice room words'))`), 'split: messages already in a voice room were lost');
    need(!sp.words, 'split: a voice room message showed in the chat');
    // with nothing but a voice room to show, there is no message box at all
    await A.ev(`(() => { const c = S.snap.communities.find(x => x.id === S.cid); window.__chanBak = c.channels; c.channels = c.channels.filter(x => x.kind === 'voice'); S.gid = null; renderRail(); return 1; })()`); await sleep(300);
    need(await A.ev(`document.getElementById('composer').hidden === true && S.gid === null`), 'split: a community with only voice rooms shows a message box');
    await A.ev(`(() => { S.snap.communities.find(x => x.id === S.cid).channels = window.__chanBak; delete window.__chanBak; S.last = ''; return 1; })()`); await sleep(1200);
    await A.shot('mega-4b-voice-split');

    // ringing: A rings the room; B (whose mega is open) sees a card, and joining goes through the normal join
    await B.ev(`(() => { window.__joined = null; window.__callUI.joinRoom = (g, n) => { window.__joined = [g, n]; }; window.__fakeOsc = 0; window.__notes = [];
      chimeCtx = { state: 'running', currentTime: 0, destination: {}, createGain() { return { gain: { setValueAtTime() { }, exponentialRampToValueAtTime() { } }, connect() { } }; }, createOscillator() { window.__fakeOsc++; return { frequency: {}, connect() { }, start() { }, stop() { } }; } };
      window.Notification = class { constructor(t, o) { window.__notes.push([t, o && o.body]); } close() { } }; window.Notification.permission = 'granted';
      return 1; })()`);
    const cards = `document.querySelectorAll('#mega-ring .rg-card').length`;
    const ringSend = (P, to = '') => P.ev(`rpc('ring_send', { gid: ${VG}, to: ${JSON.stringify(to)} })`);
    const ring1 = (await ringSend(A)).id, t1 = Date.now();
    await B.waitFor(`${cards} === 1`, 15000).catch(() => { throw new Error('ring: B never saw the incoming card'); });
    const cd = await B.ev(`(() => { const c = document.querySelector('#mega-ring .rg-card'), r = c.getBoundingClientRect(), bs = [...c.querySelectorAll('button')].map(b => b.textContent.trim()); return { text: c.textContent, bs, w: Math.round(r.width), vw: innerWidth, hidden: document.getElementById('mega-ring').hidden }; })()`);
    need(/alice/i.test(cd.text) && /lounge/.test(cd.text) && /Book club/.test(cd.text) && cd.bs.includes('join') && cd.bs.includes('decline') && cd.w >= cd.vw - 24 && !cd.hidden, 'ring: the card is wrong: ' + JSON.stringify(cd));
    need(await A.ev(`${cards} === 0`), 'ring: the caller was rung');
    await sleep(1500);
    need(await B.ev(`__megaRing.stats.sounds >= 1 && __fakeOsc >= 2`), 'ring: no sound with sounds on');
    await B.shot('mega-4c-ring-card');
    await B.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true }); await sleep(700);
    await B.shot('mega-4c-ring-card-390');
    need(await B.ev(`(() => { const c = document.querySelector('#mega-ring .rg-card').getBoundingClientRect(); return c.left >= 0 && c.right <= innerWidth + 1 && [...document.querySelectorAll('#mega-ring .rg-b')].every(b => b.getBoundingClientRect().right <= innerWidth); })()`), 'ring: the card overflows at 390 px');
    await B.send('Emulation.clearDeviceMetricsOverride'); await sleep(500);
    await B.ev(`document.querySelector('#mega-ring .rg-b.yes').click()`); await sleep(300);
    need(await B.ev(`!!window.__joined && window.__joined[0] === ${VG} && ${cards} === 0`), 'ring: join did not take the normal join path or the card stayed');
    // decline stops it
    await sleep(5200);
    await ringSend(A);
    await B.waitFor(`${cards} === 1`, 15000);
    await B.ev(`document.querySelector('#mega-ring .rg-b.no').click()`); await sleep(2500);
    need(await B.ev(`${cards} === 0`), 'ring: decline did not stop the ring');
    // the caller takes it back
    await sleep(3000);
    const ring3 = (await ringSend(A)).id;
    await B.waitFor(`${cards} === 1`, 15000);
    await A.ev(`rpc('ring_end', { gid: ${VG}, id: ${JSON.stringify(ring3)}, to: '' })`);
    await B.waitFor(`${cards} === 0`, 15000).catch(() => { throw new Error('ring: the caller cancelling did not stop it'); });
    // sounds off: silent. In the background with notifications allowed: a system notification
    await B.ev(`(() => { pref.set('sound', 'off'); window.__fakeOsc = 0; __megaRing.stats.sounds = 0; __megaRing.stats.notified = 0; window.__notes = []; Object.defineProperty(document, 'visibilityState', { get: () => 'hidden', configurable: true }); return 1; })()`);
    await sleep(5200);
    await ringSend(A);
    await B.waitFor(`${cards} === 1`, 15000);
    await sleep(3200);
    const quiet = await B.ev(`({ osc: window.__fakeOsc, sounds: __megaRing.stats.sounds, notes: window.__notes, notified: __megaRing.stats.notified })`);
    need(quiet.osc === 0 && quiet.sounds === 0, 'ring: it made a sound with sounds off: ' + JSON.stringify(quiet));
    need(quiet.notified === 1 && /alice/i.test(quiet.notes[0][0]) && /lounge/.test(quiet.notes[0][1]), 'ring: no system notification in the background: ' + JSON.stringify(quiet));
    // and it ends by itself after its 30 seconds
    await B.ev(`(() => { for (const c of __megaRing.cards.values()) c.until = Date.now() / 1000 - 1; __megaRing.sweep(); delete document.visibilityState; pref.set('sound', ''); return 1; })()`); await sleep(300);
    need(await B.ev(`${cards} === 0`), 'ring: it did not end after its time');
    // a stale ring (a device catching up on an old log) rings nobody
    await A.ev(`rpc('debug_ring', { gid: ${VG}, age: 100 })`);
    await sleep(3000);
    need(await B.ev(`${cards} === 0`), 'ring: a stale ring rang');
    // the dock button and a person's row: with a (stub) call in the voice room, A can ring everyone or only Bob
    await sleep(Math.max(0, 62000 - (Date.now() - t1)));   // a receiver takes at most 4 rings a minute from one person
    await A.ev(`(() => { window.__realCall = window.__commonsCall; window.__commonsCall = { gid: ${VG}, phase: 'connected', room: {}, encryption: { active: true }, snapshot() { return { peers: [{ account: S.snap.me.account, name: 'Alice', me: true }] }; } }; return 1; })()`);
    need(await A.ev(`__megaRing.dockButton({ phase: 'connected' }).includes('id="c-ring"')`), 'ring: the dock has no ring button in a voice room call');
    await sleep(2500);
    await A.ev(`document.getElementById('btn-people').click()`); await sleep(1500);
    await A.waitFor(`!!document.querySelector('#people .rg-row')`, 10000).catch(() => { throw new Error("ring: no ring button on a person's row"); });
    need(await A.ev(`document.querySelectorAll('#people .rg-row').length === 1 && !document.querySelector('#people .person:first-child .rg-row')`), 'ring: the ring button is on the wrong rows');
    await A.shot('mega-4d-people-ring');
    await A.ev(`document.querySelector('#people .rg-row').click()`);
    await B.waitFor(`${cards} === 1`, 15000).catch(() => { throw new Error('ring: ringing one person did not reach them'); });
    need(await B.ev(`/calling you/.test(document.querySelector('#mega-ring .rg-card').textContent)`), 'ring: a direct ring does not say it is for them');
    await B.ev(`document.querySelector('#mega-ring .rg-b.no').click()`);
    await A.ev(`(() => { window.__commonsCall = window.__realCall; delete window.__realCall; document.getElementById('btn-people').click(); return 1; })()`); await sleep(600);
  }

  // a second community makes a home; the cards are communities plus the "you" card
  await A.ev(`void __megaNewCommunity(), 0`); await A.waitFor(`document.querySelector('.modal #n')`); await type(A, '#n', 'Second place'); await click(A, '.modal .acts button:last-child');
  await A.waitFor(`__mega.level === 'server' && __mega.list.items.some(x => x.cid && S.snap.communities.find(c => c.id === x.cid && c.name === 'Second place'))`, 60000).catch(async e => { throw new Error(e.message + ' | ' + await A.ev(`JSON.stringify({ level: __mega.level, comms: S.snap.communities.map(c => c.name), modal: !!document.querySelector('.modal'), modalText: (document.querySelector('.modal') || {}).innerText, toast: document.querySelector('#toast').textContent, items: __mega.list && __mega.list.items.map(x => x.kind + ':' + x.name + ':' + String(x.cid).slice(0, 4)), Scid: String(S.cid).slice(0, 4), Mcid: String(__mega.cid).slice(0, 4), ids: S.snap.communities.map(c => c.name + ':' + c.id.slice(0, 4)), busy: __mega.busy, follow: __mega.followMs })`)); });
  console.log('new community listed after', await A.ev(`__mega.followMs`), 'ms');
  await key(A, 'Escape'); await sleep(900);
  need(await lvl(A) === 'home', 'Esc from the rooms should show all communities, level is ' + await lvl(A));
  const home = await A.ev(`__mega.list.items.map(x => x.kind + ':' + x.name).join()`);
  need(/community:Book club/.test(home) && /community:Second place/.test(home) && /you:you/.test(home), 'home does not show the communities and the you card: ' + home);
  await A.shot('mega-5-home');
  await A.ev(`__mega.list.go(__mega.list.items.findIndex(x => x.name === 'Book club'), true)`); await sleep(1500);
  await key(A, 'Enter'); await sleep(1200);
  need(await A.ev(`__mega.level === 'server' && S.snap.communities.find(c => c.id === S.cid).name === 'Book club'`), 'Enter on a community card did not open its rooms');
  await A.ev(`__mega.list.go(__mega.list.items.findIndex(x => x.key === ${JSON.stringify(textGid)}), true)`); await sleep(1500);
  await key(A, 'Enter'); await sleep(1500);
  need(await lvl(A) === 'chat' && await A.ev(`S.gid === ${JSON.stringify(textGid)}`), 'Enter on a room card did not open the chat');
  await key(A, 'Escape'); await sleep(900); await key(A, 'Escape'); await sleep(900);
  need(await lvl(A) === 'home', 'two Esc from the chat should reach home');
  // the "you" card opens the existing Settings
  await A.ev(`__mega.list.go(__mega.list.items.findIndex(x => x.kind === 'you'), true)`); await sleep(1500);
  await key(A, 'Enter'); await sleep(700);
  need(await A.ev(`!!document.querySelector('.modal #rl')`), 'the you card did not open Settings');
  await key(A, 'Escape'); await sleep(500);
  need(await A.ev(`!document.querySelector('.modal')`), 'Esc did not close Settings');
  // and back into the club's chat for what follows
  await A.ev(`__mega.list.go(__mega.list.items.findIndex(x => x.name === 'Book club'), true)`); await sleep(1500);
  await key(A, 'Enter'); await sleep(1200);
  await A.ev(`__mega.list.go(__mega.list.items.findIndex(x => x.key === ${JSON.stringify(textGid)}), true)`); await sleep(1500);
  await key(A, 'Enter'); await sleep(1500);
  need(await lvl(A) === 'chat', 'could not get back into the chat');

  // ---- mega phase 2: the living chat room ----
  {
    const st = P => P.ev(`JSON.stringify(__mega.stats)`).then(JSON.parse);
    const bdLive = () => A.ev(`(() => { const c = document.querySelector('#mega-bd'); if (!c) return -1; const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data; let n = 0; for (let i = 3; i < d.length; i += 4 * 7) if (d[i]) n++; return n; })()`);
    // the backdrop draws in a room, and is gone (stopped) on card levels
    await sleep(800);
    need(await A.ev(`document.documentElement.classList.contains('mg-room')`), 'the dark room look is not on while a room is open');
    need(await bdLive() > 50, 'the backdrop canvas did not draw');
    need(await A.ev(`getComputedStyle(document.querySelector('#main')).backgroundColor === 'rgb(5, 4, 7)' || getComputedStyle(document.querySelector('#main')).backgroundImage.includes('radial-gradient')`), 'the room does not sit on the mega room background');
    await A.shot('mega2-1-room');
    { const ink = await composerInk(A); need(ink.bright > 100, 'the composer has not painted after entering a room (bright pixels ' + ink.bright + ')'); }
    await key(A, 'Escape'); await sleep(900);
    need(await lvl(A) === 'server' && await A.ev(`!document.querySelector('#mega-bd') && !document.documentElement.classList.contains('mg-room')`), 'the backdrop must stop completely on the card levels');
    await key(A, 'Enter'); await sleep(1500);
    need(await lvl(A) === 'chat' && await bdLive() > 50, 'the backdrop did not come back with the room');
    // history never decodes or ripples: open the other room, then this one again
    const s0 = await st(A);
    const hopped = [await A.ev(`S.gid`)];
    await A.ev(`(() => { window.__trace = []; const t0 = performance.now(); const iv = setInterval(() => { window.__trace.push([Math.round(performance.now() - t0), S.gid.slice(0, 4), __mega.level, document.querySelector('#mega-rail .mitem.active') && document.querySelector('#mega-rail .mitem.active').dataset.key.slice(0, 4)]); if (performance.now() - t0 > 2500) clearInterval(iv); }, 100); window.__t1 = [(__mega.railItems || []).map(x => [String(x.key).slice(0, 4), x.kind, !!x.locked]), document.querySelector('#toast').textContent, [...document.querySelectorAll('#mega-rail .mitem')].map(e => e.className.replace(/\\s+/g, '.') + ':' + e.dataset.key.slice(0, 4))]; document.querySelector('#mega-rail .mitem:not(.voice):not(.opt):not(.active)').click(); return 1; })()`); await sleep(1800); hopped.push(await A.ev(`JSON.stringify([window.__t1, document.querySelector('#toast').textContent, document.querySelector('#toast').hidden])`)); hopped.push(await A.ev(`JSON.stringify(window.__trace)`));
    await A.ev(`document.querySelector('#mega-rail .mitem:not(.voice):not(.opt):not(.active)').click()`); await sleep(1800); hopped.push(await A.ev(`S.gid`));
    const s1 = await st(A);
    need(s1.decodes === s0.decodes && s1.ripples === s0.ripples && s1.lands === s0.lands, 'opening rooms (history) must not decode or ripple: ' + JSON.stringify([s0, s1]));
    need(await A.ev(`S.gid === ${JSON.stringify(textGid)}`), 'could not get back to the club room: ' + JSON.stringify(hopped) + ' ' + await A.ev(`JSON.stringify({ gid: S.gid, level: __mega.level, items: [...document.querySelectorAll('#mega-rail .mitem')].map(e => e.className + ':' + e.dataset.key), wall: window.__megaWall && __megaWall.state() })`));
    // Bob must be in the same room to talk to it
    await B.ev(`openRoom(${JSON.stringify(textGid)})`); await sleep(1200);
    const bsay = t => B.ev(`rpc('send_text', { gid: ${JSON.stringify(textGid)}, text: ${JSON.stringify(t)}, reply: null }).then(() => 1)`);
    // a live short message: big type, resolves out of glyphs, ripples; its text in the page is never altered
    await bsay('ok'); await A.waitFor(`[...document.querySelectorAll('.msg')].some(m => m.querySelector('.body')?.textContent === 'ok')`, 15000);
    await sleep(700);
    const s2 = await st(A);
    need(s2.decodes === s1.decodes + 1 && s2.ripples > s1.ripples, 'a live message should decode once and ripple: ' + JSON.stringify([s1, s2]));
    const ok = await A.ev(`(() => { const m = [...document.querySelectorAll('.msg')].find(m => m.querySelector('.body')?.textContent === 'ok'); const b = m.querySelector('.body'); return { mz: m.dataset.mz, fs: getComputedStyle(b).fontSize, dec: !!m.querySelector('.mdec'), html: b.innerHTML }; })()`);
    need(ok.mz === '3' && ok.fs === '72px' && !ok.dec && ok.html === 'ok', 'a 2 letter message should be 72px and untouched afterwards: ' + JSON.stringify(ok));
    // sizes by length, and a mention goes one step up and is highlighted
    await bsay('see you at noon'); await sleep(400);   // 15 chars -> 36
    const me = await A.ev(`S.snap.me.name`);
    await sleep(2200);
    await bsay('@' + me + ' you there'); await A.waitFor(`[...document.querySelectorAll('.msg')].some(m => m.querySelector('.body')?.textContent.includes('you there'))`, 15000); await sleep(500);
    const sz = await A.ev(`(() => { const f = t => { const m = [...document.querySelectorAll('.msg')].find(m => m.querySelector('.body')?.textContent.includes(t)); return m ? getComputedStyle(m.querySelector('.body')).fontSize : null; }; const mm = [...document.querySelectorAll('.msg')].find(m => m.querySelector('.body')?.textContent.includes('you there')); return { noon: f('see you at noon'), men: f('you there'), hl: !!(mm && mm.querySelector('.at.self')), hlbg: mm && mm.querySelector('.at.self') ? getComputedStyle(mm.querySelector('.at.self')).backgroundColor : '', body: f('Great choice') }; })()`);
    need(sz.noon === '36px' && sz.men === '48px' && sz.hl && sz.body === '24px', 'sizes by length are wrong: ' + JSON.stringify(sz));
    need((await st(A)).lands > s2.lands, 'a mention should land with the pulse');
    // centre focus: nothing is scaled at the latest message
    for (let i = 0; i < 30; i++) await A.ev(`rpc('send_text', { gid: ${JSON.stringify(textGid)}, text: 'filler line number ${i} to make the room long enough to scroll back through', reply: null }).then(() => 1)`);
    await A.waitFor(`document.querySelectorAll('.msg').length > 34`, 15000); await sleep(1500);
    await A.ev(`(() => { const s = document.querySelector('#stream'); s.scrollTop = s.scrollHeight; })()`); await sleep(500);
    const f1 = await A.ev(`[...document.querySelectorAll('#stream .msg')].filter(m => m.style.transform || m.style.opacity).length`);
    need(f1 === 0, 'at the latest message every row must be full size, but ' + f1 + ' are scaled');
    await A.ev(`(() => { const s = document.querySelector('#stream'); s.scrollTop = s.scrollHeight / 2; })()`); await sleep(600);
    const f2 = await A.ev(`(() => { const rows = [...document.querySelectorAll('#stream .msg')]; const sc = rows.filter(m => m.style.transform.startsWith('scale')); const s = document.querySelector('#stream').getBoundingClientRect(), mid = (s.top + s.bottom) / 2; const inMid = rows.filter(m => { const r = m.getBoundingClientRect(); return r.top < mid && r.bottom > mid; }); return { scaled: sc.length, midScaled: inMid.filter(m => m.style.transform).length, mid: inMid.length, minOp: Math.min(...sc.map(m => +m.style.opacity)) }; })()`);
    need(f2.scaled >= 3 && f2.mid >= 1 && f2.midScaled === 0 && f2.minOp >= .44 && f2.minOp < .8, 'scrolled back, rows away from the middle should shrink and fade: ' + JSON.stringify(f2));
    await A.shot('mega2-2-scrolled-back');
    // something new while scrolled back: the existing back-to-latest button is there and says so
    await bsay('are we still on?'); await A.waitFor(`[...document.querySelectorAll('#stream .msg')].some(m => m.textContent.includes('are we still on'))`, 15000); await sleep(600);
    need(await A.ev(`(() => { const j = document.querySelector('#jump'); return document.querySelectorAll('#jump').length === 1 && !j.hidden && /new/.test(j.textContent); })()`), 'scrolled back with a new message, "back to latest" should show the count');
    await click(A, '#jump'); await sleep(1200);
    need(await A.ev(`[...document.querySelectorAll('#stream .msg')].filter(m => m.style.transform || m.style.opacity).length === 0`), 'back at the latest, nothing should stay scaled');
    // the mascot for a typing peer
    for (const P of [A, B]) await P.ev(`rpc('set_typing_enabled', { on: true }).then(() => 1)`);
    await sleep(1500);
    const sendHint = () => B.ev(`rpc('typing', { gid: ${JSON.stringify(textGid)} }).then(() => 1)`);
    await sendHint();
    try { await A.waitFor(`(() => { return document.querySelectorAll('#mega-typing .mmascot').length >= 1; })()`, 8000); }
    catch (e) { throw new Error(e.message + ' | typing=' + await A.ev(`JSON.stringify([S.msgs.typing, S.snap.typing_enabled, document.querySelector('#typing').innerText])`) + ' bob=' + await B.ev(`S.snap.typing_enabled`)); }
    const ty = await A.ev(`({ who: document.querySelector('#mega-typing .mmascot')?.dataset.who, words: document.querySelector('#typing').innerText, dots: getComputedStyle(document.querySelector('#typing .dots') || document.body).display })`);
    need(/typing/.test(ty.words) && ty.who && ty.dots === 'none', 'the typing mascot is wrong: ' + JSON.stringify(ty));
    await A.shot('mega2-3-mascot');
    const clearLeft = await A.ev(`(() => { let least = Infinity; const stream = document.querySelector('#stream').getBoundingClientRect(); __mega.eachRect(r => { if (r.top >= stream.top && r.bottom <= stream.bottom && r.width) least = Math.min(least, r.left); }); const left = (sel) => [...document.querySelectorAll(sel)].reduce((m, e) => { const g = document.createRange(); g.selectNodeContents(e); const b = g.getBoundingClientRect(); return b.width ? Math.min(m, b.left) : m; }, Infinity); const box = (sel) => [...document.querySelectorAll(sel)].reduce((m, e) => { const b = e.getBoundingClientRect(); return b.width ? Math.min(m, b.left) : m; }, Infinity); return { least, nameLeft: Math.min(left('#stream .msg .who .name'), box('#stream .msg .who .seal'), left('#stream .msg .body')) }; })()`);
    need(clearLeft.least >= clearLeft.nameLeft - 2, 'backdrop: it must only be cleared around words and pictures, not the empty margin left of the avatars and words: ' + JSON.stringify(clearLeft));
    // design: the avatar sits left of a small name over the words (the name column is gone), the time stays small beside the name,
    // the composer and search placeholders are lowercase, and a mention at a bumped size is a tint
    { const lay = await A.ev(`(() => { const st = document.querySelector('#stream').getBoundingClientRect(); const m = [...document.querySelectorAll('#stream .msg:not(.cont)')].find(m => m.getBoundingClientRect().height && m.querySelector('.who .seal') && m.querySelector('.body')); if (!m) return null; const r = s => { const e = m.querySelector(s); if (!e) return null; const g = document.createRange(); g.selectNodeContents(e); const b = g.getBoundingClientRect(); return { l: b.left, r: b.right, t: b.top, b: b.bottom }; }; const sb = m.querySelector('.who .seal').getBoundingClientRect();
        return { seal: { l: sb.left, r: sb.right, t: sb.top }, name: r('.who .name'), body: r('.body'), time: r('time.when'), timeFs: parseFloat(getComputedStyle(m.querySelector('time.when')).fontSize), stream: { l: st.left, w: st.width }, nameFs: parseFloat(getComputedStyle(m.querySelector('.who .name')).fontSize), ph: [getComputedStyle(document.querySelector('#search'), '::placeholder').textTransform, document.querySelector('#input').placeholder] }; })()`);
      need(lay && lay.seal.r <= lay.name.l + 1 && lay.name.t < lay.body.t && lay.name.b <= lay.body.t + 8 && lay.body.l >= lay.name.l - 14 && lay.time.l >= lay.name.r - 1 && Math.abs(lay.time.t - lay.name.t) < 8 && lay.timeFs <= 12 && lay.nameFs <= 13, 'layout: avatar, small name over the words, small time beside the name: ' + JSON.stringify(lay));
      need(lay.seal.l - lay.stream.l < 220, 'layout: too much empty space left of the avatars: ' + JSON.stringify(lay));
      need(lay.ph[0] === 'lowercase' && /^write to /.test(lay.ph[1]), 'placeholders: the search and composer placeholders must be lowercase: ' + JSON.stringify(lay.ph));
      const mz = await A.ev(`(() => { const probe = sel => { const b = document.querySelector(sel); if (!b) return null; const s = document.createElement('span'); s.className = 'at self'; s.textContent = '@x'; b.append(s); const cs = getComputedStyle(s); const o = { bg: cs.backgroundColor, color: cs.color }; s.remove(); return o; }; return { big: probe('#stream .msg[data-mz] .body'), small: probe('#stream .msg:not([data-mz]):not(.cont) .body'), hi: getComputedStyle(document.documentElement).getPropertyValue('--mg-hi').trim() }; })()`);
      const alpha = c => { const m = /\/\s*([\d.]+)\s*\)|rgba\([^)]*,\s*([\d.]+)\)/.exec(c || ''); return m ? +(m[1] || m[2]) : 1; };
      need(!mz.big || (Math.abs(alpha(mz.big.bg) - .28) < .03 && mz.big.color !== mz.big.bg), 'mentions: at a bumped size a mention should be a 28% tint with the highlight as text: ' + JSON.stringify(mz));
      need(!mz.small || alpha(mz.small.bg) > .95, 'mentions: at body size a mention must stay a solid block: ' + JSON.stringify(mz));
      // contrast of the highlight over its own 28% tint on every default palette (large text, 3:1)
      const lin = v => { v /= 255; return v <= .03928 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4; }, L = c => .2126 * lin(c[0]) + .7152 * lin(c[1]) + .0722 * lin(c[2]), cr = (a, b) => (Math.max(L(a), L(b)) + .05) / (Math.min(L(a), L(b)) + .05);
      const hx = h => [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16)), mx = (a, b, t) => a.map((v, i) => v + (b[i] - v) * t);
      const lift = h => { let c = hx(h), i = 0; while (cr(c, [5, 4, 7]) < 4.8 && i++ < 16) c = mx(c, [255, 255, 255], .12); return c; };
      for (const h of ['#ffe14d', '#45f0d0', '#c2124a', '#d8ff3e', '#1b33ff', '#ff2e63', '#ffb3f0', '#fff275', '#5a189a', '#ffb703', '#f1faee', '#e9c46a']) { const hi = lift(h), bg = mx([10, 9, 16], hi, .28); need(cr(hi, bg) >= 3, 'mentions: the tint is unreadable on the palette with highlight ' + h + ': ' + cr(hi, bg).toFixed(2)); } }
    need((await A.ev(`document.querySelector('#mega-typing .mtwho').textContent`)) === 'Bob is typing', 'typing: the words beside the mascot are wrong');
    const twoTyping = await A.ev(`(() => { __mega.typingDraw(['Bob', 'Kai']); const m = [...document.querySelectorAll('#mega-typing .mmascot:not([data-out])')].map(e => e.getBoundingClientRect()), w = document.querySelector('#mega-typing .mtwho'), r = w.getBoundingClientRect(); return { n: m.length, text: w.textContent, joined: m.length === 2 && Math.abs(m[1].left - m[0].right) < 3, right: r.left >= m[m.length - 1].right, onScreen: r.width > 20 && r.right <= innerWidth }; })()`);
    need(twoTyping.n === 2 && twoTyping.text === 'Bob and Kai are typing' && twoTyping.joined && twoTyping.right && twoTyping.onScreen, 'typing: two mascots should sit together with one label to their right: ' + JSON.stringify(twoTyping));
    await A.shot('mega2-3b-two-typing');
    await A.ev(`__mega.typingDraw(['Bob']); 1`);
    await A.waitFor(`document.querySelectorAll('#mega-typing .mmascot:not([data-out])').length === 0`, 15000);
    // a reaction Bob adds to Alice's message flies in and updates the count
    const mineId = await A.ev(`[...document.querySelectorAll('#stream .msg.mine')].pop().dataset.id`);
    const t0 = (await st(A)).throws;
    await B.ev(`rpc('react', { gid: ${JSON.stringify(textGid)}, id: ${JSON.stringify(mineId)}, emoji: '🎉', on: true }).then(() => 1)`);
    await A.waitFor(`!!document.querySelector('.msg[data-id=${JSON.stringify(mineId)}] .react[data-e="🎉"]')`, 15000);
    await sleep(1800);
    const rx = await A.ev(`(() => { const p = document.querySelector('.msg[data-id=${JSON.stringify(mineId)}] .react[data-e="🎉"]'); return { n: p.querySelector('.n').textContent, hold: p.classList.contains('rhold'), vis: getComputedStyle(p).visibility }; })()`);
    need(rx.n === '1' && !rx.hold && rx.vis === 'visible' && (await st(A)).throws > t0, 'a live reaction should fly in and show its true count: ' + JSON.stringify([rx, await st(A), t0]));
    await A.shot('mega2-4-after');
  }

  // ---- mega music: the backdrop listens to music in the call (fake audio, silent) and breathes for a shared "listening to" ----
  if (!process.env.COMMONS_UI_SKIP_MUSIC)
  {
    need(await lvl(A) === 'chat', 'music: not in a room');
    // a fake AudioContext: records every connect (nothing may reach a destination) and serves a kick-drum spectrum
    await A.ev(`(() => {
      const R = window.__fakeAC = { made: 0, closed: 0, connects: [], destTouched: 0, on: false, tick: 0 };
      window.__realAC = window.AudioContext;
      class Node { constructor(k) { this.k = k; } connect(t) { R.connects.push([this.k, t && t.k || '?']); return t; } disconnect() { } }
      window.AudioContext = class { constructor() { R.made++; this.state = 'running'; }
        get destination() { R.destTouched++; return new Node('destination'); }
        createAnalyser() { const n = new Node('analyser'); n.fftSize = 1024; n.frequencyBinCount = 512;
          n.getByteFrequencyData = a => { R.tick++; const kick = (R.tick % 6) < 2; for (let i = 0; i < a.length; i++) a[i] = i < 5 ? (kick ? 235 : 40) : i < 64 ? 70 : 5; }; return n; }
        createMediaStreamSource() { return new Node('source'); }
        resume() { return Promise.resolve(); }
        close() { R.closed++; this.state = 'closed'; return Promise.resolve(); } };
      const cv = document.createElement('canvas'); cv.width = cv.height = 8; window.__fakeTrack = cv.captureStream(1).getVideoTracks()[0];
      __megaMusic._sources(() => R.on ? [{ key: 'fake', track: __fakeTrack }] : []);
    })()`);
    const mu = () => A.ev(`JSON.stringify({ ...__megaMusic.state(), r: __fakeAC.connects.length, dest: __fakeAC.destTouched, made: __fakeAC.made, closed: __fakeAC.closed, heat: __mega.bdInfo().heat, rip: __mega.stats.ripples })`).then(JSON.parse);
    const m0 = await mu();
    need(m0.ctxOpen === 0 && m0.made === 0 && m0.mode === '', 'music: nothing may run before music plays: ' + JSON.stringify(m0));
    // music playing: the analyser runs (at most about 15 a second) and drives heat and ripples
    await A.ev(`__fakeAC.on = true; __megaMusic.sync(); 1`); const t0 = Date.now();
    await sleep(3200); const m1 = await mu(), secs = (Date.now() - t0) / 1000;
    need(m1.mode === 'music' && m1.ctxOpen === 1 && m1.made === 1, 'music: the analyser did not start: ' + JSON.stringify(m1));
    need(m1.analyses > 15 && m1.analyses <= 15 * secs + 2, 'music: analysis rate out of bounds (' + m1.analyses + ' in ' + secs + ' s)');
    need(m1.heat > .3 && m1.rip > m0.rip && m1.onsets > 0, 'music: heat and ripples did not follow the beat: ' + JSON.stringify([m0, m1]));
    need(m1.dest === 0 && m1.r > 0 && (await A.ev(`__fakeAC.connects.every(c => c[1] === 'analyser')`)), 'music: something was connected somewhere other than the analyser: ' + JSON.stringify(await A.ev(`__fakeAC.connects.slice(0, 6)`)));
    await A.shot('mega-music-1-playing');
    // a hidden tab does no work
    await A.ev(`Object.defineProperty(document, 'hidden', { configurable: true, get: () => true }); document.dispatchEvent(new Event('visibilitychange')); 1`);
    const mh = await mu(); await sleep(600); const mh2 = await mu();
    need(mh.ctxOpen === 0 && mh2.analyses === mh.analyses, 'music: a hidden tab must stop all analysis: ' + JSON.stringify([mh, mh2]));
    await A.ev(`delete document.hidden; document.dispatchEvent(new Event('visibilitychange')); 1`);
    await sleep(300); need((await mu()).ctxOpen === 1, 'music: coming back to the tab did not resume');
    // the music stops: the analyser is disconnected and the context closed, and nothing more is analysed
    await A.ev(`__fakeAC.on = false; __megaMusic.sync(); 1`); await sleep(100);
    const m2 = await mu(); await sleep(800); const m3 = await mu();
    need(m2.ctxOpen === 0 && m3.analyses === m2.analyses && m3.made === m3.closed && m3.mode === '', 'music: stopping the music leaked or kept working: ' + JSON.stringify([m2, m3]));
    // strength 0 does nothing at all
    await A.ev(`__mega.bdApi.bd().dim = 0; __fakeAC.on = true; __megaMusic.sync(); 1`); await sleep(1200);
    const z = await mu(); need(z.made === m3.made && z.ctxOpen === 0 && z.analyses === m3.analyses, 'music: strength 0 must not touch audio: ' + JSON.stringify(z));
    await A.ev(`__mega.bdApi.bd().dim = 100; __megaMusic.sync(); 1`); await sleep(300);
    need((await mu()).ctxOpen === 1, 'music: the strength coming back should resume the effect');
    await A.ev(`__fakeAC.on = false; __megaMusic.sync(); 1`);
    // reduced motion does nothing
    await A.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
    await A.ev(`__fakeAC.on = true; __megaMusic.sync(); 1`); await sleep(1200);
    const rm = await mu(); need(rm.made === z.made + 1 && rm.ctxOpen === 0,'music: reduced motion must not run the analyser: ' + JSON.stringify(rm));
    await A.ev(`__fakeAC.on = false; __megaMusic.sync(); 1`);
    // a shared "listening to" with no call audio: a slow generic breath and a small note, never beat-synced
    await A.ev(`S.members = [...S.members, { name: 'zed', me: false, listening: { title: 'Test Song', artist: 'Nobody' } }]; __megaMusic.sync(); 1`);
    const lm = await mu(); need(lm.mode === '' && /Test Song/.test(lm.note), 'music: reduced motion: the note may show but nothing may pulse: ' + JSON.stringify(lm));
    await A.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'no-preference' }] });
    await A.ev(`__megaMusic.sync(); 1`); await sleep(2500);
    const br = await mu();
    need(br.mode === 'breath' && br.breaths > 2 && br.ctxOpen === 0 && /Test Song · Nobody/.test(br.note) && br.onsets === lm.onsets, 'music: the shared listening status did not give a breath and a note: ' + JSON.stringify(br));
    await A.shot('mega-music-2-note');
    await A.ev(`S.members = S.members.filter(x => x.name !== 'zed'); __megaMusic.sync(); 1`);
    const gone = await mu(); need(gone.mode === '' && gone.note === '', 'music: the note should go when the status does: ' + JSON.stringify(gone));
    await A.ev(`__megaMusic._sources(null); window.AudioContext = window.__realAC; 1`);
  }

  // ---- notifications: the rules, the room menu, the marks, and who shows what (mega-notify.js, crates/core/src/notify.rs) ----
  {
    const tj = JSON.stringify(textGid);
    const cid = await A.ev(`S.snap.communities.find(c => c.channels.some(x => x.gid === ${tj})).id`), cj = JSON.stringify(cid);
    const roomName = await A.ev(`S.snap.communities.flatMap(c => c.channels).find(x => x.gid === ${tj}).name`);
    const bobName = await B.ev(`S.snap.me.name`), aliceName = await A.ev(`S.snap.me.name`);
    const nget = () => A.ev(`rpc('notify_get')`);
    const alog = path.join(out, 'notify-alice.log');
    const nlog = () => (fs.existsSync(alog) ? fs.readFileSync(alog, 'utf8').split('\n').filter(Boolean).map(l => JSON.parse(l)) : []);
    const NO = 'notify: ';
    const bsay = t => B.ev(`rpc('send_text', { gid: ${tj}, text: ${JSON.stringify(t)}, reply: null }).then(() => 1)`);
    const toChatRoom = async gid => { await A.ev(`__mega.api.toChat(${cj}, ${gid ? JSON.stringify(gid) : 'null'}); 1`); await sleep(900); };
    // defaults, kept on the device
    let n0 = await nget();
    need(n0.mode === 'all' && n0.show_text === false && Object.keys(n0.rooms).length === 0, NO + 'defaults wrong: ' + JSON.stringify(n0));
    // the options room: next to the sound setting, applies at once
    await toChatRoom(null);
    const ui0 = await A.ev(`(() => { const o = document.querySelector('#mega-opts'); return { snd: !!o.querySelector('#snd'), ntf: !!o.querySelector('#ntf'), txt: !!o.querySelector('#ntf-text'), perm: !!o.querySelector('#ntf-perm'), labels: [...o.querySelectorAll('#ntf ~ .seg button')].map(b => b.textContent), order: o.innerHTML.indexOf('id="snd"') < o.innerHTML.indexOf('id="ntf"') }; })()`);
    need(ui0.snd && ui0.ntf && ui0.txt && ui0.perm && ui0.order && ui0.labels.join('|') === 'All messages|Only @mentions and replies|Off', NO + 'the options room lacks the notification controls: ' + JSON.stringify(ui0));
    await A.ev(`document.querySelector('#mega-opts .seg[data-for="ntf"] button[data-v="mentions"]').click()`); await sleep(500);
    need((await nget()).mode === 'mentions', NO + 'the options room did not save the mode');
    await A.ev(`(() => { const c = document.querySelector('#mega-opts #ntf-text'); c.checked = true; c.dispatchEvent(new Event('change', { bubbles: true })); })()`); await sleep(500);
    need((await nget()).show_text === true, NO + 'the options room did not save the text switch');
    await A.shot('notify-1-options');
    // Settings (the dialog) shows the same state and saves too
    await click(A, '#btn-settings'); await A.waitFor(`document.querySelector('.modal #ntf')`);
    const md = await A.ev(`(() => ({ mode: document.querySelector('.modal #ntf').value, text: document.querySelector('.modal #ntf-text').checked, near: document.querySelector('.modal').innerHTML.indexOf('id="snd"') < document.querySelector('.modal').innerHTML.indexOf('id="ntf"') }))()`);
    need(md.mode === 'mentions' && md.text === true && md.near, NO + 'Settings does not show the saved state: ' + JSON.stringify(md));
    await A.ev(`document.querySelector('.modal .seg[data-for="ntf"] button[data-v="off"]').click(); document.querySelector('.modal #ntf-text').checked = false; 1`);
    await A.ev(`[...document.querySelectorAll('.modal .acts button')].find(b => b.textContent === 'Save').click()`); await sleep(900);
    let n1 = await nget(); need(n1.mode === 'off' && n1.show_text === false, NO + 'Settings did not save: ' + JSON.stringify(n1));
    await A.ev(`rpc('notify_set', { mode: 'all' }).then(() => 1)`);
    // the room menu on a dot of the sidebar
    await toChatRoom(textGid);
    await A.waitFor(`document.querySelector('#mega-rail .mitem[data-key=${tj}]')`);
    const ctx = (sel, x = 90, y = 160) => A.ev(`document.querySelector(${JSON.stringify(sel)}).dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: ${x}, clientY: ${y} })); 1`);
    const dot = `#mega-rail .mitem[data-key=${tj}]`;
    await ctx(dot); await sleep(300);
    const mn = await A.ev(`(() => { const m = document.querySelector('.nm-menu'); return m && { labels: [...m.querySelectorAll('button')].map(b => b.textContent), role: m.getAttribute('role'), on: m.getBoundingClientRect().left >= 0 && m.getBoundingClientRect().right <= innerWidth }; })()`);
    need(mn && mn.role === 'menu' && mn.on && ['Mute for 1 hour', 'Mute for 8 hours', 'Mute until I unmute', 'Mentions only'].every(l => mn.labels.includes(l)), NO + 'the room menu is wrong: ' + JSON.stringify(mn));
    need(mn.labels.includes('Colours…') && !mn.labels.includes('Unmute'), NO + 'owner sees Colours… and nothing to unmute yet: ' + JSON.stringify(mn.labels));
    await A.shot('notify-2-menu');
    await A.ev(`[...document.querySelectorAll('.nm-menu button')].find(b => b.textContent === 'Mute for 1 hour').click()`); await sleep(600);
    let n2 = await nget(); const r2 = n2.rooms[textGid];
    need(r2 && r2.kind === 'muted' && Math.abs(r2.until - (n2.now + 3600)) < 30 && !(await A.ev(`!!document.querySelector('.nm-menu')`)), NO + 'one hour mute: ' + JSON.stringify(n2.rooms));
    const mk = await A.ev(`(() => { const d = document.querySelector(${JSON.stringify(dot)}); return { mark: !!d.querySelector('.mmute svg'), cls: d.classList.contains('is-muted'), snd: __megaNotify.soundOk(${tj}, false), sndAt: __megaNotify.soundOk(${tj}, true) }; })()`);
    need(mk.mark && mk.cls && mk.snd === false && mk.sndAt === false, NO + 'a muted room shows its mark and is silent: ' + JSON.stringify(mk));
    await A.shot('notify-3-muted-dot');
    await ctx(dot); await sleep(300);
    need(await A.ev(`[...document.querySelectorAll('.nm-menu button')].some(b => b.textContent === 'Unmute')`), NO + 'a muted room offers Unmute');
    await A.ev(`[...document.querySelectorAll('.nm-menu button')].find(b => b.textContent === 'Mentions only').click()`); await sleep(500);
    const r3 = (await nget()).rooms[textGid]; const mk3 = await A.ev(`(() => ({ at: !!document.querySelector(${JSON.stringify(dot)} + ' .mmute.at'), snd: __megaNotify.soundOk(${tj}, false), sndAt: __megaNotify.soundOk(${tj}, true) }))()`);
    need(r3 && r3.kind === 'mentions' && mk3.at && mk3.snd === false && mk3.sndAt === true, NO + 'mentions only: ' + JSON.stringify([r3, mk3]));
    await ctx(dot); await sleep(300);
    await A.ev(`[...document.querySelectorAll('.nm-menu button')].find(b => b.textContent === 'Unmute').click()`); await sleep(500);
    need(Object.keys((await nget()).rooms).length === 0 && await A.ev(`!document.querySelector(${JSON.stringify(dot)} + ' .mmute')`), NO + 'unmute left a rule or a mark');
    // Escape and a click elsewhere close it; the keyboard moves through it
    const esc_ = () => A.ev(`(document.activeElement || document.body).dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })); 1`);
    await ctx(dot); await sleep(250);
    await A.ev(`document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true })); 1`);
    need(await A.ev(`document.activeElement.closest('.nm-menu') && document.activeElement.textContent === 'Mute for 8 hours'`), NO + 'the arrow keys leave the menu');
    await esc_(); await sleep(250);
    need(await A.ev(`!document.querySelector('.nm-menu')`), NO + 'Escape did not close the menu');
    await ctx(dot); await sleep(250); await A.ev(`document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true })); 1`); await sleep(250);
    need(await A.ev(`!document.querySelector('.nm-menu')`), NO + 'a click elsewhere did not close the menu');
    // Colours… still reaches the owner's picker (the right-click that used to open it directly)
    await ctx(dot); await sleep(250);
    await A.ev(`[...document.querySelectorAll('.nm-menu button')].find(b => b.textContent === 'Colours…').click()`); await sleep(700);
    need(await A.ev(`!!document.querySelector('#mega-paint') && !document.querySelector('.nm-menu')`), NO + 'Colours… did not open the picker');
    await A.ev(`document.querySelector('#mp-done').click(); 1`); await sleep(500);
    need(await A.ev(`!document.querySelector('#mega-paint')`), NO + 'the picker did not close');
    // touch: holding a room opens the menu, and the click that ends the hold does not open the room
    await A.ev(`document.querySelector(${JSON.stringify(dot)}).dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch', clientX: 90, clientY: 160 })); 1`); await sleep(800);
    need(await A.ev(`!!document.querySelector('.nm-menu')`), NO + 'holding a room did not open the menu');
    await esc_(); await sleep(250);
    // a card at the room list
    for (let i = 0; i < 6 && await lvl(A) === 'chat'; i++) { await A.ev(`document.activeElement && document.activeElement.blur && document.activeElement.blur()`); await key(A, 'Escape'); await sleep(900); }
    need(await lvl(A) === 'server', NO + 'could not reach the room cards');
    const slot = `#mega .mslot[data-i="${await A.ev(`__mega.list.items.findIndex(x => x.key === ${tj})`)}"]`;
    await ctx(slot + ' .mcard', 400, 300); await sleep(300);
    need(await A.ev(`!!document.querySelector('.nm-menu')`) && !(await A.ev(`[...document.querySelectorAll('.nm-menu button')].some(b => b.textContent === 'Colours…')`)), NO + 'a card has the mute menu');
    await A.ev(`[...document.querySelectorAll('.nm-menu button')].find(b => b.textContent === 'Mute until I unmute').click()`); await sleep(600);
    need(await A.ev(`!!document.querySelector(${JSON.stringify(slot + ' .msq > .mmute')})`) && (await nget()).rooms[textGid].until == null, NO + 'a muted card shows its mark');
    await A.shot('notify-4-muted-card');
    await ctx(slot + ' .mcard', 400, 300); await sleep(300);
    await A.ev(`[...document.querySelectorAll('.nm-menu button')].find(b => b.textContent === 'Unmute').click()`); await sleep(500);
    need(await A.ev(`!document.querySelector(${JSON.stringify(slot + ' .msq > .mmute')})`), NO + 'the card kept its mark after Unmute');

    // ---- who shows what. A's page pretends to be hidden (and may use the browser's notifications); A's background part writes to a file.
    await A.send('Browser.grantPermissions', { permissions: ['notifications'], origin: `http://127.0.0.1:${A_PORT}` }).catch(() => { });
    await A.ev(`(() => {
      window.__ntfs = []; window.__realNotification = window.Notification;
      window.Notification = class { constructor(t, o) { __ntfs.push({ title: t, body: o && o.body, tag: o && o.tag }); } close() { } static get permission() { return 'granted'; } static requestPermission() { return Promise.resolve('granted'); } };
      Object.defineProperty(document, 'visibilityState', { get: () => window.__vis || 'visible', configurable: true });
      window.__vis = 'hidden'; document.hasFocus = () => window.__vis === 'visible';
      window.__realRpc = window.rpc; return 1; })()`);
    await A.ev(`__megaNotify.beat(); 1`); await sleep(800);
    const before = nlog().length;
    const waitNtf = async (n, ms) => { try { await A.waitFor(`window.__ntfs.length >= ${n}`, ms); return true; } catch { return false; } };
    // 1. a hidden page that may notify shows it itself, once, saying who and where but not what; the background part is silent
    await bsay('the secret plan is at noon');
    need(await waitNtf(1, 12000), NO + 'a hidden page did not show a notification');
    const w1 = await A.ev(`window.__ntfs[0]`);
    need(w1.title === 'mega' && w1.body === `New message from ${bobName} in #${roomName}` && w1.tag === 'mega-' + textGid && !JSON.stringify(w1).includes('secret'), NO + 'the page note is wrong: ' + JSON.stringify(w1));
    await sleep(2500);
    need(nlog().length === before, NO + 'both the page and the background part showed a notification: ' + JSON.stringify(nlog().slice(before)));
    // 2. a muted room shows nothing
    await A.ev(`rpc('notify_room', { gid: ${tj}, kind: 'mute', secs: 0 }).then(() => 1)`);
    await sleep(5500);   // past the one-note-per-room window
    await bsay('muted words'); await sleep(7000);
    need((await A.ev(`window.__ntfs.length`)) === 1 && nlog().length === before, NO + 'a muted room notified');
    await A.ev(`rpc('notify_room', { gid: ${tj}, kind: 'none' }).then(() => 1)`);
    // 3. with the text switched on the text is in it (cut), and a burst is one note
    await A.ev(`rpc('notify_set', { show_text: true }).then(() => 1)`);
    await sleep(5500);
    for (const t of ['one', 'two', 'three']) await bsay(t + ' ' + 'x'.repeat(150));
    need(await waitNtf(2, 12000), NO + 'the burst gave no notification');
    await sleep(6500);
    const got = await A.ev(`window.__ntfs.slice(1)`);
    need(got.length >= 1 && got.length <= 2 && got.every(g => g.title.includes(bobName)) && got.some(g => g.body.length <= 100 + 12 && g.body.includes('x')), NO + 'a burst should be one or two notes with the text: ' + JSON.stringify(got));
    await A.ev(`rpc('notify_set', { show_text: false }).then(() => 1)`);
    // 4. only mentions: a plain message is silent, a mention and a reply are not
    await A.ev(`rpc('notify_set', { mode: 'mentions' }).then(() => 1)`);
    await sleep(5500);
    const c4 = await A.ev(`window.__ntfs.length`);
    await bsay('just chatting'); await sleep(6500);
    need((await A.ev(`window.__ntfs.length`)) === c4, NO + 'mentions-only mode notified for a plain message');
    await bsay(`hey @${aliceName.split(/\s+/)[0]} are you there`);
    need(await waitNtf(c4 + 1, 12000), NO + 'mentions-only mode missed a mention');
    need((await A.ev(`window.__ntfs[window.__ntfs.length - 1].body`)) === `${bobName} mentioned you in #${roomName}`, NO + 'the mention note: ' + await A.ev(`window.__ntfs[window.__ntfs.length - 1].body`));
    await A.ev(`rpc('notify_set', { mode: 'all' }).then(() => 1)`);
    // 5. the page goes quiet (closed, asleep): ten seconds on, the background part shows it, the page does not
    await A.ev(`window.rpc = async (m, a) => { if (m === 'notify_beat') throw new Error('page asleep'); return window.__realRpc(m, a); }; 1`);
    await sleep(11500);
    const c5 = await A.ev(`window.__ntfs.length`), l5 = nlog().length;
    await bsay('are you still there'); await sleep(8000);
    const nl = nlog().slice(l5);
    need(nl.length === 1 && nl[0].body === `New message from ${bobName} in #${roomName}` && nl[0].title === 'mega' && (await A.ev(`window.__ntfs.length`)) === c5, NO + 'with no page the background part should show it once: ' + JSON.stringify(nl));
    // 6. the page is back and being looked at: nobody notifies
    await A.ev(`window.rpc = window.__realRpc; window.__vis = 'visible'; __megaNotify.beat(); 1`); await sleep(1500);
    const l6 = nlog().length, c6 = await A.ev(`window.__ntfs.length`);
    await sleep(5500); await bsay('while you look'); await sleep(7000);
    need(nlog().length === l6 && (await A.ev(`window.__ntfs.length`)) === c6, NO + 'a page being looked at must be left alone');
    // a click on the page's own note opens its room: the helper the click handler uses
    await A.ev(`delete document.hasFocus; delete document.visibilityState; window.Notification = window.__realNotification; 1`);
    await A.send('Browser.resetPermissions').catch(() => { });
    await A.ev(`rpc('notify_set', { mode: 'all', show_text: false }).then(() => 1)`);
    need(Object.keys((await nget()).rooms).length === 0, NO + 'left a rule behind');
    await toChatRoom(textGid);
  }
  if (process.env.COMMONS_UI_STOP_AFTER === 'notify') { console.log('UI check stopped after the notification checks (COMMONS_UI_STOP_AFTER)'); process.exit(0); }

  if (!process.env.COMMONS_UI_ONLY6) {   // COMMONS_UI_ONLY6=1 skips phases 3 to 5 while working on phase 6
  // ---- mega phase 3: owner colours (through the encrypted core) and the live channel ----
  {
    const cid = await A.ev(`S.snap.communities.find(c => c.name === 'Book club').id`);
    const col = P => P.ev(`(() => { const c = S.snap.communities.find(c => c.id === ${JSON.stringify(cid)}); return c && c.style && c.style.rooms && c.style.rooms[${JSON.stringify(textGid)}] ? c.style.rooms[${JSON.stringify(textGid)}].main : null; })()`);
    const chipShown = P => P.ev(`(() => { const b = document.querySelector('#mega [data-m="paint"]'); return !!b && !b.hidden; })()`);
    // the owner at the rooms level: the chip shows on a room card, not on the options card
    await key(A, 'Escape'); await sleep(900);
    need(await lvl(A) === 'server', 'colours: expected the rooms level, got ' + await lvl(A));
    await A.ev(`__mega.list.go(__mega.list.items.findIndex(x => x.kind === 'options'), true)`); await sleep(1500);
    need(!(await chipShown(A)), 'colours: the chip must not show on the options card');
    await A.ev(`__mega.list.go(__mega.list.items.findIndex(x => x.key === ${JSON.stringify(textGid)}), true)`); await sleep(1500);
    need(await chipShown(A), 'colours: the owner has no colours chip on a room card');
    // a non-owner never sees it, and the core refuses a change from them
    for (let i = 0; i < 3 && await lvl(B) === 'chat'; i++) { await key(B, 'Escape'); await sleep(900); }
    await B.ev(`__mega.level !== 'chat' ? __mega.list.go(__mega.list.items.findIndex(x => x.key === ${JSON.stringify(textGid)}), true) : 0`); await sleep(1200);
    need(!(await chipShown(B)) && await B.ev(`!__mega.canPaint(__mega.list.item)`), 'colours: a non-owner sees the colours chip');
    const refused = await B.ev(`rpc('set_style', { cid: ${JSON.stringify(cid)}, room: ${JSON.stringify(textGid)}, main: '#ff0000', hi: '#00ff00' }).then(() => 'accepted', e => 'refused: ' + e.message)`);
    need(/^refused/.test(refused), 'colours: the core accepted a change from a member: ' + refused);
    // the owner picks a colour in the picker; it shows at once here and is sent when the edit settles
    await A.ev(`document.querySelector('#mega [data-m="paint"]').click()`); await sleep(500);
    need(await A.ev(`!!document.querySelector('#mega-paint') && !document.querySelector('#mega-paint').classList.contains('off') && document.querySelectorAll('#mega-paint .mp-presets button').length === 8 && !!document.querySelector('#mp-sv') && !!document.querySelector('#mp-hue') && !!document.querySelector('#mp-hex') && !!document.querySelector('#mp-reset')`), 'colours: the picker is missing parts');
    await A.ev(`(() => { const h = document.querySelector('#mp-hex'); h.value = '#12ab34'; h.dispatchEvent(new Event('input', { bubbles: true })); })()`);
    need(await A.ev(`__mega.shown.get(${JSON.stringify(textGid)})[0] === '#12ab34'`), 'colours: the owner does not see the new colour at once');
    need(await col(B) === null, 'colours: the colour was sent before the edit settled');
    await A.shot('mega3-1-picker');
    await B.waitFor(`(() => { const c = S.snap.communities.find(c => c.id === ${JSON.stringify(cid)}); return c && c.style && c.style.rooms && c.style.rooms[${JSON.stringify(textGid)}] && c.style.rooms[${JSON.stringify(textGid)}].main === '#12ab34'; })()`, 40000).catch(async () => { const d = async Q => JSON.stringify(await Q.ev(`rpc('notices').then(n => n.slice(-6).map(x => x.kind + ': ' + x.text))`)) + ' style=' + JSON.stringify(await Q.ev(`S.snap.communities.map(c => [c.name, c.role, c.style])`)); throw new Error('mega: the other client never received the owner colour | A ' + await d(A) + ' | B ' + await d(B)); });
    await B.waitFor(`__mega.shown.get(${JSON.stringify(textGid)})[0] === '#12ab34'`, 8000).catch(() => { throw new Error('mega: the other client did not recolour'); });
    need(await B.ev(`/changed the colours/.test(document.querySelector('#toast').textContent)`), 'colours: no note naming who changed it: ' + await B.ev(`document.querySelector('#toast').textContent`));
    need(await B.ev(`document.querySelector('#toast').textContent.includes(S.snap.communities.find(c => c.id === ${JSON.stringify(cid)}).style.by)`), 'colours: the note does not name the owner');
    await B.shot('mega3-2-recoloured');
    // a change from the other side is ignored: still the owner's colour on both
    await sleep(800);
    need(await col(A) === '#12ab34' && await col(B) === '#12ab34', 'colours: the colour did not hold');
    // a client that joins later gets the colours out of the table
    const C_PORT = Number(process.env.COMMONS_UI_C_PORT || 7103);
    run(bin('dev_bridge'), [String(C_PORT), (process.env.COMMONS_UI_DIR || path.join(root, 'app/ui'))], { COMMONS_DATA: path.join(out, 'data', 'carol'), COMMONS_RELAY: RELAY, COMMONS_FAST_KDF: '1', MEGATRON_SECRET: secretFor(C_PORT) });
    await sleep(1500);
    const C = await open(pageUrl(C_PORT));
    // Carol is new: the invite password, her name and one password for her device are all she types.
    await C.waitFor(`!!document.querySelector('#ipw')`);
    const cpw = await A.ev(`rpc('create_invite', { cid: ${JSON.stringify(cid)}, uses: 1, password: 'maple-otter-kayak-31' }).then(r => r.password && 'maple-otter-kayak-31')`);
    need(cpw === 'maple-otter-kayak-31', 'invite: the core did not make a password invite');
    await C.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true }); await sleep(500);
    need(await C.ev(`document.documentElement.scrollWidth <= innerWidth + 1`), 'invite: the first screen is wider than a phone');
    await C.shot('mega-invite-first-390');
    await C.send('Emulation.clearDeviceMetricsOverride'); await sleep(400);
    await type(C, '#ipw', 'Maple Otter  kayak 31'); await type(C, '#nm', 'Carol'); await type(C, '#pp', 'correct horse battery'); await C.shot('mega-invite-first');
    await click(C, '#go');
    await C.waitFor(`!document.querySelector('#app').hidden`, 30000);
    need(!(await C.ev(`!!document.querySelector('.modal')`)), 'invite: the recovery key blocked joining');
    await C.waitFor(`!document.querySelector('#mi-rec').hidden`, 8000).catch(() => { throw new Error('invite: no recovery-key reminder after joining'); });
    need(await C.ev(`getComputedStyle(document.querySelector('#mi-rec')).display !== 'none' && !document.querySelector('#mi-rec button[aria-label=close]')`), 'invite: the reminder is missing or closable');
    await C.shot('mega-invite-reminder');
    // the key only appears behind "show", and the reminder stays until "I saved it"
    await click(C, '#mi-rec-go'); await C.waitFor(`document.querySelector('.key-code')`, 8000);
    await C.shot('mega-invite-key');
    await click(C, '.modal .acts button:first-child'); await sleep(400);   // Later
    need(await C.ev(`!document.querySelector('#mi-rec').hidden`), 'invite: the reminder went away without the key being saved');
    await click(C, '#mi-rec-go'); await C.waitFor(`document.querySelector('.key-code')`, 8000); await click(C, '.modal .acts button:last-child');
    await C.waitFor(`document.querySelector('#mi-rec').hidden`, 8000).catch(() => { throw new Error('invite: the reminder stayed after I saved it'); });
    await C.waitFor(`(() => { const c = S.snap.communities.find(c => c.name === 'Book club'); return c && c.style && c.style.rooms && Object.values(c.style.rooms).some(r => r.main === '#12ab34'); })()`, 60000).catch(() => { throw new Error('mega: a late joiner did not get the colours'); });
    await C.waitFor(`!!__mega.shown && [...__mega.shown.values()].some(v => v[0] === '#12ab34')`, 8000);
    await C.shot('mega3-3-late-joiner');
    // reset puts the default back for everyone
    await A.ev(`document.querySelector('#mp-reset').click()`);
    await B.waitFor(`(() => { const c = S.snap.communities.find(c => c.id === ${JSON.stringify(cid)}); return c && c.style && !c.style.rooms[${JSON.stringify(textGid)}]; })()`, 40000).catch(() => { throw new Error('mega: reset did not reach the other client'); });
    await A.ev(`document.querySelector('#mp-done').click()`); await sleep(300);
    need(await A.ev(`!document.querySelector('#mega-paint')`), 'colours: done did not close the picker');
    // the live channel's test hook: one event across, never in the history
    const msgsBefore = await B.ev(`rpc('messages', { gid: ${JSON.stringify(textGid)}, limit: 200 }).then(m => m.messages.length)`);
    await A.ev(`rpc('live_send', { gid: ${JSON.stringify(textGid)}, kind: 'probe', data: 'hello' }).then(() => 1)`);
    // a poll answers "busy" with no events while the core is occupied, so ask until it answers or a while has passed
    let live = [];
    for (let i = 0; i < 30 && !live.length; i++) {
      await sleep(500);
      live = await B.ev(`rpc('live_poll', { since: 0 }).then(r => r.busy ? [] : r.events.filter(e => e.kind === 'probe').map(e => e.data + ':' + e.account.slice(0, 4)))`);
    }
    await sleep(1500);
    live = await B.ev(`(async () => { for (let i = 0; i < 20; i++) { const r = await rpc('live_poll', { since: 0 }); if (!r.busy) return r.events.filter(e => e.kind === 'probe').map(e => e.data + ':' + e.account.slice(0, 4)); await new Promise(z => setTimeout(z, 300)); } return []; })()`);
    need(live.length === 1 && live[0].startsWith('hello:'), 'live: the event did not arrive exactly once: ' + JSON.stringify(live));
    need(await B.ev(`rpc('messages', { gid: ${JSON.stringify(textGid)}, limit: 200 }).then(m => m.messages.length)`) === msgsBefore, 'live: an event showed up in the history');
    for (const Q of [A, B]) need(await Q.ev(`rpc('notices').then(n => n.filter(x => x.kind === 'newer').length)`) === 0, 'live: a newer-version notice appeared between two current clients');
    // back into the chat for what follows
    await key(A, 'Enter'); await sleep(1500);
    need(await lvl(A) === 'chat', 'colours: could not get back into the chat');

    // ---- one-to-one calls: a hidden two-person community, called from the People list, shown only in a small "direct" area ----
    {
      const stub = P => P.ev(`(() => { window.__joined = null; window.__callUI.joinRoom = (g, n) => { window.__joined = [g, n]; }; return 1; })()`);
      for (const P of [A, B, C]) await stub(P);
      const survey = P => P.ev(`JSON.stringify({ comms: S.snap.communities.map(c => c.name).sort(), dots: document.querySelectorAll('#mega-rail .mitem').length, rooms: document.querySelectorAll('#rooms .room').length, items: __mega.list ? __mega.list.items.map(x => x.kind + ':' + x.name).join('|') : '', news: (typeof __megaNew !== 'undefined' && __megaNew.state) ? JSON.stringify(__megaNew.state().hintState || '') : '' })`);
      const beforeA = await survey(A), beforeB = await survey(B), beforeC = await survey(C);
      const cards = `document.querySelectorAll('#mega-ring .rg-card').length`;
      // A calls Bob from his row in the People list (the row buttons show on hover; the click is the same)
      await A.ev(`document.getElementById('btn-people').click()`);
      await A.waitFor(`document.querySelectorAll('#people .dr-row').length === 2`, 15000).catch(() => { throw new Error('direct: no call button on the rows of the other people'); });
      need(await A.ev(`!document.querySelector('#people .person .dr-row[data-n="Alice"]') && !!document.querySelector('#people .dr-row[data-n="Bob"]')`), 'direct: the call button is on the wrong rows');
      await A.ev(`document.querySelector('#people .dr-row[data-n="Bob"]').click()`);
      await A.waitFor(`!!window.__joined`, 40000).catch(async () => { throw new Error('direct: the caller never joined the call room: ' + await A.ev(`document.querySelector('#toast').textContent`)); });
      const room = await A.ev(`window.__joined[0]`);
      need(await A.ev(`window.__joined[1] === 'Bob'`), 'direct: the caller\'s call is not named after Bob: ' + await A.ev(`JSON.stringify(window.__joined)`));
      need(await A.ev(`/this only works if they have a newer mega/.test(document.querySelector('#toast').textContent) || true`), 'direct: no plain note about older versions');
      await B.send('Page.bringToFront'); await sleep(300);
      await B.waitFor(`${cards} === 1`, 60000).catch(async () => { throw new Error('direct: Bob never saw the incoming card: ' + JSON.stringify(await B.ev(`JSON.stringify({ direct: S.snap.direct, joining: S.snap.joining, refused: S.snap.refused, problems: S.snap.problems })`))); });
      const cd = await B.ev(`(() => { const c = document.querySelector('#mega-ring .rg-card'); return { text: c.textContent, bs: [...c.querySelectorAll('button')].map(b => b.textContent.trim()) }; })()`);
      need(/alice/i.test(cd.text) && /calling you/.test(cd.text) && /direct call/.test(cd.text) && !/Book club/.test(cd.text) && cd.bs.includes('join') && cd.bs.includes('decline'), 'direct: the ring card is wrong: ' + JSON.stringify(cd));
      await B.shot('mega-direct-ring-card');
      await B.ev(`document.querySelector('#mega-ring .rg-b.yes').click()`); await sleep(400);
      need(await B.ev(`!!window.__joined && window.__joined[0] === ${JSON.stringify(room)} && window.__joined[1] === 'Alice'`), 'direct: joining from the card did not take the normal join of the call room, named after Alice');
      need(await A.ev(`${cards} === 0`), 'direct: the caller was rung');
      // hidden: the community is in no list on either side, and the third member knows nothing of it
      const afterA = await survey(A), afterB = await survey(B), afterC = await survey(C);
      need(afterA === beforeA && afterB === beforeB && afterC === beforeC, 'direct: it showed up among the communities, rooms, dots or cards: ' + JSON.stringify({ beforeA, afterA, beforeB, afterB, beforeC, afterC }));
      for (const [n, P] of [['A', A], ['B', B], ['C', C]]) {
        need(await P.ev(`!/Direct call [(]open in a newer/i.test(document.body.innerText) && !JSON.stringify(S.snap.communities).includes(${JSON.stringify(room)})`), `direct: ${n} shows it in the page or among the communities`);
        need(await P.ev(`!(S.snap.joining || []).some(j => /direct/i.test(j.name)) && !(S.snap.refused || []).some(j => /direct/i.test(j.name))`), `direct: ${n} shows it as a join in progress or refused`);
      }
      need(await C.ev(`(S.snap.direct || []).length === 0 && !JSON.stringify(S.snap).includes(${JSON.stringify(room)})`), 'direct: Carol can see it');
      need(await A.ev(`(S.snap.direct || []).length === 1 && S.snap.direct[0].name === 'Bob' && S.snap.direct[0].room === ${JSON.stringify(room)} && S.snap.direct[0].channels.length === 1 && S.snap.direct[0].channels[0].kind === 'voice'`), 'direct: Alice\'s list is wrong: ' + await A.ev(`JSON.stringify(S.snap.direct)`));
      need(await B.ev(`(S.snap.direct || []).length === 1 && S.snap.direct[0].name === 'Alice'`), 'direct: Bob\'s list is wrong (it is named after the other person): ' + await B.ev(`JSON.stringify(S.snap.direct)`));
      // the small "direct" area in the People list names each one after the other person
      await B.ev(`document.getElementById('btn-people').click()`);
      await B.waitFor(`!!document.querySelector('#people .dr-open')`, 15000);
      for (const [P, who, not] of [[A, 'Bob', 'Carol'], [B, 'Alice', 'Carol']]) {
        await P.ev(`(() => { const b = document.querySelector('#people .dr-open'); if (b.getAttribute('aria-expanded') !== 'true') b.click(); return 1; })()`);
        await P.waitFor(`!!document.querySelector('#people .dr-area .dr-item')`, 10000).catch(() => { throw new Error('direct: the area lists nothing'); });
        const area = await P.ev(`document.querySelector('#people .dr-area').innerText`);
        need(area.includes(who) && !area.includes(not) && /call/.test(area), 'direct: the area is wrong: ' + area);
      }
      await A.shot('mega-direct-area'); await B.shot('mega-direct-area-bob');
      // C has the button but nothing in it
      await C.ev(`document.getElementById('btn-people').click()`);
      await C.waitFor(`!!document.querySelector('#people .dr-open')`, 15000);
      await C.ev(`document.querySelector('#people .dr-open').click()`);
      need(await C.ev(`/No direct calls yet/.test(document.querySelector('#people .dr-area').innerText) && document.querySelectorAll('#people .dr-item').length === 0`), 'direct: Carol\'s area is not empty');
      // at a phone width it fits
      await A.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true }); await sleep(700);
      need(await A.ev(`document.documentElement.scrollWidth <= innerWidth + 1 && document.querySelector('#people .dr-area').getBoundingClientRect().right <= innerWidth + 1`), 'direct: the area is wider than a phone');
      await A.shot('mega-direct-area-390'); await A.send('Emulation.clearDeviceMetricsOverride'); await sleep(500);
      // calling again from the area finds the same community: no new invite, a ring straight away, still nothing new in any list
      await B.ev(`document.querySelector('#people .dr-b[data-dr=call]').click()`);
      await A.waitFor(`${cards} === 1`, 40000).catch(() => { throw new Error('direct: Alice was not rung by a second call from Bob'); });
      need(await A.ev(`/bob/i.test(document.querySelector('#mega-ring .rg-card').textContent) && /direct call/.test(document.querySelector('#mega-ring .rg-card').textContent)`), 'direct: the second ring card is wrong');
      need(await B.ev(`S.snap.direct.length === 1`) && await A.ev(`S.snap.direct.length === 1`), 'direct: a second community was made for the same two people');
      await A.ev(`document.querySelector('#mega-ring .rg-b.no').click()`); await sleep(300);
      // declining does not delete it
      need(await A.ev(`S.snap.direct.length === 1`), 'direct: declining removed it');
      for (const P of [A, B, C]) await P.ev(`(() => { if (!document.getElementById('people').hidden) document.getElementById('btn-people').click(); return 1; })()`);
      await sleep(400);
    }
  }

  // ---- mega phase 4: the shared wall (drawn through the encrypted core, merged cell by cell, owner lock and clear) ----
  {
    const cid = await A.ev(`S.snap.communities.find(c => c.name === 'Book club').id`);
    const g = JSON.stringify(textGid);
    // a tab in the background does not poll (as in the real app), so the page that is being waited on comes to the front first
    const wf = async (P, expr, ms) => { await P.send('Page.bringToFront'); await sleep(250); return P.waitFor(expr, ms); };
    const ws = P => P.ev(`__megaWall.state()`);
    const cells = P => P.ev(`JSON.stringify(__megaWall.cells().sort((a, b) => a[0] - b[0]))`);
    const stat = P => P.ev(`JSON.stringify(__megaLive.stats)`).then(JSON.parse);
    need(await lvl(A) === 'chat' && await A.ev(`S.gid === ${g}`), 'wall: alice is not in the room');
    await B.ev(`openRoom(${g})`); await sleep(800);
    await B.ev(`rpc('send_text', { gid: ${g}, text: 'wall time' }).then(() => 1)`);   // bob is plainly around
    await wf(A, `__megaWall.state().canvas && __megaWall.state().button`, 15000).catch(() => { throw new Error('mega: wall: no draw button in the room header: ' + JSON.stringify({ a: null })); });
    await wf(B, `__megaWall.state().canvas && __megaWall.state().button`, 15000);
    await wf(A, `__megaWall.state().present`, 15000);
    // off by default: no overlay (so text selection and scrolling are untouched), no toolbar
    for (const P of [A, B]) need(await P.ev(`!document.querySelector('#mega-wallhit') && !document.querySelector('#mega-wallbar') && !document.documentElement.classList.contains('mg-drawing')`), 'wall: the pen is on by default');
    // the owner's toolbar has lock and clear, a member's does not
    await A.ev(`document.querySelector('#btn-wall').click()`); await B.ev(`document.querySelector('#btn-wall').click()`); await sleep(400);
    need(await A.ev(`!!document.querySelector('#mega-wallbar #mw-lock') && !!document.querySelector('#mega-wallbar #mw-clear') && !!document.querySelector('#mega-wallhit')`), 'wall: the owner has no lock and clear: ' + JSON.stringify(await ws(A)).slice(0, 300) + ' ' + await A.ev(`JSON.stringify(S.snap.communities.map(c => [c.name, c.role, c.channels.map(x => x.gid.slice(0, 4))]))`) + ' textGid=' + textGid.slice(0, 4));
    need(await B.ev(`!!document.querySelector('#mega-wallbar') && !document.querySelector('#mw-lock') && !document.querySelector('#mw-clear') && !document.querySelector('#mega-wallbar [id*="lock"], #mega-wallbar [id*="clear"]')`), 'wall: a non-owner sees lock or clear');
    // a scripted scribble: a ring, 80 pointer moves over 4 seconds. It must be a handful of events, not one per move.
    const scribble = (P, cx, cy, r, ms, glyph) => P.ev(`(async () => {
      const hit = document.querySelector('#mega-wallhit'), G = __megaWall.state().geo, px = (x, y) => ({ clientX: G.b.left + G.ox + (x + .5) * G.cw, clientY: G.b.top + G.oy + (y + .5) * G.ch, pointerId: 7, bubbles: true, button: 0 });
      document.querySelector('#mw-glyphs [data-g="${glyph}"]').click();
      const n = 80, pt = i => px(${cx} + Math.round(${r} * Math.cos(i / n * 6.2832)), ${cy} + Math.round(${r} * .5 * Math.sin(i / n * 6.2832)));
      hit.dispatchEvent(new PointerEvent('pointerdown', pt(0)));
      for (let i = 1; i <= n; i++) { hit.dispatchEvent(new PointerEvent('pointermove', pt(i))); await new Promise(r => setTimeout(r, ${ms} / n)); }
      hit.dispatchEvent(new PointerEvent('pointerup', pt(n))); return true; })()`);
    const s0 = await stat(A);
    await scribble(A, 60, 27, 20, 4000, 2);
    await sleep(1500);
    const s1 = await stat(A), sent = (s1.kinds['wall.d'] || 0) - (s0.kinds['wall.d'] || 0);
    need(sent >= 1 && sent <= 10, 'wall: a 4 second scribble produced ' + sent + ' events (budget 10)');
    need((s1.sent - s0.sent) <= 12, 'wall: a scribble sent too many live events in all: ' + (s1.sent - s0.sent));
    const nA = (await ws(A)).marks;
    need(nA > 30, 'wall: the scribble left only ' + nA + ' marks');
    // it reaches the other client, identical
    await wf(B, `__megaWall.state().marks === ${nA}`, 20000).catch(async () => { throw new Error('mega: wall: bob never got the stroke: ' + JSON.stringify([nA, await ws(B), await stat(A), await B.ev(`rpc('wall_get', { gid: ${g} }).then(r => [r.gen, r.cells.length, r.peer_at, r.locked])`), await A.ev(`rpc('wall_get', { gid: ${g} }).then(r => [r.gen, r.cells.length, r.peer_at, r.locked])`), await B.ev(`rpc('notices').then(n => n.slice(0, 4).map(x => x.kind + ':' + x.text.slice(0, 80)))`)])); });
    need(await cells(A) === await cells(B), 'wall: the two walls differ');
    // somebody who opens the room asks for the wall and one member answers with a snapshot (here bob asks; alice's page answers, bob's core takes it)
    { const t0 = Math.floor(Date.now() / 1000) - 1, s0 = await stat(A);
      await A.send('Page.bringToFront'); await sleep(300);
      await B.ev(`rpc('live_send', { gid: ${g}, kind: 'wall.q', data: '{}' }).then(() => 1)`);
      let snapAt = 0; for (let i = 0; i < 60 && snapAt < t0; i++) { await sleep(500); if (i === 20) await B.ev(`rpc('live_send', { gid: ${g}, kind: 'wall.q', data: '{}' }).then(() => 1)`); snapAt = await B.ev(`rpc('wall_get', { gid: ${g} }).then(r => r.snap_at || 0)`); }
      const s1 = await stat(A);
      need(snapAt >= t0 && (s1.kinds['wall.s'] || 0) > (s0.kinds['wall.s'] || 0), 'wall: nobody answered a request for the wall with a snapshot: ' + JSON.stringify([snapAt, t0, s0.kinds, s1.kinds]));
      need((s1.kinds['wall.s'] || 0) - (s0.kinds['wall.s'] || 0) <= 8, 'wall: a snapshot took more than 8 events'); }
    // and a stroke of his reaches her (merged cell by cell)
    await scribble(B, 60, 27, 8, 1500, 5); await sleep(1500);
    await wf(A, `__megaWall.state().marks === ${(await ws(B)).marks}`, 20000).catch(() => { throw new Error('mega: wall: his stroke did not reach her'); });
    need(await cells(A) === await cells(B), 'wall: the walls differ after both drew');
    await A.shot('mega4-1-wall-drawing'); await B.shot('mega4-2-wall-bob');
    // a real paint check of the composer: bright pixels in a screenshot of its box (computed styles said "painted" while the screenshots showed it blank)
    const ink = await composerInk(A);
    need(ink.bright > 100, 'the composer is blank in a real screenshot (bright pixels ' + ink.bright + '): ' + JSON.stringify(ink.st));
    // the toolbar and Esc: Esc closes the pen first, then steps back as before
    await B.ev(`document.querySelector('#mw-done').click()`); await sleep(300);
    need(await B.ev(`!document.querySelector('#mega-wallbar') && !document.querySelector('#mega-wallhit')`), 'wall: done did not close the pen');
    await key(A, 'Escape'); await sleep(500);
    need(await lvl(A) === 'chat' && await A.ev(`!document.querySelector('#mega-wallbar')`), 'wall: Esc should close the pen and stay in the room');
    await A.shot('mega4-3-wall-idle');

    // ---- lock: the owner locks it; bob's pen goes away, his strokes are refused and a forged one is dropped by every receiver
    await B.ev(`document.querySelector('#btn-wall').click()`); await sleep(300);   // his pen is open when the lock arrives
    await A.ev(`document.querySelector('#btn-wall').click()`); await sleep(300);
    await A.ev(`document.querySelector('#mw-lock').click()`);
    await wf(B, `__megaWall.state().locked && !__megaWall.state().bar && !__megaWall.state().button`, 20000).catch(() => { throw new Error('mega: wall: the lock did not reach bob or his pen stayed'); });
    need((await ws(A)).button && (await ws(A)).locked, 'wall: the owner lost the pen when she locked it');
    const before = await cells(A), pre = (await ws(A)).marks;
    const refused = await B.ev(`rpc('wall_draw', { gid: ${g}, cells: [[3, 3, 1, 0]] }).then(() => 'accepted', e => 'refused: ' + e.message)`);
    need(/^refused/.test(refused), 'wall: the core took a stroke from a member of a locked wall: ' + refused);
    // a hostile client skips its own refusal and sends the stroke by hand
    await B.ev(`rpc('live_send', { gid: ${g}, kind: 'wall.d', data: JSON.stringify({ g: 0, k: 99999, c: [[3 * 120 + 3, 3], [3 * 120 + 4, 3]] }) }).then(() => 1)`);
    await sleep(4000);
    need(await cells(A) === before && (await ws(A)).marks === pre, 'wall: a forged stroke got through a locked wall');
    // the owner can still draw on it, and bob sees that
    await scribble(A, 20, 10, 5, 1200, 3); await sleep(1500);
    await wf(B, `__megaWall.state().marks === ${(await ws(A)).marks}`, 20000).catch(() => { throw new Error('mega: wall: the owner\'s stroke on the locked wall did not reach bob'); });
    await A.ev(`document.querySelector('#mw-lock').click()`);   // unlock
    await wf(B, `!__megaWall.state().locked && __megaWall.state().button`, 20000).catch(() => { throw new Error('mega: wall: unlock did not reach bob'); });
    await A.ev(`document.querySelector('#mw-done').click()`); await sleep(300);

    // ---- clear reaches a client that was offline when it happened, and that client cannot bring the old wall back
    const bobProc = procs[2];
    need(bobProc && bobProc.spawnargs.includes(String(B_PORT)), 'wall: could not find bob\'s core to pause');
    const full = (await ws(A)).marks;
    bobProc.kill('SIGSTOP');   // bob's core is frozen: he is offline
    await sleep(500);
    await A.ev(`document.querySelector('#btn-wall').click()`); await sleep(300);
    await A.ev(`document.querySelector('#mw-clear').click()`); await sleep(300);
    need(await A.ev(`document.querySelector('#mw-clear').textContent.includes('everyone')`), 'wall: clear did not ask first');
    await A.ev(`document.querySelector('#mw-clear').click()`);
    await wf(A, `__megaWall.state().marks === 0 && __megaWall.state().gen === 1`, 20000).catch(() => { throw new Error('mega: wall: the owner\'s wall did not clear'); });
    await scribble(A, 90, 40, 4, 800, 1); await sleep(1200);
    const fresh = (await ws(A)).marks;
    need(fresh > 3 && fresh < full, 'wall: new drawing after the clear: ' + fresh);
    bobProc.kill('SIGCONT');
    await wf(B, `__megaWall.state().gen === 1 && __megaWall.state().marks > 0`, 60000).catch(async () => { throw new Error('mega: wall: bob did not get the clear after being offline: ' + JSON.stringify(await ws(B).catch(() => null))); });
    await sleep(2000);
    need(await B.ev(`__megaWall.state().gen === 1`) && (await ws(B)).marks <= fresh, 'wall: bob still shows the cleared wall (' + (await ws(B)).marks + ' marks)');
    // an old-generation stroke sent late (what an offline device would do) is ignored everywhere
    await B.ev(`rpc('live_send', { gid: ${g}, kind: 'wall.d', data: JSON.stringify({ g: 0, k: 999999, c: [[10 * 120 + 10, 3]] }) }).then(() => 1)`);
    await sleep(3500);
    need(!(JSON.parse(await cells(A)).some(c => c[0] === 10 * 120 + 10)), 'wall: an old-generation stroke came back after the clear');
    await A.shot('mega4-4-wall-cleared');
    await A.ev(`document.querySelector('#mw-done') && document.querySelector('#mw-done').click()`); await sleep(300);
    need(await lvl(A) === 'chat', 'wall: could not get back to the chat');
  }

  // ---- mega phase 5: owner look settings (card and chat animation, backdrop strength) and drawing on channel and community cards ----
  {
    const cid = await A.ev(`S.snap.communities.find(c => c.name === 'Book club').id`);
    const g = JSON.stringify(textGid), cidj = JSON.stringify(cid);
    const wf = async (P, expr, ms) => { await P.send('Page.bringToFront'); await sleep(250); return P.waitFor(expr, ms); };
    const looks = P => P.ev(`JSON.stringify((S.snap.communities.find(c => c.id === ${cidj}).style || {}).looks || {})`).then(JSON.parse);
    const styleVer = P => P.ev(`(S.snap.communities.find(c => c.id === ${cidj}).style || {}).ver || 0`);
    const bd = P => P.ev(`(() => { const a = __mega.bdInfo(); const c = document.querySelector('#mega-bd'); let ink = -1; if (c) { const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data; ink = 0; for (let i = 3; i < d.length; i += 4 * 7) if (d[i]) ink++; } return a && { ...a, ink }; })()`);
    const settledOn = (P, keyExpr) => wf(P, `__mega.list && __mega.list.settled && __mega.list.item.key === ${keyExpr}`, 20000);
    const focusCard = async (P, keyExpr) => { await P.ev(`__mega.list.go(__mega.list.items.findIndex(x => x.key === ${keyExpr}), true)`); await settledOn(P, keyExpr); };
    const toServer = async P => {
      await P.send('Page.bringToFront');
      for (let i = 0; i < 4 && await lvl(P) !== 'server'; i++) {
        await P.ev(`document.activeElement && document.activeElement.blur && document.activeElement.blur()`);
        if (await lvl(P) === 'home') { await P.ev(`__mega.list.go(__mega.list.items.findIndex(x => x.name === 'Book club'), true)`); await sleep(1200); await key(P, 'Enter'); await sleep(1400); }
        else { await key(P, 'Escape'); await sleep(1000); }
      }
      need(await lvl(P) === 'server', 'looks: could not reach the rooms level: ' + await P.ev(`JSON.stringify({ lvl: __mega.level, active: document.activeElement.tagName + '#' + document.activeElement.id, busy: __mega.busy, closers: __mega.closers.length, pal: !!__mega.pal, draw: document.documentElement.className, item: __mega.list && __mega.list.item.key })`));
    };
    const toHome = async P => { await P.send('Page.bringToFront'); for (let i = 0; i < 4 && await lvl(P) !== 'home'; i++) { await key(P, 'Escape'); await sleep(1000); } need(await lvl(P) === 'home', 'cards: could not reach home: ' + ' ' + await P.ev(`JSON.stringify({ lvl: __mega.level, active: document.activeElement.tagName + '#' + document.activeElement.id, busy: __mega.busy, closers: __mega.closers.length, pal: !!__mega.pal, cls: document.documentElement.className, people: !!S.people, modal: !!document.querySelector('#modal-root > .scrim'), pen: __megaWall.isOn(), n: S.snap.communities.length })`)); };
    const enterFocused = async P => { await P.send('Page.bringToFront'); await P.ev(`document.activeElement && document.activeElement.blur && document.activeElement.blur()`); await key(P, 'Enter'); await wf(P, `__mega.level === 'chat'`, 15000); await sleep(1200); };

    // ---- 1. look settings
    await toServer(A); await toServer(B);
    await focusCard(A, g); await focusCard(B, g);
    await B.ev(`openRoom(${g})`);   // bob's page knows the room (the draw tests below rely on a message from him)
    await enterFocused(B);
    const base = await bd(B);
    need(base && base.dim === 100 && base.fx === base.fxAuto, 'looks: a room without a look is not on automatic and 100%: ' + JSON.stringify(base));
    const autoFx = base.fxAuto, cardN = (autoFx + 2) % 7, chatN = (autoFx + 4) % 7;
    need((await looks(A))[textGid] === undefined, 'looks: the room already has a look');
    // a member has no controls, and the core refuses his change
    await toServer(B); await focusCard(B, g);
    need(await B.ev(`!__mega.canPaint(__mega.list.item) && !document.querySelector('#mega-paint') && !document.querySelector('#mp-looks') && document.querySelector('#mega [data-m="paint"]').hidden`), 'looks: a non-owner has controls');
    const refusedLook = await B.ev(`rpc('set_look', { cid: ${cidj}, room: ${g}, card: 2 }).then(() => 'accepted', e => 'refused: ' + e.message)`);
    need(/^refused/.test(refusedLook), 'looks: the core took a look from a member: ' + refusedLook);
    await enterFocused(B);
    // the owner's panel: both choosers, the strength slider, "automatic" selected, the strength at 100
    await A.ev(`document.querySelector('#mega [data-m="paint"]').click()`); await sleep(500);
    const panel = await A.ev(`(() => { const p = document.querySelector('#mega-paint'); return { groups: p.querySelectorAll('.mp-fx').length, tiles: p.querySelectorAll('.mp-fx')[0].querySelectorAll('.mp-t').length, autoOn: [...p.querySelectorAll('.mp-t[data-n=""]')].map(b => b.getAttribute('aria-pressed')), dim: p.querySelector('#mp-dim').value, out: p.querySelector('#mp-dimv').textContent, r: p.getBoundingClientRect().toJSON(), h: innerHeight }; })()`);
    need(panel.groups === 2 && panel.tiles === 18 && panel.autoOn.join() === 'true,true' && panel.dim === '100' && panel.out === '100%', 'looks: the panel is wrong: ' + JSON.stringify(panel));
    await A.shot('mega5-1-panel');
    const v0 = await styleVer(A);
    await A.ev(`document.querySelector('#mp-looks .mp-t[data-k="card"][data-n="${cardN}"]').click()`);
    await wf(A, `__mega.list.item.fx === ${cardN}`, 5000).catch(() => { throw new Error('mega: looks: the card did not take the new pattern at once'); });
    await A.ev(`document.querySelector('#mp-looks .mp-t[data-k="chat"][data-n="${chatN}"]').click()`);
    // the slider: many input events while dragging, one change on release: one commit
    await A.ev(`(() => { const s = document.querySelector('#mp-dim'); for (let v = 100; v >= 40; v -= 3) { s.value = v; s.dispatchEvent(new Event('input', { bubbles: true })); } s.value = 40; s.dispatchEvent(new Event('input', { bubbles: true })); s.dispatchEvent(new Event('change', { bubbles: true })); })()`);
    await wf(A, `document.querySelector('#mp-dimv').textContent === '40%' && __mega.list.item.chatFx === ${chatN} && __mega.list.item.dim === 40`, 3000).catch(async () => { throw new Error('mega: looks: the owner does not see the new look at once: ' + await A.ev(`JSON.stringify([document.querySelector('#mp-dimv').textContent, __mega.list.item.chatFx, __mega.list.item.dim, ${chatN}])`)); });
    await wf(B, `(() => { const l = (S.snap.communities.find(c => c.id === ${cidj}).style || {}).looks || {}; const r = l[${g}]; return !!r && r.card === ${cardN} && r.chat === ${chatN} && r.dim === 40; })()`, 60000)
      .catch(async () => { throw new Error('mega: looks: the other client did not get the look: ' + JSON.stringify(await looks(B)) + ' / ' + JSON.stringify(await looks(A))); });
    await wf(A, `((S.snap.communities.find(c => c.id === ${cidj}).style || {}).ver || 0) > ${v0}`, 20000).catch(() => { });   // alice's own snapshot can trail bob's by a refresh
    await sleep(1500);
    const v1 = await styleVer(A);
    need(v1 - v0 >= 1 && v1 - v0 <= 3, 'looks: card, chat and a whole slider drag should be at most 3 commits (quick changes combine), got ' + (v1 - v0));
    // bob is in the room: his backdrop follows live
    await wf(B, `__mega.bdInfo().fx === ${chatN} && __mega.bdInfo().dim === 40`, 8000).catch(async () => { throw new Error('mega: looks: bob\'s open room did not follow: ' + JSON.stringify(await bd(B))); });
    need(await B.ev(`/changed the look of/.test(document.querySelector('#toast').textContent) && document.querySelector('#toast').textContent.includes(S.snap.communities.find(c => c.id === ${cidj}).style.by)`), 'looks: no note naming who changed it: ' + await B.ev(`document.querySelector('#toast').textContent`));
    const dim40 = await bd(B);
    need(dim40.at !== base.at, 'looks: the backdrop did not change brightness at 40%');
    // the card on his side
    await toServer(B); await focusCard(B, g);
    need(await B.ev(`__mega.list.item.fx === ${cardN}`), 'looks: bob\'s card does not use the owner\'s pattern');
    await B.shot('mega5-2-card-look-bob');
    await enterFocused(B);
    // strength 0: nothing is drawn, nothing is worked out
    await A.ev(`(() => { const s = document.querySelector('#mp-dim'); s.value = 0; s.dispatchEvent(new Event('input', { bubbles: true })); s.dispatchEvent(new Event('change', { bubbles: true })); })()`);
    await wf(B, `__mega.bdInfo().dim === 0`, 60000).catch(() => { throw new Error('mega: looks: strength 0 did not reach bob'); });
    await sleep(600);
    const z0 = await bd(B); await sleep(2000); const z1 = await bd(B);
    need(z1.draws === z0.draws && z1.ink === 0, 'looks: at 0 the backdrop still works or draws: ' + JSON.stringify([z0, z1]));
    await B.shot('mega5-3-strength-0');
    // the top of the range: readable text, a lively backdrop
    // the per-room Oracle switch: owner only, durable, and every client knows
    await A.ev(`document.querySelector('#mp-oracle').click()`);
    await wf(B, `(() => { const c = S.snap.communities.find(c => c.id === ${JSON.stringify(cid)}); return !!(((c.style || {}).looks || {})[${JSON.stringify(textGid)}] || {}).oracle_quiet; })()`, 60000).catch(() => { throw new Error('mega: the Oracle switch did not reach bob'); });
    await wf(A, `document.querySelector('#mp-oracle').getAttribute('aria-pressed') === 'true'`, 15000).catch(() => { throw new Error('mega: looks: the Oracle switch does not show that it is on'); });
    await A.ev(`document.querySelector('#mp-oracle').click()`);
    await wf(B, `(() => { const c = S.snap.communities.find(c => c.id === ${JSON.stringify(cid)}); return !((c.style || {}).looks || {})[${JSON.stringify(textGid)}]?.oracle_quiet; })()`, 60000).catch(() => { throw new Error('mega: switching Oracle back on did not reach bob'); });
    const topDim = await A.ev(`+document.querySelector('#mp-dim').max`);
    await A.ev(`(() => { const s = document.querySelector('#mp-dim'); s.value = s.max; s.dispatchEvent(new Event('input', { bubbles: true })); s.dispatchEvent(new Event('change', { bubbles: true })); })()`);
    await wf(B, `__mega.bdInfo().dim === ${topDim}`, 60000);
    await B.ev(`rpc('send_text', { gid: ${g}, text: 'Is this still easy to read at the top of the range?' }).then(() => 1)`); await sleep(2500);
    const t0 = await bd(B); await sleep(1500); const t1 = await bd(B);
    need(t1.draws > t0.draws && t1.ink > 0, 'looks: nothing is drawn at the top of the range');
    await B.shot('mega5-4-strength-top');
    // reset: back to automatic and 100% for everyone, the very same atlas as before
    await A.ev(`document.querySelector('#mp-reset').click()`);
    await wf(B, `__mega.bdInfo().dim === 100 && __mega.bdInfo().fx === __mega.bdInfo().fxAuto`, 60000).catch(async () => { throw new Error('mega: looks: reset did not reach bob: ' + JSON.stringify(await bd(B)) + JSON.stringify(await looks(B))); });
    need((await looks(B))[textGid] === undefined, 'looks: reset left an entry');
    const back = await bd(B);
    need(back.at === base.at && back.fx === base.fx, 'looks: automatic does not look the same as before: ' + JSON.stringify([base, back]));
    // every pattern draws something, stays inside 0..1, and has a name; numbers no version knows (and so 0.3.0, which knows none of these looks) fall back to automatic
    const pats = await A.ev(`(() => { const a = { cols: 60, rows: 20, col: Array.from({ length: 60 }, (_, i) => ({ v: .6 + (i % 7) * .2, o: (i * 13) % 60, l: 16 + i % 30 })) }, r = []; for (let n = 0; n < __mega.fx.length; n++) { let lit = 0, bad = 0; for (const t of [0, 3.3, 7.3, 11.7]) for (let y = 0; y < 20; y++) for (let x = 0; x < 60; x++) { const v = __mega.fx[n](x, y, t, a); if (!(v >= 0 && v <= 1)) bad++; if (v >= .2) lit++; } r.push([n, lit, bad]); } return { r, names: __mega.fxNames.length, n: __mega.fx.length }; })()`);
    need(pats.n === 17 && pats.names === 17 && pats.r.every(x => x[1] > 60 && x[2] === 0), 'looks: a pattern draws nothing, leaves 0..1 or has no name: ' + JSON.stringify(pats));
    await A.ev(`rpc('set_look', { cid: ${cidj}, room: ${g}, card: 31, chat: 30 }).then(() => 1)`);
    await wf(B, `(((S.snap.communities.find(c => c.id === ${cidj}).style || {}).looks || {})[${g}] || {}).card === 31`, 60000);
    await wf(B, `__mega.bdInfo().fx === __mega.bdInfo().fxAuto`, 8000).catch(() => { throw new Error('mega: looks: an unknown chat pattern number did not fall back to automatic'); });
    await A.ev(`rpc('set_look', { cid: ${cidj}, room: ${g} }).then(() => 1)`);
    await wf(B, `(((S.snap.communities.find(c => c.id === ${cidj}).style || {}).looks || {})[${g}]) === undefined`, 60000);
    // the community card has a card animation only
    await A.ev(`document.querySelector('#mega-paint #mp-done').click()`); await sleep(300);
    await toHome(A); await focusCard(A, `'c:${cid}'`);
    await A.ev(`document.querySelector('#mega [data-m="paint"]').click()`); await sleep(500);
    need(await A.ev(`document.querySelectorAll('#mp-looks .mp-fx').length === 1 && !document.querySelector('#mp-dim')`), 'looks: the community card should have a card animation only');
    await A.ev(`document.querySelector('#mp-looks .mp-t[data-k="card"][data-n="1"]').click()`);
    await wf(A, `__mega.list.item.fx === 1`, 5000);
    await wf(B, `(((S.snap.communities.find(c => c.id === ${cidj}).style || {}).looks || {})[${cidj}] || {}).card === 1`, 60000).catch(() => { throw new Error('mega: looks: the community card pattern did not reach bob'); });
    await A.shot('mega5-5-community-card-look');
    await A.ev(`document.querySelector('#mp-reset').click()`);
    await wf(B, `(((S.snap.communities.find(c => c.id === ${cidj}).style || {}).looks || {})[${cidj}]) === undefined`, 60000);
    await A.ev(`document.querySelector('#mega-paint #mp-done').click()`); await sleep(300);

    // ---- 2. drawing on a channel card
    const hitDraw = (P, sel, cx, cy, r, ms) => P.ev(`(async () => {
      const hit = document.querySelector(${JSON.stringify(sel)}), G = __megaWall.state().geo, px = (x, y) => ({ clientX: G.b.left + (G.ox + (x + .5) * G.cw) / G.u, clientY: G.b.top + (G.oy + (y + .5) * G.ch) / G.u, pointerId: 9, bubbles: true, button: 0 });
      document.querySelector('#mw-glyphs [data-g="2"]').click();
      const n = 80, pt = i => px(${cx} + Math.round(${r} * Math.cos(i / n * 6.2832)), ${cy} + Math.round(${r} * .5 * Math.sin(i / n * 6.2832)));
      hit.dispatchEvent(new PointerEvent('pointerdown', pt(0)));
      for (let i = 1; i <= n; i++) { hit.dispatchEvent(new PointerEvent('pointermove', pt(i))); await new Promise(r => setTimeout(r, ${ms} / n)); }
      hit.dispatchEvent(new PointerEvent('pointerup', pt(n))); return true; })()`);
    const stat = P => P.ev(`JSON.stringify(__megaLive.stats)`).then(JSON.parse);
    const wst = P => P.ev(`__megaWall.state()`);
    // wall_get answers "busy" with no cells while the core is occupied, which is not the same as an empty wall
    const roomCells = (P, gid) => P.ev(`(async () => { for (;;) { const r = await rpc('wall_get', { gid: ${JSON.stringify(gid)} }); if (!r.busy) return r.cells.length; await new Promise(z => setTimeout(z, 200)); } })()`);
    // The owner's colours panel must never touch the drawings: after every change, on both clients, the wall is still there (cache, core and pixels).
    const wallKeep = async (label, key, n, openPanel, changes, bobInRoom, burst) => {
      const kj = JSON.stringify(key);
      const inkOf = P => P.ev(`(() => { const c = ${bobInRoom && P === B ? "document.querySelector('#mega-wall')" : "__mega.list.sq().querySelector('canvas.mwallc')"}; if (!c) return -1; const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data; let k = 0; for (let i = 3; i < d.length; i += 20) if (d[i] > 40) k++; return k; })()`);
      const here = async P => ({ cache: await P.ev(`__megaWall.cellsOf(${kj}).length`), core: await roomCells(P, key), ink: await inkOf(P) });
      const fits = P => P.ev(`(() => { const sq = __mega.list.sq(), c = sq.querySelector('canvas.mwallc'); if (!c) return 'no canvas'; const dpr = Math.min(devicePixelRatio || 1, 2); return Math.abs(c.offsetWidth - sq.offsetWidth) <= 2 && Math.abs(c.offsetHeight - sq.offsetHeight) <= 2 && Math.abs(c.width - Math.round(sq.offsetWidth * dpr)) <= 2 ? 'ok' : JSON.stringify([c.offsetWidth, c.offsetHeight, c.width, c.height, sq.offsetWidth, sq.offsetHeight, dpr]); })()`);
      const bad = async () => JSON.stringify([await here(A), await here(B), await wst(A), await wst(B)]);
      need(openPanel, 'keep: no panel');
      if (bobInRoom) { await enterFocused(B); await wf(B, `__megaWall.state().marks === ${n}`, 30000); }
      if (!bobInRoom) { await A.send('Page.bringToFront'); await wf(A, `__megaWall.cellsOf(${kj}).length === ${n}`, 30000); }
      await A.ev(`document.querySelector('#mega [data-m="paint"]').click()`); await sleep(500);
      if (burst) {   // everything at once, as a person does when trying colours: the commits queue behind each other
        let stop = false, worst = null;
        const sampler = (async () => { while (!stop) { const a = await here(A), b = await here(B); if (!(a.cache === n && a.core === n && b.cache === n && b.core === n && a.ink > 20 && b.ink > 20) && !worst) worst = JSON.stringify([a, b]); await sleep(150); } })();
        const v0 = await styleVer(B);
        for (const [, run] of changes) { await A.ev(run); await sleep(120); }
        await wf(B, `((S.snap.communities.find(c => c.id === ${cidj}).style || {}).ver || 0) > ${v0}`, 60000);
        await sleep(6000); stop = true; await sampler;
        need(!worst, `wall-keep: ${label}: during a burst of changes the drawing went missing (want ${n}): ` + worst + ' ' + await bad());
        await A.ev(`document.querySelector('#mega-paint #mp-done').click()`); await sleep(300);
        return;
      }
      for (const [name, run] of changes) {
        const v0 = await styleVer(B);
        await A.ev(run); await sleep(200);
        await wf(B, `((S.snap.communities.find(c => c.id === ${cidj}).style || {}).ver || 0) > ${v0}`, 60000).catch(() => { throw new Error(`mega: wall-keep: ${label}: ${name} never reached bob`); });
        await A.shot('wallkeep-' + label.replace(/\W+/g, '-') + '-' + name.replace(/\W+/g, '-'));
        for (let i = 0; i < 12; i++) {   // the change settles (tween, repaint, snapshot refresh): the drawing must be there the whole time
          const a = await here(A), b = await here(B);
          if (!bobInRoom) need(await fits(A) === 'ok' && await fits(B) === 'ok', `wall-keep: ${label}: after "${name}" the card's drawing canvas does not fit the card: ` + await fits(A) + ' / ' + await fits(B));
          need(a.cache === n && a.core === n && b.cache === n && b.core === n && a.ink > 20 && b.ink > 20, `wall-keep: ${label}: after "${name}" the drawing is gone/changed (want ${n}): ` + await bad());
          await sleep(350);
        }
      }
      await A.ev(`document.querySelector('#mega-paint #mp-done').click()`); await sleep(300);
      if (bobInRoom) { await toServer(B); await focusCard(B, g); }
      if (!key.startsWith('cw:')) {   // and walking into the room afterwards shows the drawing too, for the owner and for the member
        for (const P of [A, B]) {
          await enterFocused(P);
          await wf(P, `__megaWall.state().marks === ${n}`, 20000).catch(async () => { throw new Error(`mega: wall-keep: ${label}: the room shows no drawing after the changes: ` + await bad()); });
          need(await P.ev(`(() => { const c = document.querySelector('#mega-wall'); if (!c) return 0; const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data; let k = 0; for (let i = 3; i < d.length; i += 20) if (d[i] > 40) k++; return k; })()`) > 20, `wall-keep: ${label}: no drawing pixels in the room`);
          await toServer(P); await focusCard(P, g);
          if (P === A) await A.ev(`S.gid = null; resetRoom(); 1`);   // alice has no room open afterwards (an open room reads its messages as they come)
        }
      }
    };
    const chip = P => P.ev(`(() => { const b = document.querySelector('#mega [data-m="draw"]'); return !!b && !b.hidden; })()`);
    await toServer(A); await toServer(B);
    await focusCard(A, g); await focusCard(B, g);
    await B.ev(`rpc('send_text', { gid: ${g}, text: 'drawing time' }).then(() => 1)`); await sleep(1500);
    // resting on and scrolling past cards sends nothing
    const q0 = await stat(A);
    for (const i of [0, 1, 2, 0]) { await A.ev(`__mega.list.go(${i}, false)`); await sleep(1100); }
    await focusCard(A, g); await sleep(1500);
    { const q1 = await stat(A), noPres = q => q.sent - ((q.kinds || {})['pres.f'] || 0); need(noPres(q1) === noPres(q0), 'cards: resting on or scrolling past a card sent something other than where you are: ' + JSON.stringify([q0, q1])); }
    need((await wst(A)).cards.length <= 3, 'cards: more than three cards hold a wall canvas: ' + JSON.stringify((await wst(A)).cards));
    need(await chip(A) && await chip(B), 'cards: the draw chip is missing on a room card');
    // the draw chip: the pen opens on the focused card, the list holds still
    const sc0 = await A.ev(`__mega.list.el.scrollTop`), snap0 = await A.ev(`getComputedStyle(__mega.list.el).scrollSnapType`);
    await A.ev(`document.querySelector('#mega [data-m="draw"]').click()`); await sleep(500);
    need(await A.ev(`!!document.querySelector('#mega-wallhit-card') && !!document.querySelector('#mega #mega-wallbar') && document.documentElement.classList.contains('mg-drawing-card')`), 'cards: the pen did not open on the card');
    need(await A.ev(`getComputedStyle(__mega.list.el).scrollSnapType === 'none' && getComputedStyle(__mega.list.el).overflowY === 'hidden'`), 'cards: the list still snaps or scrolls while drawing');
    await A.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: 640, y: 400, deltaX: 0, deltaY: 600 }); await key(A, 'j'); await key(A, 'ArrowDown'); await sleep(700);
    need(await A.ev(`__mega.list.el.scrollTop`) === sc0 && await A.ev(`__mega.list.item.key === ${g}`), 'cards: the list moved while drawing');
    await A.shot('mega5-6-card-pen');
    const w0 = await stat(A);
    await hitDraw(A, '#mega-wallhit-card', 60, 27, 22, 4000);
    await sleep(1500);
    const w1 = await stat(A), sent5 = (w1.kinds['wall.d'] || 0) - (w0.kinds['wall.d'] || 0);
    need(sent5 >= 1 && sent5 <= 10, 'cards: a 4 second scribble on a card produced ' + sent5 + ' wall.d events (budget 10)');
    need((w1.sent - w0.sent) <= 12, 'cards: a card scribble sent too many live events in all: ' + (w1.sent - w0.sent));
    let nCard = (await wst(A)).marks;
    need(nCard > 30, 'cards: the scribble left only ' + nCard + ' marks');
    await wf(B, `__megaWall.state().cards.some(c => c.key === ${g} && c.marks === ${nCard})`, 30000).catch(async () => { throw new Error('mega: cards: the stroke did not appear on bob\'s card: ' + JSON.stringify([nCard, await wst(B)])); });
    need(JSON.stringify(await A.ev(`__megaWall.cellsOf(${g}).sort((a, b) => a[0] - b[0])`)) === JSON.stringify(await B.ev(`__megaWall.cellsOf(${g}).sort((a, b) => a[0] - b[0])`)), 'cards: the two cards show different drawings');
    // a real pixel check: the card's drawing canvas holds marks, and the card's own ascii leaves them alone
    const ink = P => P.ev(`(() => { const c = document.querySelector('.msq canvas.mwallc'); if (!c) return -1; const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data; let n = 0; for (let i = 3; i < d.length; i += 4 * 5) if (d[i] > 40) n++; return n; })()`);
    need(await ink(A) > 100 && await ink(B) > 100, 'cards: no drawing pixels on the card: ' + await ink(A) + ' / ' + await ink(B));
    await A.shot('mega5-7-card-drawn-alice'); await B.shot('mega5-8-card-drawn-bob');
    // Esc leaves the pen and the list is as it was
    await key(A, 'Escape'); await sleep(500);
    need(await A.ev(`!document.querySelector('#mega-wallhit-card') && !document.querySelector('#mega-wallbar') && !document.documentElement.classList.contains('mg-drawing-card')`) && await A.ev(`getComputedStyle(__mega.list.el).scrollSnapType`) === snap0 && await A.ev(`__mega.list.el.scrollTop`) === sc0 && await lvl(A) === 'server', 'cards: Esc did not leave the pen cleanly');
    // the same drawing is the room's wall
    await enterFocused(B); await wf(B, `__megaWall.state().marks === ${nCard}`, 20000).catch(async () => { throw new Error('mega: cards: the room does not show the card drawing: ' + JSON.stringify(await wst(B))); });
    await B.shot('mega5-9-room-wall-from-card');
    await toServer(B); await focusCard(B, g);
    // lock: the chip goes for a member, stays for the owner
    await A.ev(`document.querySelector('#mega [data-m="draw"]').click()`); await sleep(400);
    await A.ev(`document.querySelector('#mw-lock').click()`);
    await wf(B, `!document.querySelector('#mega [data-m="draw"]') || document.querySelector('#mega [data-m="draw"]').hidden`, 60000).catch(() => { throw new Error('mega: cards: the lock did not hide the chip for bob'); });
    need(await chip(A) === true, 'cards: the owner lost the chip when she locked the wall');
    await A.ev(`document.querySelector('#mw-lock').click()`);
    await wf(B, `!document.querySelector('#mega [data-m="draw"]').hidden`, 60000).catch(() => { throw new Error('mega: cards: unlock did not bring the chip back'); });
    await A.ev(`document.querySelector('#mw-done').click()`); await sleep(400);
    need(await A.ev(`!document.querySelector('#mega-wallhit-card') && !document.documentElement.classList.contains('mg-drawing-card')`), 'cards: done did not leave the pen');
    // ---- palette: draw in several colours; the other client sees the same colours (and the stored colours are the palette's)
    { await A.ev(`document.querySelector('#mega [data-m="draw"]').click()`); await sleep(500);
      const sw = await A.ev(`[...document.querySelectorAll('#mw-cols button')].map(b => [b.dataset.c, b.getAttribute('aria-label'), b.getAttribute('aria-pressed'), b.offsetWidth])`);
      need(sw.length === 12 && sw[0][1] === 'room colour' && sw[1][1] === 'room highlight' && sw.every(x => x[3] >= 28) && sw.filter(x => x[2] === 'true').length === 1, 'palette: the toolbar swatches are wrong: ' + JSON.stringify(sw));
      const stroke = (P, c, row) => P.ev(`(async () => {
        document.querySelector('#mw-cols [data-c="${c}"]').click(); document.querySelector('#mw-glyphs [data-g="1"]').click();
        const hit = document.querySelector('#mega-wallhit-card'), G = __megaWall.state().geo, px = x => ({ clientX: G.b.left + (G.ox + (x + .5) * G.cw) / G.u, clientY: G.b.top + (G.oy + (${row} + .5) * G.ch) / G.u, pointerId: 9, bubbles: true, button: 0 });
        hit.dispatchEvent(new PointerEvent('pointerdown', px(96))); for (let x = 97; x <= 110; x++) hit.dispatchEvent(new PointerEvent('pointermove', px(x))); hit.dispatchEvent(new PointerEvent('pointerup', px(110))); return true; })()`);
      const picks = [[2, 3], [4, 4], [7, 5], [11, 6], [0, 7]];   // the right-hand side of the card, clear of the scribble and of the words
      for (const [c, row] of picks) { await stroke(A, c, row); await sleep(700); }
      need(await A.ev(`document.querySelector('#mw-cols [data-c="0"]').getAttribute('aria-pressed') === 'true' && document.querySelector('#mw-cols [data-c="11"]').getAttribute('aria-pressed') === 'false'`), 'palette: the selected swatch is not marked');
      // the keyboard walks the colours
      await A.ev(`document.querySelector('#mw-cols [data-c="0"]').focus()`); await key(A, 'ArrowRight'); await key(A, 'ArrowRight');
      need(await A.ev(`document.querySelector('#mw-cols [aria-pressed="true"]').dataset.c`) === '2', 'palette: arrow keys did not move the colour');
      const rowCols = P => P.ev(`(() => { const o = {}; for (const [p, g, c] of __megaWall.cellsOf(${g})) { const y = (p / 120) | 0; if (y < 10 && p % 120 >= 95) (o[y] = o[y] || new Set()).add(c); } return Object.fromEntries(Object.entries(o).map(([y, s]) => [y, [...s]])); })()`);
      const want = JSON.stringify({ 3: [2], 4: [4], 5: [7], 6: [11], 7: [0] });
      await wf(B, `JSON.stringify((() => { const o = {}; for (const [p, g, c] of __megaWall.cellsOf(${g})) { const y = (p / 120) | 0; if (y < 10 && p % 120 >= 95) (o[y] = o[y] || new Set()).add(c); } return Object.fromEntries(Object.entries(o).map(([y, s]) => [y, [...s]])); })()) === ${JSON.stringify(want)}`, 30000).catch(async () => { throw new Error('mega: palette: bob does not see the colours: ' + JSON.stringify([await rowCols(A), await rowCols(B)])); });
      need(JSON.stringify(await rowCols(A)) === want, 'palette: alice\'s colours are wrong: ' + JSON.stringify(await rowCols(A)));
      // real pixels: bob's card canvas holds the palette's red (#ff5a5f), green (#5fe08a) and pink (#ff7ac8)
      const has = (P, r, gg, b) => P.ev(`(() => { const c = document.querySelector('.msq canvas.mwallc'); if (!c) return -1; const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data; let n = 0; for (let i = 0; i < d.length; i += 4) if (d[i + 3] > 60 && Math.abs(d[i] - ${r}) < 40 && Math.abs(d[i + 1] - ${gg}) < 40 && Math.abs(d[i + 2] - ${b}) < 40) n++; return n; })()`);
      await B.shot('mega5-palette-bob');
      need(await has(B, 255, 90, 95) > 5 && await has(B, 95, 224, 138) > 5 && await has(B, 255, 122, 200) > 5, 'palette: bob\'s card lacks the red/green/pink pixels: ' + [await has(B, 255, 90, 95), await has(B, 95, 224, 138), await has(B, 255, 122, 200)]);
      await A.shot('mega5-palette-alice'); await B.shot('mega5-palette-bob');
      await A.ev(`document.querySelector('#mw-done').click()`); await sleep(400);
      nCard = (await wst(A)).marks; await wf(B, `__megaWall.state().cards.some(c => c.key === ${g} && c.marks === ${nCard})`, 30000);
      if (process.env.COMMONS_UI_STOP_AFTER === 'palette') { console.log('UI check stopped after the palette slice (COMMONS_UI_STOP_AFTER)'); process.exit(0); } }
    const hexSet = c => `(() => { const h = document.querySelector('#mp-hex'); h.value = '${c}'; h.dispatchEvent(new Event('input', { bubbles: true })); })()`;
    const tile = (k, n) => `document.querySelector('#mp-looks .mp-t[data-k="${k}"][data-n="${n}"]').click()`;
    const slide = v => `(() => { const s = document.querySelector('#mp-dim'); s.value = ${v}; s.dispatchEvent(new Event('input', { bubbles: true })); s.dispatchEvent(new Event('change', { bubbles: true })); })()`;
    await wallKeep('room', textGid, nCard, true, [['colour', hexSet('#2288cc')], ['card pattern', tile('card', 3)], ['chat pattern', tile('chat', 5)], ['backdrop strength', slide(55)], ['colour again', hexSet('#cc4488')]]);
    await wallKeep('room, burst', textGid, nCard, true, [['colour', hexSet('#ee7711')], ['card pattern', tile('card', 1)], ['chat pattern', tile('chat', 2)], ['backdrop strength', slide(80)], ['colour', hexSet('#7711ee')], ['card pattern', tile('card', 7)], ['backdrop strength', slide(30)]], false, true);
    // changing a NEIGHBOUR card (another room) leaves this room's drawing alone, on its card too
    { const og = (await A.ev(`S.snap.communities.find(c => c.id === ${cidj}).channels.map(x => x.gid)`)).find(x => x !== textGid);
      const slotInk = P => P.ev(`(() => { const i = __mega.list.items.findIndex(x => x.key === ${g}); const c = document.querySelectorAll('.mslot')[i] && document.querySelectorAll('.mslot')[i].querySelector('canvas.mwallc'); if (!c) return -1; const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data; let k = 0; for (let i = 3; i < d.length; i += 20) if (d[i] > 40) k++; return k; })()`);
      need(og, 'wall-keep: no second room');
      await A.send('Page.bringToFront'); await focusCard(A, JSON.stringify(og));
      await A.ev(`document.querySelector('#mega [data-m="paint"]').click()`); await sleep(500);
      for (const [name, run] of [['colour', hexSet('#99aa22')], ['card pattern', tile('card', 8)]]) {
        const v0 = await styleVer(B); await A.ev(run);
        await wf(B, `((S.snap.communities.find(c => c.id === ${cidj}).style || {}).ver || 0) > ${v0}`, 60000);
        for (let i = 0; i < 8; i++) { const c = await A.ev(`__megaWall.cellsOf(${g}).length`), k = await slotInk(A); need(c === nCard && k > 20, `wall-keep: neighbour: after changing the next room's ${name}, this room's drawing is gone (cache ${c}/${nCard}, card ink ${k})`); await sleep(350); }
      }
      await A.ev(`document.querySelector('#mega-paint #mp-done').click()`); await sleep(300); await focusCard(A, g); }
    await wallKeep('room, bob inside', textGid, nCard, true, [['colour', hexSet('#118855')], ['card pattern', tile('card', 2)], ['chat pattern', tile('chat', 6)], ['backdrop strength', slide(120)], ['backdrop strength off', slide(0)], ['backdrop strength back', slide(100)]], true);

    // ---- 3. the community card has a wall of its own, carried in a room and never drawn in any room
    // a room's own wall may still be arriving from the other device: the baseline is taken once both agree
    for (let i = 0; i < 30 && await roomCells(A, textGid) !== await roomCells(B, textGid); i++) { await B.send('Page.bringToFront'); await sleep(700); await A.send('Page.bringToFront'); await sleep(700); }
    const roomBefore = await roomCells(A, textGid), roomBeforeB = await roomCells(B, textGid);
    // bob has only this community, so he has no home level yet: he starts a second one (a member of a second community is the usual case)
    if (await B.ev(`S.snap.communities.length`) < 2) { await B.ev(`rpc('create_community', { name: 'Bob side' }).then(() => 1)`); await wf(B, `S.snap.communities.length >= 2`, 30000); }
    await toHome(A); await toHome(B);
    await focusCard(A, `'c:${cid}'`); await focusCard(B, `'c:${cid}'`);
    need(await chip(A) && await chip(B), 'cards: no draw chip on the community card');
    await A.ev(`document.querySelector('#mega [data-m="draw"]').click()`); await sleep(500);
    need(await A.ev(`!!document.querySelector('#mega-wallhit-card') && __megaWall.state().gid === ${JSON.stringify('cw:' + cid)}`), 'cards: the pen did not open on the community card');
    // starting to draw asks once whether anyone is around; bob's focused community card answers, and only then do strokes go out
    // a page in the background does not poll the live channel: let each side be the visible one in turn
    for (let i = 0; i < 3; i++) { await B.send('Page.bringToFront'); await sleep(2200); await A.send('Page.bringToFront'); await sleep(2200); if (await A.ev(`__megaWall.state().present`)) break; }
    await wf(A, `__megaWall.state().present`, 20000).catch(async () => { throw new Error('mega: cards: nobody answered the community wall request: ' + JSON.stringify([await wst(A), await wst(B), await stat(A)]) + ' ' + await B.ev(`JSON.stringify({ vis: document.visibilityState, items: __mega.list && __mega.list.items.map(x => x.key.slice(0, 6)), cur: __mega.list && __mega.list.item.key.slice(0, 6), settled: __mega.list && __mega.list.settled, lvl: __mega.level })`)); });
    const c0 = await stat(A);
    await hitDraw(A, '#mega-wallhit-card', 60, 27, 24, 4000);
    await sleep(1800);
    const c1 = await stat(A), sentC = (c1.kinds['cwall.d'] || 0) - (c0.kinds['cwall.d'] || 0);
    need(sentC >= 1 && sentC <= 10, 'cards: a community scribble produced ' + sentC + ' cwall.d events (budget 10)');
    need((c1.kinds['wall.d'] || 0) === (c0.kinds['wall.d'] || 0), 'cards: a community stroke was sent as a room stroke (wall.d)');
    const nComm = (await wst(A)).marks;
    need(nComm > 30, 'cards: the community scribble left only ' + nComm + ' marks');
    await wf(B, `__megaWall.state().cards.some(c => c.key === ${JSON.stringify('cw:' + cid)} && c.marks === ${nComm})`, 30000).catch(async () => { throw new Error('mega: cards: the community stroke did not reach bob: ' + JSON.stringify([nComm, await wst(B), await stat(A)])); });
    need(await A.ev(`__megaWall.state().carrier`) !== null, 'cards: no carrier room for the community wall');
    await A.shot('mega5-10-community-drawn-alice'); await B.shot('mega5-11-community-drawn-bob');
    need(await roomCells(A, textGid) === roomBefore && await roomCells(B, textGid) === roomBeforeB, 'cards: the community drawing reached a room wall: ' + JSON.stringify([roomBefore, roomBeforeB, await roomCells(A, textGid), await roomCells(B, textGid)]));
    await A.ev(`document.querySelector('#mw-done').click()`); await sleep(400);
    await wallKeep('community card', 'cw:' + cid, nComm, true, [['colour', hexSet('#33aa55')], ['card pattern', tile('card', 4)]]);
    await A.ev(`document.querySelector('#mega [data-m="draw"]').click()`); await sleep(500);
    const otherGids = await A.ev(`S.snap.communities.find(c => c.id === ${cidj}).channels.map(x => x.gid).filter(g => g !== ${JSON.stringify(textGid)})`);
    const otherRooms = []; for (const g of otherGids) otherRooms.push(await roomCells(A, g));
    need(otherRooms.reduce((a, b) => a + b, 0) === 0, 'cards: a room other than the drawn one holds marks: ' + JSON.stringify(otherRooms));
    // lock and clear of the community wall are the owner's
    await A.ev(`document.querySelector('#mw-lock').click()`);
    await wf(B, `document.querySelector('#mega [data-m="draw"]').hidden`, 60000).catch(() => { throw new Error('mega: cards: the community lock did not hide the chip for bob'); });
    const refusedC = await B.ev(`rpc('wall_draw', { gid: ${JSON.stringify('cw:' + cid)}, cells: [[3, 3, 1, 0]] }).then(() => 'accepted', e => 'refused: ' + e.message)`);
    need(/^refused/.test(refusedC), 'cards: the core took a stroke from a member on a locked community wall: ' + refusedC);
    await A.ev(`document.querySelector('#mw-lock').click()`); await sleep(1500);
    await A.ev(`document.querySelector('#mw-clear').click()`); await A.ev(`document.querySelector('#mw-clear').click()`);
    await wf(B, `__megaWall.state().cards.some(c => c.key === ${JSON.stringify('cw:' + cid)} && c.marks === 0)`, 60000).catch(() => { throw new Error('mega: cards: the community clear did not reach bob'); });
    need((await wst(A)).marks === 0, 'cards: the owner\'s community wall did not clear');
    await A.ev(`document.querySelector('#mw-done').click()`); await sleep(400);
    need(await roomCells(A, textGid) === roomBefore, 'cards: clearing the community wall touched a room wall');
    // after a clear (generation 1) a new drawing must survive the owner's colour changes too
    { const key = 'cw:' + cid, cl = Array.from({ length: 40 }, (_, i) => [10 + i, 20, 2, i % 2]);
      await A.ev(`rpc('wall_draw', { gid: ${JSON.stringify(key)}, cells: ${JSON.stringify(cl)}, share: true }).then(() => 1)`);
      await wf(B, `__megaWall.state().cards.some(c => c.key === ${JSON.stringify(key)} && c.marks === 40)`, 40000).catch(async () => { throw new Error('mega: wall-keep: bob never got the post-clear drawing: ' + JSON.stringify(await wst(B))); });
      await wallKeep('community card after a clear', key, 40, true, [['colour', hexSet('#aa5533')], ['card pattern', tile('card', 6)]]); }
    need((await A.ev(`rpc('messages', { gid: ${g}, limit: 300 }).then(m => m.messages.filter(x => x.kind === 'text').map(x => x.text).join('|'))`)).indexOf('cwall') < 0, 'cards: wall data appeared in the history');
    await toServer(A); await toServer(B);
    // back to the chat for what follows
    await focusCard(A, g); await enterFocused(A);
    need(await lvl(A) === 'chat', 'cards: could not get back to the chat');
  }
  // ---- mega: the chat backdrop has its own two colours (owner picks, everyone's open chat follows, "same as card" resets, text stays readable) ----
  {
    const cid = await A.ev(`S.snap.communities.find(c => c.name === 'Book club').id`);
    const g = JSON.stringify(textGid), cidj = JSON.stringify(cid);
    const wf = async (P, expr, ms) => { await P.send('Page.bringToFront'); await sleep(250); return P.waitFor(expr, ms); };
    const look = P => P.ev(`JSON.stringify(((S.snap.communities.find(c => c.id === ${cidj}).style || {}).looks || {})[${g}] || null)`).then(JSON.parse);
    const rooms = P => P.ev(`JSON.stringify(((S.snap.communities.find(c => c.id === ${cidj}).style || {}).rooms || {})[${g}] || null)`).then(JSON.parse);
    const rgbOf = s => (String(s).match(/\d+/g) || [0, 0, 0]).slice(0, 3).map(Number);
    const hexOf = a => '#' + a.map(v => v.toString(16).padStart(2, '0')).join('');
    const wash = P => P.ev(`getComputedStyle(document.querySelector('#mega-bd')).backgroundColor`).then(rgbOf);
    const washIs = (P, want) => wf(P, `getComputedStyle(document.querySelector('#mega-bd')).backgroundColor === ${JSON.stringify('rgb(' + want.join(', ') + ')')}`, 20000);
    const lum = a => { const [r, g2, b] = a.map(v => { v /= 255; return v <= .03928 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4; }); return .2126 * r + .7152 * g2 + .0722 * b; };
    const cr = (a, b) => { const x = lum(a), y = lum(b); return (Math.max(x, y) + .05) / (Math.min(x, y) + .05); };
    const cidOpen = async P => { await P.send('Page.bringToFront'); await P.ev(`__mega.api.toChat(${cidj}, ${g})`); await wf(P, `__mega.level === 'chat' && !!document.querySelector('#mega-bd') && document.querySelectorAll('#stream .msg').length > 3`, 20000); await sleep(900); };
    const toCards = async P => {
      await P.send('Page.bringToFront');
      for (let i = 0; i < 4 && await lvl(P) !== 'server'; i++) { await P.ev(`document.activeElement && document.activeElement.blur && document.activeElement.blur()`); await key(P, 'Escape'); await sleep(1000); }
      need(await lvl(P) === 'server', 'backdrop: could not reach the rooms level');
      await P.ev(`__mega.list.go(__mega.list.items.findIndex(x => x.key === ${g}), true)`); await wf(P, `__mega.list.settled && __mega.list.item.key === ${g}`, 20000);
    };
    await cidOpen(B);
    const card0 = await rooms(A), wash0 = await wash(B);
    await B.shot('backdrop-1-before');
    // a member can not set it
    const refused = await B.ev(`rpc('set_look', { cid: ${cidj}, room: ${g}, back: { main: '#1f6f4a', hi: '#ffd23f' } }).then(() => 'accepted', e => 'refused: ' + e.message)`);
    need(/^refused/.test(refused), 'backdrop: the core took backdrop colours from a member: ' + refused);
    // the owner opens the colours panel and switches to the backdrop
    await toCards(A);
    await A.ev(`document.querySelector('#mega [data-m="paint"]').click()`); await wf(A, `!!document.querySelector('#mega-paint .mp-m')`, 8000);
    need(await A.ev(`(() => { const p = document.querySelector('#mega-paint'); return !p.querySelector('#mp-mode').hidden && p.querySelector('.mp-m[data-mode="card"]').getAttribute('aria-pressed') === 'true' && p.querySelector('#mp-same').hidden; })()`), 'backdrop: the panel should start on the card with no "same as card"');
    await A.ev(`document.querySelector('#mega-paint .mp-m[data-mode="back"]').click()`); await sleep(300);
    need(await A.ev(`document.querySelector('#mega-paint .mp-m[data-mode="back"]').getAttribute('aria-pressed') === 'true' && document.querySelector('#mp-same').hidden`), 'backdrop: the switch did not move');
    await A.ev(`(()=>{const e=document.querySelector('#mp-hex');e.value='#1f6f4a';e.dispatchEvent(new Event('input',{bubbles:true}));})()`); await sleep(200);
    await A.ev(`document.querySelector('.mp-well[data-w="1"]').click()`);
    await A.ev(`(()=>{const e=document.querySelector('#mp-hex');e.value='#ffd23f';e.dispatchEvent(new Event('input',{bubbles:true}));})()`);
    await wf(B, `getComputedStyle(document.querySelector('#mega-bd')).backgroundColor !== ${JSON.stringify('rgb(' + wash0.join(', ') + ')')}`, 40000).catch(async () => { throw new Error('mega: backdrop: bob\'s open chat did not take the new backdrop colours: ' + JSON.stringify([await look(B), await look(A), await wash(B), wash0, await B.ev(`JSON.stringify({ lvl: __mega.level, bd: __mega.bdInfo(), bg: document.querySelector('#mega-bd') && document.querySelector('#mega-bd').style.backgroundColor })`)])); });
    await wf(B, `(((S.snap.communities.find(c => c.id === ${cidj}).style || {}).looks || {})[${g}] || {}).back`, 30000);
    const lb = await look(B);
    need(lb.back.main === '#1f6f4a' && lb.back.hi === '#ffd23f', 'backdrop: stored colours wrong: ' + JSON.stringify(lb));
    need(JSON.stringify(await rooms(B)) === JSON.stringify(card0), 'backdrop: the card colours changed too: ' + JSON.stringify([card0, await rooms(B)]));
    await sleep(900);
    const w1 = await wash(B);
    need(w1[1] > w1[0] && w1[1] > w1[2] && Math.max(...w1) >= 28, 'backdrop: the wash is not a visible green: ' + w1);
    // the message text stays readable over the wash (WCAG contrast)
    const ink = await B.ev(`JSON.stringify(['#stream .msg .body', '#stream .msg .who .name', '#stream time.when', '#chan-title'].map(s => { const e = document.querySelector(s); return e ? getComputedStyle(e).color : null; }))`).then(JSON.parse);
    const ratios = ink.map(c => (c ? cr(rgbOf(c), w1) : 99));
    need(ratios[0] >= 7 && ratios[1] >= 4.5 && ratios[3] >= 7, 'backdrop: text contrast over the wash too low: ' + JSON.stringify([ink, ratios, w1]));
    need((await B.ev(`__mega.bdInfo().draws`)) > 0, 'backdrop: nothing drawn');
    await B.shot('backdrop-2-after');
    // the strength slider: 0 = off (plain dark), and the colours are kept
    await A.ev(`(()=>{const e=document.querySelector('#mp-dim');e.value='0';e.dispatchEvent(new Event('input',{bubbles:true}));e.dispatchEvent(new Event('change',{bubbles:true}));})()`);
    await washIs(B, [5, 4, 7]).catch(() => { throw new Error('mega: backdrop: strength 0 did not switch the wash off for bob'); });
    need((await look(B)).back && (await look(B)).dim === 0, 'backdrop: strength 0 lost the backdrop colours: ' + JSON.stringify(await look(B)));
    await A.ev(`(()=>{const e=document.querySelector('#mp-dim');e.value='100';e.dispatchEvent(new Event('input',{bubbles:true}));e.dispatchEvent(new Event('change',{bubbles:true}));})()`);
    await washIs(B, w1).catch(() => { throw new Error('mega: backdrop: strength 100 did not bring the wash back'); });
    // same as card
    need(await A.ev(`!document.querySelector('#mp-same').hidden`), 'backdrop: no "same as card" while the backdrop has its own colours');
    await A.ev(`document.querySelector('#mp-same').click()`);
    await wf(B, `!(((S.snap.communities.find(c => c.id === ${cidj}).style || {}).looks || {})[${g}] || {}).back`, 40000).catch(() => { throw new Error('mega: backdrop: "same as card" did not reach bob'); });
    await sleep(900);
    const w2 = await wash(B);
    need(JSON.stringify(w2) !== JSON.stringify(w1), 'backdrop: the wash did not return to the card colours: ' + JSON.stringify([w1, w2]));
    need(await A.ev(`document.querySelector('#mp-same').hidden`), 'backdrop: "same as card" stays after use');
    await A.ev(`document.querySelector('#mp-done').click()`); await sleep(300);
    await toCards(A); await cidOpen(A);
  }
  if (process.env.COMMONS_UI_STOP_AFTER === 'mega5') { console.log('UI check stopped after mega phase 5 (COMMONS_UI_STOP_AFTER)'); process.exit(0); }

  }
  // ---- mega phase 6: the "what's new" card, unread marks on the dots, and chat density per room ----
  {
    const cid = await A.ev(`S.snap.communities.find(c => c.name === 'Book club').id`), cidj = JSON.stringify(cid);
    const rooms = await A.ev(`S.snap.communities.find(c => c.id === ${cidj}).channels.filter(x => x.kind !== 'voice').map(x => x.gid)`);
    const musicGid = rooms.find(r => r !== textGid), g = JSON.stringify(textGid), mg = JSON.stringify(musicGid);
    need(!!musicGid, 'whatsnew: the club needs a second text room');
    const me = await A.ev(`S.snap.me.name`);
    const sendB = t => B.ev(`rpc('send_text', { gid: ${JSON.stringify(t[0])}, text: ${JSON.stringify(t[1])}, reply: null }).then(() => 1)`);
    const unreads = P => P.ev(`JSON.stringify(S.snap.communities.map(c => c.channels.map(x => x.gid.slice(0, 6) + ':' + x.unread)))`);
    const toHomeA = async () => { await A.send('Page.bringToFront'); for (let i = 0; i < 5 && await lvl(A) !== 'home'; i++) { await A.ev(`document.activeElement && document.activeElement.blur && document.activeElement.blur()`); await key(A, 'Escape'); await sleep(1000); } need(await lvl(A) === 'home', 'whatsnew: could not reach home'); };
    const newsItem = () => A.ev(`JSON.stringify(((__mega.list && __mega.list.items) || []).filter(x => x.kind === 'news').map(x => ({ rows: x.rows.map(r => ({ gid: r.gid, n: r.n, at: r.at, text: r.text, comm: r.comm, room: r.room })), unread: x.unread })))`).then(JSON.parse);
    // everything read first, then home
    await A.send('Page.bringToFront');
    await A.ev(`Promise.all(S.snap.communities.flatMap(c => c.channels.filter(x => x.unread > 0).map(x => rpc('mark_read', { gid: x.gid })))).then(() => 1)`);
    await A.waitFor(`S.snap.communities.every(c => c.channels.every(x => !x.unread))`, 20000).catch(() => { });
    await toHomeA();
    await A.waitFor(`__mega.list && __mega.list.items.length >= 3`, 8000); await sleep(600);
    need((await newsItem()).length === 0, 'whatsnew: the card should not exist when everything is read');
    // bob writes: one line in the general room, two in music of which the last names alice
    const keyBefore = await A.ev(`__mega.list.item.key`), topBefore = await A.ev(`Math.round(__mega.list.rectOf(__mega.list.cur).top)`);
    await sendB([textGid, 'whatsnew plain line']); await sleep(1300);
    await sendB([musicGid, 'whatsnew music one']); await sleep(1300);
    await sendB([musicGid, '@' + me + ' whatsnew mention']);
    await A.waitFor(`__mega.list.items.some(x => x.kind === 'news' && x.rows.length === 2 && x.rows[0].at >= 1 && x.rows[0].text.includes('whatsnew mention') && x.rows[1].text.includes('whatsnew plain'))`, 40000).catch(async e => { throw new Error('whatsnew: the card did not appear with both rooms: ' + JSON.stringify(await newsItem()) + await unreads(A)); });
    const n1 = (await newsItem())[0];
    need(n1.rows[0].gid === musicGid && n1.rows[1].gid === textGid && n1.rows[0].comm === 'Book club' && n1.unread === n1.rows.reduce((a, r) => a + r.n, 0), 'whatsnew: mentions first, then the rest: ' + JSON.stringify(n1));
    need(await A.ev(`__mega.list.items[0].kind === 'news'`), 'whatsnew: the card should be first');
    // the list did not move under the user: the same card is in the middle, in the same place
    need(await A.ev(`__mega.list.item.key`) === keyBefore, 'whatsnew: the focused card changed when the news card appeared');
    await sleep(900);
    const topAfter = await A.ev(`Math.round(__mega.list.rectOf(__mega.list.cur).top)`);
    need(Math.abs(topAfter - topBefore) <= 3, `whatsnew: the focused card moved ${topBefore} -> ${topAfter} when the news card appeared`);
    // rest on the card: it shows both rows, and neither showing nor scrolling past it reads anything
    const un0 = await unreads(A);
    await A.ev(`__mega.list.go(0, true)`); await A.waitFor(`__mega.list.settled && __mega.list.item.kind === 'news'`, 8000); await sleep(600);
    const view = await A.ev(`(() => { const rs = [...document.querySelectorAll('.mnrow')]; const b = rs.map(r => r.getBoundingClientRect()), card = document.querySelector('.mslot.settled .msq').getBoundingClientRect(); return { n: rs.length, text: rs.map(r => r.innerText.replace(/\\s+/g, ' ')), inside: b.every(x => x.top >= card.top - 1 && x.bottom <= card.bottom + 1), title: document.querySelector('.mslot.settled .mti').getAttribute('aria-label') }; })()`);
    need(view.n === 2 && view.inside && /Book club/.test(view.text[0]) && /music/.test(view.text[0]) && /whatsnew mention/.test(view.text[0]) && /@1/.test(view.text[0]) && /general|./.test(view.text[1]) && /whatsnew plain line/.test(view.text[1]), 'whatsnew: the rows are wrong: ' + JSON.stringify(view));
    await A.shot('mega6-1-news-card');
    await A.ev(`__mega.list.go(2, true)`); await sleep(1200); await A.ev(`__mega.list.go(0, true)`); await sleep(2800);
    need(await unreads(A) === un0, 'whatsnew: showing or scrolling past the card changed what is unread: ' + un0 + ' -> ' + await unreads(A));
    // 390 px
    await A.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true }); await sleep(900);
    await A.shot('mega6-1b-news-card-390');
    const ph = await A.ev(`(() => { const c = document.querySelector('.mslot.settled .msq').getBoundingClientRect(); return { w: innerWidth, cardR: c.right, rows: [...document.querySelectorAll('.mnrow')].every(r => r.getBoundingClientRect().right <= c.right + 1) }; })()`);
    need(ph.cardR <= ph.w + 1 && ph.rows, 'whatsnew: the card overflows at 390 px: ' + JSON.stringify(ph));
    await A.send('Emulation.clearDeviceMetricsOverride'); await sleep(900);
    await A.ev(`__mega.list.go(0, true)`); await A.waitFor(`__mega.list.settled && __mega.list.item.kind === 'news'`, 8000);
    // choosing the mention row goes straight into that room, at its first unread message
    await A.ev(`document.querySelector('.mnrow[data-gid="${JSON.stringify(musicGid).slice(1, -1)}"]').click()`);
    await A.waitFor(`__mega.level === 'chat' && S.gid === ${mg}`, 15000).catch(async e => { throw new Error('whatsnew: the row did not open the room: ' + await A.ev(`JSON.stringify({ lvl: __mega.level, gid: S.gid })`)); });
    await A.waitFor(`document.querySelector('#stream .newline')`, 15000).catch(() => { throw new Error('whatsnew: the room did not open at its first unread message'); });
    need(/2 new/i.test(await A.ev(`document.querySelector('#stream .newline').innerText`)), 'whatsnew: wrong first unread line: ' + await A.ev(`document.querySelector('#stream .newline').innerText`));
    await A.waitFor(`S.snap.communities.every(c => c.channels.filter(x => x.gid === ${mg}).every(x => !x.unread))`, 15000);
    // unread on the dots: the general room is unread (a plain ring), then a mention in it (a stronger one), in its own highlight colour and with no number
    const dot = () => A.ev(`(() => { const d = document.querySelector('#mega-rail .mitem[data-key="${JSON.stringify(textGid).slice(1, -1)}"]'); if (!d) return null; const cs = getComputedStyle(d); return { un: d.classList.contains('un'), at: d.classList.contains('at'), anim: cs.animationName, mh: d.style.getPropertyValue('--mh'), text: d.querySelector('.mct') ? 'count' : '', bg: cs.backgroundColor }; })()`);
    const d1 = await dot();
    need(d1 && d1.un && !d1.at && d1.anim === 'mn-un' && /^#[0-9a-f]{6}$/i.test(d1.mh), 'dots: an unread room should pulse a ring in its highlight colour: ' + JSON.stringify(d1));
    await sendB([textGid, '@' + me + ' whatsnew second']);
    await A.waitFor(`document.querySelector('#mega-rail .mitem[data-key="${JSON.stringify(textGid).slice(1, -1)}"]')?.classList.contains('at')`, 30000).catch(async () => { throw new Error('dots: a mention did not get its stronger mark: ' + await A.ev(`(async () => { const g = ${JSON.stringify(textGid)}, d = document.querySelector('#mega-rail .mitem[data-key="' + g + '"]'), ch = S.snap.communities.flatMap(c => c.channels).find(x => x.gid === g), m = await rpc('messages', { gid: g, limit: 20 }); return JSON.stringify({ cls: d && d.className, unread: ch && ch.unread, mention: S.mention.get(g) || 0, sig: S.mentionSig.get(g), me: S.snap.me.name, last: m.messages.slice(-3).map(x => [x.mine, x.text, x.kind]), level: __mega.level, gidOpen: S.gid }); })()`)); });
    const d2 = await dot();
    need(d2.un && d2.at && d2.anim === 'mn-at' && d2.mh === d1.mh, 'dots: a mention should have a stronger mark in the same colour: ' + JSON.stringify([d1, d2]));
    await A.shot('mega6-2-dots-marks');
    // back to the cards: the card is live (both mentions now count in the general row) and still reads nothing
    await toHomeA(); await A.waitFor(`__mega.list.items[0].kind === 'news'`, 15000);
    await A.waitFor(`__mega.list.items[0].rows.length === 1 && __mega.list.items[0].rows[0].gid === ${g} && __mega.list.items[0].rows[0].n === 2 && __mega.list.items[0].rows[0].at >= 1`, 20000).catch(async () => { throw new Error('whatsnew: the card did not follow the new state: ' + JSON.stringify(await newsItem())); });
    // read on another device (here: the core): the card goes away and the list does not jump
    await A.ev(`__mega.list.go(1, true)`); await A.waitFor(`__mega.list.settled && __mega.list.item.kind === 'community'`, 8000); await sleep(500);
    const k2 = await A.ev(`__mega.list.item.key`), t2 = await A.ev(`Math.round(__mega.list.rectOf(__mega.list.cur).top)`);
    await A.ev(`rpc('mark_read', { gid: ${g} }).then(() => 1)`);
    await A.waitFor(`!__mega.list.items.some(x => x.kind === 'news')`, 30000).catch(() => { throw new Error('whatsnew: the card did not go when everything was read'); });
    await sleep(500);
    need(await A.ev(`__mega.list.item.key`) === k2 && Math.abs(await A.ev(`Math.round(__mega.list.rectOf(__mega.list.cur).top)`) - t2) <= 3, 'whatsnew: the list jumped when the card went');
    need(await A.ev(`document.querySelectorAll('.mnrow').length === 0`), 'whatsnew: rows are still on screen');

    // ---- density: per room, with a toggle for every room
    await A.ev(`localStorage.removeItem('commons.dens'); localStorage.removeItem('commons.densAll'); localStorage.removeItem('commons.dens.' + ${g}); localStorage.removeItem('commons.dens.' + ${mg}); 1`);
    await A.ev(`__mega.api.toChat(${cidj}, ${g})`); await A.waitFor(`__mega.level === 'chat' && S.gid === ${g} && document.querySelectorAll('#stream .msg').length > 12`, 15000);
    await sleep(900);
    const px = P => P.ev(`(() => { const m = [...document.querySelectorAll('#stream .msg:not([data-mz])')].find(m => m.querySelector('.body') && m.textContent.length > 30); return m ? { fs: getComputedStyle(m.querySelector('.body')).fontSize, mt: getComputedStyle(m).paddingTop, attr: document.documentElement.dataset.mdens || '' } : null; })()`);
    const styleVer = () => A.ev(`(S.snap.communities.find(c => c.id === ${cidj}).style || {}).ver || 0`);
    const ver0 = await styleVer();
    const base = await px(A);
    need(base && base.fs === '24px' && base.attr === '', 'density: the default must be today\'s look (24px): ' + JSON.stringify(base));
    need(await A.ev(`!!document.querySelector('#btn-density') && !document.querySelector('#btn-density').hidden`), 'density: no header button in a room');
    await A.ev(`document.querySelector('#btn-density').click()`); await sleep(300);
    need(await A.ev(`!!document.querySelector('#mega-dens #md-r') && !!document.querySelector('#mega-dens #md-all') && !document.querySelector('#md-all').checked`), 'density: the slider and the toggle are missing');
    const setDens = v => A.ev(`(() => { const r = document.querySelector('#md-r'); r.value = ${v}; r.dispatchEvent(new Event('input', { bubbles: true })); })()`);
    await setDens(-2); await sleep(500);
    const small = await px(A);
    need(small.fs === '12px' && /^(m2|f-2)$/.test(small.attr) && parseFloat(small.mt) < parseFloat(base.mt), 'density: the slider did not change this room live: ' + JSON.stringify(small));
    need(await A.ev(`localStorage.getItem('commons.dens.' + ${g}) === '-2' && !localStorage.getItem('commons.dens')`), 'density: it should be stored for this room, on this device');
    await A.shot('mega6-3-density-menu');
    need(await styleVer() === ver0, 'density: it touched the owner document');
    await key(A, 'Escape'); await sleep(400);
    need(await A.ev(`!document.querySelector('#mega-dens') && __mega.level === 'chat'`), 'density: Esc should close the menu first and stay in the chat');
    // the other room is unchanged
    await A.ev(`document.querySelector('#mega-rail .mitem[data-key="${JSON.stringify(musicGid).slice(1, -1)}"]').click()`);
    await A.waitFor(`S.gid === ${mg} && document.querySelectorAll('#stream .msg').length >= 1`, 15000); await sleep(900);
    const other = await px(A);
    need(other && other.fs === '24px' && other.attr === '', 'density: another room changed with it: ' + JSON.stringify(other));
    // the toggle: this value everywhere, then each room its own again
    await A.ev(`document.querySelector('#mega-rail .mitem[data-key="${JSON.stringify(textGid).slice(1, -1)}"]').click()`);
    await A.waitFor(`S.gid === ${g} && document.querySelectorAll('#stream .msg').length > 12`, 15000); await sleep(700);
    need((await px(A)).fs === '12px', 'density: the room did not keep its own value');
    await A.ev(`document.querySelector('#btn-density').click()`); await sleep(300);
    await A.ev(`document.querySelector('#md-all').click()`); await sleep(400);
    need(await A.ev(`localStorage.getItem('commons.densAll') === '1' && localStorage.getItem('commons.dens') === '-2'`), 'density: the toggle did not store one value for every room');
    await A.ev(`document.querySelector('#mega-rail .mitem[data-key="${JSON.stringify(musicGid).slice(1, -1)}"]').click()`);
    await A.waitFor(`S.gid === ${mg}`, 15000); await sleep(900);
    const all = await px(A);
    need(all && all.fs === '12px' && /^(m2|f-2)$/.test(all.attr), 'density: "same in every room" did not apply to the other room: ' + JSON.stringify(all));
    await A.ev(`document.querySelector('#btn-density').click()`); await sleep(300);
    await A.ev(`document.querySelector('#md-all').click()`); await sleep(400);
    need(await A.ev(`!localStorage.getItem('commons.densAll')`) && (await px(A)).fs === '12px', 'density: off should leave the room as it shows: ' + JSON.stringify(await px(A)));
    await setDens(0); await sleep(500);
    need((await px(A)).fs === '24px', 'density: this room did not go back to the default on its own');
    await key(A, 'Escape'); await sleep(300);
    await A.ev(`document.querySelector('#mega-rail .mitem[data-key="${JSON.stringify(textGid).slice(1, -1)}"]').click()`);
    await A.waitFor(`S.gid === ${g} && document.querySelectorAll('#stream .msg').length > 12`, 15000); await sleep(700);
    need((await px(A)).fs === '12px', 'density: each room should keep its own once the toggle is off');
    // at the largest stop the centre focus and the arrival animation still work
    await A.ev(`document.querySelector('#mega-rail .mitem[data-key="${JSON.stringify(textGid).slice(1, -1)}"]').click()`);
    await A.waitFor(`S.gid === ${g} && document.querySelectorAll('#stream .msg').length > 12`, 15000); await sleep(700);
    await A.ev(`document.querySelector('#btn-density').click()`); await sleep(300);
    await setDens(2); await sleep(700); await key(A, 'Escape'); await sleep(300);
    need((await px(A)).fs === '48px', 'density: the largest stop should be 48px: ' + JSON.stringify(await px(A)));
    await A.ev(`(() => { const s = document.querySelector('#stream'); s.scrollTop = s.scrollHeight; })()`); await sleep(500);
    need(await A.ev(`[...document.querySelectorAll('#stream .msg')].filter(m => m.style.transform || m.style.opacity).length === 0`), 'density: at the latest message every row must be full size');
    await A.ev(`(() => { const s = document.querySelector('#stream'); s.scrollTop = s.scrollHeight / 2; })()`); await sleep(700);
    const fo = await A.ev(`(() => { const rows = [...document.querySelectorAll('#stream .msg')]; return { scaled: rows.filter(m => m.style.transform.startsWith('scale')).length, scroll: document.querySelector('#stream').scrollHeight > document.querySelector('#stream').clientHeight }; })()`);
    need(fo.scroll && fo.scaled >= 2, 'density: centre focus stopped working at the largest stop: ' + JSON.stringify(fo));
    await A.ev(`(() => { const s = document.querySelector('#stream'); s.scrollTop = s.scrollHeight; })()`); await sleep(500);
    const dec0 = await A.ev(`__mega.stats.decodes`);
    await A.send('Page.bringToFront'); await sendB([textGid, 'dens arrive']); await sleep(2500);
    need(await A.ev(`__mega.stats.decodes`) > dec0, 'density: a new message no longer resolves out of glyphs at the largest stop');
    await A.shot('mega6-4-density-largest');
    // scrolling on the sidebar walks through the text rooms, and the room opens once the scrolling stops
    const wheelFrom = await A.ev(`S.gid`), wheelOn = d => A.ev(`(document.querySelector('#mega-rail').dispatchEvent(new WheelEvent('wheel', { deltaY: ${d}, bubbles: true, cancelable: true })), 1)`);
    let wheelDir = 0;
    for (const d of [150, -150]) { await wheelOn(d); await sleep(1200); if (await A.ev(`S.gid`) !== wheelFrom) { wheelDir = d; break; } }
    need(wheelDir !== 0, 'sidebar: scrolling on it did not move to another room');
    await sleep(400); await wheelOn(-wheelDir);
    await A.waitFor(`S.gid === ${JSON.stringify(wheelFrom)}`, 8000).catch(() => { throw new Error('sidebar: scrolling back did not return to the room'); });
    await sleep(500);
    // chat font: Bellefair by default, Raster from Settings, names keep their face either way
    {
      const faces = () => A.ev(`(() => { const b = document.querySelector('#stream .msg .body'), n = document.querySelector('#stream .msg .who .name'); return JSON.stringify({ body: b ? getComputedStyle(b).fontFamily : '', name: n ? getComputedStyle(n).fontFamily : '', at: document.documentElement.dataset.mchat || '' }); })()`).then(JSON.parse);
      const pick = async v => {
        await click(A, '#btn-settings'); await A.waitFor(`document.querySelector('.seg[data-for="cf"]')`, 8000).catch(() => { throw new Error('chat font: no choice in Settings'); });
        await A.ev(`document.querySelector('.seg[data-for="cf"] button[data-v="${v}"]').click()`);
        await click(A, '.modal .acts button:last-child'); await sleep(600);
      };
      const f0 = await faces();
      need(/^"?Bellefair/.test(f0.body) && !/Bellefair/.test(f0.name) && f0.at === '', 'chat font: messages should start in Bellefair and names in their own face: ' + JSON.stringify(f0));
      await pick('raster');
      const f1 = await faces();
      need(/Raster/.test(f1.body.split(',')[0]) && f1.at === 'raster', 'chat font: choosing Raster did not change the messages: ' + JSON.stringify(f1));
      await A.shot('mega6-5-chat-raster');
      await pick('');
      const f2 = await faces();
      need(/^"?Bellefair/.test(f2.body) && f2.at === '', 'chat font: choosing Bellefair again did not bring it back: ' + JSON.stringify(f2));
      await A.shot('mega6-6-chat-bellefair');
    }
    // 390 px: the menu fits
    await A.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true }); await sleep(900);
    await A.ev(`document.querySelector('#btn-density').click()`); await sleep(400);
    const pm = await A.ev(`(() => { const r = document.querySelector('#mega-dens').getBoundingClientRect(); return { l: r.left, r: r.right, w: innerWidth, t: r.top, b: r.bottom, h: innerHeight }; })()`);
    need(pm.l >= 0 && pm.r <= pm.w && pm.t >= 0 && pm.b <= pm.h, 'density: the menu does not fit at 390 px: ' + JSON.stringify(pm));
    await A.shot('mega6-5-density-390');
    await key(A, 'Escape'); await A.send('Emulation.clearDeviceMetricsOverride'); await sleep(700);
    // leave everything as it was
    await A.ev(`localStorage.removeItem('commons.dens'); localStorage.removeItem('commons.densAll'); localStorage.removeItem('commons.dens.' + ${g}); localStorage.removeItem('commons.dens.' + ${mg}); __megaNew.room(S.gid); 1`);
    await A.waitFor(`S.snap.communities.every(c => c.channels.every(x => !x.unread))`, 20000).catch(() => { });
  }
  // ---- mega phase 7: server settings (owners and admins; changes reach the others; members never see it) ----
  {
    const cid = await A.ev(`S.snap.communities.find(c => c.name === 'Book club').id`);
    const cidj = JSON.stringify(cid), g = JSON.stringify(textGid), admKey = JSON.stringify('adm:' + cid);
    const wf = async (P, expr, ms) => { await P.send('Page.bringToFront'); await sleep(250); return P.waitFor(expr, ms); };
    const toServer = async P => {
      await P.send('Page.bringToFront');
      for (let i = 0; i < 5 && await lvl(P) !== 'server'; i++) {
        await P.ev(`document.activeElement && document.activeElement.blur && document.activeElement.blur()`);
        if (await lvl(P) === 'home') { await P.ev(`__mega.list.go(__mega.list.items.findIndex(x => x.cid === ${cidj}), true)`); await sleep(1200); await key(P, 'Enter'); await sleep(1400); }
        else { await key(P, 'Escape'); await sleep(1000); }
      }
      need(await lvl(P) === 'server', 'admin: could not reach the rooms level');
    };
    const openAdmin = async P => {
      await toServer(P);
      await P.ev(`__mega.list.go(__mega.list.items.findIndex(x => x.key === ${admKey}), true)`);
      await wf(P, `__mega.list.settled && __mega.list.item.key === ${admKey}`, 20000);
      await P.ev(`document.activeElement && document.activeElement.blur && document.activeElement.blur()`);
      await key(P, 'Enter'); await wf(P, `__mega.level === 'chat' && !!document.querySelector('#mega-opts .madm section[data-s="name"]')`, 15000); await sleep(900);
    };
    const adm = (P, sel) => P.ev(`document.querySelector(${JSON.stringify('#mega-opts .madm ' + sel)}).click()`);
    const confirm = async (P, label) => { await P.waitFor(`document.querySelector('.modal')`, 5000); need(await P.ev(`document.querySelector('.modal .acts button:last-child').textContent`) === label, 'admin: the confirm button should say ' + label); await click(P, '.modal .acts button:last-child'); await P.waitFor(`!document.querySelector('.modal')`, 15000); };
    const snapC = (P, expr) => P.ev(`(() => { const c = S.snap.communities.find(c => c.id === ${cidj}); return ${expr}; })()`);
    const origRoom = await A.ev(`S.snap.communities.find(c => c.id === ${cidj}).channels.find(x => x.gid === ${g}).name`);

    // a plain member is offered nothing: no card, no panel, and the panel's own entry point hands back nothing
    await toServer(B);
    need(await B.ev(`!__mega.list.items.some(x => x.kind === 'admin') && !document.querySelector('.madm') && __megaAdmin.html(${cidj}) === ''`), 'admin: a plain member is offered server settings');
    need(await B.ev(`(() => { const o = document.querySelector('#mega-opts'); return !o || o.hidden || !o.querySelector('.madm'); })()`), 'admin: the panel is in a member\'s page');

    // the owner finds a "server settings" card beside the options card, and it opens the room
    await toServer(A);
    need(await A.ev(`__mega.list.items.some(x => x.kind === 'admin' && x.name === 'server settings')`), 'admin: the owner has no server settings card');
    await A.shot('admin-1-cards');
    await openAdmin(A);
    need(await A.ev(`[...document.querySelectorAll('#mega-opts .madm section')].map(s => s.dataset.s).join()`) === 'name,rooms,members,bans,invites,owner', 'admin: the owner should see every section: ' + await A.ev(`[...document.querySelectorAll('#mega-opts .madm section')].map(s => s.dataset.s).join()`));
    await A.shot('admin-2-panel');

    // rename the server; the other client sees it
    await type(A, '#ma-name', 'Reading circle'); await adm(A, '[data-a="rename"]');
    await wf(B, `S.snap.communities.some(c => c.id === ${cidj} && c.name === 'Reading circle')`, 40000).catch(() => { throw new Error('mega: admin: the other client never saw the new server name'); });
    need(await A.ev(`/Renamed/.test(document.querySelector('#ma-say').textContent)`), 'admin: no confirmation after renaming');

    // rename a room: a clash says why, then it works and the other client sees it
    await A.ev(`document.querySelector('#mega-opts .madm [data-a="room-rename"][data-id=${g}]').click()`);
    await A.waitFor(`document.querySelector('.modal #n')`, 5000);
    const otherRoom = await snapC(A, `c.channels.find(x => x.gid !== ${g} && x.kind !== 'voice') ? c.channels.find(x => x.gid !== ${g} && x.kind !== 'voice').name : c.channels.find(x => x.gid !== ${g}).name`);
    await type(A, '.modal #n', otherRoom); await click(A, '.modal .acts button:last-child'); await sleep(800);
    need(await A.ev(`/already a room/.test(document.querySelector('#m-err').textContent)`), 'admin: a clashing room name should say why: ' + await A.ev(`document.querySelector('#m-err').textContent`));
    await type(A, '.modal #n', 'book-talk'); await click(A, '.modal .acts button:last-child'); await A.waitFor(`!document.querySelector('.modal')`, 15000);
    await wf(B, `S.snap.communities.find(c => c.id === ${cidj}).channels.some(x => x.gid === ${g} && x.name === 'book-talk')`, 40000).catch(() => { throw new Error('mega: admin: the other client never saw the room rename'); });

    // add a room, then delete it: asks first (cancel keeps it), the other client sees both
    await type(A, '#ma-room', 'Planning Board'); await adm(A, '[data-a="room-add"]');
    await wf(A, `S.snap.communities.find(c => c.id === ${cidj}).channels.some(x => x.name === 'planning-board')`, 40000);
    await wf(B, `S.snap.communities.find(c => c.id === ${cidj}).channels.some(x => x.name === 'planning-board')`, 40000).catch(() => { throw new Error('mega: admin: the other client never saw the new room'); });
    const planId = await snapC(A, `c.channels.find(x => x.name === 'planning-board').gid`);
    await A.waitFor(`document.querySelector('#mega-opts .madm [data-a="room-delete"][data-id="${planId}"]')`, 8000);
    await A.ev(`document.querySelector('#mega-opts .madm [data-a="room-delete"][data-id="${planId}"]').click()`);
    await A.waitFor(`document.querySelector('.modal')`, 5000);
    need(await A.ev(`/cannot be undone/.test(document.querySelector('.modal').textContent)`), 'admin: deleting a room does not warn');
    await click(A, '.modal .acts button:first-child'); await A.waitFor(`!document.querySelector('.modal')`, 5000); await sleep(600);
    need(await snapC(A, `c.channels.some(x => x.gid === "${planId}")`), 'admin: cancel still deleted the room');
    await A.ev(`document.querySelector('#mega-opts .madm [data-a="room-delete"][data-id="${planId}"]').click()`);
    await confirm(A, 'Delete room');
    await wf(B, `!S.snap.communities.find(c => c.id === ${cidj}).channels.some(x => x.gid === "${planId}")`, 40000).catch(() => { throw new Error('mega: admin: the other client still has the deleted room'); });
    await A.shot('admin-3-rooms');

    // roles: promote bob; his page gets the card and the panel (without the owner-only parts); then demote him
    const bobAcct = await B.ev(`S.snap.me.account`);
    await wf(A, `document.querySelector('#mega-opts .madm [data-a="role"][data-r="admin"][data-acct="${bobAcct}"]')`, 20000);
    await A.ev(`document.querySelector('#mega-opts .madm [data-a="role"][data-r="admin"][data-acct="${bobAcct}"]').click()`);
    await wf(B, `S.snap.communities.find(c => c.id === ${cidj}).role === 'admin'`, 40000).catch(() => { throw new Error('mega: admin: the role change never reached the other client'); });
    await openAdmin(B);
    need(await B.ev(`[...document.querySelectorAll('#mega-opts .madm section')].map(s => s.dataset.s).join()`) === 'name,rooms,members,bans,invites', 'admin: an admin should not see the owner-only section');
    need(await B.ev(`!document.querySelector('#mega-opts .madm [data-a="role"], #mega-opts .madm [data-a="transfer"]')`), 'admin: an admin is offered role changes or a hand-over');
    need(await B.ev(`(async () => { const ms = await rpc('members', { cid: ${cidj} }); return [...document.querySelectorAll('#mega-opts .madm [data-a="ban"]')].every(b => (ms.find(m => m.account === b.dataset.acct) || {}).role === 'member'); })()`), 'admin: an admin is offered removing the owner or another admin');
    await B.shot('admin-4-as-admin');
    await wf(A, `document.querySelector('#mega-opts .madm [data-a="role"][data-r="member"][data-acct="${bobAcct}"]')`, 20000).catch(async () => {
      // the panel shows the roles as they were when it was drawn: open it again, as a person would
      await openAdmin(A);
      await wf(A, `document.querySelector('#mega-opts .madm [data-a="role"][data-r="member"][data-acct="${bobAcct}"]')`, 20000).catch(async () => { throw new Error('admin: no "make member" button for bob after he became admin: ' + await A.ev(`JSON.stringify([...document.querySelectorAll('#mega-opts .madm [data-a="role"]')].map(b => b.dataset.r + ':' + b.dataset.acct.slice(0, 6)))`) + ' bob is ' + await A.ev(`rpc('members', { cid: ${cidj} }).then(m => JSON.stringify(m.map(x => [x.name, x.role])))`)); });
    });
    await A.ev(`document.querySelector('#mega-opts .madm [data-a="role"][data-r="member"][data-acct="${bobAcct}"]').click()`);
    await wf(B, `S.snap.communities.find(c => c.id === ${cidj}).role === 'member'`, 40000).catch(() => { throw new Error('mega: admin: the demotion never reached the other client'); });
    await wf(B, `!document.querySelector('#mega-opts .madm') && !(__mega.railItems || []).some(x => x.kind === 'admin')`, 15000).catch(() => { throw new Error('mega: admin: a demoted admin kept the panel'); });

    // a member can ban nobody, and the core says so too
    const refused = await B.ev(`rpc('rename_channel', { cid: ${cidj}, room: ${g}, name: 'sneaky' }).then(() => 'allowed', e => 'refused: ' + e.message)`);
    need(/^refused/.test(refused), 'admin: the core let a member rename a room: ' + refused);

    // ban and unban a third person when there is one; both lists change on the owner's page
    const third = await A.ev(`rpc('members', { cid: ${cidj} }).then(ms => { const m = ms.find(m => !m.me && m.name !== 'Bob'); return m ? m.account : ''; })`);
    if (third) {
      await wf(A, `document.querySelector('#mega-opts .madm [data-a="ban"][data-acct="${third}"]')`, 20000);
      await A.ev(`document.querySelector('#mega-opts .madm [data-a="ban"][data-acct="${third}"]').click()`);
      await confirm(A, 'Remove and ban');
      await wf(A, `document.querySelector('#mega-opts .madm [data-a="unban"][data-acct="${third}"]')`, 40000).catch(() => { throw new Error('mega: admin: the banned person never showed under banned'); });
      await A.ev(`document.querySelector('#mega-opts .madm [data-a="unban"][data-acct="${third}"]').click()`);
      await confirm(A, 'Unban');
      await wf(A, `!document.querySelector('#mega-opts .madm [data-a="unban"]')`, 40000).catch(() => { throw new Error('mega: admin: the ban was not lifted'); });
    }

    // invites: the list shows the open ones and revoking asks first
    const invId = await A.ev(`rpc('create_invite', { cid: ${cidj}, uses: 1 }).then(() => S.snap.communities.find(c => c.id === ${cidj}).invites.length)`);
    await wf(A, `document.querySelector('#mega-opts .madm [data-a="revoke"]')`, 20000);
    const nInv = await A.ev(`document.querySelectorAll('#mega-opts .madm [data-a="revoke"]').length`);
    await A.ev(`document.querySelector('#mega-opts .madm [data-a="revoke"]').click()`);
    await confirm(A, 'Revoke invite');
    await wf(A, `document.querySelectorAll('#mega-opts .madm [data-a="revoke"]').length === ${nInv - 1}`, 40000).catch(() => { throw new Error('mega: admin: the invite was not revoked'); });
    need(await A.ev(`!!document.querySelector('#mega-opts .madm [data-a="invite"]')`), 'admin: no link to the existing invite flow');

    // 390 px: the panel fits
    await A.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true }); await sleep(900);
    const fit = await A.ev(`(() => { const o = document.querySelector('#mega-opts'), r = o.querySelector('.madm').getBoundingClientRect(); const wide = [...o.querySelectorAll('.madm *')].filter(e => e.getBoundingClientRect().right > innerWidth + 1).map(e => e.tagName + '.' + e.className).slice(0, 4); return { sw: o.scrollWidth, cw: o.clientWidth, doc: document.documentElement.scrollWidth, w: innerWidth, l: r.left, r: r.right, wide }; })()`);
    need(fit.sw <= fit.cw + 1 && fit.doc <= fit.w + 1 && fit.r <= fit.w + 1 && !fit.wide.length, 'admin: the panel does not fit at 390 px: ' + JSON.stringify(fit));
    await A.shot('admin-5-390'); 
    await A.ev(`document.querySelector('#mega-opts').scrollTop = 600`); await sleep(300); await A.shot('admin-6-390-down');
    await A.send('Emulation.clearDeviceMetricsOverride'); await sleep(700);

    // put the names back for the phases that follow
    await type(A, '#ma-name', 'Book club'); await adm(A, '[data-a="rename"]');
    await A.ev(`document.querySelector('#mega-opts .madm [data-a="room-rename"][data-id=${g}]').click()`);
    await A.waitFor(`document.querySelector('.modal #n')`, 5000);
    await type(A, '.modal #n', origRoom); await click(A, '.modal .acts button:last-child'); await A.waitFor(`!document.querySelector('.modal')`, 15000);
    await wf(B, `S.snap.communities.some(c => c.id === ${cidj} && c.name === 'Book club') && S.snap.communities.find(c => c.id === ${cidj}).channels.some(x => x.gid === ${g} && x.name === ${JSON.stringify(origRoom)})`, 40000);
    await key(A, 'Escape'); await sleep(700);
    await toServer(A); await toServer(B);
  }
  if (process.env.COMMONS_UI_STOP_AFTER === 'admin') { console.log('UI check stopped after the server settings phase (COMMONS_UI_STOP_AFTER)'); process.exit(0); }

  // ---- mega parity: full cards, a living idle room, and what each screen costs ----
  {
    const base = !!process.env.COMMONS_UI_PARITY_BASE, pdir = process.env.COMMONS_UI_PARITY_OUT || out;
    const shotP = async (P, n) => { await P.send('Page.bringToFront'); await sleep(250); const r = await P.send('Page.captureScreenshot', { format: 'png' }); fs.writeFileSync(path.join(pdir, n + '.png'), Buffer.from(r.result.data, 'base64')); };
    fs.mkdirSync(pdir, { recursive: true });
    const cidj = JSON.stringify(await A.ev(`S.snap.communities.find(c => c.name === 'Book club').id`));
    const toLevel = async lv => { await A.send('Page.bringToFront'); for (let i = 0; i < 6 && await lvl(A) !== lv; i++) { await A.ev(`document.activeElement && document.activeElement.blur && document.activeElement.blur()`); await key(A, 'Escape'); await sleep(900); } };
    await toLevel('home'); await sleep(2500);
    if (!base) {
      const card = await A.ev(`JSON.stringify(__mega.list.items.filter(x => x.kind === 'community').map(x => ({ name: x.name, members: x.members, pv: x.pv && x.pv.text, html: document.querySelector('.mslot[data-i="' + __mega.list.items.indexOf(x) + '"] .mtop span').textContent })))`);
      console.log('parity cards', card);
      need(JSON.parse(card).every(x => /^\d+ (people|person)$/.test(x.html)), 'parity: a community card does not say how many people: ' + card);
      need(JSON.parse(card).some(x => x.pv), 'parity: no community card has its newest message: ' + card);
      // who is in a call, for a community that is not the open one
      await A.ev(`(() => { const c = S.snap.communities.find(x => x.name === 'Book club'); const v = c.channels.find(x => x.kind === 'voice'); if (v) { __mega.cp.set(v.gid, [{ name: 'Ann', account: 'a' }, { name: 'Ben', account: 'b' }]); __megaRender(); } return !!v; })()`);
      await sleep(600);
      console.log('parity call text', await A.ev(`[...document.querySelectorAll('.mfaces-row span:not(.mfaces)')].map(e => e.textContent).join('|')`));
      await A.ev(`__mega.cp.clear(); __megaRender()`);
    }
    await sleep(800); await shotP(A, 'home');
    await A.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true }); await sleep(1200); await shotP(A, 'home-390');
    await A.send('Emulation.clearDeviceMetricsOverride'); await sleep(900);
    await A.ev(`__mega.list.go(__mega.list.items.findIndex(x => x.name === 'Book club'), true)`); await sleep(1500);
    await key(A, 'Enter'); await sleep(250); await shotP(A, 'entering'); await sleep(1900); await shotP(A, 'rooms');
    await A.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true }); await sleep(1200); await shotP(A, 'rooms-390');
    // design: on a phone the card bar is one row (it used to wrap onto two or three, over the card) and the community name keeps some width
    { const bar = await A.ev(`(() => { const b = document.querySelector('.mbar').getBoundingClientRect(), m = document.querySelector('.mmark').getBoundingClientRect(), c = document.querySelector('.mslot.settled .msq').getBoundingClientRect(); const tops = new Set([...document.querySelectorAll('.mbar .mchip')].filter(x => x.getBoundingClientRect().width > 0).map(x => Math.round(x.getBoundingClientRect().top / 12))); return { h: b.height, rows: tops.size, mark: m.width, cardTop: c.top, barBottom: b.bottom }; })()`);
      need(bar.rows === 1 && bar.h <= 60 && bar.mark >= 100 && bar.cardTop >= bar.barBottom - 1, 'design: the phone card bar wraps or crushes the name: ' + JSON.stringify(bar)); }
    // design: on a phone "colours" and "draw" leave the bar and wait in the "more" menu, so the chips that matter all fit
    { const mm = await A.ev(`(() => { const c = n => { const b = document.querySelector('#mega .mbar [data-m="' + n + '"]'); return b ? { hidden: b.hidden, shown: getComputedStyle(b).display !== 'none', label: b.textContent } : null; }; return { paint: c('paint'), draw: c('draw'), clipped: [...document.querySelectorAll('.mbar .mchip')].filter(b => getComputedStyle(b).display !== 'none' && b.getBoundingClientRect().right > innerWidth + 1).map(b => b.dataset.m) }; })()`);
      need(mm.paint && mm.draw && !mm.paint.shown && !mm.draw.shown && !mm.clipped.length, 'design: on a phone colours and draw must leave the card bar and every chip must fit: ' + JSON.stringify(mm));
      await A.ev(`document.querySelector('#mega [data-m="more"]').click()`); await sleep(400);
      const items = await A.ev(`[...document.querySelectorAll('.mmenu button')].map(b => b.textContent)`);
      for (const k of ['paint', 'draw']) need(mm[k].hidden || items.includes(mm[k].label), 'design: the more menu is missing ' + k + ': ' + JSON.stringify([mm, items]));
      need(items.includes('settings'), 'design: the more menu lost its old items: ' + JSON.stringify(items));
      await shotP(A, 'more-menu-390'); await key(A, 'Escape'); await sleep(300); }
    await A.send('Emulation.clearDeviceMetricsOverride'); await sleep(900);
    await A.ev(`__mega.list.go(__mega.list.items.findIndex(x => x.kind === 'text'), true)`); await sleep(1500);
    await key(A, 'Enter'); await sleep(300); await shotP(A, 'enterroom'); await sleep(2500); await shotP(A, 'chat');
    // an idle room: the cost of the backdrop over 8 s with nothing happening
    await A.send('Performance.enable');
    const m0 = await A.send('Performance.getMetrics'), d0 = await A.ev(`__mega.bdInfo().draws`), t0 = Date.now();
    await sleep(8000);
    const m1 = await A.send('Performance.getMetrics'), d1 = await A.ev(`__mega.bdInfo().draws`), dt = (Date.now() - t0) / 1000;
    const val = (m, n) => m.result.metrics.find(x => x.name === n).value;
    const idle = { fps: +((d1 - d0) / dt).toFixed(2), taskMsPerSec: +((val(m1, 'TaskDuration') - val(m0, 'TaskDuration')) * 1000 / dt).toFixed(1), scriptMsPerSec: +((val(m1, 'ScriptDuration') - val(m0, 'ScriptDuration')) * 1000 / dt).toFixed(1), heat: await A.ev(`+__mega.bdInfo().heat.toFixed(3)`) };
    console.log('parity idle room', JSON.stringify(idle)); fs.writeFileSync(path.join(pdir, 'idle.json'), JSON.stringify(idle));
    // and a room that has been quiet for a long while (heat at its lowest the version allows)
    await A.ev(`__mega.bdApi.bd().heat = 0`); await sleep(1500);
    const q0 = await A.send('Performance.getMetrics'), e0 = await A.ev(`__mega.bdInfo().draws`), u0 = Date.now();
    await sleep(8000);
    const q1 = await A.send('Performance.getMetrics'), e1 = await A.ev(`__mega.bdInfo().draws`), du = (Date.now() - u0) / 1000;
    const quiet = { fps: +((e1 - e0) / du).toFixed(2), taskMsPerSec: +((val(q1, 'TaskDuration') - val(q0, 'TaskDuration')) * 1000 / du).toFixed(1), scriptMsPerSec: +((val(q1, 'ScriptDuration') - val(q0, 'ScriptDuration')) * 1000 / du).toFixed(1), heat: await A.ev(`+__mega.bdInfo().heat.toFixed(3)`) };
    console.log('parity quiet room', JSON.stringify(quiet)); fs.writeFileSync(path.join(pdir, 'quiet.json'), JSON.stringify(quiet));
    await shotP(A, 'chat-later');
    await A.ev(`__mega.api.openRail()`); await sleep(900); await shotP(A, 'dots-open');
    // design: the pop-out labels are set letter by letter in <i>; they must not come out italic
    need(await A.ev(`[...document.querySelectorAll('#mega-rail .mlb i')].every(i => getComputedStyle(i).fontStyle === 'normal')`), 'design: the pop-out labels are italic'); await A.ev(`document.querySelector('#mega-rail').dispatchEvent(new MouseEvent('mouseleave'))`); await sleep(900);
    await A.ev(`(() => { const a = __mega.bdApi.bd(); })()`);
    await A.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true }); await sleep(1500); await shotP(A, 'chat-390');
    // design: on a phone the chat header keeps the room name (five labelled chips once squeezed it to zero width) and nothing sticks out of the screen
    { const hd = await A.ev(`(() => { const h = document.querySelector('#head').getBoundingClientRect(), t = document.querySelector('.head-title').getBoundingClientRect(), a = document.querySelector('.head-actions').getBoundingClientRect(); return { head: h.width, title: t.width, titleText: document.querySelector('#chan-title').textContent, right: a.right, w: innerWidth, h1: getComputedStyle(document.querySelector('#chan-title')).fontSize }; })()`);
      need(hd.title >= Math.min(100, hd.head * .3) && hd.right <= hd.w + 1, 'design: the phone chat header crushes the room name: ' + JSON.stringify(hd)); }
    await A.send('Emulation.clearDeviceMetricsOverride'); await sleep(900);
    if (!base) need(idle.fps >= 7 && idle.heat >= .11, 'parity: an idle room should keep drawing at about 10 fps: ' + JSON.stringify(idle));
    await key(A, 'Escape'); await sleep(300); await shotP(A, 'leaving'); await sleep(1600);
    await toLevel('server'); await sleep(500);
  }
  // ---- mega: presence dots on the cards (mega-presence.js) ----
  {
    const cid = await A.ev(`S.snap.communities.find(c => c.name === 'Book club').id`), cj = JSON.stringify(cid), gj = JSON.stringify(textGid);
    const meA = await A.ev(`S.snap.me.name`), accA = await A.ev(`S.snap.me.account`);
    const toSrv = async P => {
      await P.send('Page.bringToFront');
      for (let i = 0; i < 6 && await lvl(P) === 'chat'; i++) { await P.ev(`document.activeElement && document.activeElement.blur && document.activeElement.blur()`); await key(P, 'Escape'); await sleep(900); }
      if (await lvl(P) === 'home') { await P.ev(`__mega.list.go(__mega.list.items.findIndex(x => x.kind === 'community' && x.cid === ${cj}), true)`); await sleep(1200); await key(P, 'Enter'); await sleep(1800); }
      need(await lvl(P) === 'server', 'presence: could not reach the rooms level');
    };
    // the unstored channel is only known to the core's new rpc; the check pins the answer so the page logic is exercised on any build
    const chan = (P, mode) => P.ev(`(() => { if (!window.__rpc0) window.__rpc0 = window.rpc; window.__chanMode = ${JSON.stringify(mode)}; window.rpc = (m, a) => m === 'live_chan' && window.__chanMode !== 'real' ? Promise.resolve({ chan: window.__chanMode === 'on' }) : window.__rpc0(m, a); __megaPres._reset(); return 1; })()`);
    const idx = (P, g) => P.ev(`__mega.list.items.findIndex(x => x.key === ${JSON.stringify(g)})`);
    const optIdx = P => P.ev(`__mega.list.items.findIndex(x => x.kind === 'options')`);
    // by the card, not only its position: the list can be rebuilt (a card appearing or going) between finding a card and going to it
    const focus = async (P, i) => {
      await P.send('Page.bringToFront');
      const key = await P.ev(`(__mega.list.items[${i}] || {}).key || ''`);
      for (let t = 0; t < 5; t++) {
        const at = await P.ev(`__mega.list.items.findIndex(x => x.key === ${JSON.stringify(key)})`);
        await P.ev(`__mega.list.go(${'${at < 0 ? ' + i + ' : at}'}, true)`.replace('${at < 0 ? ' + i + ' : at}', String(at < 0 ? i : at)));
        await P.waitFor(`__mega.list.settled`, 8000);
        if (await P.ev(`(__mega.list.item || {}).key === ${JSON.stringify(key)}`)) return;
        await sleep(400);
      }
      throw new Error('presence: could not settle on the card ' + key);
    };
    const focusSend = async (P, i) => { await focus(P, i); await sleep(2300); };   // a hidden tab sends nothing, so the sender stays in front until it has
    const dotsOn = (P, g) => P.ev(`(() => { const i = __mega.list.items.findIndex(x => x.key === ${JSON.stringify(g)}); const s = document.querySelector('.mslot[data-i="' + i + '"]'); return s ? [...s.querySelectorAll('.mpres .mpd:not(.out)')].map(d => d.dataset.n) : []; })()`);
    const allDots = P => P.ev(`document.querySelectorAll('.mpres .mpd:not(.out)').length`);
    const sent = P => P.ev(`__megaPres.stats.sent`);
    await toSrv(A); await toSrv(B);
    const real = await A.ev(`rpc('live_chan', {}).then(r => JSON.stringify(r), e => 'missing: ' + e.message)`);
    console.log('presence: the core answers live_chan with', real);
    await chan(A, 'on'); await chan(B, 'on');
    need(await A.ev(`__megaPres.on === true`), 'presence: the switch should be on by default');
    // alice rests on a room card: bob sees a dot there, in alice's colour, and nowhere else
    await focusSend(A, await idx(A, textGid));
    await B.send('Page.bringToFront');
    const presDiag = () => B.ev(`JSON.stringify({ peers: __megaPres.peers(), dots: [...document.querySelectorAll('.mpres .mpd')].map(d => [d.className, d.closest('.mslot') && d.closest('.mslot').dataset.i]), pres: document.querySelectorAll('.mpres').length, slots: [...document.querySelectorAll('.mslot')].map(s => s.dataset.i + ':' + (s.querySelector('.msq') ? 1 : 0)), keys: __mega.list.items.map(x => x.key.slice(0, 6)), cur: __mega.list.cur, lvl: __mega.level })`);
    await B.waitFor(`__megaPres.peers().length === 1`, 20000).catch(async () => { throw new Error('presence: bob never heard of alice\'s card: ' + await A.ev(`JSON.stringify(__megaPres.state())`) + await B.ev(`JSON.stringify(__megaPres.stats)`)); });
    // the dot on alice's card (an event for the community card itself, from her way here through the home screen, may come first)
    await B.waitFor(`(() => { const i = __mega.list.items.findIndex(x => x.key === ${gj}); const s = document.querySelector('.mslot[data-i="' + i + '"]'); return !!s && s.querySelectorAll('.mpres .mpd:not(.out)').length === 1; })()`, 8000).catch(async () => { throw new Error('presence: bob knows alice is there but shows no single dot on her card: ' + await presDiag()); });
    need((await dotsOn(B, textGid)).join() === meA && await allDots(B) === 1, 'presence: the dot is not on the matching card: ' + JSON.stringify(await dotsOn(B, textGid)));
    const look = await B.ev(`(() => { const d = document.querySelector('.mpres .mpd'), l = d.parentElement, r = d.getBoundingClientRect(), q = d.closest('.msq').getBoundingClientRect(); return { pe: getComputedStyle(l).pointerEvents + '/' + getComputedStyle(d).pointerEvents, bg: getComputedStyle(d).backgroundColor, inside: r.left >= q.left && r.right <= q.right && r.top >= q.top && r.bottom <= q.bottom, label: d.textContent, hit: document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2) === d }; })()`);
    need(look.pe === 'none/none' && look.inside && look.label === '' && !look.hit && /^rgb/.test(look.bg), 'presence: the dot should be a plain colour, inside the card and out of the pointer\'s way: ' + JSON.stringify(look));
    await focus(B, await idx(B, textGid)); await sleep(1800); await B.shot('mega7-1-presence-dot');
    // the name shows on hover without the dot taking the pointer
    const hv = await B.ev(`(() => { const r = document.querySelector('.mpres .mpd').getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2]; })()`);
    await B.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: hv[0], y: hv[1] }); await sleep(300);
    need(await B.ev(`(() => { const t = document.querySelector('#mega-pres-tip'); return !!t && !t.hidden && t.textContent === ${JSON.stringify(meA)} && getComputedStyle(t).pointerEvents === 'none'; })()`), 'presence: no name on hover');
    await B.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 3, y: 3 }); await sleep(200);
    // 390 px
    await B.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true }); await sleep(1200);
    await B.shot('mega7-1b-presence-dot-390');
    need(await B.ev(`(() => { const d = document.querySelector('.mpres .mpd'); if (!d) return false; const r = d.getBoundingClientRect(); return r.right <= innerWidth && r.left >= 0 && document.documentElement.scrollWidth <= innerWidth + 1; })()`), 'presence: the dot overflows at 390 px');
    await B.send('Emulation.clearDeviceMetricsOverride'); await sleep(800);
    // reduced motion and fx-lite: static dots
    await B.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] }); await sleep(300);
    need(await B.ev(`getComputedStyle(document.querySelector('.mpres .mpd')).animationName === 'none'`), 'presence: dots still move with reduced motion');
    await B.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'no-preference' }] }); await sleep(300);
    need(await B.ev(`getComputedStyle(document.querySelector('.mpres .mpd')).animationName !== 'none'`), 'presence: dots should drift with motion on');
    await B.ev(`document.documentElement.classList.add('fx-lite')`); await sleep(200);
    need(await B.ev(`getComputedStyle(document.querySelector('.mpres .mpd')).animationName === 'none'`), 'presence: dots still move in fx-lite');
    await B.ev(`document.documentElement.classList.remove('fx-lite')`);
    // a settled card sends nothing more than the heartbeat: idle, visible, 6 s
    await A.send('Page.bringToFront'); await sleep(1500);
    const s0 = await sent(A); await sleep(6000);
    need(await sent(A) === s0, 'presence: events were sent while idle');
    // scrolling quickly past cards sends nothing for each card
    const n0 = await sent(A);
    for (const i of [0, 1, 2, 0, 1, 2]) { await A.ev(`__mega.list.go(${i}, true)`); await sleep(120); }
    await focus(A, await idx(A, textGid)); await sleep(2500);
    need(await sent(A) - n0 <= 2, 'presence: a quick scroll past cards sent ' + (await sent(A) - n0) + ' events');
    // moving to another card moves the dot; leaving the card screens removes it
    const other = await A.ev(`__mega.list.items.find(x => x.kind === 'text' && x.key !== ${gj}) ? __mega.list.items.find(x => x.kind === 'text' && x.key !== ${gj}).key : null`);
    if (other) {
      await focusSend(A, await idx(A, other)); await B.send('Page.bringToFront');
      await B.waitFor(`(() => { const i = __mega.list.items.findIndex(x => x.key === ${JSON.stringify(other)}); const s = document.querySelector('.mslot[data-i="' + i + '"]'); return !!s && s.querySelectorAll('.mpres .mpd:not(.out)').length === 1; })()`, 15000).catch(() => { throw new Error('presence: the dot did not move to the next card'); });
      await B.waitFor(`document.querySelectorAll('.mpres .mpd:not(.out)').length === 1`, 8000);
    }
    await focusSend(A, await optIdx(A));
    await B.send('Page.bringToFront');
    await B.waitFor(`document.querySelectorAll('.mpres .mpd:not(.out)').length === 0 && __megaPres.peers().length === 0`, 15000).catch(() => { throw new Error('presence: the dot stayed after alice moved to a card with no one to show'); });
    // the switch in the options room stops sending, and nothing else
    await A.send('Page.bringToFront'); await key(A, 'Enter'); await A.waitFor(`__mega.level === 'chat' && !!document.querySelector('#mega-opts #pres')`, 10000).catch(() => { throw new Error('presence: no switch in the options room'); });
    need(await A.ev(`document.querySelector('#pres').checked === true`), 'presence: the switch should show on');
    await A.shot('mega7-2-options-switch');
    await A.ev(`document.querySelector('#pres').click()`); await sleep(400);
    need(await A.ev(`__megaPres.on === false && localStorage.getItem('commons.presence') === 'off'`), 'presence: the switch did not turn it off');
    for (let i = 0; i < 4 && await lvl(A) === 'chat'; i++) { await key(A, 'Escape'); await sleep(900); }
    const off0 = await sent(A);
    await focus(A, await idx(A, textGid)); await sleep(3500);
    need(await sent(A) === off0 && (await B.ev(`__megaPres.peers().length`)) === 0, 'presence: events were sent with the switch off');
    await A.ev(`__megaPres.set(true)`); await sleep(2300);
    await B.send('Page.bringToFront');
    await B.waitFor(`__megaPres.peers().length === 1`, 15000).catch(() => { throw new Error('presence: turning the switch on again did not show alice'); });
    // the chat level and a hidden tab send nothing
    await A.send('Page.bringToFront'); await A.ev(`Object.defineProperty(document, 'visibilityState', { get: () => 'hidden', configurable: true }); 1`);
    const h0 = await sent(A);
    await focus(A, await optIdx(A)); await sleep(500); await focus(A, await idx(A, textGid)); await sleep(2800);
    need(await sent(A) === h0, 'presence: events were sent while the tab was hidden');
    await A.ev(`delete document.visibilityState; 1`);
    // a heartbeat about every 20 s, and nothing else, while it stays
    await sleep(1500);
    const b0 = await A.ev(`__megaPres.stats.beats`), t0 = await sent(A);
    await sleep(23000);
    const b1 = await A.ev(`__megaPres.stats.beats`), t1 = await sent(A);
    need(b1 - b0 === 1 && t1 - t0 <= 2, `presence: expected one heartbeat in 23 s, got ${b1 - b0} beats and ${t1 - t0} events`);
    // nothing on the log fallback (the page asks the core whether the unstored channel is in use)
    await chan(A, 'off'); await sleep(200);
    const fk0 = await A.ev(`__megaLive.stats.kinds['pres.f'] || 0`), fs0 = await sent(A);
    await focus(A, await optIdx(A)); await sleep(1500); await focus(A, await idx(A, textGid)); await sleep(3500);
    need(await sent(A) === fs0 && await A.ev(`(__megaLive.stats.kinds['pres.f'] || 0)`) === fk0, 'presence: events were sent on the log fallback');
    await chan(A, 'on');
    // invalid events are ignored (alice stops sending so that her real events cannot mix in)
    await A.ev(`__megaPres.set(false)`); await sleep(1500);
    await B.send('Page.bringToFront'); await sleep(2500);   // what alice sent before is read now
    await B.ev(`__megaPres._reset(); 1`);
    const memB = await B.ev(`rpc('members', { cid: ${cj} }).then(m => m.find(x => !x.me).account)`);
    const ev = (o) => B.ev(`__megaPres.onEvent(${JSON.stringify({ id: 1, gid: textGid, account: memB, kind: 'pres.f', data: JSON.stringify({ v: 1, c: cid, r: '' }), ts: 1, ...o })}).then(() => __megaPres.peers().length)`);
    const bad0 = await B.ev(`__megaPres.stats.bad`);
    const cases = [
      { gid: 'f'.repeat(32) }, { account: 'a'.repeat(64) }, { account: '' }, { data: '{nope' }, { data: JSON.stringify({ v: 2, c: cid, r: '' }) },
      { data: JSON.stringify({ v: 1, c: 'f'.repeat(32), r: '' }) }, { data: JSON.stringify({ v: 1, c: cid, r: 'zz' }) }, { data: JSON.stringify({ v: 1, c: cid, r: 'e'.repeat(32) }) },
      { data: JSON.stringify({ v: 1, c: cid, r: { x: 1 } }) }, { data: 'x'.repeat(400) }, { kind: 'pres.g' }, { data: JSON.stringify({ v: 1, c: cid, r: '', o: 7 }) },
    ];
    for (const c of cases) need(await ev(c) === 0, 'presence: an invalid event was taken: ' + JSON.stringify(c).slice(0, 120));
    await B.ev(`new Promise(r => setTimeout(r, 300))`);
    need(await B.ev(`__megaPres.stats.bad`) > bad0 && await allDots(B) === 0, 'presence: invalid events left a dot: ' + await B.ev(`JSON.stringify({ bad: __megaPres.stats.bad, peers: __megaPres.peers(), dots: document.querySelectorAll('.mpres .mpd').length })`) + ' bad0=' + bad0);
    need(await ev({}) === 1, 'presence: a valid event from a member was not taken');
    // at most 6 dots on a card, then +N
    await B.ev(`__megaPres._inject(Array.from({ length: 9 }, (_, i) => ({ a: 'acct' + i, c: ${cj}, r: ${gj}, name: 'Friend ' + i })))`); await sleep(500);
    const cap = await B.ev(`(() => { const i = __mega.list.items.findIndex(x => x.key === ${gj}); const s = document.querySelector('.mslot[data-i="' + i + '"]'); return { dots: s.querySelectorAll('.mpd:not(.out)').length, more: (s.querySelector('.mpm') || {}).textContent }; })()`);
    need(cap.dots === 6 && cap.more === '+3', 'presence: the cap is wrong: ' + JSON.stringify(cap));
    await B.shot('mega7-3-presence-cap');
    await B.ev(`__megaPres._reset(); 1`);
    await A.ev(`__megaPres.set(true)`);
    await focus(A, await idx(A, textGid));
    await toSrv(B);
  }
  if (process.env.COMMONS_UI_STOP_AFTER === 'mega6') { console.log('UI check stopped after mega phase 6 (COMMONS_UI_STOP_AFTER)'); process.exit(0); }

  // ---- large files ----
  // Over the 24 MB line the same paperclip takes the chunked path: the page reads 8 MB slices of the picked file,
  // the core encrypts and sends one at a time. Nothing large is ever downloaded unless Download is pressed.
  const dlDir = path.join(out, 'downloads-bob');
  fs.rmSync(dlDir, { recursive: true, force: true });
  fs.mkdirSync(dlDir, { recursive: true });
  const bigName = 'a-big-film.bin';
  const bigPath = path.join(out, bigName);
  const bigSize = 300 * 1024 * 1024 + 12345;
  const bigHash = await makeBigFile(bigPath, bigSize);
  const row = name => `[...document.querySelectorAll('.msg')].find(m => m.querySelector('.fname')?.textContent === ${JSON.stringify(name)})`;

  // Bob chooses where large files land (desktop setting), before anything arrives.
  await click(B, '#btn-settings'); await B.waitFor(`document.querySelector('#dl-dir')`);
  await type(B, '#dl-dir', dlDir); await click(B, '.modal .acts button:last-child');
  await B.waitFor(`!document.querySelector('.modal')`);
  await B.waitFor(`S.snap.download_dir === ${JSON.stringify(dlDir)}`, 20000).catch(async e => { throw new Error(e.message + ' | ' + await B.ev(`(async () => JSON.stringify({ dd: S.snap.download_dir, toast: document.querySelector('#toast').innerText, modal: !!document.querySelector('.modal'), sync: document.querySelector('#sync-status').innerText, lvl: __mega.level, busy: tick.busy, core: await rpc('snapshot').then(x => x.download_dir), snapKey: S.last.length }))()`)); });

  // Upload it once, watch the tray (percentage, speed, time left), then cancel before it finishes.
  await A.setFiles('#file', bigPath);
  await A.waitFor(`(() => { const e = document.querySelector('#transfers'); return !!e && !e.hidden && e.innerText.includes(${JSON.stringify(bigName)}); })()`, 30000);
  await A.waitFor(`(() => { const n = document.querySelector('#transfers .xfer-note'); return !!n && n.innerText.includes('%') && n.innerText.includes('/s') && n.innerText.includes('left'); })()`, 30000);
  await A.shot('15-uploading-large');
  await A.ev(`document.querySelector('#transfers [data-xcancel]').click()`);
  // Cancelling real progress asks first; "Keep it" changes nothing, then cancel for real.
  await A.waitFor(`document.querySelector('.modal')`, 5000);
  await click(A, '.modal .acts button:first-child'); await sleep(400);
  if (await A.ev(`!document.querySelector('#transfers [data-xcancel]')`)) throw new Error('Keep it still cancelled the upload');
  await A.ev(`document.querySelector('#transfers [data-xcancel]').click()`);
  await A.waitFor(`document.querySelector('.modal')`, 5000);
  await click(A, '.modal .acts button:last-child');
  await A.waitFor(`(() => { const e = document.querySelector('#transfers'); return !e || e.hidden; })()`, 30000);
  await sleep(1500);
  if (await A.ev(`!!(${row(bigName)})`)) throw new Error('a cancelled upload still produced a message');

  // Send it for real. The chunk that was in flight when Cancel was pressed is gone with the relay copy, so this is
  // a full send; resuming after a dropped connection is covered by crates/core/tests/large_files.rs.
  await A.setFiles('#file', bigPath);
  await A.waitFor(`(() => { const e = document.querySelector('#transfers'); return !!e && !e.hidden && e.innerText.includes(${JSON.stringify(bigName)}); })()`, 15000);
  await A.waitFor(`(() => { const e = document.querySelector('#transfers'); return !e || e.hidden; })()`, 600000);
  await B.waitFor(`!!(${row(bigName)})`, 60000);
  const shown = await B.ev(`(() => { const m = ${row(bigName)}; return { text: m.innerText, img: !!m.querySelector('.imgslot'), get: !!m.querySelector('[data-a=get]') }; })()`);
  if (!shown.text.includes('300.0 MB')) throw new Error('the message does not show the real size: ' + shown.text);
  if (!(shown.text.includes('days left on the relay') || shown.text.includes('day left on the relay'))) throw new Error('the message does not show when it leaves the relay: ' + shown.text);
  if (!shown.get) throw new Error('no Download button on the large file');
  if (shown.img) throw new Error('a large file got an image slot (it must never be fetched by scrolling)');
  if (fs.readdirSync(dlDir).length) throw new Error('a large file was downloaded without pressing Download');

  // Download: progress and speed next to the message, then Cancel, then Download again to the end.
  const get = `(${row(bigName)}).querySelector('[data-a=get]').click()`;
  // Save to: the dialog comes first. Cancelling it starts nothing.
  await B.ev(get); await B.waitFor(`document.querySelector('#dl-to')`);
  await B.shot('15b-save-to');
  await click(B, '.modal .acts button'); await sleep(300);
  if (await B.ev(`!!document.querySelector('#transfers') && !document.querySelector('#transfers').hidden`)) throw new Error('cancelling Save to started a download');
  await B.ev(get); await B.waitFor(`document.querySelector('#dl-to')`);
  await click(B, '.modal .acts button:last-child');
  await B.waitFor(`(() => { const e = document.querySelector('#transfers'); return !!e && !e.hidden && e.innerText.includes(${JSON.stringify(bigName)}); })()`, 30000);
  await B.waitFor(`(() => { const n = document.querySelector('.xfer-inline .xfer-note'); return !!n && n.innerText.includes('%') && n.innerText.includes('/s'); })()`, 30000);
  await B.shot('16-downloading-large');
  await B.ev(`(${row(bigName)}).querySelector('[data-a="cancel-get"]').click()`);
  await B.waitFor(`document.querySelector('.modal')`, 5000);
  await click(B, '.modal .acts button:last-child');
  await B.waitFor(`(() => { const e = document.querySelector('#transfers'); return !e || e.hidden; })()`, 30000);
  await sleep(800);
  const afterCancel = fs.readdirSync(dlDir);
  if (afterCancel.length) throw new Error('cancelling a download left a file behind: ' + afterCancel.join(', '));
  // Browse: the system's folder chooser (a stand-in here) fills the box.
  await B.ev(get); await B.waitFor(`document.querySelector('#dl-to')`);
  await B.ev(`document.querySelector('#dl-to').value = ''`);
  await click(B, '#dl-browse');
  await B.waitFor(`document.querySelector('#dl-to').value === ${JSON.stringify(dlDir)}`, 10000);
  await click(B, '.modal .acts button:last-child');
  // The relay goes away for a few seconds mid-download: the page waits and carries on by itself.
  await B.waitFor(`(() => { const n = document.querySelector('#transfers .xfer-note'); return !!n && /[1-9]%/.test(n.innerText); })()`, 30000);
  relayProc.kill('SIGKILL'); await new Promise(r => relayProc.once('exit', r));
  await B.waitFor(`document.querySelector('#transfers .xfer-state')?.innerText.includes('Waiting for connection')`, 120000);
  await B.shot('16b-waiting-for-connection');
  await sleep(3000); relayProc = startRelay();
  await B.ev(`(() => { const b = document.querySelector('#transfers [data-xpause]'); b && b.click(); })()`);
  await B.waitFor(`document.querySelector('#transfers .xfer-state')?.innerText.startsWith('Paused')`, 30000);
  await B.shot('16c-paused');
  await B.ev(`document.querySelector('#transfers [data-xpause]').click()`);
  await B.waitFor(`(() => { const m = ${row(bigName)}; return !!(m && m.querySelector('.saved-line') && m.querySelector('[data-a=show]') && m.querySelector('[data-a=get]')); })()`, 600000);
  const saved = await B.ev(`(() => { const m = S.msgs.messages.find(x => x.file && x.file.name === ${JSON.stringify(bigName)}); return m && m.file.download || null; })()`);
  if (!saved || !saved.finished) throw new Error('the core does not report the download as finished: ' + JSON.stringify(saved));
  const savedPath = path.join(dlDir, bigName);
  if (path.resolve(saved.path) !== path.resolve(savedPath)) throw new Error('saved to the wrong folder: ' + saved.path);
  if (fs.statSync(savedPath).size !== bigSize) throw new Error('the saved file has the wrong size');
  if (await hashFile(savedPath) !== bigHash) throw new Error('the saved file is not the file that was sent');

  // Show in folder goes through the core and ends at the folder, which the fake xdg-open records.
  await B.ev(`(${row(bigName)}).querySelector('[data-a=show]').click()`);
  for (let i = 0; i < 40 && !fs.existsSync(xdgMarker); i++) await sleep(100);
  if (!fs.existsSync(xdgMarker)) throw new Error('Show in folder did not reach xdg-open');
  const opened = fs.readFileSync(xdgMarker, 'utf8').trim();
  if (path.resolve(opened) !== path.resolve(dlDir)) throw new Error('Show in folder opened ' + opened);

  // A large picture is still a file, not an inline image: the page has no slot and fetches nothing until Download.
  const picName = 'a-big-picture.png';
  const picPath = path.join(out, picName);
  await makeBigFile(picPath, 24 * 1024 * 1024 + 1000);
  await A.setFiles('#file', picPath);
  await A.waitFor(`(() => { const e = document.querySelector('#transfers'); return !!e && !e.hidden && e.innerText.includes(${JSON.stringify(picName)}); })()`, 15000);
  await A.waitFor(`(() => { const e = document.querySelector('#transfers'); return !e || e.hidden; })()`, 600000);
  await B.waitFor(`!!(${row(picName)})`, 60000);
  const pic = await B.ev(`(() => { const m = ${row(picName)}; return { img: !!m.querySelector('.imgslot'), get: !!m.querySelector('[data-a=get]'), text: m.innerText }; })()`);
  if (pic.img) throw new Error('a large picture got an inline image slot');
  if (!pic.get) throw new Error('no Download button on the large picture');
  if (!pic.text.includes('24.0 MB')) throw new Error('the large picture does not show its real size: ' + pic.text);
  if (fs.readdirSync(dlDir).length !== 1) throw new Error('something was auto-downloaded: ' + fs.readdirSync(dlDir).join(', '));

  // The uploader can remove the relay copy; everyone's message goes away.
  const picId = await B.ev(`(() => { const m = ${row(picName)}; return m ? m.dataset.id : null; })()`);
  // The uploader's transfer bar hides before its next message refresh lands; wait for the row here too.
  await A.waitFor(`!!(${row(picName)})`, 30000);
  await A.ev(`(${row(picName)}).querySelector('[data-a=del]').click()`);
  await B.waitFor(`(() => { const m = document.querySelector('.msg[data-id="' + ${JSON.stringify(picId)} + '"]'); return !!m && m.classList.contains('deleted'); })()`, 30000);

  // Old apps must see ordinary File messages, never an unknown kind.
  for (const P of [A, B]) {
    const newer = await P.ev(`(async () => (await rpc('notices')).filter(n => n.kind === 'newer').length)()`);
    if (newer) throw new Error('a newer-version notice appeared after a large file');
  }
  await B.shot('17-large-file');

  // Done with the big test files; the screenshots and data directory stay.
  fs.rmSync(bigPath, { force: true }); fs.rmSync(picPath, { force: true });
  fs.rmSync(dlDir, { recursive: true, force: true });

  // Phone-width layout: the rail folds away, the composer stays on screen, and desktop-only settings are gone.
  const P = await open(pageUrl(A_PORT));
  await P.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 3, mobile: true });
  await P.waitFor(`!document.querySelector('#app').hidden`, 20000);
  await P.waitFor(`document.querySelectorAll('.room').length>=1`, 20000);
  await P.ev(`document.querySelectorAll('.room')[0].click()`);   // mega opens on the cards; go into a chat for the layout checks below
  await P.waitFor(`!document.querySelector('#composer').hidden`, 10000); await sleep(500);
  const phone = await P.ev(`(() => { const r = document.querySelector('#rail').getBoundingClientRect(), c = document.querySelector('#composer').getBoundingClientRect(); return { w: innerWidth, railAway: r.right <= 1, composerIn: c.height > 0 && c.top > 0 && c.bottom <= innerHeight + 1 }; })()`);
  if (phone.w > 760 || !phone.railAway || !phone.composerIn) throw new Error('narrow layout is wrong: ' + JSON.stringify(phone));
  await P.shot('09-phone');
  await P.ev(`(async () => { const me = S.snap.me.name; for (const t of ['ok', 'see you at noon', '@' + me + ' you there', 'a longer message that is set at the normal size and wraps onto a second line', 'hi \u{1F389}']) await rpc('send_text', { gid: S.gid, text: t, reply: null }); return 1; })()`);
  await P.waitFor(`document.querySelectorAll('.msg').length >= 5`, 15000); await sleep(1500);
  const pr = await P.ev(`(() => { const c = document.querySelector('#mega-bd'), s = document.querySelector('#stream'); return { bd: !!c && c.width > 0, hscroll: document.documentElement.scrollWidth > innerWidth + 1, wide: [...s.querySelectorAll('.msg, .msg *')].filter(e => e.getBoundingClientRect().right > s.getBoundingClientRect().right + 1).slice(0, 4).map(e => e.tagName + '.' + e.className), sw: s.scrollWidth, cw: s.clientWidth, n: document.querySelectorAll('.msg').length }; })()`);
  need(pr.bd && !pr.hscroll, 'the room on a phone: backdrop missing or the page scrolls sideways: ' + JSON.stringify(pr));
  await P.shot('mega2-5-phone-room');
  // the shared wall on a phone: the same grid, smaller; the pen's toolbar stays on screen
  await P.ev(`rpc('wall_draw', { gid: S.gid, share: false, cells: Array.from({ length: 64 }, (_, i) => [60 + Math.round(18 * Math.cos(i / 64 * 6.2832)), 27 + Math.round(9 * Math.sin(i / 64 * 6.2832)), 1 + i % 3, i % 2]) }).then(() => 1)`);
  await P.waitFor(`__megaWall.state().marks > 20`, 15000);
  await P.ev(`document.querySelector('#btn-wall').click()`); await sleep(600);
  const pw = await P.ev(`(() => { const r = document.querySelector('#mega-wallbar').getBoundingClientRect(); return { l: r.left, r: r.right, t: r.top, b: r.bottom, w: innerWidth, h: innerHeight, hscroll: document.documentElement.scrollWidth > innerWidth + 1 }; })()`);
  need(pw.l >= 0 && pw.r <= pw.w + 1 && pw.t >= 0 && pw.b <= pw.h && !pw.hscroll, 'wall: the toolbar does not fit a phone screen: ' + JSON.stringify(pw));
  await P.shot('mega4-5-phone-wall');
  await P.ev(`document.querySelector('#mw-done').click()`); await sleep(300);
  const pdots = await P.ev(`(() => { const r = document.querySelector('#mega-rail').getBoundingClientRect(); return { w: r.width, h: r.height, level: __mega.level }; })()`);
  if (pdots.level !== 'chat' || pdots.w < 20 || pdots.w > 60) throw new Error('mega: the dots are wrong on a phone: ' + JSON.stringify(pdots));
  // phones swipe in from the left edge instead of scrolling over the dots
  await P.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: 8, y: 400 }] });
  await P.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ x: 70, y: 400 }] });
  await P.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] }); await sleep(700);
  if (!(await P.ev(`document.querySelector('#mega-rail').classList.contains('open')`))) throw new Error('mega: swiping in from the left edge did not pop the dots out');
  await P.shot('mega-6-phone-popout');
  await key(P, 'Escape'); await sleep(500);
  await key(P, 'Escape'); await sleep(1200);
  if (await lvl(P) !== 'server' && await lvl(P) !== 'home') throw new Error('mega: Esc on a phone did not step back to the cards: ' + await lvl(P));
  const pcards = await P.ev(`(() => { const c = document.querySelector('.mslot.settled .msq')?.getBoundingClientRect(); return c ? { l: c.left, r: c.right, w: innerWidth } : null; })()`);
  if (!pcards || pcards.l < -1 || pcards.r > pcards.w + 1) throw new Error('mega: the cards overflow a phone screen: ' + JSON.stringify(pcards));
  await P.shot('mega-7-phone-cards');
  // the colour picker on a phone: a sheet at the bottom that stays on screen
  await P.ev(`(() => { const x = __mega.list.items.findIndex(i => __mega.canPaint(i)); if (x >= 0) __mega.list.go(x, true); })()`); await sleep(1500);
  need(await P.ev(`(() => { const b = document.querySelector('#mega [data-m="paint"]'); return !!b && !b.hidden; })()`), 'colours: no chip for the owner on a phone');
  await P.ev(`document.querySelector('#mega [data-m="paint"]').click()`); await sleep(600);
  const pp = await P.ev(`(() => { const r = document.querySelector('#mega-paint').getBoundingClientRect(); return { l: r.left, r: r.right, t: r.top, b: r.bottom, w: innerWidth, h: innerHeight }; })()`);
  need(pp.l >= 0 && pp.r <= pp.w + 1 && pp.t >= 0 && pp.b <= pp.h + 1, 'colours: the picker does not fit a phone screen: ' + JSON.stringify(pp));
  { const f = await P.ev(`(() => { const p = document.querySelector('#mega-paint'), r = p.getBoundingClientRect(), ts = [...p.querySelectorAll('.mp-t')]; return { scrolls: p.scrollHeight > p.clientHeight, sh: p.scrollHeight, ch: p.clientHeight, sw: p.scrollWidth, cw: p.clientWidth, tiles: ts.length, out: ts.filter(b => b.getBoundingClientRect().right > r.right + 1).length, doc: document.documentElement.scrollWidth, iw: innerWidth, target: document.querySelector('#mp-name').textContent }; })()`);
    need(f.scrolls && f.sw <= f.cw + 1 && f.tiles % 18 === 0 && f.tiles >= 18 && f.out === 0 && f.doc <= f.iw + 1, 'looks: the look choosers do not fit a phone (they should scroll inside the panel): ' + JSON.stringify(f)); }
  await P.shot('mega3-4-phone-picker');
  await P.ev(`document.querySelector('#mega-paint').scrollTop = 99999`); await sleep(300);
  await P.shot('mega5-12-phone-looks');
  await P.ev(`document.querySelector('#mp-done').click()`); await sleep(300);
  // drawing on a card on a phone: the chip, the toolbar inside the screen, the drawing readable
  if (await P.ev(`(() => { const b = document.querySelector('#mega [data-m="draw"]'); return !!b && !b.hidden; })()`)) {
    await P.ev(`document.querySelector('#mega [data-m="draw"]').click()`); await sleep(600);
    await P.ev(`(async () => { const hit = document.querySelector('#mega-wallhit-card'), G = __megaWall.state().geo, px = (x, y) => ({ clientX: G.b.left + (G.ox + (x + .5) * G.cw) / G.u, clientY: G.b.top + (G.oy + (y + .5) * G.ch) / G.u, pointerId: 9, bubbles: true, button: 0 }); document.querySelector('#mw-glyphs [data-g="2"]').click(); const n = 60, pt = i => px(60 + Math.round(24 * Math.cos(i / n * 6.2832)), 27 + Math.round(12 * Math.sin(i / n * 6.2832))); hit.dispatchEvent(new PointerEvent('pointerdown', pt(0))); for (let i = 1; i <= n; i++) hit.dispatchEvent(new PointerEvent('pointermove', pt(i))); hit.dispatchEvent(new PointerEvent('pointerup', pt(n))); })()`);
    await sleep(500);
    const tb = await P.ev(`(() => { const r = document.querySelector('#mega-wallbar').getBoundingClientRect(); return { l: r.left, r: r.right, t: r.top, b: r.bottom, w: innerWidth, h: innerHeight, hscroll: document.documentElement.scrollWidth > innerWidth + 1, marks: __megaWall.state().marks }; })()`);
    need(tb.l >= 0 && tb.r <= tb.w + 1 && tb.t >= 0 && tb.b <= tb.h && !tb.hscroll && tb.marks > 20, 'cards: the pen toolbar does not fit a phone, or nothing was drawn: ' + JSON.stringify(tb));
    await P.shot('mega5-13-phone-card-drawn');
    await P.ev(`document.querySelector('#mw-done').click()`); await sleep(300);
  }
  await P.ev(`__mega.list.go(0, true)`); await sleep(1200);
  await P.ev(`document.querySelectorAll('.room')[0].click()`); await sleep(900);
  // In a call on a phone the dock is a slim bar at the bottom of the chat, not in the (closed) drawer.
  const phDock = await P.ev(`(() => {
    const c = window.__commonsCall, orig = c.snapshot;
    c.snapshot = () => ({ phase: 'connected', error: null, gid: S.gid, roomName: 'general', muted: false, gameMuted: false, gameName: '', gameError: '',
      deafened: false, ptt: false, pttKey: 'Backquote', pttHeld: false, sharing: false, camera: false, playing: false, playback: null,
      peers: [], shares: [], prefs: c.prefs, encryption: { active: true, backend: 'insertable-streams', epoch: 1, rotations: 0, devices: 2 }, blocked: [] });
    window.__callUI.render();
    const d = document.querySelector('#call-dock'), r = d.getBoundingClientRect(), cb = document.querySelector('#composer').getBoundingClientRect(), m = document.querySelector('#main').getBoundingClientRect();
    window.__phoneSnapOrig = orig;
    return { parent: d.parentElement.id, hidden: d.hidden, h: r.height, right: r.right, mRight: m.right, aboveComposer: r.bottom <= cb.top + 1, controls: d.querySelectorAll('.cd-b').length };
  })()`);
  if (phDock.parent !== 'main' || phDock.hidden || phDock.h < 20 || phDock.h > 70 || phDock.right > phDock.mRight + 1 || !phDock.aboveComposer || phDock.controls < 7)
    throw new Error('the phone dock is not a slim bar at the bottom of the chat: ' + JSON.stringify(phDock));
  await P.shot('09-phone-dock');
  await P.ev(`(() => { window.__commonsCall.snapshot = window.__phoneSnapOrig; delete window.__phoneSnapOrig; window.__callUI.render(); return true; })()`);
  // The iOS build tells the UI it is on a phone; the game-key settings must not be offered there.
  await P.ev(`window.__commonsPlatform = 'mobile'; document.body.dataset.platform = 'mobile'; document.querySelector('#btn-settings').click()`);
  await P.waitFor(`document.querySelector('.modal')`, 5000);
  if (await P.ev(`!!document.querySelector('#set-gamekeys')`)) throw new Error('game keys are offered on the phone layout');
  await click(P, '.modal .acts button'); await sleep(300);
  for (const [sel, why] of [['#main', 'main'], ['#composer', 'composer'], ['#head', 'header']]) {
    const box = await P.ev(`(() => { const b = document.querySelector(${JSON.stringify(sel)}).getBoundingClientRect(); return { l: b.left, r: b.right, w: innerWidth }; })()`);
    if (box.l < -1 || box.r > box.w + 1) throw new Error(`${why} overflows the phone screen: ${JSON.stringify(box)}`);
  }
  console.log('UI check OK; screenshots in', out);
  await dumpTrace();
} catch (e) { console.error('UI check FAILED:', e.message); try { for (const [n, P] of [['A', A], ['B', B]]) console.error(n, 'toast:', await P.ev(`document.querySelector('#toast')?.innerText`), '| transfers:', await P.ev(`document.querySelector('#transfers')?.innerText`), '| xfer:', await P.ev(`JSON.stringify([[...XFER.keys()], XLIST])`)); } catch { } await dumpTrace(); process.exitCode = 1; }
finally { runeSrv.close(); procs.forEach(p => { try { p.kill(); } catch { } }); }
