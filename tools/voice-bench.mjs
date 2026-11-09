// Voice measurements on the real call engine: real relay, real LiveKit, real Rust core for keys, real E2EE worker, in a headless
// browser with synthetic audio only (no microphone, no speakers, nothing audible). Everything is on 127.0.0.1.
//   VB_SUITES=join,latency,recover,ice,plain   default: all
//   VB_RIG=local|hosted                         hosted: go-online.sh --local-test + TLS terminator, media forced through TURN/TLS
//   COMMONS_TEST_CHROME=/opt/brave-bin/brave    browser under test; COMMONS_TEST_PORT_OFFSET to run beside another rig
//   systemd-run --user --scope -q -p MemoryMax=8G node tools/voice-bench.mjs [outdir]
// What a loopback rig cannot show: real internet delay, loss and jitter, NAT behaviour, a real IP change, real capture and playout
// device latency. The numbers measure this software path only; see the report for what is and is not covered.
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { startServers, startBridge, startChrome, openPage, rpc, sleep, stopAll, lkAdmin, operatorToken, port, run, root, LK_PORT, CHROME, FAKE_DISPLAY } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';
import { voiceProbe } from './lib/voice-probe.mjs';
import { startHost, forceRelay, relayPath } from './lib/host.mjs';

const out = process.argv[2] || '/tmp/commons-voice-bench';
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const RIG = process.env.VB_RIG || 'local';
const SUITES = new Set((process.env.VB_SUITES || 'ice,join,latency,plain,recover').split(','));
const N_JOIN = Number(process.env.VB_JOINS || 12), N_REC = Number(process.env.VB_RECOVER_N || 2), PULSES = Number(process.env.VB_PULSES || 40);
const J = JSON.stringify;
const UMD = fs.readFileSync(path.join(root, 'app/ui/vendor/livekit-client.umd.js'), 'utf8');
const stats = xs => { const v = xs.filter(x => typeof x === 'number' && isFinite(x)).sort((a, b) => a - b); if (!v.length) return { n: 0 }; const q = p => v[Math.min(v.length - 1, Math.floor(p * v.length))]; return { n: v.length, min: +v[0].toFixed(1), median: +q(0.5).toFixed(1), p95: +q(0.95).toFixed(1), max: +v[v.length - 1].toFixed(1), mean: +(v.reduce((a, b) => a + b, 0) / v.length).toFixed(1) }; };
async function until(fn, ms = 20000, what = 'condition') { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(50); } }
const R = { rig: RIG, browser: CHROME, when: new Date().toISOString(), join: null, latency: null, plain: null, recover: {}, ice: null };

let world, lk, host;
const people = {};
const page = n => people[n].page;
const call = (n, expr) => page(n).ev(`(async()=>{ const c = window.__commonsCall; return ${expr}; })()`);
const vb = (n, expr) => page(n).ev(`(async()=>{ const vb = window.__vb, c = window.__commonsCall; return ${expr}; })()`);

async function setupLocal() {
  lk = await startServers(out, { relayTtl: 900 });
  const ui = process.env.VB_UI ? { alice: process.env.VB_UI, bob: process.env.VB_UI } : {};   // measure another version of app/ui
  world = await makeWorld(out, ['alice', 'bob'], { extraChrome: [], ui });
  if (process.env.VB_ONE_BROWSER !== '1') world.people.bob.chrome = await startChrome(out, 'bob', port(29450), 500, []);   // its own process: a "sleeping" client can be stopped alone
  for (const n of ['alice', 'bob']) Object.assign(people, { [n]: world.people[n] });
  for (const n of ['alice', 'bob']) { const p = people[n]; p.page = await openPage(p.chrome, `http://127.0.0.1:${p.port}/`, n, { before: voiceProbe(n === 'alice' ? 500 : 700) }); await p.page.waitFor(`!!window.__commonsCall && !document.querySelector("#app").hidden && !!S.gid`, 30000, n + ' app ready'); }
  people.gid = world.gid;
}

