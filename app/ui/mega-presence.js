'use strict';
// mega: live presence on the cards. Friends who are looking at the card screens show up as small dots in their colour on the card
// each one has focused, and fade out shortly after they leave or go quiet. Nothing here is ever stored: it travels only on the
// relay's unstored live channel (live.rs), see "Presence on the cards" in docs/THREAT-MODEL.md.
//
// Sending (all of it goes through mega-live.js, so it is counted in window.__megaLive.stats):
//  - only when the core says live events really travel on the unstored channel (rpc live_chan). On the log fallback every event would
//    be a permanent entry, so nothing is sent at all;
//  - only when the focused card (a settled one, and the same for 0.7 s) changes, at most two events a second, plus one heartbeat every
//    20 s while you stay on the card screens with the tab visible. Nothing while the tab is hidden or in a chat; the others drop you after 45 s;
//  - one event goes to one room of one community (the lowest-id text room that takes it), so another community never learns which card
//    you look at here. The event names the community and the card (a room id, or "" for the community's own card);
//  - the switch in the options room ("show where I am on the cards", on by default) stops all of it.
// Receiving: every field is untrusted. The sender is a member the core has already verified; the community must be one we are in
// (and the room one of its rooms), the account a known member of it, and the data a small object. A card shows 6 dots at most, then "+N".
(() => {
  const HOLD = 700, GAP = 520, BEAT = 20000, GONE = 45000, TICK = 400, SHOW = 6, KIND = 'pres.f';
  const root = document.documentElement;
  const $q = (s, r = document) => r.querySelector(s);
  const P = {
    on: true, cand: '', candAt: 0, want: null, sent: null, lastSend: 0, busy: false,
    peers: new Map(),                 // account -> { c, r, at, name }
    names: new Map(),                 // cid -> { at, m: Map(account -> name) }
    chan: { at: 0, ok: false },
    stats: { sent: 0, beats: 0, offs: 0, got: 0, bad: 0 },
    shown: new Map(),                 // card element -> Set of accounts it shows (for the drift-in of new dots)
  };
  const prefGet = () => { try { return localStorage.getItem('commons.presence') !== 'off'; } catch { return true; } };
  const prefSet = on => { try { if (on) localStorage.removeItem('commons.presence'); else localStorage.setItem('commons.presence', 'off'); } catch { } };
  P.on = prefGet();
  const M = () => window.__mega;
  const comms = () => (typeof S !== 'undefined' && S.snap ? S.snap.communities : []);
  const comm = cid => comms().find(c => c.id === cid && !c.removed);
  const meAcc = () => (typeof S !== 'undefined' && S.snap && S.snap.me ? S.snap.me.account : '');
  const isId = s => typeof s === 'string' && /^[0-9a-f]{8,128}$/.test(s);
  const still = () => root.classList.contains('fx-lite') || (window.matchMedia && matchMedia('(prefers-reduced-motion: reduce)').matches);

  // ---------------------------------------------------------------- what I am looking at
  /** The settled card on a card screen as { c, r }, null when it is not a community card or a room of one (or I am in a chat), undefined while it is still moving. */
  function current() {
    const m = M();
    if (!m || !m.started || (m.level !== 'home' && m.level !== 'server') || !m.list) return null;
    if (!m.list.settled) return undefined;
    const x = m.list.item; if (!x || !comm(x.cid)) return null;
    if (x.kind === 'community') return { c: x.cid, r: '' };
    if ((x.kind === 'text' || x.kind === 'voice') && x.gid) return { c: x.cid, r: x.gid };
    return null;
  }
  const keyOf = t => (t ? t.c + '|' + t.r : '');

  async function chanOk() {
    const now = Date.now();
    if (now - P.chan.at < 5000) return P.chan.ok;
    P.chan.at = now;
    try { const r = await rpc('live_chan', {}); P.chan.ok = !!(r && r.chan === true); } catch { P.chan.ok = false; }
    return P.chan.ok;
  }
  /** The rooms of a community that can carry presence, in the same order on every device. */
  const carriers = cid => ((comm(cid) || {}).channels || []).filter(x => x.kind !== 'voice' && x.joined).map(x => x.gid).sort();
  async function emit(cid, body) {
    const L = window.__megaLive; if (!L) return false;
    const data = JSON.stringify({ v: 1, c: cid, ...body });
    for (const g of carriers(cid).slice(0, 3)) {
      let ok = false; try { ok = await L.send(g, KIND, data); } catch { }
      if (ok) { P.stats.sent++; if (body.o) P.stats.offs++; return true; }
    }
    return false;
  }

  async function tick() {
    if (P.busy) return;
    if (document.visibilityState !== 'visible') { P.cand = ''; return; }   // hidden: nothing, not even a goodbye (others drop me after 45 s)
    const cur = current();
    if (cur !== undefined) { const k = keyOf(cur); if (k !== P.cand) { P.cand = k; P.candAt = Date.now(); P.next = cur; } if (Date.now() - P.candAt >= HOLD) P.want = cur; }
    const now = Date.now();
    if (!P.on) { P.sent = null; return; }   // the switch: not one event
    if (now - P.lastSend < GAP) return;
    const want = P.want, sent = P.sent;
    let job = null;
    if (sent && (!want || want.c !== sent.c)) job = { cid: sent.c, body: { o: 1 }, after: () => { P.sent = null; } };
    else if (want && keyOf(want) !== keyOf(sent)) job = { cid: want.c, body: { r: want.r }, after: () => { P.sent = want; } };
    else if (want && now - P.lastSend >= BEAT) job = { cid: want.c, body: { r: want.r }, beat: 1 };
    if (!job) return;
    P.busy = true;
    try {
      if (!(await chanOk())) return;   // the log fallback, an old relay or an old core: never send
      if (document.visibilityState !== 'visible') return;
      if (await emit(job.cid, job.body)) { P.lastSend = Date.now(); if (job.beat) P.stats.beats++; if (job.after) job.after(); }
    } finally { P.busy = false; }
  }

  // ---------------------------------------------------------------- what others do
  async function members(cid, force) {
    let e = P.names.get(cid);
    if (e && !force && Date.now() - e.at < 60000) return e.m;
    if (e && Date.now() - e.at < 8000) return e.m;
    const m = new Map(); e = { at: Date.now(), m }; P.names.set(cid, e);
    try { const r = await rpc('members', { cid }); for (const x of Array.isArray(r) ? r : []) if (x && typeof x.account === 'string' && typeof x.name === 'string') m.set(x.account, x.name.slice(0, 40)); } catch { }
    return m;
  }
  const cidOfGid = g => { const c = comms().find(c => !c.removed && c.channels.some(x => x.gid === g)); return c ? c.id : null; };
  async function onEvent(e) {
    if (!e || e.kind !== KIND || typeof e.data !== 'string' || e.data.length > 300 || typeof e.account !== 'string' || !e.account || e.account === meAcc()) return;
    const cid = cidOfGid(e.gid); if (!cid) { P.stats.bad++; return; }
    let d; try { d = JSON.parse(e.data); } catch { P.stats.bad++; return; }
    const c = comm(cid);
    if (!d || typeof d !== 'object' || d.v !== 1 || d.c !== cid || !(d.o === 1 || d.o === undefined) || !(d.r === undefined || d.r === '' || (isId(d.r) && c.channels.some(x => x.gid === d.r && x.kind !== 'options')))) { P.stats.bad++; return; }
    const old = P.peers.get(e.account);
    if (d.o === 1) { if (old && old.c === cid) { P.peers.delete(e.account); render(); } return; }
    let name = (await members(cid)).get(e.account);
    if (!name) name = (await members(cid, true)).get(e.account);
    if (!name) { P.stats.bad++; return; }   // not a member of that community as far as we know
    P.stats.got++;
    P.peers.set(e.account, { c: cid, r: d.r || '', at: Date.now(), name });
    render();
  }

  // ---------------------------------------------------------------- the dots
  const hue = s => { let h = 5381; for (const ch of String(s)) h = ((h * 33) ^ ch.codePointAt(0)) >>> 0; return h; };
  const dotColour = name => (M() && M().api && M().api.faceColor ? M().api.faceColor(name) : `hsl(${hue(name) % 360} 78% 74%)`);
  function peopleOn(x) {
    const out = [];
    for (const [a, p] of P.peers) {
      if (p.gone) continue;
      if (x.kind === 'community' ? (p.c === x.cid && p.r === '') : ((x.kind === 'text' || x.kind === 'voice') && p.c === x.cid && p.r === x.gid)) out.push({ a, name: p.name, at: p.at });
    }
    return out.sort((u, v) => u.name.localeCompare(v.name) || (u.a < v.a ? -1 : 1));
  }
  function render() {
    const m = M(), now = Date.now();
    for (const [a, p] of P.peers) if (now - p.at > GONE) P.peers.delete(a);
    if (!m || !m.list || !m.list.el || !m.list.el.isConnected || (m.level !== 'home' && m.level !== 'server')) return;
    const slots = m.list.el.querySelectorAll('.mslot'), items = m.list.items;
    slots.forEach((slot, i) => {
      const x = items[i], sq = slot.querySelector('.msq'); if (!x || !sq) return;
      const ppl = x.kind === 'news' ? [] : peopleOn(x);
      let layer = sq.querySelector(':scope > .mpres');
      if (!ppl.length && !layer) return;
      if (!layer) { layer = document.createElement('div'); layer.className = 'mpres'; layer.setAttribute('aria-hidden', 'true'); sq.appendChild(layer); }
      const shown = ppl.slice(0, SHOW), want = new Set(shown.map(p => p.a));
      for (const d of [...layer.querySelectorAll('.mpd')]) if (!want.has(d.dataset.a)) { if (d.classList.contains('out')) continue; d.classList.add('out'); setTimeout(() => d.remove(), still() ? 0 : 700); }
      shown.forEach((p, k) => {
        let d = layer.querySelector(`.mpd[data-a="${CSS.escape(p.a)}"]:not(.out)`);
        if (!d) {
          d = document.createElement('span'); d.className = 'mpd'; d.dataset.a = p.a;
          const h = hue(p.a); d.style.setProperty('--dx', ((h % 90) - 45 + 'px')); d.style.setProperty('--dy', (((h >> 8) % 60) + 30) + 'px');
          d.style.setProperty('--ph', ((h >> 4) % 7) / 2 + 's');
          layer.appendChild(d);
        }
        d.style.setProperty('--fc', dotColour(p.name)); d.dataset.n = p.name; d.style.order = k;
      });
      let more = layer.querySelector('.mpm');
      if (ppl.length > SHOW) { if (!more) { more = document.createElement('span'); more.className = 'mpm'; layer.appendChild(more); } more.textContent = '+' + (ppl.length - SHOW); more.style.order = 99; } else if (more) more.remove();
    });
    for (const l of m.list.el.querySelectorAll('.mpres')) if (!l.children.length) l.remove();
  }

  // the name on hover: the dots never take the pointer, so the page looks for the pointer over them itself
  let tip = null, tipRaf = 0, px = 0, py = 0;
  function hover() {
    tipRaf = 0;
    let hit = null;
    for (const d of document.querySelectorAll('.mpres .mpd:not(.out)')) { const r = d.getBoundingClientRect(); if (px >= r.left - 3 && px <= r.right + 3 && py >= r.top - 3 && py <= r.bottom + 3) { hit = d; break; } }
    if (!hit) { if (tip) tip.hidden = true; return; }
    if (!tip) { tip = document.createElement('div'); tip.id = 'mega-pres-tip'; tip.setAttribute('aria-hidden', 'true'); document.body.appendChild(tip); }
    const r = hit.getBoundingClientRect();
    tip.textContent = hit.dataset.n || ''; tip.hidden = false;
    tip.style.left = Math.max(6, Math.min(innerWidth - tip.offsetWidth - 6, r.left + r.width / 2 - tip.offsetWidth / 2)) + 'px';
    tip.style.top = Math.max(6, r.top - tip.offsetHeight - 8) + 'px';
  }
  document.addEventListener('pointermove', e => { if (e.pointerType === 'touch') return; px = e.clientX; py = e.clientY; if (!tipRaf && document.querySelector('.mpres')) tipRaf = requestAnimationFrame(hover); }, { passive: true });

  // ---------------------------------------------------------------- wiring
  function start() {
    const L = window.__megaLive; if (!L) return;
    // events are read only while somebody is on the card screens with the tab in front; the rest is dropped by the 45 s rule
    L.sub('presence', () => (document.visibilityState === 'visible' && M() && M().started && (M().level === 'home' || M().level === 'server') ? comms().flatMap(c => c.channels.map(x => x.gid)) : []), e => { onEvent(e); });
    setInterval(() => { tick(); render(); }, TICK);
  }
  window.__megaPres = {
    stats: P.stats,
    get on() { return P.on; },
    set(on) { P.on = !!on; prefSet(P.on); if (!P.on) P.sent = null; },
    peers: () => [...P.peers].map(([a, p]) => ({ a, ...p })),
    state: () => ({ on: P.on, want: P.want, sent: P.sent, cur: current(), chan: P.chan.ok, dots: document.querySelectorAll('.mpres .mpd:not(.out)').length }),
    render, onEvent, tick,
    _inject(list) { P.peers.clear(); for (const x of list) P.peers.set(x.a, { c: x.c, r: x.r, at: Date.now(), name: x.name }); render(); },
    _reset() { P.peers.clear(); P.names.clear(); P.chan.at = 0; render(); },
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start); else start();
})();
