'use strict';
// Stand-alone call page, served by the desktop app on 127.0.0.1 for use in a regular browser tab when the app's own web
// view cannot do calls (see docs/CALLS-PLATFORMS.md). It gets exactly two rpc methods (call_state, call_token), for one room,
// protected by a one-time secret that is only in this tab's URL fragment (never sent to any server, not in the history sent anywhere).
const hp = new URLSearchParams(location.hash.slice(1));
const SECRET = hp.get('s'), GID = hp.get('g'), NAME = hp.get('n') || 'Call';
history.replaceState(null, '', location.pathname);   // drop the secret from the address bar
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const toast = () => { };
async function rpc(method, args = {}) {
  const r = await fetch('/rpc', { method: 'POST', headers: { 'x-call-secret': SECRET }, body: JSON.stringify({ method, args }) });
  const j = await r.json(); if (j.err !== undefined) throw new Error(j.err); return j.ok;
}
const S = { snap: { communities: [{ id: 'c', name: '', channels: [{ gid: GID, name: NAME, joined: true }] }] }, cid: 'c', gid: GID };
window.addEventListener('load', () => { window.__callUI && window.__callUI.open(); });
