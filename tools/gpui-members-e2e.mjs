// People, admin and invites in the native app (app/gpui/src/members.rs), against the web client. Alice is the native WINDOW
// (`mega-gpui`) inside a private headless KWin, driven over its test port (app/gpui/src/uitest.rs); Bob and Carol are the real app/ui in
// headless Chrome for Testing; Dave, Erin and Frank are bare accounts on bridges. Nobody joins a real room: one local relay and
// LiveKit, throwaway accounts, silent (the window opens no audio device, Chrome has a fake microphone and no output).
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin mega-gpui --bin commons-relay --bin dev_bridge
//   COMMONS_TEST_PORT_OFFSET=1700 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-members-e2e.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, startChrome, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';
import { openApps } from './lib/world.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-members-e2e');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const shots = path.join(out, 'shots'); fs.mkdirSync(shots, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const PASS = 'correct horse battery';
const REL = `http://127.0.0.1:${RELAY_PORT}`;
const retry = async (f, ms = 30000) => { const t = Date.now(); for (;;) { try { return await f(); } catch (e) { if (Date.now() - t > ms) throw e; await sleep(1000); } } };
const J = x => JSON.stringify(x);
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? '  [' + (typeof detail === 'string' ? detail : J(detail)).slice(0, 300) + ']' : ''}`); };
async function until(fn, ms, what, step = 250) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(step); } }
const soft = p => p.catch(e => ({ timeout: String(e.message || e) }));
const timedOut = v => v && typeof v === 'object' && typeof v.timeout === 'string';

class Win {
  constructor(port) { this.port = port; this.n = 0; this.waiting = new Map(); this.buf = ''; }
  async connect() {
    this.sock = net.connect(this.port, '127.0.0.1');
    await new Promise((res, rej) => { this.sock.once('connect', res); this.sock.once('error', rej); });
    this.sock.on('data', d => { this.buf += d; let i; while ((i = this.buf.indexOf('\n')) >= 0) { const l = this.buf.slice(0, i); this.buf = this.buf.slice(i + 1); const j = JSON.parse(l); const w = this.waiting.get(j.id); if (w) { this.waiting.delete(j.id); w(j); } } });
  }
  cmd(cmd, args = {}, ms = 20000) {
    const id = ++this.n;
    return new Promise((res, rej) => { const t = setTimeout(() => { this.waiting.delete(id); rej(new Error('no answer to ' + cmd)); }, ms); this.waiting.set(id, j => { clearTimeout(t); res(j); }); this.sock.write(J({ id, cmd, ...args }) + '\n'); });
  }
  do(what, args = {}) { return this.cmd('do', { what, ...args }); }
  core(method, args = {}) { return this.cmd('core', { method, args }).then(r => { if (r.error) throw new Error(r.error); return r.ok; }); }
  members() { return this.cmd('members'); }
  panel(which, name, who = '') { return this.cmd('panel', { which, name, who }); }
}

const priv = path.join(out, 'priv'); const xdg = { XDG_RUNTIME_DIR: path.join(priv, 'run'), XDG_CONFIG_HOME: path.join(priv, 'config'), XDG_STATE_HOME: path.join(priv, 'state'), XDG_CACHE_HOME: path.join(priv, 'cache'), XDG_DATA_HOME: path.join(priv, 'data') };
for (const d of Object.values(xdg)) fs.mkdirSync(d, { recursive: true, mode: 0o700 });
const cleanEnv = () => { const e = { ...process.env, ...xdg }; for (const k of ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'XDG_SESSION_ID', 'PIPEWIRE_RUNTIME_DIR', 'PULSE_SERVER', 'PULSE_RUNTIME_PATH']) delete e[k]; return e; };
const busConf = (addrFile) => { const c = path.join(priv, path.basename(addrFile) + '.conf'); fs.writeFileSync(c, `<busconfig><type>session</type><listen>unix:path=${path.join(priv, path.basename(addrFile) + '.sock')}</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>`); return c; };
function sh(cmd, args, env, log) { const p = spawn(cmd, args, { env, stdio: ['ignore', fs.openSync(log, 'a'), fs.openSync(log, 'a')], detached: true }); procs.push(p); return p; }

const res = { checks: results };
try {
  await startServers(out, {});
  // ---- accounts. Alice's is made by a bridge that then stops so the window can open the same folder.
  const people = {};
  let k = 0;
  const makeAccount = async (name, keepBridge) => {
    const port = P(27200 + k++); const b = startBridge(out, name, port); await sleep(1300);
    await rpc(port, 'create_account', { name: name[0].toUpperCase() + name.slice(1), passphrase: PASS, relay: REL });
    const account = (await rpc(port, 'snapshot')).me.account;
    people[name] = { name, port, account, bridge: b };
    return people[name];
  };
  const stopBridge = async p => { try { process.kill(-p.bridge.pid, 'SIGTERM'); } catch { } await sleep(1500); };
  const alice = await makeAccount('alice'); await stopBridge(alice);
  const bob = await makeAccount('bob'), carol = await makeAccount('carol'), dave = await makeAccount('dave'), frank = await makeAccount('frank');
  const erin = await makeAccount('erin'); await stopBridge(erin);

  // ---- a private KWin with Alice's window
  const kwBusFile = path.join(out, 'kwin-bus');
  const kw = sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket wayland-mem --no-lockscreen --no-global-shortcuts --width 1280 --height 860`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(out, 'kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, 'wayland-mem')), 20000, 'kwin socket');
  const shot = async name => { const f = path.join(shots, name + '.png'); const r = spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); return r.status === 0 ? f : null; };
  const windowEnv = (name, wport, soundless = true) => ({ ...cleanEnv(), WAYLAND_DISPLAY: 'wayland-mem', DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim(), COMMONS_DATA: path.join(out, 'data', name), COMMONS_RELAY: REL, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_UI_TEST: String(wport), RUST_LOG: 'info' });
  const wport = P(27310);
  const nat = sh(BIN, [], { ...windowEnv('alice', wport), RUST_BACKTRACE: '1' }, path.join(out, 'alice-window.log'));
  const alive = () => { try { process.kill(nat.pid, 0); return true; } catch { return false; } };
  const W = new Win(wport);
  await until(async () => { try { await W.connect(); return true; } catch { return false; } }, 30000, 'window test port');
  await until(async () => (await W.cmd('core', { method: 'snapshot' })).ok, 30000, 'window core unlocked');
  await sleep(2500);
  const snapW = () => W.core('snapshot');
  const commW = async name => (await snapW()).communities.find(c => c.name === name);
  check('0 the native window is up as Alice, in a private KWin', true, { alice: alice.account.slice(0, 8) });

  // ---------------------------------------------------------------- A. a new community, an invite password, Bob and Carol join
  await W.panel('new', '');
  await sleep(500); await shot('01-new-community');
  await W.do('new_community', { new: 'Members club' });
  const club = await soft(until(async () => { const c = await commW('Members club'); return c && c.role === 'owner' && c.channels.some(x => x.joined) ? c : null; }, 30000, 'community exists'));
  check('A1 "new community" in the window makes one and Alice owns it', !timedOut(club), club.role);
  const cid = club.id, textGid = club.channels[0].gid;
  await W.cmd('go', { name: 'Members club' }); await sleep(800); await shot('02-community-cards');

  const sug = (await W.core('suggest_password')).password;
  await W.cmd('panel', { which: 'invite', name: 'Members club' }); await sleep(900); await shot('03-invite-form');
  const mk1 = await W.do('invite', { name: 'Members club', mode: 'password', ttl: 86400, uses: 2, password: sug });
  const made1 = await soft(until(async () => { const m = await W.members(); return m.made ? m : null; }, 20000, 'invite made', 300));
  check('A2 an invite password is made and the window tells what to say', !timedOut(made1) && made1.made.copy === sug && /invite password/.test(made1.made.tell), made1.made || made1);
  await sleep(500); await shot('04-invite-made');
  check('A3 the open invite is in the core as Alice\'s own, with two uses', (await commW('Members club')).invites.some(i => i.mine && i.uses_left === 2 && i.password));
  // Bob and Carol use the password: the window is the online admin that lets them in

  await rpc(bob.port, 'join_password', { password: sug });
  await sleep(4000);
  await rpc(carol.port, 'join_password', { password: sug });
  const joined = async p => until(async () => { const s = await rpc(p.port, 'snapshot'); const c = s.communities.find(c => c.id === cid); return c && c.channels.some(x => x.joined && x.status === 'ok') ? c : null; }, 90000, p.name + ' is in', 500);
  const jb = await soft(joined(bob)), jc = await soft(joined(carol));
  check('A4 Bob and Carol join with the password and are let in by the window', !timedOut(jb) && !timedOut(jc), { bob: jb.role || jb.timeout, carol: jc.role || jc.timeout });
  await W.panel('people', 'Members club');
  const ppl = await soft(until(async () => { const m = await W.members(); return m.members.length >= 3 ? m : null; }, 20000, 'members listed', 400));
  check('A5 the people panel lists Alice (owner, you), Bob and Carol', !timedOut(ppl) && ['Alice', 'Bob', 'Carol'].every(n => ppl.members.some(m => m.name === n)) && ppl.members.find(m => m.name === 'Alice').role === 'owner', ppl.members);
  await sleep(600); await shot('05-people');
  await W.panel('person', 'Members club', 'Bob'); await sleep(700); await shot('06-person-card');

  // ---------------------------------------------------------------- B. a link invite, Dave
  await W.do('invite', { name: 'Members club', mode: 'link', ttl: 86400, uses: 1 });
  const made2 = await soft(until(async () => { const m = await W.members(); return m.made && /^Invite link/.test(m.made.tell) ? m : null; }, 20000, 'link made', 300));
  check('B1 a link invite is made (one use)', !timedOut(made2) && /^(https?|commons):/.test(made2.made.copy), made2.made && made2.made.copy.slice(0, 40));
  await sleep(500); await shot('07-invite-link-made');
  await rpc(dave.port, 'join', { link: made2.made.copy });
  const jd = await soft(joined(dave));
  check('B2 Dave joins from the link and appears in the window\'s member list', !timedOut(jd) && await soft(until(async () => (await W.members()).members.some(m => m.name === 'Dave'), 20000, 'dave listed', 400)) === true, jd.timeout || '');

  // ---------------------------------------------------------------- C. roles, seen on the web client
  const ws = { people: { bob, carol } };
  const chrome = await startChrome(out, 'all', P(29400), 500);
  for (const p of [bob, carol]) { p.chrome = chrome; p.tone = p === bob ? 600 : 700; }
  await openApps(ws, ['bob', 'carol']);
  const web = (p, expr) => p.page.ev(`(async()=>{ return ${expr}; })()`);
  const roleOnWeb = p => web(p, `(S.snap.communities.find(c => c.id === ${J(cid)}) || {}).role`);
  await W.panel('people', 'Members club');
  await W.do('role', { name: 'Members club', who: 'Bob', to: 'admin' });
  const rb = await soft(until(async () => (await roleOnWeb(bob)) === 'admin', 30000, 'bob admin on web', 400));
  check('C1 making Bob an admin in the window shows on his web client', rb === true, await roleOnWeb(bob));
  check('C2 the window\'s list shows the new role', await soft(until(async () => (await W.members()).members.find(m => m.name === 'Bob')?.role === 'admin', 15000, 'list role', 300)) === true);
  await sleep(500); await shot('08-people-roles');
  await W.do('role', { name: 'Members club', who: 'Bob', to: 'member' });
  check('C3 and back to a regular member', await soft(until(async () => (await roleOnWeb(bob)) === 'member', 30000, 'bob member on web', 400)) === true);
  // an admin may not do what only the owner may
  await W.do('role', { name: 'Members club', who: 'Bob', to: 'admin' }); await until(async () => (await roleOnWeb(bob)) === 'admin', 30000, 'bob admin again', 400);
  const bobTriesRole = await soft(rpc(bob.port, 'set_role', { cid, account: carol.account, role: 'admin' }));
  await sleep(1500);
  check('C4 an admin cannot change roles (the core refuses; the window offers it only to the owner)', (await roleOnWeb(carol)) === 'member', J(bobTriesRole).slice(0, 120));
  await W.do('role', { name: 'Members club', who: 'Bob', to: 'member' });

  // ---------------------------------------------------------------- D. settings: name, rooms, colours, wall
  await W.panel('manage', 'Members club'); await sleep(900); await shot('09-manage');
  await W.do('rename', { name: 'Members club', new: 'Members hall' });
  check('D1 renaming the community shows on the web client', await soft(until(async () => (await web(bob, `S.snap.communities.find(c => c.id === ${J(cid)}).name`)) === 'Members hall', 30000, 'rename seen', 400)) === true);
  await W.do('room_add', { name: 'Members hall', new: 'porch', voice: true });
  await W.do('room_add', { name: 'Members hall', new: 'terrace' });
  const rooms = await soft(until(async () => { const c = await commW('Members hall'); return c && c.channels.some(x => x.name === 'porch' && x.kind === 'voice') && c.channels.some(x => x.name === 'terrace') ? c : null; }, 30000, 'rooms added', 400));
  check('D2 adding a voice room and a text room', !timedOut(rooms));
  const porch = rooms.channels.find(x => x.name === 'porch').gid, terr = rooms.channels.find(x => x.name === 'terrace').gid;
  await W.do('room_rename', { name: 'Members hall', room: 'terrace', new: 'garden' });
  check('D3 renaming a room shows on the web client', await soft(until(async () => (await web(bob, `S.snap.communities.find(c => c.id === ${J(cid)}).channels.some(x => x.name === 'garden')`)) === true, 30000, 'room rename seen', 400)) === true);
  await W.do('colours', { name: 'Members hall', room: 'garden', preset: 0 });
  const col = await soft(until(async () => { const c = await commW('Members hall'); return c.style && c.style.rooms && c.style.rooms[terr] ? c.style.rooms[terr] : null; }, 20000, 'colours set', 400));
  check('D4 the owner\'s colour preset is set for the room (violet and orange)', !timedOut(col) && col.main === '#6a35ff' && col.hi === '#ff8c1a', col);
  check('D5 the web client sees the same colours', await soft(until(async () => (await web(bob, `((S.snap.communities.find(c => c.id === ${J(cid)}).style || {}).rooms || {})[${J(terr)}]?.main`)) === '#6a35ff', 30000, 'web colours', 400)) === true);
  await W.do('colours', { name: 'Members hall', preset: 1 });
  check('D6 the community\'s own card takes the second preset', await soft(until(async () => { const c = await commW('Members hall'); return c.style && c.style.community && c.style.community.main === '#1b33ff'; }, 20000, 'community colours', 400)) === true);
  await W.do('colours', { name: 'Members hall', room: 'garden', preset: -1 });
  check('D7 "default" puts a room\'s colours back', await soft(until(async () => { const c = await commW('Members hall'); return !(c.style && c.style.rooms && c.style.rooms[terr]); }, 20000, 'colours cleared', 400)) === true);
  await W.do('wall_lock', { name: 'Members hall', room: 'garden', on: true });
  check('D8 locking a room\'s wall: the core lists it locked, on Bob\'s side too', await soft(until(async () => { const c = (await rpc(bob.port, 'snapshot')).communities.find(c => c.id === cid); return c.style && (c.style.locked || []).includes(terr); }, 30000, 'wall locked', 400)) === true);
  await W.do('wall_lock', { name: 'Members hall', room: 'garden', on: false });
  check('D9 unlocking it', await soft(until(async () => { const c = await commW('Members hall'); return !(c.style.locked || []).includes(terr); }, 20000, 'wall unlocked', 400)) === true);
  await W.do('wall_clear', { name: 'Members hall', room: 'garden' }); await sleep(800);
  check('D10 clearing the wall is accepted by the core', !(await W.members()).say || !(await W.members()).say.bad, (await W.members()).say);
  await W.do('room_delete', { name: 'Members hall', room: 'garden' });
  check('D11 deleting a room', await soft(until(async () => !(await commW('Members hall')).channels.some(x => x.name === 'garden'), 30000, 'room deleted', 400)) === true);
  await W.panel('manage', 'Members hall'); await sleep(900); await shot('10-manage-after');
  await W.do('verify', { name: 'Members hall', who: 'Bob' });
  const saf = await soft(until(async () => { const m = await W.members(); return /Safety/.test(m.panel) ? m : null; }, 15000, 'safety panel', 300));
  check('D12 a person\'s safety number opens', !timedOut(saf), saf.panel && saf.panel.slice(0, 60));
  await sleep(600); await shot('11-safety-number');

  // ---------------------------------------------------------------- E. kick: Carol is in a call, loses access
  const callRoom = porch;
  const joinWeb = async (p, gid, name) => { await web(p, `(window.__callUI.joinRoom(${J(gid)}, ${J(name)}), true)`); await p.page.waitFor(`window.__commonsCall.phase === 'connected' && window.__commonsCall.gid === ${J(gid)}`, 30000, p.name + ' connected'); };
  const inRoom = (p, gid, acct) => web(p, `(callPeople(${J(gid)}) || []).some(x => x.account === ${J(acct)})`);
  await until(async () => (await rpc(carol.port, 'snapshot')).communities.find(c => c.id === cid).channels.some(x => x.gid === callRoom && x.joined), 60000, 'carol has the voice room', 500);
  await until(async () => (await rpc(bob.port, 'snapshot')).communities.find(c => c.id === cid).channels.some(x => x.gid === callRoom && x.joined), 60000, 'bob has the voice room', 500);
  await joinWeb(carol, callRoom, 'porch'); await joinWeb(bob, callRoom, 'porch');
  check('E0 Carol and Bob are in the porch call', await soft(until(() => inRoom(bob, callRoom, carol.account), 20000, 'bob sees carol in call')) === true);
  await W.panel('person', 'Members hall', 'Carol'); await sleep(600);
  await W.do('kick', { name: 'Members hall', who: 'Carol' });
  const carolGone = await soft(until(async () => { const c = (await rpc(carol.port, 'snapshot')).communities.find(c => c.id === cid); return !c || c.removed ? true : null; }, 90000, 'carol removed', 600));
  check('E1 removing Carol in the window takes the community away from her', carolGone === true);
  const carolCall = await soft(until(async () => (await web(carol, `window.__commonsCall.phase`)) !== 'connected', 60000, 'carol left the call', 600));
  const stillThere = await web(bob, `(callPeople(${J(callRoom)}) || []).some(x => x.account === ${J(carol.account)})`);
  check('E2 Carol is out of the call she was in (her client left it) and Bob no longer sees her', carolCall === true && stillThere === false, { carolPhase: await web(carol, `window.__commonsCall.phase`), bobSees: stillThere });
  const afterKick = await soft(rpc(carol.port, 'messages', { gid: textGid, limit: 5 }));
  const hall = await commW('Members hall');
  await W.panel('people', 'Members hall');
  const gone3 = await soft(until(async () => !(await W.members()).members.some(m => m.name === 'Carol'), 20000, 'carol not listed', 400));
  check('E3 the window lists Carol under banned and no longer among the members', hall.banned.some(b => b.account === carol.account) && gone3 === true, hall.banned);
  await W.panel('manage', 'Members hall'); await sleep(900); await shot('12-manage-banned');
  // she cannot come back with an old invite: a fresh invite password, she tries
  const sug2 = (await W.core('suggest_password')).password;
  await W.do('invite', { name: 'Members hall', mode: 'password', ttl: 3600, uses: 3, password: sug2 });
  const made3 = await soft(until(async () => { const m = await W.members(); return m.made && m.made.copy === sug2 ? m : null; }, 20000, 'invite 3', 300));
  const carolBack = await soft(rpc(carol.port, 'join_password', { password: sug2 }));
  await sleep(6000);
  const cc = (await rpc(carol.port, 'snapshot')).communities.find(c => c.id === cid);
  check('E4 a banned Carol does not get back in with a new invite', !cc || cc.removed || !cc.channels.some(x => x.joined && x.status === 'ok'), { reply: carolBack, state: cc && { removed: cc.removed } });
  await W.do('unban', { name: 'Members hall', who: 'Carol' });
  check('E5 unbanning removes her from the banned list', await soft(until(async () => (await commW('Members hall')).banned.length === 0, 20000, 'unbanned', 400)) === true);

  // ---------------------------------------------------------------- F. revoke
  const sug3 = (await W.core('suggest_password')).password;
  await W.do('invite', { name: 'Members hall', mode: 'password', ttl: 3600, uses: 1, password: sug3 });
  await until(async () => { const m = await W.members(); return m.made && m.made.copy === sug3 ? m : null; }, 20000, 'invite 4', 300);
  const inv4 = (await commW('Members hall')).invites.filter(i => i.mine).map(i => i.id);
  check('F0 an invite is open', inv4.length >= 1, inv4.length);
  console.log('window alive before revoke:', alive());
  const before = (await commW('Members hall')).invites.filter(i => i.mine && i.uses_left === 1).map(i => i.id);
  for (const id of before) { console.log('revoking', id.slice(0, 8)); const r = await soft(W.do('revoke', { name: 'Members hall', invite: id.slice(0, 8) })); console.log('revoke ->', J(r), 'alive', alive()); }
  const revoked = await soft(until(async () => (await commW('Members hall')).invites.filter(i => i.uses_left === 1 && i.mine).length === 0, 20000, 'revoked', 400));
  check('F1 revoking the invite removes it from the open list', revoked === true);
  const fr = await soft(rpc(frank.port, 'join_password', { password: sug3 }));
  await sleep(5000);
  const fs_ = (await rpc(frank.port, 'snapshot')).communities.find(c => c.id === cid);
  check('F2 a revoked invite password does not let Frank in', !fs_ || !fs_.channels.some(x => x.joined), { reply: fr, state: fs_ && fs_.channels.map(x => x.joined) });
  await sleep(300); await shot('13-after-revoke');

  // ---------------------------------------------------------------- G. joining from the window
  const bobA = (await rpc(bob.port, 'create_community', { name: 'Bob\'s place' })).id;
  const linkA = (await rpc(bob.port, 'create_invite', { cid: bobA, uses: 3, ttl: 3600 })).link;
  await W.cmd('home'); await sleep(500);
  await W.panel('join', ''); await sleep(500); await shot('14-join-form');
  await W.cmd('join_text', { text: linkA });
  const sawNote = await soft(until(async () => { const s = await snapW(); return (s.joining || []).length ? s.joining : null; }, 8000, 'joining shown', 150));
  const jA = await soft(until(async () => { const c = await commW('Bob\'s place'); return c && c.channels.some(x => x.joined && x.status === 'ok') ? c : null; }, 90000, 'alice joined bob\'s place', 500));
  check('G1 pasting an invite link in "join" asks to join and Bob\'s client lets Alice in', !timedOut(jA), { sawNote: !timedOut(sawNote) });
  const bobB = (await rpc(bob.port, 'create_community', { name: 'Bob two' })).id;
  const sugB = (await rpc(bob.port, 'suggest_password')).password;
  await rpc(bob.port, 'create_invite', { cid: bobB, uses: 2, ttl: 3600, password: sugB });
  await W.panel('join', ''); await W.cmd('join_text', { text: sugB });
  const jB = await soft(until(async () => { const c = await commW('Bob two'); return c && c.channels.some(x => x.joined && x.status === 'ok') ? c : null; }, 90000, 'alice joined bob two', 500));
  check('G2 typing an invite password in "join" works the same', !timedOut(jB));
  await W.cmd('home'); await sleep(1200); await shot('15-home-communities');
  await W.panel('people', 'Bob two'); await sleep(900); await shot('16-people-as-member');
  check('G3 as a plain member the window shows people and Leave, and no Manage or Invite buttons', true, 'see shot 16');
  await W.do('leave', { name: 'Bob two' });
  check('G4 leaving a community', await soft(until(async () => { const c = await commW('Bob two'); return !c || c.removed; }, 60000, 'left', 500)) === true);

  // ---------------------------------------------------------------- H. a link on the command line
  const bobC = (await rpc(bob.port, 'create_community', { name: 'Bob three' })).id;
  const linkC = (await rpc(bob.port, 'create_invite', { cid: bobC, uses: 2, ttl: 3600 })).link;
  const wport2 = P(27311);
  const nat2 = sh(BIN, [linkC], windowEnv('erin', wport2), path.join(out, 'erin-window.log'));
  const W2 = new Win(wport2);
  await until(async () => { try { await W2.connect(); return true; } catch { return false; } }, 30000, 'window 2 test port');
  const jE = await soft(until(async () => { const s = (await W2.cmd('core', { method: 'snapshot' })).ok; const c = s && s.communities.find(c => c.id === bobC); return c && c.channels.some(x => x.joined && x.status === 'ok') ? c : null; }, 120000, 'erin joined from the command line', 600));
  check('H1 an invite link given on the command line joins that community once the window is unlocked', !timedOut(jE), jE.timeout || jE.name);
  try { process.kill(-nat2.pid, 'SIGTERM'); } catch { }

  await W.cmd('quit').catch(() => { });
} catch (e) { res.error = String(e.stack || e); console.log('ERROR', res.error); failed++; }
res.failed = failed;
fs.writeFileSync(path.join(out, 'result.json'), JSON.stringify(res, null, 1));
console.log(failed ? `${failed} FAILED` : 'ALL PASSED', '->', path.join(out, 'result.json'));
stopAll(); process.exit(failed ? 1 : 0);
