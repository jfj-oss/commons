'use strict';
// ---- bridge: Tauri command when packaged, local HTTP bridge for development/headless checks ----
const T = window.__TAURI__;
async function rpc(method, args = {}) {
  if (T && T.core) {
    try { return await T.core.invoke('rpc', { method, args }); } catch (e) { throw new Error(typeof e === 'string' ? e : (e && e.message) || String(e)); }
  }
  const r = await fetch('/rpc', { method: 'POST', body: JSON.stringify({ method, args }) });
  const j = await r.json();
  if (j.err !== undefined) throw new Error(j.err);
  return j.ok;
}
const $ = (s, r = document) => r.querySelector(s);
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const hue = s => { let h = 0; for (const c of s) h = (h * 31 + c.charCodeAt(0)) % 360; return h; };
const color = s => `hsl(${hue(s || 'x')} 55% ${matchMedia('(prefers-color-scheme: dark)').matches ? 66 : 36}%)`;
const fmtTime = ts => new Date(ts * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
const fmtDay = ts => new Date(ts * 1000).toLocaleDateString([], { weekday: 'long', month: 'long', day: 'numeric' });
function toast(t) { const el = $('#toast'); el.textContent = t; el.hidden = false; clearTimeout(toast.t); toast.t = setTimeout(() => el.hidden = true, 2600); }

const S = { snap: null, cid: null, gid: null, msgs: null, reply: null, edit: null, people: false, last: '', lastMsgs: '', imgs: new Map() };

// ---- modal helper ----
function modal(title, html, actions = [{ label: 'Close', primary: false }]) {
  return new Promise(resolve => {
    const root = $('#modal-root');
    const w = document.createElement('div');
    w.className = 'scrim';
    w.innerHTML = `<div class="modal" role="dialog" aria-label="${esc(title)}"><h2>${esc(title)}</h2><div class="mbody">${html}</div><div class="err" id="m-err"></div><div class="acts"></div></div>`;
    root.appendChild(w);
    const close = v => { w.remove(); resolve(v); };
    actions.forEach((a, i) => {
      const b = document.createElement('button');
      b.className = 'btn' + (a.primary === false ? ' ghost' : '') + (a.danger ? ' danger' : '');
      b.textContent = a.label;
      b.onclick = async () => {
        if (a.run) { b.disabled = true; try { const r = await a.run(w); if (r !== false) close(r ?? i); } catch (e) { $('#m-err', w).textContent = e.message; } b.disabled = false; }
        else close(i);
      };
      $('.acts', w).appendChild(b);
    });
    w.addEventListener('mousedown', e => { if (e.target === w) close(-1); });
    const first = $('input, textarea', w); if (first) first.focus();
  });
}
const val = (w, id) => $('#' + id, w).value;

// ---- boot / onboarding ----
async function boot() {
  const st = await rpc('state');
  if (!st.locked) return startApp();
  const el = $('#boot'); el.hidden = false; $('#app').hidden = true;
  if (st.exists) {
    el.innerHTML = `<div class="boot-card"><h1>Commons</h1><div class="tag">Private group chat. Only your devices can read it.</div>
      <label>Passphrase for this device</label><input type="password" id="pp" autofocus>
      <div class="err" id="e"></div><div style="margin-top:14px"><button class="btn" id="go">Unlock</button></div></div>`;
    const go = async () => { try { await rpc('unlock', { passphrase: $('#pp').value }); el.hidden = true; startApp(); } catch (e) { $('#e').textContent = e.message; } };
    $('#go').onclick = go; $('#pp').onkeydown = e => { if (e.key === 'Enter') go(); };
    return;
  }
  const relay = esc(st.default_relay || 'http://127.0.0.1:8787');
  const form = (title, extra, btn, run, links) => {
    el.innerHTML = `<div class="boot-card"><h1>Commons</h1><div class="tag">${title}</div>${extra}
      <label>Relay server</label><input type="text" id="relay" value="${relay}">
      <div class="err" id="e"></div><div style="margin-top:14px"><button class="btn" id="go">${btn}</button></div><div class="boot-links">${links}</div></div>`;
    $('#go').onclick = async () => { $('#go').disabled = true; try { await run(); } catch (e) { $('#e').textContent = e.message; $('#go').disabled = false; } };
  };
  const links = `<a id="l-new">Create an account</a><a id="l-pair">This is a new device for my account</a><a id="l-rec">I only have a recovery key</a>`;
  const wire = () => { $('#l-new').onclick = showNew; $('#l-pair').onclick = showPair; $('#l-rec').onclick = showRec; };
  const showNew = () => { form('Create your account on this device. Your keys are made here and never leave it.',
    `<label>Display name</label><input type="text" id="nm"><label>Passphrase (protects the keys on this device)</label><input type="password" id="pp">`, 'Create account',
    async () => { await rpc('create_account', { name: $('#nm').value, passphrase: $('#pp').value, relay: $('#relay').value }); await showRecoveryOnce(); el.hidden = true; startApp(); }, links); wire(); };
  const showPair = () => { form('Add this device to your account. You will get a code to approve on a device you already use.',
    `<label>Passphrase for this device</label><input type="password" id="pp">`, 'Get pairing code', async () => {
      const r = await rpc('start_pairing', { passphrase: $('#pp').value, relay: $('#relay').value });
      el.innerHTML = `<div class="boot-card"><h1>Approve this device</h1><div class="tag">On a device you already use: Devices → “Add a device”, then scan or paste this code.</div>
        <div class="qr">${r.qr}</div><div class="code" style="font-size:12px">${esc(r.code)}</div><div class="muted small" style="margin-top:10px">Waiting for approval…</div></div>`;
      const t = setInterval(async () => { try { const s = await rpc('pairing_status'); if (s.done) { clearInterval(t); el.hidden = true; startApp(); } } catch (e) { } }, 1500);
    }, links); wire(); };
  const showRec = () => { form('Recover your account on this device with your recovery key. Old messages are not recoverable (they only live on devices).',
    `<label>Display name</label><input type="text" id="nm"><label>Recovery key</label><input type="text" id="rk" class="mono"><label>New passphrase for this device</label><input type="password" id="pp">`, 'Recover',
    async () => { await rpc('recover_account', { name: $('#nm').value, recovery_key: $('#rk').value, passphrase: $('#pp').value, relay: $('#relay').value }); el.hidden = true; startApp(); }, links); wire(); };
  showNew();
}
async function showRecoveryOnce() {
  const k = (await rpc('recovery_key')).key;
  await modal('Save your recovery key', `<p>If you lose every device, this key restores <b>your identity</b> (so friends can add a new device of yours). It does <b>not</b> restore old messages.</p><div class="code">${esc(k)}</div><p class="muted small">Write it down or store it in a password manager. Anyone who has it can act as you.</p>`, [{ label: 'I saved it', primary: true }]);
}

// ---- main app ----
let polling = false;
function startApp() { $('#app').hidden = false; $('#boot').hidden = true; if (!polling) { polling = true; tick(); setInterval(tick, 1000); } }
async function tick() {
  try {
    const snap = await rpc('snapshot');
    { const st = $('#sync-status'); if (st.classList.contains('bad') && !snap.outbox) st.className = 'sync'; }
    const key = JSON.stringify(snap);
    const changed = key !== S.last; S.last = key; S.snap = snap;
    if (changed) renderRail();
    if (S.gid) {
      const m = await rpc('messages', { gid: S.gid });
      const k2 = JSON.stringify(m);
      if (k2 !== S.lastMsgs) { S.lastMsgs = k2; S.msgs = m; renderStream(); }
      const ch = currentChannel();
      if (ch && ch.unread > 0 && document.visibilityState === 'visible') { await rpc('mark_read', { gid: S.gid }); }
    }
  } catch (e) { const st = $('#sync-status'); st.textContent = 'Not reachable: ' + e.message; st.className = 'sync bad'; }
}
const currentCommunity = () => S.snap && S.snap.communities.find(c => c.id === S.cid);
const currentChannel = () => { const c = currentCommunity(); return c && c.channels.find(x => x.gid === S.gid); };

function renderRail() {
  const sn = S.snap;
  $('#me-name').textContent = sn.me.name; $('#me-dot').style.background = color(sn.me.account);
  $('#me-sub').textContent = 'this device ' + sn.me.device.slice(0, 6);
  const live = sn.communities.filter(c => !c.removed);
  if (!S.cid || !sn.communities.find(c => c.id === S.cid)) S.cid = (live[0] || sn.communities[0] || {}).id || null;
  const c = currentCommunity();
  $('#sw-name').textContent = c ? c.name : (sn.joining.length ? 'Waiting to join…' : 'No community');
  const isAdmin = c && (c.role === 'admin' || c.role === 'owner');
  $('#add-room').hidden = !isAdmin; $('#btn-invite').hidden = !isAdmin;
  if (c && !c.channels.find(x => x.gid === S.gid)) { const first = c.channels.find(x => x.joined) || c.channels[0]; S.gid = first ? first.gid : null; S.lastMsgs = ''; S.msgs = null; }
  $('#rooms').innerHTML = c ? c.channels.map(ch => `<button class="room ${ch.gid === S.gid ? 'active' : ''} ${ch.joined ? '' : 'pending'}" data-g="${ch.gid}"><span class="nm">${esc(ch.name)}</span>${ch.unread ? `<span class="pill">${ch.unread}</span>` : ''}</button>`).join('')
    : sn.joining.map(j => `<div class="muted small" style="padding:8px">Asked to join “${esc(j.name)}”. An admin has to let you in; this can take a while if nobody is online.</div>`).join('');
  document.querySelectorAll('.room').forEach(b => b.onclick = () => { S.gid = b.dataset.g; S.lastMsgs = ''; S.msgs = null; S.reply = null; S.edit = null; renderRail(); tick(); });
  const ch = currentChannel();
  $('#chan-title').textContent = ch ? ch.name : (c ? c.name : 'Welcome');
  $('#chan-sub').textContent = c ? `${c.name} · you are ${c.role}${ch && ch.status !== 'ok' ? ' · ' + ch.status : ''}` : '';
  $('#composer').hidden = !(ch && ch.joined && !(c && c.removed));
  if (!ch) $('#stream').innerHTML = `<div class="empty">${c ? 'This community has no rooms you can read yet.' : (sn.joining.length ? 'Waiting for an admin to let you in…' : 'Create a community or join one with an invite link.')}</div>`;
  const st = $('#sync-status');
  const unsent = sn.outbox ? `${sn.outbox} unsent · ` : '';
  st.textContent = unsent + 'relay ' + sn.relay.replace(/^https?:\/\//, ''); st.className = 'sync' + (sn.outbox ? ' bad' : '');
  const b = $('#banner');
  if (c && c.removed) { b.hidden = false; b.textContent = c.removed_reason || 'You are no longer in this community.'; }
  else if (ch && ch.status !== 'ok' && ch.joined) { b.hidden = false; b.textContent = 'This room is ' + ch.status + '. Open the activity log (!) for details.'; }
  else b.hidden = true;
  if (S.people) renderPeople();
}

function renderStream() {
  const st = $('#stream');
  const near = st.scrollHeight - st.scrollTop - st.clientHeight < 80;
  const m = S.msgs; if (!m) return;
  let html = '', lastDay = '', prev = null;
  for (const x of m.messages) {
    const day = fmtDay(x.ts);
    if (day !== lastDay && x.ts) { html += `<div class="day">${esc(day)}</div>`; lastDay = day; prev = null; }
    if (x.kind === 'notice') { html += `<div class="sys">${esc(x.text)}</div>`; prev = null; continue; }
    const cont = prev && prev.sender === x.sender && x.ts - prev.ts < 300 && !x.reply;
    prev = x;
    const reacts = x.reactions.length ? `<div class="reacts">${x.reactions.map(r => `<button class="react ${r.mine ? 'mine' : ''}" data-id="${x.id}" data-e="${esc(r.emoji)}" title="${esc(r.names.join(', '))}">${esc(r.emoji)} ${r.count}</button>`).join('')}</div>` : '';
    let body;
    if (x.deleted) body = `<div class="body">message deleted</div>`;
    else if (x.kind === 'file') body = `<div class="file" data-file="${x.id}"><span>📎</span><div><div>${esc(x.file.name)}</div><div class="muted small">${(x.file.size / 1024).toFixed(1)} KB · <a href="#" class="dl" data-id="${x.id}">download</a></div></div></div>${x.file.mime.startsWith('image/') ? `<div class="imgslot" data-id="${x.id}"></div>` : ''}${x.text ? `<div class="body">${esc(x.text)}</div>` : ''}`;
    else body = `<div class="body">${esc(x.text)}${x.edited ? '<span class="edited">(edited)</span>' : ''}</div>`;
    const quote = x.reply ? `<div class="quote"><b>${esc(x.reply.name)}</b> ${esc(x.reply.text)}</div>` : '';
    const state = x.status === 'sending' ? '<span class="state">sending…</span>' : '';
    const acts = x.deleted ? '' : `<div class="actions"><button data-a="reply" data-id="${x.id}">Reply</button><button data-a="react" data-id="${x.id}">React</button>${x.mine && x.kind === 'text' ? `<button data-a="edit" data-id="${x.id}">Edit</button>` : ''}<button data-a="del" data-id="${x.id}">Delete</button></div>`;
    html += `<div class="msg ${cont ? 'cont' : ''} ${x.deleted ? 'deleted' : ''} ${x.status === 'sending' ? 'sending' : ''}" style="--c:${color(x.sender)}" data-id="${x.id}"><div class="who">${esc(x.name)}<span class="when">${fmtTime(x.ts)}</span>${state}</div>${quote}${body}${reacts}${acts}</div>`;
  }
  st.innerHTML = html || '<div class="empty">No messages yet. Say hello.</div>';
  $('#typing').textContent = m.typing.length ? m.typing.join(', ') + (m.typing.length > 1 ? ' are typing…' : ' is typing…') : '';
  if (near) st.scrollTop = st.scrollHeight;
  wireStream();
}
function wireStream() {
  const byId = id => S.msgs.messages.find(x => x.id === id);
  document.querySelectorAll('.actions button').forEach(b => b.onclick = async e => {
    e.stopPropagation(); const id = b.dataset.id, m = byId(id), a = b.dataset.a;
    if (a === 'reply') { S.reply = m; S.edit = null; renderComposer(); $('#input').focus(); }
    else if (a === 'edit') { S.edit = m; S.reply = null; $('#input').value = m.text; renderComposer(); $('#input').focus(); }
    else if (a === 'del') { try { await rpc('delete', { gid: S.gid, id }); tick(); } catch (er) { toast(er.message); } }
    else if (a === 'react') {
      document.querySelectorAll('.emojis').forEach(x => x.remove());
      const p = document.createElement('div'); p.className = 'emojis';
      for (const em of ['👍', '❤️', '😂', '🎉', '😮', '🙏']) { const eb = document.createElement('button'); eb.textContent = em; eb.onclick = async () => { p.remove(); await rpc('react', { gid: S.gid, id, emoji: em, on: !(m.reactions.find(r => r.emoji === em && r.mine)) }); tick(); }; p.appendChild(eb); }
      b.closest('.msg').appendChild(p);
    }
  });
  document.querySelectorAll('.react').forEach(b => b.onclick = async () => { const m = byId(b.dataset.id); const r = m.reactions.find(r => r.emoji === b.dataset.e); await rpc('react', { gid: S.gid, id: m.id, emoji: b.dataset.e, on: !r.mine }); tick(); });
  document.querySelectorAll('.dl').forEach(a => a.onclick = async e => {
    e.preventDefault();
    try { const f = await rpc('fetch_file', { gid: S.gid, id: a.dataset.id }); const blob = new Blob([Uint8Array.from(atob(f.data.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0))], { type: f.mime }); const u = URL.createObjectURL(blob); const l = document.createElement('a'); l.href = u; l.download = f.name; l.click(); setTimeout(() => URL.revokeObjectURL(u), 5000); } catch (er) { toast(er.message); }
  });
  document.querySelectorAll('.imgslot').forEach(async slot => {
    const id = slot.dataset.id;
    if (!S.imgs.has(id)) { try { const f = await rpc('fetch_file', { gid: S.gid, id }); S.imgs.set(id, `data:${f.mime};base64,${f.data.replace(/-/g, '+').replace(/_/g, '/')}`); } catch (er) { S.imgs.set(id, null); slot.textContent = '⚠ ' + er.message; return; } }
    const u = S.imgs.get(id); if (u) slot.innerHTML = `<img src="${u}" alt="">`;
  });
}
function renderComposer() {
  const r = $('#replybar');
  if (S.reply) { r.hidden = false; r.innerHTML = `<span>Replying to <b>${esc(S.reply.name)}</b>: ${esc(S.reply.text.slice(0, 80))}</span><a href="#" id="cancel">cancel</a>`; }
  else if (S.edit) { r.hidden = false; r.innerHTML = `<span>Editing your message</span><a href="#" id="cancel">cancel</a>`; }
  else r.hidden = true;
  const c = $('#cancel'); if (c) c.onclick = e => { e.preventDefault(); S.reply = null; S.edit = null; $('#input').value = ''; renderComposer(); };
}
async function send() {
  const inp = $('#input'); const text = inp.value; if (!text.trim() || !S.gid) return;
  try {
    if (S.edit) await rpc('edit', { gid: S.gid, id: S.edit.id, text });
    else await rpc('send_text', { gid: S.gid, text, reply: S.reply ? S.reply.id : null });
    inp.value = ''; inp.style.height = ''; S.reply = null; S.edit = null; renderComposer(); S.lastMsgs = ''; tick();
  } catch (e) { toast(e.message); }
}
$('#send').onclick = send;
$('#input').onkeydown = e => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send(); } else if (S.gid) rpc('typing', { gid: S.gid }).catch(() => { }); };
$('#input').oninput = e => { e.target.style.height = 'auto'; e.target.style.height = Math.min(e.target.scrollHeight, 180) + 'px'; };
$('#attach').onclick = () => $('#file').click();
$('#file').onchange = async e => {
  const f = e.target.files[0]; if (!f) return; e.target.value = '';
  if (f.size > 24 * 1024 * 1024) return toast('Files are limited to 24 MB.');
  const buf = new Uint8Array(await f.arrayBuffer()); let s = ''; for (let i = 0; i < buf.length; i += 0x8000) s += String.fromCharCode.apply(null, buf.subarray(i, i + 0x8000));
  toast('Encrypting and uploading…');
  try { await rpc('send_file', { gid: S.gid, name: f.name, mime: f.type || 'application/octet-stream', data: btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, ''), caption: '' }); S.lastMsgs = ''; tick(); } catch (er) { toast(er.message); }
};

