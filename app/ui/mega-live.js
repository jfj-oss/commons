'use strict';
// mega: one place that sends and reads the live channel (live_send / live_poll) for the camera stack and the shared wall.
//
// Every live event is an ordinary entry in the relay's log, kept for good and counted against the sender, and each one makes
// the sender's device rewrite its encrypted state. So nothing in mega sends per pointer move or per frame: callers batch,
// stop when idle, and every send goes through here so it is counted (window.__megaLive.stats, read by the tests).
(() => {
  const L = {
    stats: { sent: 0, kinds: {}, polls: 0 },
    subs: new Map(),   // name -> { gid: () => string|null, on: (ev) => void }
    since: 0,
    timer: 0,
    busy: false,
    /** Count an event that was sent some other way (a stroke goes through its own call), so the tests see every one. */
    count(kind) { L.stats.sent++; L.stats.kinds[kind] = (L.stats.kinds[kind] || 0) + 1; },
    /** Send one event. Resolves to true when the core took it (false: the room cannot encrypt right now, or it was dropped). */
    async send(gid, kind, data) {
      const text = typeof data === 'string' ? data : JSON.stringify(data);
      if (text.length > 3900) return false;
      L.stats.sent++; L.stats.kinds[kind] = (L.stats.kinds[kind] || 0) + 1;
      try { const r = await rpc('live_send', { gid, kind, data: text }); return !!(r && r.sent); } catch { return false; }
    },
    /** Subscribe to events; `gids` says which rooms are worth asking about right now. Polling runs only while someone is subscribed to a room. */
    sub(name, gids, on) { L.subs.set(name, { gids, on }); L.kick(); },
    unsub(name) { L.subs.delete(name); },
    kick() { if (!L.timer) L.timer = setInterval(L.poll, 700); },
    async poll() {
      if (L.busy || document.visibilityState !== 'visible') return;
      const want = new Set(); for (const s of L.subs.values()) for (const g of s.gids() || []) want.add(g);
      if (!want.size) return;
      L.busy = true;
      try {
        const r = await rpc('live_poll', { since: L.since });
        L.stats.polls++;
        for (const e of r.events || []) {
          if (e.id > L.since) L.since = e.id;
          if (!want.has(e.gid)) continue;
          for (const s of L.subs.values()) { try { s.on(e); } catch (er) { console.error(er); } }
        }
        if (r.head > L.since && !(r.events || []).length) L.since = r.head;
      } catch { } finally { L.busy = false; }
    },
  };
  // Start from "now": events from before this page looked are not interesting (they are seconds old at most anyway).
  rpc('live_poll', { since: 0 }).then(r => { L.since = Math.max(L.since, r.head || 0); }).catch(() => { });
  window.__megaLive = L;
})();