async function setupHosted() {
  host = await startHost(out);
  const { NAME, PUB, PASS, RELAY } = host;
  const bridge = (name, p) => run(path.join(root, 'target/debug/dev_bridge'), [String(p), process.env.VB_UI || path.join(root, 'app/ui')], { COMMONS_DATA: path.join(out, name), COMMONS_RELAY: '', COMMONS_FAST_KDF: '1' }, path.join(out, name + '.log'));
  const A = port(26701), B = port(26702);
  bridge('alice', A); bridge('bob', B); await sleep(1500);
  const link = JSON.parse(Buffer.from(host.web.split('#relay/')[1], 'base64url'));
  await rpc(A, 'create_account', { name: 'Alice', passphrase: PASS, relay: RELAY, invite: link.secret });
  const cid = (await rpc(A, 'create_community', { name: 'Friends' })).id;
  const inv = (await rpc(A, 'create_invite', { cid, uses: 1, ttl: 3600 })).link;
  const p2 = await rpc(B, 'parse_link', { link: inv });
  await rpc(B, 'create_account', { name: 'Bob', passphrase: PASS, relay: p2.relay, invite: p2.secret });
  await rpc(B, 'join', { link: p2.link });
  let gid;
  await until(async () => { const s = await rpc(B, 'snapshot'); const c = s.communities.find(x => x.id === cid); const ch = c && c.channels.find(x => x.joined && x.status === 'ok'); gid = ch && ch.gid; return gid; }, 40000, 'bob joined');
  const chA = await startChrome(out, 'alice', port(26800), 500, ['--ignore-certificate-errors']), chB = await startChrome(out, 'bob', port(26801), 500, ['--ignore-certificate-errors']);
  const force = process.env.VB_FORCE_RELAY === '0' ? '' : forceRelay;
  people.alice = { port: A, chrome: chA, page: await openPage(chA, `http://127.0.0.1:${A}/`, 'alice', { before: force + voiceProbe(500) }) };
  people.bob = { port: B, chrome: chB, page: await openPage(chB, `http://127.0.0.1:${B}/`, 'bob', { before: force + voiceProbe(700) }) };
  for (const n of ['alice', 'bob']) await page(n).waitFor(`!!window.__commonsCall && !document.querySelector("#app").hidden && !!S.gid`, 30000, n + ' app ready');
  people.gid = gid;
}

// ---- measurement helpers
const dev = async n => people[n].device || (people[n].device = (await rpc(people[n].port, 'snapshot')).me.device);
async function joinCall(n, opts = {}) { await call(n, `(window.__vb.instrument(c), c.join(S.gid, 'general'))`); }
async function leave(n) { await call(n, `c.leave()`); }
const secs = ms => (ms / 1000).toFixed(2) + ' s';

async function ensureCalled() {
  for (const n of ['alice', 'bob']) { if ((await call(n, `c.phase`)) !== 'connected') { await call(n, `c.phase === 'idle' ? (window.__vb.instrument(c), c.join(S.gid, 'general')) : true`); await until(() => call(n, `c.phase === 'connected'`), 30000, n + ' connected'); } }
  const [a, b] = [await dev('alice'), await dev('bob')];
  await until(async () => (await vb('alice', `vb.firstAudio(${J(b)})`)) && (await vb('bob', `vb.firstAudio(${J(a)})`)), 30000, 'both hear each other');
  return { a, b };
}

