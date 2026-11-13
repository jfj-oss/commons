// Mixed versions and read receipts. Usage: node tools/interop-receipts.mjs [outdir]. A NEW owner and a NEW member whose apps send `Read`, and an OLD 0.3.5 member who has never heard
// of it (set OLD_BRIDGE to a dev_bridge built from an older release). The old app must keep working (messages both ways, no problem shown, nothing odd in the room); the new ones see each other's place.
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
const home = process.env.HOME;
const NEW = process.env.NEW_TARGET || path.resolve(new URL('..', import.meta.url).pathname, 'target/debug');
const OLD = process.env.OLD_BRIDGE || `${home}/data/agent-tmp/mega-sec2/old-target/debug/dev_bridge`;   // built from bfda74b (Release 0.3.5)
const UI = path.resolve(new URL('..', import.meta.url).pathname, 'app/ui');
const out = process.argv[2] || `${home}/data/agent-tmp/mega-receipts/interop`;
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const sleep = ms => new Promise(r => setTimeout(r, ms));
const procs = [], log = fs.openSync(path.join(out, 'procs.log'), 'a');
const run = (cmd, args, env = {}) => { const p = spawn(cmd, args, { env: { ...process.env, ...env }, stdio: ['ignore', log, log] }); procs.push(p); return p; };
const RELAY_PORT = 18963, RELAY = `http://127.0.0.1:${RELAY_PORT}`;
let failed = 0;
const check = (ok, what, extra) => { console.log(`${ok ? 'PASS' : 'FAIL'}  ${what}${extra !== undefined ? '  [' + JSON.stringify(extra).slice(0, 300) + ']' : ''}`); if (!ok) failed++; };
function bridge(name, bin, port) {
  run(bin, [String(port), UI], { COMMONS_DATA: path.join(out, 'data', name), COMMONS_RELAY: RELAY, COMMONS_FAST_KDF: '1' });
  const rpc = async (method, args = {}) => { const r = await fetch(`http://127.0.0.1:${port}/rpc`, { method: 'POST', body: JSON.stringify({ method, args }) }); const j = await r.json(); if ('err' in j) throw new Error(`${name} ${method}: ${j.err}`); return j.ok; };
  return { name, rpc };
}
async function until(f, what, ms = 45000) { const t = Date.now(); let last; while (Date.now() - t < ms) { try { last = await f(); if (last) return last; } catch (e) { last = e.message; } await sleep(250); } throw new Error(`timed out: ${what} (last: ${JSON.stringify(last)})`); }
const comm = (s, cid) => s.communities.find(c => c.id === cid);
const rows = async (c, gid) => (await c.rpc('messages', { gid, limit: 200 })).messages;
const account = async c => { await until(() => c.rpc('state').then(() => true), `${c.name} bridge up`); await c.rpc('create_account', { passphrase: 'interop test pass', name: c.name, relay: RELAY }); return (await c.rpc('snapshot')).me.account; };
const exchange = async (from, to, gid, text) => { await until(() => from.rpc('send_text', { gid, text, reply: null }).then(() => true), `${from.name} send`, 30000); await until(async () => (await rows(to, gid)).some(m => m.text === text), `${to.name} receives "${text}" from ${from.name}`); return true; };
try {
  run(`${NEW}/commons-relay`, ['--bind', `127.0.0.1:${RELAY_PORT}`, '--data', path.join(out, 'data/relay')]);
  await sleep(800);
  const A = bridge('owner-new', `${NEW}/dev_bridge`, 7771), N = bridge('member-new', `${NEW}/dev_bridge`, 7772), O = bridge('member-old', OLD, 7773);
  await sleep(800);
  for (const c of [A, N, O]) await account(c);
  const cid = (await A.rpc('create_community', { name: 'Mixed receipts' })).id;
  const gid = (await A.rpc('create_channel', { cid, name: 'mixed-room' })).id;
  for (const c of [N, O]) {
    const inv = (await A.rpc('create_invite', { cid, uses: 1 })).link;
    await c.rpc('join', { link: inv });
    await until(async () => { const co = comm(await c.rpc('snapshot'), cid); return co && !co.removed && co.channels.length > 0; }, `${c.name} joined`);
    await until(async () => { const ch = comm(await c.rpc('snapshot'), cid).channels.find(x => x.gid === gid); if (!ch) return false; if (!ch.joined) { await c.rpc('join_channel', { gid }).catch(() => {}); return false; } return true; }, `${c.name} in room`);
  }
  check((await A.rpc('snapshot')).read_receipts === true && (await O.rpc('snapshot')).read_receipts === undefined, 'the new app has the switch (on), the 0.3.5 app has never heard of it');
  check(await exchange(A, O, gid, 'hello from new') && await exchange(O, A, gid, 'hello from old') && await exchange(O, N, gid, 'old to both'), 'messages both ways before any receipt');
  const before = (await rows(O, gid)).length, unreadBefore = comm(await O.rpc('snapshot'), cid).channels.find(x => x.gid === gid).unread;
  // the two new apps read the room: each posts a receipt, which the old app receives too
  await until(async () => (await rows(A, gid)).some(m => m.text === 'old to both'), 'owner has the line');
  await A.rpc('mark_read', { gid }); await N.rpc('mark_read', { gid });
  const last = (await rows(A, gid)).at(-1);
  await until(async () => ((await rows(A, gid)).find(m => m.id === last.id).read_by || []).includes('member-new'), 'the new owner sees the new member\'s place');
  await until(async () => ((await rows(N, gid)).find(m => m.id === last.id).read_by || []).includes('owner-new'), 'the new member sees the new owner\'s place');
  check(true, 'the two new apps see each other\'s place on the newest message');
  await sleep(2500);
  const oRows = await rows(O, gid);
  check(oRows.length === before && oRows.every(m => !('read_by' in m)), 'the 0.3.5 app shows nothing new in the room', oRows.length - before);
  check(comm(await O.rpc('snapshot'), cid).channels.find(x => x.gid === gid).unread === unreadBefore, 'a receipt is not an unread message on the 0.3.5 app');
  const oProblems = (await O.rpc('snapshot')).problems;
  check(oProblems.length === 0, 'no problem shown on the 0.3.5 app', oProblems);
  // several more rounds of reading, with messages from the old app in between
  for (let i = 0; i < 4; i++) { await exchange(O, A, gid, 'old line ' + i); await until(async () => (await rows(N, gid)).some(m => m.text === 'old line ' + i), 'new member has the line'); await A.rpc('mark_read', { gid }); await N.rpc('mark_read', { gid }); await sleep(900); }
  await sleep(4000);
  const lastOld = (await rows(A, gid)).at(-1);
  await until(async () => ((await rows(A, gid)).find(m => m.id === lastOld.id).read_by || []).includes('member-new'), 'the place follows the newest message');
  check(true, 'receipts keep moving forward with a 0.3.5 member in the room');
  // the old app marks the room read as it always did: nobody is told (it sends no receipt), and nobody shows a place for it
  await O.rpc('mark_read', { gid }); await sleep(2500);
  check((await rows(A, gid)).every(m => !(m.read_by || []).includes('member-old')), 'the 0.3.5 member has no place on the new apps (it sends none)');
  check(await exchange(O, A, gid, 'old after receipts') && await exchange(A, O, gid, 'new after receipts') && await exchange(N, O, gid, 'second new after receipts'), 'messages both ways after the receipts');
  const oNotes = await O.rpc('notices');
  const newer = oNotes.filter(n => n.kind === 'newer').length;
  console.log(`INFO  the 0.3.5 app wrote ${newer} "newer version" line(s) in its activity log for ${oNotes.length} notes in all`);
  check(newer >= 1 && newer <= 12, 'the 0.3.5 app notes "newer version" in its log and nothing worse', newer);
  check(!oNotes.some(n => ['decrypt', 'broken', 'rollback', 'reject', 'gap'].includes(n.kind)), 'no decrypt, gap or reject notes on the 0.3.5 app', oNotes.filter(n => ['decrypt', 'broken', 'rollback', 'reject', 'gap'].includes(n.kind)).map(n => n.text.slice(0, 80)));
  const finals = await Promise.all([A, N, O].map(async c => { const s = await c.rpc('snapshot'); return [c.name, s.problems.length, s.outbox]; }));
  check(finals.every(f => f[1] === 0), 'no client reports problems at the end', finals);
} catch (e) { failed++; console.log('FAIL  ' + e.message); }
finally { for (const p of procs) p.kill('SIGTERM'); await sleep(500); for (const p of procs) p.kill('SIGKILL'); }
console.log(failed ? `${failed} FAILED` : 'INTEROP OK');
process.exit(failed ? 1 : 0);
