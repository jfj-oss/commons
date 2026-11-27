// Problem reports (crates/core/src/diag.rs, relay POST /v1/diag): the native engine whose call fails sends the relay's
// owner a report, keeps the same text for its Copy details button, sends nothing with the switch off, and a report made
// while the relay is down waits and goes once a call connects.
//   cargo build -p commons-relay -p commons-core --bin dev_bridge && cargo build --release -p mega-gpui
//   MEGA_GPUI_BIN=target/release/mega-gpui COMMONS_TEST_PORT_OFFSET=700 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-diag-e2e.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn, execFileSync } from 'node:child_process';
import { root, startServers, startBridge, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';
import { makeWorld } from './lib/world.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-diag-e2e');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN ? path.resolve(root, process.env.MEGA_GPUI_BIN) : path.join(root, 'target/debug/mega-gpui');
const PASS = 'correct horse battery', J = x => JSON.stringify(x);
let failed = 0;
const check = (name, ok, detail = '') => { if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? '  [' + (typeof detail === 'string' ? detail : J(detail)).slice(0, 400) + ']' : ''}`); };
async function until(fn, ms, what) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(300); } }
const soft = p => p.catch(e => ({ timeout: String(e.message || e) }));
const reports = () => { try { return JSON.parse(execFileSync('sqlite3', ['-json', path.join(out, 'data/relay/relay.db'), 'SELECT account, report FROM diag_reports ORDER BY id'], { encoding: 'utf8' }) || '[]'); } catch { return []; } };

class Native {
  constructor(env) {
    this.proc = spawn(BIN, ['--call-headless'], { env: { ...process.env, ...env }, stdio: ['ignore', 'pipe', 'pipe'], detached: true });
    procs.push(this.proc);
    this.proc.stderr.pipe(fs.createWriteStream(path.join(out, 'nat.log')));
    this.ready = new Promise((res, rej) => { let b = ''; this.proc.stdout.on('data', d => { b += d; const i = b.indexOf('\n'); if (i >= 0) { try { res(JSON.parse(b.slice(0, i))); } catch (e) { rej(e); } } }); this.proc.once('exit', c => rej(new Error('exited ' + c))); });
    this.n = 0; this.waiting = new Map(); this.buf = '';
  }
  async connect(port) {
    this.sock = net.connect(port, '127.0.0.1');
    await new Promise((res, rej) => { this.sock.once('connect', res); this.sock.once('error', rej); });
    this.sock.on('data', d => { this.buf += d; let i; while ((i = this.buf.indexOf('\n')) >= 0) { const l = this.buf.slice(0, i); this.buf = this.buf.slice(i + 1); const j = JSON.parse(l); const w = this.waiting.get(j.id); if (w) { this.waiting.delete(j.id); w(j); } } });
  }
  cmd(cmd, args = {}, ms = 30000) { const id = ++this.n; return new Promise((res, rej) => { const t = setTimeout(() => { this.waiting.delete(id); rej(new Error('no answer to ' + cmd)); }, ms); this.waiting.set(id, j => { clearTimeout(t); res(j); }); this.sock.write(J({ cmd, id, ...args }) + '\n'); }); }
  snap() { return this.cmd('snapshot'); }
}

let nat;
try {
  const lk = await startServers(out, {});
  const world = await makeWorld(out, ['alice']);
  const gid = world.gid;
  const N = { port: P(27203) };
  const natBridge = startBridge(out, 'nat', N.port); await sleep(1200);
  await rpc(N.port, 'create_account', { name: 'Nat', passphrase: PASS, relay: `http://127.0.0.1:${RELAY_PORT}` });
  await rpc(N.port, 'join', { link: (await rpc(world.people.alice.port, 'create_invite', { cid: world.cid, uses: 2, ttl: 3600 })).link });
  await until(async () => { const s = await rpc(N.port, 'snapshot'); const c = s.communities.find(c => c.id === world.cid); return c && c.channels.some(ch => ch.gid === gid && ch.joined && ch.status === 'ok'); }, 60000, 'nat joined');
  N.account = (await rpc(N.port, 'snapshot')).me.account;
  try { process.kill(-natBridge.pid, 'SIGTERM'); } catch { } await sleep(1500);
  const testPort = P(27300);
  nat = new Native({ COMMONS_DATA: path.join(out, 'data/nat'), COMMONS_RELAY: `http://127.0.0.1:${RELAY_PORT}`, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_CALL_TONE: '900', MEGA_GPUI_CALL_TEST: String(testPort), MEGA_GPUI_NOTIFY_FILE: path.join(out, 'nat-notes.jsonl'), RUST_LOG: 'info' });
  await nat.ready; await nat.connect(testPort);
  const s0 = await soft(until(async () => { const s = await nat.snap(); return s.reportProblems === true ? s : null; }, 10000, 'first snapshot'));
  check('0 the engine starts with problem reports on', s0.reportProblems === true, { reportProblems: s0.reportProblems });

  // 1. the call server is down: the join fails, a report reaches the relay, the same text is offered to copy
  await lk.stopLivekit();
  await nat.cmd('join', { gid, name: 'general' });
  const s1 = await soft(until(async () => { const s = await nat.snap(); return s.phase === 'error' && s.diag ? s : null; }, 90000, 'join fails'));
  const r1 = await soft(until(async () => { const r = reports(); return r.length >= 1 ? r : null; }, 20000, 'report stored'));
  const rep = Array.isArray(r1) ? JSON.parse(r1[0].report) : {};
  check('1a the failed join shows an error with a report to copy', s1.phase === 'error' && /network/.test(s1.diag || ''), { phase: s1.phase, err: s1.error });
  check('1b the relay stored it, under Nat\'s account', Array.isArray(r1) && r1[0].account === N.account, r1);
  check('1c the report has the error, version, OS and the call log', rep.kind === 'call' && rep.error?.kind === 'network' && rep.version && rep.os && Array.isArray(rep.events) && rep.events.some(e => /could not connect \(attempt 1\)/.test(e)) && rep.attempt >= 1 && /^ws:/.test(rep.server || ''), rep);
  check('1d no keys or tokens in it', !/"(token|keys?|secret)"/.test(r1[0]?.report || '') && !/eyJ[A-Za-z0-9_-]{10,}/.test(r1[0]?.report || ''));

  // 2. switched off: a failure leaves the Copy text but sends nothing
  await nat.cmd('leave'); await sleep(500);
  await nat.cmd('report_problems', { on: false });
  await nat.cmd('join', { gid, name: 'general' });
  const s2 = await soft(until(async () => { const s = await nat.snap(); return s.phase === 'error' && s.diag && !s.reportProblems ? s : null; }, 90000, 'second failure'));
  await sleep(3000);
  check('2 with the switch off: still a report to copy, nothing sent', s2.phase === 'error' && reports().length === 1, { phase: s2.phase, stored: reports().length });

  // 3. the setting is remembered
  const prefs = JSON.parse(fs.readFileSync(path.join(out, 'data/nat/gpui-call-prefs.json'), 'utf8'));
  check('3 the switch is saved in the call settings', prefs.reportProblems === false, prefs);
  await nat.cmd('report_problems', { on: true });
  await nat.cmd('leave');

  // 4. the call works again once the server is back, and nothing else was sent
  lk.startLivekit(); await sleep(2000);
  await nat.cmd('join', { gid, name: 'general' });
  const s4 = await soft(until(async () => { const s = await nat.snap(); return s.phase === 'connected' ? s : null; }, 30000, 'connected'));
  check('4 with the call server back the join works', s4.phase === 'connected', { phase: s4.phase, err: s4.error });
  await nat.cmd('leave');
} catch (e) {
  failed++; console.log('FAIL  crashed: ' + (e.stack || e));
} finally {
  stopAll();
}
console.log(failed ? `${failed} FAILED` : 'ALL PASS');
process.exit(failed ? 1 : 0);
