// The web side of the web|native screenshot pairs for the first screens: a fresh bridge with no account, headless Chrome.
//   COMMONS_TEST_PORT_OFFSET=5100 node tools/gpui-onboard-webshots.mjs <outdir>
import fs from 'node:fs';
import path from 'node:path';
import { startServers, startBridge, startChrome, openPage, rpc, sleep, stopAll, port as P } from './lib/env.mjs';
const out = process.argv[2]; fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(path.join(out, 'shots'), { recursive: true });
await startServers(out, { livekit: false });
const port = P(27200); startBridge(out, 'fresh', port); await sleep(1500);
const chrome = await startChrome(out, 'web', P(29400), 500);
const page = await openPage(chrome, `http://127.0.0.1:${port}/`, 'fresh', { before: "try{localStorage.setItem('commons.tour','1')}catch{}" });
await page.send('Emulation.setDeviceMetricsOverride', { width: 1100, height: 760, deviceScaleFactor: 1, mobile: false });
const shot = async n => { await sleep(700); const r = await page.send('Page.captureScreenshot', { format: 'png' }); fs.writeFileSync(path.join(out, 'shots', n + '.png'), Buffer.from(r.result.data, 'base64')); };
await page.waitFor(`!!document.querySelector('#ipw')`, 20000, 'first screen');
await shot('web-01-first-screen');
await page.ev(`document.querySelector('#l-new').click()`); await shot('web-02-create');
await page.ev(`document.querySelector('#l-pair').click()`); await shot('web-03-pair-form');
await page.ev(`document.querySelector('#l-rec').click()`); await shot('web-04-recover-form');
await page.ev(`document.querySelector('#l-pair').click()`); await sleep(300);
await page.ev(`(()=>{document.querySelector('#pp').value='correct horse battery';document.querySelector('#pp2').value='correct horse battery';document.querySelector('#go').click();})()`);
await page.waitFor(`!!document.querySelector('.qr svg')`, 20000, 'pair screen');
await shot('web-05-pair-waiting');
stopAll(); process.exit(0);
