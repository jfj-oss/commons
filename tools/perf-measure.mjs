// Performance measurement: idle CPU of the page and the core, JS heap, and request/byte counts page<->core and core<->relay,
// at the home cards, in an open chat, in a chat while the other side talks, and with the tab hidden.
// Real relay + two dev bridges (alice measured, bob the other member) + headless Brave/Chromium on the GPU (ANGLE/Vulkan).
// While the other side talks it also measures how long a message takes from the send call to its row on the screen.
// Usage: node tools/perf-measure.mjs <outdir>     (ports and lengths via PERF_* below; PERF_BIN and PERF_UI point at another
// build's binaries and page, to measure a baseline with the same harness). Prints a table and writes <outdir>/perf.json.
import { spawn } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';

const out = process.argv[2] || '/tmp/mega-perf';
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const root = path.resolve(new URL('..', import.meta.url).pathname);
const bin = p => path.join(process.env.PERF_BIN || path.join(root, 'target/debug'), p);
const env = (k, d) => process.env[k] ?? d;
const RELAY_PORT = +env('PERF_RELAY_PORT', 18891), PROXY_PORT = +env('PERF_PROXY_PORT', 18892);
const A_PORT = +env('PERF_A_PORT', 7691), B_PORT = +env('PERF_B_PORT', 7692), CDP_PORT = +env('PERF_CDP_PORT', 9601);
const SECS = +env('PERF_SECS', 60), SOAK_MIN = +env('PERF_SOAK_MIN', 0), MSGS = +env('PERF_MSGS', 60);
const BROWSER = env('PERF_BROWSER', '/opt/brave-bin/brave');
const UI = env('PERF_UI', path.join(root, 'app/ui'));
const sleep = ms => new Promise(r => setTimeout(r, ms));
const procs = [];
const plog = fs.openSync(path.join(out, 'procs.log'), 'a');
const run = (cmd, args, e = {}) => { const p = spawn(cmd, args, { env: { ...process.env, ...e }, stdio: ['ignore', plog, plog] }); procs.push(p); return p; };
const stopAll = () => { for (const p of procs) { try { p.kill(); } catch { } } };
process.on('exit', stopAll); process.on('SIGINT', () => { stopAll(); process.exit(1); });

// ---- counting proxy in front of the relay: every byte and request line the cores send ----
const NET = { reset() { this.conns = 0; this.up = 0; this.down = 0; this.by = {}; }, note(line) { const m = /^(GET|POST|PUT|DELETE) (\S+) HTTP/.exec(line); if (!m) return; const r = m[2].replace(/\/[0-9a-zA-Z_-]{16,}/g, '/*').replace(/\?.*/, ''); const k = m[1] + ' ' + r; this.by[k] = (this.by[k] || 0) + 1; } };
NET.reset();
const proxy = net.createServer(c => {
  NET.conns++;
  const s = net.connect(RELAY_PORT, '127.0.0.1');
  c.on('data', d => { NET.up += d.length; const t = d.toString('latin1'); let i = 0; while (i < t.length) { const nl = t.indexOf('\n', i); const line = t.slice(i, nl < 0 ? t.length : nl); NET.note(line); if (nl < 0) break; i = nl + 1; } });
  s.on('data', d => { NET.down += d.length; });
  c.pipe(s); s.pipe(c);
  const die = () => { c.destroy(); s.destroy(); };
  c.on('error', die); s.on('error', die); c.on('close', die); s.on('close', die);
});
await new Promise(r => proxy.listen(PROXY_PORT, '127.0.0.1', r));
const RELAY = `http://127.0.0.1:${PROXY_PORT}`;

