// Room-open measurement: what the screen does when a room card is clicked in a community's card view (the cards fly into dots and the chat opens).
// Real relay + two dev bridges + headless Brave/Chromium on the GPU (ANGLE/Vulkan, never SwiftShader). Alice's page is measured.
// A room is filled with a realistic history first (MSGS messages from both sides, some long, a few images and files, a drawing on the wall),
// the page is loaded cold, the community card is clicked, then the room card, with real mouse events.
// Per run it records, for 1 s after the click: requestAnimationFrame frame-to-frame times (worst, p95, count >16.7 ms, >33 ms), long tasks,
// and the time from the click to the first message row painted. One extra run is traced (CDP Tracing + the V8 sampling profiler): the trace
// is saved and summarised by category (scripting, layout, paint, GC) and by the top JS functions.
// Usage: node tools/room-open-measure.mjs <outdir>
//   ROOM_SIZES=1920x1080,3840x2160  ROOM_RUNS=5  ROOM_MSGS=330  ROOM_SHOTS=1 (screenshots mid-flight and after)  ROOM_UI / ROOM_BIN as in perf-measure.
import { spawn } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';

const out = process.argv[2] || '/tmp/mega-room-open';
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const root = path.resolve(new URL('..', import.meta.url).pathname);
const env = (k, d) => process.env[k] ?? d;
const bin = p => path.join(env('ROOM_BIN', path.join(root, 'target/debug')), p);
const RELAY_PORT = +env('ROOM_RELAY_PORT', 19051), A_PORT = +env('ROOM_A_PORT', 7851), B_PORT = +env('ROOM_B_PORT', 7852), CDP_PORT = +env('ROOM_CDP_PORT', 9783);
const SIZES = env('ROOM_SIZES', '1920x1080,3840x2160').split(',').map(s => s.split('x').map(Number));
const RUNS = +env('ROOM_RUNS', 5), MSGS = +env('ROOM_MSGS', 330), SHOTS = env('ROOM_SHOTS', '1') === '1';
const BROWSER = env('ROOM_BROWSER', '/opt/brave-bin/brave');
const UI = env('ROOM_UI', path.join(root, 'app/ui'));
const sleep = ms => new Promise(r => setTimeout(r, ms));
const procs = [];
const plog = fs.openSync(path.join(out, 'procs.log'), 'a');
const run = (cmd, args, e = {}) => { const p = spawn(cmd, args, { env: { ...process.env, ...e }, stdio: ['ignore', plog, plog] }); procs.push(p); return p; };
const stopAll = () => { for (const p of procs) { try { p.kill(); } catch { } } };
process.on('exit', stopAll); process.on('SIGINT', () => { stopAll(); process.exit(1); });

run(bin('commons-relay'), ['--bind', `127.0.0.1:${RELAY_PORT}`, '--data', path.join(out, 'data/relay')]);
const secret = crypto.randomBytes(32).toString('hex');
for (const [n, port] of [['alice', A_PORT], ['bob', B_PORT]]) run(bin('dev_bridge'), [String(port), UI], { COMMONS_DATA: path.join(out, 'data', n), COMMONS_RELAY: `http://127.0.0.1:${RELAY_PORT}`, COMMONS_FAST_KDF: '1', MEGATRON_SECRET: secret });
const RELAY = `http://127.0.0.1:${RELAY_PORT}`;
const rpcOf = port => async (method, args = {}) => {
  const r = await fetch(`http://127.0.0.1:${port}/rpc`, { method: 'POST', headers: { 'X-Megatron-Secret': secret }, body: JSON.stringify({ method, args }) });
  const j = await r.json(); if ('err' in j) throw new Error(`${method}: ${j.err}`); return j.ok;
};
const A = rpcOf(A_PORT), B = rpcOf(B_PORT);
async function until(f, what, ms = 60000) { const t = Date.now(); let last; while (Date.now() - t < ms) { try { last = await f(); if (last) return last; } catch (e) { last = e.message; } await sleep(250); } throw new Error(`timed out: ${what} (${JSON.stringify(last)})`); }
await until(() => A('state').then(() => true), 'alice up'); await until(() => B('state').then(() => true), 'bob up');
await until(() => fetch(`${RELAY}/health`).then(r => r.ok), 'relay up');
await A('create_account', { passphrase: 'room test pass', name: 'alice', relay: RELAY });
await B('create_account', { passphrase: 'room test pass', name: 'bob', relay: RELAY });

