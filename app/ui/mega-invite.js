'use strict';
// mega: joining and inviting with a password.
//   first(ctx)   the first screen: invite password, your name, one password for this device
//   join(link)   an existing account joins another community with a password (or a link, if that is what was pasted)
//   invite(link) the inviter's screen: choose a password, how long and how many, then what to tell the friend
// Everything that touches keys, the relay or the rules is in the core (pwinvite.rs); this file only asks and shows.
// The old invite links still work: a pasted link in the password box is handled, and every screen can fall back to them.
(() => {
  const isLink = v => /^(commons:|https?:)/i.test(v.trim());
  const dl = (rel, relay) => (relay || rel || '').replace(/\/$/, '') + '/join';
  let kept = '';   // the device password just typed, held in memory only until the recovery key has been shown once

  // ---------------------------------------------------------------- the first screen
  function first(ctx) {
    const el = ctx.el, adv = !ctx.relay;
    el.innerHTML = `<div class="boot-card mi-first">${ctx.BRAND}<h1>mega</h1><div class="tag">Got an invite password? Type it, pick a name, and you are in.</div>
      <label for="ipw">Invite password</label><input type="text" id="ipw" autocomplete="off" autocapitalize="none" spellcheck="false" autofocus placeholder="the password you were told">
      <label for="nm">Your name</label><input type="text" id="nm" autocomplete="off">
      <label for="pp">A password for this device</label><div class="mi-pw"><input type="password" id="pp" autocomplete="new-password" aria-describedby="pph"><button type="button" class="mi-eye" id="mi-eye" aria-pressed="false">show</button></div>
      <div class="muted small" id="pph" aria-live="polite">It locks mega on this computer. Nobody can reset it, so make it one you will remember.</div>
      ${adv ? `<label for="mi-relay">Server address</label><input type="text" id="mi-relay" spellcheck="false" placeholder="https://chat.example.org">` : `<details class="relay-box mi-adv"><summary>Invite from another server</summary><input type="text" id="mi-relay" aria-label="Server address" spellcheck="false" placeholder="https://chat.example.org"></details>`}
      <div class="err" id="e" role="alert"></div><div class="boot-go"><button class="btn" id="go">Join</button></div><div class="boot-links">${ctx.links('pw')}</div></div>`;
    const $e = id => el.querySelector('#' + id);
    $e('mi-eye').onclick = () => { const p = $e('pp'), on = p.type === 'password'; p.type = on ? 'text' : 'password'; $e('mi-eye').textContent = on ? 'hide' : 'show'; $e('mi-eye').setAttribute('aria-pressed', on); };
    $e('pp').oninput = () => { $e('pph').textContent = ppStrength($e('pp').value) || 'It locks mega on this computer. Nobody can reset it, so make it one you will remember.'; };
    const go = async () => {
      const b = $e('go'), err = $e('e'); err.textContent = ''; b.disabled = true;
      try {
        const pw = $e('ipw').value.trim(), name = $e('nm').value.trim(), pass = $e('pp').value, relay = $e('mi-relay').value.trim();
        if (!pw) throw new Error('Type the invite password you were given.');
        if (!name) throw new Error('Choose a name.');
        if (pass.length < 8) throw new Error('The password for this device needs at least 8 characters.');
        if (isLink(pw)) {
          // an invite link in the same box: the old way, with the same screen
          const inv = await rpc('parse_link', { link: pw });
          await rpc('create_account', { name, passphrase: pass, relay: inv.relay, invite: inv.secret || '' });
          if (inv.kind === 'community') await rpc('join', { link: inv.link }).catch(x => toast(x.message));
          await rpc('recovery_saved', { unsaved: true });
        } else {
          const r = await rpc('create_account_password', { password: pw, name, passphrase: pass, relay });
          if (r.join_error) toast(r.join_error);
        }
        kept = pass;
        ctx.start();
      } catch (x) { err.textContent = x.message; b.disabled = false; }
    };
    $e('go').onclick = go;
    el.querySelectorAll('input').forEach(i => i.onkeydown = e => { if (e.key === 'Enter') go(); });
    ctx.wire();
  }

  // ---------------------------------------------------------------- joining another community
  function join(byLink) {
    const p = modal('Join a community', `<p class="muted small">Type the invite password you were given. An admin has to be online (now or later) to let you in.</p>
      <input type="text" id="mi-jpw" autocomplete="off" autocapitalize="none" spellcheck="false" aria-label="Invite password" placeholder="the password you were told"><button type="button" class="linkish mi-alt" id="mi-link">I have an invite link instead</button>`,
      [{ label: 'Cancel', primary: false }, { label: 'Join', run: async w => {
        const v = $('#mi-jpw', w).value.trim();
        if (!v) throw new Error('Type the invite password you were given.');
        await (isLink(v) ? rpc('join', { link: v }) : rpc('join_password', { password: v }));
        toast('Request sent');
      } }], { cls: 'mi-modal' });
    const w = $('.mi-modal');
    $('#mi-link', w).onclick = () => { $('.acts button', w).click(); setTimeout(byLink, 170); };
    return p.then(() => tick());
  }

  // ---------------------------------------------------------------- the inviter's screen
  function invite(byLink) {
    const c = currentCommunity(); window.megaInvite.byLink = byLink;
    const open = (c.invites || []).filter(i => i.mine && i.expires > Date.now() / 1000);
    const list = open.length ? `<details class="mi-open"><summary>Open invites <span class="count">${open.length}</span></summary>${open.map(i => `<div class="inv-row"><span>${i.password ? 'Password' : 'Link'} · ${i.uses_left == null ? 'unlimited' : `${i.uses_left} use${i.uses_left === 1 ? '' : 's'} left`} · ${esc(fmtLeft(i.expires))}</span><button type="button" class="linkish" data-rev="${esc(i.id)}">Revoke</button></div>`).join('')}</details>` : '';
    let made = null, told = '';
    const p = modal('Invite someone', `<div id="mi-form">
        <label for="mi-pw">Choose a password to tell your friend</label>
        <div class="mi-pw"><input type="text" id="mi-pw" autocomplete="off" autocapitalize="none" spellcheck="false" aria-describedby="mi-hint"><button type="button" class="mi-eye" id="mi-new">another</button></div>
        <div class="small mi-hint" id="mi-hint" aria-live="polite"></div>
        <label>Valid for</label>${seg('mi-ttl', [['86400', '1 day'], ['259200', '3 days'], ['604800', '7 days'], ['2592000', '30 days']], '604800')}
        <label for="mi-uses">How many people can use it</label><input type="number" id="mi-uses" min="1" max="200" value="1" inputmode="numeric">
        <p class="muted small">Anyone who guesses the password can join, so it expires and has a limit. You can revoke it at any time.</p>
        ${/^https:\/\//.test(S.snap.relay || '') ? `<p class="muted small">Download page for friends: <span class="mi-dl">${esc(new URL(S.snap.relay).origin + '/join')}</span> <button type="button" class="linkish" id="mi-copy-dl">copy</button></p>` : ''}</div><div id="mi-out"></div>${list}`,
      [{ label: 'Close', primary: false }, { label: 'Create invite', run: async w => {
        if (made) { await copy(told, 'Copied'); return false; }
        const pw = $('#mi-pw', w).value.trim(), ttl = +val(w, 'mi-ttl'), uses = Math.max(1, Math.min(200, +$('#mi-uses', w).value || 1));
        const r = await rpc('create_invite', { cid: S.cid, password: pw, ttl, uses });
        made = r;
        const until = new Date((Date.now() / 1000 + ttl) * 1000).toLocaleString([], { weekday: 'short', day: 'numeric', month: 'short', hour: 'numeric', minute: '2-digit' });
        const who = uses === 1 ? 'one person' : `up to ${uses} people`;
        told = `Join "${c.name}" on mega.\n1. Get the app: ${r.download}\n2. Open it and type this invite password: ${pw}\n3. Choose your name and a password for your computer.\nThe password works for ${who} until ${until}.`;
        $('#mi-form', w).hidden = true;
        const noSecret = r.signup_needs_secret ? `<div class="state-line">This device does not know the server's sign-up secret, so this only works for people who already have an account on ${esc(S.snap.relay)}.</div>` : '';
        $('#mi-out', w).innerHTML = `<div class="mi-tell"><div class="mi-t">Tell your friend</div>
          <ol><li>Get mega<span class="mi-dl">${esc(r.download)}</span></li><li>Open it and type this invite password<span class="mi-code" id="mi-code">${esc(pw)}</span></li><li>Choose a name and a password for their computer</li></ol>
          <div class="muted small">Works for ${esc(who)} until ${esc(until)}. mega does not keep the password: write it down now.</div></div>${noSecret}<button type="button" class="linkish mi-alt" id="mi-copy-pw">Copy the password</button>`;
        $('.acts button:last-child', w).textContent = 'Copy message';
        $('#mi-copy-pw', w).onclick = () => copy(pw, 'Password copied');
        tick();
        return false;
      } }], { cls: 'mi-modal invite-modal' });
    const w = $('.mi-modal');
    const dl = $('#mi-copy-dl', w); if (dl) dl.onclick = () => copy(new URL(S.snap.relay).origin + '/join', 'Download link copied');
    const create = $('.acts button:last-child', w), pw = $('#mi-pw', w), hint = $('#mi-hint', w);
    let seq = 0;
    const check = async () => {
      const my = ++seq, v = pw.value.trim();
      if (!v) { hint.textContent = ''; hint.className = 'small mi-hint'; create.disabled = true; return; }
      let r; try { r = await rpc('password_strength', { password: v }); } catch { return; }
      if (my !== seq) return;
      create.disabled = !r.ok;
      hint.className = 'small mi-hint ' + (!r.ok ? 'bad' : r.warn ? 'warn' : 'good');
      hint.textContent = !r.ok ? 'Too easy to guess: the server could work it out and read the invite. Use the suggestion, or five or more unrelated words.' : r.warn ? 'Fine for a few days. A longer password is safer.' : 'Good.';
    };
    const suggest = async () => { pw.value = (await rpc('suggest_password', {})).password; check(); };
    $('#mi-new', w).onclick = suggest; pw.oninput = check; suggest();
    w.addEventListener('click', async e => { const b = e.target.closest('[data-rev]'); if (!b) return; try { await rpc('revoke_invite', { cid: c.id, id: b.dataset.rev }); b.closest('.inv-row').remove(); toast('Invite revoked'); tick(); } catch (er) { toast(er.message); } });
    return p;
  }

  // ---------------------------------------------------------------- the recovery key, afterwards
  // A new account made with a password is let in at once. Until its key has been saved, a card stays in the corner; it has no
  // close button, and goes only when the person says they saved it.
  const reminder = document.createElement('div');
  reminder.id = 'mi-rec'; reminder.hidden = true;
  reminder.innerHTML = `<div class="mi-rt"><b>Save your recovery key</b><span>Without it, a lost computer means a lost account.</span></div><button type="button" id="mi-rec-go">Show my key</button>`;
  document.body.appendChild(reminder);
  const showKey = async () => {
    let key = '';
    const reveal = async (w, pass) => { key = (await rpc('recovery_key', { passphrase: pass })).key; $('#mi-kbody', w).innerHTML = `<p>If this computer is lost or broken, this key gets you back into your account on a new one, and your communities let you back in. Messages from before do <b>not</b> come back.</p><div class="code key-code">${esc(key)}</div><button type="button" class="linkish copy-line" id="mi-kcopy">${icon('copy')}Copy</button><p class="muted small">Write it down, or keep it in a password manager, somewhere that is not this computer. Anyone who has it can act as you. You can see it again under Devices.</p>`; $('#mi-kcopy', w).onclick = () => copy(key, 'Key copied'); };
    modal('Save your recovery key', `<div id="mi-kbody"><p class="muted small">Type the password for this device to show the key.</p><input type="password" id="mi-kpw" autocomplete="current-password" aria-label="Password for this device"></div>`,
      [{ label: 'Later', primary: false }, { label: 'I saved it', run: async w => {
        if (!key) { if (!$('#mi-kpw', w)) return false; await reveal(w, $('#mi-kpw', w).value); return false; }
        await rpc('recovery_saved', { unsaved: false }); kept = ''; reminder.hidden = true;
      } }], { sticky: true, cls: 'mi-modal' });
    const w = $('.mi-modal'); if (kept) { try { await reveal(w, kept); } catch { } }
    const act = $('.acts button:last-child', w); const upd = () => { act.textContent = key ? 'I saved it' : 'Show key'; }; upd();
    new MutationObserver(upd).observe($('#mi-kbody', w), { childList: true });
  };
  reminder.querySelector('#mi-rec-go').onclick = showKey;
  const poll = () => { const on = typeof S !== 'undefined' && !!(S.snap && S.snap.me && S.snap.me.recovery_unsaved); if (reminder.hidden === on) reminder.hidden = !on; };
  setInterval(poll, 1500);

  window.megaInvite = { first, join, invite };
})();
