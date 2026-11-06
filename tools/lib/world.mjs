// Builds a small world: named people, a community with one room, everyone joined, each with a Chromium page open on their app.
import path from 'node:path';
import { startBridge, startChrome, openPage, rpc, sleep, RELAY_PORT } from './env.mjs';

export async function makeWorld(out, names, { tones = {}, extraChrome = [] } = {}) {
  const w = { out, people: {} };
  let i = 0;
  for (const n of names) { const port = 27200 + i; startBridge(out, n, port); w.people[n] = { name: n, port, dbg: 29400 + i, tone: tones[n] || 400 + 100 * (i + 1) }; i++; }
  await sleep(1500);
  const rel = `http://127.0.0.1:${RELAY_PORT}`;
  for (const n of names) await rpc(w.people[n].port, 'create_account', { name: n[0].toUpperCase() + n.slice(1), passphrase: 'correct horse battery', relay: rel });
  const first = w.people[names[0]];
  const snapA = await rpc(first.port, 'snapshot');
  w.cid = (await rpc(first.port, 'create_community', { name: 'Test club' })).id;
  const link = (await rpc(first.port, 'create_invite', { cid: w.cid, uses: 10, ttl: 3600 })).link;
  for (const n of names.slice(1)) await rpc(w.people[n].port, 'join', { link });
  // wait for everybody to hold a joined room
  const t0 = Date.now();
  for (;;) {
    let all = true;
    for (const n of names) { const s = await rpc(w.people[n].port, 'snapshot'); const c = s.communities.find(c => c.id === w.cid); const ch = c && c.channels.find(x => x.joined && x.status === 'ok'); if (!ch) all = false; else w.gid = ch.gid; w.people[n].account = s.me.account; w.people[n].device = s.me.device; }
    if (all) break; if (Date.now() - t0 > 60000) throw new Error('world did not assemble'); await sleep(700);
  }
  for (const n of names) {
    const p = w.people[n];
    p.chrome = await startChrome(out, n, p.dbg, p.tone, extraChrome);
  }
  return w;
}
export async function openApps(w, names, opts = {}) {
  for (const n of names) {
    const p = w.people[n];
    p.page = await openPage(p.chrome, `http://127.0.0.1:${p.port}/`, n, opts);
    await p.page.waitFor(`!!window.__commonsCall && !document.querySelector("#app").hidden && !!S.gid`, 30000, "app ready").catch(async e => { console.log(p.page.console, (p.page.net || []).filter(x => !x.includes('/rpc')).join('\n'), await p.page.ev(`JSON.stringify({cc: !!window.__commonsCall, hid: document.querySelector("#app").hidden, boot: document.querySelector("#boot").innerText.slice(0,100), url: location.href, scripts: [...document.scripts].map(s => s.src.split("/").pop()).join(), ccl: typeof CommonsCall, lk: typeof LivekitClient, res: performance.getEntriesByType("resource").map(r => r.name.split("/").pop() + ":" + r.responseStatus + ":" + Math.round(r.duration)).join(), gid: typeof S !== "undefined" && S.gid})`).catch(x => String(x))); throw e; });
  }
}
