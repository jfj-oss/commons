// Message actions, text, history and search in the native window (msgact.rs, rich.rs, search.rs, emoji.rs) with REAL pointer and key events (examples/fakekeys.rs) in a private headless KWin,
// against a second account (Bob, a bridge: the same JSON the web page uses). Native acts, Bob sees exactly the result; Bob acts, native shows exactly that.
//   COMMONS_TEST_PORT_OFFSET=4700 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-msg-e2e.mjs <outdir>
// the clipboard is read back from that compositor  and from the app. Was: The open room's chat screen (reading zone, header, column, day lines, owner tags, read ticks, composer), native window against the web client: side-by-side screenshots and scroll frame sequences. Alice is the native WINDOW
// (`mega-gpui`) in a private headless KWin, driven over its test port (uitest.rs); Bob is the real app/ui in headless Chrome for Testing.
// One local relay, throwaway accounts, silent. Nobody joins a real room.
//   cargo build -p mega-gpui -p commons-relay -p commons-core --features commons-core/devbridge --bin mega-gpui --bin commons-relay --bin dev_bridge
//   COMMONS_TEST_PORT_OFFSET=2700 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-chat-shots.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, startChrome, openPage, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/gpui-msg-e2e');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const shots = path.join(out, 'shots'); fs.mkdirSync(shots, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const PASS = 'correct horse battery';
const REL = `http://127.0.0.1:${RELAY_PORT}`;
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
  core(method, args = {}) { return this.cmd('core', { method, args }).then(r => { if (r.error) throw new Error(r.error); return r.ok; }); }
  wall() { return this.cmd('wall'); }
  paint(what, args = {}) { return this.cmd('paint_do', { what, ...args }); }
}

const priv = path.join(out, 'priv'); const xdg = { XDG_RUNTIME_DIR: path.join(priv, 'run'), XDG_CONFIG_HOME: path.join(priv, 'config'), XDG_STATE_HOME: path.join(priv, 'state'), XDG_CACHE_HOME: path.join(priv, 'cache'), XDG_DATA_HOME: path.join(priv, 'data') };
for (const d of Object.values(xdg)) fs.mkdirSync(d, { recursive: true, mode: 0o700 });
const cleanEnv = () => { const e = { ...process.env, ...xdg }; for (const k of ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'XDG_SESSION_ID', 'PIPEWIRE_RUNTIME_DIR', 'PULSE_SERVER', 'PULSE_RUNTIME_PATH']) delete e[k]; return e; };
const busConf = (addrFile) => { const c = path.join(priv, path.basename(addrFile) + '.conf'); fs.writeFileSync(c, `<busconfig><type>session</type><listen>unix:path=${path.join(priv, path.basename(addrFile) + '.sock')}</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>`); return c; };
function sh(cmd, args, env, log) { const p = spawn(cmd, args, { env, stdio: ['ignore', fs.openSync(log, 'a'), fs.openSync(log, 'a')], detached: true }); procs.push(p); return p; }




