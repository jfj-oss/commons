// The window's own frame and top row (titlebar.rs): shadow margin, 1 px light edge, dark band, rounded inner corners, glossy traffic lights,
// drag, edge resize, double-click maximize, close/minimize, with REAL pointer events (examples/fakekeys.rs) in a private headless KWin.
//   COMMONS_TEST_PORT_OFFSET=6100 systemd-run --user --scope -q -p MemoryMax=8G node tools/gpui-titlebar-e2e.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { root, startServers, startBridge, rpc, sleep, stopAll, procs, RELAY_PORT, port as P } from './lib/env.mjs';

const out = process.argv[2] || path.join(process.env.HOME, 'data/agent-tmp/ds-mega/titlebar/e2e');
fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const shots = path.join(out, 'shots'); fs.mkdirSync(shots, { recursive: true });
const BIN = process.env.MEGA_GPUI_BIN || path.join(root, 'target/debug/mega-gpui');
const PASS = 'correct horse battery';
const REL = `http://127.0.0.1:${RELAY_PORT}`;
const J = x => JSON.stringify(x);
const results = []; let failed = 0;
const check = (name, ok, detail = '') => { results.push({ name, ok: !!ok, detail }); if (!ok) failed++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? '  [' + (typeof detail === 'string' ? detail : J(detail)).slice(0, 300) + ']' : ''}`); };
async function until(fn, ms, what, step = 250) { const t = Date.now(); for (;;) { try { const v = await fn(); if (v) return v; } catch { } if (Date.now() - t > ms) throw new Error('timeout: ' + what); await sleep(step); } }

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
}

const priv = path.join(out, 'priv'); const xdg = { XDG_RUNTIME_DIR: path.join(priv, 'run'), XDG_CONFIG_HOME: path.join(priv, 'config'), XDG_STATE_HOME: path.join(priv, 'state'), XDG_CACHE_HOME: path.join(priv, 'cache'), XDG_DATA_HOME: path.join(priv, 'data') };
for (const d of Object.values(xdg)) fs.mkdirSync(d, { recursive: true, mode: 0o700 });
const cleanEnv = () => { const e = { ...process.env, ...xdg }; for (const k of ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'XDG_SESSION_ID', 'PIPEWIRE_RUNTIME_DIR', 'PULSE_SERVER', 'PULSE_RUNTIME_PATH']) delete e[k]; return e; };
const busConf = (addrFile) => { const c = path.join(priv, path.basename(addrFile) + '.conf'); fs.writeFileSync(c, `<busconfig><type>session</type><listen>unix:path=${path.join(priv, path.basename(addrFile) + '.sock')}</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>`); return c; };
function sh(cmd, args, env, log) { const p = spawn(cmd, args, { env, stdio: ['ignore', fs.openSync(log, 'a'), fs.openSync(log, 'a')], detached: true }); procs.push(p); return p; }

const FAKE = process.env.MEGA_GPUI_FAKEKEYS || path.join(root, 'target/release/examples/fakekeys');
const SOCK = 'wayland-uishot';
const SHADOW = 20, BAND = 4, ROW = 24, R_IN = 20, BALL = 14, GAP = 5.6, LX = 12;
try {
  await startServers(out, { livekit: false });
  const a = P(27200); const b = startBridge(out, 'alice', a); await sleep(1300);
  await rpc(a, 'create_account', { name: 'Alice', passphrase: PASS });
  const cid = (await rpc(a, 'create_community', { name: 'Frame club' })).id;
  await rpc(a, 'create_channel', { cid, name: 'lounge' });
  await sleep(1500);
  const gid = (await rpc(a, 'snapshot')).communities.find(c => c.id === cid).channels.find(x => x.name === 'lounge').gid;
  for (const t of ['first words here', 'and a second line']) { await rpc(a, 'send_text', { gid, text: t }); await sleep(200); }
  try { process.kill(-b.pid, 'SIGTERM'); } catch { } await sleep(1800);

  fs.cpSync(path.join(out, 'data', 'alice'), path.join(out, 'data', 'alice2'), { recursive: true });

  const kwBusFile = path.join(out, 'kwin-bus');
  sh('dbus-run-session', [`--config-file=${busConf(kwBusFile)}`, '--', 'bash', '-c', `echo "$DBUS_SESSION_BUS_ADDRESS" > ${kwBusFile}; exec kwin_wayland --virtual --socket ${SOCK} --no-lockscreen --no-global-shortcuts --width 1600 --height 1100`],
    { ...cleanEnv(), KWIN_SCREENSHOT_NO_PERMISSION_CHECKS: '1', KWIN_WAYLAND_NO_PERMISSION_CHECKS: '1' }, path.join(out, 'kwin.log'));
  await until(() => fs.existsSync(path.join(xdg.XDG_RUNTIME_DIR, SOCK)), 20000, 'kwin socket');
  const nshot = async name => { const f = path.join(shots, name + '.png'); spawnSync('python3', [path.join(root, 'tools/lib/kwinshot.py'), kwBusFile, f], { env: cleanEnv() }); return f; };
  const probe = (f, ...xy) => JSON.parse(spawnSync('python3', [path.join(root, 'tools/lib/frameprobe.py'), f, ...xy.map(Math.round)], { encoding: 'utf8' }).stdout || '{}');
  const winEnv = { ...cleanEnv(), WAYLAND_DISPLAY: SOCK, DBUS_SESSION_BUS_ADDRESS: fs.readFileSync(kwBusFile, 'utf8').trim() };
  const launch = (dir, port, size, log) => sh(BIN, [], { ...winEnv, COMMONS_DATA: dir, COMMONS_RELAY: REL, COMMONS_FAST_KDF: '1', MEGA_GPUI_PASS: PASS, MEGA_GPUI_CALL_AUDIO: 'none', MEGA_GPUI_SHORTCUTS: 'off', MEGA_GPUI_UI_TEST: String(port), MEGA_GPUI_SIZE: size, RUST_BACKTRACE: '1', MEGA_TB_DEBUG: '1', ...(process.env.ONDOWN ? { MEGA_TB_ON_DOWN: '1' } : {}) }, path.join(out, log));
  const wport = P(27310);
  const app = launch(path.join(out, 'data', 'alice'), wport, '1280x900', 'alice-window.log');
  const W = new Win(wport);
  await until(async () => { try { await W.connect(); return true; } catch { return false; } }, 30000, 'window test port');
  await until(async () => (await W.cmd('core', { method: 'snapshot' })).ok, 30000, 'window core unlocked');
  const fk = async (...steps) => { let r; for (let i = 0; i < 3; i++) { r = spawnSync(FAKE, steps, { env: winEnv, encoding: 'utf8' }); if (r.status === 0) return; await sleep(300); } throw new Error('fakekeys ' + steps.join(' ') + ': ' + r.stderr + ' ' + r.error + ' ' + r.status + ' ' + r.signal); };
  try {
    await W.cmd('go', { name: 'Frame club' }); await sleep(1200);
    await W.cmd('open', { name: 'lounge' }); await sleep(2500);
    const fr = async () => (await W.cmd('frame'));
    let f = await fr();
    check('F1 client decorations, framed', f.mode === 'Framed' && f.decorations.includes('Client'), f);
    check('F2 the content is the size asked for', Math.abs(f.content[0] - 1280) < 1 && Math.abs(f.content[1] - 900) < 1, f.content);
    const s1 = await nshot('1-focused'); let p = probe(s1);
    check('F3 the band is found, 4 px wide', !!p.band, p);
    if (p.band) {
      const [x0, y0, x1, y1] = p.band;
      check('F4 the band is the focused tone #151515', p.tone === 0x15, p.tone);
      check('F5 the framed window is content + 2 band, + the top row', Math.abs((x1 - x0 + 1) - (1280 + 2 * BAND)) <= 3 && Math.abs((y1 - y0 + 1) - (900 + ROW + 2 * BAND)) <= 3, { w: x1 - x0 + 1, h: y1 - y0 + 1 });
      const q = probe(s1, x0 - 1, y0 + 300, x0 - 3, y0 + 300, x0 + BAND + 2, y0 + 300, x0 + 8, y0 + 8, x0 + BAND + 2, y0 + BAND + 2);
      const [edge, outside, content, cornerBand, innerCorner] = q.px;
      check('F6 a faint light edge just outside the band (lighter than the wallpaper behind it)', edge[0] > outside[0] + 5 && edge[0] < 80, { edge, outside });
      check('F7 the corner mask: a band pixel where the square corner would be content', cornerBand.every(v => Math.abs(v - 0x15) <= 3), { cornerBand, innerCorner });
    }

    // ---- the lights: hover, press ----
    const [bx, by] = p.band;
    const ballAt = (i, bx_, by_) => [bx_ + BAND + LX + BALL / 2 + i * (BALL + GAP), by_ + BAND + ROW / 2];
    const lum = c => c[0] + c[1] + c[2];
    const names = ['close', 'minimize', 'maximize'];
    const normal = [0, 1, 2].map(i => { const [x, y] = ballAt(i, bx, by); return probe(s1, x, y - 3).px[0]; });
    check('L1 the three balls are red, yellow, green (normal)', normal[0][0] > 200 && normal[0][1] < 120 && normal[1][0] > 200 && normal[1][1] > 170 && normal[2][1] > 150 && normal[2][0] < 120, normal);
    for (let i = 0; i < 3; i++) {
      const [x, y] = ballAt(i, bx, by);
      await fk(`move:${Math.round(x)},${Math.round(y)}`, 'sleep:300');
      const sh = await nshot(`2-hover-${names[i]}`); const h = probe(sh, x, y - 3).px[0];
      check(`L${2 + i} hover on ${names[i]}: the same ball, brighter, no icon`, lum(h) > lum(normal[i]) + 15, { normal: normal[i], hover: h });
    }
    { const [x, y] = ballAt(2, bx, by);
      await fk(`move:${Math.round(x)},${Math.round(y)}`, 'mousedown', 'sleep:300');
      const sh = await nshot('3-pressed-maximize'); const pr = probe(sh, x, y - 3).px[0];
      check('L5 pressed: darker than normal', lum(pr) < lum(normal[2]) - 15, { normal: normal[2], pressed: pr });
      await fk('mouseup', 'sleep:1000');
      if ((await fr()).maximized) { await fk(`move:800,${BAND + ROW / 2 - 4}`, 'dblclick', 'sleep:1200'); }
      await fk('move:50,50', 'sleep:200'); }

    // ---- resize from an edge: the right edge grows the window ----
    const edgeX = bx + (1280 + 2 * BAND) , edgeY = by + 400;
    const w0 = (await fr()).viewport[0];
    await fk(`move:${edgeX},${edgeY}`, 'sleep:200', 'mousedown', 'sleep:100', `move:${edgeX + 10},${edgeY}`, 'sleep:150', `move:${edgeX + 30},${edgeY}`, 'sleep:150', `move:${edgeX + 55},${edgeY}`, 'sleep:150', `move:${edgeX + 80},${edgeY}`, 'sleep:600', `move:${edgeX + 80},${edgeY + 1}`, 'sleep:600', 'mouseup', 'sleep:800');
    const w1 = (await fr()).viewport[0];
    check('R1 dragging the right edge resizes the window', Math.abs((w1 - w0) - 80) <= 3, { w0, w1 });
    // a corner
    const sc = await nshot('5-resized'); const pc = probe(sc); const [cx0, cy0, cx1, cy1] = pc.band;
    const h0 = (await fr()).viewport[1];
    await fk(`move:${cx1 + 1},${cy1 + 1}`, 'sleep:200', 'mousedown', 'sleep:100', `move:${cx1 - 20},${cy1 - 20}`, `move:${cx1 - 40},${cy1 - 40}`, 'sleep:200', 'mouseup', 'sleep:800');
    const f2 = await fr();
    check('R2 dragging the bottom-right corner resizes both ways', Math.abs((h0 - f2.viewport[1]) - 40) <= 3 && Math.abs((w1 - f2.viewport[0]) - 40) <= 3, { h0, now: f2.viewport });
    // ---- drag the empty row: the window moves ----
    const pr0 = probe(await nshot('3b-before-drag')).band; const rowX = pr0[0] + 500, rowY = pr0[1] + BAND + ROW / 2;
    await fk(`move:${rowX},${rowY}`, 'mousedown', 'sleep:200', `move:${rowX + 20},${rowY + 10}`, `move:${rowX + 60},${rowY + 30}`, `move:${rowX + 120},${rowY + 50}`, 'sleep:200', 'mouseup', 'sleep:600');
    const s4 = await nshot('4-dragged'); const p4 = probe(s4);
    check('D1 dragging empty row space moves the window', p4.band && Math.abs((p4.band[0] - pr0[0]) - 120) <= 8 && Math.abs((p4.band[1] - pr0[1]) - 50) <= 8, { before: pr0, after: p4.band });
        // ---- double-click: maximize and back ----
    const s6 = await nshot('6-before-max'); const p6 = probe(s6);
    await fk(`move:${p6.band[0] + 500},${p6.band[1] + BAND + ROW / 2}`, 'dblclick', 'sleep:1200');
    f = await fr();
    check('M1 double-click on the row maximizes: flush, no inset', f.maximized && f.mode === 'Flush' && f.inset === 0, f);
    check('M2 maximized: the content is the whole output (1600 x 1100 less the row)', Math.abs(f.content[0] - 1600) <= 1 && Math.abs(f.content[1] - (1100 - ROW)) <= 1, f.content);
    const s7 = await nshot('7-maximized'); const p7 = probe(s7, 0, 0, 1, 40, 799, 1099, 1599, 600);
    check('M3 maximized: no band, no edge, no shadow (corners and edges are the content)', !p7.band && p7.px.slice(0, 2).every(c => lum(c) < 40), p7);
    const mb = ballAt(0, -BAND, -BAND); void mb;
    await fk(`move:800,${BAND + ROW / 2 - 4}`, 'dblclick', 'sleep:1200');
    f = await fr();
    check('M4 double-click again restores: framed', !f.maximized && f.mode === 'Framed', f);
    // the maximize ball does the same
    { const sh = await nshot('8-restored'); const pb = probe(sh).band; const [x, y] = ballAt(2, pb[0], pb[1]);
      await fk(`move:${Math.round(x)},${Math.round(y)}`, 'click', 'sleep:1200');
      check('M5 the green ball maximizes', (await fr()).maximized === true);
      await fk(`move:${Math.round(LX + BALL / 2 + 2 * (BALL + GAP))},${Math.round(ROW / 2)}`, 'click', 'sleep:1200');
      check('M6 and restores', (await fr()).maximized === false); }

    // ---- unfocused: a second window takes the focus ----
    const app2 = launch(path.join(out, 'data', 'alice2'), P(27311), '800x500', 'alice2-window.log');
    const W2 = new Win(P(27311));
    await until(async () => { try { await W2.connect(); return true; } catch { return false; } }, 30000, 'second window test port');
    await until(async () => (await W2.cmd('core', { method: 'snapshot' })).ok, 30000, 'second window core unlocked');
    await sleep(1500);
    const f1u = await fr();
    check('U1 the first window is not active any more', f1u.active === false, f1u);
    const su = await nshot('9-unfocused'); const pu = probe(su);
    check('U2 two frames on screen: the focused one #151515, the other #121212', pu.tone === 0x15, pu);
    // the first window's frame: find its own band (the 0x12 ring) through the probe of that tone
    const probeTone = (f, tone) => JSON.parse(spawnSync('python3', ['-c', `import sys,json\nfrom PIL import Image\nim=Image.open(sys.argv[1]).convert('RGB');W,H=im.size;p=im.load();t=${tone};rows=[y for y in range(H) if sum(1 for x in range(W) if p[x,y]==(t,t,t))>=100]\nmid=(rows[0]+rows[-1])//2 if rows else 0\nxs=[x for x in range(W) if p[x,mid]==(t,t,t)]\nprint(json.dumps({'band':[xs[0],rows[0],xs[-1],rows[-1]] if rows and xs else None}))`, f], { encoding: 'utf8' }).stdout || '{}');
    const pq = probeTone(su, 0x12);
    check('U3 the unfocused window has the lighter-off band and grey balls', !!pq.band, pq);
    if (pq.band) {
      const gb = [0, 1, 2].map(i => { const [x, y] = ballAt(i, pq.band[0], pq.band[1]); return probe(su, x, y - 3).px[0]; });
      check('U4 unfocused balls are grey', gb.every(c => Math.max(...c) - Math.min(...c) < 14), gb);
      const [x, y] = ballAt(0, pq.band[0], pq.band[1]);
      await fk(`move:${Math.round(x)},${Math.round(y)}`, 'sleep:300');
      const sh = await nshot('10-unfocused-hover'); const hc = probe(sh, x, y - 3).px[0];
      check('U5 hovering an unfocused ball shows the coloured one', hc[0] > 180 && hc[1] < 140, hc);
    }
    // ---- minimize the second window, then close the first ----
    { const pw = probe(su).band; const [x, y] = ballAt(1, pw[0], pw[1]);
      await fk(`move:${Math.round(x)},${Math.round(y)}`, 'click', 'sleep:1200');
      const sm = await nshot('11-after-minimize'); const pm = probe(sm);
      check('X1 the yellow ball minimizes the window (the first window is on top again)', pm.band && (pm.band[2] - pm.band[0]) > 1000, pm.band); }
    { const pw = probe(await nshot('12-before-close')).band; const [x, y] = ballAt(0, pw[0], pw[1]);
      await fk(`move:${Math.round(x)},${Math.round(y)}`, 'click');
      const gone = await until(() => app.exitCode !== null || app.signalCode !== null, 8000, 'the window closed and the app ended').then(() => true, () => false);
      check('X2 the red ball closes the window (and the app)', gone, { exit: app.exitCode }); }
    try { process.kill(-app2.pid, 'SIGTERM'); } catch { }
  } catch (e) { try { console.log('app exit', app.exitCode, app.signalCode); console.log(spawnSync('ps', ['-o', 'pid,stat,pcpu,etime', '-p', String(app.pid)], { encoding: 'utf8' }).stdout); } catch { } check('the run itself', false, String(e && e.stack || e).slice(0, 600)); }
  fs.writeFileSync(path.join(out, 'results.json'), J(results, null, 1));
  console.log(failed ? `${failed} FAILED` : 'ALL PASSED');
} catch (e) { check('setup', false, String(e && e.stack || e).slice(0, 600)); }
stopAll(); process.exit(failed ? 1 : 0);
