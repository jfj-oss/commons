// Shared test environment for the call tests: a local LiveKit server, a relay with call tokens, several Megatron clients
// (dev bridges) and one headless Chromium per client with FAKE media devices only. Everything is on 127.0.0.1.
// Chromium runs on the GPU through ANGLE/Vulkan (never SwiftShader). No real microphone, camera, screen or sound is used:
// the fake devices come from Chromium itself, audio output is muted, and "screen" frames are synthetic (see fakeDisplay).
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';

export const root = path.resolve(new URL('../..', import.meta.url).pathname);
export const sleep = ms => new Promise(r => setTimeout(r, ms));
export const procs = [];
export function run(cmd, args, env = {}, log) {
  const out = log ? fs.openSync(log, 'a') : 'inherit';
  const p = spawn(cmd, args, { env: { ...process.env, ...env }, stdio: ['ignore', out, out], detached: true });
  procs.push(p); return p;
}
export function stopAll() { for (const p of procs) { try { process.kill(-p.pid, "SIGKILL"); } catch { try { p.kill("SIGKILL"); } catch { } } } }
process.on('exit', stopAll);
for (const s of ['SIGINT', 'SIGTERM']) process.on(s, () => { stopAll(); process.exit(130); });

// Chrome for Testing from Playwright's cache: the distro's Chromium 151 gathered no ICE candidates in headless mode here
// (WebRTC could not connect at all), while Chrome for Testing 151 does. Override with COMMONS_TEST_CHROME.
// Every fixed test port moves by this much, so a run can sit beside another one on the same machine. 0 (the default) leaves them all as written.
export const PORT_OFFSET = Number(process.env.COMMONS_TEST_PORT_OFFSET || 0);
export const port = n => n + PORT_OFFSET;
export const CHROME = process.env.COMMONS_TEST_CHROME || (process.env.HOME + '/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome');
export const LK_PORT = port(27880), LK_KEY = 'commonstest', LK_SECRET = 'commons-local-test-secret-0123456789abcdef';
export const RELAY_PORT = port(28787);

const b64u = b => Buffer.from(b).toString('base64url');
export function jwt(claims, secret = LK_SECRET) {
  const h = b64u(JSON.stringify({ alg: 'HS256', typ: 'JWT' })), p = b64u(JSON.stringify(claims));
  return `${h}.${p}.${crypto.createHmac('sha256', secret).update(`${h}.${p}`).digest('base64url')}`;
}
// What an operator of the media server can do: mint any token, list participants.
export function operatorToken(grant, identity = 'operator') { const now = Math.floor(Date.now() / 1000); return jwt({ iss: LK_KEY, sub: identity, nbf: now - 10, exp: now + 600, video: grant }); }
export async function lkAdmin(method, body) {
  const r = await fetch(`http://127.0.0.1:${LK_PORT}/twirp/livekit.RoomService/${method}`, { method: 'POST', headers: { 'content-type': 'application/json', authorization: 'Bearer ' + operatorToken({ roomAdmin: true, roomList: true, room: body.room || undefined }) }, body: JSON.stringify(body) });
  return r.json();
}

export function wav(file, hz, secs = 2) {
  const sr = 48000, n = sr * secs, buf = Buffer.alloc(44 + n * 2);
  buf.write('RIFF', 0); buf.writeUInt32LE(36 + n * 2, 4); buf.write('WAVEfmt ', 8); buf.writeUInt32LE(16, 16); buf.writeUInt16LE(1, 20); buf.writeUInt16LE(1, 22);
  buf.writeUInt32LE(sr, 24); buf.writeUInt32LE(sr * 2, 28); buf.writeUInt16LE(2, 32); buf.writeUInt16LE(16, 34); buf.write('data', 36); buf.writeUInt32LE(n * 2, 40);
  for (let i = 0; i < n; i++) buf.writeInt16LE(Math.round(Math.sin(2 * Math.PI * hz * i / sr) * 12000), 44 + i * 2);
  fs.writeFileSync(file, buf);
}

export async function rpc(port, method, args = {}) {
  const r = await fetch(`http://127.0.0.1:${port}/rpc`, { method: 'POST', body: JSON.stringify({ method, args }) });
  const j = await r.json(); if (j.err !== undefined) { const e = new Error(j.err); e.rpc = true; throw e; } return j.ok;
}