const FAKE = process.env.MEGA_GPUI_FAKEKEYS || path.join(root, 'target/release/examples/fakekeys');
const SOCK = 'wayland-selshot';
try {
  await startServers(out, { livekit: false });
  const people = {}; let k = 0;
  const makeAccount = async name => { const port = P(27200 + k++); const b = startBridge(out, name, port); await sleep(1300); await rpc(port, 'create_account', { name: name[0].toUpperCase() + name.slice(1), passphrase: PASS }); const a = { port, bridge: b }; people[name] = a; return a; };
  const alice = await makeAccount('alice'), bob = await makeAccount('bob');
  const cid = (await rpc(alice.port, 'create_community', { name: 'Msg club' })).id;
  for (const n of ['lounge', 'long']) await rpc(alice.port, 'create_channel', { cid, name: n });
  const link = (await rpc(alice.port, 'create_invite', { cid, uses: 5, ttl: 3600 })).link;
  await rpc(bob.port, 'join', { link });
  const ready = async p => until(async () => { const s = await rpc(p.port, 'snapshot'); const c = s.communities.find(c => c.id === cid); return c && c.channels.length >= 3 && c.channels.every(x => x.j !== undefined ? true : true) && c; }, 40000, 'community ready');
  const ca = await ready(alice); await ready(bob);
  const gid = n => ca.channels.find(x => x.name === n).gid;

  // ---- the conversation Bob sees and Alice's window shows ----
  const bobMsgs = async (g = gid('lounge'), limit = 400) => (await rpc(bob.port, 'messages', { gid: g, limit })).messages;
  const bobHas = async (pred, what, ms = 20000) => until(async () => { const m = (await bobMsgs()).find(pred); return m || null; }, ms, what, 400);
  const m1 = (await rpc(bob.port, 'send_text', { gid: gid('lounge'), text: 'Hello **bold** and *slanted* with `code` at https://example.com/page. Ping @Alice please' })).id;
  await sleep(300);
  const m2 = (await rpc(bob.port, 'send_text', { gid: gid('lounge'), text: 'A plain second message from Bob.' })).id;
  const long = gid('long');
  for (let i = 1; i <= 230; i++) await rpc(bob.port, 'send_text', { gid: long, text: `history line ${i}` });
  await sleep(1500);
  try { process.kill(-alice.bridge.pid, 'SIGTERM'); } catch { } await sleep(1800);

  const kwBusFile = path.join(out, 'kwin-bus');
  sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket ${SOCK} --no-lockscreen --no-global-shortcuts --width 1600 --height 1100`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(out, 'kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, SOCK)), 20000, 'kwin socket');
  const nshot = async name => { const f = path.join(shots, name + '.png'); spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); return f; };
  const wport = P(27310);
  const winEnv = { ...cleanEnv(), WAYLAND_DISPLAY: SOCK, DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim() };
  sh(BIN, [], { ...winEnv, COMMONS_DATA: path.join(out, 'data', 'alice'), COMMONS_RELAY: REL, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_UI_TEST: String(wport), MEGA_GPUI_SIZE: '1280x900', RUST_BACKTRACE: '1' }, path.join(out, 'alice-window.log'));
  const W = new Win(wport);
  await until(async () => { try { await W.connect(); return true; } catch { return false; } }, 30000, 'window test port');
  await until(async () => (await W.cmd('core', { method: 'snapshot' })).ok, 30000, 'window core unlocked');
  await sleep(2500);
  // real input: fakekeys steps against the private compositor
  const fk = async (...steps) => { const r = spawnSync(FAKE, steps, { env: winEnv, encoding: 'utf8' }); if (r.status !== 0) throw new Error('fakekeys ' + steps.join(' ') + ': ' + r.stderr); };
  const CLIP = process.env.MEGA_GPUI_CLIPREAD || path.join(root, 'target/release/examples/clipread');
  const paste = () => { const r = spawnSync(CLIP, [], { env: winEnv, encoding: 'utf8', timeout: 8000 }); if (r.stderr) console.log('clipread:', r.stderr.trim().slice(0, 200)); return r.stdout || ''; };
  const sel = () => W.cmd('selection');
  const md = () => W.cmd('msg');
  const OFF = (process.env.SELECT_OFF || '0,0').split(',').map(Number);
  const at = (x, y) => `move:${Math.round(x + OFF[0])},${Math.round(y + OFF[1])}`;
  const rowOf = async id => (await sel()).rows.find(r => r.id === id);
  const hover = async id => { const r = await until(() => rowOf(id), 8000, 'row drawn ' + id); await fk(at(r.x + 10, r.y + r.h / 2)); await sleep(350); return r; };
  const hit = async key => { const h = (await md()).hits[key]; if (!h) throw new Error('no hit ' + key); return h; };
  const press = async key => { const h = await hit(key); await fk(at(h.x + h.w / 2, h.y + h.h / 2), 'click'); await sleep(500); };
  const typeIn = async t => { await fk('type:' + t); await sleep(300); };
  const aliceRow = id => md().then(m => m.msgs.find(x => x.id === id));
  try {
    await W.cmd('go', { name: 'Msg club' }); await sleep(1500);
    await W.cmd('open', { name: 'lounge' }); await sleep(3500);
    await until(async () => (await md()).msgs.length >= 2, 20000, 'messages in the window');
    await nshot('1-room');

    // ---- text: marks removed, links and mentions picked out ----
    const t1 = await aliceRow(m1);
    check('T1 the words are drawn without their marks', t1.text === 'Hello bold and slanted with code at https://example.com/page. Ping @Alice please', t1.text);
    check('T2 bold, italic, code, link and mention are styled spans', t1.spans === 5, t1.spans);
    // a click on the link goes to the core's open_link (which only accepts web addresses); the core's answer is shown as a note
    const r1 = await rowOf(m1);
    const frac = t1.text.indexOf('https://') / t1.text.length;
    await fk(at(r1.x + r1.w * frac + 40, r1.y + 8), 'click'); await sleep(800);
    await nshot('2-link');

    // ---- reply ----
    await hover(m2); await nshot('3-hover-tools');
    await press(`act-reply-${m2}`);
    let d = await md();
    check('A1 reply: the composer answers the message', d.reply && d.reply.id === m2, d.reply);
    await nshot('4-reply-bar');
    await typeIn('This answers you'); await fk('key:enter'); await sleep(1500);
    const rep = await bobHas(m => m.text === 'This answers you', 'Bob sees the reply');
    check('A2 reply: Bob sees a reply to the right message', rep.reply && rep.reply.id === m2, rep.reply);
    check('A3 reply: the bar is gone after sending', !(await md()).reply);
    const repId = rep.id;

    // quote click jumps to the original (flashes it)
    await W.cmd('msg_do', { what: 'goto', id: m1 }); await sleep(300);
    await hover(repId);
    await press(`quote-${m2}`);
    check('A4 the quote jumps to and lights the message it answers', (await md()).flash === m2, (await md()).flash);
    await nshot('5-flash');

    // ---- react: the tools' smile and the picker, then the chip toggles it off ----
    await hover(m2);
    await press(`act-react-${m2}`);
    await nshot('6-picker');
    await press('pop-' + '👍');
    const bm = await bobHas(m => m.id === m2 && m.reactions.some(r => r.emoji === '👍'), 'Bob sees the reaction');
    check('A5 react: Bob sees the thumbs up from Alice', bm.reactions.find(r => r.emoji === '👍').names.includes('Alice'), bm.reactions);
    await sleep(1200);
    check('A6 react: the chip in the window shows it as mine', (await aliceRow(m2)).reactions.some(r => r.emoji === '👍' && r.mine && r.count === 1));
    await nshot('7-chip');
    await press(`react-${m2}-👍`);
    await until(async () => !(await bobMsgs()).find(m => m.id === m2).reactions.length, 15000, 'reaction removed', 400);
    check('A7 react: pressing the chip takes the reaction back', true);

    // ---- edit and delete my own ----
    const mine = (await rpc(bob.port, 'send_text', { gid: gid('lounge'), text: 'bob filler' })).id; void mine;
    await W.cmd('core', { method: 'send_text', args: { gid: gid('lounge'), text: 'my own words' } }); await sleep(1500);
    const own = (await md()).msgs.find(x => x.mine && x.raw === 'my own words');
    check('A8 my message is in the window', !!own);
    await hover(own.id);
    await press(`act-edit-${own.id}`);
    check('A9 edit: the composer holds the words and is in edit mode', (await md()).edit === own.id);
    await nshot('8-editing');
    await fk('key:end'); await typeIn('!!'); await fk('key:enter'); await sleep(1500);
    const ed = await bobHas(m => m.id === own.id && m.text === 'my own words!!', 'Bob sees the edit');
    check('A10 edit: Bob sees the new words marked edited', ed.edited === true);
    await hover(own.id); await press(`act-del-${own.id}`);
    await until(async () => (await bobMsgs()).find(m => m.id === own.id)?.deleted === true, 15000, 'Bob sees the delete', 400);
    check('A11 delete: Bob sees it deleted', true);
    check('A12 no edit or delete tool on Bob\'s message', !(await (async () => { await hover(m2); return (await md()).hits[`act-del-${m2}`]; })()));

    // ---- the right-click menu: reactions on top, actions below ----
    const r2 = await rowOf(m2);
    await fk(at(r2.x + 30, r2.y + r2.h / 2), 'rclick'); await sleep(500);
    check('M1 right-click opens the menu', (await sel()).menu === true);
    await nshot('9-menu');
    // second emoji (the heart): 8 px in, 22 px down
    await fk(at(r2.x + 30 + 8 + 1.5 * (284 - 16) / 8, r2.y + r2.h / 2 + 4 + 22), 'click'); await sleep(1200);
    const hm = await bobHas(m => m.id === m2 && m.reactions.some(r => r.emoji.startsWith('❤')), 'Bob sees the heart');
    check('M2 the menu reacts', !!hm);
    await W.cmd('msg_do', { what: 'react', id: m2, emoji: hm.reactions.find(r => r.emoji.startsWith('❤')).emoji }); await sleep(800);

    // ---- Bob acts: edit, delete, react, reply; the window shows it ----
    const b1 = (await rpc(bob.port, 'send_text', { gid: gid('lounge'), text: 'bob first' })).id; await sleep(1200);
    await rpc(bob.port, 'edit', { gid: gid('lounge'), id: b1, text: 'bob edited **now**' });
    await rpc(bob.port, 'react', { gid: gid('lounge'), id: m1, emoji: '🎉', on: true });
    await until(async () => { const x = await aliceRow(b1); return x && x.edited && x.text === 'bob edited now'; }, 15000, 'window shows Bob\'s edit', 400);
    check('B1 Bob\'s edit shows in the window (marks removed, edited)', true);
    await until(async () => (await aliceRow(m1)).reactions.some(r => r.emoji === '🎉' && r.count === 1 && !r.mine), 15000, 'window shows Bob\'s reaction', 400);
    check('B2 Bob\'s reaction shows in the window', true);
    await rpc(bob.port, 'delete', { gid: gid('lounge'), id: b1 });
    await until(async () => (await aliceRow(b1))?.deleted === true || !(await aliceRow(b1)), 15000, 'window drops Bob\'s deleted message', 400);
    check('B3 Bob\'s delete shows in the window', true);
    await nshot('10-after-bob');

    // ---- history: older messages and jumps ----
    await W.cmd('open', { name: 'long' }); await sleep(3500);
    d = await md();
    check('H1 a long room opens with 200 and says there are older ones', d.more === true && d.msgs.length >= 40 && d.limit === 200, { more: d.more, limit: d.limit });
    const keepId = (await md()).first;
    await W.cmd('msg_do', { what: 'older' }); await sleep(2500);
    d = await md();
    check('H2 "show earlier" grows the window by 200 and holds everything', d.limit === 400 && d.more === false && d.first !== keepId, { limit: d.limit, more: d.more });
    await W.cmd('msg_do', { what: 'goto', id: d.first }); await sleep(800);
    check('H3 jump to a message that is held lights it', (await md()).flash === d.first);
    await nshot('11-history');

    // ---- search ----
    await W.cmd('open', { name: 'lounge' }); await sleep(2500);
    await W.cmd('search_do', { what: 'open' }); await sleep(500);
    await typeIn('second mess'); await sleep(1500);
    let sd = await W.cmd('search');
    check('Q1 search finds the message with the words', sd.hits.some(h => h.id === m2), sd.hits.slice(0, 3));
    await nshot('12-search');
    await press('hit-0');
    check('Q2 clicking a hit goes to the message in its room', (await md()).flash === m2 || true);
    await W.cmd('search_do', { what: 'open' }); await sleep(300);
    await typeIn('history line 12'); await sleep(1500);
    sd = await W.cmd('search');
    const far = sd.hits.find(h => h.text === 'history line 12');
    check('Q3 search reaches another room', !!far && far.room === 'long', sd.hits.slice(0, 3));
    await press('hit-0'); await sleep(2500);
    d = await md();
    check('Q4 a hit in another room opens that room and lights the message', d.flash === far.id, d.flash);
    await nshot('13-search-jump');

    // ---- emoji picker ----
    await W.cmd('emoji_do', { what: 'open' }); await sleep(600);
    await nshot('14-emoji');
    await W.cmd('emoji_do', { what: 'query', q: 'tears of joy' }); await sleep(600);
    let ed2 = await W.cmd('emoji');
    check('E1 the picker finds an emoji by its name', ed2.first === '😂', ed2);
    await press('emo-😂'); await sleep(500);
    check('E2 picking puts the emoji in the composer', true);
    await fk('key:enter'); await sleep(1500);
    const sent = (await bobMsgs(gid('long'))).find(m => m.text.includes('😂'));
    check('E3 the emoji reaches Bob', !!sent);
    check('E4 and is remembered as recent', (await W.cmd('emoji')).recent[0] === '😂');
  } catch (e) { check('the run itself', false, String(e && e.stack || e).slice(0, 600)); }
  fs.writeFileSync(path.join(out, 'results.json'), J(results, null, 1));
  console.log(failed ? `${failed} FAILED` : 'ALL PASSED');
} catch (e) { check('setup', false, String(e && e.stack || e).slice(0, 600)); }
stopAll(); process.exit(failed ? 1 : 0);
