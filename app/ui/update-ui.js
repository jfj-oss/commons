'use strict';
// Updates (docs/UPDATES.md). Asks the app whether this account's relay offers a newer signed version, on start and every
// few hours, and offers it: "Install now" or "Later". Never silently, and never during a call: an offer waits until the
// call has ended, and so does the restart. The checks themselves (same server, signature, version) happen in Rust.
// Only in the packaged app: the development bridge has no updater. Depends on app.js (modal, esc, toast).
(function () {
  if (!(window.__TAURI__ && window.__TAURI__.core)) return;
  const inv = (c, a) => window.__TAURI__.core.invoke(c, a || {}).catch(e => { throw new Error(typeof e === 'string' ? e : (e && e.message) || String(e)); });
  const T0 = Date.now();
  const U = {
    every: 4 * 3600e3,      // how often to look
    first: 15e3,            // the first look, after start (and after unlocking)
    snooze: 4 * 3600e3,     // "Later"
    last: 0, later: 0, offer: null, open: false, ready: null, error: '', checks: 0, log: [],
  };
  window.__commonsUpdate = U;   // the tests read and tune this
  const note = m => { U.log.push(Math.round((Date.now() - T0) / 1000) + 's ' + m); if (U.log.length > 50) U.log.shift(); };

  async function inCall() {
    const c = window.__commonsCall;
    if (c && c.phase && c.phase !== 'idle' && c.phase !== 'error') return true;
    try { return !!(await inv('update_status')).in_call; } catch { return false; }
  }
  function bar(html, cls = '') {
    const b = document.getElementById('upd'); if (!b) return;
    b.hidden = !html; b.className = cls; b.innerHTML = html || '';
  }
  const unlocked = () => { const a = document.getElementById('app'); return a && !a.hidden; };

  async function check() {
    U.last = Date.now(); U.checks++;
    try {
      const r = await inv('update_check');
      U.error = '';
      note('check: ' + JSON.stringify(r));
      if (r.available) {
        if (!U.offer || U.offer.version !== r.version) U.later = 0;
        U.offer = r;
      } else U.offer = null;
      if (r.required && r.minimum) bar(`<span>This server asks everyone to use Megatron ${esc(r.minimum)} or newer, and this is ${esc(r.current)}. ${r.available ? 'Please install the update.' : 'No update for this system is published yet: ask whoever runs the server.'}</span>${r.available ? '<button type="button" class="linkish" data-u="offer">Update</button>' : ''}`, 'need');
      else if (!U.ready) bar('');
    } catch (e) { U.error = e.message; note('check failed: ' + e.message); }
  }

  async function offer() {
    const r = U.offer; U.open = true;
    const notes = r.notes ? `<div class="upd-notes muted small">${esc(r.notes)}</div>` : '';
    const need = r.required ? `<p>This server asks everyone to use version ${esc(r.minimum)} or newer.</p>` : '';
    let poll = null;
    const res = await modal('Update available', `<p>Version <b>${esc(r.version)}</b> of Megatron is available (you have ${esc(r.current)}). Install now?</p>${need}${notes}
      <p class="muted small upd-what">Megatron checks the update's signature before installing it, then restarts. You will need your passphrase again.</p>`,
    [{ label: 'Later', primary: false }, {
      label: 'Install now', run: async w => {
        if (await inCall()) { U.later = 0; return 'call'; }
        const what = w.querySelector('.upd-what');
        what.textContent = 'Downloading…';
        poll = setInterval(async () => { try { const s = await inv('update_status'); if (s.total) what.textContent = `Downloading… ${Math.floor(100 * s.got / s.total)}%`; } catch { } }, 500);
        try {
          const x = await inv('update_install');
          note('install: ' + JSON.stringify(x));
          if (x.restarting) { what.textContent = 'Installed. Restarting…'; return false; }
          if (x.ready) { U.ready = { version: r.version }; return 'ready'; }
          if (x.waiting_for_call) { U.later = 0; return 'call'; }
          if (x.busy) { what.textContent = 'Already downloading…'; return false; }
          return 'done';
        } catch (e) { what.textContent = 'Nothing was changed.'; U.error = e.message; note('install failed: ' + e.message); throw e; }
        finally { clearInterval(poll); }
      },
    }], { sticky: true });
    clearInterval(poll);
    U.open = false;
    if (res === 0 || res === -1) { U.later = Date.now() + (r.required ? Math.min(U.snooze, 3600e3) : U.snooze); note('later'); }
    else if (res === 'call') { toast('A call is running. The update waits until it ends.'); }
    else if (res === 'ready') bar('<span>The update is downloaded. Megatron will ask to restart once your call has ended.</span>', '');
  }

  async function askRestart() {
    U.open = true;
    const res = await modal('Update ready', `<p>The update to version <b>${esc(U.ready.version)}</b> is downloaded and checked. Restart Megatron now to finish it? You will need your passphrase again.</p>`,
      [{ label: 'Later', primary: false }, { label: 'Restart now', run: async () => {
        if (await inCall()) return 'call';
        const x = await inv('update_apply'); note('apply: ' + JSON.stringify(x));
        return x.restarting ? false : 'call';
      } }], { sticky: true });
    U.open = false;
    if (res === 0 || res === -1) { U.ready.later = Date.now() + U.snooze; note('restart later'); }
  }

  async function step() {
    if (U.open || !unlocked()) return;
    const now = Date.now();
    if (!U.last ? now - T0 > U.first : now - U.last > U.every) await check();
    if (U.open || !unlocked()) return;
    if (U.ready) { if (now > (U.ready.later || 0) && !(await inCall())) await askRestart(); return; }
    if (U.offer && now > U.later && !(await inCall())) await offer();
  }
  U.step = step; U.check = check;
  document.getElementById('upd').addEventListener('click', e => { if (e.target.closest('[data-u=offer]') && U.offer && !U.open) { U.later = 0; offer(); } });
  setInterval(() => { step().catch(e => note('step: ' + e.message)); }, 5000);
})();