// ---- people drawer ----
async function renderPeople() {
  const c = currentCommunity(); const el = $('#people'); if (!c) { el.hidden = true; return; }
  const ms = await rpc('members', { cid: c.id });
  const iAmOwner = c.role === 'owner', iAmAdmin = iAmOwner || c.role === 'admin';
  el.innerHTML = `<h2>People <span class="muted small">${ms.length}</span></h2>` + ms.map(m => `<div class="person"><span class="dot" style="background:${color(m.account)}"></span><div class="nm"><b>${esc(m.name)}${m.me ? ' (you)' : ''}</b><span class="muted small">${m.devices} device${m.devices === 1 ? '' : 's'}</span></div><span class="chip ${m.role !== 'member' ? 'ok' : ''}">${m.role}</span>${m.verified ? '<span class="chip ok" title="Safety number verified">✓</span>' : ''}${m.me ? '' : `<button class="mini" data-a="verify" data-acct="${m.account}" title="Verify safety number">🔒</button>`}${(iAmOwner && !m.me && m.role !== 'owner') ? `<button class="mini" data-a="role" data-acct="${m.account}" data-r="${m.role}" title="Change role">★</button>` : ''}${(!m.me && m.role !== 'owner' && (iAmOwner || (iAmAdmin && m.role === 'member'))) ? `<button class="mini" data-a="kick" data-acct="${m.account}" data-n="${esc(m.name)}" title="Remove from community">✕</button>` : ''}</div>`).join('') +
    `<div style="margin-top:14px;display:flex;gap:8px;flex-wrap:wrap">${c.role !== 'owner' ? '<button class="btn ghost" id="leave">Leave community</button>' : ''}${iAmAdmin ? '<button class="btn ghost" id="rename">Rename</button>' : ''}</div>`;
  el.querySelectorAll('button[data-a]').forEach(b => b.onclick = () => personAction(b.dataset, c));
  const lv = $('#leave'); if (lv) lv.onclick = async () => { if ((await modal('Leave ' + c.name + '?', '<p>You will stop receiving messages and your keys for this community are discarded after an admin removes you.</p>', [{ label: 'Cancel', primary: false }, { label: 'Leave', danger: true, run: async () => { await rpc('leave', { cid: c.id }); } }])) === 1) { toast('Left.'); tick(); } };
  const rn = $('#rename'); if (rn) rn.onclick = () => modal('Rename community', `<input type="text" id="n" value="${esc(c.name)}">`, [{ label: 'Cancel', primary: false }, { label: 'Rename', run: async w => { await rpc('rename_community', { cid: c.id, name: val(w, 'n') }); } }]).then(tick);
}
async function personAction(d, c) {
  if (d.a === 'kick') {
    await modal('Remove ' + d.n + '?', `<p>They are removed from every room and the keys are rotated, so they cannot read anything sent from now on. They can still read what they already received. They cannot rejoin with an old invite.</p>`, [{ label: 'Cancel', primary: false }, { label: 'Remove', danger: true, run: async () => { await rpc('kick', { cid: c.id, account: d.acct }); } }]);
    setTimeout(() => { tick(); renderPeople(); }, 800);
  } else if (d.a === 'role') {
    const to = d.r === 'admin' ? 'member' : 'admin';
    await modal('Change role', `<p>Make this person ${to === 'admin' ? 'an <b>admin</b> (can invite, remove members and create rooms)' : 'a regular <b>member</b>'}?</p>`, [{ label: 'Cancel', primary: false }, { label: 'Yes', run: async () => { await rpc('set_role', { cid: c.id, account: d.acct, role: to }); } }]);
    setTimeout(() => { tick(); renderPeople(); }, 800);
  } else if (d.a === 'verify') {
    const s = await rpc('safety', { account: d.acct });
    const groups = s.number.split(' ').map(g => `<span>${g}</span>`).join('');
    await modal('Verify ' + s.name, `<p>Compare these numbers with ${esc(s.name)} in person or over a call. If they match, nobody (including the relay) is sitting between you.</p><div class="safety">${groups}</div><div class="qr">${s.qr}</div><p class="muted small">The QR contains the same number; scan it on the other device or compare by eye.</p>`,
      [{ label: 'Close', primary: false }, { label: s.verified ? 'Mark as not verified' : 'They match, mark verified', run: async () => { await rpc('set_verified', { account: d.acct, on: !s.verified }); } }]);
    renderPeople();
  }
}
$('#btn-people').onclick = () => { S.people = !S.people; $('#people').hidden = !S.people; if (S.people) renderPeople(); };