// ---- the world: two communities, the first with three text rooms and a voice room; the history goes in the first text room ----
const comm = (s, cid) => s.communities.find(c => c.id === cid);
const rooms = [];
for (const name of ['Book club', 'Band']) {
  const cid = (await A('create_community', { name })).id;
  const gid = await until(async () => comm(await A('snapshot'), cid)?.channels[0]?.gid, 'first room of ' + name);
  if (name === 'Book club') { for (const n of ['music', 'plans']) await A('create_channel', { cid, name: n, kind: 'text' }).catch(() => { }); }
  await A('create_channel', { cid, name: 'voice', kind: 'voice' }).catch(() => { });
  const inv = (await A('create_invite', { cid, uses: 1 })).link;
  await B('join', { link: inv });
  await until(async () => { const co = comm(await B('snapshot'), cid); return co && !co.removed && co.channels.length > 0; }, 'bob joined ' + name);
  await until(async () => { const co = comm(await B('snapshot'), cid), ch = co && co.channels.find(x => x.gid === gid); if (!ch) return false; if (!ch.joined) { await B('join_channel', { gid }).catch(() => { }); return false; } return true; }, 'bob in ' + name);
  rooms.push({ cid, gid, name });
}
const main = rooms[0];
const png = (w, h, seed) => {   // a photo-ish gradient with noise, as a real PNG
  const crc = (() => { const t = new Int32Array(256); for (let n = 0; n < 256; n++) { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; t[n] = c; } return b => { let c = -1; for (const x of b) c = t[(c ^ x) & 255] ^ (c >>> 8); return (c ^ -1) >>> 0; }; })();
  const raw = Buffer.alloc((w * 3 + 1) * h); let s = seed;
  for (let y = 0; y < h; y++) { raw[y * (w * 3 + 1)] = 0; for (let x = 0; x < w; x++) { s = (s * 1103515245 + 12345) >>> 0; const o = y * (w * 3 + 1) + 1 + x * 3, n = (s >>> 24) % 24; raw[o] = (x * 255 / w + n) & 255; raw[o + 1] = (y * 255 / h + n) & 255; raw[o + 2] = ((x + y) * 255 / (w + h) + seed + n) & 255; } }
  const chunk = (t, d) => { const b = Buffer.concat([Buffer.from(t), d]); const l = Buffer.alloc(4); l.writeUInt32BE(d.length); const c = Buffer.alloc(4); c.writeUInt32BE(crc(b)); return Buffer.concat([l, b, c]); };
  const ih = Buffer.alloc(13); ih.writeUInt32BE(w, 0); ih.writeUInt32BE(h, 4); ih[8] = 8; ih[9] = 2;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ih), chunk('IDAT', zlib.deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
};
const WORDS = 'the band practice moved to thursday so bring the amp and those long cables because last time we ran short; has anyone read chapter nine yet? I thought the ending was a bit rushed but the middle was great, honestly one of the best things we have read this year. Also: lunch? pizza? tacos? whatever works. See https://example.com/some/long/path?with=query for the setlist and notes.'.split(' ');
let rs = 12345; const rnd = () => (rs = (rs * 1103515245 + 12345) >>> 0) / 4294967296;
const text = i => {
  const long = i % 9 === 4, n = long ? 70 + Math.floor(rnd() * 60) : 3 + Math.floor(rnd() * 22);
  let t = ''; for (let k = 0; k < n; k++) t += WORDS[Math.floor(rnd() * WORDS.length)] + (long && k % 24 === 23 ? '\n\n' : ' ');
  return (i % 17 === 0 ? `**msg ${i}** ` : `msg ${i}: `) + t.trim();
};
const sendFile = (who, name, mime, buf, caption) => who('send_file', { gid: main.gid, name, mime, data: buf.toString('base64url'), caption });
for (let i = 0; i < MSGS; i++) {
  const who = i % 3 ? A : B;
  await until(() => who('send_text', { gid: main.gid, text: text(i), reply: null }).then(() => true), 'send ' + i, 30000);
  if (i === 60 || i === 140 || i === 220 || i === 290) await until(() => sendFile(i % 2 ? A : B, `photo-${i}.png`, 'image/png', png(640, 400, i), 'a picture').then(() => true), 'png ' + i, 30000);
  if (i === 100 || i === 250) await until(() => sendFile(B, `notes-${i}.txt`, 'text/plain', Buffer.from('some notes\n'.repeat(200)), '').then(() => true), 'file ' + i, 30000);
}
{ // a drawing on the wall of the room: a few strokes of marks
  const cells = []; for (let a = 0; a < 6; a++) for (let k = 0; k < 120; k++) cells.push([20 + Math.round(30 * Math.cos(k / 19 + a)) + a * 9, 12 + Math.round(10 * Math.sin(k / 9)) + a * 3, 1 + (a % 3), k % 2]);
  for (let i = 0; i < cells.length; i += 120) await A('wall_draw', { gid: main.gid, cells: cells.slice(i, i + 120), share: true }).catch(e => console.log('wall draw:', e.message));
}
await until(async () => (await A('messages', { gid: main.gid, limit: 1000 })).messages.length >= MSGS, 'alice has the history');
await until(async () => (await B('messages', { gid: main.gid, limit: 1000 })).messages.length >= MSGS, 'bob has the history');
console.log('world ready:', MSGS, 'messages');

