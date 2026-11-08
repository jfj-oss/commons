// "Play into the call" end-to-end test: real relay, real LiveKit, two real Megatron clients and the real call engine in
// headless Chromium (fake media devices only). A local file is played into the call, then the canvas fallback is forced,
// then a stand-in Jellyfin server (same API shapes and CORS as Jellyfin, real CORS and Range handling) is used.
//   systemd-run --user --scope -q -p MemoryMax=8G node tools/call-play-e2e.mjs [outdir]
// Nothing is audible: --mute-audio, --disable-audio-output; tones are measured with an AnalyserNode.
import fs from 'node:fs';
import path from 'node:path';
import http from 'node:http';
import crypto from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { startServers, openPage, rpc, sleep, stopAll, lkAdmin, LK_PORT, root } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';

const out = process.argv[2] || '/tmp/commons-call-play-e2e';
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? '  [' + detail + ']' : ''}`); };
const J = x => JSON.stringify(x);
async function until(fn, ms = 20000, what = 'condition') { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(250); } }

// ---------------------------------------------------------------- test media and a stand-in Jellyfin
const mp4 = path.join(out, 'test-1080p.mp4');
console.log('generating test video with ffmpeg…');
execFileSync('ffmpeg', ['-y', '-f', 'lavfi', '-i', 'testsrc2=size=1920x1080:rate=30', '-f', 'lavfi', '-i', 'sine=frequency=880:sample_rate=48000',
  '-t', '30', '-c:v', 'libx264', '-preset', 'ultrafast', '-crf', '38', '-maxrate', '2500k', '-bufsize', '5M', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-b:a', '128k', '-ac', '2', '-movflags', '+faststart', mp4], { stdio: 'ignore' });
const MP4 = fs.readFileSync(mp4);
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==', 'base64');
const CORS = {
  'Access-Control-Allow-Origin': '*', 'Access-Control-Allow-Methods': 'GET, HEAD, OPTIONS',
  'Access-Control-Allow-Headers': 'X-Emby-Token, Content-Type, Range', 'Access-Control-Expose-Headers': 'Content-Range, Accept-Ranges, Content-Length',
};
const jfPort = 28996, jfBase = `http://127.0.0.1:${jfPort}`;
let jfHits = { items: 0, image: 0, stream: 0, badKeys: 0, transcode: 0 };
const jf = http.createServer((req, res) => {
  const send = (code, type, body, extra = {}) => { res.writeHead(code, { 'Content-Type': type, ...CORS, ...extra }); res.end(req.method === 'HEAD' ? undefined : body); };
  if (req.method === 'OPTIONS') return send(204, 'text/plain', '');
  const u = new URL(req.url, jfBase);
  if (u.pathname === '/Items') {
    if (req.headers['x-emby-token'] !== 'test-jellyfin-key') { jfHits.badKeys++; return send(401, 'text/plain', 'no'); }
    jfHits.items++;
    return send(200, 'application/json', JSON.stringify({ Items: [
      { Id: 'movie1', Name: 'Colour Bars: The Movie', Type: 'Movie', ProductionYear: 2024, ImageTags: { Primary: 't1' }, RunTimeTicks: 80000000 },
      { Id: 'ep2', Name: 'An Episode', Type: 'Episode', SeriesName: 'A Show', ProductionYear: 2023, ImageTags: {}, RunTimeTicks: 70000000 },
    ] }));
  }
  if (/^\/Items\/[^/]+\/Images\/Primary$/.test(u.pathname)) { jfHits.image++; return send(200, 'image/png', PNG, { 'Cache-Control': 'public, max-age=60' }); }
  if (/^\/Videos\/[^/]+\/stream(\.mp4)?$/.test(u.pathname)) {
    if (u.pathname.endsWith('.mp4')) jfHits.transcode++; else jfHits.stream++;
    const range = req.headers.range;
    if (range) {
      const m = /^bytes=(\d*)-(\d*)$/.exec(range);
      let a = m && m[1] ? Number(m[1]) : 0, b = m && m[2] ? Number(m[2]) : MP4.length - 1;
      b = Math.min(b, MP4.length - 1);
      return send(206, 'video/mp4', MP4.subarray(a, b + 1), { 'Accept-Ranges': 'bytes', 'Content-Range': `bytes ${a}-${b}/${MP4.length}`, 'Content-Length': String(b - a + 1) });
    }
    return send(200, 'video/mp4', MP4, { 'Accept-Ranges': 'bytes', 'Content-Length': String(MP4.length) });
  }
  send(404, 'text/plain', 'not found');
});
await new Promise(r => jf.listen(jfPort, '127.0.0.1', r));