// ---- community switcher, create, join, invite ----
$('#switcher').onclick = e => {
  e.stopPropagation(); const m = $('#sw-menu'); if (!m.hidden) { m.hidden = true; return; }
  m.innerHTML = S.snap.communities.map(c => `<button data-c="${c.id}">${esc(c.name)}${c.removed ? ' (left)' : ''}${c.unread ? ` <span class="pill">${c.unread}</span>` : ''}</button>`).join('') + '<hr><button id="mk-new">New community…</button><button id="mk-join">Join with an invite link…</button>';
  m.hidden = false;
  m.querySelectorAll('[data-c]').forEach(b => b.onclick = () => { S.cid = b.dataset.c; S.gid = null; S.lastMsgs = ''; m.hidden = true; renderRail(); tick(); });
  $('#mk-new').onclick = () => { m.hidden = true; modal('New community', `<label>Name</label><input type="text" id="n" placeholder="e.g. Book club">`, [{ label: 'Cancel', primary: false }, { label: 'Create', run: async w => { const r = await rpc('create_community', { name: val(w, 'n') }); S.cid = r.id; S.gid = null; } }]).then(() => tick()); };
  $('#mk-join').onclick = () => { m.hidden = true; modal('Join with an invite link', `<p class="muted small">Paste the link you were sent. An admin has to be online (now or later) to let you in.</p><input type="text" id="l" placeholder="commons://join/…">`, [{ label: 'Cancel', primary: false }, { label: 'Join', run: async w => { await rpc('join', { link: val(w, 'l') }); toast('Request sent'); } }]).then(() => tick()); };
};
document.addEventListener('click', () => { $('#sw-menu').hidden = true; document.querySelectorAll('.emojis').forEach(x => x.remove()); });
$('#add-room').onclick = () => modal('New room', `<label>Name</label><input type="text" id="n" placeholder="e.g. music">`, [{ label: 'Cancel', primary: false }, { label: 'Create', run: async w => { const r = await rpc('create_channel', { cid: S.cid, name: val(w, 'n') }); S.gid = r.id; } }]).then(() => tick());
$('#btn-invite').onclick = () => modal('Invite someone', `<label>Valid for</label><select id="ttl"><option value="3600">1 hour</option><option value="86400">1 day</option><option value="604800" selected>7 days</option></select><label>Uses</label><select id="uses"><option value="1" selected>One person</option><option value="5">Up to 5 people</option><option value="">Unlimited</option></select><div id="out"></div>`,
  [{ label: 'Close', primary: false }, { label: 'Create link', run: async w => { const r = await rpc('create_invite', { cid: S.cid, ttl: +val(w, 'ttl'), uses: val(w, 'uses') ? +val(w, 'uses') : null }); $('#out', w).innerHTML = `<p>Send this link privately. Whoever has it can ask to join.</p><div class="code" style="font-size:12px">${esc(r.link)}</div><div class="qr">${r.qr}</div>`; return false; } }]);

