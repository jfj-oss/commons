// The whole hosted path on loopback: deploy/go-online.sh in its local-test mode, then a stand-in for Tailscale Funnel (a TLS
// terminator on one port, which is all Funnel does here), and two people who only ever use that one port. Media is forced
// through TURN over TLS, the only route a friend outside the tailnet has. Fake media devices, muted, headless, no Tailscale.
//   systemd-run --user --scope -q -p MemoryMax=6G node tools/hosting-e2e.mjs [outdir]
import fs from 'node:fs'; import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { root, run, startChrome, openPage, rpc, sleep, stopAll, FAKE_DISPLAY } from './lib/env.mjs';
import { toneScript } from './lib/world.mjs';
import { startHost, forceRelay, relayPath, screenShot } from './lib/host.mjs';

const out = process.argv[2] || '/tmp/commons-hosting-e2e';
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
let failed = 0; const check = (n, ok, d = '') => { if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${n}${d ? '  [' + d + ']' : ''}`); };
const J = JSON.stringify;
async function until(fn, ms = 20000, what = 'condition') { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(250); } }

const host = await startHost(out);
const { BASE, FRONT, PUB, NAME, PASS, RELAY, offline, stopTerminator, tlsIn } = host;
const bridge = (name, port) => run(path.join(root, 'target/debug/dev_bridge'), [String(port), path.join(root, 'app/ui')], { COMMONS_DATA: path.join(out, name), COMMONS_RELAY: '', COMMONS_FAST_KDF: '1' }, path.join(out, name + '.log'));

try {
  const web = host.web;
  check('H1 go-online prints a link and the secret is not on any command line', !!web && !fs.readFileSync(`/proc/${fs.readFileSync(path.join(out, 'host/relay.pid'), 'utf8').trim()}/cmdline`, 'latin1').includes(fs.readFileSync(path.join(out, 'host/invite-secret'), 'utf8').trim()), web && web.slice(0, 60));
  const mode = f => (fs.statSync(path.join(out, 'host', f)).mode & 0o777).toString(8);
  check('H2 secrets are private files', mode('invite-secret') === '600' && mode('lk-secret') === '600' && mode('livekit.yaml') === '600');
  const listening = execFileSync('ss', ['-ltnH'], { encoding: 'utf8' }).split('\n').filter(l => new RegExp(`:(${[0, 1, 4, 5].map(i => BASE + i).join('|')})\\s`).test(l));
  check('H3 relay, front and LiveKit signalling are on loopback only (TURN cannot be; it needs credentials)', listening.length === 3 && listening.every(l => l.includes('127.0.0.1:')), listening.map(l => l.trim().split(/\s+/)[3]).join(' '));

  bridge('alice', 26701); bridge('bob', 26702); await sleep(1200);
  // A link as a friend's would be, but with the address the test can reach without a public certificate.
  const payload = web.split('#relay/')[1], link = JSON.parse(Buffer.from(payload, 'base64url'));
  check('H4 link names the public address and carries the secret', link.relay === `https://${NAME}:${PUB}` && link.secret.length >= 32);
  const parsed = await rpc(26701, 'parse_link', { link: web });
  check('H5 the app reads the web link', parsed.kind === 'relay' && parsed.relay === link.relay && parsed.secret === link.secret, parsed.kind);
  const refused = await rpc(26701, 'create_account', { name: 'Eve', passphrase: PASS, relay: RELAY, invite: '' }).then(() => null, e => e.message);
  check('H6 no secret, no account', /invite secret/.test(refused || ''), refused);
  const wrong = await rpc(26701, 'create_account', { name: 'Eve', passphrase: PASS, relay: RELAY, invite: 'not-the-secret' }).then(() => null, e => e.message);
  check('H7 wrong secret, no account', /not this relay/.test(wrong || ''), wrong);
  const down = await rpc(26701, 'create_account', { name: 'A', passphrase: PASS, relay: 'http://127.0.0.1:9', invite: '' }).then(() => null, e => e.message);
  check('H8 an unreachable relay says what to do', /switched off/.test(down || ''), down);

  await rpc(26701, 'create_account', { name: 'Alice', passphrase: PASS, relay: RELAY, invite: link.secret });
  const cid = (await rpc(26701, 'create_community', { name: 'Friends' })).id;
  const inv = (await rpc(26701, 'create_invite', { cid, uses: 1, ttl: 3600 })).link;
  const p2 = await rpc(26702, 'parse_link', { link: inv });
  check('H9 a community invite alone is enough for a newcomer', p2.kind === 'community' && p2.secret === link.secret && p2.name === 'Friends');
  await rpc(26702, 'create_account', { name: 'Bob', passphrase: PASS, relay: p2.relay, invite: p2.secret });
  await rpc(26702, 'join', { link: p2.link });
  let gid;
  await until(async () => { const s = await rpc(26702, 'snapshot'); const c = s.communities.find(x => x.id === cid); const ch = c && c.channels.find(x => x.joined && x.status === 'ok'); gid = ch && ch.gid; return gid; }, 40000, 'bob joined');
  await until(async () => (await rpc(26701, 'snapshot')).communities[0].channels.some(x => x.joined && x.status === 'ok'), 20000, 'alice ready');
  await rpc(26701, 'send_text', { gid, text: 'hello through the front' });
  await until(async () => (await rpc(26702, 'messages', { gid })).messages.some(m => m.text === 'hello through the front'), 15000, 'chat');
  check('H10 chat through the one port', true);
  const blob = Buffer.alloc(300000, 7).toString('base64url');
  await rpc(26702, 'send_file', { gid, name: 'notes.bin', mime: 'application/octet-stream', data: blob, caption: '' });
  const m = await until(async () => (await rpc(26701, 'messages', { gid })).messages.find(x => x.kind === 'file'), 15000, 'file');
  const got = await rpc(26701, 'fetch_file', { gid, id: m.id });
  check('H11 file through the one port', got.data === blob, got.name);

  const pub = async p => { const r = await fetch(`${RELAY}${p}`); return { s: r.status, t: await r.text(), csp: r.headers.get('content-security-policy') }; };
  const join = await pub('/join');
  check('H12 join page served, with a strict policy', join.s === 200 && /Join on Commons/.test(join.t) && /default-src 'none'/.test(join.csp || ''));
  fs.mkdirSync(path.join(root, 'dist'), { recursive: true });
  const list = JSON.parse((await pub('/download/')).t);
  check('H13 download list works and refuses paths', Array.isArray(list) && (await pub('/download/..%2f..%2fetc%2fpasswd')).s === 404 && (await pub('/download/.env')).s === 404, list.map(x => x.name).join());
  const admin = await fetch(`${RELAY}/twirp/livekit.RoomService/ListRooms`, { method: 'POST', body: '{}' });
  check('H14 LiveKit admin API is not reachable through the front', admin.status === 404 || admin.status === 401 || admin.status === 405, String(admin.status));

  // Calls. Relay-only: if this works, nothing depended on a direct path to the server.
  const force = forceRelay;
  const chrome = await startChrome(out, 'all', 26800, 500, ['--ignore-certificate-errors', ...(process.env.E2E_VLOG ? ['--enable-logging=stderr', '--vmodule=*turn_port*=3'] : [])]);
  const A = await openPage(chrome, 'http://127.0.0.1:26701/', 'alice', { before: toneScript(500) + force }), B = await openPage(chrome, 'http://127.0.0.1:26702/', 'bob', { before: toneScript(700) + force });
  for (const [p, pw] of [[A, 26701], [B, 26702]]) await p.waitFor(`!!window.__commonsCall && !document.querySelector("#app").hidden && !!S.gid`, 30000, 'app ready');
  const call = (p, e) => p.ev(`(async()=>{ const c = window.__commonsCall; return ${e}; })()`);
  await call(A, `c.join(S.gid, 'general')`); await call(B, `c.join(S.gid, 'general')`);
  await until(async () => (await call(A, `c.room.remoteParticipants.size`)) === 1, 30000, 'call formed');
  const devA = (await rpc(26701, 'snapshot')).me.device, devB = (await rpc(26702, 'snapshot')).me.device;
  check('H15 frame encryption active over the relayed path', (await call(A, `c.room.isE2EEEnabled`)) && (await call(B, `c.room.isE2EEEnabled`)));
  const path_ = relayPath;
  const pa = await until(async () => { const v = await path_(A); return v.length ? v : null; }, 15000, 'stats');
  check('H16 media path is TURN over TLS', pa.every(x => x === 'relay/tls'), pa.join());
  const servers = await A.ev(`JSON.stringify((window.__pcs||[]).map(p=>p.getConfiguration().iceServers))`);
  check('H17 no third-party STUN, and TURN points at the public port', !/google|twilio/.test(servers) && servers.includes(`:${PUB}?`), servers.replace(/"(username|credential)":"[^"]*",?/g, '').slice(0, 120));
  const hears = async (p, dev, hz, src = 'microphone') => { for (let i = 0; i < 8; i++) { await sleep(900); const r = await call(p, `c.probeTone(${J(dev)}, ${hz}, 1500, ${J(src)})`).catch(() => null); if (r && r.present && r.toneShare > 0.15) return true; } return false; };
  check('H18 bob hears alice', await hears(B, devA, 500));
  check('H19 alice hears bob', await hears(A, devB, 700));
  await A.ev(FAKE_DISPLAY); await A.ev(`window.__shareName = 'alice'`);
  await call(A, `c.startShare({ audio: false, profile: 'text' })`);
  const shot = await screenShot(B, devA);
  const near = (a, b) => a && a.every((x, i) => Math.abs(x - b[i]) < 60);
  check('H20 screen share arrives and decrypts: the pattern has the right colours (synthetic screen)', shot.present && near(shot.red, [224, 32, 32]) && near(shot.green, [32, 192, 64]), J(shot));
  check('H21 media really crossed the TLS port', tlsIn() > 100000, tlsIn() + ' bytes in');
  await call(A, `c.leave()`).catch(() => { }); await call(B, `c.leave()`).catch(() => { });
} catch (e) { failed++; console.log('FAIL  stopped early: ' + (e.stack || e)); }
offline();
stopTerminator();
await sleep(400);
const left = execFileSync('ss', ['-ltnH'], { encoding: 'utf8' }).split('\n').filter(l => new RegExp(`:(${[0, 1, 3, 4, 5].map(i => BASE + i).join('|')})\\s`).test(l));
check('H22 go-offline leaves nothing listening', left.length === 0, left.join(';'));
stopAll();
console.log(failed ? `${failed} FAILED` : 'ALL PASSED');
process.exit(failed ? 1 : 0);
