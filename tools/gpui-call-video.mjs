// Step 4 and step 6, video: the native engine's pictures and camera (CALLS-PLAN.md sections 5 and 6), against the web client.
// One local LiveKit room: Alice is the real app/ui call.js in headless Chrome for Testing (Chromium's own fake camera, and the
// synthetic 1280x720 "screen" of tools/lib/env.mjs); Nat is `mega-gpui --call-headless` (no audio device: MEGA_GPUI_CALL_AUDIO=none),
// whose pictures go to a `video::Frames` that the test reads through the test port (frames per tile: count, size, rate, pixels).
// No real camera is ever opened: the native camera is the engine's synthetic source (same fit, publish and preview path as a
// device's pictures), and the V4L2 path is only asked for a device that does not exist. Nothing is audible, no window opens.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin mega-gpui --bin commons-relay --bin dev_bridge
//   COMMONS_TEST_PORT_OFFSET=1100 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-call-video.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn } from 'node:child_process';
import { root, startServers, startBridge, rpc, sleep, stopAll, procs, FAKE_DISPLAY, RELAY_PORT, port as P } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-call-video');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const PASS = 'correct horse battery';
const J = x => JSON.stringify(x);
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? '  [' + (typeof detail === 'string' ? detail : J(detail)) + ']' : ''}`); };
async function until(fn, ms, what) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(250); } }
const soft = p => p.catch(e => ({ timeout: String(e.message || e) }));

// ------------------------------------------------------------------ the native engine's test port
class Native {
  constructor(env) {
    // a private environment: no session bus, no desktop, nothing of the user's runtime folder
    const e = { ...process.env, ...env }; delete e.DBUS_SESSION_BUS_ADDRESS; delete e.DISPLAY; delete e.WAYLAND_DISPLAY;
    this.proc = spawn(BIN, ['--call-headless'], { env: e, stdio: ['ignore', 'pipe', 'pipe'], detached: true });
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
    return new Promise((res, rej) => { const t = setTimeout(() => { this.waiting.delete(id); rej(new Error('no answer to ' + cmd + ' (process exit ' + this.proc.exitCode + ' signal ' + this.proc.signalCode + ')')); }, ms); this.waiting.set(id, j => { clearTimeout(t); res(j); }); this.sock.write(J({ cmd, id, ...args }) + '\n'); });
  }
  snap() { return this.cmd('snapshot'); }
  frames(ms = 0) { return this.cmd('frames', { ms }, 30000 + ms); }
  px(sid, fx, fy) { return this.cmd('frame_px', { sid, fx, fy }).then(r => r.bgra); }
}
const cpuSeconds = pid => { const s = fs.readFileSync(`/proc/${pid}/stat`, 'utf8'); const f = s.slice(s.lastIndexOf(')') + 2).split(' '); return (Number(f[11]) + Number(f[12])) / 100; };

const res = { checks: results };
let lk, world, nat;
try {
  lk = await startServers(out, {});
  world = await makeWorld(out, ['alice']);
  await openApps(world, ['alice'], { before: FAKE_DISPLAY });
  const A = world.people.alice;
  const call = (p, expr) => p.page.ev(`(async()=>{ const c = window.__commonsCall; return ${expr}; })()`);
  const gid = world.gid;
  await call(A, `c.join(S.gid, 'general')`);
  await until(() => call(A, `c.phase === 'connected'`), 20000, 'alice connected');

  const N = { name: 'nat', port: P(27203) };
  const natBridge = startBridge(out, 'nat', N.port); await sleep(1200);
  await rpc(N.port, 'create_account', { name: 'Nat', passphrase: PASS, relay: `http://127.0.0.1:${RELAY_PORT}` });
  await rpc(N.port, 'join', { link: (await rpc(A.port, 'create_invite', { cid: world.cid, uses: 4, ttl: 3600 })).link });
  await until(async () => { const s = await rpc(N.port, 'snapshot'); const c = s.communities.find(c => c.id === world.cid); return c && c.channels.some(ch => ch.gid === gid && ch.joined && ch.status === 'ok'); }, 60000, 'nat joined');
  const ns = await rpc(N.port, 'snapshot'); N.device = ns.me.device;
  try { process.kill(-natBridge.pid, 'SIGTERM'); } catch { } await sleep(1500);

  const testPort = P(27300);
  nat = new Native({ COMMONS_DATA: path.join(out, 'data/nat'), COMMONS_RELAY: `http://127.0.0.1:${RELAY_PORT}`, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_TEST: String(testPort), MEGA_GPUI_CALL_AUDIO: 'none',
    MEGA_GPUI_NOTIFY_FILE: path.join(out, 'nat-notes.jsonl'), RUST_LOG: process.env.RUST_LOG || 'info' });
  const ready = await nat.ready;
  await nat.connect(testPort);
  check('0 the headless engine unlocked Nat\'s account', ready.ready && ready.device === N.device);

  // what Alice sees of Nat (web side)
  const natPub = source => call(A, `(() => { const rp = c.room.remoteParticipants.get(${J(N.device)}); if (!rp) return { present: false }; const p = [...rp.trackPublications.values()].find(x => x.source === ${J(source)}); return p ? { present: true, muted: p.isMuted, name: p.trackName, enc: p.isEncrypted, sid: p.trackSid, sub: p.isSubscribed } : { present: false }; })()`);
  const natTrack = async (source, ms = 10000, wantMuted = false) => { const t = Date.now(); let r; for (;;) { r = await natPub(source); if (r.present && r.muted === wantMuted && (wantMuted || r.sub)) return r; if (Date.now() - t > ms) return { ...r, timeout: true }; await sleep(250); } };
  // the picture at one quality on Alice's side: size once it settles, then frames per second over 4 s; and two sampled pixels
  const view = (source, q, wantH) => call(A, `(async () => { const sleep = ms => new Promise(r => setTimeout(r, ms));
    let t; for (let i = 0; i < 60 && !t; i++) { t = [...c.tracks.values()].find(x => x.identity === ${J(N.device)} && x.source === ${J(source)} && !x.pub.isMuted); if (!t) await sleep(250); } if (!t) return { present: false };
    c.setViewQuality(t.pub.trackSid, ${J(q)}); const v = t.track.attach(); v.muted = true; await v.play().catch(() => {});
    const stats = async () => { const r = await t.track.receiver.getStats(); let o = null; r.forEach(s => { if (s.type === 'inbound-rtp' && s.kind === 'video') o = s; }); return o; };
    let s = null; const t0 = Date.now(); while (Date.now() - t0 < 25000) { await sleep(500); s = await stats(); if (s && s.frameHeight === ${wantH}) break; }
    await sleep(1500); const a = await stats(); await sleep(4000); const b = await stats();
    const cv = document.createElement('canvas'); cv.width = v.videoWidth; cv.height = v.videoHeight; const g = cv.getContext('2d'); g.drawImage(v, 0, 0);
    const px = (fx, fy) => Array.from(g.getImageData(Math.floor(fx * cv.width), Math.floor(fy * cv.height), 1, 1).data);
    const r = { present: true, w: b && b.frameWidth, h: b && b.frameHeight, fps: a && b ? (b.framesDecoded - a.framesDecoded) / 4 : -1, enc: t.pub.isEncrypted, sid: t.pub.trackSid, top: px(0.08, 0.14), bottom: px(0.08, 0.9) };
    t.track.detach(v); return r; })()`);
  const viewBest = async (source, q, wantH, min) => { let best = null, n = 0; for (; n < 3; n++) { const r = await view(source, q, wantH); if (!best || r.fps > best.fps) best = r; if (best.fps >= min && best.h === wantH) break; await sleep(2000); } return { ...best, attempts: n + 1 }; };
  const reddish = p => p && p[0] > 170 && p[2] < 140, bluish = p => p && p[2] > 170 && p[0] < 110;   // [r, g, b, a] from a canvas
  const bgraReddish = p => p && p[2] > 170 && p[0] < 140, bgraBluish = p => p && p[0] > 170 && p[2] < 110;   // [b, g, r, a] from the frame sink

  await nat.cmd('join', { gid, name: 'general' });
  const s1 = await soft(until(async () => { const s = await nat.snap(); return s.phase === 'connected' && s.peers.filter(p => !p.me && p.member).length === 1 ? s : null; }, 20000, 'nat connected'));
  check('1 Nat joins the call and Alice sees him', s1.phase === 'connected' && s1.encryption.active, { phase: s1.phase });

  // ============================================================ A. the native camera, to the web client
  await nat.cmd('camera', { source: 'test', profile: '720p' });
  const live = await soft(until(async () => { const s = await nat.snap(); return s.camera.state === 'live' && s.camera.sid ? s : null; }, 15000, 'camera live'));
  const camSid = live.camera && live.camera.sid;
  check('2 the snapshot says the camera is live, encrypted, 1280x720, 720p, with your own picture listed', live.camera && live.camera.state === 'live' && live.camera.encrypted && live.camera.width === 1280 && live.camera.height === 720 && live.camera.profile === '720p' && live.camera.source === 'test' && live.localVideo.includes(`${N.device}/${camSid}:Camera`), { camera: live.camera, localVideo: live.localVideo });
  const tr = await natTrack('camera');
  check('3 Alice receives it as an encrypted camera track', tr.present && !tr.muted && tr.enc && tr.name === 'camera', tr);
  const hi = await viewBest('camera', 'high', 720, 20), mid = await view('camera', 'medium', 360), lo = await view('camera', 'low', 180);
  check('4 Alice gets 1280x720 on HIGH at about 30 fps, 640x360 on MEDIUM and 320x180 on LOW (the three layers of 0.3.11)', hi.w === 1280 && hi.h === 720 && hi.fps >= 20 && mid.w === 640 && mid.h === 360 && lo.w === 320 && lo.h === 180, { high: hi, medium: mid, low: lo });
  check('5 the picture on Alice\'s screen is Nat\'s: reddish above, bluish below', reddish(hi.top) && bluish(hi.bottom), { top: hi.top, bottom: hi.bottom });

  // your own picture as a tile: from the buffer being sent
  const lf = await nat.frames(3000);
  const mine = lf.tiles && lf.tiles[camSid];
  const mpx = [await nat.px(camSid, 0.08, 0.14), await nat.px(camSid, 0.08, 0.9)];
  check('6 the local preview tile gets frames: 1280x720 at about 30 per second, with Nat\'s own colors', mine && mine.w === 1280 && mine.h === 720 && mine.fps >= 20 && bgraReddish(mpx[0]) && bgraBluish(mpx[1]), { tile: mine, px: mpx });

  // camera off: muted, not unpublished
  await nat.cmd('camera_stop');
  const offS = await soft(until(async () => { const s = await nat.snap(); return s.camera.state === 'off' ? s : null; }, 8000, 'camera off'));
  const offW = await natTrack('camera', 8000, true);
  await sleep(500);
  const offF = await nat.frames(0);
  check('7 camera off: the track is muted, not unpublished (same sid), no own tile, the preview picture is gone', offS.camera && offS.camera.sid === camSid && offS.localVideo.length === 0 && offW.present && offW.muted && offW.sid === camSid && !(offF.tiles && offF.tiles[camSid] && (await nat.px(camSid, 0.5, 0.5))), { camera: offS.camera, web: offW, localVideo: offS.localVideo });
  await nat.cmd('camera', { source: 'test', profile: '720p' });
  const backS = await soft(until(async () => { const s = await nat.snap(); return s.camera.state === 'live' ? s : null; }, 15000, 'camera back'));
  const backW = await natTrack('camera', 10000, false);
  const backV = await viewBest('camera', 'high', 720, 20);
  check('8 camera on again: the same publication is unmuted, Alice gets pictures again, the tile comes back', backS.camera && backS.camera.sid === camSid && backW.present && !backW.muted && backW.sid === camSid && backV.h === 720 && backV.fps >= 20 && backS.localVideo.length === 1, { sid: backS.camera && backS.camera.sid, web: backW, view: { w: backV.w, h: backV.h, fps: backV.fps } });

  // switching: the same size keeps the track
  await nat.cmd('camera_switch', { source: 'test' });
  await sleep(2500);
  const sw = await nat.snap();
  check('9 switching to another camera of the same size keeps the same track (no republish)', sw.camera.state === 'live' && sw.camera.sid === camSid, { sid: sw.camera.sid, was: camSid });
  // 1080p: a new track with the new encodings
  await nat.cmd('camera_profile', { profile: '1080p' });
  const p1080 = await soft(until(async () => { const s = await nat.snap(); return s.camera.state === 'live' && s.camera.height === 1080 && s.camera.sid !== camSid ? s : null; }, 20000, '1080p live'));
  const h1 = await viewBest('camera', 'high', 1080, 15), h1m = await view('camera', 'medium', 720), h1l = await view('camera', 'low', 360);
  check('10 the 1080p profile republishes: HIGH 1920x1080, MEDIUM 1280x720, LOW 640x360', p1080.camera && h1.w === 1920 && h1.h === 1080 && h1m.w === 1280 && h1m.h === 720 && h1l.w === 640 && h1l.h === 360, { camera: p1080.camera, high: h1, medium: h1m, low: h1l });
  await nat.cmd('camera_profile', { profile: '720p' });
  await soft(until(async () => { const s = await nat.snap(); return s.camera.state === 'live' && s.camera.height === 720 ? s : null; }, 20000, '720p live again'));

  // a device that does not exist: said plainly, nothing breaks (the real V4L2 path is never pointed at a real camera here)
  await nat.cmd('camera_stop'); await sleep(800);
  const cams = await nat.cmd('cameras');
  await nat.cmd('camera', { source: 'device', dev: "/dev/video-mega-test-none", profile: '720p' }, 8000).catch(async e => { console.log('DEBUG', e.message, J(await nat.cmd('snapshot', {}, 5000).then(s => ({ cam: s.camera, ev: s.events, err: s.error })).catch(x => String(x)))); throw e; });
  const gone = await soft(until(async () => { const s = await nat.snap(); return s.camera.ended ? s : null; }, 10000, 'device error'));
  check('11 the camera list works and a device that is not there ends with a plain message', Array.isArray(cams.cameras) && gone.camera && gone.camera.ended && gone.camera.ended.reason === 'error' && /not there|camera/i.test(gone.camera.ended.msg) && gone.camera.state === 'off', { cameras: cams.cameras, ended: gone.camera && gone.camera.ended });

  // unplugging
  await nat.cmd('camera', { source: 'test', profile: '720p' });
  await soft(until(async () => { const s = await nat.snap(); return s.camera.state === 'live' ? s : null; }, 15000, 'camera live before unplug'));
  await nat.cmd('camera_end');
  const unp = await soft(until(async () => { const s = await nat.snap(); return s.camera.ended && s.camera.state === 'off' ? s : null; }, 8000, 'unplug event'));
  const unpW = await natTrack('camera', 8000, true);
  check('12 the camera being unplugged raises an event (reason system), the track is muted and the tile goes', unp.camera && unp.camera.ended.reason === 'system' && unp.localVideo.length === 0 && unpW.muted, { ended: unp.camera && unp.camera.ended, web: unpW });

  // ============================================================ B. the web client's pictures, to the native engine
  await call(A, `c.setCamera(true)`);
  await call(A, `c.startShare({ audio: false, profile: 'motion' })`);
  const webSids = await until(async () => call(A, `(() => { const lp = c.room.localParticipant; const g = s => { const p = [...lp.trackPublications.values()].find(x => x.source === s); return p && p.track ? { sid: p.trackSid, w: p.track.mediaStreamTrack.getSettings().width, h: p.track.mediaStreamTrack.getSettings().height, enc: p.isEncrypted } : null; }; const cam = g('camera'), scr = g('screen_share'); return cam && scr ? { cam, scr } : null; })()`), 15000, 'alice published camera and screen');
  const gotBoth = await soft(until(async () => { const s = await nat.snap(); return s.subscribed.some(k => k.endsWith(':Camera') && k.includes(webSids.cam.sid)) && s.subscribed.some(k => k.endsWith(':Screenshare') && k.includes(webSids.scr.sid)) ? s : null; }, 20000, 'native subscribed to both'));
  check('13 the native engine subscribes to Alice\'s camera and screen share, both encrypted', gotBoth.subscribed && webSids.cam.enc && webSids.scr.enc, { subscribed: gotBoth.subscribed });
  // the server starts a new subscription on a lower layer and raises it as its bandwidth estimate allows: wait for the full picture
  const settled = await soft(until(async () => { const f = (await nat.frames(0)).tiles || {}; const c = f[webSids.cam.sid], r = f[webSids.scr.sid]; return c && r && c.w === webSids.cam.w && r.w === webSids.scr.w ? { cam: c, scr: r } : null; }, 40000, 'full-size layers'));
  console.log('      layers settled:', J(settled));
  await sleep(1000);
  const cpu0 = cpuSeconds(nat.proc.pid), t0 = Date.now();
  const rf = await nat.frames(5000);
  const cpuPct = (cpuSeconds(nat.proc.pid) - cpu0) / ((Date.now() - t0) / 1000) * 100;
  const rc = rf.tiles && rf.tiles[webSids.cam.sid], rs = rf.tiles && rf.tiles[webSids.scr.sid];
  check('14 pictures of Alice\'s camera reach video::Frames: her camera\'s size, at about 30 per second', rc && rc.w === webSids.cam.w && rc.h === webSids.cam.h && rc.fps >= 20, { tile: rc, webSettings: webSids.cam });
  check('15 pictures of Alice\'s screen share reach video::Frames: 1280x720 at her capture rate (15 fps)', rs && rs.w === 1280 && rs.h === 720 && rs.fps >= 10, { tile: rs, webSettings: webSids.scr });
  // the red, green and blue blocks of the synthetic screen, at (200,200), (700,200), (1000,200) of 1280x720
  const spx = [await nat.px(webSids.scr.sid, 200 / 1280, 200 / 720), await nat.px(webSids.scr.sid, 700 / 1280, 200 / 720), await nat.px(webSids.scr.sid, 1000 / 1280, 200 / 720), await nat.px(webSids.scr.sid, 20 / 1280, 20 / 720)];
  check('16 the screen\'s pictures are Alice\'s: the red, green and blue blocks and the dark background are where they should be (B,G,R,A)', spx[0] && spx[0][2] > 170 && spx[0][0] < 90 && spx[1] && spx[1][1] > 140 && spx[1][2] < 90 && spx[2] && spx[2][0] > 170 && spx[2][2] < 90 && spx[3] && spx[3][0] < 90 && spx[3][2] < 90, spx);
  res.nativeCpuPercentOneCore = Math.round(cpuPct * 10) / 10;
  console.log(`      native process while decoding a 720p camera and a 720p screen share into Frames (debug build): ${res.nativeCpuPercentOneCore} % of one core`);

  // a hidden tile costs nothing
  await nat.cmd('frames_hide', { sid: webSids.scr.sid, on: true });
  await sleep(500);
  const h0 = (await nat.frames(0)).tiles[webSids.scr.sid], h1c = (await nat.frames(2500)).tiles[webSids.scr.sid], camAlive = (await nat.frames(0)).tiles[webSids.cam.sid];
  await nat.cmd('frames_hide', { sid: webSids.scr.sid, on: false });
  const h2 = await soft(until(async () => { const t = (await nat.frames(0)).tiles[webSids.scr.sid]; return t && t.count > h1c.count + 5 ? t : null; }, 8000, 'frames again after show'));
  check('17 a hidden tile is not converted or pushed (its counter stands still while the camera tile moves on), and showing it again brings frames back', h0 && h1c.hidden === true && h1c.count === h0.count && camAlive.count > rc.count && h2.count > h1c.count, { before: h0 && h0.count, whileHidden: h1c.count, after: h2.count });

  // Alice's camera off: muted, so no tile; on again: tile back
  await call(A, `c.setCamera(false)`);
  const muted = await soft(until(async () => { const s = await nat.snap(); return s.mutedVideo.some(k => k.includes(webSids.cam.sid)) ? s : null; }, 10000, 'muted video'));
  await call(A, `c.setCamera(true)`);
  const unmuted = await soft(until(async () => { const s = await nat.snap(); return !s.mutedVideo.some(k => k.includes(webSids.cam.sid)) && s.subscribed.some(k => k.endsWith(':Camera')) ? s : null; }, 15000, 'unmuted video'));
  const camAfter = await soft(until(async () => { const s = await nat.snap(); const k = s.subscribed.find(k => k.endsWith(':Camera') && k.startsWith(A.device)); const sid = k && k.split('/')[1].split(':')[0]; const f = sid && (await nat.frames(1500)).tiles[sid]; return f && f.fps >= 15 ? { sid, f } : null; }, 15000, 'camera frames after unmute'));
  check('18 Alice\'s camera off lists it as muted (no tile), on again clears it and pictures flow', muted.mutedVideo && !unmuted.timeout && camAfter.f, { muted: muted.mutedVideo, after: camAfter.f });

  await call(A, `c.stopShare()`);
  const scrGone = await soft(until(async () => { const s = await nat.snap(); const f = await nat.frames(0); return !s.subscribed.some(k => k.endsWith(':Screenshare')) && !(f.tiles && f.tiles[webSids.scr.sid]) ? s : null; }, 10000, 'screen tile gone'));
  check('19 Alice stopping her share removes the track and its tile from Frames', !scrGone.timeout, scrGone.timeout || '');
  await call(A, `c.setCamera(false)`);

  // ============================================================ C. your own screen share as a tile
  await nat.cmd('share', { profile: 'text', source: 'test', w: 2560, h: 1440 });
  const sl = await soft(until(async () => { const s = await nat.snap(); return s.share.state === 'live' && s.share.sid ? s : null; }, 15000, 'share live'));
  const shareSid = sl.share && sl.share.sid;
  const sf = (await nat.frames(3000)).tiles[shareSid];
  check('20 your own screen share has a preview tile: listed as :Screenshare, 1280x720 (a 2560x1440 picture scaled down for the tile) at its sending rate', sl.localVideo && sl.localVideo.includes(`${N.device}/${shareSid}:Screenshare`) && sf && sf.w === 1280 && sf.h === 720 && sf.fps >= 15, { tile: sf, localVideo: sl.localVideo });
  await nat.cmd('share_stop');
  const sg = await soft(until(async () => { const s = await nat.snap(); const f = await nat.frames(0); return s.localVideo.length === 0 && !(f.tiles && f.tiles[shareSid]) ? s : null; }, 8000, 'preview gone'));
  check('21 stopping the share removes its preview tile', !sg.timeout, sg.timeout || '');

  // ============================================================ D. the media server restarts with the camera on
  await nat.cmd('camera', { source: 'test', profile: '720p' });
  const b4 = await until(async () => { const s = await nat.snap(); return s.camera.state === 'live' ? s : null; }, 15000, 'camera live before restart');
  await lk.stopLivekit(); await sleep(2500); lk.startLivekit();
  const tBack = Date.now();
  const back = await soft(until(async () => { const s = await nat.snap(); return s.phase === 'connected' && s.camera.state === 'live' && s.camera.sid && s.camera.sid !== b4.camera.sid ? s : null; }, 60000, 'camera back after restart'));
  const backTr = back.camera ? await natTrack('camera', 20000) : {};
  const backF = back.camera && back.camera.sid ? (await nat.frames(2500)).tiles[back.camera.sid] : null;
  check('22 after the media server restarts, the camera is published again from the running capture and Alice receives it; the preview follows', back.camera && backTr.present && backTr.enc && backF && backF.fps >= 15, { seconds: (Date.now() - tBack) / 1000, camera: back.camera && { sid: back.camera.sid, state: back.camera.state }, web: backTr, preview: backF });

  // ============================================================ E. leaving
  await nat.cmd('leave');
  await sleep(2500);
  const left = await nat.snap();
  const lf2 = await nat.frames(0);
  check('23 leaving the call turns the camera off and clears every tile', left.phase === 'idle' && left.camera.state === 'off' && left.localVideo.length === 0 && Object.keys(lf2.tiles || {}).length === 0, { phase: left.phase, camera: left.camera.state, tiles: Object.keys(lf2.tiles || {}) });

  await nat.cmd('quit').catch(() => { });
  await Promise.race([new Promise(r => nat.proc.on('exit', r)), sleep(5000)]);
} catch (e) { res.error = String(e.stack || e); console.log('ERROR', res.error); failed++; }
res.failed = failed;
fs.writeFileSync(path.join(out, 'result.json'), JSON.stringify(res, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED', '->', path.join(out, 'result.json'));
stopAll(); process.exit(failed ? 1 : 0);
