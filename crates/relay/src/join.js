'use strict';
// The invite stays in the address bar's fragment: browsers do not send that part to the server.
const frag = location.hash.slice(1);
if (/^(join|relay)\/[A-Za-z0-9_-]+$/.test(frag)) {
  const link = 'commons://' + frag;
  document.getElementById('code').textContent = link;
  document.getElementById('have').hidden = false;
  document.getElementById('copy').onclick = async e => { try { await navigator.clipboard.writeText(link); e.target.textContent = 'Copied'; } catch { e.target.textContent = 'Select the text above and copy it'; } };
} else document.getElementById('none').hidden = false;
const label = n =>
  /^mega-linux-[^.]+\.tar\.gz$/i.test(n) ? 'Linux, native app (fastest; unpack, then run install.sh)' :
  /^mega-macos-[^.]+\.app\.tar\.gz$/i.test(n) ? 'macOS, native app (Apple silicon)' :
  /windows-browser\.zip$/i.test(n) ? 'Windows, opens in Edge, Brave or Chrome (smoother)' :
  /macos-browser\.zip$/i.test(n) ? 'macOS, opens in Brave, Chrome or Edge (smoother)' :
  /linux-browser\.tar\.gz$/i.test(n) ? 'Linux, opens in Brave or Chrome (smoother)' :
  /\.appimage$/i.test(n) ? 'Linux (AppImage)' :
  /\.(dmg|app\.zip)$/i.test(n) ? 'macOS (Apple silicon)' :
  /\.(exe|msi)$/i.test(n) ? 'Windows' :
  /\.zip$/i.test(n) ? 'Windows' :
  /\.deb$/i.test(n) ? 'Linux (.deb)' : n;
// The visitor's own system first, and its browser version before its desktop app (both are there: some prefer each).
const OS = /Windows/i.test(navigator.userAgent) ? 'win' : /Mac OS X|Macintosh/i.test(navigator.userAgent) ? 'mac' : /Linux|X11/i.test(navigator.userAgent) ? 'linux' : '';
const fileOs = n => /windows/i.test(n) ? 'win' : /macos|\.dmg$/i.test(n) ? 'mac' : /linux|\.appimage$/i.test(n) ? 'linux' : '';
// the native app first where there is one, then the browser version, then the older desktop app
const native = n => /^mega-(linux|macos)-/i.test(n);
const score = n => (fileOs(n) === OS ? 0 : 10) + (native(n) ? 0 : /browser\.(zip|tar\.gz)$/i.test(n) ? 1 : 2);
fetch('/download/').then(r => r.ok ? r.json() : []).then(list => {
  const el = document.getElementById('dl'); el.textContent = '';
  if (!list.length) { el.innerHTML = '<span class="muted">No downloads on this server yet. Ask whoever sent you here for the app.</span>'; return; }
  list.sort((a, b) => score(a.name) - score(b.name) || a.name.localeCompare(b.name));
  for (const f of list) { const a = document.createElement('a'); a.className = 'btn'; a.href = '/download/' + encodeURIComponent(f.name); a.textContent = label(f.name) + ' · ' + Math.round(f.size / 1048576) + ' MB'; el.appendChild(a); }
}).catch(() => { document.getElementById('dl').textContent = 'Could not list the downloads.'; });