// ---- relay + bridges ----
run(bin('commons-relay'), ['--bind', `127.0.0.1:${RELAY_PORT}`, '--data', path.join(out, 'data/relay')]);
const secret = crypto.randomBytes(32).toString('hex');
const bridges = {};
for (const [n, port] of [['alice', A_PORT], ['bob', B_PORT]]) {
  bridges[n] = run(bin('dev_bridge'), [String(port), UI], { COMMONS_DATA: path.join(out, 'data', n), COMMONS_RELAY: RELAY, COMMONS_FAST_KDF: '1', MEGATRON_SECRET: secret });
}
const rpcOf = port => async (method, args = {}) => {
  const r = await fetch(`http://127.0.0.1:${port}/rpc`, { method: 'POST', headers: { 'X-Megatron-Secret': secret }, body: JSON.stringify({ method, args }) });
  const j = await r.json(); if ('err' in j) throw new Error(`${method}: ${j.err}`); return j.ok;
};
const A = rpcOf(A_PORT), B = rpcOf(B_PORT);
async function until(f, what, ms = 45000) { const t = Date.now(); let last; while (Date.now() - t < ms) { try { last = await f(); if (last) return last; } catch (e) { last = e.message; } await sleep(250); } throw new Error(`timed out: ${what} (${JSON.stringify(last)})`); }
await until(() => A('state').then(() => true), 'alice up'); await until(() => B('state').then(() => true), 'bob up');
await until(() => fetch(`${RELAY}/health`).then(r => r.ok), 'relay up');   // on a busy machine the relay can start after the bridges
await A('create_account', { passphrase: 'perf test pass', name: 'alice', relay: RELAY });
await B('create_account', { passphrase: 'perf test pass', name: 'bob', relay: RELAY });
// two communities so the page lands on the home cards; a text room and a voice room in each, bob in both
const comm = (s, cid) => s.communities.find(c => c.id === cid);
const rooms = [];
for (const name of ['Book club', 'Band']) {
  const cid = (await A('create_community', { name })).id;
  const gid = (await until(async () => comm(await A('snapshot'), cid)?.channels[0]?.gid, 'first room of ' + name));   // the room the community came with, which the card opens
  await A('create_channel', { cid, name: 'voice', kind: 'voice' }).catch(() => { });
  const inv = (await A('create_invite', { cid, uses: 1 })).link;
  await B('join', { link: inv });
  await until(async () => { const co = comm(await B('snapshot'), cid); return co && !co.removed && co.channels.length > 0; }, 'bob joined ' + name);
  await until(async () => { const co = comm(await B('snapshot'), cid), ch = co && co.channels.find(x => x.gid === gid); if (!ch) return false; if (!ch.joined) { await B('join_channel', { gid }).catch(() => { }); return false; } return true; }, 'bob in ' + name);
  rooms.push({ cid, gid, name });
}
// a history to render: MSGS messages in the first room, both ways
const main = rooms[0];
for (let i = 0; i < MSGS; i++) {
  const who = i % 3 ? A : B;
  await until(() => who('send_text', { gid: main.gid, text: `message ${i} ${'lorem ipsum dolor sit amet '.repeat(1 + (i % 4))}`, reply: null }).then(() => true), 'send ' + i, 30000);
}
await until(async () => (await A('messages', { gid: main.gid, limit: 500 })).messages.length >= MSGS, 'alice has the history');
await until(async () => (await B('messages', { gid: main.gid, limit: 500 })).messages.length >= MSGS, 'bob has the history');

// ---- browser ----
run(BROWSER, ['--headless=new', '--password-store=basic', '--use-mock-keychain', `--remote-debugging-port=${CDP_PORT}`, '--user-data-dir=' + path.join(out, 'chrome'), '--use-gl=angle', '--use-angle=vulkan',
  '--enable-features=Vulkan', '--ignore-gpu-blocklist', '--enable-gpu', '--ozone-platform=headless', '--disable-software-rasterizer', '--mute-audio', '--disable-audio-output',
  '--disable-background-timer-throttling', '--disable-renderer-backgrounding', '--disable-backgrounding-occluded-windows', '--no-first-run', '--window-size=1280,800', 'about:blank']);