// ---------------------------------------------------------------- A. ICE paths and the numbers WebRTC itself reports
async function suiteIce() {
  await vb('alice', `vb.setMode('tone')`); await vb('bob', `vb.setMode('tone')`);
  await joinCall('alice'); await joinCall('bob');
  const { a, b } = await ensureCalled();
  await sleep(4000);
  R.ice = {};
  for (const n of ['alice', 'bob']) {
    const s = await vb(n, `vb.stats()`); const marks = await vb(n, `vb.marks.filter(m => /candidate|ice-|pc-|pc:/.test(m[1])).slice(0, 80)`);
    R.ice[n] = { pcs: s, marks };
    console.log(`\n=== ICE and audio settings, ${n}`);
    for (const pc of s) {
      console.log(' pc state', pc.state, pc.ice, 'cfg', J(pc.cfg));
      console.log('  local candidates:', pc.local.map(r => r.slice(1, 6).join(' ')).join(' | ') || '(none)');
      console.log('  remote candidates:', pc.remote.map(r => r.slice(1).join(' ')).join(' | ') || '(none)');
      console.log('  pairs:', pc.pairs.map(p => `${p.local} -> ${p.remote} ${p.state}${p.nominated ? ' nom' : ''} rtt ${p.rtt}`).join('\n         ') || '(none)');
      console.log('  selected:', J(pc.selected));
      console.log('  local SDP:', pc.localSdp.join(' ; '));
      console.log('  remote SDP:', pc.remoteSdp.join(' ; '));
      console.log('  sender:', J(pc.encodings), ' receivers:', J(pc.receivers));
      console.log('  rtp:', J(pc.rtp));
      if (pc.errors.length) console.log('  ice errors:', J(pc.errors));
    }
  }
  await leave('bob'); await leave('alice'); await sleep(1500);
}

// ---------------------------------------------------------------- B. join: press to connected, press to first audio, both directions
async function suiteJoin() {
  const { } = {};
  await vb('alice', `vb.setMode('tone')`); await vb('bob', `vb.setMode('tone')`);
  await joinCall('alice');
  await until(() => call('alice', `c.phase === 'connected'`), 30000, 'alice connected');
  await sleep(1000);
  const [a, b] = [await dev('alice'), await dev('bob')];
  const rows = [];
  for (let i = 0; i < N_JOIN; i++) {
    await vb('bob', `vb.clearTaps()`); await vb('alice', `vb.clearTaps()`);
    const t0 = await vb('bob', `vb.instrument(c), (vb.t0 = performance.timeOrigin + performance.now(), vb.mark('press'), vb.t0)`);
    let joinErr = null;
    await call('bob', `c.join(S.gid, 'general')`).catch(e => { joinErr = e.message; });
    let bobFirst = 0, aliceFirst = 0;
    try {
      await until(async () => { bobFirst = bobFirst || await vb('bob', `vb.firstAudio(${J(a)})`); aliceFirst = aliceFirst || await vb('alice', `vb.firstAudio(${J(b)})`); return bobFirst && aliceFirst; }, 15000, 'first audio');
    } catch { }
    const marks = await vb('bob', `vb.marks`);
    const at = (k, v, pick = 'first') => { const m = marks.filter(x => x[1] === k && (v === undefined || x[2] === v)); return m.length ? (pick === 'last' ? m[m.length - 1][0] : m[0][0]) - t0 : null; };
    const phaseConnected = marks.find(m => m[1] === 'phase' && m[2] === 'connected');
    rows.push({
      i, err: joinErr,
      creds: at('creds-end'), signal: at('room:SignalConnected'), pcConnected: at('pc-state', 'connected'), roomConnected: at('room:Connected'),
      phaseConnected: phaseConnected ? phaseConnected[0] - t0 : null, micPublished: at('mic-publish-end'), localTrack: at('room:LocalTrackPublished'), subscribed: at('subscribed'),
      firstAudioHere: bobFirst ? bobFirst - t0 : null, firstAudioOther: aliceFirst ? aliceFirst - t0 : null, marks: marks.map(m => [m[0] - t0, m[1], m[2]]),
    });
    console.log(`join ${i}:`, J({ ...rows[i], marks: undefined }));
    await call('bob', `c.leave()`); await sleep(1500);
  }
  await leave('alice'); await sleep(1000);
  const col = k => stats(rows.map(r => r[k]));
  R.join = { summary: Object.fromEntries(['creds', 'signal', 'pcConnected', 'roomConnected', 'phaseConnected', 'micPublished', 'subscribed', 'firstAudioHere', 'firstAudioOther'].map(k => [k, col(k)])), rows };
  console.log('\n=== join timeline (ms from press, median/p95/min/max, n)');
  for (const [k, v] of Object.entries(R.join.summary)) console.log(`  ${k.padEnd(16)} median ${v.median}  p95 ${v.p95}  min ${v.min}  max ${v.max}  n ${v.n}`);
}