// ---- browser ----
const [W0, H0] = SIZES[0];
run(BROWSER, ['--headless=new', '--password-store=basic', '--use-mock-keychain', `--remote-debugging-port=${CDP_PORT}`, '--user-data-dir=' + path.join(out, 'chrome'), '--use-gl=angle', '--use-angle=vulkan',
  '--enable-features=Vulkan', '--ignore-gpu-blocklist', '--enable-gpu', '--ozone-platform=headless', '--disable-software-rasterizer', '--mute-audio', '--disable-audio-output',
  '--disable-background-timer-throttling', '--disable-renderer-backgrounding', '--disable-backgrounding-occluded-windows', '--no-first-run', `--window-size=${W0},${H0}`, 'about:blank']);
let ver; for (let i = 0; i < 50; i++) { try { ver = await (await fetch(`http://127.0.0.1:${CDP_PORT}/json/version`)).json(); break; } catch { await sleep(200); } }
if (!ver) throw new Error('browser did not start');
class Cdp {
  constructor(ws) { this.id = 0; this.cb = new Map(); this.on = {}; this.ws = ws; ws.onmessage = m => { const j = JSON.parse(m.data); if (j.id && this.cb.has(j.id)) { this.cb.get(j.id)(j); this.cb.delete(j.id); } else if (j.method && this.on[j.method]) this.on[j.method](j.params); }; }
  static async open(url) { const ws = new WebSocket(url); await new Promise(r => ws.onopen = r); return new Cdp(ws); }
  send(method, params = {}) { return new Promise(res => { const id = ++this.id; this.cb.set(id, res); this.ws.send(JSON.stringify({ id, method, params })); }); }
  async ev(expr) { const r = await this.send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true }); if (r.result.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails).slice(0, 400)); return r.result.result.value; }
  async waitFor(expr, ms = 30000) { const t = Date.now(); while (Date.now() - t < ms) { if (await this.ev(expr).catch(() => false)) return; await sleep(200); } throw new Error('timeout waiting for ' + expr); }
}
const browser = await Cdp.open(ver.webSocketDebuggerUrl);
const tab = (await (await fetch(`http://127.0.0.1:${CDP_PORT}/json/list`)).json()).find(t => t.type === 'page');
const P = await Cdp.open(tab.webSocketDebuggerUrl);
await P.send('Page.enable'); await P.send('Runtime.enable');
await P.send('Page.addScriptToEvaluateOnNewDocument', { source: "try { localStorage.setItem('commons.tour', '1'); } catch {}" });