let ver; for (let i = 0; i < 50; i++) { try { ver = await (await fetch(`http://127.0.0.1:${CDP_PORT}/json/version`)).json(); break; } catch { await sleep(200); } }
if (!ver) throw new Error('browser did not start');
class Cdp {
  constructor(ws) { this.id = 0; this.cb = new Map(); this.ws = ws; ws.onmessage = m => { const j = JSON.parse(m.data); if (j.id && this.cb.has(j.id)) { this.cb.get(j.id)(j); this.cb.delete(j.id); } }; }
  static async open(url) { const ws = new WebSocket(url); await new Promise(r => ws.onopen = r); return new Cdp(ws); }
  send(method, params = {}) { return new Promise(res => { const id = ++this.id; this.cb.set(id, res); this.ws.send(JSON.stringify({ id, method, params })); }); }
  async ev(expr) { const r = await this.send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true }); if (r.result.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails).slice(0, 400)); return r.result.result.value; }
  async waitFor(expr, ms = 20000) { const t = Date.now(); while (Date.now() - t < ms) { if (await this.ev(expr).catch(() => false)) return; await sleep(250); } throw new Error('timeout waiting for ' + expr); }
}
const browser = await Cdp.open(ver.webSocketDebuggerUrl);
// the one page lives in the first tab, so the browser has one renderer worth measuring
const targets = await (await fetch(`http://127.0.0.1:${CDP_PORT}/json/list`)).json();
const tab = targets.find(t => t.type === 'page');
const P = await Cdp.open(tab.webSocketDebuggerUrl);
await P.send('Page.enable'); await P.send('Runtime.enable'); await P.send('Performance.enable');
await P.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false });
await P.send('Page.navigate', { url: `http://127.0.0.1:${A_PORT}/#s=${secret}` });
await P.waitFor(`!document.querySelector('#app').hidden && window.__mega && __mega.started && !!S.snap`);
await P.waitFor(`document.querySelectorAll('.mcard').length >= 2`);
// count every call the page makes to its core: calls, bytes up (request body), bytes down (Content-Length)
await P.ev(`(() => { const f = window.fetch; window.__perfNet = { n: 0, up: 0, down: 0, by: {} }; window.fetch = async function (u, o) { const r = await f.apply(this, arguments); try { const N = window.__perfNet; let m = String(u).replace(/^.*\\/\\//, '').replace(/^[^/]*/, ''); if (o && typeof o.body === 'string') { try { m = JSON.parse(o.body).method || m; } catch { } N.up += o.body.length; } const cl = +r.headers.get('content-length') || 0; N.n++; N.down += cl; const b = N.by[m] || (N.by[m] = { n: 0, down: 0 }); b.n++; b.down += cl; if (m === 'messages') { const t = await r.clone().text(); const k = t.includes('"same":true') ? 'same' : t.includes('"delta":true') ? 'delta' : 'full'; b[k] = (b[k] || 0) + 1; if (k === 'full' && !/"if":/.test(o.body)) b.noIf = (b.noIf || 0) + 1; } } catch { } return r; }; return 1; })()`);
// when a message marked lat-N first appears anywhere on the page (its row in the stream, or a card's preview)
await P.ev(`(() => { window.__lat = {}; new MutationObserver(ms => { const now = Date.now(); for (const m of ms) for (const n of m.addedNodes) { const t = n.textContent || ''; let x; const re = /lat-(\\d+)\\b/g; while ((x = re.exec(t))) if (!(x[1] in __lat)) __lat[x[1]] = now; } }).observe(document.body, { childList: true, subtree: true, characterData: false }); return 1; })()`);

