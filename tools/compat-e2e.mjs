// Old and new apps in one community: an update must never split it. Runs the clients of an OLD version (built from an
// older commit) and of THIS version side by side, through the dev bridge (the same core and rpc the app uses), against
// this version's relay and against the old relay. Checks chat, replies, reactions, edits, files and joins in both
// directions, and that the new relay's /update/ route leaves old clients alone. Nothing leaves 127.0.0.1.
//
//   OLD=$HOME/data/agent-tmp/commons-update/compat/old      # git archive <old commit> | tar -x -C $OLD/src, then build there:
//   (cd $OLD/src && CARGO_TARGET_DIR=$OLD/target cargo build -j2 -p commons-relay && CARGO_TARGET_DIR=$OLD/target cargo build -j2 -p commons-core --features devbridge --bin dev_bridge)
//   cargo build -j2 -p commons-relay && cargo build -j2 -p commons-core --features devbridge --bin dev_bridge
//   node tools/compat-e2e.mjs $OLD <outdir>
import fs from 'node:fs'; import path from 'node:path';
import { root, run, sleep, stopAll } from './lib/env.mjs';

const OLD = path.resolve(process.argv[2] || ''), out = path.resolve(process.argv[3] || '');
const oldBin = n => path.join(OLD, 'target/debug', n), newBin = n => path.join(root, 'target/debug', n);
if (!process.argv[3] || !fs.existsSync(oldBin('dev_bridge'))) { console.error('usage: node tools/compat-e2e.mjs <old build dir> <outdir> (see the top of this file)'); process.exit(2); }
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
let failed = 0; const check = (n, ok, d = '') => { if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${n}${d ? '  [' + d + ']' : ''}`); };
async function until(fn, ms = 40000, what = 'condition') { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(300); } }
async function rpc(port, method, args = {}) {
  const r = await fetch(`http://127.0.0.1:${port}/rpc`, { method: 'POST', body: JSON.stringify({ method, args }) });
  const j = await r.json(); if (j.err !== undefined) throw new Error(`${method}: ${j.err}`); return j.ok;
}

let port = 27600;
async function relay(bin, name) {
  const p = port++; run(bin, ['--bind', `127.0.0.1:${p}`, '--data', path.join(out, name), '--site', path.join(out, name + '-site')], {}, path.join(out, name + '.log'));
  fs.mkdirSync(path.join(out, name + '-site'), { recursive: true });
  const url = `http://127.0.0.1:${p}`; await until(() => fetch(url + '/health').then(r => r.ok), 15000, name); return url;
}
async function person(bin, name, rel) {
  const p = port++; run(bin, [String(p), path.join(root, 'app/ui')], { COMMONS_DATA: path.join(out, 'data', name), COMMONS_RELAY: rel, COMMONS_FAST_KDF: '1' }, path.join(out, name + '.log'));
  await until(() => fetch(`http://127.0.0.1:${p}/rpc`, { method: 'POST', body: '{"method":"state","args":{}}' }).then(r => r.ok), 15000, name);
  await rpc(p, 'create_account', { name, passphrase: 'correct horse battery', relay: rel });
  return { name, port: p };
}
const room = async (x, cid) => until(async () => { const s = await rpc(x.port, 'snapshot'); const c = s.communities.find(c => c.id === cid); const ch = c && c.channels.find(h => h.joined && h.status === 'ok'); return ch && ch.gid; }, 60000, x.name + ' in the room');
const msgs = (x, gid) => rpc(x.port, 'messages', { gid }).then(r => r.messages);
const sees = (x, gid, f, what) => until(async () => (await msgs(x, gid)).find(f), 40000, `${x.name} sees ${what}`);

// One community, `a` the founder and `b` joining; everything either way.
async function together(label, a, b) {
  const cid = (await rpc(a.port, 'create_community', { name: label })).id;
  const link = (await rpc(a.port, 'create_invite', { cid, uses: 5, ttl: 3600 })).link;
  await rpc(b.port, 'join', { link });
  const [ga, gb] = [await room(a, cid), await room(b, cid)];
  check(`${label}: ${b.name} joined ${a.name}'s community`, ga === gb);
  const t1 = `hello from ${a.name}`, t2 = `hello back from ${b.name}`;
  const m1 = (await rpc(a.port, 'send_text', { gid: ga, text: t1 })).id;
  const r1 = await sees(b, gb, m => m.text === t1, t1).catch(() => null);
  check(`${label}: ${a.name} -> ${b.name}`, !!r1);
  await rpc(b.port, 'send_text', { gid: gb, text: t2, reply: r1 && r1.id });
  const r2 = await sees(a, ga, m => m.text === t2, t2).catch(() => null);
  check(`${label}: ${b.name} -> ${a.name}, as a reply`, !!r2 && r2.reply && r2.reply.text === t1, JSON.stringify(r2 && r2.reply));
  if (r1) await rpc(b.port, 'react', { gid: gb, id: r1.id, emoji: '👍', on: true });
  check(`${label}: a reaction from ${b.name}`, !!(await sees(a, ga, m => m.text === t1 && m.reactions.some(x => x.emoji === '👍'), 'the reaction').catch(() => null)));
  await rpc(a.port, 'edit', { gid: ga, id: m1, text: t1 + ' (edited)' });
  check(`${label}: an edit from ${a.name}`, !!(await sees(b, gb, m => m.text === t1 + ' (edited)' && m.edited, 'the edit').catch(() => null)));
  await rpc(b.port, 'send_file', { gid: gb, name: 'note.txt', mime: 'text/plain', data: Buffer.from('a file across versions').toString('base64url'), caption: 'a file' });
  check(`${label}: a file from ${b.name}`, !!(await sees(a, ga, m => m.kind === 'file' && m.file && m.file.name === 'note.txt', 'the file').catch(() => null)));
  const bad = (await msgs(a, ga)).concat(await msgs(b, gb)).filter(m => m.status === 'system' && /not understood|malformed|unexpected/.test(m.text));
  check(`${label}: nothing either side could not read`, bad.length === 0, bad.map(m => m.text).join(' | '));
}

try {
  const relNew = await relay(newBin('commons-relay'), 'relay-new');
  const oa = await person(oldBin('dev_bridge'), 'OldAnn', relNew), nb = await person(newBin('dev_bridge'), 'NewBen', relNew);
  await together('new relay, old founder', oa, nb);
  const na = await person(newBin('dev_bridge'), 'NewAda', relNew), ob = await person(oldBin('dev_bridge'), 'OldBob', relNew);
  await together('new relay, new founder', na, ob);
  const relOld = await relay(oldBin('commons-relay'), 'relay-old');
  const nc = await person(newBin('dev_bridge'), 'NewCal', relOld), od = await person(oldBin('dev_bridge'), 'OldDee', relOld);
  await together('old relay, new founder', nc, od);
  const r = await fetch(relOld + '/update/latest.json');
  check('an old relay answers the update check with "nothing here", not an error page the app would trip on', r.status === 404 || r.status === 400 || r.status === 401, String(r.status));
} catch (e) { failed++; console.log('FAIL  stopped early: ' + (e.stack || e)); }
stopAll();
console.log(failed ? `${failed} FAILED` : 'ALL PASSED');
process.exit(failed ? 1 : 0);
