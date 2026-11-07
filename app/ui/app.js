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
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const calm = () => matchMedia('(prefers-reduced-motion: reduce)').matches;
const icon = (n, cls = '') => `<svg class="ic ${cls}" aria-hidden="true"><use href="#i-${n}"/></svg>`;

// Per-device display preferences only. Nothing about messages or keys goes here.
const pref = {
  get(k) { try { return localStorage.getItem('commons.' + k); } catch { return null; } },
  set(k, v) { try { if (v) localStorage.setItem('commons.' + k, v); else localStorage.removeItem('commons.' + k); } catch { } },
};
function applyPrefs() {
  const r = document.documentElement, t = pref.get('theme'), m = pref.get('read');
  if (t) r.dataset.theme = t; else delete r.dataset.theme;
  if (m) r.dataset.read = m; else delete r.dataset.read;
}
applyPrefs();

// ---- time ----
const fmtTime = ts => new Date(ts * 1000).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
const fmtFull = ts => new Date(ts * 1000).toLocaleString([], { dateStyle: 'full', timeStyle: 'short' });
const dayKey = ts => new Date(ts * 1000).toDateString();
function fmtDay(ts) {
  const d = new Date(ts * 1000), now = new Date(), y = new Date(now); y.setDate(now.getDate() - 1);
  if (d.toDateString() === now.toDateString()) return 'Today';
  if (d.toDateString() === y.toDateString()) return 'Yesterday';
  return d.toLocaleDateString([], { weekday: 'long', month: 'long', day: 'numeric', ...(d.getFullYear() === now.getFullYear() ? {} : { year: 'numeric' }) });
}
const fmtSize = n => n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1048576).toFixed(1)} MB`;
function fmtLeft(ts) {
  const s = ts - Date.now() / 1000; if (s <= 0) return 'expired';
  if (s < 3600) return `${Math.ceil(s / 60)} min left`;
  if (s < 86400) return `${Math.round(s / 3600)} h left`;
  return `${Math.round(s / 86400)} days left`;
}

// ---- seals and crests: a person's mark is drawn from their account id, a club's from its id ----
const WAX = ['#8e2f2a', '#ad4e28', '#94661a', '#5a7232', '#2b6a56', '#226c78', '#34568d', '#68407e', '#a04468', '#73523a'];
const hash = s => { let h = 2166136261; for (const c of String(s)) { h ^= c.codePointAt(0); h = Math.imul(h, 16777619); } return h >>> 0; };
// Friends in one community should not share a colour, so collisions step to the next free wax.
let palette = new Map();
const wax = s => palette.get(s) || WAX[hash(s) % WAX.length];
function assignWax(ms) {
  palette = new Map(); const used = new Set();
  for (const m of [...ms].sort((a, b) => (a.account < b.account ? -1 : 1))) {
    if (used.size === WAX.length) used.clear();
    let i = hash(m.account) % WAX.length; while (used.has(i)) i = (i + 1) % WAX.length;
    used.add(i); palette.set(m.account, WAX[i]);
  }
}
function initials(n) {
  const w = String(n || '?').trim().split(/\s+/).filter(Boolean);
  const first = x => ([...x][0] || '?').toUpperCase();
  return w.length > 1 ? first(w[0]) + first(w[w.length - 1]) : first(w[0] || '?');
}
{ // scalloped outlines, one per lobe count; each seal picks a count and a rotation
  let out = '';
  for (let l = 7; l <= 11; l++) {
    let d = '';
    for (let i = 0; i < 120; i++) { const a = i / 120 * Math.PI * 2, r = 18.6 + 1.05 * Math.cos(l * a); d += `${i ? 'L' : 'M'}${(20 + r * Math.cos(a)).toFixed(2)} ${(20 + r * Math.sin(a)).toFixed(2)}`; }
    out += `<symbol id="s${l}" viewBox="0 0 40 40"><path d="${d}Z"/></symbol>`;
  }
  $('#sprite').insertAdjacentHTML('beforeend', out);
}
function seal(id, name, cls = '') {
  const h = hash(id || name || 'x'), lobes = 7 + (h >>> 4) % 5, turn = (h >>> 9) % 72 * 5;
  return `<span class="seal ${cls}" style="--wax:${wax(id || name || 'x')}" aria-hidden="true"><svg viewBox="0 0 40 40"><use href="#s${lobes}" transform="rotate(${turn} 20 20)"/><circle cx="20" cy="20" r="14.6"/></svg><i>${esc(initials(name))}</i></span>`;
}
const crest = (id, name, cls = '') => `<span class="crest ${cls}" style="--wax:${wax(id || name)}" aria-hidden="true"><svg viewBox="0 0 40 46"><use href="#crest"/></svg><i>${esc(initials(String(name).replace(/^(the|a|an)\s+(?=\S)/i, '')))}</i></span>`;

function toast(t) { const el = $('#toast'); el.textContent = t; el.hidden = false; clearTimeout(toast.t); toast.t = setTimeout(() => el.hidden = true, 2600); }
async function copy(text, done = 'Copied') {
  try { await navigator.clipboard.writeText(text); }
  catch { const t = document.createElement('textarea'); t.value = text; document.body.appendChild(t); t.select(); document.execCommand('copy'); t.remove(); }
  toast(done);
}

const S = {
  snap: null, cid: null, gid: null, msgs: null, reply: null, edit: null, people: false, last: '', lastMsgs: '', imgs: new Map(), limit: 200,
  members: [], membersFor: null, membersKey: '',
  mention: new Map(), mentionSig: new Map(),   // gid -> unread messages that name me, gid -> unread count it was computed for
  first: true, painted: 0, firstNew: null, newCount: 0, jumpTo: null, pinned: true, below: 0, drafts: new Map(),
};
const rowCache = new Map(), roomCache = new Map();

// Keyed patch: rows are {k, h}. Unchanged rows keep their element, so nothing flickers, hover survives,
// and only rows that are really new get to animate.
const tpl = document.createElement('template');
function patch(host, rows, cache, { onNew, onChange } = {}) {
  for (const n of [...host.childNodes]) if (!n._k) n.remove();
  const seen = new Set(); let cur = host.firstChild;
  for (const r of rows) {
    seen.add(r.k);
    let c = cache.get(r.k);
    if (!c || c.h !== r.h) {
      tpl.innerHTML = r.h;
      const el = tpl.content.firstElementChild; el._k = r.k;
      if (c) { if (cur === c.el) cur = el; c.el.replaceWith(el); onChange && onChange(el, c.el); } else if (onNew) onNew(el);
      c = { el, h: r.h }; cache.set(r.k, c);
    }
    if (c.el === cur) cur = cur.nextSibling; else host.insertBefore(c.el, cur);
  }
  for (const [k, c] of cache) if (!seen.has(k)) { c.el.remove(); cache.delete(k); }
}

// ---- modal helper ----
function modal(title, html, actions = [{ label: 'Close', primary: false }], opts = {}) {
  return new Promise(resolve => {
    const root = $('#modal-root'), opener = document.activeElement;
    const w = document.createElement('div');
    w.className = 'scrim';
    w.innerHTML = `<div class="modal ${opts.cls || ''}" role="dialog" aria-modal="true" aria-label="${esc(title)}" tabindex="-1">${opts.top || ''}<h2>${esc(title)}</h2><div class="mbody">${html}</div><div class="err" id="m-err" role="alert"></div><div class="acts"></div></div>`;
    root.appendChild(w);
    let closed = false;
    const close = v => {
      if (closed) return; closed = true;
      document.removeEventListener('keydown', onKey, true);
      // Rename first: anything looking for an open dialog must not find this one while it fades.
      $('.modal', w).classList.replace('modal', 'modal-out'); w.className = 'scrim-out';
      setTimeout(() => w.remove(), calm() ? 0 : 140);
      if (opener && opener.focus && document.contains(opener)) opener.focus({ preventScroll: true });
      resolve(v);
    };
    const onKey = e => {
      if (e.key === 'Escape') { e.stopPropagation(); close(-1); }
      else if (e.key === 'Tab') {
        const f = $$('button:not(:disabled), input:not([type=hidden]), select, textarea, a[href], summary', w).filter(x => x.offsetParent !== null);
        if (!f.length) return;
        const a = f[0], z = f[f.length - 1];
        if (e.shiftKey && (document.activeElement === a || document.activeElement === $('.modal', w))) { e.preventDefault(); z.focus(); }
        else if (!e.shiftKey && document.activeElement === z) { e.preventDefault(); a.focus(); }
      }
    };
    document.addEventListener('keydown', onKey, true);
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
    const first = $('input:not([type=hidden]), textarea', w); (first || $('.modal', w)).focus({ preventScroll: true });
  });
}
const val = (w, id) => $('#' + id, w).value;
// Two to four choices side by side; the value lives in a hidden input so the caller reads it like any field.
const seg = (id, options, def) => `<input type="hidden" id="${id}" value="${esc(def)}"><div class="seg" role="radiogroup" data-for="${id}">${options.map(([v, l]) => `<button type="button" role="radio" aria-checked="${v === def}" data-v="${esc(v)}">${esc(l)}</button>`).join('')}</div>`;
$('#modal-root').addEventListener('click', e => {
  const b = e.target.closest('.seg button'); if (!b) return;
  const g = b.parentElement; $$('button', g).forEach(x => x.setAttribute('aria-checked', x === b));
  $('#' + g.dataset.for, g.parentElement).value = b.dataset.v;
});

// ---- boot / onboarding ----
const BRAND = `<div class="brand"><svg viewBox="0 0 40 46" aria-hidden="true"><use href="#crest"/></svg><svg class="hole" viewBox="0 0 12 14" aria-hidden="true"><use href="#i-keyhole"/></svg></div>`;
async function boot() {
  const st = await rpc('state');
  if (!st.locked) return st.has_identity ? startApp() : pairScreen(await rpc('pairing_resume'));
  const el = $('#boot'); el.hidden = false; $('#app').hidden = true;
  if (st.exists) {
    el.innerHTML = `<div class="boot-card">${BRAND}<h1>Commons</h1><div class="tag">Private group chat. Only your devices can read it.</div>
      <label for="pp">Passphrase for this device</label><input type="password" id="pp" autofocus>
      <div class="err" id="e" role="alert"></div><div class="boot-go"><button class="btn" id="go">Unlock</button></div></div>`;
    const go = async () => { try { await rpc('unlock', { passphrase: $('#pp').value }); if (!(await rpc('state')).has_identity) return pairScreen(await rpc('pairing_resume')); el.hidden = true; startApp(); } catch (e) { $('#e').textContent = e.message; } };
    $('#go').onclick = go; $('#pp').onkeydown = e => { if (e.key === 'Enter') go(); };
    return;
  }
  const relay = esc(st.default_relay || '');
  let invited = null; // what a pasted invite said
  const invBox = `<label for="lnk">Invite link</label><input type="text" id="lnk" autocomplete="off" spellcheck="false" placeholder="Paste what your friend sent you"><div class="lnk-note small muted" id="lnk-note" role="status"></div>`;
  const readLink = async () => {
    const box = $('#lnk'); if (!box) return; const v = box.value.trim(); invited = null; $('#lnk-note').textContent = '';
    if (!v) return;
    try {
      invited = await rpc('parse_link', { link: v });
      $('#relay').value = invited.relay; $('#inv').value = invited.secret;
      $('#lnk-note').textContent = invited.kind === 'community' ? `You are joining “${invited.name}” on ${invited.relay.replace(/^https?:\/\//, '')}.` : `This puts you on the relay at ${invited.relay.replace(/^https?:\/\//, '')}.`;
    } catch (e) { invited = null; $('#lnk-note').textContent = e.message; throw e; }
  };
  const form = (title, extra, btn, run, links) => {
    el.innerHTML = `<div class="boot-card">${BRAND}<h1>Commons</h1><div class="tag">${title}</div>${extra}
      <details class="relay-box"><summary>Relay server</summary><input type="text" id="relay" value="${relay}" aria-label="Relay server" spellcheck="false" placeholder="https://chat.example.org"><label for="inv">Invite secret (only if this relay asks for one)</label><input type="password" id="inv" autocomplete="off"></details>
      <div class="err" id="e" role="alert"></div><div class="boot-go"><button class="btn" id="go">${btn}</button></div><div class="boot-links">${links}</div></div>`;
    const pp = $('#pp'); if (pp && $('#pp2')) pp.oninput = () => { $('#pph').textContent = ppStrength(pp.value); };
    if ($('#lnk')) $('#lnk').oninput = () => readLink().catch(() => { });
    $('#go').onclick = async () => { $('#go').disabled = true; try { await readLink(); if ($('#pp2') && $('#pp').value !== $('#pp2').value) throw new Error('The two passphrases are different.'); await run(); } catch (e) { $('#e').textContent = e.message; $('#go').disabled = false; if (/relay/i.test(e.message)) $('.relay-box').open = true; } };
    $$('input', el).forEach(i => i.onkeydown = e => { if (e.key === 'Enter') $('#go').click(); });
  };
  const links = which => [['new', 'Create an account'], ['pair', 'This is a new device for my account'], ['rec', 'I only have a recovery key']].filter(([k]) => k !== which).map(([k, l]) => `<button type="button" class="linkish" id="l-${k}">${l}</button>`).join('');
  const wire = () => { for (const [k, f] of [['new', showNew], ['pair', showPair], ['rec', showRec]]) { const b = $('#l-' + k); if (b) b.onclick = f; } };
  const showNew = () => { form('Create your account on this device. Your keys are made here and never leave it.',
    `${invBox}<label for="nm">Display name</label><input type="text" id="nm" autocomplete="off"><label for="pp">Passphrase (protects the keys on this device)</label><input type="password" id="pp">${PP2}`, 'Create account',
    async () => { const pass = $('#pp').value; await rpc('create_account', { name: $('#nm').value, passphrase: pass, relay: $('#relay').value, invite: $('#inv').value }); if (invited && invited.kind === 'community') await rpc('join', { link: invited.link }).catch(e => toast(e.message)); await showRecoveryOnce(pass); el.hidden = true; startApp(); }, links('new')); wire(); };
  const showPair = () => { form('Add this device to your account. You will get a code to approve on a device you already use.',
    `${invBox}<label for="pp">Passphrase for this device</label><input type="password" id="pp">${PP2}`, 'Get pairing code', async () => {
      const r = await rpc('start_pairing', { passphrase: $('#pp').value, relay: $('#relay').value });
      pairScreen(r);
    }, links('pair')); wire(); };
  const showRec = () => { form('Recover your account on this device with your recovery key. Old messages are not recoverable (they only live on devices).',
    `${invBox}<label for="nm">Display name</label><input type="text" id="nm" autocomplete="off"><label for="rk">Recovery key</label><input type="text" id="rk" class="mono" autocomplete="off" spellcheck="false"><label for="pp">New passphrase for this device</label><input type="password" id="pp">${PP2}`, 'Recover',
    async () => { await rpc('recover_account', { name: $('#nm').value, recovery_key: $('#rk').value, passphrase: $('#pp').value, relay: $('#relay').value }); el.hidden = true; startApp(); }, links('rec')); wire(); };
  showNew();
}
// Waiting for a device you already use to approve this one. Also where a pairing resumes after a restart, and the way out of one that went nowhere.
function pairScreen(r) {
  const el = $('#boot'); el.hidden = false; $('#app').hidden = true;
  el.innerHTML = `<div class="boot-card">${BRAND}<h1>Approve this device</h1><div class="tag">On a device you already use: Devices → “Add a device”, then scan or paste this code.</div>
    <div class="qr">${r.qr}</div><div class="code small-code">${esc(r.code)}</div><p>Check code <b class="mono">${esc(r.check)}</b> (the other device must show the same one when you approve).</p><div class="waiting"><span class="dots"><i></i><i></i><i></i></span> Waiting for approval…</div><div class="boot-links"><button type="button" class="linkish" id="l-over">Start over</button></div></div>`;
  const t = setInterval(async () => { try { const s = await rpc('pairing_status'); if (s.done) { clearInterval(t); el.hidden = true; startApp(); } } catch (e) { } }, 1500);
  $('#l-over').onclick = async () => { clearInterval(t); try { await rpc('pairing_abandon'); } catch (e) { } boot(); };
}
const PP2 = `<label for="pp2">Type it again</label><input type="password" id="pp2"><div class="muted small" id="pph" aria-live="polite"></div>`;
function ppStrength(p) {
  if (!p) return '';
  if (p.length < 8) return 'Too short: at least 8 characters.';
  const kinds = [/[a-z]/, /[A-Z]/, /[0-9]/, /[^a-zA-Z0-9]/].filter(r => r.test(p)).length, words = p.trim().split(/\s+/).length;
  const score = p.length + kinds * 2 + (words >= 4 ? 8 : 0);
  return score < 16 ? 'Weak. Someone with this disk could guess it. Four or more random words work well.' : score < 24 ? 'Fair. A little longer would be better.' : 'Strong.';
}
async function showRecoveryOnce(pass) {
  const k = (await rpc('recovery_key', { passphrase: pass })).key;
  await modal('Save your recovery key', `<p>If you lose every device, this key restores <b>your identity</b> (so friends can add a new device of yours). It does <b>not</b> restore old messages.</p><div class="code key-code">${esc(k)}</div><button type="button" class="linkish copy-line" id="cp">${icon('copy')}Copy</button><p class="muted small">Write it down or store it in a password manager. Anyone who has it can act as you.</p>`, [{ label: 'I saved it', primary: true }]);
}
document.addEventListener('click', e => { if (e.target.closest('#cp')) copy($('.modal .code').textContent); });