// ---- search ----
let st_t; $('#search').oninput = e => { clearTimeout(st_t); st_t = setTimeout(async () => { const q = e.target.value; if (q.trim().length < 2) return; const r = await rpc('search', { q }); await modal('Search: ' + q, r.length ? r.map(x => `<div class="log-row"><span class="k">${esc(x.community)} / ${esc(x.channel)}</span><b>${esc(x.from)}</b> ${esc(x.text)}</div>`).join('') : '<p class="muted">Nothing found. Search only covers messages stored on this device.</p>'); e.target.value = ''; }, 500); };

// ---- devices, activity, settings ----
$('#btn-devices').onclick = async () => {
  const render = async w => {
    const ds = await rpc('devices');
    $('#mb', w).innerHTML = `<p class="muted small">Every device holds its own keys. A lost device must be revoked: it is then removed from every room and cannot read anything new.</p>` +
      ds.map(d => `<div class="person"><div class="nm"><b class="mono">${d.device.slice(0, 12)}</b><span class="muted small">added ${new Date(d.created * 1000).toLocaleDateString()}${d.this ? ' · this device' : ''}</span></div>${d.revoked ? '<span class="chip">revoked</span>' : (d.this ? '' : `<button class="btn ghost" data-r="${d.device}">Revoke</button>`)}</div>`).join('');
    $('#mb', w).querySelectorAll('[data-r]').forEach(b => b.onclick = async () => { try { await rpc('revoke_device', { device: b.dataset.r }); render(w); } catch (e) { toast(e.message); } });
  };
  const p = modal('Devices and recovery', `<div id="mb"></div><label>Add a device</label><input type="text" id="code" placeholder="Paste the code shown on the new device (commons-pair:…)"><div class="muted small" id="chk"></div>
    <label>Recovery key</label><button class="btn ghost" id="showkey">Show my recovery key</button><div id="key"></div>`,
    [{ label: 'Close', primary: false }, { label: 'Approve new device', run: async w => { const r = await rpc('approve_pairing', { code: val(w, 'code') }); $('#chk', w).textContent = 'Approved. The new device will join your rooms within a few seconds. Check code: ' + r.check; return false; } }]);
  setTimeout(() => { const w = document.querySelector('.modal'); if (!w) return; render(w); $('#showkey', w).onclick = async () => { $('#key', w).innerHTML = `<div class="code" style="margin-top:8px">${esc((await rpc('recovery_key')).key)}</div>`; }; }, 30);
  await p;
};
$('#btn-activity').onclick = async () => {
  const ns = await rpc('notices');
  modal('Activity log', `<p class="muted small">Everything unusual the app noticed, in plain words. Nothing here is hidden from you: if a message could not be read, it says why.</p>` + (ns.length ? ns.map(n => `<div class="log-row ${['decrypt', 'broken', 'rollback', 'reject', 'gap'].includes(n.kind) ? 'bad' : ''}"><span class="k">${esc(n.kind)}</span>${esc(n.text)}</div>`).join('') : '<p class="muted">Nothing yet.</p>'));
};
$('#btn-settings').onclick = () => modal('Settings', `<label>Display name</label><input type="text" id="n" value="${esc(S.snap.me.name)}">
  <label><input type="checkbox" id="ty" ${S.snap.typing_enabled ? 'checked' : ''}> Send “typing…” hints (the relay can see that a message-sized packet was sent at that moment)</label>
  <label>Theme</label><select id="th"><option value="">Match system</option><option value="light">Light</option><option value="dark">Dark</option></select>
  <p class="muted small">Relay: ${esc(S.snap.relay)}<br>Your account id: <span class="mono">${esc(S.snap.me.account.slice(0, 16))}…</span></p>`,
  [{ label: 'Cancel', primary: false }, { label: 'Save', run: async w => { await rpc('set_name', { name: val(w, 'n') }); await rpc('set_typing_enabled', { on: $('#ty', w).checked }); const t = val(w, 'th'); if (t) document.documentElement.dataset.theme = t; else delete document.documentElement.dataset.theme; } }]).then(tick);

boot();
