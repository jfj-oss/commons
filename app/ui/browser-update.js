'use strict';
// The browser versions' launchers check for a signed update before the page opens (packaging/*-browser,
// docs/UPDATES.md). The bridge leaves a note for this page: "Updated to x.y.z" after an install, or "Update available"
// and the reason when a newer version exists but could not be installed. Shown once, in the same bar the desktop app
// uses. The desktop app has its own update-ui.js and never runs this (there is a Tauri shell).
(function () {
  if (window.__TAURI__) return;
  const bar = document.getElementById('upd');
  if (!bar) return;
  const esc = s => String(s == null ? '' : s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
  fetch('/update-note', { cache: 'no-store' }).then(r => r.ok ? r.json() : null).then(n => {
    if (!n || !n.kind) return;
    const v = n.version ? ' ' + esc(n.version) : '';
    if (n.kind === 'updated') {
      bar.className = '';
      bar.innerHTML = `<span>Updated to${v}.</span>`;
    } else if (n.kind === 'available') {
      bar.className = 'need';
      bar.innerHTML = `<span>Update available${v ? ' (' + esc(n.version) + ')' : ''}: ${esc(n.reason || 'it could not be installed.')}</span>`;
    } else {
      return;
    }
    bar.hidden = false;
  }).catch(() => {});
})();
