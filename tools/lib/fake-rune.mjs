// A small fake of rune's local HTTP API (what crates/core/src/rune.rs reads), for tests: never the user's real rune.
//   const r = await startFakeRune(port, { tracks, spotify: false, status: 'playing' });
//   r.cmds    every POST /api/player/pc body received, in order
//   r.state   the player state it serves (change it to simulate rune playing something else)
//   r.close()
import http from 'node:http';

export const SP_ID = '4uLU6hMCjMI75M1A2tKUQC';

export function startFakeRune(port, { tracks, spotify = false, status = 'playing' } = {}) {
  tracks = tracks || [{ title: 'Detective Arriving on the Scene', artist: 'Sea Power' }, { title: 'Evening Jazz', artist: 'The Test Quartet' }, { title: 'Third Song', artist: 'Someone' }];
  const mk = (t, i) => ({ queueId: 'q' + i, track: { id: spotify ? 'sp:' + SP_ID : 'l:' + i, source: spotify ? 'spotify' : 'local', spotifyUri: spotify ? 'spotify:track:' + SP_ID : undefined, title: t.title, artists: [{ name: t.artist }] }, album: { id: 'la:1', title: 'Test', coverUrl: '' } });
  const r = { cmds: [], gets: 0, state: { zone: 'pc', status, queue: tracks.map(mk), index: 0, positionMs: 61000, volume: 80, shuffle: false, repeat: 'off', updatedAt: Date.now() } };
  const n = () => r.state.queue.length;
  const server = http.createServer((req, res) => {
    const send = (code, body) => { res.writeHead(code, { 'content-type': 'application/json' }); res.end(JSON.stringify(body)); };
    const url = req.url.split('?')[0];
    if (req.method === 'GET' && url === '/api/zones') return send(200, [{ id: 'pc', name: 'Linux PC speakers', kind: 'pc' }]);
    if (req.method === 'GET' && url === '/api/player/pc') { r.gets++; r.state.updatedAt = Date.now(); return send(200, r.state); }
    if (req.method === 'GET' && url === '/__cmds') return send(200, r.cmds);
    if (req.method === 'POST' && url === '/api/player/pc') {
      let b = ''; req.on('data', d => b += d); req.on('end', () => {
        let c = {}; try { c = JSON.parse(b); } catch { }
        r.cmds.push(c);
        const s = r.state;
        if (c.type === 'pause') s.status = 'paused';
        else if (c.type === 'resume') s.status = 'playing';
        else if (c.type === 'next') s.index = (s.index + 1) % n();
        else if (c.type === 'prev') s.index = (s.index + n() - 1) % n();
        else if (c.type === 'seek') s.positionMs = c.positionMs;
        else if (c.type === 'play') { s.status = c.paused ? 'paused' : 'playing'; if (c.positionMs != null) s.positionMs = c.positionMs; }
        s.updatedAt = Date.now();
        send(200, s);
      });
      return;
    }
    send(404, { error: 'not found' });
  });
  return new Promise((res, rej) => { server.once('error', rej); server.listen(port, '127.0.0.1', () => res({ ...r, get cmds() { return r.cmds; }, get state() { return r.state; }, url: `http://127.0.0.1:${port}`, close: () => server.close() })); });
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const port = Number(process.argv[2] || 4546);
  const r = await startFakeRune(port, { spotify: process.argv[3] === 'spotify' });
  console.log('fake rune on', r.url);
}