// ---------------------------------------------------------------- C. mouth-to-ear through the encrypted path
function matchLatency(sent, recv) {
  const out = [];
  for (const s of sent) { const r = recv.find(x => x > s - 5 && x < s + 800); if (r !== undefined) out.push(r - s); }
  return out;
}
async function pulseRun(n1, n2, id1, mode, seconds) {
  await vb(n1, `vb.setMode(${J(mode)})`);
  await vb(n2, `vb.setMode('bursts')`).catch(() => { });
  await vb(n1, `vb.taps.forEach(t => { if (t.id === 'self') t.onsets = []; }), true`);
  await vb(n2, `vb.taps.forEach(t => { t.onsets = []; }), true`);
  await sleep(600);
  const statsStart = await vb(n2, `vb.stats()`);
  await sleep(seconds * 1000);
  const [sent, recv] = [await vb(n1, `vb.onsets('self')`), await vb(n2, `vb.onsets(${J(id1)})`)];
  const statsEnd = await vb(n2, `vb.stats()`);
  const sendStats = await vb(n1, `vb.stats()`);
  const lat = matchLatency(sent, recv);
  const inA = statsStart.flatMap(p => p.rtp).find(r => r.dir === 'in' && r.packets), inB = statsEnd.flatMap(p => p.rtp).find(r => r.dir === 'in' && r.packets && r.mid === (inA ? inA.mid : r.mid));
  const emitted = inB && inA ? (inB.jbEmitted - inA.jbEmitted) : null;
  const jb = emitted ? { jbDelayAvgMs: +(1000 * (inB.jbDelay - inA.jbDelay) / emitted).toFixed(2), jbTargetAvgMs: +(1000 * (inB.jbTarget - inA.jbTarget) / emitted).toFixed(2), jbMinAvgMs: +(1000 * (inB.jbMin - inA.jbMin) / emitted).toFixed(2), packets: inB.packets - inA.packets, lost: inB.lost - inA.lost, concealedRatio: +((inB.concealed - inA.concealed) / Math.max(1, inB.total - inA.total)).toFixed(4), codec: inB.codec, fmtp: inB.fmtp, fecRecv: inB.fec, fecDiscarded: inB.fecDiscard } : null;
  const out = sendStats.flatMap(p => p.rtp).find(r => r.dir === 'out' && r.packets);
  return { sent: sent.length, recv: recv.length, matched: lat.length, ms: stats(lat), jb, senderOut: out, raw: lat };
}
async function suiteLatency() {
  await joinCall('alice'); await joinCall('bob');
  const { a, b } = await ensureCalled();
  await sleep(2500);
  if (process.env.VB_JBT !== undefined) console.log('jitterBufferTarget set on receivers:', await vb('alice', `vb.setJbTarget(${Number(process.env.VB_JBT)})`), await vb('bob', `vb.setJbTarget(${Number(process.env.VB_JBT)})`));
  // Calibration: the same probe on a track looped back inside the sender's own page, no network, no codec. It covers the probe's own timing error.
  const local = await vb('alice', `(async () => { const gen = vb.gens[vb.gens.length - 1]; vb.setMode('bursts'); const self = [...vb.taps].reverse().find(t => t.id === 'self'); self.onsets = []; const mirror = vb.tap('loopback', self.sid ? gen.dest.stream.getAudioTracks()[0].clone() : gen.dest.stream.getAudioTracks()[0]); await new Promise(r => setTimeout(r, 12000)); const ons = { sent: self.onsets.slice(), recv: mirror.onsets.slice() }; clearInterval(mirror.timer); return ons; })()`);
  const loopLat = matchLatency(local.sent, local.recv);
  console.log('\nprobe self-loopback (no WebRTC; both ends of the probe):', J(stats(loopLat)), 'bursts', local.sent.length, local.recv.length);
  R.latency = { calibration: { ms: stats(loopLat) } };
  await vb('alice', `vb.setMode('bursts')`); await vb('bob', `vb.setMode('bursts')`);
  await sleep(1000);
  for (const [dir, n1, n2, id] of [['alice->bob', 'alice', 'bob', a], ['bob->alice', 'bob', 'alice', b]]) {
    for (const mode of ['bursts', 'talk']) {
      const r = await pulseRun(n1, n2, id, mode, Math.max(10, PULSES));
      R.latency[dir + ' ' + mode] = r;
      const line = { sent: r.sent, recv: r.recv, matched: r.matched, ms: r.ms, jb: r.jb };
      console.log(`latency ${dir} ${mode}:`, J(line));
      if (r.senderOut) console.log('   sender outbound', J({ packets: r.senderOut.packets, bytes: r.senderOut.bytes, retrans: r.senderOut.retrans }));
    }
  }
  await vb('alice', `vb.setMode('tone')`); await vb('bob', `vb.setMode('tone')`);
  await leave('bob'); await leave('alice'); await sleep(1500);
}