export class Page {
  constructor(ws, name) { this.id = 0; this.cb = new Map(); this.ws = ws; this.name = name; this.console = []; ws.onmessage = m => { const j = JSON.parse(m.data); if (j.id && this.cb.has(j.id)) { this.cb.get(j.id)(j); this.cb.delete(j.id); } else if (j.method === 'Runtime.consoleAPICalled') this.console.push(j.params.type + ': ' + j.params.args.map(a => a.value ?? a.description).join(' ')); else if (j.method && j.method.startsWith('Network.') && !j.method.includes('ExtraInfo') && !j.method.includes('dataReceived')) { this.net = this.net || []; this.net.push(Date.now() % 100000 + ' ' + j.method.replace('Network.', '') + ' ' + (j.params.request?.url || j.params.errorText || j.params.requestId) + ' ' + (j.params.encodedDataLength || '')); } else if (j.method === 'Log.entryAdded') this.console.push('LOG ' + j.params.entry.level + ': ' + j.params.entry.text + ' ' + (j.params.entry.url || '')); else if (j.method === 'Runtime.exceptionThrown') this.console.push('EXC: ' + JSON.stringify(j.params.exceptionDetails).slice(0, 300)); }; }
  send(method, params = {}) { return new Promise(res => { const id = ++this.id; this.cb.set(id, res); this.ws.send(JSON.stringify({ id, method, params })); }); }
  async ev(expr) { const r = await this.send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true }); if (r.result.exceptionDetails) throw new Error(this.name + ': ' + JSON.stringify(r.result.exceptionDetails).slice(0, 500)); return r.result.result.value; }
  async evGesture(expr) { const r = await this.send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true, userGesture: true }); if (r.result.exceptionDetails) throw new Error(this.name + ': ' + JSON.stringify(r.result.exceptionDetails).slice(0, 500)); return r.result.result.value; }
  async waitFor(expr, ms = 20000, what) { const t = Date.now(); while (Date.now() - t < ms) { if (await this.ev(expr).catch(() => false)) return; await sleep(200); } throw new Error(`${this.name}: timeout waiting for ${what || expr}`); }
}

// A synthetic "screen": an animated canvas with a frame counter and four fixed color blocks. Used instead of a real screen.
export const FAKE_DISPLAY = `(() => {
  if (window.__fakeDisplayInstalled) return; window.__fakeDisplayInstalled = true;
  navigator.mediaDevices.getDisplayMedia = async (opts) => {
    const c = document.createElement('canvas'); c.width = 1280; c.height = 720; const g = c.getContext('2d'); let n = 0;
    const draw = () => { g.fillStyle = '#202830'; g.fillRect(0, 0, 1280, 720); g.fillStyle = '#e02020'; g.fillRect(100, 100, 300, 200); g.fillStyle = '#20c040'; g.fillRect(500, 100, 300, 200); g.fillStyle = '#2040e0'; g.fillRect(900, 100, 300, 200);
      g.fillStyle = '#fff'; g.font = '48px sans-serif'; g.fillText('commons test screen ' + (window.__shareName || '') + ' ' + n, 100, 500); g.fillRect(100 + (n * 7 % 1000), 600, 60, 60); n++; };
    draw(); const timer = setInterval(draw, 66);
    const ms = c.captureStream(15); ms.getVideoTracks()[0].addEventListener('ended', () => clearInterval(timer));
    if (opts && opts.audio) { const ctx = (() => { try { return new AudioContext({ sinkId: { type: 'none' } }); } catch { return new AudioContext(); } })(); const o = ctx.createOscillator(); o.frequency.value = 1000; const d = ctx.createMediaStreamDestination(); o.connect(d); o.start(); d.stream.getAudioTracks().forEach(t => ms.addTrack(t)); }
    return ms;
  };
})()`;

// scripts/livekit-local.yaml as it stands, or a copy with its ports moved when an offset is set.
export function livekitConfig(out, transform = x => x) {
  const src = path.join(root, 'scripts/livekit-local.yaml');
  const text = fs.readFileSync(src, 'utf8');
  const moved = PORT_OFFSET ? text.replace(/^(\s*(?:port|tcp_port|udp_port):\s*)(\d+)/gm, (m, k, n) => k + port(Number(n))) : text;
  const final = transform(moved);
  if (final === text) return src;
  const dst = path.join(out, 'livekit-local.yaml'); fs.writeFileSync(dst, final); return dst;
}