const centre = async sel => JSON.parse(await P.ev(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); const r = e.getBoundingClientRect(); return JSON.stringify({ x: r.left + r.width / 2, y: r.top + r.height / 2 }); })()`));
const click = async (x, y) => { for (const type of ['mouseMoved', 'mousePressed', 'mouseReleased']) await P.send('Input.dispatchMouseEvent', { type, x, y, button: 'left', buttons: type === 'mousePressed' ? 1 : 0, clickCount: 1 }); };

// the page-side recorder: armed before the click, read after one second
const RECORDER = `(() => {
  const R = window.__rm = { t0: 0, frames: [], long: [], firstRow: null, flyAt: null, done: false, rows0: document.querySelectorAll('#stream .msg').length };
  addEventListener('click', e => { if (!R.t0) { R.t0 = performance.now(); R.rows0 = document.querySelectorAll('#stream .msg').length; start(); } }, { capture: true, once: true });
  new PerformanceObserver(l => { for (const e of l.getEntries()) R.long.push({ at: e.startTime - R.t0, dur: e.duration }); }).observe({ type: 'longtask', buffered: false });
  const fx = document.getElementById('mega-fx'); if (fx) new MutationObserver(() => { if (R.t0 && R.flyAt == null && fx.querySelector('.mfly')) R.flyAt = performance.now() - R.t0; }).observe(fx, { childList: true });
  const st = document.getElementById('stream'); new MutationObserver(() => { if (R.t0 && R.firstRow == null && st.querySelector('.msg')) { const n = performance.now() - R.t0; requestAnimationFrame(() => requestAnimationFrame(() => { R.firstRow = performance.now() - R.t0; R.firstRowDom = n; })); } }).observe(st, { childList: true, subtree: true });
  function start() { let last = R.t0; const tick = t => { R.frames.push(t - last); last = t; if (t - R.t0 < 1000) requestAnimationFrame(tick); else R.done = true; }; requestAnimationFrame(tick); }
  return 1; })()`;

async function toRoomCards(w, h) {
  await P.send('Emulation.setDeviceMetricsOverride', { width: w, height: h, deviceScaleFactor: 1, mobile: false });
  await P.send('Page.navigate', { url: 'about:blank' }); await sleep(300);
  await P.send('Page.navigate', { url: `http://127.0.0.1:${A_PORT}/#s=${secret}` });
  await P.waitFor(`!document.querySelector('#app').hidden && window.__mega && __mega.started && !!S.snap && document.querySelectorAll('.mcard').length >= 2`);
  await sleep(1500);
  // home -> the community: pick the Book club card by scrolling it into focus (keys), then a real click on it
  await P.ev(`(() => { const L = __mega.list; const i = L.items.findIndex(x => x.cid === ${JSON.stringify(main.cid)}); L.go(i, true); return i; })()`).catch(() => { });
  await sleep(900);
  const c = await centre('.mslot.settled .mcard'); await click(c.x, c.y);
  await P.waitFor(`__mega.level === 'server' && document.querySelectorAll('.mcard').length >= 3 && !!document.querySelector('.mslot.settled')`);
  await sleep(1800);
  // make the text room with the history the focused card
  await P.ev(`(() => { const L = __mega.list; const i = L.items.findIndex(x => x.gid === ${JSON.stringify(main.gid)} && x.kind === 'text'); L.go(i, true); return i; })()`);
  await sleep(1500);
  return await P.ev(`JSON.stringify({ cur: __mega.list.item && __mega.list.item.gid, want: ${JSON.stringify(main.gid)}, level: __mega.level })`);
}

