'use strict';
// mega: ringing a voice room.
//
// Sending: while you are in a voice room's call, the dock has a ring button (rings everyone in the community who is not in the
// call) and each person's row in the People list has one (rings just them). Both are the core's `ring_send`: one end-to-end
// encrypted message in the room's own group, which an older app skips as "a newer version" and nothing else.
//
// Receiving: the core has already enforced the rules (a current member, fresh, not too many, not from you or your other devices),
// so what reaches `ring_poll` is only shown here: a full-width card with who, which room, join and decline. It goes away when it is
// answered, declined, taken back, or after 30 seconds. Joining is an ordinary click on join, so it is the normal encrypted join.
// A sound plays only if the app's sound setting is not "off", and a system notification only if the page is in the background and
// the person has already allowed notifications. Nothing here reaches a person whose mega is closed.
(() => {
  const TTL = 30, TONE_EVERY = 2600;
  const R = {
    cards: new Map(),            // ring id -> card
    declined: new Set(),
    outs: new Map(),             // '*' or an account -> { gid, id, until }: rings we sent that are still ringing
    since: 0, busy: false, timer: 0, toneAt: 0, drawSig: '',
    stats: { shown: 0, sounds: 0, notified: 0, polls: 0 },
  };
  const $r = id => document.getElementById(id);
  const esc = s => String(s == null ? '' : s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const now = () => Date.now() / 1000;
  const bell = '<svg class="rg-ic" viewBox="0 0 20 20" aria-hidden="true" focusable="false"><path d="M5 14V9a5 5 0 0 1 10 0v5l1.5 1.5h-13zM8.5 17.5h3"/></svg>';
  const callObj = () => window.__commonsCall;
  const myCall = () => { try { const c = callObj(); return c && c.gid && c.phase !== 'idle' && c.phase !== 'error' ? c : null; } catch { return null; } };
  const comms = () => (typeof S !== 'undefined' && S.snap ? S.snap.communities.concat(S.snap.direct || []) : []);   // a direct call's room is found here only, never listed
  const findRoom = gid => { for (const c of comms()) { const ch = c.channels.find(x => x.gid === gid); if (ch) return { c, ch }; } return null; };
  const voiceCall = () => { const c = myCall(); if (!c) return null; const f = findRoom(c.gid); return f && f.ch.kind === 'voice' ? { gid: c.gid, ...f, call: c } : null; };
  const inCall = (gid, acct) => { try { return (callPeople(gid) || []).some(p => p.account === acct); } catch { return false; } };
  const toast_ = m => { try { toast(m); } catch { } };
  const hasSound = () => { try { return soundPref() !== 'off'; } catch { return false; } };
  const redock = () => { try { if (window.__callUI) window.__callUI.render(); } catch { } };

  // ---------------------------------------------------------------- sending
  async function send(key, gid, to) {
    const out = R.outs.get(key);
    if (out) { R.outs.delete(key); redock(); redraw(); try { await rpc('ring_end', { gid: out.gid, id: out.id, to: to || '' }); } catch { } return false; }
    try {
      const r = await rpc('ring_send', { gid, to: to || '' });
      R.outs.set(key, { gid, id: r.id, to: to || '', until: now() + TTL });
      redock(); redraw(); return true;
    } catch (e) { toast_(e.message); return false; }
  }
  async function ringAll() {
    const v = voiceCall(); if (!v) return;
    if (await send('*', v.gid, '')) toast_(`ringing everyone in ${v.c.name} who is not in the call. People with mega closed are not reached.`);
  }
  async function ringOne(acct, name) {
    const v = voiceCall(); if (!v || inCall(v.gid, acct)) return;
    if (v.c.id !== S.cid) return;
    if (await send(acct, v.gid, acct)) toast_(`ringing ${name || 'them'}. If their mega is closed they will not hear it.`);
  }
  // The ring button in the dock: only inside a voice room's call.
  function dockButton(s) {
    const v = voiceCall(); if (!v || v.c.direct || s.phase !== 'connected') return '';   // a direct call has nobody else to ring
    const on = R.outs.has('*');
    const label = on ? 'Stop ringing' : 'Ring everyone in this community who is not in the call';
    return `<button type="button" class="cd-b${on ? ' on' : ''}" id="c-ring" title="${esc(label)}" aria-label="${esc(label)}" aria-pressed="${on ? 'true' : 'false'}">${bell}</button>`;
  }
  // The ring button on a person's row of the People list: only in a voice room's call, only for someone who is not in it.
  function rowButton(m) {
    const v = voiceCall(); if (!v || m.me || v.c.id !== S.cid || inCall(v.gid, m.account)) return '';
    const on = R.outs.has(m.account);
    const label = on ? `Stop ringing ${m.name}` : `Ring ${m.name} into ${v.ch.name}`;
    return `<button class="mini rg-row${on ? ' on' : ''}" data-a="ring" data-acct="${esc(m.account)}" data-n="${esc(m.name)}" title="${esc(label)}" aria-label="${esc(label)}" aria-pressed="${on ? 'true' : 'false'}">${bell}</button>`;
  }

  // ---------------------------------------------------------------- receiving
  const nameOf = async (cid, acct) => {
    try { const ms = await rpc('members', { cid }); const m = ms.find(x => x.account === acct); if (m) return m.name; } catch { }
    return acct.slice(0, 6);
  };
  async function take(ev) {
    if (ev.end) { for (const [k, c] of [...R.cards]) if (c.ring === ev.ring && c.account === ev.account) R.cards.delete(k); return; }
    if (R.cards.has(ev.ring) || R.declined.has(ev.ring)) return;
    const f = findRoom(ev.gid); if (!f) return;
    const c0 = myCall(); if (c0 && c0.gid === ev.gid) return;               // already in this call: nothing to answer
    const age = Math.max(0, now() - ev.ts);
    if (age >= TTL) return;
    const card = { ring: ev.ring, gid: ev.gid, cid: f.c.id, oneToOne: !!f.c.direct, room: f.ch.name, comm: f.c.name, account: ev.account, name: ev.account.slice(0, 6), direct: !!ev.direct, until: now() + TTL - age };
    R.cards.set(ev.ring, card);
    card.name = await nameOf(f.c.id, ev.account);
    R.stats.shown++;
    notify(card);
  }
  function notify(card) {
    if (document.visibilityState === 'visible') return;
    try {
      if (!('Notification' in window) || Notification.permission !== 'granted') return;
      const n = new Notification(card.direct ? `${card.name} is calling you` : `${card.name} is calling in ${card.room}`, { body: `${card.room} · ${card.comm}`, tag: 'mega-ring-' + card.ring, requireInteraction: false });
      n.onclick = () => { try { window.focus(); n.close(); } catch { } };
      setTimeout(() => { try { n.close(); } catch { } }, TTL * 1000);
      R.stats.notified++;
    } catch { }
  }
  function tone() {
    if (!hasSound() || window.__megaRingSilent) return;
    try {
      if (typeof chimeCtx === 'undefined' || !chimeCtx || chimeCtx.state !== 'running') return;
      const c = chimeCtx, t = c.currentTime, g = c.createGain(); g.connect(c.destination);
      g.gain.setValueAtTime(0.0001, t); g.gain.exponentialRampToValueAtTime(0.08, t + 0.02); g.gain.exponentialRampToValueAtTime(0.0001, t + 0.9);
      [[740, 0], [988, 0.16], [740, 0.5], [988, 0.66]].forEach(([f, d]) => { const o = c.createOscillator(); o.type = 'sine'; o.frequency.value = f; o.connect(g); o.start(t + d); o.stop(t + d + 0.3); });
      R.stats.sounds++;
    } catch { }
  }
  async function poll() {
    if (R.busy || typeof S === 'undefined' || !S.snap) return;
    R.busy = true;
    try {
      R.stats.polls++;
      const r = await rpc('ring_poll', { since: R.since });
      if (r && r.events) for (const ev of r.events) { R.since = Math.max(R.since, ev.id); await take(ev); }
    } catch { }
    finally { R.busy = false; }
    sweep();
  }
  // Expiry, answering elsewhere, and our own rings ending when we leave the call.
  function sweep() {
    const t = now(), mine = myCall();
    let changed = false;
    for (const [k, c] of [...R.cards]) {
      if (t >= c.until || (mine && mine.gid === c.gid) || !findRoom(c.gid)) { R.cards.delete(k); changed = true; }
    }
    for (const [k, o] of [...R.outs]) {
      if (t >= o.until) { R.outs.delete(k); changed = true; continue; }
      if (!mine || mine.gid !== o.gid) { R.outs.delete(k); changed = true; rpc('ring_end', { gid: o.gid, id: o.id, to: o.to || '' }).catch(() => { }); continue; }
      if (k !== '*' && inCall(o.gid, k)) { R.outs.delete(k); changed = true; rpc('ring_end', { gid: o.gid, id: o.id, to: o.to || '' }).catch(() => { }); }   // they came: stop
    }
    if (changed) redock();
    redraw();
    if (R.cards.size && Date.now() - R.toneAt >= TONE_EVERY) { R.toneAt = Date.now(); tone(); }
  }

  // ---------------------------------------------------------------- the card
  function host() {
    let h = $r('mega-ring');
    if (!h) {
      h = document.createElement('div'); h.id = 'mega-ring'; h.setAttribute('role', 'region'); h.setAttribute('aria-label', 'incoming calls'); h.setAttribute('aria-live', 'assertive');
      document.body.appendChild(h);
      h.addEventListener('click', e => {
        const b = e.target.closest('button[data-r]'); if (!b) return;
        const c = R.cards.get(b.dataset.ring); if (!c) return;
        if (b.dataset.r === 'join') {
          R.cards.delete(c.ring); redraw();
          try { if (window.__callUI && window.__callUI.joinRoom) window.__callUI.joinRoom(c.gid, c.oneToOne ? c.name : c.room); else toast_('Calls are not available in this window.'); } catch (er) { toast_(er.message); }
        } else if (b.dataset.r === 'no') { R.declined.add(c.ring); R.cards.delete(c.ring); redraw(); }
        else if (b.dataset.r === 'allow') { try { Notification.requestPermission().then(() => redraw()); } catch { } }
      });
    }
    return h;
  }
  const colours = gid => {
    let t = null; try { t = window.__mega && __mega.callTheme && __mega.callTheme(gid); } catch { }
    t = t || ['#1f3d36', '#b6ff5c'];
    let ink = '#efedf2'; try { ink = __mega.inkOn(t[0]); } catch { }
    return [t[0], t[1], ink];
  };
  function redraw() {
    const h = host();
    const list = [...R.cards.values()].slice(0, 3);
    const canAsk = (() => { try { return 'Notification' in window && Notification.permission === 'default'; } catch { return false; } })();
    const sig = JSON.stringify([list.map(c => [c.ring, c.name]), canAsk]);
    if (sig === R.drawSig) return;
    R.drawSig = sig;
    h.hidden = !list.length;
    h.innerHTML = list.map(c => {
      const [a, b, ink] = colours(c.gid);
      return `<article class="rg-card" data-ring="${esc(c.ring)}" style="--rg-a:${a};--rg-b:${b};--rg-ink:${ink}">
        <div class="rg-who"><small>${c.direct ? 'calling you' : 'ringing the room'}</small><b>${esc(c.name)}</b><span>${c.oneToOne ? 'direct call' : `${esc(c.room)} · ${esc(c.comm)}`}</span></div>
        <div class="rg-acts">${canAsk ? `<button type="button" class="rg-link" data-r="allow" data-ring="${esc(c.ring)}">notify me when away</button>` : ''}<button type="button" class="rg-b no" data-r="no" data-ring="${esc(c.ring)}" aria-label="Decline the call from ${esc(c.name)}">decline</button><button type="button" class="rg-b yes" data-r="join" data-ring="${esc(c.ring)}" aria-label="Join ${esc(c.room)}, called by ${esc(c.name)}">join</button></div>
      </article>`;
    }).join('');
  }

  window.__megaRing = { rowButton, dockButton, ringAll, ringOne, poll, sweep, cards: R.cards, outs: R.outs, stats: R.stats, get since() { return R.since; } };
  R.timer = setInterval(poll, 1000);
  document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible') poll(); });
})();
