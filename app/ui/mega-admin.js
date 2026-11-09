'use strict';
// mega: the server settings room. Reached from a community's cards; offered only to its owner and admins.
// It draws into the same place as the options room (#mega-opts) but is the community's, not the device's:
// everything here goes through the core's own operations (rename_community, create_channel, rename_channel,
// delete_channel, set_role, kick, unban, revoke_invite, transfer_ownership) and everyone in the community sees the result.
// Each control appears only for the role the core lets use it (same checks as crates/core/src/ops.rs).
(() => {
  const A = { cid: null, members: [], mkey: '', mt: 0, ver: null, cache: new Map(), loading: false, say: null };
  const cid2 = () => A.cid;
  const comm = () => (S.snap ? S.snap.communities.find(c => c.id === A.cid && !c.removed) : null) || null;
  const isOwner = c => c.role === 'owner';
  const isAdmin = c => c.role === 'owner' || c.role === 'admin';
  const plain = er => String((er && er.message) || er || 'something went wrong').replace(/^\[retry\] /, '');
  const when = t => { const s = t - Date.now() / 1000; if (s <= 0) return 'expired'; const d = Math.floor(s / 86400), h = Math.floor(s % 86400 / 3600), m = Math.floor(s % 3600 / 60); return 'expires in ' + (d ? `${d} day${d === 1 ? '' : 's'}` : h ? `${h} hour${h === 1 ? '' : 's'}` : `${Math.max(1, m)} minute${m === 1 ? '' : 's'}`); };
  const uses = n => n == null ? 'unlimited uses' : `${n} use${n === 1 ? '' : 's'} left`;
  const roleWord = r => r === 'owner' ? 'owner' : r === 'admin' ? 'admin' : 'member';

  // what the core lets this role do to this person
  const canRemove = (c, m) => !m.me && m.role !== 'owner' && (isOwner(c) || (isAdmin(c) && m.role === 'member'));
  const canRole = (c, m) => isOwner(c) && !m.me && m.role !== 'owner';

  // ---------------------------------------------------------------- sections (each returns html; refreshed on its own)
  function secName(c) {
    return `<h3>server name</h3><div class="madm-row"><input type="text" id="ma-name" value="${esc(c.name)}" maxlength="60" spellcheck="false" aria-label="Server name" autocomplete="off"><button class="ma" type="button" data-a="rename">save</button></div><p>Everyone in the community sees the new name.</p>`;
  }
  function secRooms(c) {
    const n = c.channels.length;
    const rows = c.channels.map(x => `<div class="madm-item"><span class="madm-nm"><span class="madm-k">${x.kind === 'voice' ? 'voice' : 'text'}</span>${esc(x.name)}</span><span class="madm-acts"><button class="ma" type="button" data-a="room-rename" data-id="${esc(x.gid)}" data-n="${esc(x.name)}" aria-label="Rename ${esc(x.name)}">rename</button>${n > 1 ? `<button class="ma bad" type="button" data-a="room-delete" data-id="${esc(x.gid)}" data-n="${esc(x.name)}" aria-label="Delete ${esc(x.name)}">delete</button>` : ''}</span></div>`).join('');
    return `<h3>rooms</h3>${rows}${n <= 1 ? '<p>A community keeps at least one room, so the last one cannot be deleted.</p>' : ''}
      <div class="madm-row"><input type="text" id="ma-room" placeholder="new room name" maxlength="40" spellcheck="false" aria-label="New room name" autocomplete="off"><span class="madm-seg" role="radiogroup" aria-label="Room kind"><button type="button" role="radio" aria-checked="${A.kind !== 'voice'}" data-a="kind" data-v="text">text</button><button type="button" role="radio" aria-checked="${A.kind === 'voice'}" data-a="kind" data-v="voice">voice</button></span><button class="ma" type="button" data-a="room-add">add room</button></div>`;
  }
  function secMembers(c) {
    const rows = A.members.map(m => {
      const acts = [];
      if (canRole(c, m)) acts.push(`<button class="ma" type="button" data-a="role" data-acct="${esc(m.account)}" data-n="${esc(m.name)}" data-r="${m.role === 'admin' ? 'member' : 'admin'}">${m.role === 'admin' ? 'make member' : 'make admin'}</button>`);
      if (canRemove(c, m)) acts.push(`<button class="ma bad" type="button" data-a="ban" data-acct="${esc(m.account)}" data-n="${esc(m.name)}">remove and ban</button>`);
      return `<div class="madm-item"><span class="madm-nm">${esc(m.name)}${m.me ? '<span class="madm-k">you</span>' : ''}<span class="madm-k ${m.role !== 'member' ? 'on' : ''}">${roleWord(m.role)}</span></span><span class="madm-acts">${acts.join('')}</span></div>`;
    }).join('');
    const note = isOwner(c) ? 'You can make people admins or take that back. Admins can create and rename rooms, remove members, and manage invites.' : 'As an admin you can remove regular members. Only the owner can change roles.';
    return `<h3>members and roles</h3>${rows || '<p>loading…</p>'}<p>${note} Removing someone also bans them: they cannot come back with an old invite.</p>`;
  }
  function secBans(c) {
    const b = c.banned || [];
    return `<h3>banned</h3>${b.length ? b.map(x => `<div class="madm-item"><span class="madm-nm">${esc(x.name)}</span><span class="madm-acts"><button class="ma" type="button" data-a="unban" data-acct="${esc(x.account)}" data-n="${esc(x.name)}">unban</button></span></div>`).join('') : '<p>Nobody is banned.</p>'}${b.length ? '<p>Unbanning only lets them be invited again. They are not added back by themselves.</p>' : ''}`;
  }
  function secInvites(c) {
    const now = Date.now() / 1000, open = (c.invites || []).filter(i => i.expires > now);
    return `<h3>invites</h3>${open.length ? open.map(i => `<div class="madm-item"><span class="madm-nm">${esc(uses(i.uses_left))}<span class="madm-k">${esc(when(i.expires))}</span>${i.mine ? '<span class="madm-k on">yours</span>' : ''}</span><span class="madm-acts"><button class="ma bad" type="button" data-a="revoke" data-id="${esc(i.id)}">revoke</button></span></div>`).join('') : '<p>No open invites.</p>'}
      <div class="madm-row"><button class="ma" type="button" data-a="invite">create an invite</button></div><p>Revoking stops the link working for anyone who has not used it yet.</p>`;
  }
  function secOwner(c) {
    const others = A.members.filter(m => !m.me && m.role !== 'owner');
    return `<h3>hand over ownership</h3><p>Make someone else the owner. You stay on as an admin and can leave afterwards. Only they could hand it back.</p>${others.length ? `<div class="madm-row"><select id="ma-to" aria-label="New owner">${others.map(m => `<option value="${esc(m.account)}" data-n="${esc(m.name)}">${esc(m.name)}${m.role === 'admin' ? ' (admin)' : ''}</option>`).join('')}</select><button class="ma bad" type="button" data-a="transfer">hand over…</button></div>` : '<p>There is nobody else in the community yet.</p>'}`;
  }
  const SECTIONS = [['name', secName, isAdmin], ['rooms', secRooms, isAdmin], ['members', secMembers, isAdmin], ['bans', secBans, isAdmin], ['invites', secInvites, isAdmin], ['owner', secOwner, isOwner]];

  // ---------------------------------------------------------------- drawing
  const status = () => { const s = A.say; return s ? `<div class="madm-say ${s.bad ? 'bad' : ''}" role="${s.bad ? 'alert' : 'status'}">${esc(s.text)}</div>` : ''; };
  function html(cid) {
    if (A.cid !== cid) { A.cid = cid; A.members = []; A.mkey = ''; A.mt = 0; A.say = null; A.kind = 'text'; }
    A.cache.clear();
    const c = comm(); if (!c || !isAdmin(c)) return '';
    loadMembers(true);
    const T = window.__mega.api.ti;
    return `<div class="mopts-in madm mrise">${T('server settings', '', 'h1')}<p>${isOwner(c) ? 'You own' : 'You are an admin of'} <b>${esc(c.name)}</b>. Everyone in it sees these changes.</p><div id="ma-say">${status()}</div>${SECTIONS.map(([k, f, ok]) => ok(c) ? `<section data-s="${k}">${f(c)}</section>` : '').join('')}</div>`;
  }
  const root = () => document.querySelector('#mega-opts .madm');
  function say(text, bad) { A.say = { text, bad: !!bad }; const e = document.getElementById('ma-say'); if (e) e.innerHTML = status(); if (!bad) { clearTimeout(say.t); say.t = setTimeout(() => { if (A.say && A.say.text === text) { A.say = null; const e2 = document.getElementById('ma-say'); if (e2) e2.innerHTML = ''; } }, 5000); } }
  // redraw only the sections whose content changed, and never one the person is typing in
  function refresh() {
    const r = root(); if (!r) return;
    const c = comm();
    if (!c || !isAdmin(c)) { if (window.__mega.admExit) { toast(c ? 'You are no longer an admin here.' : 'You are no longer in this community.'); window.__mega.admExit(); } return; }
    loadMembers(false);
    const list = SECTIONS.filter(([, , ok]) => ok(c));
    const have = [...r.querySelectorAll('section')].map(s => s.dataset.s);
    if (have.join() !== list.map(s => s[0]).join()) { const keep = document.querySelector('#mega-opts').scrollTop; document.querySelector('#mega-opts').innerHTML = html(A.cid); document.querySelector('#mega-opts').scrollTop = keep; return; }
    const intro = r.querySelector(':scope > p'); if (intro) { const t = `${isOwner(c) ? 'You own' : 'You are an admin of'} <b>${esc(c.name)}</b>. Everyone in it sees these changes.`; if (intro.innerHTML !== t) intro.innerHTML = t; }
    for (const [k, f] of list) {
      const el = r.querySelector(`section[data-s="${k}"]`), h = f(c);
      if (A.cache.get(k) === h) continue;
      if (el.contains(document.activeElement) && document.activeElement.matches('input, select')) continue;
      A.cache.set(k, h); el.innerHTML = h;
    }
  }
  async function loadMembers(force) {
    const c = comm(); if (!c || A.loading) return;
    const v = S.snap.version;
    if (!force && v === A.ver && Date.now() - A.mt < 4000) return;
    A.loading = true; const cid = A.cid;
    try {
      const ms = await rpc('members', { cid }); if (cid !== A.cid) return;
      A.ver = v; A.mt = Date.now();
      const k = JSON.stringify(ms.map(m => [m.account, m.name, m.role, m.me]));
      if (k !== A.mkey) { A.mkey = k; A.members = ms; A.cache.delete('members'); A.cache.delete('owner'); refresh(); }
    } catch { } finally { A.loading = false; }
  }

  // ---------------------------------------------------------------- doing things
  async function act(fn, ok) {
    try { await fn(); if (ok) say(ok); tick(); A.mt = 0; loadMembers(true); return true; }
    catch (er) { say('Could not do that: ' + plain(er), true); return false; }
  }
  const sure = (title, body, label, run) => modal(title, `<p>${body}</p>`, [{ label: 'Cancel', primary: false }, { label, danger: true, run }]);

  async function onClick(e) {
    const b = e.target.closest('[data-a]'); if (!b || !b.closest('.madm')) return;
    const c = comm(); if (!c) return; const d = b.dataset, cid = c.id;
    if (d.a === 'kind') { A.kind = d.v; b.parentElement.querySelectorAll('button').forEach(x => x.setAttribute('aria-checked', x === b)); return; }
    if (d.a === 'invite') { const bi = document.getElementById('btn-invite'); if (bi) bi.click(); return; }
    if (d.a === 'rename') {
      const name = document.getElementById('ma-name').value.trim();
      if (!name) { say('A server needs a name.', true); return; }
      if (name === c.name) { say('That is already its name.', true); return; }
      await act(() => rpc('rename_community', { cid, name }), 'Renamed to ' + name + '.');
    } else if (d.a === 'room-add') {
      const inp = document.getElementById('ma-room'), name = inp.value.trim();
      if (!name) { say('Give the room a name first.', true); return; }
      if (await act(() => rpc('create_channel', { cid, name, kind: A.kind === 'voice' ? 'voice' : '' }), 'Added the room ' + name.toLowerCase().replace(/ /g, '-') + '.')) inp.value = '';
    } else if (d.a === 'room-rename') {
      const w = await modal('Rename ' + d.n, `<input type="text" id="n" value="${esc(d.n)}" maxlength="40" spellcheck="false" aria-label="Room name">`, [{ label: 'Cancel', primary: false }, { label: 'Rename', run: async w => { await rpc('rename_channel', { cid, room: d.id, name: val(w, 'n') }); } }]);
      if (w === 1) { say('Renamed the room.'); tick(); }
    } else if (d.a === 'room-delete') {
      const r = await sure('Delete ' + d.n + '?', `The room <b>${esc(d.n)}</b> disappears for everyone in ${esc(c.name)}, and nobody can open it again. This cannot be undone.`, 'Delete room', async () => { await rpc('delete_channel', { cid, room: d.id }); });
      if (r === 1) { say('Deleted the room ' + d.n + '.'); tick(); }
    } else if (d.a === 'role') {
      const to = d.r;
      await act(() => rpc('set_role', { cid, account: d.acct, role: to }), to === 'admin' ? d.n + ' is now an admin.' : d.n + ' is now a regular member.');
    } else if (d.a === 'ban') {
      const r = await sure('Remove and ban ' + d.n + '?', `${esc(d.n)} is removed from every room and the keys change, so they cannot read anything sent from now on. They keep what they already received. They are banned, so an old invite will not let them back in. An admin can lift the ban later.`, 'Remove and ban', async () => { await rpc('kick', { cid, account: d.acct }); });
      if (r === 1) { say(d.n + ' is being removed. It finishes as soon as an admin has mega open.'); tick(); A.mt = 0; loadMembers(true); }
    } else if (d.a === 'unban') {
      const r = await sure('Unban ' + d.n + '?', `${esc(d.n)} can be invited again. They are not added back by themselves.`, 'Unban', async () => { await rpc('unban', { cid, account: d.acct }); });
      if (r === 1) { say(d.n + ' is no longer banned.'); tick(); }
    } else if (d.a === 'revoke') {
      const r = await sure('Revoke this invite?', 'The link stops working for anyone who has not used it yet. People already in are not affected.', 'Revoke invite', async () => { await rpc('revoke_invite', { cid, id: d.id }); });
      if (r === 1) { say('Invite revoked.'); tick(); }
    } else if (d.a === 'transfer') {
      const sel = document.getElementById('ma-to'), acct = sel.value, nm = sel.selectedOptions[0].dataset.n;
      const r = await sure('Make ' + nm + ' the owner?', `${esc(nm)} becomes the owner of ${esc(c.name)}. You stay on as an admin and can leave afterwards. You cannot take it back: only ${esc(nm)} could hand it over again. You will lose the owner-only controls, such as roles and colours.`, 'Hand over ownership', async () => { await rpc('transfer_ownership', { cid, account: acct }); });
      if (r === 1) { say('Ownership handed to ' + nm + '.'); tick(); A.mt = 0; loadMembers(true); }
    }
  }
  document.addEventListener('click', onClick);
  document.addEventListener('keydown', e => {
    if (e.key !== 'Enter' || !e.target.closest || !e.target.closest('.madm')) return;
    if (e.target.id === 'ma-name') document.querySelector('.madm [data-a="rename"]').click();
    else if (e.target.id === 'ma-room') document.querySelector('.madm [data-a="room-add"]').click();
  });

  window.__megaAdmin = { html, refresh, may: c => !!c && (c.role === 'owner' || c.role === 'admin') };
})();