async function oneRun(w, h, tag, { trace = false, shots = false } = {}) {
  const st = await toRoomCards(w, h);
  if (JSON.parse(st).cur !== main.gid) throw new Error('room card not focused: ' + st);
  await P.ev(RECORDER);
  let events = [], prof = null;
  if (trace) {
    P.on['Tracing.dataCollected'] = p => { events.push(...p.value); };
    await P.send('Profiler.enable'); await P.send('Profiler.setSamplingInterval', { interval: 200 }); await P.send('Profiler.start');
    await P.send('Tracing.start', { transferMode: 'ReportEvents', categories: 'devtools.timeline,disabled-by-default-devtools.timeline,v8.execute,blink.user_timing,loading,latencyInfo,cc,viz,gpu' });
    await sleep(300);
  }
  if (env('ROOM_PROBE', '')) await P.ev(fs.readFileSync(env('ROOM_PROBE'), 'utf8'));
  const c = await centre('.mslot.settled .mcard');
  await click(c.x, c.y);
  let mid = null;
  if (shots) { await sleep(150); await P.send('Page.captureScreenshot', { format: 'png' }).then(r => fs.writeFileSync(path.join(out, `${tag}-flight.png`), Buffer.from(r.result.data, 'base64'))); }
  await P.waitFor(`window.__rm && __rm.done`, 8000);
  await sleep(300);
  if (shots) { await P.send('Page.captureScreenshot', { format: 'png' }).then(r => fs.writeFileSync(path.join(out, `${tag}-after.png`), Buffer.from(r.result.data, 'base64'))); }
  if (trace) {
    const done = new Promise(r => P.on['Tracing.tracingComplete'] = r);
    await P.send('Tracing.end'); await done;
    prof = (await P.send('Profiler.stop')).result.profile;
  }
  if (env('ROOM_PROBE', '')) console.log('probe:', await P.ev(`JSON.stringify(window.__probe)`));
  const R = JSON.parse(await P.ev(`JSON.stringify(window.__rm)`));
  const st2 = JSON.parse(await P.ev(`JSON.stringify({ level: __mega.level, gid: S.gid, rows: document.querySelectorAll('#stream .msg').length, bd: !!(__mega.bdInfo && __mega.bdInfo()) })`));
  const f = R.frames.slice(1).sort((a, b) => a - b);   // the first interval includes the click's own task
  const res = { worst: +Math.max(...R.frames).toFixed(1), p95: +f[Math.floor(f.length * .95)].toFixed(1), over16: R.frames.filter(x => x > 16.7 + 1).length, over33: R.frames.filter(x => x > 33.4 + 1).length, frames: R.frames.length,
    longTasks: R.long.length, longMs: +R.long.reduce((a, b) => a + b.dur, 0).toFixed(0), longestMs: R.long.length ? +Math.max(...R.long.map(x => x.dur)).toFixed(0) : 0, firstRowMs: R.firstRow == null ? null : +R.firstRow.toFixed(0), flyAtMs: R.flyAt == null ? null : +R.flyAt.toFixed(0), rows: st2.rows, backdrop: st2.bd, level: st2.level, long: R.long.map(x => `${x.at.toFixed(0)}+${x.dur.toFixed(0)}`) };
  if (trace) { fs.writeFileSync(path.join(out, `${tag}-trace.json`), JSON.stringify({ traceEvents: events })); res.trace = summarise(events, prof, R, tag); }
  return res;
}