// ---------------------------------------------------------------- D. the same pulses without E2EE: what the frame worker costs
async function suitePlain() {
  const roomName = (await rpc(people.alice.port, 'call_token', { gid: people.gid })).room + '-plain';
  const mk = async (name, hz) => {
    const p = await openPage(people.alice.chrome, `http://127.0.0.1:${people.alice.port}/vendor/README.txt`, name, { before: voiceProbe(hz) });
    await p.ev(UMD);
    const tok = operatorToken({ room: roomName, roomJoin: true, canPublish: true, canSubscribe: true }, name);
    await p.ev(`(async()=>{ const r = window.__r = new LivekitClient.Room({ dynacast: true }); r.on('trackSubscribed', (t, pub, part) => { if (t.kind === 'audio') { const el = t.attach(); el.muted = true; document.body.appendChild(el); window.__vb.tap(part.identity, t.mediaStreamTrack); } }); await r.connect('ws://127.0.0.1:${LK_PORT}', ${J(tok)}); await r.localParticipant.setMicrophoneEnabled(true, undefined, { red: false, dtx: true }); return true; })()`);
    return p;
  };
  const [pa, pb] = [await mk('plainA', 500), await mk('plainB', 700)];
  await sleep(2000);
  const run_ = async (mode) => {
    await pa.ev(`window.__vb.setMode(${J(mode)})`); await pb.ev(`window.__vb.setMode('bursts')`);
    await pa.ev(`(window.__vb.taps.forEach(t => t.onsets = []), true)`); await pb.ev(`(window.__vb.taps.forEach(t => t.onsets = []), true)`);
    await sleep(Math.max(10, PULSES) * 1000);
    const [s, r] = [await pa.ev(`window.__vb.onsets('self')`), await pb.ev(`window.__vb.onsets('plainA')`)];
    return { sent: s.length, recv: r.length, ms: stats(matchLatency(s, r)) };
  };
  const res = { bursts: await run_('bursts'), talk: await run_('talk') };
  // what one AES-GCM frame costs in this browser (the operation the worker does per frame)
  const micro = await pa.ev(`(async()=>{ const k = await crypto.subtle.importKey('raw', new Uint8Array(32), 'AES-GCM', false, ['encrypt','decrypt']); const iv = new Uint8Array(12), d = new Uint8Array(120); const t0 = performance.now(); let n = 0; for (; performance.now() - t0 < 500; n++) { await crypto.subtle.encrypt({ name: 'AES-GCM', iv, additionalData: new Uint8Array(1) }, k, d); } return { encryptsPerSec: n * 2, usPerFrame: 500000 / n }; })()`);
  await pa.ev(`window.__r.disconnect().then(() => true)`); await pb.ev(`window.__r.disconnect().then(() => true)`);
  R.plain = { noE2ee: res, aesGcmMicro: micro };
  console.log('\nno-E2EE control:', J(res), '\nAES-GCM per frame:', J(micro));
}

