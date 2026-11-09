// Headless UI check: real relay + two dev bridges (two "windows") + Chromium on the GPU (ANGLE/Vulkan, never SwiftShader).
// No window is ever shown. Usage: node tools/ui-check.mjs <outdir>
import { spawn, execSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';

const out = process.argv[2] || '/tmp/commons-ui';
fs.mkdirSync(out, { recursive: true });
const root = path.resolve(new URL('..', import.meta.url).pathname);
const bin = p => path.join(root, 'target/debug', p);
const sleep = ms => new Promise(r => setTimeout(r, ms));
const procs = [];
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
run(bin('commons-relay'), ['--bind', `127.0.0.1:${RELAY_PORT}`, '--data', path.join(out, 'data/relay')]);
for (const [n, port] of [['alice', A_PORT], ['bob', B_PORT]]) {
  const env = { COMMONS_DATA: path.join(out, 'data', n), COMMONS_RELAY: RELAY, COMMONS_FAST_KDF: '1' };
  if (n === 'bob') Object.assign(env, { PATH: fakeBin + ':' + process.env.PATH, COMMONS_XDG_MARKER: xdgMarker, COMMONS_PICKED: path.join(out, 'downloads-bob') });
  run(bin('dev_bridge'), [String(port), path.join(root, 'app/ui')], env);
}
await sleep(1500);

const chrome = run('chromium', ['--headless=new', '--password-store=basic', '--use-mock-keychain', `--remote-debugging-port=${CDP_PORT}`, '--user-data-dir=' + path.join(out, 'chrome'), '--use-gl=angle', '--use-angle=vulkan',
  '--enable-features=Vulkan', '--ignore-gpu-blocklist', '--enable-gpu', '--ozone-platform=headless', '--disable-software-rasterizer', '--mute-audio', '--disable-background-timer-throttling', '--disable-renderer-backgrounding', '--disable-backgrounding-occluded-windows', '--disable-audio-output', '--no-first-run', '--window-size=1280,800', 'about:blank']);
let ver; for (let i = 0; i < 50; i++) { try { ver = await (await fetch(`http://127.0.0.1:${CDP_PORT}/json/version`)).json(); break; } catch { await sleep(200); } }
if (!ver) throw new Error('chromium did not start');

class Page {
  constructor(ws) { this.id = 0; this.cb = new Map(); this.ws = ws; ws.onmessage = m => { const j = JSON.parse(m.data); if (j.id && this.cb.has(j.id)) { this.cb.get(j.id)(j); this.cb.delete(j.id); } }; }
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
  const p = new Page(ws); await p.send('Page.enable'); await p.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false }); return p;
}
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

