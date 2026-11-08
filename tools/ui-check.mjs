// Headless UI check: real relay + two dev bridges (two "windows") + Chromium on the GPU (ANGLE/Vulkan, never SwiftShader).
// No window is ever shown. Usage: node tools/ui-check.mjs <outdir>
import { spawn, execSync } from 'node:child_process';
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
run(bin('commons-relay'), ['--bind', `127.0.0.1:${RELAY_PORT}`, '--data', path.join(out, 'data/relay')]);
for (const [n, port] of [['alice', A_PORT], ['bob', B_PORT]]) run(bin('dev_bridge'), [String(port), path.join(root, 'app/ui')], { COMMONS_DATA: path.join(out, 'data', n), COMMONS_RELAY: RELAY, COMMONS_FAST_KDF: '1' });
await sleep(1500);

const chrome = run('chromium', ['--headless=new', `--remote-debugging-port=${CDP_PORT}`, '--user-data-dir=' + path.join(out, 'chrome'), '--use-gl=angle', '--use-angle=vulkan',
  '--enable-features=Vulkan', '--ignore-gpu-blocklist', '--enable-gpu', '--ozone-platform=headless', '--disable-software-rasterizer', '--mute-audio', '--disable-audio-output', '--no-first-run', '--window-size=1280,800', 'about:blank']);
let ver; for (let i = 0; i < 50; i++) { try { ver = await (await fetch(`http://127.0.0.1:${CDP_PORT}/json/version`)).json(); break; } catch { await sleep(200); } }
if (!ver) throw new Error('chromium did not start');

class Page {
  constructor(ws) { this.id = 0; this.cb = new Map(); this.ws = ws; ws.onmessage = m => { const j = JSON.parse(m.data); if (j.id && this.cb.has(j.id)) { this.cb.get(j.id)(j); this.cb.delete(j.id); } }; }
  send(method, params = {}) { return new Promise(res => { const id = ++this.id; this.cb.set(id, res); this.ws.send(JSON.stringify({ id, method, params })); }); }
  async ev(expr) { const r = await this.send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true }); if (r.result.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails).slice(0, 400)); return r.result.result.value; }
  async shot(name) { await this.send('Page.bringToFront'); await sleep(250); const r = await this.send('Page.captureScreenshot', { format: 'png' }); fs.writeFileSync(path.join(out, name + '.png'), Buffer.from(r.result.data, 'base64')); }
  async waitFor(expr, ms = 15000) { const t = Date.now(); while (Date.now() - t < ms) { if (await this.ev(expr).catch(() => false)) return; await sleep(250); } throw new Error('timeout waiting for ' + expr); }
}
async function open(url) {
  const t = await (await fetch(`http://127.0.0.1:${CDP_PORT}/json/new?` + encodeURIComponent(url), { method: 'PUT' })).json();
  const ws = new WebSocket(t.webSocketDebuggerUrl); await new Promise(r => ws.onopen = r);
  const p = new Page(ws); await p.send('Page.enable'); await p.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false }); return p;
}
const type = (p, sel, v) => p.ev(`(()=>{const e=document.querySelector(${JSON.stringify(sel)});e.value=${JSON.stringify(v)};e.dispatchEvent(new Event('input',{bubbles:true}));})()`);
const click = (p, sel) => p.ev(`document.querySelector(${JSON.stringify(sel)}).click()`);
const textOf = (p, sel) => p.ev(`document.querySelector(${JSON.stringify(sel)})?.innerText||''`);

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
  const titled = await B.ev(`(async () => { for (let i = 0; i < 25; i++) { if (/^[(][0-9]+[)] Megatron$/.test(document.title)) return 'ok'; await new Promise(r => setTimeout(r, 200)); } return document.title + ' ' + JSON.stringify(S.snap.communities.map(c => c.channels.map(x => x.name + ':' + x.unread))); })()`);
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
  await B.waitFor(`![...document.querySelectorAll('.person .listen')].some(e=>e.innerText.includes('Detective'))`, 30000);

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
  // The call panel uses the same seal; the fill hook that runs after each of its renders does the same here.
  const tile = await B.ev(`(() => {
    const m = S.members.find(x => !x.me) || { account: 'x', name: 'x' };
    const host = document.createElement('div'); host.className = 'cpeople';
    host.innerHTML = '<div class="av sealed">' + seal(m.account, m.name) + '</div>';
    document.body.appendChild(host);
    window.__commonsFillAvatars(host);
    const ok = !!host.querySelector('.av.sealed .seal img.av-img');
    host.remove();
    return ok;
  })()`);
  if (!tile) throw new Error('a call tile did not get the picture');
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
  // Phone-width layout: the rail folds away, the composer stays on screen, and desktop-only settings are gone.
  const P = await open('http://127.0.0.1:7101/');
  await P.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 3, mobile: true });
  await P.waitFor(`!document.querySelector('#app').hidden`, 20000);
  await P.waitFor(`document.querySelectorAll('.room').length>=1`, 20000);
  if (await P.ev(`document.querySelector('#composer').hidden`)) { await P.ev(`document.querySelectorAll('.room')[0].click()`); }
  await P.waitFor(`!document.querySelector('#composer').hidden`, 10000); await sleep(500);
  const phone = await P.ev(`(() => { const r = document.querySelector('#rail').getBoundingClientRect(), c = document.querySelector('#composer').getBoundingClientRect(); return { w: innerWidth, railAway: r.right <= 1, composerIn: c.height > 0 && c.top > 0 && c.bottom <= innerHeight + 1 }; })()`);
  if (phone.w > 760 || !phone.railAway || !phone.composerIn) throw new Error('narrow layout is wrong: ' + JSON.stringify(phone));
  await P.shot('09-phone');
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