// ---- main app ----
let polling = false;
function startApp() { $('#app').hidden = false; $('#boot').hidden = true; if (!polling) { polling = true; tick(); setInterval(tick, 1000); } }
async function tick() {
  try {
    const snap = await rpc('snapshot');
    const st = $('#sync-status'); if (st.classList.contains('down')) { st.className = 'sync'; S.last = ''; }
    const key = JSON.stringify(snap);
    const changed = key !== S.last; S.last = key; S.snap = snap;
    if (changed) { renderRail(); refreshMembers(); refreshMentions(); }
    const gid = S.gid;
    if (gid) {
      const m = await rpc('messages', { gid, limit: S.limit, around: S.jumpTo || '' });
      if (gid !== S.gid) return;
      if (S.jumpTo) S.limit = Math.max(S.limit, m.messages.length);
      const k2 = JSON.stringify(m);
      if (k2 !== S.lastMsgs) { S.lastMsgs = k2; S.msgs = m; renderStream(); }
      const ch = currentChannel();
      if (ch && ch.unread > 0 && document.visibilityState === 'visible') { await rpc('mark_read', { gid }); }
    }
  } catch (e) { const st = $('#sync-status'); st.textContent = 'Not reachable: ' + e.message; st.className = 'sync bad down'; }
}
const currentCommunity = () => S.snap && S.snap.communities.find(c => c.id === S.cid);
const currentChannel = () => { const c = currentCommunity(); return c && c.channels.find(x => x.gid === S.gid); };
const isPhone = () => matchMedia('(max-width: 760px)').matches;
const railOpen = on => $('#app').classList.toggle('rail-open', on);

function resetRoom() {
  S.limit = 200; S.lastMsgs = ''; S.msgs = null; S.reply = null; S.edit = null; S.first = true; S.firstNew = null; S.newCount = 0; S.pinned = true; S.below = 0;
  $('#jump').hidden = true; renderComposer();
}
function openRoom(gid, jumpTo = null) {
  const inp = $('#input');
  if (gid !== S.gid) { if (S.gid) S.drafts.set(S.gid, inp.value); S.gid = gid; resetRoom(); inp.value = S.drafts.get(gid) || ''; sizeInput(); }
  S.jumpTo = jumpTo; railOpen(false); renderRail(); tick();
}
$('#rooms').addEventListener('click', async e => {
  const x = e.target.closest('[data-join]');
  if (x) { try { await rpc('cancel_join', { id: x.dataset.join }); tick(); } catch (er) { toast(er.message); } return; }
  const b = e.target.closest('.room'); if (b) openRoom(b.dataset.g);
});

