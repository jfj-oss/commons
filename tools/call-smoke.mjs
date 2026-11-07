import path from 'node:path';
import fs from 'node:fs';
import { startServers, sleep, lkAdmin, stopAll, FAKE_DISPLAY, root } from './lib/env.mjs';
import { makeWorld, openApps } from './lib/world.mjs';
const out = process.argv[2] || '/tmp/commons-call-smoke';
fs.rmSync(out, { recursive: true, force: true });
try {
  await startServers(out);
  const w = await makeWorld(out, ['alice', 'bob']);
  await openApps(w, ['alice', 'bob']);
  const A = w.people.alice.page, B = w.people.bob.page;
  console.log('support', JSON.stringify(await A.ev(`CommonsCall.support()`)));
  await A.ev(`__commonsCall.join(S.gid, 'room')`);
  console.log('A joined', await A.ev(`__commonsCall.phase`));
  await B.ev(`__commonsCall.join(S.gid, 'room')`);
  await sleep(4000);
  console.log('A snap', JSON.stringify(await A.ev(`(()=>{const s=__commonsCall.snapshot(); return {phase:s.phase,peers:s.peers.map(p=>[p.name,p.speaking,p.muted]),enc:s.encryption,blocked:s.blocked,events:s.events.map(e=>e.msg)}})()`)));
  const bId = w.people.bob.device;
  console.log('A hears B tone', JSON.stringify(await A.ev(`__commonsCall.probeTone(${JSON.stringify(bId)}, ${w.people.bob.tone})`)));
  console.log('server participants', JSON.stringify((await lkAdmin('ListParticipants', { room: (await A.ev(`__commonsCall.tracks.size`)) && undefined })).participants?.length));
  console.log('A console', A.console.slice(-10));
} catch (e) { console.error('FAILED', e); process.exitCode = 1; }
finally { stopAll(); }
