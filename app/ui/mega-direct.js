'use strict';
// mega: calling one person directly.
//
// A one-to-one call is a hidden two-person community made by the core (see crates/core/src/direct.rs): "call" on a person's row
// asks the core for it, and the core either rings them straight away (they are already in it) or sends their app an invite inside
// a room you share, and rings the moment their app has joined. Nothing here is shown among the communities, the home cards, "what's
// new" or the dots: the core keeps them out of S.snap.communities and lists them in S.snap.direct, which only this file reads.
// The "direct" area is a small list inside the People drawer, each entry named after the other person. The incoming call is the
// ordinary ring card (mega-ring.js); the call itself is the ordinary call (join, tiles, dock, end-to-end encryption).
//
// Honest limits, said in the interface: an older mega ignores the invite, so a call to someone on one is never answered; a ring
// nobody answers ends after 30 seconds, and then so does the call.
(() => {
  const TTL = 30;
  const D = { open: false, calls: new Map(), sig: '', timer: 0, stats: { calls: 0, noanswer: 0 } };
  const esc = s => String(s == null ? '' : s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const now = () => Date.now() / 1000;
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const toast_ = m => { try { toast(m); } catch { } };
  const phone = '<svg class="dr-ic" viewBox="0 0 20 20" aria-hidden="true" focusable="false"><path d="M5.2 3.5h2.6l1.2 3.4-1.7 1.2a9 9 0 0 0 4.1 4.1l1.2-1.7 3.4 1.2v2.6a1.6 1.6 0 0 1-1.7 1.6A12.6 12.6 0 0 1 3.6 5.2 1.6 1.6 0 0 1 5.2 3.5z"/></svg>';
  const callObj = () => window.__commonsCall;
  const myCall = () => { try { const c = callObj(); return c && c.gid && c.phase !== 'idle' && c.phase !== 'error' ? c : null; } catch { return null; } };
  const list = () => (typeof S !== 'undefined' && S.snap && S.snap.direct) || [];
  const refused = () => (typeof S !== 'undefined' && S.snap && S.snap.direct_refused) || [];
  const ago = t => {
    if (!t) return 'no call yet';
    const s = Math.max(0, now() - t);
    return s < 60 ? 'just now' : s < 3600 ? Math.round(s / 60) + ' min ago' : s < 86400 ? Math.round(s / 3600) + ' h ago' : Math.round(s / 86400) + ' d ago';
  };

  // ---------------------------------------------------------------- calling
  async function waitForRoom(room) {
    for (let i = 0; i < 40; i++) {
      const e = list().find(x => x.room === room);
      if (e) return e;
      try { if (typeof tick === 'function') await tick(); } catch { }
      await sleep(250);
    }
    return null;
  }
  async function call(peer, name) {
    if (myCall()) { toast_('Leave your current call first.'); return false; }
    let r;
    try { r = await rpc('direct_call', { peer }); } catch (e) { toast_(e.message); return false; }
    D.stats.calls++;
    D.calls.set(r.room, { peer, name: name || peer.slice(0, 6), until: now() + TTL + 3, started: now() });
    toast_(r.invited
      ? `calling ${name || 'them'}. This only works if they have a newer mega open; it ends in 30 seconds if nobody answers.`
      : `calling ${name || 'them'}…`);
    const e = await waitForRoom(r.room);
    if (!e) { D.calls.delete(r.room); toast_('The call room is not ready yet. Try again in a moment.'); return false; }
    try { if (window.__callUI && window.__callUI.joinRoom) window.__callUI.joinRoom(r.room, name || e.name); else toast_('Calls are not available in this window.'); } catch (er) { toast_(er.message); }
    redraw();
    return true;
  }
  // The ring runs 30 seconds. Nobody came: say so, and end the call we started.
  function sweep() {
    const t = now(), mine = myCall();
    for (const [room, c] of [...D.calls]) {
      let here = false; try { here = (callPeople(room) || []).some(p => p.account === c.peer); } catch { }
      if (here) { D.calls.delete(room); continue; }
      if (!mine || mine.gid !== room) { if (t - c.started > 3) D.calls.delete(room); continue; }   // we left: nothing more to wait for
      if (t >= c.until) {
        D.calls.delete(room); D.stats.noanswer++;
        toast_(`${c.name} did not answer. If their mega is older or closed it cannot ring.`);
        try { mine.leave(); } catch { }
      }
    }
  }

  // ---------------------------------------------------------------- the People list
  function rowButton(m) {
    if (!m || m.me) return '';
    const label = `Call ${m.name} directly`;
    return `<button class="mini dr-row" data-a="dcall" data-acct="${esc(m.account)}" data-n="${esc(m.name)}" title="${esc(label)}" aria-label="${esc(label)}">${phone}</button>`;
  }
  const areaHtml = () => {
    const items = list(), no = refused();
    const rows = items.map(d => `<div class="dr-item" data-d="${esc(d.id)}"><span class="dr-nm"><b>${esc(d.name)}</b><small>${esc(ago(d.last))}${d.supported === false ? ' · older mega, cannot take calls' : ''}</small></span><span class="dr-btns"><button class="mini dr-b" data-dr="call" data-acct="${esc(d.peer)}" data-n="${esc(d.name)}" aria-label="Call ${esc(d.name)}">call</button><button class="mini dr-b dim" data-dr="stop" data-acct="${esc(d.peer)}" data-n="${esc(d.name)}" aria-label="Stop calls from ${esc(d.name)}">stop calls</button></span></div>`).join('');
    const stopped = no.map(d => `<div class="dr-item stopped"><span class="dr-nm"><b>${esc(d.name)}</b><small>calls stopped</small></span><span class="dr-btns"><button class="mini dr-b" data-dr="allow" data-acct="${esc(d.account)}" aria-label="Allow calls from ${esc(d.name)}">allow</button></span></div>`).join('');
    return `<div class="dr-list">${rows || '<p class="dr-none">No direct calls yet. Press the phone on a person to call them.</p>'}${stopped}</div>
      <p class="dr-note">Direct calls are private between the two of you. They need a recent mega on both sides: an older one never rings. A call nobody answers ends after 30 seconds.</p>`;
  };
  function areaClick(e) {
    const b = e.target.closest('button[data-dr]'); if (!b) return;
    const a = b.dataset.acct, n = b.dataset.n || '';
    if (b.dataset.dr === 'call') call(a, n);
    else if (b.dataset.dr === 'stop') rpc('direct_refuse', { account: a, on: true }).then(() => { toast_(`Calls from ${n || 'them'} are stopped.`); try { tick(); } catch { } }).catch(er => toast_(er.message));
    else if (b.dataset.dr === 'allow') rpc('direct_refuse', { account: a, on: false }).then(() => { try { tick(); } catch { } }).catch(er => toast_(er.message));
  }
  // Called at the end of every People render: a small "direct" switch in the header and, when open, the list under it.
  function decorate(el) {
    if (!el) return;
    const head = el.querySelector('.p-head'); if (!head) return;
    const n = list().length;
    if (!head.querySelector('.dr-open')) {
      const b = document.createElement('button');
      b.type = 'button'; b.className = 'mini dr-open'; b.setAttribute('aria-expanded', D.open ? 'true' : 'false');
      b.title = 'Direct calls: private calls with one person'; b.textContent = n ? `direct ${n}` : 'direct';
      b.onclick = () => { D.open = !D.open; b.setAttribute('aria-expanded', D.open ? 'true' : 'false'); show(el); };
      head.insertBefore(b, head.lastElementChild);
    }
    show(el);
  }
  function show(el) {
    let a = el.querySelector('.dr-area');
    if (!D.open) { if (a) a.remove(); return; }
    if (!a) { a = document.createElement('div'); a.className = 'dr-area'; a.setAttribute('aria-label', 'direct calls'); a.addEventListener('click', areaClick); el.querySelector('.p-head').after(a); }
    const h = areaHtml();
    if (a.dataset.h !== h) { a.innerHTML = h; a.dataset.h = h; }
  }
  function redraw() {
    try { const el = document.getElementById('people'); if (el && !el.hidden && D.open) show(el); } catch { }
  }
  D.timer = setInterval(() => { sweep(); redraw(); }, 1000);
  window.__megaDirect = { rowButton, decorate, call, sweep, state: () => ({ open: D.open, calls: [...D.calls.keys()], stats: D.stats }), list };
})();