try {
  const A = await open(`http://127.0.0.1:${A_PORT}/`); const B = await open(`http://127.0.0.1:${B_PORT}/`);
  await sleep(800);
  await A.shot('01-onboarding');
  for (const [P, name] of [[A, 'Alice'], [B, 'Bob']]) {
    await P.waitFor(`!!document.querySelector('#nm')`);
    await type(P, '#nm', name); await type(P, '#pp', 'correct horse battery'); await type(P, '#pp2', 'correct horse battery'); await click(P, '#go');
    await P.waitFor(`document.querySelector('.modal')`); await sleep(300);
    if (name === 'Alice') await A.shot('02-recovery-key');
    await click(P, '.modal .acts button');
    await P.waitFor(`!document.querySelector('#app').hidden`);
  }
  // Alice makes a community and an invite
  await click(A, '#switcher'); await click(A, '#mk-new'); await type(A, '#n', 'Book club'); await click(A, '.modal .acts button:last-child');
  await A.waitFor(`document.querySelector('.room')`);
  await click(A, '#btn-invite'); await click(A, '.modal .acts button:last-child');
  await A.waitFor(`document.querySelector('.modal .code')`);
  const link = await textOf(A, '.modal .code'); await A.shot('03-invite'); await click(A, '.modal .acts button');
  // Bob joins with it
  await click(B, '#switcher'); await click(B, '#mk-join'); await type(B, '#l', link.trim()); await click(B, '.modal .acts button:last-child');
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
  await B.waitFor(`![...document.querySelectorAll('.person .listen')].some(e=>e.innerText.includes('Detective'))`, 90000);

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
    return { inRail: !!d && d.parentElement.id === 'rail', hidden: !d || d.hidden, status: d && d.querySelector('.cd-phase') && d.querySelector('.cd-phase').textContent,
      name: room.name, room: d && d.querySelector('.cd-room') && d.querySelector('.cd-room').textContent, lock: !!d && !!d.querySelector('button.call-enc.ok'),
      title: d && d.querySelector('#c-enc') && d.querySelector('#c-enc').title, buttons: d ? [...d.querySelectorAll('.cd-b')].map(b => b.id) : [],
      labels: !!d && [...d.querySelectorAll('.cd-b')].every(b => b.title && b.getAttribute('aria-label')),
      mutedRed: !!d && !!d.querySelector('#c-mute.on') && d.querySelector('#c-mute').getAttribute('aria-pressed') === 'true' };
  })()`);
  if (!dock.inRail || dock.hidden || dock.status !== 'Voice connected' || dock.room !== dock.name) throw new Error('the call dock is not the connected status card in the rail: ' + JSON.stringify(dock));
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

  // ---- mega: the navigation around the chat ----
  // The old rail is kept only for its dock; people move through home -> a community's rooms -> the chat with dots.
  const key = async (P, k, mods = 0, code) => {
    const vk = k === 'Escape' ? 27 : k === 'Enter' ? 13 : k.length === 1 ? k.toUpperCase().charCodeAt(0) : 0;
    await P.send('Input.dispatchKeyEvent', { type: 'keyDown', key: k, code: code || k, modifiers: mods, windowsVirtualKeyCode: vk });
    await P.send('Input.dispatchKeyEvent', { type: 'keyUp', key: k, code: code || k, modifiers: mods });
  };
  const lvl = P => P.ev(`__mega.level`);
  const need = (cond, why) => { if (!cond) throw new Error('mega: ' + why); };
  // entering a room shows the dots and the existing stream
  need(await lvl(A) === 'chat', 'a chat should be open, level is ' + await lvl(A));
  const chat = await A.ev(`(() => { const r = document.querySelector('#mega-rail'), s = document.querySelector('#stream').getBoundingClientRect(), d = [...document.querySelectorAll('#mega-rail .mitem')];
    return { rail: !r.hidden && r.getBoundingClientRect().width > 20, dots: d.length, active: d.filter(x => x.classList.contains('active')).length, voice: d.filter(x => x.classList.contains('voice')).length, opt: d.filter(x => x.classList.contains('opt')).length,
      stream: s.width > 200 && s.left >= r.getBoundingClientRect().right - 1, oldRailGone: document.querySelector('#rooms').getBoundingClientRect().width === 0 }; })()`);
  need(chat.rail && chat.dots >= 4 && chat.active === 1 && chat.voice >= 1 && chat.opt === 1 && chat.stream && chat.oldRailGone, 'the chat does not show the dots beside the stream: ' + JSON.stringify(chat));
  await A.shot('mega-1-chat');
  // Esc closes the People sheet first when it is open
  if (await A.ev(`S.people`)) { await key(A, 'Escape'); await sleep(400); need(await A.ev(`!S.people && !document.querySelector('#people:not([hidden])')`), 'Esc did not close the People sheet'); }
  // wheel over the dots pops them out as small cards; Esc closes that first, leaving the chat where it was
  await A.ev(`document.querySelector('#mega-rail').dispatchEvent(new WheelEvent('wheel', { deltaY: 40, bubbles: true, cancelable: true }))`); await sleep(700);
  need(await A.ev(`document.querySelector('#mega-rail').classList.contains('open')`), 'wheel over the dots did not pop them out');
  await A.shot('mega-2-popout');
  await key(A, 'Escape'); await sleep(500);
  need(!(await A.ev(`document.querySelector('#mega-rail').classList.contains('open')`)) && await lvl(A) === 'chat', 'Esc should only close the pop-out first');
  // options room: client-only, with the existing controls and their ids
  await A.ev(`document.querySelector('#mega-rail .mitem.opt').click()`); await sleep(500);
  const op = await A.ev(`(() => { const o = document.querySelector('#mega-opts'); return { open: !o.hidden, ids: ['snd', 'th', 'ty', 'dl-dir'].filter(i => !o.querySelector('#' + i)), privacy: o.innerText.includes('the relay can see that a message-sized packet was sent at that moment'), noRelay: !o.querySelector('#rl') && !o.querySelector('#n') }; })()`);
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
  await A.ev(`document.querySelector('.mslot.settled .mcard').dispatchEvent(new MouseEvent('click', { bubbles: true }))`); await sleep(300);
  need(await A.ev(`window.__joined === null`), 'clicking the body of a voice card joined its call');
  await key(A, 'Enter'); await sleep(300);
  need(await A.ev(`!!window.__joined && window.__joined[0] === ${JSON.stringify(vgid)}`), 'Enter on a voice card did not join it');
  // jump to: picking a voice room focuses its card but never joins
  await A.ev(`window.__joined = null`);
  await key(A, 'k', 2, 'KeyK'); await sleep(400);
  need(await A.ev(`!!document.querySelector('#mega-pal')`), 'Ctrl+K did not open jump to');
  await A.ev(`(() => { const q = document.querySelector('#m-pq'); q.value = 'lounge'; q.dispatchEvent(new Event('input')); })()`);
  await key(A, 'Enter'); await sleep(900);
  need(await A.ev(`!document.querySelector('#mega-pal') && __mega.level === 'server' && __mega.list.item.name === 'lounge' && window.__joined === null`), 'jump to a voice room should focus its card without joining');
  // a second community makes a home; the cards are communities plus the "you" card
  await A.ev(`void __megaNewCommunity(), 0`); await A.waitFor(`document.querySelector('.modal #n')`); await type(A, '#n', 'Second place'); await click(A, '.modal .acts button:last-child');
  await A.waitFor(`__mega.level === 'server' && __mega.list.items.some(x => x.cid && S.snap.communities.find(c => c.id === x.cid && c.name === 'Second place'))`, 60000);
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
    await key(A, 'Escape'); await sleep(900);
    need(await lvl(A) === 'server' && await A.ev(`!document.querySelector('#mega-bd') && !document.documentElement.classList.contains('mg-room')`), 'the backdrop must stop completely on the card levels');
    await key(A, 'Enter'); await sleep(1500);
    need(await lvl(A) === 'chat' && await bdLive() > 50, 'the backdrop did not come back with the room');
    // history never decodes or ripples: open the other room, then this one again
    const s0 = await st(A);
    await A.ev(`document.querySelector('#mega-rail .mitem:not(.voice):not(.opt):not(.active)').click()`); await sleep(1800);
    await A.ev(`document.querySelector('#mega-rail .mitem:not(.voice):not(.opt):not(.active)').click()`); await sleep(1800);
    const s1 = await st(A);
    need(s1.decodes === s0.decodes && s1.ripples === s0.ripples && s1.lands === s0.lands, 'opening rooms (history) must not decode or ripple: ' + JSON.stringify([s0, s1]));
    need(await A.ev(`S.gid === ${JSON.stringify(textGid)}`), 'could not get back to the club room');
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
    for (let i = 0; i < 18; i++) await A.ev(`rpc('send_text', { gid: ${JSON.stringify(textGid)}, text: 'filler line number ${i} to make the room long enough to scroll back through', reply: null }).then(() => 1)`);
    await A.waitFor(`document.querySelectorAll('.msg').length > 22`, 15000); await sleep(1500);
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
    run(bin('dev_bridge'), [String(C_PORT), path.join(root, 'app/ui')], { COMMONS_DATA: path.join(out, 'data', 'carol'), COMMONS_RELAY: RELAY, COMMONS_FAST_KDF: '1' });
    await sleep(1500);
    const C = await open(`http://127.0.0.1:${C_PORT}/`);
    await C.waitFor(`!!document.querySelector('#nm')`);
    await type(C, '#nm', 'Carol'); await type(C, '#pp', 'correct horse battery'); await type(C, '#pp2', 'correct horse battery'); await click(C, '#go');
    await C.waitFor(`document.querySelector('.modal')`); await sleep(300); await click(C, '.modal .acts button');
    await C.waitFor(`!document.querySelector('#app').hidden`);
    const inv = await A.ev(`rpc('create_invite', { cid: ${JSON.stringify(cid)}, uses: 1 }).then(r => r.link)`);
    await C.ev(`rpc('join', { link: ${JSON.stringify(inv)} }).then(() => 1)`);
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
    await sleep(2500);
    const live = await B.ev(`rpc('live_poll', { since: 0 }).then(r => r.events.filter(e => e.kind === 'probe').map(e => e.data + ':' + e.account.slice(0, 4)))`);
    need(live.length === 1 && live[0].startsWith('hello:'), 'live: the event did not arrive exactly once: ' + JSON.stringify(live));
    need(await B.ev(`rpc('messages', { gid: ${JSON.stringify(textGid)}, limit: 200 }).then(m => m.messages.length)`) === msgsBefore, 'live: an event showed up in the history');
    for (const Q of [A, B]) need(await Q.ev(`rpc('notices').then(n => n.filter(x => x.kind === 'newer').length)`) === 0, 'live: a newer-version notice appeared between two current clients');
    // back into the chat for what follows
    await key(A, 'Enter'); await sleep(1500);
    need(await lvl(A) === 'chat', 'colours: could not get back into the chat');
  }

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
  const P = await open(`http://127.0.0.1:${A_PORT}/`);
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
  await P.shot('mega3-4-phone-picker');
  await P.ev(`document.querySelector('#mp-done').click()`); await sleep(300);
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
} catch (e) { console.error('UI check FAILED:', e.message); process.exitCode = 1; }
finally { runeSrv.close(); procs.forEach(p => { try { p.kill(); } catch { } }); }