// ---------------------------------------------------------------- E. recovery
const descendants = pid => { const out = [pid]; const scan = p => { let kids; try { kids = fs.readFileSync(`/proc/${p}/task/${p}/children`, 'utf8').trim().split(/\s+/).filter(Boolean).map(Number); } catch { return; } for (const k of kids) { out.push(k); scan(k); } }; scan(pid); return out; };
const freeze = (pid, on) => { for (const p of descendants(pid)) { try { process.kill(p, on ? 'SIGSTOP' : 'SIGCONT'); } catch { } } };

function analyse(levels, tEvent, tEnd, thr = 0.01) {
  // levels [[t, rms], ...]: find first silent bin at or after the event; recovery is the first bin after that with sound that stays
  const pts = levels.filter(x => x[0] >= tEvent - 1000);
  const after = pts.filter(x => x[0] >= tEvent);
  const firstSilent = after.find(x => x[1] < thr);
  if (!firstSilent) return { gap: 0, recoveryMs: 0, note: 'no audible gap' };
  let lastSilent = firstSilent[0];
  for (const x of after) if (x[1] < thr) lastSilent = x[0];
  const backFirst = after.find(x => x[0] > firstSilent[0] && x[1] >= thr);
  const backStable = after.find(x => x[0] > lastSilent && x[1] >= thr);
  return { firstSilentAfterMs: firstSilent[0] - tEvent, firstBackMs: backFirst ? backFirst[0] - tEvent : null, recoveryMs: backStable ? backStable[0] - tEvent : null, silentMs: backStable ? backStable[0] - firstSilent[0] : null, stable: !!backStable };
}

async function recoverOnce(name, trigger, { waitMs = 90000, downMs = 0 } = {}) {
  const { a, b } = await ensureCalled();
  await sleep(1500);
  await vb('alice', `vb.clearLevels()`); await vb('bob', `vb.clearLevels()`);
  await sleep(500);
  const tE = Date.now();
  await trigger();
  await sleep(2000);   // the old track's decoder keeps sounding for a moment; judge recovery only after the drop has had time to show
  const res = { name };
  try {
    await until(async () => {
      const [pa, pb] = await Promise.all(['alice', 'bob'].map(n => call(n, `c.phase`)));
      const la = await vb('bob', `vb.levels(${J(a)}, ${tE}, ${tE + 300000}).slice(-4).map(x => x[1])`);
      const lb = await vb('alice', `vb.levels(${J(b)}, ${tE}, ${tE + 300000}).slice(-4).map(x => x[1])`);
      const heard = x => x.length >= 3 && x.every(v => v >= 0.01);
      return pa === 'connected' && pb === 'connected' && heard(la) && heard(lb);
    }, waitMs, name + ': audio back both ways');
    res.healed = true;
  } catch (e) { res.healed = false; res.err = e.message; }
  await sleep(1200);
  const tEnd = Date.now();
  const lvA = await vb('bob', `vb.levels(${J(a)}, ${tE - 500}, ${tEnd})`), lvB = await vb('alice', `vb.levels(${J(b)}, ${tE - 500}, ${tEnd})`);
  res.bobHearsAlice = analyse(lvA, tE, tEnd); res.aliceHearsBob = analyse(lvB, tE, tEnd);
  // A rejoin makes a new track: its first sound is the real end of the gap (the old track's decoder keeps concealing for a while,
  // which levels alone would count as sound). For an in-place resume there is no new track and the level analysis stands.
  const fresh = async (n, id) => { const ts = await vb(n, `vb.taps.filter(t => t.id === ${J(id)} && t.created >= ${tE} && t.first).map(t => t.first)`); return ts.length ? Math.round(Math.min(...ts)) - tE : null; };
  res.bobHearsAlice.newTrackMs = await fresh('bob', a); res.aliceHearsBob.newTrackMs = await fresh('alice', b);
  res.phases = { alice: await call('alice', `c.phase`), bob: await call('bob', `c.phase`) };
  res.events = { alice: await call('alice', `c.events.slice(-6).map(e => e.msg)`), bob: await call('bob', `c.events.slice(-8).map(e => e.msg)`) };
  res.bobError = await call('bob', `c.error`);
  res.e2ee = { alice: await call('alice', `c.room && c.room.isE2EEEnabled`), bob: await call('bob', `c.room && c.room.isE2EEEnabled`) };
  res.bobMarks = (await vb('bob', `vb.marks.filter(m => m[0] >= ${tE} - 100 && !/candidate/.test(m[1]))`)).map(m => [m[0] - tE, m[1], m[2]]);
  console.log(`recover ${name}: healed ${res.healed} bob<-alice ${J(res.bobHearsAlice)} alice<-bob ${J(res.aliceHearsBob)} phases ${J(res.phases)} ${res.err || ''}`);
  return res;
}

