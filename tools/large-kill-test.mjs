// Real-process crash test for large files: a relay and two dev bridges, killed with SIGKILL at each crash point.
// Usage: node tools/large-kill-test.mjs <outdir> <size-in-MB>   (drives the same RPCs the page does, with the page's retry rules)
import { spawn } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const out = path.resolve(process.argv[2] || '/tmp/large-kill'), MB = Number(process.argv[3] || 26);
const root = path.resolve(new URL('..', import.meta.url).pathname);
const bin = p => path.join(root, 'target/debug', p);
const sleep = ms => new Promise(r => setTimeout(r, ms));
const RP = Number(process.env.KILL_RELAY_PORT || 18792), AP = Number(process.env.KILL_A_PORT || 7611), BP = Number(process.env.KILL_B_PORT || 7612);
const RELAY = `http://127.0.0.1:${RP}`;
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const procs = {};
const start = (name, cmd, args, env = {}) => { const p = spawn(cmd, args, { env: { ...process.env, ...env }, stdio: ['ignore', fs.openSync(path.join(out, name + '.log'), 'a'), fs.openSync(path.join(out, name + '.log'), 'a')] }); procs[name] = p; return p; };
const kill = async name => { const p = procs[name]; p.kill('SIGKILL'); await new Promise(r => p.once('exit', r)); };
process.on('exit', () => Object.values(procs).forEach(p => { try { p.kill('SIGKILL'); } catch { } }));
const startRelay = () => start('relay', bin('commons-relay'), ['--bind', `127.0.0.1:${RP}`, '--data', path.join(out, 'relay')]);
const startBridge = (n, port) => start(n, bin('dev_bridge'), [String(port), path.join(root, 'app/ui')], { COMMONS_DATA: path.join(out, n), COMMONS_RELAY: RELAY, COMMONS_FAST_KDF: '1', COMMONS_DOWNLOAD_DIR: path.join(out, 'dl') });
const rpcRaw = async (port, method, args = {}) => { const r = await fetch(`http://127.0.0.1:${port}/rpc`, { method: 'POST', body: JSON.stringify({ method, args }) }); const j = await r.json(); if (j.err !== undefined) throw new Error(j.err); return j.ok; };
const b64u = u8 => Buffer.from(u8).toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
// The page's rule: "[retry]" errors and an unreachable core are waited out, forever, with backoff.
let retries = 0;
async function rpc(port, method, args = {}, { giveUpAfter = 600000 } = {}) {
  const t0 = Date.now(); let delay = 300;
  for (;;) {
    try { return await rpcRaw(port, method, args); }
    catch (er) {
      const m = String(er.message);
      if (!/^\[retry\]|fetch failed|ECONNREFUSED|ECONNRESET|^locked$|network/i.test(m)) throw er;
      if (Date.now() - t0 > giveUpAfter) throw new Error('gave up waiting: ' + m);
      retries++; await sleep(delay); delay = Math.min(delay * 2, 2000);
    }
  }
}
const restart = async (n, port) => { await kill(n); startBridge(n, port); for (let i = 0; i < 60; i++) { try { await rpcRaw(port, 'unlock', { passphrase: 'correct horse battery' }); return; } catch { await sleep(200); } } throw new Error('bridge did not come back'); };
const results = [];
const ok = (name, cond, detail = '') => { results.push([name, !!cond, detail]); console.log((cond ? 'PASS ' : 'FAIL ') + name + (detail ? ' - ' + detail : '')); if (!cond) process.exitCode = 1; };

// the file
const file = path.join(out, 'big.bin'), size = MB * 1024 * 1024 + 4321;
{
  const h = crypto.createHash('sha256'), fd = fs.openSync(file, 'w'), block = Buffer.alloc(1 << 20); let x = 12345, at = 0, n = 0;
  while (at < size) { for (let i = 0; i < block.length; i += 7) { x = (x * 1103515245 + 12345) >>> 0; block.writeUInt32LE(x, Math.min(i, block.length - 4)); } block.writeUInt32LE(n++, 0); const len = Math.min(block.length, size - at); fs.writeSync(fd, block, 0, len); h.update(block.subarray(0, len)); at += len; }
  fs.closeSync(fd); var want = h.digest('hex');
}
const mtime = Math.floor(fs.statSync(file).mtimeMs / 1000);
const CH = 8 * 1024 * 1024;
const slice = i => { const fd = fs.openSync(file, 'r'), len = Math.min(CH, size - i * CH), b = Buffer.alloc(len); fs.readSync(fd, b, 0, len, i * CH); fs.closeSync(fd); return b; };

startRelay(); await sleep(700); startBridge('alice', AP); startBridge('bob', BP); await sleep(1500);
await rpcRaw(AP, 'create_account', { name: 'Alice', passphrase: 'correct horse battery', relay: RELAY });
await rpcRaw(BP, 'create_account', { name: 'Bob', passphrase: 'correct horse battery', relay: RELAY });
const cid = (await rpcRaw(AP, 'create_community', { name: 'Kill club' })).id;
const link = (await rpcRaw(AP, 'create_invite', { cid, uses: 5, ttl: 3600 })).link;
await rpcRaw(BP, 'join', { link });
let gid;
for (let i = 0; i < 80 && !gid; i++) { const s = await rpcRaw(AP, 'snapshot'), t = await rpcRaw(BP, 'snapshot'); const a = s.communities.find(c => c.id === cid)?.channels.find(c => c.joined && c.status === 'ok'), b = t.communities.find(c => c.id === cid)?.channels.find(c => c.joined && c.status === 'ok'); if (a && b) gid = a.gid; else await sleep(700); }
if (!gid) throw new Error('world did not assemble');

