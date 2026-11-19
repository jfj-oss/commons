// The native call UI against the web's: side-by-side screenshots (web | native, same window size) of each call state,
// and every native call control clicked with REAL pointer events (examples/fakekeys.rs) with the engine state asserted.
// Alice is the native WINDOW (`mega-gpui`) in a private headless KWin; Bob, Carol and Erin are the real app/ui in headless
// Chrome for Testing (fake media, silent). One local LiveKit + relay, throwaway accounts. Nobody joins a real room.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin mega-gpui --bin commons-relay --bin dev_bridge
//   cargo build --release -p mega-gpui --example fakekeys
//   COMMONS_TEST_PORT_OFFSET=4100 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-callui-shots.mjs <outdir>
// CALLUI_ONLY=shots|controls limits the run. Pairs are written to <outdir>/shots/<n>-<name>.png (web left, native right).
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, openPage, rpc, sleep, stopAll, procs, RELAY_PORT, port as P, FAKE_DISPLAY } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';
import { startFakeRune } from './lib/fake-rune.mjs';
const fakes = [];
const soft = p => p.catch(e => ({ timeout: String(e.message || e) }));

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/ds-mega/callui/run');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const shots = path.join(out, 'shots'); fs.mkdirSync(shots, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const FAKE = process.env.MEGA_GPUI_FAKEKEYS || path.join(root, 'target/release/examples/fakekeys');
const ONLY = process.env.CALLUI_ONLY || '';
const PASS = 'correct horse battery';
const REL = `http://127.0.0.1:${RELAY_PORT}`;
const SOCK = 'wayland-callshot';
const W_ = 1280, H_ = 900;
const J = x => JSON.stringify(x);
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? '  [' + (typeof detail === 'string' ? detail : J(detail)).slice(0, 300) + ']' : ''}`); };
async function until(fn, ms, what, step = 250) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(step); } }

class Win {
  constructor(port) { this.port = port; this.n = 0; this.waiting = new Map(); this.buf = ''; }
  async connect() {
    this.sock = net.connect(this.port, '127.0.0.1');
    await new Promise((res, rej) => { this.sock.once('connect', res); this.sock.once('error', rej); });
    this.sock.on('data', d => { this.buf += d; let i; while ((i = this.buf.indexOf('\n')) >= 0) { const l = this.buf.slice(0, i); this.buf = this.buf.slice(i + 1); const j = JSON.parse(l); const w = this.waiting.get(j.id); if (w) { this.waiting.delete(j.id); w(j); } } });
  }
  cmd(cmd, args = {}, ms = 20000) {
    const id = ++this.n;
    return new Promise((res, rej) => { const t = setTimeout(() => { this.waiting.delete(id); rej(new Error('no answer to ' + cmd)); }, ms); this.waiting.set(id, j => { clearTimeout(t); res(j); }); this.sock.write(J({ id, cmd, ...args }) + '\n'); });
  }
}

const priv = path.join(out, 'priv'); const xdg = { XDG_RUNTIME_DIR: path.join(priv, 'run'), XDG_CONFIG_HOME: path.join(priv, 'config'), XDG_STATE_HOME: path.join(priv, 'state'), XDG_CACHE_HOME: path.join(priv, 'cache'), XDG_DATA_HOME: path.join(priv, 'data') };
for (const d of Object.values(xdg)) fs.mkdirSync(d, { recursive: true, mode: 0o700 });
const cleanEnv = () => { const e = { ...process.env, ...xdg }; for (const k of ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'XDG_SESSION_ID', 'PIPEWIRE_RUNTIME_DIR', 'PULSE_SERVER', 'PULSE_RUNTIME_PATH']) delete e[k]; return e; };
const busConf = (addrFile) => { const c = path.join(priv, path.basename(addrFile) + '.conf'); fs.writeFileSync(c, `<busconfig><type>session</type><listen>unix:path=${path.join(priv, path.basename(addrFile) + '.sock')}</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>`); return c; };
function sh(cmd, args, env, log) { const p = spawn(cmd, args, { env, stdio: ['ignore', fs.openSync(log, 'a'), fs.openSync(log, 'a')], detached: true }); procs.push(p); return p; }

try {
  await startServers(out, {});
  await until(async () => (await fetch(REL + '/health')).ok, 30000, 'relay up', 400);
  const world = await makeWorld(out, ['bob', 'carol', 'erin']);
  const people = world.people, bob = people.bob, carol = people.carol, erin = people.erin;
  const cid = world.cid;
  await rpc(bob.port, 'create_channel', { cid, name: 'lounge', kind: 'voice' });
  const voiceGid = async p => { const c = (await rpc(p.port, 'snapshot')).communities.find(c => c.id === cid); const v = c && c.channels.find(x => x.kind === 'voice'); return v && v.joined && v.status === 'ok' ? v.gid : null; };
  for (const p of [bob, carol, erin]) await until(() => voiceGid(p), 60000, p.name + ' has the voice room', 500);
  const gid = await voiceGid(bob);
  // Alice: her account is made by a bridge, which then stops so the native window can open the same folder
  const alice = { name: 'alice', port: P(27210) };
  const ab = startBridge(out, 'alice', alice.port); await sleep(1300);
  await rpc(alice.port, 'create_account', { name: 'Alice', passphrase: PASS, relay: REL });
  await rpc(alice.port, 'join', { link: (await rpc(bob.port, 'create_invite', { cid, uses: 5, ttl: 3600 })).link });
  await until(async () => { const c = (await rpc(alice.port, 'snapshot')).communities.find(c => c.id === cid); return c && c.channels.length >= 2 && c.channels.every(x => x.joined && x.status === 'ok'); }, 90000, 'alice ready', 500);
  alice.account = (await rpc(alice.port, 'snapshot')).me.account;
  try { process.kill(-ab.pid, 'SIGTERM'); } catch { } await sleep(1800);

  const natRune = await startFakeRune(P(28100));
  const bobRune = await startFakeRune(P(28101), { spotify: true, tracks: [{ title: 'Shared Spotify Song', artist: 'Bob Band' }] });
  await rpc(bob.port, 'set_listening_sharing', { on: true, rune: bobRune.url });
  const kwBusFile = path.join(out, 'kwin-bus');
  sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket ${SOCK} --no-lockscreen --no-global-shortcuts --width ${W_} --height ${H_}`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(out, 'kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, SOCK)), 20000, 'kwin socket');
  const winEnv = () => ({ ...cleanEnv(), WAYLAND_DISPLAY: SOCK, DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim() });
  const nshotTo = f => { spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); return f; };
  const fk = async (...steps) => { const r = spawnSync(FAKE, steps, { env: winEnv(), encoding: 'utf8' }); if (r.status !== 0) throw new Error('fakekeys ' + steps.join(' ') + ': ' + r.stderr); };
  const fkAsync = (...steps) => new Promise((res, rej) => { const c = spawn(FAKE, steps, { env: winEnv(), stdio: ['ignore', 'ignore', 'pipe'] }); let e = ''; c.stderr.on('data', d => e += d); c.on('exit', code => code === 0 ? res() : rej(new Error('fakekeys ' + steps.join(' ') + ': ' + e))); });
  const wport = P(27311);
  const winProc = sh(BIN, [], { ...winEnv(), COMMONS_DATA: path.join(out, 'data', 'alice'), COMMONS_RELAY: REL, COMMONS_FAST_KDF: '1', COMMONS_RUNE: natRune.url, MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_CALL_TONE: '900', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_UI_TEST: String(wport), MEGA_GPUI_SIZE: `${W_}x${H_}`, RUST_BACKTRACE: '1' }, path.join(out, 'alice-window.log'));
  const A = new Win(wport);
  await until(async () => { try { await A.connect(); return true; } catch { return false; } }, 30000, 'window test port');
  await until(async () => (await A.cmd('core', { method: 'snapshot' })).ok, 30000, 'window core unlocked');
  await sleep(2500);

  await openApps(world, ['bob', 'carol', 'erin'], { before: FAKE_DISPLAY });
  for (const p of [bob, carol, erin]) await p.page.send('Emulation.setDeviceMetricsOverride', { width: W_, height: H_, deviceScaleFactor: 1, mobile: false });
  const web = (p, expr) => p.page.ev(`(async()=>{ const c = window.__commonsCall; return ${expr}; })()`);
  const wshotOf = async (p, name) => { const r = await p.page.send('Page.captureScreenshot', { format: 'png' }); fs.writeFileSync(path.join(shots, name + '.png'), Buffer.from(r.result.data, 'base64')); };
  const nshot = async name => nshotTo(path.join(shots, name + '.png'));
  const pairs = []; let pn = 0;
  const pair = async (name, who = bob) => { const n = String(++pn).padStart(2, '0') + '-' + name; await wshotOf(who, n + '-web'); await nshot(n + '-native'); pairs.push(n); spawnSync('python3', [path.join(root, 'tools/lib/sidebyside.py'), path.join(shots, n + '-web.png'), path.join(shots, n + '-native.png'), path.join(shots, n + '.png')]); };
  const only = k => ONLY === '' || ONLY === k;
  const join = async p => { await web(p, `c.join(${J(gid)}, 'lounge')`); await until(() => web(p, `c.phase === 'connected'`), 25000, p.name + ' connected'); };
  const snap = () => A.cmd('callsnap');
  const hold = async tag => { if (process.env.CALLUI_HOLD === tag) { console.log('HOLD', J({ wport, out })); await sleep(900000); } };
  const waitSnap = (pred, what, ms = 10000) => until(async () => { const s = await snap(); return pred(s) ? s : null; }, ms, what, 200);
  // the window is laid out a margin larger than the output on every side (the client-side shadow): window pixels to screen pixels
  let _off = [0, 0];
  const toScreen = h => h && { x: h.x + _off[0], y: h.y + _off[1], w: h.w, h: h.h };
  const H = async k => toScreen((await snap()).hits[k]);
  const center = h => [Math.round(h.x + h.w / 2), Math.round(h.y + h.h / 2)];
  const clickAt = async (x, y) => { await fk(`move:${x},${y}`, 'click'); await sleep(450); };
  const click = async k => { const h = await H(k); if (!h) throw new Error('no control painted: ' + k); const [x, y] = center(h); await clickAt(x, y); };
  const alicePeer = p => web(p, `(() => { const q = c.snapshot().peers.find(x => x.name === 'Alice'); return q ? { muted: q.muted, deafened: q.deafened, speaking: q.speaking } : null })()`);
  const SENT = ['mute', 'deafen', 'share', 'camera', 'settings'];
  await sleep(2500);

  // ---- 1. the voice room before joining: the web's card against the native one
  await A.cmd('server', { name: 'Test club' }); await sleep(1500);
  await bob.page.send('Page.bringToFront'); await sleep(400);
  await bob.page.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'ArrowDown', code: 'ArrowDown', windowsVirtualKeyCode: 40 }); await bob.page.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'ArrowDown', code: 'ArrowDown', windowsVirtualKeyCode: 40 }); await sleep(2500);
  await A.cmd('navkey', { key: 'down' }); await sleep(2500);
  await pair('voice-before');
  // ---- 2. an incoming ring: Bob joins and rings the room; Erin's page (not in the call) against Alice's window
  await join(bob);
  await bob.page.ev(`__megaRing.ringAll()`);
  await until(async () => (await snap()).ring.cards.length === 1, 20000, 'alice gets the ring card');
  await erin.page.ev(`1`); await until(() => erin.page.ev(`__megaRing.cards.size > 0`), 20000, 'erin gets the ring card'); await sleep(1500);
  await pair('ring', erin);
  // decline with a real click on the card, then ring again and join with a real click
  await click('ring-no');
  check('R1 clicking decline on the ring card removes it and Alice stays out of the call', (await snap()).ring.cards.length === 0 && (await snap()).phase !== 'connected');
  await join(carol); await carol.page.ev(`__megaRing.ringAll()`);
  await until(async () => (await snap()).ring.cards.length === 1, 20000, 'second ring card');
  await click('ring-yes');
  await waitSnap(s => s.phase === 'connected' && s.peers.length >= 2, 'alice joined by the ring card', 30000);
  check('R2 clicking join on the ring card joins the call with the others in it', true);
  await sleep(3500);
  if (only('shots')) {
    await pair('call-2');
    await join(erin); await sleep(4000);
    await pair('call-4');
    await web(carol, `c.setMuted(true)`); await web(erin, `c.setDeafened(true)`); await sleep(2500);
    await pair('muted-deafened');
    await web(carol, `c.setMuted(false)`); await web(erin, `c.setDeafened(false)`); await sleep(1500);
    await web(carol, `c.setCamera(true)`); await sleep(5000);
    await pair('camera');
    await web(erin, `c.startShare()`); await sleep(5000);
    await pair('share');
    await bob.page.ev(`document.querySelector('#c-set').click()`); await A.cmd('call_settings', { on: true }); await sleep(1800);
    await pair('settings');
    await bob.page.ev(`document.querySelector('#cs-close').click()`); await A.cmd('call_settings', { on: false }); await sleep(800);
    await A.cmd('call_settings', { on: false });
  }
  if (ONLY === 'controls') { await join(erin); await sleep(3000); }

  // ---------------------------------------------------------------- the controls, clicked with real pointer events
  let s0 = await snap();
  _off = [Math.round((W_ - s0.viewport[0]) / 2), Math.round((H_ - s0.viewport[1]) / 2)]; console.log('off', J(_off));
  console.log('viewport', J(s0.viewport), J(s0.bounds));
  check('K0 the card is on screen with its controls painted', ['c-mute', 'c-deaf', 'c-share', 'c-cam', 'c-set', 'c-ring', 'c-pop', 'c-mini', 'c-leave', 'c-enc'].every(k => s0.hits[k]), Object.keys(s0.hits));
  // mute
  await click('c-mute'); let s = await waitSnap(x => x.muted, 'muted');
  const pm = await soft(until(async () => { const q = await alicePeer(bob); return q && q.muted ? q : null; }, 10000, 'bob sees alice muted'));
  check('K1 the mute button mutes Alice (engine) and the others see her muted', s.muted && !pm.timeout, pm);
  await hold('unmute');
  await click('c-mute'); s = await waitSnap(x => !x.muted, 'unmuted');
  check('K2 the mute button again unmutes', !s.muted);
  // deafen
  await click('c-deaf'); s = await waitSnap(x => x.deafened, 'deafened');
  const pd = await soft(until(async () => { const q = await alicePeer(carol); return q && q.deafened ? q : null; }, 10000, 'carol sees alice deafened'));
  check('K3 the deafen button deafens Alice and the others see it', s.deafened && !pd.timeout, pd);
  await pair('native-deafened', bob);
  await click('c-deaf'); s = await waitSnap(x => !x.deafened, 'undeafened'); check('K4 deafen again undoes it', !s.deafened);
  // lock
  await click('c-enc'); s = await waitSnap(x => x.ui.explain, 'explainer'); await nshot('native-explainer');
  check('K5 the lock opens the explainer', s.ui.explain);
  await click('c-enc'); s = await waitSnap(x => !x.ui.explain, 'explainer closed'); check('K6 the lock again closes it', !s.ui.explain);
  // share, with the synthetic screen
  await click('c-share'); s = await waitSnap(x => x.share === 'live', 'share live', 25000);
  const sh1 = await soft(until(() => web(bob, `c.snapshot().shares.some(x => x.name === 'Alice' && x.source === 'screen_share')`), 20000, 'bob sees the share'));
  check('K7 the share button starts a screen share and the others receive it', s.share === 'live' && sh1 === true, sh1);
  await click('c-share'); s = await waitSnap(x => x.share === 'idle', 'share idle'); check('K8 the share button again stops it', s.share === 'idle');
  // camera
  await click('c-cam'); s = await waitSnap(x => x.camera === 'live', 'camera live', 25000);
  const cm1 = await soft(until(() => web(bob, `c.snapshot().shares.some(x => x.name === 'Alice' && x.source === 'camera')`), 20000, 'bob sees the camera'));
  check('K9 the camera button turns the camera on and the others receive it', s.camera === 'live' && cm1 === true, cm1);
  await click('c-cam'); s = await waitSnap(x => x.camera === 'off', 'camera off'); check('K10 the camera button again turns it off', s.camera === 'off');
  // settings
  await click('c-set'); s = await waitSnap(x => x.ui.settings && x.hits['cs-close'], 'settings open');
  check('K11 the settings button opens the dialog', s.ui.settings);
  const sw = async (k, key, want) => { await click(k); const r = await soft(waitSnap(x => x[key] === want, k)); return r; };
  let r = await sw('cs-agc', 'agc', false); check('K12 the automatic gain switch changes the engine', !r.timeout && r.agc === false, r.agc);
  await sw('cs-agc', 'agc', true);
  r = await sw('cs-noise', 'noise', false); check('K13 the noise suppression switch changes the engine', !r.timeout && r.noise === false, r.noise);
  r = await sw('cs-echo', 'echo', false); check('K14 the echo cancellation switch changes the engine (noise off first: the two are tied)', !r.timeout && r.echo === false, r.echo);
  await sw('cs-echo', 'echo', true); await sw('cs-noise', 'noise', true);
  await click('cs-share'); await click('cs-share-1'); s = await waitSnap(x => x.ui.shareProfile === 'motion', 'share as motion');
  check('K15 the "share as" list picks a profile', s.ui.shareProfile === 'motion', s.ui.shareProfile);
  await click('cs-camq'); await click('cs-camq-1'); await nshot('native-settings-open-list'); await sleep(300);
  s = await snap(); check('K16 the camera quality list closes after a pick', s.ui.dd === null, s.ui.dd);
  await click('cs-mic'); s = await snap(); check('K17 the microphone list opens', s.ui.dd === 'cs-mic', s.ui.dd);
  await click('cs-mic-0'); s = await snap(); check('K18 picking the system default closes it', s.ui.dd === null && s.micId === null, s.micId);
  r = await sw('cs-ptt', 'pushToTalk', true); check('K19 the push to talk switch turns the mode on (and mutes)', !r.timeout && r.pushToTalk && r.muted, r.pushToTalk);
  await click('cs-close'); s = await waitSnap(x => !x.ui.settings, 'settings closed'); check('K20 the close button closes the dialog', !s.ui.settings);
  await click('c-set'); await waitSnap(x => x.ui.settings, 'settings again');
  await clickAt(8, 8); s = await waitSnap(x => !x.ui.settings, 'scrim closes'); check('K21 a click on the dark scrim closes the dialog', !s.ui.settings);
  // push to talk: the button on the card, then the key
  s = await snap(); check('K22 with push to talk on, the card shows the talk button', !!s.hits['c-ptt']);
  await hold('ptt');
  { const [x, y] = center(toScreen(s.hits['c-ptt'])); const held = fkAsync(`move:${x},${y}`, 'mousedown', 'sleep:3500', 'mouseup');
    s = await waitSnap(x => x.pttHeld && !x.muted, 'ptt held by the button'); await nshot('native-ptt-held'); check('K23 holding the talk button unmutes while held', s.pttHeld && !s.muted);
    await held; s = await waitSnap(x => !x.pttHeld && x.muted, 'ptt released'); check('K24 releasing it mutes again', !s.pttHeld && s.muted); }
  { const keyHeld = fkAsync('keydown:`', 'sleep:2500', 'keyup:`'); s = await waitSnap(x => x.pttHeld && !x.muted, 'ptt held by the key'); check('K25 holding the push-to-talk key (backtick) unmutes while held', s.pttHeld);
    await keyHeld; s = await waitSnap(x => !x.pttHeld && x.muted, 'ptt key released'); check('K26 releasing the key mutes again', !s.pttHeld); }
  await A.cmd('call_settings', { on: true }); await sleep(300); await click('cs-ptt'); s = await waitSnap(x => !x.pushToTalk, 'ptt off'); await click('cs-close');
  s = await snap(); if (s.muted) { await click('c-mute'); await waitSnap(x => !x.muted, 'unmuted again'); }
  // ring
  await click('c-ring'); s = await waitSnap(x => x.ring.outs.includes('*'), 'ringing'); check('K27 the ring button starts ringing the community', s.ring.outs.includes('*'), s.ring.outs);
  await click('c-ring'); s = await waitSnap(x => !x.ring.outs.includes('*'), 'ring stopped'); check('K28 the ring button again stops it', !s.ring.outs.includes('*'));
  // people: a click opens the volume menu
  s = await snap(); const bobP = s.peers.find(p => p.name === 'Bob'), carolP = s.peers.find(p => p.name === 'Carol');
  await click('peer-' + bobP.identity); s = await waitSnap(x => x.ui.menu === 'Bob', 'bob menu'); await nshot('native-volume-menu'); check('K29 clicking a person opens their volume menu', s.ui.menu === 'Bob', s.ui.menu);
  await click('v-0'); s = await waitSnap(x => x.peers.find(p => p.name === 'Bob').volume === 0, 'bob silenced'); check('K30 "mute for me" silences that person for Alice', s.peers.find(p => p.name === 'Bob').volume === 0);
  await click('v-1'); s = await waitSnap(x => x.peers.find(p => p.name === 'Bob').volume === 1, 'bob normal'); check('K31 "normal" puts the volume back', s.peers.find(p => p.name === 'Bob').volume === 1);
  { const h = await H('v-slider'); await clickAt(Math.round(h.x + h.w * 0.25), Math.round(h.y + h.h / 2)); s = await snap(); const v = s.peers.find(p => p.name === 'Bob').volume; check('K32 a click on the volume slider sets the volume', v > 0 && v < 1, v); }
  await click('v-x'); s = await waitSnap(x => x.ui.menu === null, 'menu closed'); check('K33 close shuts the menu', s.ui.menu === null);
  { const h = await H('peer-' + carolP.identity); const [x, y] = center(h); await fk(`move:${x},${y}`, 'rclick'); await sleep(500); s = await snap(); check('K34 a right click on a person opens their menu too', s.ui.menu === 'Carol', s.ui.menu); await click('v-x'); }
  // the column of cameras and screens
  await web(carol, `c.setCamera(true)`); await web(erin, `c.startShare()`);
  s = await waitSnap(x => x.ui.tiles.length >= 2, 'two tiles', 30000);
  check('K35 the camera and the screen of the others appear as tiles in the column (in a room list too, not only in a chat)', s.ui.tiles.length >= 2 && !!s.hits['deck-grip'], s.ui.tiles);
  await sleep(1500); await nshot('native-column');
  const w0 = await H('deck-grip');
  { const [x, y] = center(w0); await fk(`move:${x},${y}`, 'mousedown', `move:${x + 60},${y}`, `move:${x + 110},${y}`, 'mouseup'); }
  s = await waitSnap(x => x.ui.deckW > 0, 'column resized'); const wDrag = s.ui.deckW;
  check('K36 dragging the column\'s edge resizes it (kept per device)', wDrag > 270 && wDrag < 420, wDrag);
  await nshot('native-column-resized');
  { const h = await H('deck-grip'); const [x, y] = center(h); await fk(`move:${x},${y}`, 'dblclick'); }
  s = await waitSnap(x => x.ui.deckW === 0, 'column reset'); check('K37 a double click on the edge puts the width back', s.ui.deckW === 0);
  { const h = await H('deck-grip'); const [x, y] = center(h); await fk(`move:${x},${y}`, 'click', 'key:right', 'key:right'); }
  s = await waitSnap(x => x.ui.deckW > 0, 'arrow keys'); check('K38 the arrow keys on the edge widen the column by steps', s.ui.deckW > 0, s.ui.deckW);
  await fk('key:end'); s = await snap(); const wEnd = s.ui.deckW; await fk('key:home'); s = await snap();
  check('K39 End and Home take the column to its widest and narrowest', wEnd > s.ui.deckW && s.ui.deckW === 200, [wEnd, s.ui.deckW]);
  { const h = await H('deck-grip'); const [x, y] = center(h); await fk(`move:${x},${y}`, 'dblclick'); }
  // reorder: drag the first tile onto the second
  s = await snap(); const [t0, t1] = s.ui.order;
  { const a = toScreen(s.hits['tile-' + t0]), b = toScreen(s.hits['tile-' + t1]); const [ax, ay] = center(a), [bx, by] = center(b);
    await fk(`move:${ax},${ay}`, 'mousedown', `move:${ax + 6},${ay + 10}`, `move:${bx},${by - 10}`, `move:${bx},${by}`, 'sleep:300', 'mouseup'); }
  s = await waitSnap(x => x.ui.order[0] === t1, 'order swapped'); check('K40 dragging a tile onto another swaps their places', s.ui.order[0] === t1 && s.ui.order[1] === t0, s.ui.order);
  const webOrder = await soft(until(async () => { const o = await bob.page.ev(`JSON.stringify(__megaDeck.order())`); const j = JSON.parse(o); return j.length >= 2 && j[0].endsWith(t1.split('/').pop()) ? j : null; }, 30000, 'web order follows'));
  check('K41 the others\' column follows the new order (the shared deck order)', !webOrder.timeout, webOrder.timeout ? [t0, t1, await bob.page.ev(`JSON.stringify(__megaDeck.order())`)] : webOrder);
  await click('deck-hide'); s = await waitSnap(x => x.ui.deckHidden, 'hidden'); check('K42 "hide" takes the column away for Alice', s.ui.deckHidden && !!s.hits['deck-show']);
  await nshot('native-column-hidden');
  await click('deck-show'); s = await waitSnap(x => !x.ui.deckHidden, 'shown'); check('K43 "show tiles" brings it back', !s.ui.deckHidden);
  // rune, beside the card
  await waitSnap(x => x.rune.now && x.rune.now.title, 'rune shows', 20000);
  await nshot('native-rune');
  check('K50 what rune plays shows beside the card', !!(await snap()).rune.now, (await snap()).rune.now);
  await click('rune-play'); await until(() => natRune.cmds.some(c => c.type === 'pause'), 8000, 'pause'); check('K51 the rune pause button reaches rune', true);
  await click('rune-play'); await until(() => natRune.cmds.some(c => c.type === 'resume'), 8000, 'resume'); check('K52 the rune play button reaches rune', true);
  await click('rune-next'); await until(() => natRune.cmds.some(c => c.type === 'next'), 8000, 'next'); check('K53 the rune next button reaches rune', true);
  await click('rune-prev'); await until(() => natRune.cmds.some(c => c.type === 'prev'), 8000, 'prev'); check('K54 the rune previous button reaches rune', true);
  await waitSnap(x => x.rune.friends.length > 0, 'friend sharing', 30000);
  await click('rune-along'); const sp = await soft(until(() => natRune.cmds.find(c => c.type === 'play' && (c.trackIds || []).some(t => String(t).startsWith('sp:'))), 15000, 'along plays'));
  check('K55 listen along plays the friend\'s song on rune', !sp.timeout, sp);
  await click('rune-along'); await waitSnap(x => !x.rune.along, 'along off', 10000); check('K56 the button again stops listening along', true);
  // pop out one tile (its own window), the call's pictures (the column leaves), the mini window
  s = await snap(); const win0 = s.windows;
  await click('tile-pop-' + s.ui.order[0]); s = await waitSnap(x => x.windows === win0 + 1, 'tile window'); check('K44 a tile\'s pop out button opens a window of its own', s.windows === win0 + 1, s.windows);
  await nshot('native-tile-window');
  await click('c-mini'); s = await waitSnap(x => x.ui.mini, 'mini'); await sleep(800); await nshot('native-mini'); check('K45 the mini button opens the small window', s.ui.mini);
  await click('c-mini'); s = await waitSnap(x => !x.ui.mini, 'mini closed'); check('K46 the mini button again closes it', !s.ui.mini);
  await click('c-pop'); s = await waitSnap(x => x.ui.popped, 'popped'); await sleep(1200); await nshot('native-popped'); check('K47 the pop out button opens the call in its own window and the column leaves', s.ui.popped);
  // rune
  // leave
  await click('c-leave'); s = await waitSnap(x => x.phase === 'idle', 'left');
  check('K48 the leave button leaves the call', s.phase === 'idle' && !s.ui.docked);
  const gone = await soft(until(async () => !(await web(bob, `[...c.room.remoteParticipants.values()].some(p => p.name === 'Alice')`)), 15000, 'alice gone for bob'));
  check('K49 the others see Alice leave', gone === true);
  await nshot('native-after-leave');
} catch (e) { check('the run itself', false, String(e && e.stack || e).slice(0, 600)); }
fs.writeFileSync(path.join(out, 'results.json'), J(results, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED');
stopAll(); process.exit(failed ? 1 : 0);