let world, lk;
try {
  lk = await startServers(out, { relayTtl: 900 });
  world = await makeWorld(out, ['alice', 'bob']);
  await openApps(world, ['alice', 'bob']);
  const P = n => world.people[n];
  const page = n => P(n).page;
  const call = (n, expr) => page(n).ev(`(async()=>{ const c = window.__commonsCall; return ${expr}; })()`);
  const dev = n => P(n).device;
  const room = await rpc(P('alice').port, 'call_token', { gid: world.gid });
  const roomName = room.room;

  const probe = async (listener, ms = 1800) => call(listener, `c.probeTone(${J(dev('alice'))}, 880, ${ms}, 'screen_share_audio')`);
  const heard = async (listener, tries = 8) => { for (let i = 0; i < tries; i++) { const r = await probe(listener); if (r.present && r.toneShare > 0.12) return r; await sleep(900); } return await probe(listener); };

  // one frame sample of the remote movie: resolution, a checksum of the picture, and whether the video is playing
  const mad = (a, b) => { let s = 0; for (let i = 0; i < a.length; i++) s += Math.abs(a[i] - b[i]); return s / a.length; };
  const frame = async (viewer, what = 'movie') => page(viewer).ev(`(async()=>{
    const c = window.__commonsCall; let t;
    for (let i = 0; i < 120 && !t; i++) { t = [...c.tracks.values()].find(x => x.identity === ${J(dev('alice'))} && x.source === 'screen_share' && (x.pub.trackName || '') === ${J(what)}); if (!t) await new Promise(r => setTimeout(r, 200)); }
    if (!t) return { present: false };
    try { t.pub.setVideoQuality && t.pub.setVideoQuality(c.LK.VideoQuality.HIGH); } catch { }   // the SFU may start a subscriber on a low layer
    // Keep one video element per track (like a real call tile), so two samples see the same layer.
    window.__frameEls = window.__frameEls || {};
    let v = window.__frameEls[${J(what)}];
    if (!v || !v.isConnected || v.__track !== t.track) {
      v = document.createElement('video'); v.muted = true; v.autoplay = true; t.track.attach(v); document.body.appendChild(v); v.style.cssText = 'position:fixed;left:-9999px;width:8px';
      v.__track = t.track; window.__frameEls[${J(what)}] = v;
    }
    const t0 = Date.now(); while (!(v.videoWidth > 0) && Date.now() - t0 < 15000) await new Promise(r => setTimeout(r, 100));
    while (v.videoWidth !== 1920 && Date.now() - t0 < 20000) await new Promise(r => setTimeout(r, 150));   // wait for the top layer
    await new Promise(r => setTimeout(r, 700));
    const cv = document.createElement('canvas'); const w = Math.min(v.videoWidth || 320, 320); cv.width = w; cv.height = Math.max(2, Math.round(w * (v.videoHeight || 180) / (v.videoWidth || 320)));
    const g = cv.getContext('2d'); g.drawImage(v, 0, 0, cv.width, cv.height);
    const d = g.getImageData(0, 0, cv.width, cv.height).data; let h = 0, nonblack = 0;
    for (let i = 0; i < d.length; i += 4) { if (i % 97 === 0) h = (h * 31 + d[i] + d[i + 1] * 3 + d[i + 2] * 7) >>> 0; if (d[i] + d[i + 1] + d[i + 2] > 30) nonblack++; }
    // a coarse signature, so a still picture can be compared with tolerance for codec noise
    const sg = document.createElement('canvas'); sg.width = 32; sg.height = 18; const sx = sg.getContext('2d'); sx.drawImage(cv, 0, 0, 32, 18);
    const sd = sx.getImageData(0, 0, 32, 18).data; const sig = []; for (let i = 0; i < sd.length; i += 4) sig.push(sd[i], sd[i + 1], sd[i + 2]);
    const r = { present: true, w: v.videoWidth, h: v.videoHeight, hash: h, nonblack, sig, playing: !v.paused && v.readyState >= 2 };
    return r; })()`);

  const fileToPage = async (name) => page(name).ev(`(async()=>{ const bin = atob(${J(MP4.toString('base64'))}); const u = new Uint8Array(bin.length); for (let i = 0; i < bin.length; i++) u[i] = bin.charCodeAt(i);
    window.__testFile = new File([u], 'colour-bars.mp4', { type: 'video/mp4' }); return window.__testFile.size; })()`);

  // ---------------------------------------------------------------- join
  await call('alice', `c.join(S.gid, 'general')`);
  await call('bob', `c.join(S.gid, 'general')`);
  await until(async () => (await call('bob', `c.room.remoteParticipants.size`)) === 1, 20000, 'bob sees alice');
  await page('alice').ev(`(window.__callUI.open(), true)`);
  await page('bob').ev(`(window.__callUI.open(), true)`);
  // Background tabs defer loading media elements in Chromium; the person picking a file has their tab in front.
  await page('alice').send('Page.bringToFront'); await sleep(500);
  const hasScreenCapture = await page('alice').ev(`typeof HTMLMediaElement.prototype.captureStream === 'function'`);
  console.log('HTMLMediaElement.captureStream present:', hasScreenCapture);

  // ---------------------------------------------------------------- A. a local file, native element capture
  await fileToPage('alice');
  check('A0 the test file is 1080p', (await page('alice').ev(`(async()=>{ const v=document.createElement('video'); v.src=URL.createObjectURL(window.__testFile); await new Promise(r=>v.onloadedmetadata=r); return [v.videoWidth,v.videoHeight]; })()`)).join('x') === '1920x1080');
  const startErr = await page('alice').ev(`(async()=>{ try { await window.__commonsCall.startFilePlayback(window.__testFile); return null; } catch(e) { return { msg: e.message, kind: e.kind, events: window.__commonsCall.events.slice(-10).map(x=>x.msg) }; } })()`);
  if (startErr) console.log('startFilePlayback failed:', J(startErr));
  console.log('after start:', J(await call('alice', `({ pb: c.play, err: c.error, ev: c.events.slice(-12).map(x => x.msg) })`)));
  const pb = await until(async () => await call('alice', `c.play`), 25000, 'playback state');
  check('A1 a local file starts playing into the call', pb && pb.title === 'colour-bars.mp4' && !pb.paused, J({ title: pb && pb.title, capture: pb && pb.capture, duration: pb && pb.duration }));
  check('A2 it uses native element capture when the browser has it', pb.capture === (hasScreenCapture ? 'element' : 'canvas'), pb.capture);

  const f1 = await frame('bob'), f2 = await frame('bob');
  check('A3 the viewer receives the movie as a video track and it decodes', f1.present && f1.w === 1920 && f1.nonblack > 100, J({ w: f1.w, h: f1.h, nonblack: f1.nonblack }));
  check('A4 the picture moves (real frames, not one frozen frame)', f2.present && mad(f1.sig, f2.sig) > 2, J({ h1: f1.hash, h2: f2.hash, mad: mad(f1.sig, f2.sig) }));
  const firstTone = await heard('bob');
  check('A5 the viewer hears the movie audio (880 Hz tone through the call)', firstTone.present && firstTone.toneShare > 0.12, J(firstTone));

  const parts = (await lkAdmin('ListParticipants', { room: roomName })).participants || [];
  const aliceTracks = ((parts.find(p => p.identity === dev('alice')) || {}).tracks) || [];
  const movieT = aliceTracks.find(t => t.name === 'movie'), movieA = aliceTracks.find(t => t.name === 'movie-audio');
  console.log('server sees alice tracks:', J(aliceTracks.map(t => ({ name: t.name, source: t.source, kind: t.kind, type: t.type, enc: t.encryption, w: t.width, h: t.height }))));
  check('A6 the media server sees both movie tracks marked encrypted (AES-GCM)', !!movieT && !!movieA && [movieT, movieA].every(t => t.encryption === 'GCM' || t.encryption === 1), J(aliceTracks.map(t => [t.name, t.encryption])));

  const encs = await call('alice', `(()=>{ const p=c.play; const tr=p && p.vpub && p.vpub.track; const s=tr && tr.sender; return s ? s.getParameters().encodings.map(e => ({ mb: e.maxBitrate, fr: e.maxFrameRate, active: e.active })) : null; })()`);
  console.log('sender encodings:', J(encs));
  check('A7 simulcast: the sender has more than one encoding layer', Array.isArray(encs) && encs.length >= 2, J(encs));

  // ---------------------------------------------------------------- B. pause, seek, volume, stop
  await call('alice', `c.pausePlayback()`);
  await sleep(1200);
  const pausedTone = await probe('bob');
  const p1 = await frame('bob'), p2 = await frame('bob');
  check('B1 pause is reported to the sender UI', (await call('alice', `c.play.paused`)) === true);
  check('B2 paused: the viewer hears silence (no stray audio samples)', pausedTone.present && pausedTone.toneShare < 0.02, J(pausedTone));
  check('B3 paused: the viewer keeps the last picture, stable, not garbage', p1.present && p2.present && p1.nonblack > 100 && mad(p1.sig, p2.sig) < 1.5, J({ h1: p1.hash, h2: p2.hash, mad: mad(p1.sig, p2.sig) }));
  await call('alice', `c.resumePlayback()`);
  await sleep(1200);
  const resumedTone = await heard('bob');
  const r2 = await frame('bob');
  check('B4 resume brings the sound and the moving picture back', resumedTone.toneShare > 0.12 && mad(r2.sig, p2.sig) > 2, J({ tone: resumedTone.toneShare, h: r2.hash, mad: mad(r2.sig, p2.sig) }));

  const beforeSeek = await frame('bob');
  await call('alice', `c.seekPlayback(6)`);
  await sleep(1500);
  const seekPos = await call('alice', `(c.play.video.currentTime)`);
  const afterSeek = await frame('bob');
  check('B5 seek moves the sender and the viewer sees the new moment', seekPos > 5.5 && afterSeek.present && afterSeek.hash !== beforeSeek.hash, J({ seekPos, before: beforeSeek.hash, after: afterSeek.hash }));

  await call('alice', `c.seekPlayback(1)`); await sleep(1200);   // the clip is short; keep sound flowing for the volume check
  const loud = await heard('bob');
  await call('alice', `c.setPlaybackVolume(0.2)`);
  await sleep(1500);
  const quiet = await probe('bob');
  await call('alice', `c.setPlaybackVolume(1)`);
  check('B6 the sender\'s volume control lowers what the viewer hears', loud.toneShare > 0.12 && quiet.rms < loud.rms * 0.55, J({ loud, quiet }));

  await call('alice', `c.stopPlayback()`);
  await until(async () => (await call('alice', `c.play`)) === null, 8000, 'playback stopped');
  await until(async () => (await page('bob').ev(`(window.__commonsCall.tracks.size)`)) === 0, 12000, 'viewer tracks removed').catch(() => { });
  const gone = await page('bob').ev(`[...window.__commonsCall.tracks.values()].filter(x => x.identity === ${J(dev('alice'))} && x.source === 'screen_share').length`);
  check('B7 stop unpublishes the movie from the viewer', gone === 0, `tracks left ${gone}`);

  // ---------------------------------------------------------------- C. forced canvas fallback (the WebKit path)
  await page('alice').send('Page.bringToFront'); await sleep(300);
  await page('alice').ev(`(window.__commonsCall.prefs.forceCanvasPlayback = true, true)`);
  await call('alice', `c.startFilePlayback(window.__testFile)`);
  const cb = await until(async () => await call('alice', `c.play`), 25000, 'canvas playback');
  const c1 = await frame('bob'), c2 = await frame('bob');
  const cTone = await heard('bob');
  check('C1 the forced canvas path is in use', cb.capture === 'canvas', cb.capture);
  check('C2 canvas-captured video reaches the viewer, moves, and keeps 1080p', c1.present && c1.w === 1920 && mad(c1.sig, c2.sig) > 2 && c1.nonblack > 100, J({ w: c1.w, h: c1.h, h1: c1.hash, h2: c2.hash, mad: mad(c1.sig, c2.sig), nonblack: c1.nonblack }));
  check('C3 canvas-captured audio still reaches the viewer', cTone.toneShare > 0.12, J(cTone));
  // pause on the canvas path too
  await call('alice', `c.pausePlayback()`); await sleep(1200);
  const cq = await probe('bob'); const cp1 = await frame('bob'), cp2 = await frame('bob');
  check('C4 canvas path pause: silence and a stable last frame', cq.toneShare < 0.02 && cp1.hash === cp2.hash, J({ tone: cq.toneShare, h1: cp1.hash, h2: cp2.hash }));
  await call('alice', `c.stopPlayback()`);
  await page('alice').ev(`(window.__commonsCall.prefs.forceCanvasPlayback = false, true)`);

  // ---------------------------------------------------------------- D. Jellyfin (a stand-in with the same API and CORS)
  await page('alice').send('Page.bringToFront'); await sleep(300);
  await call('alice', `c.saveJellyfinConfig(${J(jfBase)}, 'test-jellyfin-key')`);
  const items = await call('alice', `c.jellyfinSearch('colour')`);
  check('D1 Jellyfin search returns items (API key header and CORS work)', Array.isArray(items) && items.length === 2 && items[0].name === 'Colour Bars: The Movie', J(items));
  const imgOk = await page('alice').ev(`(async()=>{ const url = window.__commonsCall.jellyfinImageUrl(${J(items[0])}); if (!url) return 'no url'; return await new Promise(r => { const i = new Image(); i.onload = () => r('ok ' + i.naturalWidth + 'x' + i.naturalHeight); i.onerror = () => r('error'); i.src = url; setTimeout(() => r('timeout'), 8000); }); })()`);
  check('D2 Jellyfin poster images load (API key in the query)', String(imgOk).startsWith('ok'), String(imgOk));
  await call('alice', `c.startJellyfinPlayback(${J(items[0])})`);
  const jp = await until(async () => await call('alice', `c.play`), 30000, 'jellyfin playback');
  const j1 = await frame('bob'), j2 = await frame('bob');
  const jTone = await heard('bob', 10);
  check('D3 a Jellyfin item plays into the call with picture and sound', jp && jp.kind === 'jellyfin' && j1.present && j1.hash !== j2.hash && jTone.toneShare > 0.12, J({ kind: jp && jp.kind, w: j1.w, h1: j1.hash, h2: j2.hash, tone: jTone.toneShare }));
  check('D4 it used direct play (static stream), not a transcode', jfHits.stream >= 1 && jfHits.transcode === 0, J(jfHits));
  const jparts = (await lkAdmin('ListParticipants', { room: roomName })).participants || [];
  const jt = (((jparts.find(p => p.identity === dev('alice')) || {}).tracks) || []).filter(t => t.name === 'movie' || t.name === 'movie-audio');
  check('D5 the Jellyfin movie tracks are encrypted on the server too', jt.length === 2 && jt.every(t => t.encryption === 'GCM' || t.encryption === 1), J(jt.map(t => [t.name, t.encryption])));

  // ---------------------------------------------------------------- E. one share at a time
  const errs = await page('alice').ev(`(async()=>{
    const c = window.__commonsCall; const out = {};
    try { await c.startFilePlayback(window.__testFile); out.second = 'started'; } catch (e) { out.second = e.message; }
    await c.stopPlayback();
    return out; })()`);
  check('E1 a second movie while one is playing is refused', /already playing/.test(errs.second), J(errs));
  await page('alice').ev(`(async()=>{ const d = document.createElement('canvas'); d.width=1280; d.height=720; const g=d.getContext('2d'); setInterval(()=>{g.fillStyle='#123';g.fillRect(0,0,1280,720);},66); navigator.mediaDevices.getDisplayMedia = async()=>d.captureStream(15); return true; })()`);
  await call('alice', `c.startShare({ audio: false, profile: 'motion' })`);
  const busy = await page('alice').ev(`(async()=>{ const c=window.__commonsCall; try { await c.startFilePlayback(window.__testFile); return 'started'; } catch(e) { return e.message; } })()`);
  check('E2 a movie during a screen share is refused', /Stop sharing your screen/.test(busy), busy);
  await call('alice', `c.stopShare()`);
  await call('alice', `c.startFilePlayback(window.__testFile)`);
  const busy2 = await page('alice').ev(`(async()=>{ const c=window.__commonsCall; try { await c.startShare(); return 'started'; } catch(e) { return e.message; } })()`);
  check('E3 starting a screen share while a movie plays is refused', /Stop playing the video/.test(busy2), busy2);
  await call('alice', `c.stopPlayback()`);

  // ---------------------------------------------------------------- F. the picker dialog in the real UI
  await page('alice').ev(`(document.querySelector('#c-playopen').click(), true)`);
  await page('alice').waitFor(`!document.querySelector('#play-dialog').hidden`, 8000, 'play dialog');
  await page('alice').waitFor(`document.querySelectorAll('#pd-results .pd-item').length === 2`, 15000, 'jellyfin results in the dialog');
  const ui = await page('alice').ev(`({ file: !!document.querySelector('#pd-file'), profiles: document.querySelectorAll('#pd-profile option').length,
    items: document.querySelectorAll('#pd-results .pd-item').length, note: document.querySelector('#pd-jfnote').innerText })`);
  check('F1 the play dialog offers a file picker, three qualities and Jellyfin results', ui.file && ui.profiles === 3 && ui.items === 2, J(ui));
  await page('alice').ev(`(document.querySelector('#pd-close').click(), true)`);

  // ---------------------------------------------------------------- G. frame encryption reaches the new tracks (operator view)
  // An operator who can mint tokens joins without the keys, subscribes to the movie audio and tries to decode the Opus
  // frames it receives. With E2EE these are ciphertext: no tone.
  await page('alice').ev(`(window.__commonsCall.prefs.forceCanvasPlayback = false, true)`);
  await call('alice', `c.startFilePlayback(window.__testFile)`);
  await until(async () => await call('alice', `c.play`), 25000, 'playback for operator');
  await sleep(2500);
  const spy = await openPage(P('alice').chrome, `http://127.0.0.1:${P('alice').port}/vendor/README.txt`, 'spy');
  await page('alice').send('Page.bringToFront'); await sleep(300);   // the spy tab must not put the playing tab in the background
  const UMD = fs.readFileSync(path.join(root, 'app/ui/vendor/livekit-client.umd.js'), 'utf8');
  await spy.ev(UMD);
  const b64u = b => Buffer.from(b).toString('base64url');
  const jwt = claims => { const h = b64u(JSON.stringify({ alg: 'HS256', typ: 'JWT' })), p = b64u(JSON.stringify(claims)); return `${h}.${p}.${b64u(crypto.createHmac('sha256', 'commons-local-test-secret-0123456789abcdef').update(`${h}.${p}`).digest())}`; };
  const now = Math.floor(Date.now() / 1000);
  const tok = jwt({ iss: 'commonstest', sub: 'operator-movie', nbf: now - 10, exp: now + 600, video: { room: roomName, roomJoin: true, canPublish: false, canSubscribe: true } });
  const spyRes = await spy.ev(`(async()=>{
    const Orig = window.RTCPeerConnection; window.__frames = {};
    window.RTCPeerConnection = function (cfg, ...r) { cfg = Object.assign({}, cfg, { encodedInsertableStreams: true }); const pc = new Orig(cfg, ...r); const set = pc.setConfiguration.bind(pc); pc.setConfiguration = c => set(Object.assign({}, c, { encodedInsertableStreams: true }));
      pc.addEventListener('track', ev => { if (ev.track.kind !== 'audio') return; const key = ev.track.id; window.__frames[key] = []; const { readable, writable } = ev.receiver.createEncodedStreams();
        readable.pipeThrough(new TransformStream({ transform(f, ctl) { if (window.__frames[key].length < 120) window.__frames[key].push(Array.from(new Uint8Array(f.data))); ctl.enqueue(f); } })).pipeTo(writable); });
      return pc; };
    window.RTCPeerConnection.prototype = Orig.prototype;
    const room = new LivekitClient.Room({ adaptiveStream: false }); window.__room = room;
    room.on('trackSubscribed', (t, pub, p) => { window.__sub = window.__sub || []; window.__sub.push({ who: p.identity, src: pub.source, name: pub.trackName, enc: pub.isEncrypted, id: t.mediaStreamTrack.id }); });
    await room.connect('ws://127.0.0.1:${LK_PORT}', ${J(tok)}, { autoSubscribe: true });
    return true; })()`);
  await sleep(5000);
  const sub = await spy.ev(`window.__sub || []`);
  const an = await spy.ev(`(async()=>{
    const out = [];
    for (const [id, fr] of Object.entries(window.__frames)) {
      const s = (window.__sub || []).find(x => x.id === id); if (!s) continue;
      let toneShare = null, derr = 0;
      try { const pcm = []; const dec = new AudioDecoder({ output: d => { const a = new Float32Array(d.numberOfFrames); d.copyTo(a, { planeIndex: 0, format: 'f32-planar' }); pcm.push(a); d.close(); }, error: () => derr++ });
        dec.configure({ codec: 'opus', sampleRate: 48000, numberOfChannels: 2 }); let ts = 0;
        for (const f of fr) { try { dec.decode(new EncodedAudioChunk({ type: 'key', timestamp: ts, data: new Uint8Array(f) })); } catch { derr++; } ts += 20000; }
        await dec.flush().catch(() => derr++);
        const x = new Float32Array(pcm.reduce((n, a) => n + a.length, 0)); let o = 0; for (const a of pcm) { x.set(a, o); o += a.length; }
        const N = Math.min(x.length, 48000); let re = 0, im = 0, tot = 0; for (let i = 0; i < N; i++) { re += x[i] * Math.cos(2 * Math.PI * 880 * i / 48000); im += x[i] * Math.sin(2 * Math.PI * 880 * i / 48000); tot += x[i] * x[i]; }
        toneShare = tot > 0 ? (2 * (re * re + im * im) / N) / tot : 0; if (!(N > 4000)) toneShare = null;
      } catch (e) { toneShare = 'ERR ' + e.message; }
      out.push({ who: s.who.slice(0, 8), name: s.name, enc: s.enc, frames: fr.length, toneShare, derr });
    }
    return out; })()`);
  console.log('operator view of movie audio:', J(an));
  const movieFrames = an.filter(a => a.name === 'movie-audio');
  check('G1 the operator sees the movie audio marked encrypted and cannot decode it', movieFrames.length > 0 && movieFrames.every(a => a.enc && (typeof a.toneShare === 'number' ? a.toneShare < 0.05 : a.toneShare === null)) && sub.some(s => s.name === 'movie-audio'), J(an));
  await spy.ev(`window.__room.disconnect().then(() => true)`);
  await call('alice', `c.stopPlayback()`);

  // ---------------------------------------------------------------- H. setup is per device and never in snapshot / relay
  const jfStored = await page('alice').ev(`JSON.parse(localStorage.getItem('commons.call.jellyfin')).key`);
  const snapHasKey = await page('alice').ev(`JSON.stringify(window.__commonsCall.snapshot()).includes('test-jellyfin-key')`);
  check('H1 the Jellyfin key is in this device\'s localStorage only, and not in the UI snapshot', jfStored === 'test-jellyfin-key' && !snapHasKey, J({ stored: jfStored, inSnapshot: snapHasKey }));

} catch (e) {
  console.error('TEST RUN ABORTED:', e);
  failed++; results.push({ name: 'run', ok: false, detail: String(e.stack || e) });
} finally {
  fs.writeFileSync(path.join(out, 'results.json'), JSON.stringify(results, null, 1));
  jf.close();
  if (world) for (const n of Object.keys(world.people)) { const p = world.people[n]; if (p.page && p.page.console.length) fs.writeFileSync(path.join(out, `console-${n}.txt`), p.page.console.join('\n')); }
  stopAll();
  console.log(`\n${results.filter(r => r.ok).length} passed, ${failed} failed. Output in ${out}`);
  process.exitCode = failed ? 1 : 0;
}
