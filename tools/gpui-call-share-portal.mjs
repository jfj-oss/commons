// Real screen capture through the desktop portal, for a person to try (CALLS-PLAN.md step 5). The automated test
// (gpui-call-share.mjs) cannot pick a screen, so this one waits for YOU. It starts a private local relay and LiveKit, a
// headless Chromium viewer (Alice, no window) and the native engine (Nat) in YOUR desktop session. Nat asks the portal;
// the system's picker opens; choose a screen or a window. Then Alice measures what arrives (size on HIGH and LOW, frames per
// second over 6 s) and saves a frame of the HIGH picture as <outdir>/high.png, to judge the sharpness of text against the
// browser share. Nothing is sent anywhere but 127.0.0.1. Press "stop sharing" in the system's tray to try the ended event.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin dev_bridge
//   COMMONS_TEST_PORT_OFFSET=600 node tools/gpui-call-share-portal.mjs <outdir> [text|motion|motion60]
// With SHARE_SOURCE=test the synthetic source is used instead of the portal (to check this script itself, silently).
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn } from 'node:child_process';
import { root, startServers, startBridge, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-call-share-portal');
const profile = process.argv[3] || 'text';
const source = process.env.SHARE_SOURCE === 'test' ? 'test' : 'portal';
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const PASS = 'correct horse battery', J = x => JSON.stringify(x);
async function until(fn, ms, what) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(250); } }

class Native {
  constructor(env) {
    this.proc = spawn(BIN, ['--call-headless'], { env: { ...process.env, ...env }, stdio: ['ignore', 'pipe', 'pipe'], detached: true });
    procs.push(this.proc);
    this.proc.stderr.pipe(fs.createWriteStream(path.join(out, 'nat.log')));
    this.ready = new Promise((res, rej) => { let b = ''; this.proc.stdout.on('data', d => { b += d; const i = b.indexOf('\n'); if (i >= 0) { try { res(JSON.parse(b.slice(0, i))); } catch (e) { rej(e); } } }); this.proc.on('exit', c => rej(new Error('mega-gpui exited ' + c))); });
    this.n = 0; this.waiting = new Map(); this.buf = '';
  }
  async connect(port) {
    this.sock = net.connect(port, '127.0.0.1');
    await new Promise((res, rej) => { this.sock.once('connect', res); this.sock.once('error', rej); });
    this.sock.on('data', d => { this.buf += d; let i; while ((i = this.buf.indexOf('\n')) >= 0) { const l = this.buf.slice(0, i); this.buf = this.buf.slice(i + 1); const j = JSON.parse(l); const w = this.waiting.get(j.id); if (w) { this.waiting.delete(j.id); w(j); } } });
  }
  cmd(cmd, args = {}, ms = 30000) {
    const id = ++this.n;
    return new Promise((res, rej) => { const t = setTimeout(() => { this.waiting.delete(id); rej(new Error('no answer to ' + cmd)); }, ms); this.waiting.set(id, j => { clearTimeout(t); res(j); }); this.sock.write(J({ cmd, id, ...args }) + '\n'); });
  }
  snap() { return this.cmd('snapshot'); }
}