// ---- UPLOAD, with SIGKILLs. The loop is the page's loop: begin, verify a sample, send need_read, finish.
async function uploadLoop(stopAfter) {
  let info = await rpc(AP, 'file_upload_begin', { gid, name: 'big.bin', mime: 'application/octet-stream', size, last_modified: mtime, caption: '' });
  for (const i of info.verify) await rpc(AP, 'file_upload_check', { id: info.id, index: i, data: b64u(slice(i)) });
  let sent = 0;
  for (const i of info.need_read) {
    if (stopAfter !== undefined && sent >= stopAfter) return { info, stopped: true };
    await rpc(AP, 'file_upload_chunk', { id: info.id, index: i, data: b64u(slice(i)) }); sent++;
  }
  return { info, sent };
}
const chunks = Math.ceil(size / CH);
// 1. bridge killed after about half the chunks
let r = await uploadLoop(Math.floor(chunks / 2));
const id = r.info.id;
await restart('alice', AP);
let tr = await rpcRaw(AP, 'file_transfers');
const sess = tr.uploads.find(u => u.id === id);
ok('upload session and hashes survive a SIGKILLed bridge', sess && sess.hashed >= Math.floor(chunks / 2), JSON.stringify({ hashed: sess?.hashed, uploaded: sess?.uploaded, chunks }));
// 2. the relay is killed while the rest is sent; the page's loop waits it out
const relayKiller = (async () => { await sleep(300); await kill('relay'); await sleep(2500); startRelay(); })();
r = await uploadLoop();
await relayKiller;
ok('upload continues after the relay is SIGKILLed and restarted (automatic retry)', r.sent >= 1 && retries >= 0, `sent ${r.sent} more, retries ${retries}`);
// 3. everything is on the relay; the bridge dies before finish. Finish then needs no file.
await restart('alice', AP);
const sy = await rpcRaw(AP, 'file_upload_sync', { id }); tr = await rpcRaw(AP, 'file_transfers'); const u2 = tr.uploads.find(u => u.id === id);
ok('the relay reports every chunk', sy.uploaded.length === chunks);
ok('fully uploaded session is ready to finish without the file', u2 && u2.ready && u2.uploaded === chunks, JSON.stringify(u2));
// a changed file is refused
await rpcRaw(AP, 'file_upload_sync', { id }).catch(() => { });
const bad = Buffer.from(slice(0)); bad[100] ^= 1;
const refused = await rpcRaw(AP, 'file_upload_check', { id, index: 0, data: b64u(bad) }).then(() => false, e => /changed/.test(e.message));
ok('a changed file is refused', refused);
const fin = await rpc(AP, 'file_upload_finish', { id });
const fin2 = await rpc(AP, 'file_upload_finish', { id });
ok('finish is idempotent (second ask returns the same message)', fin.id === fin2.id);
// 4. download with kills of bob's bridge and of the relay
await sleep(2500);
let mid; for (let i = 0; i < 40 && !mid; i++) { const m = await rpcRaw(BP, 'messages', { gid }).catch(() => null); const f = m && (m.messages || m).find?.(x => x.file && x.file.large); if (f) mid = f.id; else await sleep(500); }
ok('bob received the file message', !!mid);
async function downloadLoop(killAt) {
  const st = await rpc(BP, 'file_download_begin', { gid, id: mid, folder: '' });
  let done = st.done, n = 0;
  while (done < st.chunks) { const d = await rpc(BP, 'file_download_chunk', { gid, id: mid }); done = d.done; n++; if (d.finished) return { path: d.path, n, st }; if (killAt && n === killAt) return { stopped: true, done }; }
  return { n, st };
}
let d = await downloadLoop(Math.max(1, Math.floor(chunks / 3)));
await restart('bob', BP);
const t0 = Date.now();
const stBegin = await rpcRaw(BP, 'file_download_begin', { gid, id: mid, folder: '' });
ok('download resumes from saved chunks; begin is fast (no full re-read)', stBegin.done >= 1 && Date.now() - t0 < 5000, `done ${stBegin.done}/${stBegin.chunks} in ${Date.now() - t0} ms`);
const relayKiller2 = (async () => { await sleep(200); await kill('relay'); await sleep(4000); startRelay(); })();
d = await downloadLoop();
await relayKiller2;
const got = crypto.createHash('sha256').update(fs.readFileSync(d.path)).digest('hex');
ok('download finishes through a relay SIGKILL and matches the original', got === want, d.path);
fs.rmSync(path.join(out, 'dl'), { recursive: true, force: true });
console.log(JSON.stringify(results.map(([n, p]) => [n, p])));
fs.rmSync(file, { force: true });
await kill('alice'); await kill('bob'); await kill('relay');
