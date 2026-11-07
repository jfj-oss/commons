'use strict';
// The invite stays in the address bar's fragment: browsers do not send that part to the server.
const frag = location.hash.slice(1);
if (/^(join|relay)\/[A-Za-z0-9_-]+$/.test(frag)) {
  const link = 'commons://' + frag;
  document.getElementById('code').textContent = link;
  document.getElementById('have').hidden = false;
  document.getElementById('copy').onclick = async e => { try { await navigator.clipboard.writeText(link); e.target.textContent = 'Copied'; } catch { e.target.textContent = 'Select the text above and copy it'; } };
} else document.getElementById('none').hidden = false;
const label = n => /\.appimage$/i.test(n) ? 'Linux (AppImage)' : /\.(dmg|app\.zip)$/i.test(n) ? 'macOS (Apple silicon)' : /\.(exe|msi|zip)$/i.test(n) ? 'Windows' : /\.deb$/i.test(n) ? 'Linux (.deb)' : n;
fetch('/download/').then(r => r.ok ? r.json() : []).then(list => {
  const el = document.getElementById('dl'); el.textContent = '';
  if (!list.length) { el.innerHTML = '<span class="muted">No downloads on this server yet. Ask whoever sent you here for the app.</span>'; return; }
  for (const f of list) { const a = document.createElement('a'); a.className = 'btn'; a.href = '/download/' + encodeURIComponent(f.name); a.textContent = label(f.name) + ' · ' + Math.round(f.size / 1048576) + ' MB'; el.appendChild(a); }
}).catch(() => { document.getElementById('dl').textContent = 'Could not list the downloads.'; });