try {
  await startServers(out, {});
  const world = await makeWorld(out, ['alice']);
  await openApps(world, ['alice']);
  const A = world.people.alice, gid = world.gid;
  const call = (p, expr) => p.page.ev(`(async()=>{ const c = window.__commonsCall; return ${expr}; })()`);
  await call(A, `c.join(S.gid, 'general')`);
  await until(() => call(A, `c.phase === 'connected'`), 20000, 'alice connected');
  const N = { name: 'nat', port: P(27203) };
  const natBridge = startBridge(out, 'nat', N.port); await sleep(1200);
  await rpc(N.port, 'create_account', { name: 'Nat', passphrase: PASS, relay: `http://127.0.0.1:${RELAY_PORT}` });
  await rpc(N.port, 'join', { link: (await rpc(A.port, 'create_invite', { cid: world.cid, uses: 4, ttl: 3600 })).link });
  await until(async () => { const s = await rpc(N.port, 'snapshot'); const c = s.communities.find(c => c.id === world.cid); return c && c.channels.some(ch => ch.gid === gid && ch.joined && ch.status === 'ok'); }, 60000, 'nat joined');
  const dev = (await rpc(N.port, 'snapshot')).me.device;
  try { process.kill(-natBridge.pid, 'SIGTERM'); } catch { } await sleep(1500);
  const testPort = P(27300);
  const nat = new Native({ COMMONS_DATA: path.join(out, 'data/nat'), COMMONS_RELAY: `http://127.0.0.1:${RELAY_PORT}`, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_TEST: String(testPort), MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_NOTIFY_FILE: path.join(out, 'nat-notes.jsonl') });
  await nat.ready; await nat.connect(testPort);
  await nat.cmd('join', { gid, name: 'general' });
  await until(async () => (await nat.snap()).phase === 'connected', 20000, 'nat connected');

  console.log(source === 'portal' ? '\n>>> The system picker opens now. Choose a screen or a window (you have 3 minutes).\n' : 'synthetic source');
  await nat.cmd('share', { profile, source });
  const live = await until(async () => { const s = await nat.snap(); if (s.share.ended) throw new Error('ended: ' + J(s.share.ended)); return s.share.state === 'live' ? s : null; }, 190000, 'share live');
  console.log('sharing:', J({ w: live.share.width, h: live.share.height, profile: live.share.profile, encrypted: live.share.encrypted }));

  const r = await call(A, `(async () => { const sleep = ms => new Promise(r => setTimeout(r, ms));
    let t; for (let i = 0; i < 60 && !t; i++) { t = [...c.tracks.values()].find(x => x.identity === ${J(dev)} && x.source === 'screen_share'); if (!t) await sleep(250); } if (!t) return { present: false };
    const stats = async () => { const r = await t.track.receiver.getStats(); let o = null; r.forEach(s => { if (s.type === 'inbound-rtp' && s.kind === 'video') o = s; }); return o; };
    const one = async (q, want) => { c.setViewQuality(t.pub.trackSid, q); const v = t.track.attach(); v.muted = true; await v.play().catch(() => {});
      for (let i = 0; i < 40; i++) { await sleep(500); const s = await stats(); if (s && s.frameHeight === want) break; }
      await sleep(1500); const a = await stats(); await sleep(6000); const b = await stats(); let png = null;
      try { const cv = document.createElement('canvas'); cv.width = v.videoWidth; cv.height = v.videoHeight; cv.getContext('2d').drawImage(v, 0, 0); png = cv.toDataURL('image/png'); } catch { }
      t.track.detach(v); return { w: b.frameWidth, h: b.frameHeight, fps: (b.framesDecoded - a.framesDecoded) / 6, png }; };
    const sz = ${J(live.share.height)};
    const hi = await one('high', sz); const lo = await one('low', 540); return { present: true, high: hi, low: lo }; })()`);
  if (r.high && r.high.png) { fs.writeFileSync(path.join(out, 'high.png'), Buffer.from(r.high.png.split(',')[1], 'base64')); delete r.high.png; }
  if (r.low) delete r.low.png;
  console.log('Alice (browser) receives:', J(r));
  console.log('Frame saved to', path.join(out, 'high.png'));
  if (source === 'portal') { console.log('\n>>> Now stop the share from the system\'s "stop sharing" control (you have 60 s) to try the ended event, or wait.'); }
  const ended = await until(async () => { const s = await nat.snap(); return s.share.ended ? s.share.ended : null; }, source === 'portal' ? 60000 : 3000, 'ended').catch(() => null);
  console.log('ended event:', J(ended));
  await nat.cmd('quit').catch(() => { });
} catch (e) { console.log('ERROR', String(e.stack || e)); }
stopAll(); process.exit(0);
