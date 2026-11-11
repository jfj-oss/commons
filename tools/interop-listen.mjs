// Listen-along interop: a NEW sharer (fake rune playing a Spotify song), an OLD 0.3.5 member who must see a plain "listening to"
// and stay healthy, and a NEW joiner whose own fake rune must be driven to the same song and position, then follow a pause and a seek.
// Only fake servers: no real Spotify, no real rune.
import { spawn } from 'node:child_process';
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const home = process.env.HOME;
const NEW = `${home}/wt-clean/megatron-style/target/debug`;
const OLD = `${home}/data/agent-tmp/mega-sec2/old-target/debug/dev_bridge`;   // 0.3.5 (bfda74b)
const UI = `${home}/wt-clean/megatron-listen/app/ui`;
const out = `${home}/data/agent-tmp/listen-along/interop`;
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const sleep = ms => new Promise(r => setTimeout(r, ms));
const procs = []; const log = fs.openSync(path.join(out, 'procs.log'), 'a');
const run = (cmd, args, env = {}) => { const p = spawn(cmd, args, { env: { ...process.env, ...env }, stdio: ['ignore', log, log] }); procs.push(p); return p; };
const RELAY = 'http://127.0.0.1:18959';
let failed = 0;
const check = (ok, what, extra) => { console.log(`${ok ? 'PASS' : 'FAIL'}  ${what}${extra !== undefined ? '  [' + JSON.stringify(extra).slice(0, 300) + ']' : ''}`); if (!ok) failed++; };
const ID = '4uLU6hMCjMI75M1A2tKUQC';

function fakeRune(port) {
  const st = { status: 'playing', pos: 60000, at: Date.now(), posts: [] };
  const state = () => ({ status: st.status, index: 0, positionMs: st.pos, updatedAt: st.at, queue: [{ track: { id: 'sp:' + ID, source: 'spotify', spotifyUri: 'spotify:track:' + ID, title: 'Fake Song', artists: [{ name: 'Fake Band' }] } }] });
  const srv = http.createServer((req, res) => {
    let b = ''; req.on('data', d => b += d); req.on('end', () => {
      const j = (o) => { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify(o)); };
      if (req.url === '/api/zones') return j([{ id: 'pc', name: 'PC speakers', kind: 'pc' }]);
      if (req.url === '/api/player/pc' && req.method === 'GET') return j(state());
      if (req.url === '/api/player/pc' && req.method === 'POST') {
        const c = JSON.parse(b || '{}'); st.posts.push(c);
        if (c.type === 'play') { st.status = c.paused ? 'paused' : 'playing'; st.pos = c.positionMs || 0; st.at = Date.now(); }
        if (c.type === 'pause') { st.pos += st.status === 'playing' ? Date.now() - st.at : 0; st.at = Date.now(); st.status = 'paused'; }
        if (c.type === 'seek') { st.pos = c.positionMs; st.at = Date.now(); }
        return j({});
      }
      res.writeHead(404); res.end();
    });
  }).listen(port, '127.0.0.1');
  return { st, srv };
}
function bridge(name, bin, port) {
  run(bin, [String(port), UI], { COMMONS_DATA: path.join(out, 'data', name), COMMONS_RELAY: RELAY, COMMONS_FAST_KDF: '1' });
  const rpc = async (method, args = {}) => { const r = await fetch(`http://127.0.0.1:${port}/rpc`, { method: 'POST', body: JSON.stringify({ method, args }) }); const j = await r.json(); if ('err' in j) throw new Error(`${name} ${method}: ${j.err}`); return j.ok; };
  return { name, rpc };
}
async function until(f, what, ms = 45000) { const t = Date.now(); let last; while (Date.now() - t < ms) { try { last = await f(); if (last) return last; } catch (e) { last = e.message; } await sleep(250); } throw new Error(`timed out: ${what} (last: ${JSON.stringify(last)})`); }
const up = c => until(() => c.rpc('state').then(() => true), `${c.name} up`);
const account = async c => { await up(c); await c.rpc('create_account', { passphrase: 'interop test pass', name: c.name, relay: RELAY }); return (await c.rpc('snapshot')).me.account; };
const comm = (s, cid) => s.communities.find(c => c.id === cid);
async function joinAll(c, cid, inv) { await c.rpc('join', { link: inv }); await until(async () => { const co = comm(await c.rpc('snapshot'), cid); return co && !co.removed && co.channels.length > 0; }, `${c.name} joined`); }