// trace summary: self time on the renderer main thread in the second after the click, by category, and the top JS functions from the sampling profile
function summarise(events, prof, R, tag) {
  const click = events.find(e => e.name === 'EventDispatch' && e.args?.data?.type === 'click' && e.ph === 'X');
  const t0 = click ? click.ts : null;
  const main = events.find(e => e.name === 'thread_name' && e.args?.name === 'CrRendererMain');
  const pid = main?.pid, tid = main?.tid;
  const xs = events.filter(e => e.ph === 'X' && e.pid === pid && e.tid === tid && e.dur != null && t0 != null && e.ts >= t0 - 1000 && e.ts <= t0 + 1e6).sort((a, b) => a.ts - b.ts || b.dur - a.dur);
  const self = new Map(); const stack = [];
  for (const e of xs) {
    while (stack.length && stack[stack.length - 1].end <= e.ts) stack.pop();
    if (stack.length) stack[stack.length - 1].child += e.dur;
    const o = { e, end: e.ts + e.dur, child: 0 }; stack.push(o); o.self = () => Math.max(0, e.dur - o.child); (self.set(e, o));
  }
  const cat = { scripting: 0, layout: 0, paint: 0, gc: 0, other: 0 }, byName = {};
  const classify = n => /^(FunctionCall|EvaluateScript|v8\.run|v8\.compile|V8\.|RunMicrotasks|TimerFire|FireAnimationFrame|FireIdleCallback|EventDispatch|XHRReadyStateChange|ParseHTML|ResourceFinish|HitTest|ScheduledAction|v8\.callFunction)/.test(n) ? 'scripting'
    : /^(Layout|UpdateLayoutTree|RecalculateStyles|IntersectionObserverController|ComputeIntersections|UpdateLayer)/.test(n) ? 'layout'
    : /^(Paint|PrePaint|Layerize|Commit|CompositeLayers|RasterTask|Decode|ImageDecode|UpdateLayerTree|PaintImage|Animation|HitTest)/.test(n) ? 'paint'
    : /GC/.test(n) ? 'gc' : 'other';
  for (const [e, o] of self) { const s = o.self(); if (e.name === 'RunTask' || e.name === 'ThreadControllerImpl::RunTask') { byName.RunTask = (byName.RunTask || 0) + s; continue; } const k = classify(e.name); cat[k] += s; byName[e.name] = (byName[e.name] || 0) + s; }
  const ms = v => +(v / 1000).toFixed(1);
  // JS functions by self time from the cpu profile (samples in the second after the click)
  const fn = new Map(); let total = 0;
  if (prof && t0 != null) {
    const nodes = new Map(prof.nodes.map(n => [n.id, n])); let t = prof.startTime;
    const parent = new Map(); for (const n of prof.nodes) for (const c of n.children || []) parent.set(c, n.id);
    // profile time is in the same clock (microseconds, monotonic) as trace ts in practice
    for (let i = 0; i < prof.samples.length; i++) { t += prof.timeDeltas[i]; if (t < t0 || t > t0 + 1e6) continue; const n = nodes.get(prof.samples[i]); const cf = n.callFrame; const name = cf.functionName || '(anon)'; if (name === '(idle)' || name === '(program)' || name === '(root)') continue; let site = n, hops = 0; while (site && !site.callFrame.url && hops++ < 50) site = nodes.get(parent.get(site.id)); const sf = site && site.callFrame; const k = `${name} <- ${sf ? sf.functionName + ' ' + sf.url.split('/').pop() + ':' + (sf.lineNumber + 1) : '?'}`; const d = prof.timeDeltas[i + 1] || 200; fn.set(k, (fn.get(k) || 0) + d); total += d; }
  }
  const top = [...fn].sort((a, b) => b[1] - a[1]).slice(0, 12).map(([k, v]) => `${ms(v)} ms  ${k}`);
  const topEv = Object.entries(byName).sort((a, b) => b[1] - a[1]).slice(0, 12).map(([k, v]) => `${ms(v)} ms  ${k}`);
  return { clickFound: !!click, mainThreadSelfMs: { scripting: ms(cat.scripting), layout: ms(cat.layout), paint: ms(cat.paint), gc: ms(cat.gc), other: ms(cat.other) }, topEvents: topEv, topJsFunctions: top, jsSampledMs: ms(total) };
}

const med = a => { const s = [...a].sort((x, y) => x - y); return s[Math.floor(s.length / 2)]; };
const all = {};
try {
  for (const [w, h] of SIZES) {
    const key = `${w}x${h}`, runs = [];
    for (let r = 0; r < RUNS; r++) {
      const x = await oneRun(w, h, `${key}-run${r}`, { shots: SHOTS && r === 0 });
      runs.push(x); console.log(key, 'run', r, JSON.stringify({ ...x, long: x.long.join(' ') }));
    }
    const tr = await oneRun(w, h, `${key}-traced`, { trace: true });
    console.log(key, 'traced', JSON.stringify(tr.trace, null, 1), 'frames:', tr.worst, tr.over16, tr.over33);
    const m = k => med(runs.map(x => x[k]));
    all[key] = { median: { worstFrameMs: m('worst'), p95FrameMs: m('p95'), framesOver16_7: m('over16'), framesOver33: m('over33'), longTasks: m('longTasks'), longTaskTotalMs: m('longMs'), longestTaskMs: m('longestMs'), clickToFirstRowMs: m('firstRowMs') }, runs, traced: tr };
    console.log(key, 'MEDIAN', JSON.stringify(all[key].median));
  }
  fs.writeFileSync(path.join(out, 'room-open.json'), JSON.stringify(all, null, 1));
  console.log('\nwrote', path.join(out, 'room-open.json'));
} finally { stopAll(); }
process.exit(0);
