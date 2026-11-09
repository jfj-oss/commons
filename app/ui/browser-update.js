'use strict';
// Updates for the browser versions (packaging/*-browser, docs/UPDATES.md). The launcher installs a signed update when
// the background part starts, and leaves a note for this page: "Updated to x.y.z", or "Update available" and the reason.
// That background part can run for days, so the page also asks the bridge whether something newer is published and offers
// an Update button here and in Settings. The bridge does the checks (same server, signature, version) and refuses during
// a call. The desktop app has its own update-ui.js and never runs this (there is a Tauri shell).
(function () {
  if (window.__TAURI__) return;
  const bar = document.getElementById('upd');
  if (!bar) return;
  const esc = s => String(s == null ? '' : s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
  const say = m => { if (typeof toast === 'function') toast(m); };
  const U = { every: 4 * 3600e3, first: 15e3, last: 0, offer: null, busy: false };
  window.__commonsBrowserUpdate = U;   // the tests read and tune this
  const T0 = Date.now();
  const unlocked = () => { const a = document.getElementById('app'); return a && !a.hidden; };
  function show(html, cls = '') { bar.className = cls; bar.innerHTML = html; bar.hidden = false; }
  function hide() { bar.hidden = true; bar.innerHTML = ''; }
  const offerBar = () => show(`<span>mega ${esc(U.offer)} is available.</span><button type="button" class="btn slim" data-u="install">Update now</button>`);
  const failedBar = why => show(`<span>The update did not install: ${esc(why)} Nothing was changed.</span><button type="button" class="btn ghost slim" data-u="install">Try again</button>`, 'need');
  const settingsButton = () => document.getElementById('set-update');

  (window.__bridgeFetch || fetch)('/update-note', { cache: 'no-store' }).then(r => r.ok ? r.json() : null).then(n => {
    if (!n || !n.kind) return;
    const v = n.version ? ' ' + esc(n.version) : '';
    if (n.kind === 'updated') show(`<span>Updated to${v}.</span>`);
    else if (n.kind === 'available') show(`<span>Update available${v ? ' (' + esc(n.version) + ')' : ''}: ${esc(n.reason || 'it could not be installed.')}</span>`, 'need');
  }).catch(() => {}).then(() => fetch('/mode', { cache: 'no-store' })).then(r => r.ok ? r.json() : null).then(m => {
    // An installed copy that was started without the per-start key (an older launcher, or your own script or service).
    // It still works; this says plainly what is missing. A test rig or a source checkout is not "packaged" and says nothing.
    if (!m || !m.packaged || m.secret || !bar.hidden) return;
    show(`<span>This mega was started by an older launcher, so other programs on this computer can talk to it. ${m.launcher_updated ? 'The launcher has just been updated: close mega and open it again from your app menu.' : 'Close mega and open it again from your app menu; if you start it with your own script or service, that needs the newer launcher (see docs/UPDATES.md).'}</span>`, 'need');
  }).catch(() => {});

  async function check() {
    U.last = Date.now();
    const r = await (await (window.__bridgeFetch || fetch)('/update-check', { cache: 'no-store' })).json();
    U.offer = r.available ? r.version : null;
    if (U.offer && !U.busy) offerBar();
    return r;
  }

  // The bridge is gone for a moment while the new one starts; the page reloads from the new one.
  function comeBack(version) {
    show(`<span>Installed ${esc(version)}. Restarting mega…</span>`);
    const end = Date.now() + 30e3;
    const look = () => fetch('/', { cache: 'no-store' }).then(() => location.reload()).catch(() => {
      if (Date.now() < end) setTimeout(look, 500);
      else { U.busy = false; show('<span>mega did not come back. Close it and open it again.</span>', 'need'); }
    });
    setTimeout(look, 1000);
  }

  async function install() {
    if (U.busy) return;
    U.busy = true;
    show('<span>Downloading the update…</span>');
    let r;
    try { r = await (await (window.__bridgeFetch || fetch)('/update-install', { method: 'POST', cache: 'no-store' })).json(); }
    catch (e) { U.busy = false; failedBar('mega stopped answering.'); return; }
    if (r.restarting) { comeBack(r.version); return; }
    U.busy = false;
    if (r.in_call) { if (U.offer) offerBar(); else hide(); say('Leave the call first, then update.'); }
    else if (r.current) { U.offer = null; hide(); say('You have the newest version.'); }
    else if (r.busy) show('<span>Already updating…</span>');
    else if (r.packaged === false) { hide(); say('This copy of mega cannot update itself.'); }
    else failedBar(r.error || 'something went wrong.');
  }

  bar.addEventListener('click', e => { if (e.target.closest('[data-u=install]')) install(); });

  // Settings: the button checks; when something newer exists it becomes the Update button.
  document.addEventListener('click', async e => {
    const b = e.target.closest('#set-update');
    if (!b || b.disabled) return;
    if (b.dataset.version) { b.disabled = true; b.textContent = 'Updating…'; install(); return; }
    b.disabled = true; b.textContent = 'Checking…';
    let r = {};
    try { r = await check(); } catch { r = { error: 'mega stopped answering.' }; }
    b.disabled = false;
    if (r.available) { b.dataset.version = r.version; b.textContent = 'Update to ' + r.version; return; }
    b.textContent = 'Check for updates';
    if (r.packaged === false) say('This copy of mega cannot update itself.');
    else if (r.error) say('Could not check: ' + r.error);
    else say(r.reason ? 'No update for this system yet.' : `You have the newest version (${r.current}).`);
  });

  setInterval(() => {
    if (U.busy || !unlocked()) return;
    if (U.last ? Date.now() - U.last > U.every : Date.now() - T0 > U.first) check().catch(() => {});
  }, 5000);
})();