try {
  run(`${NEW}/commons-relay`, ['--bind', '127.0.0.1:18959', '--data', path.join(out, 'data/relay')]);
  const runeA = fakeRune(18971), runeN = fakeRune(18972);
  runeN.st.status = 'stopped';
  await sleep(800);
  const A = bridge('sharer-new', `${NEW}/dev_bridge`, 7771), O = bridge('member-old', OLD, 7772), N = bridge('joiner-new', `${NEW}/dev_bridge`, 7773);
  await sleep(800);
  const aAcct = await account(A); await account(O); await account(N);
  const cid = (await A.rpc('create_community', { name: 'Listen along' })).id;
  const gid = (await A.rpc('create_channel', { cid, name: 'music' })).id;
  for (const c of [O, N]) await joinAll(c, cid, (await A.rpc('create_invite', { cid, uses: 1 })).link);
  await N.rpc('set_listening_sharing', { on: false, rune: 'http://127.0.0.1:18972' });
  await A.rpc('set_listening_sharing', { on: true, rune: 'http://127.0.0.1:18971' });

  const seen = async (c) => (await c.rpc('members', { cid })).find(m => m.account === aAcct)?.listening;
  const fromOld = await until(() => seen(O), 'old member sees the status');
  check(fromOld.title === 'Fake Song' && fromOld.artist === 'Fake Band', '0.3.5 member sees the plain "listening to" for a Spotify song', fromOld);
  const fromNew = await until(async () => (await seen(N))?.spotify, 'new member sees the Spotify part');
  check(fromNew.id === ID && fromNew.paused === false, 'new member sees the track id', fromNew);
  const notes = (await O.rpc('notices')).filter(n => n.kind === 'newer' || n.kind === 'reject' || n.kind === 'decrypt');
  check(notes.length === 0 && (await O.rpc('snapshot')).problems.length === 0, '0.3.5 member has no notices or problems', notes);

  // join
  const j = await N.rpc('listen_join', { account: aAcct, target: '' });
  check(j.state === 'on' && j.target === 'rune (PC speakers)', 'join picks the joiner\'s rune', j);
  const play = await until(() => runeN.st.posts.find(p => p.type === 'play'), 'the joiner\'s rune is told to play');
  check(play.trackIds[0] === 'sp:' + ID && Math.abs(play.positionMs - (60000 + (Date.now() - runeA.st.at))) < 25000, 'same song, near the sharer\'s position', play);
  const nPlays = () => runeN.st.posts.length;
  await sleep(6000);
  check(nPlays() === 1, 'no extra commands while all is steady', runeN.st.posts);

  // pause on the sharer
  runeA.st.pos += Date.now() - runeA.st.at; runeA.st.at = Date.now(); runeA.st.status = 'paused';
  await until(() => runeN.st.posts.find(p => p.type === 'pause'), 'follows the pause', 30000);
  check(true, 'the joiner\'s rune follows the sharer\'s pause');
  const st1 = await N.rpc('listen_status');
  check(st1.state === 'on', 'still listening along while paused', st1);
  // sharer seeks (while paused: sent as news) then resumes
  runeA.st.pos = 200000; runeA.st.at = Date.now(); runeA.st.status = 'playing';
  const resumed = await until(() => runeN.st.posts.slice(2).find(p => p.type === 'play' || p.type === 'seek'), 'follows resume at the new spot', 30000);
  check(Math.abs((resumed.positionMs || 0) - 200000) < 25000, 'resumes near the sharer\'s new position', resumed);

  // sharer switches to a local file: ended
  runeA.st.pos = 1; runeA.st.at = Date.now();
  // (the fake only knows the Spotify song; stop it instead)
  runeA.st.status = 'stopped';
  const ended = await until(async () => { const s = await N.rpc('listen_status'); return s.state === 'ended' && s; }, 'ended when the sharer stops', 40000);
  check(/stopped sharing/.test(ended.message), 'shows ended', ended);
  check(!runeN.st.posts.slice(-1).some(p => p.type === 'next'), 'nothing was sent to the joiner\'s rune at the end', runeN.st.posts.slice(-1));
  await N.rpc('listen_leave');
  check((await N.rpc('listen_status')).state === 'off', 'leave clears the status');
} catch (e) { console.log('FAIL  ' + e.message); failed++; }
for (const p of procs) { try { p.kill(); } catch { } }
console.log(failed ? `${failed} FAILED` : 'ALL PASS');
process.exit(failed ? 1 : 0);
