// In-page instrumentation for tools/voice-bench.mjs: a controllable synthetic microphone, taps on the audio tracks that are
// measured the same way on both sides, and a timeline of what the call engine did. Injected before the page's own scripts.
// Nothing here is shipped; the real microphone, speakers and sound are never used (AudioContext sinkId 'none', --mute-audio).
export const voiceProbe = (hz) => `(() => {
  const vb = window.__vb = { gens: [], taps: [], marks: [], mode: 'tone', pcs: [] };
  const now = () => performance.timeOrigin + performance.now();
  const mark = vb.mark = (k, v) => vb.marks.push([Math.round(now()), k, v]);
  const newCtx = () => { try { return new AudioContext({ sinkId: { type: 'none' } }); } catch { return new AudioContext(); } };
  const HZ = ${hz}, BURST = 0.1, PERIOD = 1.0, AMP = 0.35, NOISE = 0.05;

  // ---------------- the "microphone": a generator. mode: 'tone' continuous, 'bursts' 100 ms of tone per second then true silence
  // (Opus DTX applies), 'talk' the same bursts over a noise floor, so the encoder never goes quiet.
  function schedule(gen) {
    const c = gen.ctx, g = gen.g.gain, ng = gen.noise.gain;
    for (let t = gen.next; t < c.currentTime + 2; t += PERIOD) {
      if (t < c.currentTime + 0.02) continue;
      g.setValueAtTime(AMP, t); g.setValueAtTime(0, t + BURST);
      gen.next = t + PERIOD;
    }
  }
  vb.setMode = m => {
    vb.mode = m;
    for (const gen of vb.gens) {
      clearInterval(gen.timer);
      const c = gen.ctx, t0 = c.currentTime + 0.05;
      gen.g.gain.cancelScheduledValues(t0 - 0.02); gen.noise.gain.cancelScheduledValues(t0 - 0.02);
      gen.noise.gain.setValueAtTime(m === 'talk' ? NOISE : 0, t0);
      if (m === 'tone') gen.g.gain.setValueAtTime(AMP, t0);
      else { gen.g.gain.setValueAtTime(0, t0); gen.next = t0 + 0.3; schedule(gen); gen.timer = setInterval(() => schedule(gen), 400); }
    }
    return true;
  };
  const orig = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
  navigator.mediaDevices.getUserMedia = async (cons) => {
    if (cons && cons.audio && !cons.video) {
      mark('gum-start');
      const ctx = newCtx(); await ctx.resume().catch(() => { });
      const dest = ctx.createMediaStreamDestination();
      const osc = ctx.createOscillator(); osc.frequency.value = HZ;
      const g = ctx.createGain(); g.gain.value = 0;
      const noise = ctx.createGain(); noise.gain.value = 0;
      const nb = ctx.createBuffer(1, 48000 * 2, 48000), d = nb.getChannelData(0); for (let i = 0; i < d.length; i++) d[i] = (Math.random() * 2 - 1);
      const ns = ctx.createBufferSource(); ns.buffer = nb; ns.loop = true;
      osc.connect(g); g.connect(dest); ns.connect(noise); noise.connect(dest); osc.start(); ns.start();
      const gen = { ctx, g, noise, dest, next: 0 }; vb.gens.push(gen);
      vb.setMode(vb.mode);
      // The tap on our own signal is what the far side's arrival is compared with: the same detector at both ends.
      vb.tap('self', dest.stream.getAudioTracks()[0]);
      mark('gum-done');
      return dest.stream;
    }
    return orig(cons);
  };

  // ---------------- taps: one AudioContext per track, polled every ~4 ms. A sample index inside the analyser's window dates an onset.
  const THR_FIRST = 0.03, THR_ONSET = 0.15;
  vb.tap = (id, track) => {
    const ctx = newCtx(); ctx.resume().catch(() => { });
    const src = ctx.createMediaStreamSource(new MediaStream([track]));
    const an = ctx.createAnalyser(); an.fftSize = 2048; src.connect(an);
    const buf = new Float32Array(2048), sr = ctx.sampleRate;
    const tap = { id, sid: track.id, created: Math.round(now()), levels: [], onsets: [], first: 0, lastLoud: 0, lastLevel: 0 };
    const poll = () => {
      an.getFloatTimeDomainData(buf);
      const t = now();
      let rms = 0, max = 0;
      for (let i = 0; i < buf.length; i++) { const a = Math.abs(buf[i]); if (a > max) max = a; rms += a * a; }
      rms = Math.sqrt(rms / buf.length);
      if (t - tap.lastLevel >= 20) { tap.lastLevel = t; tap.levels.push([Math.round(t), +rms.toFixed(4), +max.toFixed(3)]); if (tap.levels.length > 30000) tap.levels.shift(); }
      if (!tap.first && max > THR_FIRST) { let idx = buf.findIndex(x => Math.abs(x) > THR_FIRST); tap.first = t - (buf.length - idx) / sr * 1000; mark('first-audio', id); }
      if (max > THR_ONSET) {
        if (t - tap.lastLoud > 250) { let idx = buf.findIndex(x => Math.abs(x) > THR_ONSET); tap.onsets.push(t - (buf.length - idx) / sr * 1000); }
        tap.lastLoud = t;
      }
    };
    tap.timer = setInterval(poll, 4);
    track.addEventListener('ended', () => { clearInterval(tap.timer); ctx.close().catch(() => { }); });
    vb.taps.push(tap);
    return tap;
  };
  vb.clearTaps = () => { vb.taps = vb.taps.filter(t => t.id === 'self'); for (const t of vb.taps) { t.levels = []; t.onsets = []; t.first = 0; } vb.marks.length = 0; return true; };
  vb.clearLevels = () => { for (const t of vb.taps) { t.levels = []; t.onsets = []; } vb.marks.length = 0; return true; };
  vb.firstAudio = id => { const ts = vb.taps.filter(t => t.id === id && t.first).map(t => t.first); return ts.length ? Math.min(...ts) : 0; };
  // Loudest level per 20 ms bin across all taps of one identity: a rejoin makes a new track, so a new tap.
  vb.levels = (id, from, to) => { const m = new Map(); for (const t of vb.taps) if (t.id === id) for (const [ts, rms] of t.levels) if (ts >= from && ts <= to) { const b = Math.round(ts / 20) * 20; m.set(b, Math.max(m.get(b) || 0, rms)); } return [...m.entries()].sort((a, b) => a[0] - b[0]); };
  vb.onsets = id => vb.taps.filter(t => t.id === id).flatMap(t => t.onsets).sort((a, b) => a - b);

  // ---------------- every peer connection, from the moment it is created
  const Native = window.RTCPeerConnection;
  const wrap = function (cfg, ...rest) {
    const pc = new Native(cfg, ...rest);
    const rec = { pc, cfg: JSON.parse(JSON.stringify({ iceServers: (cfg && cfg.iceServers || []).map(s => ({ urls: s.urls })), iceTransportPolicy: cfg && cfg.iceTransportPolicy, bundlePolicy: cfg && cfg.bundlePolicy })), local: [], remote: [], errors: [] };
    vb.pcs.push(rec); mark('pc-created', vb.pcs.length);
    pc.addEventListener('icegatheringstatechange', () => mark('ice-gathering', pc.iceGatheringState));
    pc.addEventListener('iceconnectionstatechange', () => mark('ice-connection', pc.iceConnectionState));
    pc.addEventListener('connectionstatechange', () => mark('pc-state', pc.connectionState));
    pc.addEventListener('icecandidate', e => { if (e.candidate) { const c = e.candidate; const row = [Math.round(now()), c.type, c.protocol, c.address, c.port, c.relayProtocol]; rec.local.push(row); mark('local-candidate', c.type + '/' + c.protocol); } else mark('ice-end'); });
    pc.addEventListener('icecandidateerror', e => { rec.errors.push({ url: e.url, code: e.errorCode, text: e.errorText, t: Math.round(now()) }); mark('ice-candidate-error', e.url + ' ' + e.errorCode); });
    const add = pc.addIceCandidate.bind(pc);
    pc.addIceCandidate = (c, ...r) => { if (c && c.candidate) { const p = /candidate:\\S+ \\d+ (\\S+) (\\d+) (\\S+) (\\d+) typ (\\S+)/.exec(c.candidate); rec.remote.push([Math.round(now()), p && p[6], p && p[1], p && p[3], p && p[4]]); mark('remote-candidate', p ? p[6] + '/' + p[1] : c.candidate); } return add(c, ...r); };
    return pc;
  };
  wrap.prototype = Native.prototype; Object.setPrototypeOf(wrap, Native); window.RTCPeerConnection = wrap;

  vb.setJbTarget = ms => { let n = 0; for (const r of vb.pcs) for (const x of r.pc.getReceivers()) if (x.track && x.track.kind === 'audio') { x.jitterBufferTarget = ms; n++; } return n; };
  // What the engine reports, straight from WebRTC.
  vb.stats = async () => {
    const out = [];
    for (const r of vb.pcs) {
      const pc = r.pc; if (pc.connectionState === 'closed') continue;
      const s = await pc.getStats(); const all = new Map(); s.forEach(x => all.set(x.id, x));
      const pairs = [], rtp = [];
      let selected = null;
      s.forEach(x => {
        if (x.type === 'transport' && x.selectedCandidatePairId) selected = x.selectedCandidatePairId;
        if (x.type === 'candidate-pair') { const l = all.get(x.localCandidateId) || {}, rc = all.get(x.remoteCandidateId) || {}; pairs.push({ id: x.id, state: x.state, nominated: x.nominated, local: l.candidateType + '/' + l.protocol + '/' + l.address + ':' + l.port + (l.relayProtocol ? '(' + l.relayProtocol + ')' : ''), remote: rc.candidateType + '/' + rc.protocol + '/' + rc.address + ':' + rc.port, rtt: x.currentRoundTripTime, requests: x.requestsSent, responses: x.responsesReceived }); }
        if (x.type === 'inbound-rtp' && x.kind === 'audio') rtp.push({ dir: 'in', mid: x.mid, packets: x.packetsReceived, lost: x.packetsLost, jitter: x.jitter, jbDelay: x.jitterBufferDelay, jbTarget: x.jitterBufferTargetDelay, jbMin: x.jitterBufferMinimumDelay, jbEmitted: x.jitterBufferEmittedCount, concealed: x.concealedSamples, silentConcealed: x.silentConcealedSamples, total: x.totalSamplesReceived, audioLevel: x.audioLevel, energy: x.totalAudioEnergy, duration: x.totalSamplesDuration, fec: x.fecPacketsReceived, fecDiscard: x.fecPacketsDiscarded, codec: (all.get(x.codecId) || {}).mimeType, fmtp: (all.get(x.codecId) || {}).sdpFmtpLine, playoutId: x.playoutId });
        if (x.type === 'outbound-rtp' && x.kind === 'audio') rtp.push({ dir: 'out', mid: x.mid, packets: x.packetsSent, bytes: x.bytesSent, retrans: x.retransmittedPacketsSent, codec: (all.get(x.codecId) || {}).mimeType, fmtp: (all.get(x.codecId) || {}).sdpFmtpLine, nack: x.nackCount, target: x.targetBitrate });
        if (x.type === 'media-playout') rtp.push({ dir: 'playout', delay: x.totalPlayoutDelay, count: x.totalSamplesCount, synth: x.synthesizedSamplesDuration });
      });
      out.push({ state: pc.connectionState, ice: pc.iceConnectionState, cfg: r.cfg, local: r.local, remote: r.remote, errors: r.errors, selected: pairs.find(p => p.id === selected) || null, pairs, rtp,
        localSdp: pc.localDescription ? pc.localDescription.sdp.split('\\r\\n').filter(l => /^(a=fmtp|a=rtpmap.*(opus|red)|a=ptime|a=maxptime)/.test(l)) : [], remoteSdp: pc.remoteDescription ? pc.remoteDescription.sdp.split('\\r\\n').filter(l => /^(a=fmtp|a=rtpmap.*(opus|red)|a=ptime|a=maxptime|m=audio)/.test(l)) : [],
        encodings: pc.getSenders().filter(x => x.track && x.track.kind === 'audio').map(x => { const p = x.getParameters(); return { encodings: p.encodings, codecs: p.codecs && p.codecs.slice(0, 2).map(c => c.mimeType + ' ' + (c.sdpFmtpLine || '')) }; }),
        receivers: pc.getReceivers().filter(x => x.track && x.track.kind === 'audio').map(x => ({ jitterBufferTarget: x.jitterBufferTarget, playoutDelayHint: x.playoutDelayHint })) });
    }
    return out;
  };

  // ---------------- the engine's own steps, once call.js has loaded
  vb.instrument = (c) => {
    const Call = c.constructor, P = Call.prototype;
    if (P.__vb) return true; P.__vb = true;
    const attach = P._attachAudio;
    P._attachAudio = function (identity, pub, track) { mark('subscribed', identity.slice(0, 8)); vb.tap(identity, track.mediaStreamTrack); return attach.call(this, identity, pub, track); };
    const connect = P._connect;
    P._connect = async function () { mark('connect-start'); try { return await connect.call(this); } finally { mark('connect-end'); } };
    const fetchCreds = P._fetchCredentials;
    P._fetchCredentials = async function () { mark('creds-start'); try { return await fetchCreds.call(this); } finally { mark('creds-end'); } };
    const pub = P._publishMic;
    P._publishMic = async function (on) { mark('mic-publish-start', on); try { return await pub.call(this, on); } finally { mark('mic-publish-end', on); } };
    const wire = P._wire;
    P._wire = function (room) { const E = this.LK.RoomEvent; for (const k of ['SignalConnected', 'Connected', 'Reconnecting', 'SignalReconnecting', 'Reconnected', 'Disconnected', 'TrackSubscribed', 'LocalTrackPublished', 'ParticipantConnected', 'TrackPublished']) if (E[k]) room.on(E[k], (...a) => mark('room:' + k, k === 'Disconnected' ? a[0] : undefined)); return wire.call(this, room); };
    const rejoin = P._autoRejoin;
    P._autoRejoin = function (r) { mark('auto-rejoin', r); return rejoin.call(this, r); };
    let phase = c.phase; setInterval(() => { if (c.phase !== phase) { phase = c.phase; mark('phase', phase); } }, 5);
    return true;
  };
})();
`;
