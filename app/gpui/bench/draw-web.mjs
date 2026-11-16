// Screenshots (and a driver) of the real web UI for the colours / draw comparison. COMMONS_TEST_PORT_OFFSET=2300 node draw-web.mjs <worlddir> <outdir> <W> <H>
// STEPS is a comma list: home, key:<n>:<name> (arrow down n times), enter, chip:<paint|draw>, shot:<name>, sleep<ms>, hex:<#rrggbb>, mode:<card|back>, well:<0|1>,
// drag:x1:y1:x2:y2 (pen strokes in window pixels), tool:<glyph index>, col:<index>, click:<css selector>, eval (EXPR). Needs the world from scroll-world.mjs to be up.
import fs from 'node:fs';
import path from 'node:path';
import { startBridge, startChrome, openPage, sleep, stopAll, port as P } from '../../../tools/lib/env.mjs';
const [wd, od, W, H] = [process.argv[2], process.argv[3], +process.argv[4] || 1280, +process.argv[5] || 900];
fs.mkdirSync(od, { recursive: true });
const copy = path.join(od, 'webdata'); fs.rmSync(copy, { recursive: true, force: true }); fs.mkdirSync(path.join(copy, 'data'), { recursive: true });
fs.cpSync(path.join(wd, process.env.WHO ? process.env.WHO + '-pristine' : 'alice-pristine'), path.join(copy, 'data/' + (process.env.WHO || 'alice')), { recursive: true });
const port = P(27290);
startBridge(copy, process.env.WHO || 'alice', port);
const chrome = await startChrome(od, 'web', P(29490), 500, [`--window-size=${W},${H}`]);
const page = await openPage(chrome, `http://127.0.0.1:${port}/`, process.env.WHO || 'alice', { before: "try { localStorage.setItem('commons.tour', '1'); } catch {}" });
await page.send('Emulation.setDeviceMetricsOverride', { width: W, height: H, deviceScaleFactor: 1, mobile: false });
await page.ev(`1`);
await sleep(2500);
await page.ev(`(()=>{const i=document.querySelector('input[type=password]')||document.querySelector('input');i.focus();i.value='throwaway-pass-1';i.dispatchEvent(new Event('input',{bubbles:true}));const b=[...document.querySelectorAll('button')].find(b=>/unlock/i.test(b.textContent));b.click();})()`);
await sleep(5000);
const shot = async name => { const r = await page.send('Page.captureScreenshot', { format: 'png' }); fs.writeFileSync(path.join(od, name + '.png'), Buffer.from(r.result.data, 'base64')); console.log(name); };
const key = async k => { const vk = { ArrowDown: 40, ArrowUp: 38, Enter: 13, Escape: 27 }[k]; await page.send('Input.dispatchKeyEvent', { type: 'keyDown', key: k, code: k, windowsVirtualKeyCode: vk }); await page.send('Input.dispatchKeyEvent', { type: 'keyUp', key: k, code: k, windowsVirtualKeyCode: vk }); };
const mouse = (type, x, y) => page.send('Input.dispatchMouseEvent', { type, x, y, button: 'left', buttons: type === 'mouseReleased' ? 0 : 1, clickCount: 1 });
globalThis.__page = page;
const steps = (process.env.STEPS || 'home').split(',');
for (const s of steps) {
  const [k, ...a] = s.split(':');
  if (k === 'home') { await page.ev(`document.querySelector('.mlist')?.scrollTo(0,0)`); await sleep(1500); }
  else if (k === 'key') { for (let i = 0; i < +a[0]; i++) { await key('ArrowDown'); await sleep(700); } await sleep(1600); }
  else if (k === 'keyup') { for (let i = 0; i < +a[0]; i++) { await key('ArrowUp'); await sleep(700); } await sleep(1600); }
  else if (k === 'enter') { await key('Enter'); await sleep(2500); }
  else if (k === 'esc') { await key('Escape'); await sleep(800); }
  else if (k === 'chip') { await page.ev(`document.querySelector('[data-m="${a[0]}"]').click()`); await sleep(700); }
  else if (k === 'shot') await shot(a[0]);
  else if (k === 'sleep') await sleep(+a[0]);
  else if (k === 'hex') { await page.ev(`(()=>{const h=document.getElementById('mp-hex');h.focus();h.value=${JSON.stringify(a[0])};h.dispatchEvent(new Event('input',{bubbles:true}));})()`); await sleep(900); }
  else if (k === 'mode') { await page.ev(`document.querySelector('.mp-m[data-mode="${a[0]}"]').click()`); await sleep(500); }
  else if (k === 'well') { await page.ev(`document.querySelector('.mp-well[data-w="${a[0]}"]').click()`); await sleep(300); }
  else if (k === 'preset') { await page.ev(`document.querySelector('[data-p="${a[0]}"]').click()`); await sleep(900); }
  else if (k === 'tile') { await page.ev(`document.querySelector('.mp-t[data-k="${a[0]}"][data-n="${a[1]}"]').click()`); await sleep(900); }
  else if (k === 'click') { await page.ev(`document.querySelector(${JSON.stringify(a.join(':'))}).click()`); await sleep(500); }
  else if (k === 'tool') { await page.ev(`document.querySelector('#mw-glyphs [data-g="${a[0]}"]').click()`); await sleep(200); }
  else if (k === 'col') { await page.ev(`document.querySelector('#mw-cols [data-c="${a[0]}"]').click()`); await sleep(200); }
  else if (k === 'drag') { const [x1, y1, x2, y2] = a.map(Number); await mouse('mouseMoved', x1, y1); await mouse('mousePressed', x1, y1); for (let i = 1; i <= 24; i++) { await mouse('mouseMoved', x1 + (x2 - x1) * i / 24, y1 + (y2 - y1) * i / 24); await sleep(30); } await mouse('mouseReleased', x2, y2); await sleep(900); }
  else if (k === 'text') { await page.ev(`[...document.querySelectorAll('button')].find(b => b.textContent.trim().startsWith(${JSON.stringify(a[0])}) && b.offsetParent).click()`); await sleep(700); }
  else if (k === 'eval') { console.log(JSON.stringify(await page.ev(process.env.EXPR))); }
}
stopAll(); process.exit(0);
