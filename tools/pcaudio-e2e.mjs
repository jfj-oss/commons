// "Play PC audio into the call" end-to-end: real relay, real LiveKit, two Megatron clients, headless Chromium with FAKE media
// only. The tab route uses a faked getDisplayMedia (an oscillator, as call-e2e does); the app route uses a fake `pactl` and
// `pw-loopback` on the bridge's PATH (tools/fake-pw) and a faked input device named "Megatron PC audio". Nothing is audible,
// no real PipeWire device is created and no real stream is moved. What this CANNOT show: that Chromium really hands over a
// protected player's tab audio, or that real pw-loopback devices appear in a real browser's device list.
//   COMMONS_TEST_PORT_OFFSET=900 COMMONS_TEST_CHROME=/opt/brave-bin/brave systemd-run --user --scope -q -p MemoryMax=8G node tools/pcaudio-e2e.mjs [outdir]
import fs from 'node:fs';
import path from 'node:path';
import { startServers, rpc, sleep, stopAll, lkAdmin, root, FAKE_DISPLAY } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';

const out = process.argv[2] || '/tmp/commons-pcaudio-e2e';
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? '  [' + detail + ']' : ''}`); };
const J = x => JSON.stringify(x);
async function until(fn, ms = 20000, what = 'condition') { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(250); } }

// the fake sound system, shared by Alice's bridge
const fakeBin = path.join(out, 'fakebin'), st = path.join(out, 'fakestate');
fs.mkdirSync(fakeBin, { recursive: true }); fs.mkdirSync(st, { recursive: true });
for (const f of ['pactl', 'pw-loopback']) fs.copyFileSync(path.join(root, 'tools/fake-pw', f), path.join(fakeBin, f)), fs.chmodSync(path.join(fakeBin, f), 0o755);
fs.writeFileSync(path.join(st, 'ids'), '52 77 90');
process.env.PATH = fakeBin + ':' + process.env.PATH; process.env.FAKE_STATE = st;
const lines = f => { try { return fs.readFileSync(path.join(st, f), 'utf8').split('\n').filter(Boolean); } catch { return []; } };
const alive = pid => { try { return !fs.readFileSync(`/proc/${pid}/stat`, 'utf8').includes(') Z'); } catch { return false; } };
const loopPids = () => lines('loopbacks.log').map(l => l.split(' ')[1]);

// page-side fakes: a faked tab picker (records what was asked) and a faked virtual input in the device list
const PAGE_FAKES = FAKE_DISPLAY + `;(() => {
  const orig = navigator.mediaDevices.getDisplayMedia.bind(navigator.mediaDevices);
  window.__gdmCalls = [];
  navigator.mediaDevices.getDisplayMedia = async (o) => {
    window.__gdmCalls.push(JSON.parse(JSON.stringify(o)));
    const ms = await orig(o);
    if (window.__pcNoAudio) ms.getAudioTracks().forEach(t => { t.stop(); ms.removeTrack(t); });
    if (window.__pcSilent) { ms.getAudioTracks().forEach(t => { t.stop(); ms.removeTrack(t); }); const c = new AudioContext(); const d = c.createMediaStreamDestination(); d.stream.getAudioTracks().forEach(t => ms.addTrack(t)); }
    return ms;
  };
  const enumOrig = navigator.mediaDevices.enumerateDevices.bind(navigator.mediaDevices);
  navigator.mediaDevices.enumerateDevices = async () => { const l = await enumOrig(); if (window.__pcDevice) l.push({ kind: 'audioinput', label: 'Megatron PC audio', deviceId: 'fake-pc-audio', groupId: 'g' }); return l; };
})();`;

let world;
try {
  await startServers(out, { relayTtl: 900 });
  world = await makeWorld(out, ['alice', 'bob']);
  await openApps(world, ['alice', 'bob'], { before: PAGE_FAKES });
  const P = n => world.people[n], page = n => P(n).page;
  const call = (n, expr) => page(n).ev(`(async()=>{ const c = window.__commonsCall; return ${expr}; })()`);
  const dev = n => P(n).device;
  const aliceTone = P('alice').tone;

  await call('alice', `c.join(S.gid, 'general')`);
  await call('bob', `c.join(S.gid, 'general')`);
  await until(async () => (await call('bob', `c.room.remoteParticipants.size`)) === 1, 20000, 'bob sees alice');
  await page('alice').ev(`(window.__callUI.open(), true)`);
  await page('bob').ev(`(window.__callUI.open(), true)`);
  await page('alice').send('Page.bringToFront');

  const heard = async (hz, tries = 8) => { for (let i = 0; i < tries; i++) { const r = await call('bob', `c.probeTone(${J(dev('alice'))}, ${hz}, 1800, 'screen_share_audio')`); if (r.present && r.toneShare > 0.12) return r; await sleep(800); } return await call('bob', `c.probeTone(${J(dev('alice'))}, ${hz}, 1800, 'screen_share_audio')`); };
  const serverTracks = async () => ((((await lkAdmin('ListParticipants', { room: (await rpc(P('alice').port, 'call_token', { gid: world.gid })).room })).participants || []).find(p => p.identity === dev('alice')) || {}).tracks) || [];

  // ---------------------------------------------------------------- menu: what is offered where
  await page('alice').ev(`document.getElementById('c-playopen').click()`);
  await until(() => page('alice').ev(`!!document.getElementById('pc-tab') && !!document.getElementById('pc-app')`), 8000, 'both routes in the menu');
  check('M1 on Linux with the tools present the menu offers both routes', true);
  const hide = await page('alice').ev(`(async()=>{
    const c = window.__commonsCall, r = {};
    const set = (k, v) => Object.defineProperty(navigator, k, { get: () => v, configurable: true });
    const ua = navigator.userAgent, pf = navigator.platform;
    set('platform', 'MacIntel'); set('userAgent', 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/150 Safari/537.36'); r.mac = { tab: c.pcAudioTabSupported(), app: await c.pcAudioAppSupported() };
    set('platform', 'Win32'); set('userAgent', 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/150 Safari/537.36'); r.win = { tab: c.pcAudioTabSupported(), app: await c.pcAudioAppSupported() };
    set('platform', 'iPhone'); set('userAgent', 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148'); r.ios = { tab: c.pcAudioTabSupported(), app: await c.pcAudioAppSupported() };
    set('platform', pf); set('userAgent', ua); return r; })()`);
  check('M2 macOS and Windows get route 1 only, the iPhone gets neither', hide.mac.tab && !hide.mac.app && hide.win.tab && !hide.win.app && !hide.ios.tab && !hide.ios.app, J(hide));
  await page('alice').ev(`document.getElementById('play-dialog').hidden = true`);

  // ---------------------------------------------------------------- route 1: a browser tab
  await page('alice').ev(`window.__pcNoAudio = true`);
  const noAudio = await page('alice').ev(`(async()=>{ try { await window.__commonsCall.startTabAudio(); return null; } catch (e) { return e.message; } })()`);
  check('T0 a tab shared without its sound is refused with a clear message', /Share tab audio/.test(noAudio || '') && !(await call('alice', `c.play`)), noAudio);
  await page('alice').ev(`window.__pcNoAudio = false`);

  await page('alice').ev(`document.getElementById('c-playopen').click()`);
  await page('alice').evGesture(`document.getElementById('pc-tab').click()`);
  const pb = await until(async () => await call('alice', `c.play && c.playbackState()`), 15000, 'tab audio playing');
  check('T1 the tab starts as the call\'s play state', pb.kind === 'pcaudio' && pb.profile === 'music', J({ kind: pb.kind, title: pb.title }));
  const asked = await page('alice').ev(`window.__gdmCalls.slice(-1)[0]`);
  check('T2 the tab picker asks for stereo audio with all voice processing off, tabs only, never the call tab', asked.audio && asked.audio.echoCancellation === false && asked.audio.noiseSuppression === false && asked.audio.autoGainControl === false && asked.audio.channelCount === 2 && asked.selfBrowserSurface === 'exclude' && asked.monitorTypeSurfaces === 'exclude', J(asked));
  const tone1 = await heard(1000);
  check('T3 Bob hears the tab\'s sound (1000 Hz) through the call', tone1.present && tone1.toneShare > 0.12, J(tone1));
  let tr = await serverTracks();
  check('T4 the media server sees the same two music tracks, both encrypted', ['music', 'music-audio'].every(n => tr.some(t => t.name === n && (t.encryption === 'GCM' || t.encryption === 1))), J(tr.map(t => [t.name, t.encryption])));
  const enc = await call('alice', `(()=>{ const s = c.play.apub.track.sender; return s ? s.getParameters().encodings.map(e => e.maxBitrate) : null; })()`);
  check('T5 the audio goes out at the 192 kbps music preset', Array.isArray(enc) && enc[0] === 192000, J(enc));
  const shown = await call('bob', `[...c.tracks.values()].filter(x => x.identity === ${J(dev('alice'))}).map(x => x.pub.trackName)`);
  check('T6 Bob sees it like any other played music', shown.includes('music'), J(shown));
  const setts = await call('alice', `c.play.atrack.getSettings()`);
  check('T7 the picture is a canvas and the captured track has no voice processing', (await call('alice', `c.play.capture`)) === 'canvas' && setts.echoCancellation !== true && setts.noiseSuppression !== true, J(setts));
  const busy = await page('alice').ev(`(async()=>{ try { await window.__commonsCall.startTabAudio(); return null; } catch (e) { return e.message; } })()`);
  check('T8 only one thing plays at a time', /already playing/.test(busy || ''), busy);
  const ui = await page('alice').ev(`({ stop: !!document.getElementById('c-playstop'), toggle: !!document.getElementById('c-playtoggle'), title: document.querySelector('.cplay b')?.textContent })`);
  check('T9 the existing Stop control is shown (and no pause or seek)', ui.stop && !ui.toggle, J(ui));
  await page('alice').ev(`document.getElementById('c-playstop').click()`);
  await until(async () => !(await call('alice', `c.play`)), 5000, 'stopped');
  await until(async () => !(await serverTracks()).some(t => t.name === 'music-audio'), 8000, 'tracks unpublished');
  check('T10 Stop ends it and unpublishes both tracks', true);

  // a tab that makes no sound
  await page('alice').ev(`window.__pcSilent = true`);
  await call('alice', `c.startTabAudio()`);
  const quiet = await until(async () => await call('alice', `c.error && c.error.kind === 'pcaudio-silent' ? { msg: c.error.msg, note: c.playbackState().note } : null`), 12000, 'silence warning');
  const noteUi = await until(async () => await page('alice').ev(`(document.getElementById('c-pcnote') || {}).textContent || ''`), 5000, 'note in the play bar');
  check('T11 a silent tab is reported plainly ("no sound is coming from that tab")', /No sound is coming from that tab/.test(quiet.msg) && /No sound is coming from that tab/.test(noteUi), J(quiet));
  await call('alice', `c.stopPlayback()`);
  await page('alice').ev(`window.__pcSilent = false; window.__commonsCall.error = null`);

  // ---------------------------------------------------------------- route 2: an app (fake pactl / pw-loopback)
  await page('alice').ev(`window.__pcDevice = true; window.__commonsCall.pcAudioLabelOverride = null`);
  await page('alice').ev(`document.getElementById('c-playopen').click()`);
  await page('alice').evGesture(`document.getElementById('pc-app').click()`);
  const items = await until(async () => { const l = await page('alice').ev(`[...document.querySelectorAll('#pc-list [data-id]')].map(b => b.dataset.id + ':' + b.textContent)`); return l.length ? l : null; }, 8000, 'app list');
  check('A1 only the music app is listed (browser and our own streams are left out)', items.length === 1 && items[0].startsWith('52:spotify'), J(items));
  const note = await page('alice').ev(`document.getElementById('pc-status').textContent`);
  check('A2 the menu says browser sound is not listed and points to the tab route', /A browser tab/.test(note), note);
  await page('alice').evGesture(`document.querySelector('#pc-list [data-id="52"]').click()`);
  const pb2 = await until(async () => await call('alice', `c.play && c.playbackState()`), 20000, 'app audio playing');
  check('A3 the app starts as the call\'s play state, titled from the stream', pb2.kind === 'pcaudio' && /Waving Flags/.test(pb2.title + pb2.artist), J({ title: pb2.title, artist: pb2.artist }));
  check('A4 the stream was moved onto the virtual sink and the defaults were not touched', lines('moves.log').join() === '52 64' && lines('defaults.log').length === 0, J(lines('moves.log')));
  const lb = lines('loopbacks.log');
  check('A5 two loopbacks: the route and the local listening copy', lb.length === 2 && lb[0].startsWith('megatron_pcaudio_loop ') && lb[1].startsWith('megatron_pcaudio_listen '), J(lb));
  const tone2 = await heard(aliceTone);
  check('A6 Bob hears it through the call', tone2.present && tone2.toneShare > 0.12, J(tone2));
  tr = await serverTracks();
  check('A7 published as the same encrypted music tracks', ['music', 'music-audio'].every(n => tr.some(t => t.name === n && (t.encryption === 'GCM' || t.encryption === 1))), J(tr.map(t => [t.name, t.encryption])));
  const lease = await call('alice', `c.rpc('pcaudio_status', {})`);
  check('A8 the page keeps the route alive with status polls', lease.active === true && lease.alive === true, J(lease));
  await page('alice').ev(`document.getElementById('c-playstop').click()`);
  await until(() => lines('moves.log').join() === '52 64,52 63', 8000, 'moved back');
  await until(() => loopPids().every(p => !alive(p)), 8000, 'loopbacks gone');
  check('A9 Stop moves the stream back and removes both virtual devices', true, J(lines('moves.log')));

  // leaving the call while sharing an app undoes it
  fs.writeFileSync(path.join(st, 'moves.log'), ''); fs.writeFileSync(path.join(st, 'loopbacks.log'), ''); fs.rmSync(path.join(st, 'sink_of_52'), { force: true });
  await call('alice', `c.startAppAudio(52)`);
  await until(() => lines('moves.log').join() === '52 64', 5000, 'moved');
  await call('alice', `c.leave()`);
  await until(() => lines('moves.log').join() === '52 64,52 63', 8000, 'moved back after leaving');
  await until(() => loopPids().every(p => !alive(p)), 8000, 'loopbacks gone after leaving');
  check('A10 leaving the call undoes the route', true);

  // the app's stream ends: sharing stops by itself with a message
  await call('alice', `c.join(S.gid, 'general')`);
  fs.writeFileSync(path.join(st, 'moves.log'), ''); fs.writeFileSync(path.join(st, 'loopbacks.log'), ''); fs.rmSync(path.join(st, 'sink_of_52'), { force: true });
  await call('alice', `c.startAppAudio(52)`);
  fs.writeFileSync(path.join(st, 'ids'), '77 90');
  await until(async () => !(await call('alice', `c.play`)), 25000, 'ended by itself');
  const endMsg = await call('alice', `c.error && c.error.msg`);
  check('A11 when the app stops playing, sharing ends with a clear message', /stopped playing/.test(endMsg || ''), endMsg);
  await until(() => loopPids().every(p => !alive(p)), 8000, 'loopbacks gone after the stream ended');
  fs.writeFileSync(path.join(st, 'ids'), '52 77 90');

  // the bridge is killed while sharing: the kernel ends the loopbacks (parent-death signal)
  fs.writeFileSync(path.join(st, 'moves.log'), ''); fs.writeFileSync(path.join(st, 'loopbacks.log'), ''); fs.rmSync(path.join(st, 'sink_of_52'), { force: true }); call('alice', 'c.error = null');
  await call('alice', `c.startAppAudio(52)`);
  const pids = loopPids();
  const bridgePid = fs.readdirSync('/proc').filter(d => /^\d+$/.test(d)).find(d => { try { const a = fs.readFileSync(`/proc/${d}/cmdline`, 'utf8').split('\0'); return a[0].endsWith('dev_bridge') && a[1] === String(P('alice').port); } catch { return false; } });
  process.kill(Number(bridgePid), 'SIGKILL');
  await until(() => pids.every(p => !alive(p)), 8000, 'loopbacks end with the bridge');
  check('A12 killing the bridge removes the virtual devices (no stale loopbacks)', pids.length === 2);
} catch (e) {
  console.error('TEST RUN ABORTED:', e);
  failed++; results.push({ name: 'run', ok: false, detail: String(e.stack || e) });
} finally {
  fs.writeFileSync(path.join(out, 'results.json'), JSON.stringify(results, null, 1));
  if (world) for (const n of Object.keys(world.people)) { const p = world.people[n]; if (p.page && p.page.console.length) fs.writeFileSync(path.join(out, `console-${n}.txt`), p.page.console.join('\n')); }
  stopAll();
  console.log(`\n${results.filter(r => r.ok).length} passed, ${failed} failed. Output in ${out}`);
  process.exitCode = failed ? 1 : 0;
}
