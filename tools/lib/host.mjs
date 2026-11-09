// The hosted setup for the end-to-end tests: deploy/go-online.sh in its local-test mode, plus a stand-in for Tailscale Funnel
// (a TLS terminator on one port, which is all Funnel does here). Ports stay below the kernel's ephemeral range.
import fs from 'node:fs'; import path from 'node:path'; import tls from 'node:tls'; import net from 'node:net';
import { execFileSync } from 'node:child_process';
import { root, port } from './env.mjs';

export async function startHost(out) {
  const BASE = port(26600), FRONT = BASE + 5, PUB = port(26443), NAME = 'commons-test.localhost', PASS = 'correct horse battery';
  const env = { ...process.env, COMMONS_HOST_DIR: path.join(out, 'host'), COMMONS_HOST_BASE: String(BASE) };
  const script = (s, ...a) => execFileSync(path.join(root, 'deploy', s), a, { env, encoding: 'utf8', timeout: 60000 });
  execFileSync('openssl', ['req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes', '-keyout', path.join(out, 'k.pem'), '-out', path.join(out, 'c.pem'), '-days', '2', '-subj', '/CN=' + NAME], { stdio: 'ignore' });
  let bytes = 0;
  const terminator = tls.createServer({ key: fs.readFileSync(path.join(out, 'k.pem')), cert: fs.readFileSync(path.join(out, 'c.pem')) }, c => {
    const u = net.connect(FRONT, '127.0.0.1'); c.on('data', d => { bytes += d.length; }); c.pipe(u); u.pipe(c); c.on('error', () => u.destroy()); u.on('error', () => c.destroy());
  });
  terminator.listen(PUB, '127.0.0.1');
  const printed = script('go-online.sh', '--local-test', NAME, String(PUB));
  const web = /https:\/\/\S+\/join#relay\/\S+/.exec(printed)?.[0];
  return { BASE, FRONT, PUB, NAME, PASS, RELAY: `http://127.0.0.1:${FRONT}`, web, tlsIn: () => bytes, stopTerminator: () => terminator.close(),
    offline: () => { try { script('go-offline.sh', '--local-test'); } catch (e) { console.log(String(e)); } } };
}

// Relay-only: if a call works like this, nothing depended on a direct path to the server.
export const forceRelay = `(()=>{const N=window.RTCPeerConnection;const P=function(c,...r){const p=new N({...c,iceTransportPolicy:'relay'},...r);(window.__pcs=window.__pcs||[]).push(p);return p};P.prototype=N.prototype;Object.setPrototypeOf(P,N);window.RTCPeerConnection=P;const sc=N.prototype.setConfiguration;N.prototype.setConfiguration=function(c){return sc.call(this,{...c,iceTransportPolicy:'relay'})}})();`;
export const relayPath = p => p.ev(`(async()=>{const o=[];for(const pc of window.__pcs||[]){if(pc.connectionState!=='connected')continue;const s=await pc.getStats();const m=new Map();s.forEach(x=>m.set(x.id,x));s.forEach(x=>{if(x.type==='transport'&&x.selectedCandidatePairId){const l=m.get(m.get(x.selectedCandidatePairId).localCandidateId);o.push(l.candidateType+'/'+l.relayProtocol)}})}return o})()`);
export const screenShot = (page, dev) => page.ev(`(async()=>{ const c = window.__commonsCall; let t; for (let i = 0; i < 150 && !t; i++) { t = [...c.tracks.values()].find(x => x.identity === ${JSON.stringify(dev)} && x.source === 'screen_share'); if (!t) await new Promise(r => setTimeout(r, 200)); } if (!t) return { present: false };
  const v = document.createElement('video'); v.muted = true; v.autoplay = true; t.track.attach(v); document.body.appendChild(v); v.style.cssText = 'position:fixed;left:-9999px';
  const t0 = Date.now(); while (!(v.videoWidth > 0) && Date.now() - t0 < 15000) await new Promise(r => setTimeout(r, 100));
  await new Promise(r => setTimeout(r, 2500));
  const cv = document.createElement('canvas'); cv.width = v.videoWidth; cv.height = v.videoHeight; const g = cv.getContext('2d'); g.drawImage(v, 0, 0);
  const px = (fx, fy) => Array.from(g.getImageData(Math.round(fx * cv.width), Math.round(fy * cv.height), 1, 1).data.slice(0, 3));
  return { present: true, w: v.videoWidth, red: px(.19, .28), green: px(.51, .28) }; })()`);