export async function startServers(out, { relayTtl = 900, livekit = true } = {}) {
  fs.mkdirSync(out, { recursive: true });
  const secretFile = path.join(out, 'lk-secret'); fs.writeFileSync(secretFile, LK_SECRET);
  const ctl = {};
  ctl.startLivekit = () => { ctl.lk = run(path.join(root, 'tools/bin/livekit-server'), ['--config', livekitConfig(out)], {}, path.join(out, 'livekit.log')); return ctl.lk; };
  ctl.stopLivekit = async () => { ctl.lk.kill('SIGKILL'); await sleep(500); };
  if (livekit) ctl.startLivekit();
  const relayArgs = ['--bind', `127.0.0.1:${RELAY_PORT}`, '--data', path.join(out, 'data/relay')];
  if (livekit) relayArgs.push('--livekit-url', `ws://127.0.0.1:${LK_PORT}`, '--livekit-key', LK_KEY, '--livekit-secret-file', secretFile);
  run(path.join(root, 'target/debug/commons-relay'), relayArgs, { COMMONS_LIVEKIT_TOKEN_TTL: String(relayTtl) }, path.join(out, 'relay.log'));
  await sleep(1200);
  return ctl;
}

export function startBridge(out, name, port, ui = path.join(root, 'app/ui')) {
  return run(path.join(root, 'target/debug/dev_bridge'), [String(port), ui], { COMMONS_DATA: path.join(out, 'data', name), COMMONS_RELAY: `http://127.0.0.1:${RELAY_PORT}`, COMMONS_FAST_KDF: '1' }, path.join(out, name + '.log'));
}

export async function startChrome(out, name, dbgPort, toneHz, extra = []) {
  const toneFile = path.join(out, `tone-${toneHz}.wav`); if (!fs.existsSync(toneFile)) wav(toneFile, toneHz);
  const p = run(CHROME, ['--headless=new', '--password-store=basic', '--use-mock-keychain', `--remote-debugging-port=${dbgPort}`, '--user-data-dir=' + path.join(out, 'chrome-' + name),
    '--use-gl=angle', '--use-angle=vulkan', '--enable-features=Vulkan', '--ignore-gpu-blocklist', '--enable-gpu', '--ozone-platform=headless', '--disable-software-rasterizer',
    '--use-fake-ui-for-media-stream', '--use-fake-device-for-media-stream', `--use-file-for-fake-audio-capture=${toneFile}`, '--autoplay-policy=no-user-gesture-required', '--mute-audio', '--disable-audio-output', '--allow-loopback-in-peer-connection', '--disable-features=WebRtcHideLocalIpsWithMdns',
    '--no-first-run', '--no-default-browser-check', '--window-size=1280,800', '--disable-background-timer-throttling', '--disable-renderer-backgrounding', '--disable-backgrounding-occluded-windows', ...extra, 'about:blank'],
    {}, path.join(out, 'chrome-' + name + '.log'));
  let ver; for (let i = 0; i < 60; i++) { try { ver = await (await fetch(`http://127.0.0.1:${dbgPort}/json/version`)).json(); break; } catch { await sleep(250); } }
  if (!ver) throw new Error('chromium did not start: ' + name);
  return { proc: p, dbgPort };
}

export async function openPage(chrome, url, name, { before } = {}) {
  const t = await (await fetch(`http://127.0.0.1:${chrome.dbgPort}/json/new?` + encodeURIComponent('about:blank'), { method: 'PUT' })).json();
  const ws = new WebSocket(t.webSocketDebuggerUrl); await new Promise(r => ws.onopen = r);
  const p = new Page(ws, name); await p.send('Page.enable'); await p.send('Runtime.enable'); await p.send('Log.enable'); await p.send('Network.enable');
  if (before) await p.send('Page.addScriptToEvaluateOnNewDocument', { source: before });
  await p.send('Page.navigate', { url }); await sleep(500);
  return p;
}
