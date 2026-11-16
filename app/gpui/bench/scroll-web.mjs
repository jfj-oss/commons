// Screenshots of the real web UI for the scroll comparison. COMMONS_TEST_PORT_OFFSET=2100 node scroll-web.mjs <worlddir> <outdir> <W> <H>
// Needs the world from scroll-world.mjs to be up (relay) but not its bridges.
import fs from 'node:fs';
import path from 'node:path';
import { startBridge, startChrome, openPage, sleep, stopAll, port as P } from '../../../tools/lib/env.mjs';
const [wd, od, W, H] = [process.argv[2], process.argv[3], +process.argv[4] || 1280, +process.argv[5] || 900];
fs.mkdirSync(od, { recursive: true });
const wj = JSON.parse(fs.readFileSync(path.join(wd, 'world.json')));
const copy = path.join(od, 'webdata'); fs.rmSync(copy, { recursive: true, force: true }); fs.mkdirSync(path.join(copy, 'data'), { recursive: true });
fs.cpSync(path.join(wd, 'alice-pristine'), path.join(copy, 'data/alice'), { recursive: true });
const port = P(27290);
startBridge(copy, 'alice', port);
const chrome = await startChrome(od, 'web', P(29490), 500, [`--window-size=${W},${H}`]);
const page = await openPage(chrome, `http://127.0.0.1:${port}/`, 'alice', { before: "try { localStorage.setItem('commons.tour', '1'); } catch {}" });
await page.send('Emulation.setDeviceMetricsOverride', { width: W, height: H, deviceScaleFactor: 1, mobile: false });
await page.ev(`1`);
await sleep(2500);
await page.ev(`(()=>{const i=document.querySelector('input[type=password]')||document.querySelector('input');i.focus();i.value='throwaway-pass-1';i.dispatchEvent(new Event('input',{bubbles:true}));const b=[...document.querySelectorAll('button')].find(b=>/unlock/i.test(b.textContent));b.click();})()`);
await sleep(5000);
const shot = async name => { const r = await page.send('Page.captureScreenshot', { format: 'png' }); fs.writeFileSync(path.join(od, name + '.png'), Buffer.from(r.result.data, 'base64')); console.log(name); };
const wheel = async dy => { await page.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: W / 2, y: H / 2, deltaX: 0, deltaY: dy }); };
globalThis.__page = page;
const steps = (process.env.STEPS || 'home').split(',');
for (const s of steps) {
  if (s === 'home') { await page.ev(`document.querySelector('.mlist')?.scrollTo(0,0)`); await sleep(1500); await shot('web-home-rest'); }
  else if (s.startsWith('wheel')) { await wheel(+s.slice(5)); await sleep(80); await shot('web-' + s + '-mid'); await sleep(1500); await shot('web-' + s + '-rest'); }
  else if (s === 'key') { await page.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'ArrowDown', code: 'ArrowDown', windowsVirtualKeyCode: 40 }); await page.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'ArrowDown', code: 'ArrowDown', windowsVirtualKeyCode: 40 }); await sleep(1500); await shot('web-key'); }
  else if (s.startsWith('enter')) { await page.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 }); await page.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 }); await sleep(2500); if (s.length > 5) await shot('web-' + s); }
  else if (s.startsWith('keys:')) { const [, n, name, k = 'ArrowDown'] = s.split(':'); for (let i = 0; i < +n; i++) { await page.send('Input.dispatchKeyEvent', { type: 'keyDown', key: k, code: k, windowsVirtualKeyCode: k === 'ArrowDown' ? 40 : 38 }); await page.send('Input.dispatchKeyEvent', { type: 'keyUp', key: k, code: k, windowsVirtualKeyCode: k === 'ArrowDown' ? 40 : 38 }); await sleep(700); } await sleep(1600); await shot(name); }
  else if (s === 'gotit') { await page.ev(`[...document.querySelectorAll('button')].find(b=>/got it/i.test(b.textContent))?.click()`); await sleep(600); }
  else if (s.startsWith('railwheel')) { await page.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 28, y: 450 }); await page.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: 28, y: 450, deltaX: 0, deltaY: +s.slice(9) }); await sleep(1200); await shot('web-railopen'); }
  else if (s === 'eval') { console.log(await page.ev(process.env.EXPR)); }
  else if (s.startsWith('shot:')) { await shot(s.slice(5)); }
  else if (s.startsWith('sleep')) await sleep(+s.slice(5));
}
stopAll(); process.exit(0);
