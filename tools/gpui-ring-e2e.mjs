// Rings, direct calls, quick-reply notifications and rune in the call bar of the native app (CALLS-PLAN.md step 7), against the
// web client. Alice and Bob are the real app/ui in headless Chrome for Testing (mega-ring.js, mega-direct.js, call.js); Nat is the
// native WINDOW (`mega-gpui`) inside a private headless KWin, driven over its test port (app/gpui/src/uitest.rs).
//
// Silent and private: the window opens no audio device (MEGA_GPUI_CALL_AUDIO=none), the ring sound is only recorded in a file
// (MEGA_GPUI_RING_SOUND_FILE), notifications go to a MOCK org.freedesktop.Notifications on its own private bus
// (tools/lib/notifmock.py), rune is a fake (tools/lib/fake-rune.mjs). KWin, the mock and the window run on private buses, in a
// private XDG_RUNTIME_DIR/XDG_CONFIG_HOME/XDG_STATE_HOME: nothing of the user's desktop, bus or audio is seen or touched.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin mega-gpui --bin commons-relay --bin dev_bridge
//   COMMONS_TEST_PORT_OFFSET=1300 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-ring-e2e.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';
import { startFakeRune, SP_ID } from './lib/fake-rune.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-ring-e2e');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const shots = path.join(out, 'shots'); fs.mkdirSync(shots, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const PASS = 'correct horse battery';
const J = x => JSON.stringify(x);
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? '  [' + (typeof detail === 'string' ? detail : J(detail)).slice(0, 300) + ']' : ''}`); };
async function until(fn, ms, what, step = 250) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(step); } }
const soft = p => p.catch(e => ({ timeout: String(e.message || e) }));
const lines = f => { try { return fs.readFileSync(f, 'utf8').split('\n').filter(Boolean); } catch { return []; } };

// ---- the window's test port
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
  dump() { return this.cmd('dump'); }
}

// ---- private KWin, with its own runtime dir and configuration
const priv = path.join(out, 'priv'); const xdg = { XDG_RUNTIME_DIR: path.join(priv, 'run'), XDG_CONFIG_HOME: path.join(priv, 'config'), XDG_STATE_HOME: path.join(priv, 'state'), XDG_CACHE_HOME: path.join(priv, 'cache'), XDG_DATA_HOME: path.join(priv, 'data') };
for (const d of Object.values(xdg)) fs.mkdirSync(d, { recursive: true, mode: 0o700 });
const cleanEnv = () => { const e = { ...process.env, ...xdg }; for (const k of ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'XDG_SESSION_ID', 'PIPEWIRE_RUNTIME_DIR', 'PULSE_SERVER', 'PULSE_RUNTIME_PATH']) delete e[k]; return e; };
const busConf = (addrFile) => { const c = path.join(priv, path.basename(addrFile) + '.conf'); fs.writeFileSync(c, `<busconfig><type>session</type><listen>unix:path=${path.join(priv, path.basename(addrFile) + '.sock')}</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>`); return c; };
function sh(cmd, args, env, log, opts = {}) { const p = spawn(cmd, args, { env, stdio: opts.stdin ? ['pipe', fs.openSync(log, 'a'), fs.openSync(log, 'a')] : ['ignore', fs.openSync(log, 'a'), fs.openSync(log, 'a')], detached: true }); procs.push(p); return p; }

const res = { checks: results };
let nat, kw, mock, fakes = [];
try {
  const lk = await startServers(out, {});
  const world = await makeWorld(out, ['alice', 'bob']);
  await openApps(world, ['alice', 'bob']);
  const A = world.people.alice, B = world.people.bob;
  const web = (p, expr) => p.page.ev(`(async()=>{ return ${expr}; })()`);
  const cid = world.cid;
  const rooms = {};
  for (const name of ['porch', 'terrace']) rooms[name] = (await rpc(A.port, 'create_channel', { cid, name, kind: 'voice' })).gid || (await rpc(A.port, 'create_channel', { cid, name: name + '2', kind: 'voice' })).gid;
  const snapA = await rpc(A.port, 'snapshot');
  const chans = snapA.communities.find(c => c.id === cid).channels;
  const gidOf = name => chans.find(c => c.name === name).gid;
  rooms.porch = gidOf('porch'); rooms.terrace = gidOf('terrace');
  const textGid = chans.find(c => c.kind !== 'voice').gid, textName = chans.find(c => c.kind !== 'voice').name;
  A.account = snapA.me.account; B.account = (await rpc(B.port, 'snapshot')).me.account;

  // Nat's account, made by a bridge that then stops so the window can open the same folder
  const N = { port: P(27203) };
  const natBridge = startBridge(out, 'nat', N.port); await sleep(1200);
  await rpc(N.port, 'create_account', { name: 'Nat', passphrase: PASS, relay: `http://127.0.0.1:${RELAY_PORT}` });
  await rpc(N.port, 'join', { link: (await rpc(A.port, 'create_invite', { cid, uses: 4, ttl: 3600 })).link });
  await until(async () => { const s = await rpc(N.port, 'snapshot'); const c = s.communities.find(c => c.id === cid); return c && ['porch', 'terrace'].every(n => c.channels.some(ch => ch.name === n && ch.joined && ch.status === 'ok')); }, 90000, 'nat has the rooms');
  N.account = (await rpc(N.port, 'snapshot')).me.account;
  try { process.kill(-natBridge.pid, 'SIGTERM'); } catch { } await sleep(1500);
  await until(async () => { const s = await rpc(B.port, 'snapshot'); const c = s.communities.find(c => c.id === cid); return c && ['porch', 'terrace'].every(n => c.channels.some(ch => ch.name === n && ch.joined)); }, 60000, 'bob has the rooms');

  // fake runes: Nat's own (a local queue), and Bob's (a Spotify song he shares)
  const natRune = await startFakeRune(P(28100));
  const bobRune = await startFakeRune(P(28101), { spotify: true, tracks: [{ title: 'Shared Spotify Song', artist: 'Bob Band' }] });
  fakes.push(natRune, bobRune);
  await rpc(B.port, 'set_listening_sharing', { on: true, rune: bobRune.url });

  // the private notification service (its own bus), and a private KWin with the window in it
  const mockLog = path.join(out, 'notif-mock.jsonl'), mockBusFile = path.join(out, 'notif-bus');
  mock = sh('dbus-run-session', [`--config-file=${busConf(mockBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${mockBusFile}; exec python3 ${path.join(root, 'tools/lib/notifmock.py')} ${mockLog} reply`], cleanEnv(), path.join(out, 'notif-mock.log'), { stdin: true });
  await until(() => fs.existsSync(mockBusFile) && fs.readFileSync(mockBusFile, 'utf8').trim(), 10000, 'mock bus');
  const mockSay = line => mock.stdin.write(line + '\n');
  const kwBusFile = path.join(out, 'kwin-bus');
  kw = sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket wayland-ring --no-lockscreen --no-global-shortcuts --width 1280 --height 800`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(out, 'kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, 'wayland-ring')), 20000, 'kwin socket');
  const shot = async name => { const f = path.join(shots, name + '.png'); const r = spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); return r.status === 0 ? f : null; };

  const soundFile = path.join(out, 'ring-sound.jsonl');
  const winPort = P(27310);
  nat = sh(BIN, [], { ...cleanEnv(), WAYLAND_DISPLAY: 'wayland-ring', DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim(), COMMONS_DATA: path.join(out, 'data/nat'), COMMONS_RELAY: `http://127.0.0.1:${RELAY_PORT}`,
    COMMONS_FAST_KDF: '1', COMMONS_RUNE: natRune.url, MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_UI_TEST: String(winPort), MEGA_GPUI_RING_SOUND_FILE: soundFile,
    MEGA_GPUI_NOTIFY_BUS: fs.readFileSync(mockBusFile, 'utf8').trim(), RUST_LOG: 'info' }, path.join(out, 'nat.log'));
  const W = new Win(winPort);
  await until(async () => { try { await W.connect(); return true; } catch { return false; } }, 30000, 'window test port');
  await until(async () => { const s = await rpc(B.port, 'snapshot'); return true; }, 5000, 'bob');
  await until(async () => (await W.cmd('core', { method: 'snapshot' })).ok, 30000, 'window core unlocked');
  await until(async () => { const d = await W.dump(); return d.level; }, 10000, 'window up');
  await sleep(3000);
  // let Nat's window show the rooms: wait until its snapshot has them
  await until(async () => { const s = (await W.cmd('core', { method: 'snapshot' })).ok; const c = s && s.communities.find(c => c.id === cid); return c && c.channels.length >= 3; }, 30000, 'window has the community');
  check('0 the native window is up (private KWin, silent engine, mock notifications, fake rune)', true, { nat: N.account.slice(0, 8) });

  const web_cards = p => web(p, `[...window.__megaRing.cards.values()].map(c => ({ name: c.name, direct: c.direct, room: c.room }))`);
  const natPeers = p => web(p, `(callPeople(${J(rooms.porch)}) || []).map(x => x.account)`);
  const inRoom = (p, gid, acct) => web(p, `(callPeople(${J(gid)}) || []).some(x => x.account === ${J(acct)})`);
  const joinWeb = async (p, gid, name) => { await web(p, `(window.__callUI.joinRoom(${J(gid)}, ${J(name)}), true)`); await p.page.waitFor(`window.__commonsCall.phase === 'connected' && window.__commonsCall.gid === ${J(gid)}`, 30000, p.name + ' connected'); };
  const leaveWeb = p => web(p, `(window.__commonsCall.phase !== 'idle' ? window.__commonsCall.leave() : 0, true)`);
  const natCall = async () => (await W.dump()).call;
  const natConnected = async gid => { const c = await natCall(); return c.phase === 'connected' && c.gid === gid; };

  // ---------------------------------------------------------------- A. a ring from the web shows as a card in the app
  await joinWeb(A, rooms.porch, 'porch');
  const t0 = Date.now();
  await web(A, `window.__megaRing.ringAll()`);
  const dA = await soft(until(async () => { const d = await W.dump(); return d.ring.cards.length ? d : null; }, 8000, 'card appears', 150));
  const secsA = (Date.now() - t0) / 1000;
  check('A1 a ring from Chromium shows a card in the app within 2 s', !dA.timeout && secsA <= 2.5, { secs: secsA, card: dA.ring && dA.ring.cards[0] });
  check('A2 the card names the caller and the room', dA.ring && dA.ring.cards[0] && dA.ring.cards[0].name === 'Alice' && dA.ring.cards[0].room === 'porch', dA.ring && dA.ring.cards[0]);
  await sleep(700);
  const f1 = await shot('ring-card'); check('A3 screenshot of the ring card', !!f1, f1 || 'no screenshot');
  await sleep(2600);
  const tones = lines(soundFile).length;
  check('A4 the ring sound is due on arrival and again every 2.6 s (recorded, never played: tests are silent)', tones >= 2, { tones });
  await W.cmd('accept');
  const joinedA = await soft(until(() => natConnected(rooms.porch), 30000, 'nat joined porch'));
  check('A5 joining from the card is the ordinary join: Nat is in the call and Alice sees Nat', joinedA === true && await soft(until(() => inRoom(A, rooms.porch, N.account), 20000, 'alice sees nat')) === true, await natCall());
  const dA2 = await W.dump();
  check('A6 the card is gone once answered, and no missed call is noted', dA2.ring.cards.length === 0 && dA2.ring.missed.length === 0, dA2.ring);
  const toneAfter = lines(soundFile).length; await sleep(3000);
  check('A7 no more ring sound after answering', lines(soundFile).length === toneAfter);

  // ---------------------------------------------------------------- B. one thing at a time
  await joinWeb(B, rooms.terrace, 'terrace');
  await web(B, `window.__megaRing.ringAll()`);
  await until(async () => (await W.dump()).ring.cards.length === 1, 8000, 'card for terrace');
  const dB = await W.dump();
  check('B1 a ring from another room shows while Nat is in a call', dB.ring.cards[0].name === 'Bob' && dB.ring.cards[0].room === 'terrace', dB.ring.cards[0]);
  await W.cmd('accept');
  const movedB = await soft(until(() => natConnected(rooms.terrace), 30000, 'nat moved to terrace'));
  check('B2 joining the second call ends the first (Nat is in terrace only)', movedB === true);
  check('B3 Alice no longer sees Nat in porch; Bob sees Nat in terrace', await soft(until(async () => !(await inRoom(A, rooms.porch, N.account)), 25000, 'nat left porch')) === true && await soft(until(() => inRoom(B, rooms.terrace, N.account), 20000, 'bob sees nat')) === true);
  await W.cmd('leave'); await until(async () => (await natCall()).phase === 'idle', 15000, 'nat idle');

  // ---------------------------------------------------------------- C. the caller gives up, decline, expiry
  const bRing = async on => { const has = await web(B, `window.__megaRing.outs.has('*')`); if (has !== on) { if (on) await sleep(5500); await web(B, `window.__megaRing.ringAll()`); } };   // the core refuses a second ring within 5 s
  await bRing(false); await bRing(true);
  console.log('  (C debug)', J(await web(B, `({ outs: [...window.__megaRing.outs.keys()], call: window.__commonsCall.phase, gid: window.__commonsCall.gid })`)), J((await W.dump()).ring));
  await until(async () => (await W.dump()).ring.cards.length >= 1, 8000, 'card C1');
  await bRing(false);                                                                       // the caller stops ringing
  const goneC = await soft(until(async () => (await W.dump()).ring.cards.length === 0, 8000, 'card goes when the caller ends it'));
  const dC = await W.dump();
  check('C1 the card goes when the caller ends the ring', goneC === true, dC.ring.cards);
  check('C2 a ring taken back is a missed call, with a note', dC.ring.missed.length === 1 && /Missed call from Bob in terrace/.test(dC.ring.missed[0]), dC.ring.missed);
  await bRing(true);
  await until(async () => (await W.dump()).ring.cards.length === 1, 8000, 'card C3');
  await W.cmd('decline');
  const dC3 = await W.dump();
  check('C3 declining takes the card away without a missed call', dC3.ring.cards.length === 0 && dC3.ring.missed.length === 1, dC3.ring);
  await sleep(2500);
  check('C4 a declined ring does not come back', (await W.dump()).ring.cards.length === 0);
  await bRing(false); await bRing(true);       // a new ring, left to run out
  await until(async () => (await W.dump()).ring.cards.length === 1, 8000, 'card C5');
  const t30 = Date.now();
  const goneE = await soft(until(async () => (await W.dump()).ring.cards.length === 0, 40000, 'card expires', 500));
  const secsE = (Date.now() - t30) / 1000;
  const dE = await W.dump();
  check('C5 an unanswered ring ends by itself after about 30 s, as a missed call', goneE === true && secsE > 20 && secsE < 36 && dE.ring.missed.length === 2, { secs: secsE, missed: dE.ring.missed });
  await leaveWeb(B); await leaveWeb(A);
  await until(async () => (await web(A, `window.__commonsCall.phase`)) === 'idle', 15000, 'alice idle');

  // ---------------------------------------------------------------- D. a ring from the app shows in the web
  await W.cmd('join', { name: 'porch' });
  await until(() => natConnected(rooms.porch), 30000, 'nat in porch');
  await W.cmd('ring_all');
  const t1 = Date.now();
  const cardsD = await soft(until(async () => { const a = await web_cards(A), b = await web_cards(B); return a.length && b.length ? { a, b } : null; }, 10000, 'cards in the web', 150));
  check('D1 a ring from the app shows as a card in Chromium (both people not in the call)', !cardsD.timeout && cardsD.a[0].name === 'Nat' && cardsD.b[0].name === 'Nat', { cardsD, secs: (Date.now() - t1) / 1000 });
  const dD = await W.dump();
  check('D2 the app remembers its ring until it ends', dD.ring.outs.includes('*'), dD.ring.outs);
  await joinWeb(A, rooms.porch, 'porch');
  await W.cmd('leave');
  const goneD = await soft(until(async () => (await web_cards(B)).length === 0, 10000, 'bob card ends when nat leaves', 300));
  check('D3 leaving the call ends the ring for everyone who did not answer', goneD === true, await web_cards(B));
  await leaveWeb(A); await sleep(1500);

  // ---------------------------------------------------------------- E. direct calls both ways
  await W.cmd('open', { name: 'Alice' }).catch(() => { });
  await W.cmd('dial', { peer: A.account, name: 'Alice' });
  const cardE = await soft(until(async () => { const c = await web_cards(A); return c.find(x => x.direct) ? c : null; }, 40000, 'alice gets the direct ring', 300));
  check('E1 a direct call from the app rings the person in Chromium', !cardE.timeout, cardE);
  const dE1 = await W.dump();
  check('E2 the app shows it is calling and is in the call room already', dE1.call.phase !== 'idle' && dE1.ring.dials.length === 1, { call: dE1.call.phase, dials: dE1.ring.dials });
  await web(A, `(window.__callUI.joinRoom([...window.__megaRing.cards.values()][0].gid, 'Nat'), true)`);
  const bothE = await soft(until(async () => (await W.dump()).call.phase === 'connected' && (await W.dump()).call.peers.length >= 2, 40000, 'both in the direct call', 400));
  check('E3 the direct call connects both ways', bothE && !bothE.timeout, (await W.dump()).call);
  await sleep(1500);
  check('E4 the pending call is forgotten once they came', (await W.dump()).ring.dials.length === 0);
  const dmRoom = (await W.dump()).call.gid;
  await W.cmd('leave'); await leaveWeb(A); await sleep(2500);
  // web -> app
  const t2 = Date.now();
  await web(A, `(async () => { await window.__megaDirect.call(${J(N.account)}, 'Nat'); return true; })()`);
  const cardF = await soft(until(async () => { const d = await W.dump(); return d.ring.cards.find(c => c.direct) ? d : null; }, 40000, 'nat gets the direct ring', 300));
  check('E5 a direct call from Chromium rings the app as "calling you"', !cardF.timeout && cardF.ring.cards[0].one_to_one === true, { secs: (Date.now() - t2) / 1000, card: cardF.ring && cardF.ring.cards[0] });
  const f2 = await shot('direct-ring-card');
  await W.cmd('accept');
  const bothF = await soft(until(async () => { const c = await natCall(); return c.phase === 'connected' && c.peers.length >= 2; }, 40000, 'direct web->app connects', 400));
  check('E6 answering connects it', bothF === true || (bothF && !bothF.timeout), await natCall());
  await W.cmd('leave'); await leaveWeb(A); await sleep(2500);
  // nobody answers
  const bobAcct = B.account;
  await W.cmd('dial', { peer: bobAcct, name: 'Bob' });
  const t3 = Date.now();
  const ended = await soft(until(async () => { const d = await W.dump(); return d.ring.notes.some(n => /did not answer/.test(n)) ? d : null; }, 70000, 'no answer', 500));
  check('E7 a call nobody answers ends after about 30 s and says so', !ended.timeout && (Date.now() - t3) / 1000 > 25 && (await natCall()).phase === 'idle', { secs: (Date.now() - t3) / 1000, notes: ended.ring && ended.ring.notes });
  // one thing at a time for calling out
  await W.cmd('join', { name: 'porch' }); await until(() => natConnected(rooms.porch), 30000, 'nat in porch again');
  await W.cmd('dial', { peer: A.account, name: 'Alice' }); await sleep(1500);
  const dF = await W.dump();
  check('E8 calling a person while in a call is refused (leave first, as the web says)', dF.ring.notes.some(n => /Leave your current call first/.test(n)) && dF.call.gid === rooms.porch, dF.ring.notes);
  await W.cmd('leave'); await until(async () => (await natCall()).phase === 'idle', 15000, 'nat idle E9');

  // ---------------------------------------------------------------- F. rune in the call bar
  await joinWeb(B, rooms.porch, 'porch');
  await W.cmd('join', { name: 'porch' });
  await until(() => natConnected(rooms.porch), 30000, 'nat in porch for rune');
  const dR = await soft(until(async () => { const d = await W.dump(); return d.rune.now ? d : null; }, 15000, 'rune shows'));
  check('F1 what rune plays shows in the call bar', !dR.timeout && dR.rune.now.title === 'Detective Arriving on the Scene' && dR.rune.now.playing === true, dR.rune && dR.rune.now);
  const dR2 = await soft(until(async () => { const d = await W.dump(); return d.rune.friends.length ? d : null; }, 25000, 'friend sharing', 500));
  check('F2 a person in the call who shares a Spotify song is offered for listening along', !dR2.timeout && dR2.rune.friends[0].name === 'Bob', dR2.rune && dR2.rune.friends);
  const f3 = await shot('call-bar-rune'); check('F3 screenshot of the call bar with rune', !!f3);
  await W.cmd('rune', { what: 'pause' });
  await until(() => natRune.cmds.some(c => c.type === 'pause'), 8000, 'pause reaches rune');
  const dP = await until(async () => { const d = await W.dump(); return d.rune.now && d.rune.now.playing === false ? d : null; }, 8000, 'paused shows');
  check('F4 pause reaches rune and the bar shows paused', natRune.state.status === 'paused' && !!dP);
  await W.cmd('rune', { what: 'resume' }); await until(() => natRune.cmds.some(c => c.type === 'resume'), 8000, 'resume');
  await W.cmd('rune', { what: 'next' });
  const dN = await soft(until(async () => { const d = await W.dump(); return d.rune.now && d.rune.now.title === 'Evening Jazz' ? d : null; }, 10000, 'next shows'));
  check('F5 next reaches rune and the new song shows', natRune.cmds.some(c => c.type === 'next') && !dN.timeout, natRune.cmds.map(c => c.type));
  await W.cmd('rune', { what: 'prev' }); await until(() => natRune.cmds.some(c => c.type === 'prev'), 8000, 'prev');
  const before = natRune.cmds.length;
  await W.cmd('along');
  const sp = await soft(until(() => natRune.cmds.find(c => c.type === 'play' && (c.trackIds || []).includes('sp:' + SP_ID)), 15000, 'listen along plays on rune'));
  check('F6 one click on listen along plays the friend\'s song on rune, in step (the core\'s listen-along)', !sp.timeout, sp);
  const dL = await soft(until(async () => { const d = await W.dump(); return d.rune.along ? d : null; }, 8000, 'along state'));
  check('F7 the bar then offers to stop listening along', !dL.timeout && dL.rune.along === B.account, dL.rune && dL.rune.along);
  await W.cmd('along');
  await until(async () => !(await W.dump()).rune.along, 8000, 'along off');
  check('F8 a second click leaves', ((await W.cmd('core', { method: 'listen_status' })).ok || {}).state === 'off');
  // rune goes away: the bar disappears
  natRune.close();
  const gone = await soft(until(async () => !(await W.dump()).rune.now, 25000, 'rune absent hides the bar', 500));
  check('F9 when rune stops answering the bar shows nothing of it', gone === true);
  await W.cmd('leave'); await leaveWeb(B);

  // ---------------------------------------------------------------- G. notifications with quick reply (mock service)
  await W.cmd('attend', { on: false });
  await W.cmd('home');
  await sleep(6500);   // the heartbeat tells the core nobody is looking
  const nBefore = lines(mockLog).length;
  await rpc(A.port, 'send_text', { gid: textGid, text: 'hello Nat, are you there?' });
  const note = await soft(until(() => { const l = lines(mockLog); return l.length > nBefore ? JSON.parse(l[l.length - 1]) : null; }, 20000, 'a notification for the message', 300));
  check('G1 a new message makes a desktop notification through org.freedesktop.Notifications', typeof note.timeout !== 'string' && note.category === 'im.received' && /Alice/.test(note.body), note);
  check('G2 it offers inline reply (the mock lists the capability) and Open', typeof note.timeout !== 'string' && note.actions.includes('inline-reply') && note.actions.includes('default'), note.actions);
  check('G3 the text stays out of the note unless the person allowed it', typeof note.timeout !== 'string' && !/are you there/.test(note.body + note.summary), note.body);
  const f4 = await shot('notify-window'); if (f4 && typeof note.timeout !== 'string') spawnSync('python3', [path.join(root, 'tools/lib/notifshot.py'), mockLog, f4, path.join(shots, 'notification-mock.png'), 'on my way']);
  mockSay(`reply ${note.id} on my way`);
  const gotReply = await soft(until(async () => { const m = await rpc(A.port, 'messages', { gid: textGid, limit: 20, around: '' }); return (m.messages || []).some(x => x.text === 'on my way' && !x.mine) ? true : null; }, 20000, 'the reply reaches the room', 400));
  check('G4 a quick reply typed into the note is sent into that room through the core', gotReply === true);
  const f5 = await shot('after-quick-reply');
  // a click opens the room
  await W.cmd('home');
  mockSay(`action ${note.id} default`);
  const opened = await soft(until(async () => { const d = await W.dump(); return d.gid === textGid && d.level === 'Chat' ? d : null; }, 10000, 'Open opens the room'));
  check('G5 clicking Open brings the window forward on the room', !opened.timeout, opened.timeout || opened.gid);
  // modes (the window goes back to the home screen: a room that is open is read at once and never counts as unread)
  await W.cmd('home'); await sleep(1500);
  await W.cmd('core', { method: 'notify_set', args: { mode: 'off' } });
  const n2 = lines(mockLog).length;
  await rpc(A.port, 'send_text', { gid: textGid, text: 'nobody hears this' });
  await sleep(9000);
  check('G6 notification mode "off" shows nothing', lines(mockLog).length === n2);
  await W.cmd('core', { method: 'notify_set', args: { mode: 'mentions' } });
  await rpc(A.port, 'send_text', { gid: textGid, text: 'plain message, no mention' });
  await sleep(9000);
  check('G7 "mentions only" skips an ordinary message', lines(mockLog).length === n2);
  await rpc(A.port, 'send_text', { gid: textGid, text: 'hey @Nat this one is for you' });
  const mention = await soft(until(() => lines(mockLog).length > n2, 20000, 'mention notifies', 400));
  check('G8 ...and shows one that names Nat', mention === true, lines(mockLog).slice(n2).map(l => JSON.parse(l).body));
  await W.cmd('core', { method: 'notify_set', args: { mode: 'all' } });
  // a ring while nobody looks
  const n3 = lines(mockLog).length;
  await joinWeb(A, rooms.porch, 'porch'); await web(A, `window.__megaRing.ringAll()`);
  const ringNote = await soft(until(() => { const l = lines(mockLog); return l.length > n3 ? l.slice(n3).map(x => JSON.parse(x)).find(x => /calling/.test(x.summary)) : null; }, 15000, 'ring notification', 300));
  check('G9 a ring while the window is not looked at also notifies, with only Open', typeof ringNote.timeout !== 'string' && !ringNote.actions.includes('inline-reply'), ringNote);
  await web(A, `window.__megaRing.ringAll()`); await leaveWeb(A);

  await W.cmd('quit').catch(() => { });
} catch (e) { res.error = String(e.stack || e); console.log('ERROR', res.error); failed++; }
res.failed = failed;
fs.writeFileSync(path.join(out, 'result.json'), JSON.stringify(res, null, 1));
for (const f of fakes) { try { f.close(); } catch { } }
console.log(failed ? `${failed} FAILED` : 'ALL PASSED', '->', path.join(out, 'result.json'));
stopAll(); process.exit(failed ? 1 : 0);
