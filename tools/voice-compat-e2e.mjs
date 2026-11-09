// Mixed-version calls: an older client (the app/ui of release 0.3.0, from git) and the current one in the same call.
// Real relay, real LiveKit, real keys, in a muted headless browser with synthetic audio only. Both directions must be heard,
// frame encryption must stay on, and after the server restarts both must find each other again.
//   systemd-run --user --scope -q -p MemoryMax=8G node tools/voice-compat-e2e.mjs [outdir]
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { startServers, sleep, stopAll, lkAdmin, root } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';

const out = process.argv[2] || '/tmp/commons-voice-compat';
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const OLD_REF = process.env.COMMONS_OLD_REF || 'bf0a893';   // Release 0.3.0
const oldUi = path.join(out, 'old');
fs.mkdirSync(oldUi, { recursive: true });
execFileSync('sh', ['-c', `git -C '${root}' archive ${OLD_REF} app/ui | tar -x -C '${oldUi}'`]);

let failed = 0;
const check = (name, ok, detail = '') => { if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? '  [' + detail + ']' : ''}`); };
const J = x => JSON.stringify(x);
async function until(fn, ms = 20000, what = 'condition') { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(250); } }

let world, lk;
try {
  lk = await startServers(out, { relayTtl: 900 });
  const names = ['old', 'neo'];
  world = await makeWorld(out, names, { ui: { old: path.join(oldUi, 'app/ui') } });
  await openApps(world, names);
  const P = n => world.people[n], page = n => P(n).page, dev = n => P(n).device;
  const call = (n, expr) => page(n).ev(`(async()=>{ const c = window.__commonsCall; return ${expr}; })()`);
  const hears = async (l, s) => {
    await until(() => call(l, `[...c.tracks.values()].some(x => x.identity === ${J(dev(s))} && x.source === 'microphone')`), 20000, `${l} has ${s}`).catch(() => { });
    for (let i = 0; i < 6; i++) { await sleep(600); const r = await call(l, `c.probeTone(${J(dev(s))}, ${P(s).tone}, 1200, 'microphone')`); if (r.present && r.toneShare > 0.15) return true; await sleep(800); }
    return false;
  };
  const versionOf = n => page(n).ev(`typeof CommonsCall.prototype._subscribeAll`);
  check('the two clients really are two versions', (await versionOf('old')) === 'undefined' && (await versionOf('neo')) === 'function', `${await versionOf('old')} / ${await versionOf('neo')}`);

  // the older client is in the room first
  await call('old', `c.join(S.gid, 'general')`); await call('neo', `c.join(S.gid, 'general')`);
  await until(async () => (await call('old', `c.room.remoteParticipants.size`)) === 1 && (await call('neo', `c.room.remoteParticipants.size`)) === 1, 15000, 'both in the call');
  check('C1 mixed call: both report frame encryption active', (await call('old', `c.room.isE2EEEnabled`)) && (await call('neo', `c.room.isE2EEEnabled`)));
  const room = (await call('old', `c.room.name`));
  const parts = (await lkAdmin('ListParticipants', { room })).participants || [];
  check('C2 the server sees only encrypted tracks from both', parts.length === 2 && parts.every(p => (p.tracks || []).length && p.tracks.every(t => t.encryption === 'GCM' || t.encryption === 1)), J(parts.map(p => (p.tracks || []).map(t => t.encryption))));
  check('C3 the new client hears the old one', await hears('neo', 'old'));
  check('C4 the old client hears the new one', await hears('old', 'neo'));

  // the new client leaves and comes back while the old one stays
  await call('neo', `c.leave()`); await sleep(1500);
  await call('neo', `c.join(S.gid, 'general')`);
  await until(async () => (await call('old', `c.room.remoteParticipants.size`)) === 1 && (await call('neo', `c.room.remoteParticipants.size`)) === 1, 15000, 'neo back');
  check('C5 after the new client rejoins, it hears the old one again', await hears('neo', 'old'));
  check('C6 and the old one hears it', await hears('old', 'neo'));

  // a server restart: each client heals in its own way, then they must hear each other again
  await lk.stopLivekit(); await sleep(1000); lk.startLivekit();
  await until(async () => (await call('old', `c.phase`)) === 'connected' && (await call('neo', `c.phase`)) === 'connected' && (await call('old', `c.room && c.room.remoteParticipants.size`)) === 1 && (await call('neo', `c.room && c.room.remoteParticipants.size`)) === 1, 90000, 'both rejoin after the restart');
  await sleep(2000);
  check('C7 after a server restart the new client hears the old one', await hears('neo', 'old'));
  check('C8 after a server restart the old client hears the new one', await hears('old', 'neo'));
  check('C9 encryption still on after the restart', (await call('old', `c.room.isE2EEEnabled`)) && (await call('neo', `c.room.isE2EEEnabled`)));
  await call('old', `c.leave()`); await call('neo', `c.leave()`);
} catch (e) { console.error('RUN ABORTED:', e); failed++; }
finally { stopAll(); console.log(`\n${failed} failed. Output in ${out}`); process.exitCode = failed ? 1 : 0; }
