// A throwaway world for looking at the native call UI. Run it with COMMONS_TEST_PORT_OFFSET=600 (or more). Starts a local
// LiveKit and relay, makes Alice (the one the window will sign in as), Bob and Carol, a community with a text room and a
// voice room, a direct conversation, and some talk. Bob and Carol then become headless native call engines
// (`mega-gpui --call-headless`) with a test port each. Writes world.json (next to the data) and waits until killed.
//   COMMONS_TEST_PORT_OFFSET=600 node ui-world.mjs <outdir> <mega-gpui binary>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn } from 'node:child_process';
import { startServers, startBridge, rpc, sleep, procs, stopAll, RELAY_PORT, port as P } from '../../../tools/lib/env.mjs';

const out = process.argv[2], BIN = process.argv[3];
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const PASS = 'throwaway-pass-1', rel = `http://127.0.0.1:${RELAY_PORT}`;
await startServers(out, {});
const ports = { alice: P(27210), bob: P(27211), carol: P(27212) };
const bridges = {};
for (const [n, port] of Object.entries(ports)) bridges[n] = startBridge(out, n, port);
await sleep(1500);
for (const [n, port] of Object.entries(ports)) await rpc(port, 'create_account', { name: n[0].toUpperCase() + n.slice(1), passphrase: PASS, relay: rel });
const cid = (await rpc(ports.alice, 'create_community', { name: 'Test club' })).id;
const text = await rpc(ports.alice, 'create_channel', { cid, name: 'lounge' });
const voice = await rpc(ports.alice, 'create_channel', { cid, name: 'porch', kind: 'voice' });
await rpc(ports.alice, 'create_channel', { cid, name: 'garden' });
const link = (await rpc(ports.alice, 'create_invite', { cid, uses: 10, ttl: 3600 })).link;
for (const n of ['bob', 'carol']) await rpc(ports[n], 'join', { link });
const rooms = async n => { const s = await rpc(ports[n], 'snapshot'); const c = s.communities.find(c => c.id === cid); return c ? c.channels : []; };
for (let t = Date.now(); ;) {
  let ok = true;
  for (const n of ['alice', 'bob', 'carol']) { const ch = await rooms(n); if (!['lounge', 'porch', 'garden'].every(x => ch.some(c => c.name === x && c.joined && c.status === 'ok'))) ok = false; }
  if (ok) break; if (Date.now() - t > 90000) throw new Error('rooms did not assemble'); await sleep(700);
}
const gid = async (n, name) => (await rooms(n)).find(c => c.name === name).gid;
const g = { lounge: await gid('alice', 'lounge'), porch: await gid('alice', 'porch'), garden: await gid('alice', 'garden') };
const say = (n, name, t) => rpc(ports[n], 'send_text', { gid: g[name], text: t });
await say('bob', 'lounge', 'anyone up for the porch call?'); await say('carol', 'lounge', 'yes, give me two minutes'); await say('alice', 'lounge', 'I am here');
await say('bob', 'lounge', 'this room has colours the owner chose, and a pattern that flows around the words'); await say('alice', 'garden', 'seeds are in');
// a direct conversation Bob started (his account is Alice\'s peer)
const snapB = await rpc(ports.bob, 'snapshot');
const aliceAcct = (await rpc(ports.alice, 'snapshot')).me.account;
try { await rpc(ports.bob, 'direct_call', { peer: aliceAcct }); } catch (e) { console.error('direct_call:', e.message); }
const accts = {}; for (const n of Object.keys(ports)) accts[n] = (await rpc(ports[n], 'snapshot')).me.account;
await sleep(1500);
fs.writeFileSync(path.join(out, 'world.json'), JSON.stringify({ relay: rel, cid, gids: g, accts, ports }, null, 1));

// Alice's folder is for the window; Bob and Carol become native headless engines
for (const n of ['alice', 'bob', 'carol']) { try { process.kill(-bridges[n].pid, 'SIGTERM'); } catch { } }
await sleep(1800);
fs.cpSync(path.join(out, 'data/alice'), path.join(out, 'alice-pristine'), { recursive: true });
class Native {
  constructor(name, hz, testPort) {
    this.n = 0; this.waiting = new Map(); this.buf = '';
    this.proc = spawn(BIN, ['--call-headless'], { env: { ...process.env, COMMONS_DATA: path.join(out, 'data', name), COMMONS_RELAY: rel, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_CALL_TONE: String(hz), MEGA_GPUI_CALL_TEST: String(testPort), MEGA_GPUI_NOTIFY_FILE: path.join(out, name + '-notes.jsonl') }, stdio: ['ignore', 'pipe', fs.openSync(path.join(out, name + '-native.log'), 'a')], detached: true });
    procs.push(this.proc);
    this.ready = new Promise((res, rej) => { let b = ''; this.proc.stdout.on('data', d => { b += d; if (b.includes('\n')) res(JSON.parse(b.split('\n')[0])); }); this.proc.on('exit', c => rej(new Error('exit ' + c))); });
    this.port = testPort;
  }
}
const nat = { bob: new Native('bob', 700, P(27320)), carol: new Native('carol', 1100, P(27321)) };
await nat.bob.ready; await nat.carol.ready;
fs.writeFileSync(path.join(out, 'world.json'), JSON.stringify({ relay: rel, cid, gids: g, accts, ports, natPorts: { bob: nat.bob.port, carol: nat.carol.port } }, null, 1));
console.log('READY');
process.on('SIGTERM', () => { stopAll(); process.exit(0); });
await new Promise(() => { });