// ---- samplers ----
const procStat = pid => { try { const f = fs.readFileSync(`/proc/${pid}/stat`, 'utf8'); const a = f.slice(f.lastIndexOf(')') + 2).split(' '); return { cpu: (+a[11] + +a[12]) / 100, rss: +a[21] * 4096 }; } catch { return { cpu: 0, rss: 0 }; } };
async function cpuTimes() {
  const r = await browser.send('SystemInfo.getProcessInfo');
  const by = {}; for (const p of r.result.processInfo) { const k = p.type === 'renderer' ? 'renderer' : p.type === 'GPU' ? 'gpu' : p.type === 'browser' ? 'browser' : 'other'; by[k] = (by[k] || 0) + p.cpuTime; }
  return { ...by, bridgeA: procStat(bridges.alice.pid).cpu, bridgeB: procStat(bridges.bob.pid).cpu, load: fs.readFileSync('/proc/loadavg', 'utf8').split(' ')[0] };
}
async function heap() { await P.send('HeapProfiler.collectGarbage'); const m = (await P.send('Performance.getMetrics')).result.metrics; const g = n => m.find(x => x.name === n)?.value || 0; return { js: g('JSHeapUsedSize'), nodes: g('Nodes'), listeners: g('JSEventListeners') }; }
const results = {};
const MB = b => (b / 1048576).toFixed(1);
async function measure(name, secs, during) {
  await sleep(1500);   // let the switch settle before the count starts
  await P.ev(`(() => { const N = window.__perfNet; N.n = 0; N.up = 0; N.down = 0; N.by = {}; return 1; })()`);
  NET.reset();
  const h0 = await heap(), c0 = await cpuTimes(), t0 = Date.now();
  let sent = null;
  if (during) sent = await during(secs); else await sleep(secs * 1000);
  const c1 = await cpuTimes(), dt = (Date.now() - t0) / 1000, h1 = await heap();
  const page = await P.ev(`JSON.stringify(window.__perfNet)`);
  let latency = null;
  if (sent && sent.length) {
    await sleep(1500);   // the last message's row
    const seen = JSON.parse(await P.ev(`JSON.stringify(window.__lat)`));
    const ms = sent.map(s => seen[s.i] != null ? seen[s.i] - s.t0 : null).filter(v => v != null).sort((a, b) => a - b);
    const q = p => ms.length ? ms[Math.min(ms.length - 1, Math.floor(p * ms.length))] : null;
    latency = { sent: sent.length, shown: ms.length, medianMs: q(.5), p90Ms: q(.9), maxMs: ms.length ? ms[ms.length - 1] : null };
  }
  const pct = k => +(100 * ((c1[k] || 0) - (c0[k] || 0)) / dt).toFixed(1);
  const r = { secs: +dt.toFixed(1), load: `${c0.load}->${c1.load}`, cpu: { renderer: pct('renderer'), gpu: pct('gpu'), browser: pct('browser'), bridgeA: pct('bridgeA'), bridgeB: pct('bridgeB') },
    heapMB: { start: +MB(h0.js), end: +MB(h1.js), nodes: h1.nodes, listeners: h1.listeners }, bridgeRssMB: { alice: +MB(procStat(bridges.alice.pid).rss), bob: +MB(procStat(bridges.bob.pid).rss) },
    pageToCore: (() => { const N = JSON.parse(page); const per = 60 / dt; return { reqPerMin: Math.round(N.n * per), upKBPerMin: +(N.up * per / 1024).toFixed(1), downKBPerMin: +(N.down * per / 1024).toFixed(1), by: Object.fromEntries(Object.entries(N.by).map(([k, v]) => [k, { perMin: Math.round(v.n * per), downKB: +(v.down * per / 1024).toFixed(1), ...(k === 'messages' ? { same: v.same || 0, delta: v.delta || 0, full: v.full || 0, fullNoIf: v.noIf || 0 } : {}) }])) }; })(),
    coreToRelay2devices: (() => { const per = 60 / dt; return { reqPerMin: Math.round(Object.values(NET.by).reduce((a, b) => a + b, 0) * per), connsPerMin: Math.round(NET.conns * per), upKBPerMin: +(NET.up * per / 1024).toFixed(1), downKBPerMin: +(NET.down * per / 1024).toFixed(1), by: Object.fromEntries(Object.entries(NET.by).map(([k, v]) => [k, Math.round(v * per)])) }; })() };
  if (latency) r.latency = latency;
  results[name] = r;
  console.log(`\n== ${name} (${r.secs}s, load ${r.load})`);
  console.log(`  cpu%: renderer ${r.cpu.renderer}  gpu ${r.cpu.gpu}  browser ${r.cpu.browser}  | bridge alice ${r.cpu.bridgeA}  bob ${r.cpu.bridgeB}`);
  console.log(`  heap MB: ${r.heapMB.start} -> ${r.heapMB.end}  (nodes ${r.heapMB.nodes}, listeners ${r.heapMB.listeners});  bridge RSS MB alice ${r.bridgeRssMB.alice} bob ${r.bridgeRssMB.bob}`);
  console.log(`  page->core: ${r.pageToCore.reqPerMin} req/min, up ${r.pageToCore.upKBPerMin} KB/min, down ${r.pageToCore.downKBPerMin} KB/min  ${JSON.stringify(r.pageToCore.by)}`);
  console.log(`  core->relay (2 devices): ${r.coreToRelay2devices.reqPerMin} req/min on ${r.coreToRelay2devices.connsPerMin} conns/min, up ${r.coreToRelay2devices.upKBPerMin} KB/min, down ${r.coreToRelay2devices.downKBPerMin} KB/min  ${JSON.stringify(r.coreToRelay2devices.by)}`);
  if (latency) console.log(`  send -> on screen: ${latency.shown}/${latency.sent} shown, median ${latency.medianMs} ms, p90 ${latency.p90Ms} ms, max ${latency.maxMs} ms`);
  return r;
}
// Bob talks: types, then sends, every three seconds. Each message carries a marker the page's observer looks for.
let talkN = 0;
const talk = async secs => {
  const end = Date.now() + secs * 1000, sent = [];
  while (Date.now() < end) {
    await B('typing', { gid: main.gid }).catch(() => { });
    await sleep(1200);
    const i = talkN++, t0 = Date.now();
    await B('send_text', { gid: main.gid, text: `live ${i} lat-${i} ${'word '.repeat(8)}`, reply: null }).then(() => sent.push({ i, t0 })).catch(() => { });
    await sleep(1800);
  }
  return sent;
};

