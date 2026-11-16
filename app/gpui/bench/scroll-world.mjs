// A throwaway world for comparing the web and native navigation: Alice owns four communities (one with eight rooms), plus a
// direct conversation. Run with COMMONS_TEST_PORT_OFFSET=2100:  node scroll-world.mjs <outdir>
// Writes world.json and waits until killed. data/alice is for the web bridge, alice-pristine for the native window.
import fs from 'node:fs';
import path from 'node:path';
import { startServers, startBridge, rpc, sleep, stopAll, RELAY_PORT, port as P } from '../../../tools/lib/env.mjs';
const out = process.argv[2];
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const PASS = 'throwaway-pass-1', rel = `http://127.0.0.1:${RELAY_PORT}`;
await startServers(out, {});
const ports = { alice: P(27210), bob: P(27211) };
const bridges = {};
for (const [n, port] of Object.entries(ports)) bridges[n] = startBridge(out, n, port);
await sleep(1500);
for (const [n, port] of Object.entries(ports)) await rpc(port, 'create_account', { name: n[0].toUpperCase() + n.slice(1), passphrase: PASS, relay: rel });
const comms = { 'Test club': ['lounge', 'porch:voice', 'garden', 'workshop', 'library', 'kitchen', 'attic:voice', 'cellar'], 'Night owls': ['late:voice'], 'Book circle': ['reading', 'quotes', 'plans'], 'Chess': ['openings', 'endgames'] };
const ids = {};
for (const [name, rooms] of Object.entries(comms)) {
  const cid = (await rpc(ports.alice, 'create_community', { name })).id; ids[name] = cid;
  for (const r of rooms) { const [n, k] = r.split(':'); await rpc(ports.alice, 'create_channel', { cid, name: n, ...(k ? { kind: k } : {}) }); }
}
const link = (await rpc(ports.alice, 'create_invite', { cid: ids['Test club'], uses: 10, ttl: 3600 })).link;
await rpc(ports.bob, 'join', { link });
for (let t = Date.now(); ;) { const s = await rpc(ports.alice, 'snapshot'); const c = s.communities.find(c => c.id === ids['Test club']); if (c && c.channels.length >= 8 && c.channels.every(x => x.joined && x.status === 'ok')) break; if (Date.now() - t > 90000) throw new Error('rooms'); await sleep(700); }
const s = await rpc(ports.alice, 'snapshot'); const g = {}; for (const c of s.communities) for (const ch of c.channels) g[c.name + '/' + ch.name] = ch.gid;
const say = (n, key, t) => rpc(ports[n], 'send_text', { gid: g[key], text: t });
await say('alice', 'Test club/lounge', 'I am here'); await say('alice', 'Test club/garden', 'seeds are in'); await say('alice', 'Night owls/general', 'who is awake');
await say('alice', 'Book circle/reading', 'chapter four tonight');
const aliceAcct = s.me.account;
try { await rpc(ports.bob, 'direct_call', { peer: aliceAcct }); } catch (e) { console.error('direct_call:', e.message); }
await sleep(1500);
fs.writeFileSync(path.join(out, 'world.json'), JSON.stringify({ relay: rel, ids, gids: g, ports }, null, 1));
await sleep(500);
for (const n of ['alice', 'bob']) { try { process.kill(-bridges[n].pid, 'SIGTERM'); } catch { } }
await sleep(1800);
fs.cpSync(path.join(out, 'data/alice'), path.join(out, 'alice-pristine'), { recursive: true });
console.log('READY');
process.on('SIGTERM', () => { stopAll(); process.exit(0); });
await new Promise(() => { });