async function suiteRecover() {
  await vb('alice', `vb.setMode('tone')`); await vb('bob', `vb.setMode('tone')`);
  await joinCall('alice'); await joinCall('bob');
  await ensureCalled();
  const scenarios = {
    'signal drop, resume works': () => call('bob', `c.room.simulateScenario('signal-reconnect').then(() => true)`),
    'drop, resume fails (full rejoin)': () => call('bob', `c.room.simulateScenario('resume-reconnect').then(() => true)`),
    'path change (force-tcp)': () => call('bob', `c.room.simulateScenario('force-tcp').then(() => true)`),
  };
  if (RIG === 'local') {
    scenarios['server restart (SIGKILL, back in 1 s)'] = async () => { await lk.stopLivekit(); await sleep(500); lk.startLivekit(); };
    scenarios['server restart (SIGKILL, back in 8 s)'] = async () => { await lk.stopLivekit(); await sleep(7500); lk.startLivekit(); };
    scenarios['server stall 8 s (SIGSTOP, path blackhole)'] = async () => { const pid = lk.lk.pid; process.kill(pid, 'SIGSTOP'); await sleep(8000); process.kill(pid, 'SIGCONT'); };
  }
  scenarios['client sleep 15 s (SIGSTOP browser)'] = async () => { const pid = people.bob.chrome.proc.pid; freeze(pid, true); await sleep(15000); freeze(pid, false); };
  scenarios['client sleep 60 s (SIGSTOP browser)'] = async () => { const pid = people.bob.chrome.proc.pid; freeze(pid, true); await sleep(60000); freeze(pid, false); };
  const only = process.env.VB_RECOVER_ONLY;
  for (const [name, fn] of Object.entries(scenarios)) {
    if (only && !only.split('|').some(x => name.includes(x))) continue;
    R.recover[name] = [];
    for (let i = 0; i < N_REC; i++) {
      try { R.recover[name].push(await recoverOnce(name + ' #' + i, fn, { waitMs: name.includes('60 s') ? 150000 : 90000 })); } catch (e) { R.recover[name].push({ err: String(e) }); console.log('recover error', name, e.message); }
      await sleep(1500);
    }
  }
  await leave('bob').catch(() => { }); await leave('alice').catch(() => { }); await sleep(800);
}

let code = 0;
try {
  if (RIG === 'hosted') await setupHosted(); else await setupLocal();
  console.log('browser:', await page('alice').ev(`navigator.userAgent`), ' rig', RIG);
  if (SUITES.has('ice')) await suiteIce();
  if (SUITES.has('join')) await suiteJoin();
  if (SUITES.has('latency')) await suiteLatency();
  if (SUITES.has('plain') && RIG === 'local') await suitePlain();
  if (SUITES.has('recover')) await suiteRecover();
} catch (e) { console.error('BENCH ABORTED:', e); code = 1; }
finally {
  fs.writeFileSync(path.join(out, 'results.json'), JSON.stringify(R, null, 1));
  for (const n of ['alice', 'bob']) { const p = people[n]; if (p && p.page && p.page.console.length) fs.writeFileSync(path.join(out, `console-${n}.txt`), p.page.console.join('\n')); }
  if (host) host.offline();
  stopAll();
  console.log('results in', out);
  process.exitCode = code;
  setTimeout(() => process.exit(code), 300);   // nothing may keep the bench alive after its report (the hosted rig's TLS stand-in would)
}
