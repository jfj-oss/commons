// ui-ctl.mjs <port> '<json command>' ...  : one command per argument to a headless native engine's test port
import net from 'node:net';
const [port, ...cmds] = process.argv.slice(2);
const s = net.connect(Number(port), '127.0.0.1'); let buf = '', n = 0, left = cmds.length;
s.on('connect', () => { for (const c of cmds) s.write(JSON.stringify({ id: ++n, ...JSON.parse(c) }) + '\n'); });
s.on('data', d => { buf += d; let i; while ((i = buf.indexOf('\n')) >= 0) { const l = buf.slice(0, i); buf = buf.slice(i + 1); const j = JSON.parse(l); console.log(JSON.stringify(j).slice(0, process.env.FULL ? 100000 : 400)); if (--left === 0) { s.end(); process.exit(0); } } });
setTimeout(() => { console.error('timeout'); process.exit(1); }, 20000);