function renderRail() {
  const sn = S.snap;
  $('#me-name').textContent = sn.me.name;
  $('#me-seal').innerHTML = seal(sn.me.account, sn.me.name);
  $('#me-sub').textContent = 'this device ' + sn.me.device.slice(0, 6);
  const live = sn.communities.filter(c => !c.removed);
  if (!S.cid || !sn.communities.find(c => c.id === S.cid)) S.cid = (live[0] || sn.communities[0] || {}).id || null;
  const c = currentCommunity();
  $('#sw-name').textContent = c ? c.name : (sn.joining.length ? 'Waiting to join…' : 'No community');
  $('#sw-crest').innerHTML = c ? crest(c.id, c.name) : '';
  $('#sw-sub').textContent = c ? (c.removed ? 'no longer a member' : `you are ${roleWords(c.role)}`) : 'Open the menu to start';
  const isAdmin = c && (c.role === 'admin' || c.role === 'owner');
  $('#add-room').hidden = !isAdmin; $('#btn-invite').hidden = !isAdmin; $('#btn-people').hidden = !c;
  $('.search-wrap').hidden = !c; $('#btn-search').hidden = !c;
  if (c && !c.channels.find(x => x.gid === S.gid)) { const first = c.channels.find(x => x.joined) || c.channels[0]; S.gid = first ? first.gid : null; resetRoom(); }
  if (!c) { S.gid = null; }
  const seen = document.visibilityState === 'visible';
  patch($('#rooms'), (c ? c.channels.map(ch => {
    const active = ch.gid === S.gid, un = active && seen ? 0 : ch.unread, at = active ? 0 : (S.mention.get(ch.gid) || 0);
    const said = [ch.name, un ? `${un} unread` : '', at ? `${at} mention${at === 1 ? '' : 's'} of you` : '', ch.joined ? '' : 'waiting for keys'].filter(Boolean).join(', ');
    return { k: ch.gid, h: `<button class="room ${active ? 'active' : ''} ${ch.joined ? '' : 'pending'} ${un ? 'unread' : ''} ${at ? 'mention' : ''}" data-g="${esc(ch.gid)}" aria-label="${esc(said)}"${active ? ' aria-current="true"' : ''}><svg class="kh" viewBox="0 0 12 14" aria-hidden="true"><use href="#i-keyhole"/></svg><span class="nm">${esc(ch.name)}</span>${at ? '<span class="at-mark">@</span>' : ''}${un ? `<span class="pill">${un}</span>` : ''}</button>` };
  }) : []).concat(joinNotes(sn)), roomCache);
  const ch = currentChannel();
  $('#chan-title').textContent = ch ? ch.name : (c ? c.name : 'Welcome');
  const people = S.membersFor === S.cid && S.members.length ? `${S.members.length} ${S.members.length === 1 ? 'person' : 'people'}` : '';
  $('#chan-sub').textContent = c ? [c.name, people, ch && ch.status !== 'ok' ? ch.status : ''].filter(Boolean).join(' · ') : '';
  $('#composer').hidden = !(ch && ch.joined && !(c && c.removed)) || sn.revoked;
  if (!ch) paintEmpty(c, sn);
  else if (!ch.joined && !S.msgs) paint([{ k: 'e:keys', h: KEYS_WAIT }]);
  const st = $('#sync-status');
  const unsent = sn.outbox ? `${sn.outbox} unsent · ` : '';
  const link = sn.net === 'online' ? '' : sn.net === 'connecting' ? 'connecting… · ' : (/clock/.test(sn.net_detail) ? sn.net_detail : 'cannot reach the relay, retrying') + ' · ';
  st.textContent = link + unsent + 'relay ' + sn.relay.replace(/^https?:\/\//, ''); st.className = 'sync' + (sn.outbox || sn.net === 'offline' ? ' bad' : ''); st.title = sn.net_detail || '';
  const b = $('#banner');
  const log = '<button type="button" class="linkish" data-a="log">Open the activity log</button>';
  if (sn.revoked) { b.hidden = false; b.className = ''; b.innerHTML = `${icon('door')}<span>This device has been revoked, so it can no longer send or receive. What it already received stays readable here.</span>${log}`; }
  else if (c && c.removed) { b.hidden = false; b.className = ''; b.innerHTML = `${icon('door')}<span>${esc(c.removed_reason || 'You are no longer in this community.')}</span>`; }
  else if (ch && ch.joined && /out of sync|removed/.test(ch.status)) { b.hidden = false; b.className = ''; b.innerHTML = `${icon('sync')}<span>This room is ${esc(ch.status)}.</span>${log}`; }
  else if (ch && ch.joined && ch.status === 'refreshing keys') { b.hidden = false; b.className = 'calm'; b.innerHTML = `${icon('key')}<span>This room is refreshing its keys. Sending resumes in a moment.</span>${log}`; }
  else b.hidden = true;
  const elsewhere = sn.communities.some(x => x.channels.some(y => y.unread && y.gid !== S.gid));
  $('#rooms-dot').hidden = !elsewhere;
  const seenLog = +pref.get('log-seen') || 0;
  $('#activity-dot').hidden = !sn.problems.some(p => p.ts > seenLog);
  if (S.people) renderPeople();
}
// Joins live outside any community, so they show whichever community is open. A refusal stays until it is read.
const joinNotes = sn => [
  ...sn.refused.map(j => ({ k: 'r' + j.id, h: `<div class="room-note">Could not join “${esc(j.name)}”: ${esc(j.reason)}. <button type="button" class="linkish" data-join="${esc(j.id)}">Dismiss</button></div>` })),
  ...sn.joining.map(j => ({ k: 'j' + j.id, h: `<div class="room-note">Asked to join “${esc(j.name)}”. An admin has to let you in; this can take a while if nobody is online. <button type="button" class="linkish" data-join="${esc(j.id)}">Stop waiting</button></div>` })),
];
const roleWords = r => r === 'owner' ? 'the owner' : r === 'admin' ? 'an admin' : 'a member';
const emptyState = (art, title, text, extra = '') => `<div class="empty"><svg class="art" viewBox="0 0 320 190" aria-hidden="true"><use href="#art-${art}"/></svg><h3>${esc(title)}</h3><p>${esc(text)}</p>${extra}</div>`;
const KEYS_WAIT = emptyState('wait', 'The keys are on their way', 'You are in the community, but an admin still has to add this device to this room. It opens by itself once they do.');
function paintEmpty(c, sn) {
  S.msgs = null;
  if (c) return paint([{ k: 'e:norooms', h: emptyState('door', 'No rooms to read yet', 'This community has no rooms you can read yet.') }]);
  if (sn.joining.length) return paint([{ k: 'e:joining', h: emptyState('wait', 'You are on the doorstep', 'Waiting for an admin to let you in…') }]);
  paint([{ k: 'e:none', h: emptyState('door', 'Come on in', 'Create a community or join one with an invite link.', `<div class="empty-acts"><button class="btn" data-a="new-community">New community</button><button class="btn ghost" data-a="join-community">Join with an invite link</button></div>`) }]);
}
function paint(rows, opts) { patch($('#stream'), rows, rowCache, opts); }

// ---- members: names for mentions, the header stack, verified marks ----
async function refreshMembers() {
  const cid = S.cid; if (!cid) { S.members = []; S.membersFor = null; S.membersKey = ''; $('#people-stack').innerHTML = ''; return; }
  try {
    const ms = await rpc('members', { cid }); if (cid !== S.cid) return;
    const k = cid + JSON.stringify(ms); if (k === S.membersKey) return;
    S.membersKey = k; S.members = ms; S.membersFor = cid; assignWax(ms);
    $('#people-stack').innerHTML = ms.slice(0, 3).map(m => seal(m.account, m.name)).join('');
    renderRail(); renderStream();
  } catch { }
}
const nameAlts = names => [...new Set(names.flatMap(n => [n, n.split(/\s+/)[0]]))].filter(Boolean).sort((a, b) => b.length - a.length).map(n => n.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|');
const AT = alts => new RegExp(`(^|[^\\p{L}\\p{N}_@])@(${alts})(?![\\p{L}\\p{N}_])`, 'giu');
function mentionsMe(t) { if (!t || !S.snap) return false; const re = AT(nameAlts([S.snap.me.name])); return re.test(t); }
function fmt(text) {
  const me = S.snap ? S.snap.me.name : '';
  const names = [...S.members.map(m => m.name), me].filter(Boolean);
  const re = names.length ? AT(nameAlts(names.map(esc))) : null;
  const mine = AT(nameAlts([esc(me)]));
  return esc(text).split(/(`[^`\n]+`)/).map((seg, i) => {
    if (i % 2) return `<code>${seg.slice(1, -1)}</code>`;
    let s = seg.replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>').replace(/(^|[\s(])\*([^*\s](?:[^*\n]*[^*\s])?)\*(?=$|[\s.,;:!?)])/gm, '$1<em>$2</em>');
    if (re) s = s.replace(re, (m, pre, n) => { mine.lastIndex = 0; return `${pre}<span class="at${mine.test("@" + n) ? " self" : ""}">@${n}</span>`; });
    return s;
  }).join('');
}
const plain = t => String(t).replace(/\*\*([^*\n]+)\*\*/g, '$1').replace(/(^|[\s(])\*([^*\s](?:[^*\n]*[^*\s])?)\*/g, '$1$2').replace(/`([^`\n]+)`/g, '$1');
function onlyEmoji(t) {
  const s = (t || '').trim(); if (!s || s.length > 28) return false;
  if (!/^(?:\p{Extended_Pictographic}|\p{Emoji_Modifier}|\p{Regional_Indicator}|️|‍|\s)+$/u.test(s)) return false;
  return (window.Intl && Intl.Segmenter ? [...new Intl.Segmenter().segment(s.replace(/\s/g, ''))].length : 1) <= 4;
}
async function refreshMentions() {
  if (refreshMentions.busy) return; refreshMentions.busy = true;
  try {
    let dirty = false;
    for (const c of S.snap.communities) for (const ch of c.channels) {
      if (!ch.unread) { S.mentionSig.delete(ch.gid); if (S.mention.delete(ch.gid)) dirty = true; continue; }
      if (S.mentionSig.get(ch.gid) === ch.unread) continue;
      const list = (await rpc('messages', { gid: ch.gid, limit: ch.unread + 50 })).messages;
      let left = ch.unread, n = 0;
      for (let i = list.length - 1; i >= 0 && left > 0; i--) { const x = list[i]; if (x.mine || x.deleted || x.kind === 'notice') continue; left--; if (mentionsMe(x.text)) n++; }
      S.mentionSig.set(ch.gid, ch.unread);
      if ((S.mention.get(ch.gid) || 0) !== n) { if (n) S.mention.set(ch.gid, n); else S.mention.delete(ch.gid); dirty = true; }
    }
    if (dirty) renderRail();
  } catch { } finally { refreshMentions.busy = false; }
}

// ---- the stream ----
// Failure lines: the words are always the core's own, the heading only says which family they belong to.
function lineKind(t) {
  if (/fell out of sync/.test(t)) return { c: 'behind', ic: 'sync', h: 'This device fell behind' };
  if (/re-added to the group/.test(t)) return { c: 'back', ic: 'key', h: 'Back in the room' };
  if (/sent from this device but this copy/.test(t)) return { c: 'behind', ic: 'sync', h: 'This copy is out of date' };
  if (/^Message #\d+/.test(t)) return { c: 'sealed', h: 'A message could not be opened' };
  return { c: 'note', ic: 'info', h: 'A note from this device' };
}
const crackedSeal = `<svg class="cracked" viewBox="0 0 40 40" aria-hidden="true"><use href="#s9"/><path d="M22 3.500 17.500 13l6 4.500-6.500 6.500 4 5-2.500 8"/></svg>`;
function sysRow(x) {
  const k = lineKind(x.text);
  return `<div class="sys ${k.c}" data-sys="${esc(x.id)}"><div class="who"><span class="name">${k.c === 'sealed' ? 'Unopened' : 'This device'}</span>${k.c === 'sealed' ? crackedSeal : `<span class="sys-ic">${icon(k.ic)}</span>`}</div><div class="txt"><div class="sys-h"><span>${esc(k.h)}</span><button type="button" class="linkish" data-a="log">Activity log</button></div><div class="sys-t">${esc(x.text)}</div></div><time class="when" title="${esc(fmtFull(x.ts))}">${fmtTime(x.ts)}</time></div>`;
}
function gapRow(g) {
  return `<div class="gap"><div class="gap-in"><div class="sys-h">${icon('warn')}Missing from the log</div><div class="sys-t">${esc(g.text)}</div><button type="button" class="linkish" data-a="log">Activity log</button></div></div>`;
}
// A gap is logged by group; the client stops at it, so while it is open nothing newer can appear in this room.
function openGap(m) {
  const pre = 'group ' + S.gid.slice(0, 8) + ': ';
  const p = (S.snap.problems || []).find(n => n.kind === 'gap' && n.text.startsWith(pre)); if (!p) return null;
  const got = /relay sent (\d+)/.exec(p.text);
  if (got && m.messages.some(x => x.seq >= +got[1])) return null;
  return { key: 'gap' + p.ts, text: p.text.slice(pre.length).replace(/^./, c => c.toUpperCase()) + '.' };
}
// Who this really is, when the name alone cannot say: the same six characters on everyone's screen.
const nameMarks = x => (x.same_name ? `<span class="tag" title="Someone else here has this name. This is the start of this person's account id.">${esc(x.tag)}</span>` : '') + (x.role === 'owner' || x.role === 'admin' ? `<span class="chip ok">${esc(x.role)}</span>` : '');
function msgRow(x, cont, m, byId, admin) {
  const mention = !x.mine && !x.deleted && mentionsMe(x.text);
  const person = S.members.find(p => p.account === x.sender);
  const reacts = x.reactions.length ? `<div class="reacts">${x.reactions.map(r => `<button class="react ${r.mine ? 'mine' : ''}" data-id="${esc(x.id)}" data-e="${esc(r.emoji)}" title="${esc(r.names.join(', '))}" aria-pressed="${r.mine}"><span class="em">${esc(r.emoji)}</span><span class="n">${r.count}</span></button>`).join('')}${x.deleted ? '' : `<button class="react add" data-a="react" data-id="${esc(x.id)}" aria-label="Add a reaction" title="Add a reaction">${icon('smile')}</button>`}</div>` : '';
  let body;
  if (x.deleted) body = `<div class="body gone">message deleted</div>`;
  else if (x.kind === 'file') {
    const img = x.file.mime.startsWith('image/');
    const meta = `<span class="small">${esc(fmtSize(x.file.size))}${img ? '' : ` · ${esc((x.file.mime.split('/')[1] || 'file').toUpperCase().slice(0, 12))}`}</span>`;
    const dl = `<a href="#" class="dl" data-id="${esc(x.id)}" title="Download ${esc(x.file.name)}">${icon('download')}<span>Download</span></a>`;
    body = (img ? `<div class="imgslot" data-id="${esc(x.id)}"></div><div class="file photo" data-file="${esc(x.id)}"><span class="fname">${esc(x.file.name)}</span>${meta}${dl}</div>`
      : `<div class="file" data-file="${esc(x.id)}">${icon('file', 'fi')}<div class="fbody"><div class="fname">${esc(x.file.name)}</div>${meta}</div>${dl}</div>`)
      + (x.text ? `<div class="body">${fmt(x.text)}</div>` : '');
  } else body = `<div class="body${onlyEmoji(x.text) ? ' jumbo' : ''}">${fmt(x.text)}${x.edited ? '<span class="edited">(edited)</span>' : ''}</div>`;
  const src = x.reply && byId.get(x.reply.id);
  const quote = x.reply ? `<button type="button" class="quote" data-a="goto" data-id="${esc(x.reply.id)}" title="Go to this message">${seal(src ? src.sender : x.reply.name, x.reply.name, 'tiny')}<b>${esc(x.reply.name)}</b><span>${esc(plain(x.reply.text))}</span></button>` : '';
  const state = x.status === 'sending' ? `<span class="state">${icon('clock')}sending…</span>` : '';
  const acts = x.deleted ? '' : `<div class="actions"><button data-a="reply" data-id="${esc(x.id)}" title="Reply" aria-label="Reply">${icon('reply')}</button><button data-a="react" data-id="${esc(x.id)}" title="React" aria-label="React">${icon('smile')}</button>${x.mine && x.kind === 'text' ? `<button data-a="edit" data-id="${esc(x.id)}" title="Edit" aria-label="Edit">${icon('pencil')}</button>` : ''}${x.mine || admin ? `<button data-a="del" data-id="${esc(x.id)}" title="Delete" aria-label="Delete">${icon('trash')}</button>` : ''}</div>`;
  const who = cont ? `<div class="who"><span class="sr">${esc(x.name)}</span></div>`
    : `<div class="who"><span class="name">${esc(x.name)}${nameMarks(x)}${person && person.verified ? `<span class="vf" title="Safety number verified">${icon('check')}</span>` : ''}</span>${seal(x.sender, x.name)}</div>`;
  return `<article class="msg ${cont ? 'cont' : ''} ${x.mine ? 'mine' : ''} ${mention ? 'mention' : ''} ${x.deleted ? 'deleted' : ''} ${x.status === 'sending' ? 'sending' : ''}" style="--wax:${wax(x.sender)}" data-id="${esc(x.id)}">${who}<div class="txt">${quote}${body}${reacts}</div><time class="when" title="${esc(fmtFull(x.ts))}">${fmtTime(x.ts)}${state}</time>${acts}</article>`;
}
function renderStream() {
  const st = $('#stream'), m = S.msgs; if (!m) return;
  const near = st.scrollHeight - st.scrollTop - st.clientHeight < 80;
  const ch = currentChannel(), c = currentCommunity(), admin = c && (c.role === 'admin' || c.role === 'owner');
  if (S.first && ch && ch.unread > 0) {   // the first unread message, counted the way the core counts it
    let left = ch.unread;
    for (let i = m.messages.length - 1; i >= 0 && left > 0; i--) { const x = m.messages[i]; if (x.mine || x.deleted || x.kind === 'notice') continue; left--; if (!left) { S.firstNew = x.id; S.newCount = ch.unread; } }
  }
  const byId = new Map(m.messages.map(x => [x.id, x]));
  const rows = m.more ? [{ k: 'e:older', h: `<div class="day"><button type="button" class="linkish" data-a="older">Show earlier messages</button></div>` }] : []; let lastDay = '', prev = null, known = 0;
  for (const x of m.messages) {
    if (x.ts && dayKey(x.ts) !== lastDay) { lastDay = dayKey(x.ts); rows.push({ k: 'd' + lastDay, h: `<div class="day"><span>${esc(fmtDay(x.ts))}</span></div>` }); prev = null; }
    if (x.id === S.firstNew) { rows.push({ k: 'new', h: `<div class="newline" role="separator"><span>${S.newCount} new</span></div>` }); prev = null; }
    if (x.kind === 'notice') { rows.push({ k: x.id, h: sysRow(x) }); prev = null; continue; }
    const cont = prev && prev.sender === x.sender && x.ts - prev.ts < 300 && !x.reply;
    prev = x;
    rows.push({ k: x.id, h: msgRow(x, !!cont, m, byId, admin) });
    if (rowCache.has(x.id)) known++;
  }
  const gap = openGap(m); if (gap) rows.push({ k: gap.key, h: gapRow(gap) });
  if (!rows.length && ch && !ch.joined) rows.push({ k: 'e:keys', h: KEYS_WAIT });
  if (!rows.length) rows.push({ k: 'e:quiet' + S.members.length, h: emptyState('table', ch ? `Nobody has said anything in ${ch.name}` : 'No messages yet', 'No messages yet. Say hello.', admin && S.members.length === 1 ? '<div class="empty-acts"><button class="btn" data-a="invite">Invite someone</button></div>' : '') });
  let added = 0;
  st.setAttribute('aria-busy', S.first);   // a room change should not be read out line by line
  paint(rows, {
    onNew: el => { if (S.first) return; if (el.matches('.msg, .sys, .gap')) { el.classList.add('fresh'); setTimeout(() => el.classList.remove('fresh'), 400); if (el.matches('.msg:not(.mine)')) added++; } },
    onChange: (el, old) => {
      if (old.classList.contains('open')) el.classList.add('open');
      const had = new Map($$('.react[data-e]', old).map(r => [r.dataset.e, $('.n', r).textContent]));
      for (const r of $$('.react[data-e]', el)) if (had.get(r.dataset.e) !== $('.n', r).textContent) r.classList.add('pop');
    },
  });
  const t = m.typing.length ? m.typing.join(', ') + (m.typing.length > 1 ? ' are typing' : ' is typing') : '';
  const ty = $('#typing'); ty.innerHTML = t ? `<span>${esc(t)}</span><span class="dots" aria-hidden="true"><i></i><i></i><i></i></span>` : ''; $('#hint').hidden = !!t;
  fillImages();
  if (S.jumpTo) { const el = $(`.msg[data-id="${CSS.escape(S.jumpTo)}"]`, st); S.jumpTo = null; if (el) flashTo(el); }
  else if (S.first) {
    const nl = $('.newline', st);
    if (nl && nl.offsetTop > st.clientHeight * .4) st.scrollTop = nl.offsetTop - st.clientHeight * .3; else st.scrollTop = st.scrollHeight;
    st.classList.remove('swap'); void st.offsetWidth; st.classList.add('swap');
  }
  else if (near) st.scrollTop = st.scrollHeight;
  else if (added) { S.below += added; }
  if (S.keep != null) { st.scrollTop = st.scrollHeight - S.keep; S.keep = null; }
  S.first = false; S.painted = Date.now(); st.setAttribute('aria-busy', false); onScroll();
}
function flashTo(el) {
  el.scrollIntoView({ block: 'center', behavior: calm() ? 'auto' : 'smooth' });
  el.classList.remove('flash'); void el.offsetWidth; el.classList.add('flash'); setTimeout(() => el.classList.remove('flash'), 1700);
}
function onScroll() {
  const st = $('#stream'), d = st.scrollHeight - st.scrollTop - st.clientHeight;
  S.pinned = d < 80; if (S.pinned) S.below = 0;
  const j = $('#jump'); j.hidden = d < 260;
  $('#jump-label').textContent = S.below ? `${S.below} new` : 'Latest';
  j.classList.toggle('has-new', !!S.below);
}
// While scrolling through old pages, a small label says which day you are in.
function showDay() {
  const st = $('#stream'), f = $('#day-float'); let cur = null;
  if (Date.now() - S.painted < 500) return;   // the scroll came from us placing the room, not from reading
  for (const d of $$('.day', st)) { if (d.offsetTop < st.scrollTop + 4) cur = d; else break; }
  if (!cur || cur.offsetTop > st.scrollTop - 6) { f.classList.remove('on'); return; }
  f.textContent = cur.textContent; f.classList.add('on');
  clearTimeout(showDay.t); showDay.t = setTimeout(() => f.classList.remove('on'), 1100);
}
$('#stream').addEventListener('scroll', () => { onScroll(); showDay(); }, { passive: true });
$('#stream').addEventListener('load', () => { if (S.pinned) { const st = $('#stream'); st.scrollTop = st.scrollHeight; } }, true);
$('#jump').onclick = () => { const st = $('#stream'); st.scrollTo({ top: st.scrollHeight, behavior: calm() ? 'auto' : 'smooth' }); };

const QUICK = ['👍', '❤️', '😂', '🎉', '😮', '🙏', '👀', '🥂'];
const closePop = () => $$('.emojis').forEach(x => x.remove());
function emojiPop(btn, m) {
  closePop();
  const p = document.createElement('div'); p.className = 'emojis'; p.setAttribute('role', 'menu');
  for (const em of QUICK) {
    const b = document.createElement('button'); b.textContent = em; b.setAttribute('aria-label', 'React with ' + em);
    if (m.reactions.find(r => r.emoji === em && r.mine)) b.className = 'on';
    b.onclick = async e => { e.stopPropagation(); p.remove(); await rpc('react', { gid: S.gid, id: m.id, emoji: em, on: !(m.reactions.find(r => r.emoji === em && r.mine)) }); tick(); };
    p.appendChild(b);
  }
  document.body.appendChild(p);
  const r = btn.getBoundingClientRect(), w = p.offsetWidth, h = p.offsetHeight;
  p.style.left = Math.max(8, Math.min(innerWidth - w - 8, r.right - w)) + 'px';
  p.style.top = (r.top - h - 8 > 8 ? r.top - h - 8 : r.bottom + 8) + 'px';
}
function showActivity() { $('#btn-activity').click(); }
$('#banner').addEventListener('click', e => { if (e.target.closest('[data-a=log]')) showActivity(); });
$('#stream').addEventListener('click', async e => {
  const t = e.target, byId = id => S.msgs && S.msgs.messages.find(x => x.id === id);
  const b = t.closest('[data-a]');
  if (b) {
    e.stopPropagation();
    const a = b.dataset.a, id = b.dataset.id, m = byId(id);
    if (a === 'log') showActivity();
    else if (a === 'older') { const st = $('#stream'); S.keep = st.scrollHeight - st.scrollTop; S.limit += 200; S.lastMsgs = ''; tick(); }
    else if (a === 'invite') $('#btn-invite').click();
    else if (a === 'new-community') newCommunity();
    else if (a === 'join-community') joinCommunity();
    else if (a === 'goto') { const el = $(`.msg[data-id="${CSS.escape(id)}"]`); if (el) flashTo(el); else toast('That message is not on this device.'); }
    else if (a === 'reply') { S.reply = m; S.edit = null; renderComposer(); $('#input').focus(); }
    else if (a === 'edit') { startEdit(m); }
    else if (a === 'del') { try { await rpc('delete', { gid: S.gid, id }); tick(); } catch (er) { toast(er.message); } }
    else if (a === 'react') emojiPop(b, m);
    return;
  }
  const r = t.closest('.react[data-e]');
  if (r) { const m = byId(r.dataset.id), x = m.reactions.find(y => y.emoji === r.dataset.e); await rpc('react', { gid: S.gid, id: m.id, emoji: r.dataset.e, on: !x.mine }); tick(); return; }
  const dl = t.closest('.dl');
  if (dl) {
    e.preventDefault();
    try { const f = await fileBlob(dl.dataset.id); const u = URL.createObjectURL(f.blob); const l = document.createElement('a'); l.href = u; l.download = f.name; l.click(); setTimeout(() => URL.revokeObjectURL(u), 5000); } catch (er) { toast(er.message); }
    return;
  }
  if (t.matches('.imgslot img')) { lightbox(t.src, t.alt); return; }
  // Touch has no hover: a tap on a message shows its tools.
  if (matchMedia('(hover: none)').matches) { const row = t.closest('.msg'); const was = row && row.classList.contains('open'); $$('.msg.open').forEach(x => x.classList.remove('open')); if (row && !was) row.classList.add('open'); }
});
function lightbox(src, alt) {
  const w = document.createElement('div'); w.className = 'lightbox';
  const big = new Image(); big.src = src; big.alt = alt; w.appendChild(big);
  const done = () => { w.remove(); document.removeEventListener('keydown', key); };
  const key = e => { if (e.key === 'Escape') done(); };
  w.onclick = done; document.addEventListener('keydown', key); document.body.appendChild(w);
}
// An attachment comes in pieces, so no single message between the core and this page is tens of megabytes.
const unb64 = t => Uint8Array.from(atob(t.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0));
async function fileBlob(id) {
  const gid = S.gid, f = await rpc('file_open', { gid, id }), parts = [];
  for (let at = 0; at < f.size; at += f.chunk) parts.push(unb64((await rpc('file_read', { gid, id, offset: at })).data));
  return { name: f.save_name, blob: new Blob(parts, { type: f.mime }) };
}
// Object URLs, never strings of markup. The newest ones stay; an old image is simply fetched again from the local cache.
const MAX_IMGS = 48;
function keepImg(id, url) {
  S.imgs.set(id, url);
  while (S.imgs.size > MAX_IMGS) { const [k, u] = S.imgs.entries().next().value; S.imgs.delete(k); if (u) URL.revokeObjectURL(u); }
}
function fillImages() {
  $$('.imgslot').forEach(async slot => {
    if (slot.firstChild) return;
    const id = slot.dataset.id;
    if (!S.imgs.has(id)) {
      try { keepImg(id, URL.createObjectURL((await fileBlob(id)).blob)); }
      catch (er) { keepImg(id, null); slot.innerHTML = `<div class="img-fail">${icon('warn')}<span>${esc(er.message)}</span></div>`; return; }
    }
    const u = S.imgs.get(id), m = S.msgs && S.msgs.messages.find(x => x.id === id);
    if (u && !slot.firstChild) { const i = new Image(); i.src = u; i.alt = m && m.file ? m.file.name : ''; slot.appendChild(i); }
  });
}
function startEdit(m) { S.edit = m; S.reply = null; $('#input').value = m.text; sizeInput(); renderComposer(); $('#input').focus(); }

// ---- composer ----
function sizeInput() { const i = $('#input'); i.style.height = 'auto'; i.style.height = Math.min(i.scrollHeight, 220) + 'px'; $('#send').classList.toggle('ready', !!i.value.trim()); }
function renderComposer() {
  const r = $('#replybar');
  const excerpt = m => m.text || (m.file ? m.file.name : '');
  if (S.reply) { r.hidden = false; r.innerHTML = `${icon('reply')}<span class="what">Replying to <b>${esc(S.reply.name)}</b> <span class="ex">${esc(plain(excerpt(S.reply)).slice(0, 120))}</span></span><button type="button" class="mini" id="cancel" aria-label="Cancel reply" title="Cancel">${icon('x')}</button>`; }
  else if (S.edit) { r.hidden = false; r.innerHTML = `${icon('pencil')}<span class="what">Editing your message</span><button type="button" class="mini" id="cancel" aria-label="Cancel edit" title="Cancel">${icon('x')}</button>`; }
  else r.hidden = true;
  $('#composer').classList.toggle('editing', !!S.edit);
  $('#send-label').textContent = S.edit ? 'Save' : 'Send';
  const ch = currentChannel(); const ph = ch ? `Write to ${ch.name}` : 'Write a message'; $('#input').placeholder = ph; $('#input').setAttribute('aria-label', ph);
  const c = $('#cancel'); if (c) c.onclick = cancelCompose;
}
function cancelCompose() { if (S.edit) $('#input').value = ''; S.reply = null; S.edit = null; sizeInput(); renderComposer(); }
async function send() {
  const inp = $('#input'); const text = inp.value; if (!text.trim() || !S.gid) return;
  try {
    if (S.edit) await rpc('edit', { gid: S.gid, id: S.edit.id, text });
    else await rpc('send_text', { gid: S.gid, text, reply: S.reply ? S.reply.id : null });
    inp.value = ''; inp.style.height = ''; S.drafts.delete(S.gid); S.reply = null; S.edit = null; renderComposer(); sizeInput(); mentionClose(); S.lastMsgs = ''; S.pinned = true; const st = $('#stream'); st.scrollTop = st.scrollHeight; tick();
  } catch (e) { toast(e.message); }
}
$('#send').onclick = send;
// @ names: a small picker above the composer. Text stays plain, so mentions need nothing from the core.
const MP = { list: [], at: 0, start: 0 };
function mentionClose() { $('#mention-pop').hidden = true; MP.list = []; }
function mentionOpen() {
  const i = $('#input'), upto = i.value.slice(0, i.selectionStart), m = /(?:^|\s)@([\p{L}\p{N}_-]*)$/u.exec(upto);
  if (!m) return mentionClose();
  const q = m[1].toLowerCase();
  MP.list = S.members.filter(p => !p.me && p.name.toLowerCase().includes(q)).slice(0, 6);
  if (!MP.list.length) return mentionClose();
  MP.start = upto.length - m[1].length - 1; MP.at = Math.min(MP.at, MP.list.length - 1);
  const pop = $('#mention-pop');
  pop.innerHTML = MP.list.map((p, n) => `<button type="button" class="${n === MP.at ? 'on' : ''}" data-n="${n}">${seal(p.account, p.name, 'small')}<span>${esc(p.name)}</span></button>`).join('');
  pop.hidden = false;
}
function mentionPick(n) {
  const i = $('#input'), p = MP.list[n]; if (!p) return;
  const tail = i.value.slice(i.selectionStart), ins = '@' + p.name + ' ';
  i.value = i.value.slice(0, MP.start) + ins + tail; const at = MP.start + ins.length; i.setSelectionRange(at, at); i.focus(); mentionClose(); sizeInput();
}
$('#mention-pop').addEventListener('mousedown', e => { const b = e.target.closest('button'); if (b) { e.preventDefault(); mentionPick(+b.dataset.n); } });
$('#input').onkeydown = e => {
  if (e.isComposing || e.keyCode === 229) return; // Enter is confirming an input method's word, not sending
  if (MP.list.length) {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); MP.at = (MP.at + (e.key === 'ArrowDown' ? 1 : MP.list.length - 1)) % MP.list.length; mentionOpen(); return; }
    if (e.key === 'Enter' || e.key === 'Tab') { e.preventDefault(); mentionPick(MP.at); return; }
    if (e.key === 'Escape') { mentionClose(); return; }
  }
  if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send(); return; }
  if (e.key === 'Escape' && (S.reply || S.edit)) { cancelCompose(); return; }
  if (e.key === 'ArrowUp' && !e.target.value && S.msgs) { const m = [...S.msgs.messages].reverse().find(x => x.mine && x.kind === 'text' && !x.deleted); if (m) { e.preventDefault(); startEdit(m); return; } }
  if (S.gid) rpc('typing', { gid: S.gid }).catch(() => { });
};
$('#input').oninput = () => { sizeInput(); mentionOpen(); };
$('#input').onblur = () => setTimeout(mentionClose, 120);
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
  const others = ms.filter(m => !m.me), done = others.filter(m => m.verified).length;
  const rank = m => m.role === 'owner' ? 0 : m.role === 'admin' ? 1 : 2;
  const row = m => `<div class="person"><span class="pseal">${seal(m.account, m.name)}${m.verified ? `<span class="vf-badge" title="Safety number verified">${icon('check')}</span>` : ''}</span><div class="nm"><b><span>${esc(m.name)}</span>${m.me ? '<span class="you">you</span>' : ''}${m.role !== 'member' ? `<span class="chip ok">${m.role}</span>` : ''}</b><span class="small">${m.same_name ? `<span class="tag" title="Someone else here has this name.">${esc(m.tag)}</span> · ` : ''}${m.devices} device${m.devices === 1 ? '' : 's'}${m.me ? '' : m.verified ? ' · <span class="ok">verified</span>' : ' · not verified'}</span></div><span class="p-acts">${m.me ? '' : `<button class="mini" data-a="verify" data-acct="${esc(m.account)}" title="Verify safety number" aria-label="Verify safety number of ${esc(m.name)}">${icon('key')}</button>`}${(iAmOwner && !m.me && m.role !== 'owner') ? `<button class="mini" data-a="role" data-acct="${esc(m.account)}" data-r="${m.role}" title="Change role" aria-label="Change role of ${esc(m.name)}">${icon('star')}</button>` : ''}${(!m.me && m.role !== 'owner' && (iAmOwner || (iAmAdmin && m.role === 'member'))) ? `<button class="mini" data-a="kick" data-acct="${esc(m.account)}" data-n="${esc(m.name)}" title="Remove from community" aria-label="Remove ${esc(m.name)}">${icon('x')}</button>` : ''}</span></div>`;
  el.innerHTML = `<div class="p-head"><h2>People <span class="count">${ms.length}</span></h2><button class="mini" id="people-close" aria-label="Close people" title="Close">${icon('x')}</button></div>
    ${others.length ? `<div class="p-trust"><div class="meter" aria-hidden="true"><i style="width:${others.length ? done / others.length * 100 : 0}%"></i></div><span>${done ? `You have verified ${done} of ${others.length}` : `Nobody verified yet. Compare safety numbers with the ${icon('key', 'inline')}`}</span></div>` : ''}
    <div class="p-list">${[...ms].sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name)).map(row).join('')}</div>` +
    `<div class="p-foot">${c.role !== 'owner' ? '<button class="btn ghost" id="leave">Leave community</button>' : ''}${iAmAdmin ? '<button class="btn ghost" id="rename">Rename</button>' : ''}</div>`;
  el.querySelectorAll('button[data-a]').forEach(b => b.onclick = () => personAction(b.dataset, c));
  $('#people-close').onclick = () => $('#btn-people').click();
  const lv = $('#leave'); if (lv) lv.onclick = async () => { if ((await modal('Leave ' + c.name + '?', '<p>You will stop receiving messages and your keys for this community are discarded after an admin removes you.</p>', [{ label: 'Cancel', primary: false }, { label: 'Leave', danger: true, run: async () => { await rpc('leave', { cid: c.id }); } }])) === 1) { toast('Asked to leave. An admin finishes it.'); tick(); } };
  const rn = $('#rename'); if (rn) rn.onclick = () => modal('Rename community', `<input type="text" id="n" value="${esc(c.name)}" aria-label="Community name">`, [{ label: 'Cancel', primary: false }, { label: 'Rename', run: async w => { await rpc('rename_community', { cid: c.id, name: val(w, 'n') }); } }]).then(tick);
}
async function personAction(d, c) {
  if (d.a === 'kick') {
    await modal('Remove ' + d.n + '?', `<p>They are removed from every room and the keys are rotated, so they cannot read anything sent from now on. They can still read what they already received. They cannot rejoin with an old invite.</p>`, [{ label: 'Cancel', primary: false }, { label: 'Remove', danger: true, run: async () => { await rpc('kick', { cid: c.id, account: d.acct }); } }]);
    setTimeout(() => { tick(); renderPeople(); }, 800);
  } else if (d.a === 'role') {
    const to = d.r === 'admin' ? 'member' : 'admin';
    await modal('Change role', `<p>Make this person ${to === 'admin' ? 'an <b>admin</b> (can invite, remove members and create rooms)' : 'a regular <b>member</b>'}?</p><p class="muted small">Or hand them the whole community: they become the owner and you stay on as an admin, free to leave. Only they could hand it back.</p>`, [{ label: 'Cancel', primary: false }, { label: 'Make them the owner', danger: true, run: async () => { await rpc('transfer_ownership', { cid: c.id, account: d.acct }); } }, { label: 'Yes', run: async () => { await rpc('set_role', { cid: c.id, account: d.acct, role: to }); } }]);
    setTimeout(() => { tick(); renderPeople(); }, 800);
  } else if (d.a === 'verify') {
    const s = await rpc('safety', { account: d.acct });
    const parts = s.number.split(' ');
    const groups = parts.map((g, i) => `<button type="button" class="grp" aria-pressed="false" title="Tick off as you compare"><i>${i + 1}</i>${esc(g)}</button>`).join('');
    const me = S.snap.me;
    const top = `<div class="pair">${seal(me.account, me.name, 'big')}<span class="tie ${s.verified ? 'on' : ''}">${icon(s.verified ? 'check' : 'key')}</span>${seal(d.acct, s.name, 'big')}</div>`;
    const p = modal('Verify ' + s.name, `<div class="state-line ${s.verified ? 'ok' : ''}">${s.verified ? 'Verified by you' : 'Not verified yet'}</div><p>Compare these numbers with ${esc(s.name)} in person or over a call. If they match, nobody (including the relay) is sitting between you.</p><div class="safety-wrap"><div><div class="safety">${groups}</div><div class="tally" aria-live="polite">Tap each group as you read it out.</div></div><div class="qr">${s.qr}</div></div><p class="muted small">The QR contains the same number; scan it on the other device or compare by eye.</p>`,
      [{ label: 'Close', primary: false }, { label: s.verified ? 'Mark as not verified' : 'They match, mark verified', run: async () => { await rpc('set_verified', { account: d.acct, on: !s.verified }); } }], { cls: 'safety-modal', top });
    const w = $('.safety-modal');
    w.addEventListener('click', e => {
      const b = e.target.closest('.grp'); if (!b) return;
      b.setAttribute('aria-pressed', b.getAttribute('aria-pressed') !== 'true');
      const n = $$('.grp[aria-pressed=true]', w).length;
      $('.tally', w).textContent = n === parts.length ? `All ${n} groups compared. If every one matched, mark them verified.` : n ? `${n} of ${parts.length} groups compared.` : 'Tap each group as you read it out.';
    });
    await p;
    S.membersKey = ''; refreshMembers(); renderPeople();
  }
}
$('#btn-people').onclick = () => { S.people = !S.people; $('#people').hidden = !S.people; $('#btn-people').setAttribute('aria-expanded', S.people); $('#app').classList.toggle('people-open', S.people); if (S.people) renderPeople(); };
$('#btn-rooms').onclick = e => { e.stopPropagation(); railOpen(true); };
$('#rail-scrim').onclick = () => railOpen(false);

// ---- community switcher, create, join, invite ----
const newCommunity = () => modal('New community', `<p class="muted small">A private place with its own rooms. You will be its owner, and only people you invite can read it.</p><label for="n">Name</label><input type="text" id="n" placeholder="e.g. Book club">`, [{ label: 'Cancel', primary: false }, { label: 'Create', run: async w => { const r = await rpc('create_community', { name: val(w, 'n') }); S.cid = r.id; S.gid = null; resetRoom(); } }]).then(() => tick());
const joinCommunity = () => modal('Join with an invite link', `<p class="muted small">Paste the link you were sent. An admin has to be online (now or later) to let you in.</p><input type="text" id="l" placeholder="commons://join/…" aria-label="Invite link" spellcheck="false">`, [{ label: 'Cancel', primary: false }, { label: 'Join', run: async w => { await rpc('join', { link: val(w, 'l') }); toast('Request sent'); } }]).then(() => tick());
$('#switcher').onclick = e => {
  e.stopPropagation(); const m = $('#sw-menu'); if (!m.hidden) { m.hidden = true; return; }
  m.innerHTML = S.snap.communities.map(c => `<button data-c="${esc(c.id)}" role="menuitem" class="${c.id === S.cid ? 'on' : ''}">${crest(c.id, c.name, 'small')}<span class="c-nm">${esc(c.name)}<span class="small">${c.removed ? 'you left' : `${c.channels.length} room${c.channels.length === 1 ? '' : 's'}`}</span></span>${c.unread ? `<span class="pill">${c.unread}</span>` : ''}</button>`).join('') + `${S.snap.communities.length ? '<hr>' : ''}<button id="mk-new" role="menuitem">${icon('plus')}<span>New community…</span></button><button id="mk-join" role="menuitem">${icon('key')}<span>Join with an invite link…</span></button>`;
  m.hidden = false;
  m.querySelectorAll('[data-c]').forEach(b => b.onclick = () => { if (S.cid !== b.dataset.c) { S.cid = b.dataset.c; S.gid = null; resetRoom(); S.membersKey = ''; refreshMembers(); } m.hidden = true; renderRail(); tick(); });
  $('#mk-new').onclick = () => { m.hidden = true; newCommunity(); };
  $('#mk-join').onclick = () => { m.hidden = true; joinCommunity(); };
};
document.addEventListener('click', e => { $('#sw-menu').hidden = true; closePop(); if (!e.target.closest('.search-wrap, #btn-search')) closeSearch(); });
document.addEventListener('keydown', e => { if (e.key !== 'Escape') return; closePop(); closeSearch(); $('#sw-menu').hidden = true; railOpen(false); });
$('#add-room').onclick = () => modal('New room', `<label for="n">Name</label><input type="text" id="n" placeholder="e.g. music">`, [{ label: 'Cancel', primary: false }, { label: 'Create', run: async w => { const r = await rpc('create_channel', { cid: S.cid, name: val(w, 'n') }); S.gid = r.id; resetRoom(); } }]).then(() => tick());
$('#btn-invite').onclick = () => {
  const c = currentCommunity(); let link = null;
  const open = (c.invites || []).filter(i => i.mine && i.expires > Date.now() / 1000);
  const openList = open.length ? `<details class="open-invites"><summary>Open invites <span class="count">${open.length}</span></summary>${open.map(i => `<div class="inv-row"><span>${i.uses_left == null ? 'Unlimited' : `${i.uses_left} use${i.uses_left === 1 ? '' : 's'} left`} · ${esc(fmtLeft(i.expires))}</span><button type="button" class="linkish" data-rev="${esc(i.id)}">Revoke</button></div>`).join('')}</details>` : '';
  const p = modal('Invite someone', `<div id="inv-form"><label>Valid for</label>${seg('ttl', [['3600', '1 hour'], ['86400', '1 day'], ['604800', '7 days']], '604800')}<label>Uses</label>${seg('uses', [['1', 'One person'], ['5', 'Up to 5 people'], ['', 'Unlimited']], '1')}</div><div id="out"></div>${openList}`,
    [{ label: 'Close', primary: false }, { label: 'Create link', run: async w => {
      if (link) { await copy(link, 'Invite link copied'); return false; }
      const ttl = +val(w, 'ttl'), uses = val(w, 'uses');
      const r = await rpc('create_invite', { cid: S.cid, ttl, uses: uses ? +uses : null });
      link = r.link;
      const until = new Date((Date.now() / 1000 + ttl) * 1000).toLocaleString([], { weekday: 'short', day: 'numeric', month: 'short', hour: 'numeric', minute: '2-digit' });
      const admits = !uses ? 'Admits anyone with the link' : uses === '1' ? 'Admits one' : `Admits up to ${uses}`;
      $('#inv-form', w).hidden = true;
      $('#out', w).innerHTML = `<div class="ticket"><div class="t-main"><div class="t-head">${crest(c.id, c.name)}<div><div class="t-club">${esc(c.name)}</div><div class="t-admit">${esc(admits)}</div></div></div><div class="code">${esc(r.link)}</div><div class="t-until">Valid until ${esc(until)}</div></div><div class="t-stub"><div class="qr">${r.qr}</div></div></div><p>Send this link privately. Whoever has it can ask to join.</p><p class="muted small">An admin has to be online (now or later) to let them in.</p>`;
      $('.acts button:last-child', w).textContent = 'Copy link';
      return false;
    } }], { cls: 'invite-modal' });
  const w = $('.invite-modal');
  w.addEventListener('click', async e => { const b = e.target.closest('[data-rev]'); if (!b) return; try { await rpc('revoke_invite', { cid: c.id, id: b.dataset.rev }); b.closest('.inv-row').remove(); toast('Invite revoked'); tick(); } catch (er) { toast(er.message); } });
  return p;
};

// ---- search ----
function closeSearch() { $('#search-pop').hidden = true; $('#head').classList.remove('searching'); }
let st_t;
$('#btn-search').onclick = e => { e.stopPropagation(); $('#head').classList.add('searching'); $('#search').focus(); };
$('#search').oninput = e => {
  clearTimeout(st_t); const pop = $('#search-pop');
  st_t = setTimeout(async () => {
    const q = e.target.value.trim(); if (q.length < 2) { pop.hidden = true; return; }
    const r = await rpc('search', { q });
    const mark = t => esc(t).replace(new RegExp(esc(q).replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi'), x => `<mark>${x}</mark>`);
    pop.innerHTML = r.length ? r.slice(0, 40).map(x => `<button class="hit" data-c="${esc(x.cid)}" data-g="${esc(x.gid)}" data-id="${esc(x.id)}"><span class="k">${esc(x.community)} / ${esc(x.channel)}</span><span class="t"><b>${esc(x.from)}</b> ${mark(x.text)}</span></button>`).join('') : '<p class="muted">Nothing found. Search only covers messages stored on this device.</p>';
    pop.hidden = false;
  }, 220);
};
$('#search-pop').addEventListener('click', e => {
  const b = e.target.closest('.hit'); if (!b) return;
  if (S.cid !== b.dataset.c) { S.cid = b.dataset.c; S.gid = null; S.membersKey = ''; refreshMembers(); }
  closeSearch(); $('#search').value = ''; openRoom(b.dataset.g, b.dataset.id);
  if (S.msgs && !S.first) renderStream();
});

// ---- devices, activity, settings ----
$('#btn-devices').onclick = async () => {
  const render = async w => {
    const ds = await rpc('devices');
    $('#mb', w).innerHTML = `<p class="muted small">Every device holds its own keys. A lost device must be revoked: it is then removed from every room and cannot read anything new.</p>` +
      ds.map(d => `<div class="person device">${icon('devices', 'big')}<div class="nm"><b class="mono">${d.device.slice(0, 12)}</b><span class="small">added ${new Date(d.created * 1000).toLocaleDateString()}${d.this ? ' · this device' : ''}</span></div>${d.revoked ? '<span class="chip">revoked</span>' : (d.this ? '' : `<button class="btn ghost slim" data-r="${d.device}">Revoke</button>`)}</div>`).join('');
    $('#mb', w).querySelectorAll('[data-r]').forEach(b => b.onclick = async () => { try { await rpc('revoke_device', { device: b.dataset.r }); render(w); } catch (e) { toast(e.message); } });
  };
  const p = modal('Devices and recovery', `<div id="mb"></div><label for="code">Add a device</label><input type="text" id="code" placeholder="Paste the code shown on the new device (commons-pair:…)" spellcheck="false"><div class="muted small" id="chk"></div>
    <label>Recovery key</label><button class="btn ghost slim" id="showkey">Show my recovery key</button><div id="key"></div>`,
    [{ label: 'Close', primary: false }, { label: 'Approve new device', run: async w => { const r = await rpc('approve_pairing', { code: val(w, 'code') }); $('#chk', w).textContent = 'Approved. The new device will join your rooms within a few seconds. Check code: ' + r.check; return false; } }]);
  setTimeout(() => { const w = document.querySelector('.modal'); if (!w) return; render(w); $('#code', w).oninput = async e => { try { const r = await rpc('pairing_preview', { code: e.target.value }); $('#chk', w).textContent = `Check code ${r.check}. It must match the new device before you approve.`; } catch (er) { $('#chk', w).textContent = ''; } }; $('#showkey', w).onclick = () => { $('#key', w).innerHTML = `<label for="keypp">Your passphrase</label><input type="password" id="keypp" autocomplete="current-password"><button class="btn ghost slim" id="keygo">Show</button><div class="muted small" id="keyerr" role="alert"></div>`; $('#keygo', w).onclick = async () => { try { $('#key', w).innerHTML = `<div class="code key-code">${esc((await rpc('recovery_key', { passphrase: $('#keypp', w).value })).key)}</div>`; } catch (er) { $('#keyerr', w).textContent = 'Wrong passphrase.'; } }; }; }, 30);
  await p;
};
$('#btn-activity').onclick = async () => {
  const ns = await rpc('notices');
  pref.set('log-seen', String(Math.floor(Date.now() / 1000))); $('#activity-dot').hidden = true;
  const stamp = ts => new Date(ts * 1000).toLocaleString([], { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });
  modal('Activity log', `<p class="muted small">Everything unusual the app noticed, in plain words. Nothing here is hidden from you: if a message could not be read, it says why.</p>` + (ns.length ? `<div class="log">${ns.map(n => `<div class="log-row ${['decrypt', 'broken', 'rollback', 'reject', 'gap'].includes(n.kind) ? 'bad' : ''}"><span class="k">${esc(n.kind)}</span><span class="v">${esc(n.text)}</span><time>${esc(stamp(n.ts))}</time></div>`).join('')}</div>` : '<p class="muted">Nothing yet.</p>'), undefined, { cls: 'log-modal' });
};
$('#btn-settings').onclick = () => modal('Settings', `<label for="n">Display name</label><input type="text" id="n" value="${esc(S.snap.me.name)}">
  <label>Theme</label>${seg('th', [['', 'Match system'], ['light', 'Daylight'], ['dark', 'After hours']], pref.get('theme') || '')}
  <label>Message type</label>${seg('rd', [['', 'Book'], ['plain', 'Plain']], pref.get('read') || '')}
  <label class="check"><input type="checkbox" id="ty" ${S.snap.typing_enabled ? 'checked' : ''}><span>Send “typing…” hints (the relay can see that a message-sized packet was sent at that moment)</span></label>
  <label>Calls</label><button type="button" class="btn ghost" id="set-gamekeys">Game keys…</button><p class="muted small">Mute yourself in Commons while you talk to your game team.</p>
  <label for="rl">Relay address</label><input type="text" id="rl" value="${esc(S.snap.relay)}" spellcheck="false"><p class="muted small">Change this only if your relay moved. A different relay does not have your communities.<br>Your account id: <span class="mono">${esc(S.snap.me.account.slice(0, 16))}…</span></p>`,
  [{ label: 'Cancel', primary: false }, { label: 'Save', run: async w => { if (val(w, 'rl').trim() !== S.snap.relay) await rpc('set_relay', { relay: val(w, 'rl') }); await rpc('set_name', { name: val(w, 'n') }); await rpc('set_typing_enabled', { on: $('#ty', w).checked }); pref.set('theme', val(w, 'th')); pref.set('read', val(w, 'rd')); applyPrefs(); } }]).then(tick);

// Clicks inside Settings are caught here, so that the dialog above can stay as it is.
$('#modal-root').addEventListener('click', e => { if (e.target.closest('#set-gamekeys')) gameKeys(); });
const keyName = c => !c ? 'Press to set' : c.replace(/^Key/, '').replace(/^Digit/, '').replace(/^Mouse(\d)/, 'Mouse button $1').replace(/(Left|Right)$/, ' $1');
async function gameKeys() {
  const got = await rpc('game_keys_get').catch(e => { toast(e.message); return null; }); if (!got) return;
  let rows = (got.settings.rules || []).filter(r => r.process).map(r => ({ ...r }));
  const any = (got.settings.rules || []).find(r => !r.process) || { process: null, key: '', mode: 'hold' };
  let portal = !!got.settings.use_portal, programs = [];
  const row = (r, i) => `<div class="gk-row" data-i="${i}"><span class="gk-name" title="${esc(r.process || '')}">${r.process ? esc(r.process) : 'Any app'}</span><button type="button" class="btn ghost gk-key" data-i="${i}">${esc(keyName(r.key))}</button>
    <select class="gk-mode" data-i="${i}" aria-label="Mode"><option value="hold" ${r.mode !== 'toggle' ? 'selected' : ''}>Hold to mute</option><option value="toggle" ${r.mode === 'toggle' ? 'selected' : ''}>Press to toggle</option></select>
    ${r.process ? `<button type="button" class="mini gk-del" data-i="${i}" aria-label="Remove ${esc(r.process)}">×</button>` : '<span></span>'}</div>`;
  const list = i => i < 0 ? any : rows[i];
  const paint = w => {
    $('#gk-list', w).innerHTML = row(any, -1) + rows.map(row).join('');
    $('#gk-pick', w).innerHTML = programs.map(p => `<option value="${esc(p)}"></option>`).join('');
  };
  const note = esc(got.note) + (got.hold ? '' : ' Holding a key cannot be seen here.') + (got.error ? `<br>Last problem: ${esc(got.error)}` : '');
  const p = modal('Game keys', `<p class="muted small">While the game runs and you hold its key, your mic is off in the Commons call, so friends do not hear what you say to your team. Set the game's own voice key here too. These settings stay on this device.</p>
    <div id="gk-list"></div>
    <label for="gk-add">Add a game (a running program)</label><div class="gk-add"><input type="text" id="gk-add" list="gk-pick" autocomplete="off" spellcheck="false" placeholder="Start typing, or pick one"><datalist id="gk-pick"></datalist><button type="button" class="btn ghost" id="gk-addb">Add</button></div>
    <p class="muted small" id="gk-note">${note}</p>
    ${got.permission ? '<button type="button" class="btn ghost" id="gk-allow">Allow Input Monitoring</button>' : ''}
    ${got.portal_possible ? `<label class="check"><input type="checkbox" id="gk-portal" ${portal ? 'checked' : ''}><span>Use system shortcuts (the desktop asks you once). Your game may stop receiving a key bound this way.</span></label>` : ''}`,
    [{ label: 'Cancel', primary: false }, { label: 'Save', run: async w => { const on = $('#gk-portal', w); await rpc('game_keys_set', { rules: [...(any.key ? [any] : []), ...rows.filter(r => r.key)], use_portal: on ? on.checked : false }); toast('Game keys saved'); } }], { cls: 'gk-modal' });
  const w = $('.gk-modal'); paint(w);
  rpc('running_programs').then(l => { programs = l; paint(w); }).catch(() => { });
  $('#gk-addb', w).onclick = () => { const v = $('#gk-add', w).value.trim(); if (!v) return; if (!rows.some(r => r.process.toLowerCase() === v.toLowerCase())) rows.push({ process: v, key: '', mode: 'hold' }); $('#gk-add', w).value = ''; paint(w); };
  w.addEventListener('change', e => { const m = e.target.closest('.gk-mode'); if (m) list(+m.dataset.i).mode = m.value; });
  w.addEventListener('click', e => {
    const d = e.target.closest('.gk-del'); if (d) { rows.splice(+d.dataset.i, 1); paint(w); return; }
    const k = e.target.closest('.gk-key'); if (!k) return;
    if ($('#gk-allow', w) && e.target.id === 'gk-allow') return;
    k.textContent = 'Press a key…';
    // Taken in the capture phase so this key does nothing else; Escape leaves things as they were.
    const done = code => { document.removeEventListener('keydown', onK, true); document.removeEventListener('mousedown', onM, true); if (code) list(+k.dataset.i).key = code; paint(w); };
    const onK = ev => { ev.preventDefault(); ev.stopPropagation(); done(ev.code === 'Escape' ? null : ev.code); };
    const onM = ev => { if (ev.button === 3 || ev.button === 4) { ev.preventDefault(); ev.stopPropagation(); done('Mouse' + (ev.button + 1)); } else done(null); };
    setTimeout(() => { document.addEventListener('keydown', onK, true); document.addEventListener('mousedown', onM, true); }, 0);
  });
  const allow = $('#gk-allow', w); if (allow) allow.onclick = () => rpc('game_keys_allow').then(() => toast('If macOS asked, switch Commons on, then restart Commons.')).catch(e => toast(e.message));
  return p;
}

boot();