try {
  await P.ev(`JSON.stringify({ level: __mega.level, cards: document.querySelectorAll('.mcard').length })`).then(s => console.log('page at', s));
  await measure('home', SECS);
  await P.ev(`__mega.api.toChat(${JSON.stringify(main.cid)}, ${JSON.stringify(main.gid)}); 1`);
  await P.waitFor(`__mega.level === 'chat' && S.gid === ${JSON.stringify(main.gid)} && document.querySelectorAll('#stream .msg').length > 10`).catch(async e => { console.log('state', await P.ev(`JSON.stringify({ level: __mega.level, cid: __mega.cid, gid: S.gid, msgs: document.querySelectorAll('#stream .msg').length, snap: S.snap && S.snap.communities.map(c => [c.id, c.channels.map(x => [x.gid, x.kind, x.joined])]) })`)); throw e; });
  await measure('chat', SECS);
  await measure('chat-talking', SECS, talk);
  await P.ev(`Object.defineProperty(document, 'hidden', { configurable: true, get: () => true }); Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' }); document.dispatchEvent(new Event('visibilitychange')); 1`);
  await measure('chat-hidden', SECS);
  await P.ev(`delete document.hidden; delete document.visibilityState; document.dispatchEvent(new Event('visibilitychange')); 1`);
  if (SOAK_MIN > 0) await measure(`soak-${SOAK_MIN}min`, SOAK_MIN * 60, talk);
  fs.writeFileSync(path.join(out, 'perf.json'), JSON.stringify(results, null, 1));
  console.log('\nwrote', path.join(out, 'perf.json'));
} finally { stopAll(); proxy.close(); }
process.exit(0);
