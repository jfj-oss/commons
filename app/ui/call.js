'use strict';
// Megatron voice rooms and screen share: the call engine. No DOM except hidden <audio> elements.
// The user interface lives in call-ui.js; tests drive this class directly through window.__commonsCall.
//
// Security model (see docs/DESIGN.md section 11 and docs/THREAT-MODEL.md):
//  * Media goes through a LiveKit SFU. Every audio/video frame is encrypted in the browser (insertable streams,
//    AES-GCM, in a Web Worker) before it is sent. The SFU forwards ciphertext.
//  * The frame key of each sending device comes from the room's MLS group (MLS exporter, done in Rust:
//    rpc "call_state"). It changes whenever the group's epoch changes (every membership change).
//  * A call REFUSES to start if frame encryption is unavailable. There is no unencrypted mode in this file.
//  * Remote tracks are only played if their sender is a current member of the room's MLS group and the track is
//    marked encrypted; everything else is left unsubscribed and reported.
(function () {
  const KEY_RING = 16;               // LiveKit's key ring size; the key index of an epoch is epoch % 16
  const OLD_KEY_GRACE_MS = 3000;     // how long the previous epoch's key stays usable for late frames
  const POLL_MS = 1500;              // how often the Rust core is asked for the current epoch
  const TOKEN_REFRESH_MARGIN = 60;   // seconds

  // "Play into the call": a movie is captured on the sender and published as the same screen-share sources as a real
  // screen share, so the existing tiled view shows it and every frame goes through the same E2EE pipeline. These are the
  // sender's quality choices. The top layer is the source; the others are simulcast layers so viewers on weaker links
  // still get a picture. Audio is stereo Opus at music quality (192 kbit/s), with no voice processing at all: the
  // audio never touches getUserMedia, it is captured with Web Audio from the <video> element.
  const PLAY_PROFILES = {
    '1080p30': { label: '1080p, 30 fps', width: 1920, height: 1080, fps: 30, bitrate: 7_000_000, layers: [[640, 360, 400_000, 15], [1280, 720, 2_500_000, 30]] },
    '1080p60': { label: '1080p, 60 fps', width: 1920, height: 1080, fps: 60, bitrate: 8_000_000, layers: [[640, 360, 400_000, 15], [1280, 720, 2_500_000, 30]] },
    '4k': { label: '4K, 30 fps', width: 3840, height: 2160, fps: 30, bitrate: 16_000_000, layers: [[640, 360, 400_000, 15], [1920, 1080, 5_000_000, 30]] },
  };
  const PLAY_AUDIO_BITRATE = 192_000;
  const PLAY_HOST_ID = 'call-media-host';
  const PLAY_MAX_LOAD_MS = 25000;    // a Jellyfin transcode may take a moment to produce its first frames

  const b64d = s => { s = s.replace(/-/g, '+').replace(/_/g, '/'); while (s.length % 4) s += '='; const bin = atob(s); const u = new Uint8Array(bin.length); for (let i = 0; i < bin.length; i++) u[i] = bin.charCodeAt(i); return u; };
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const lsGet = (k, d) => { try { const v = localStorage.getItem(k); return v === null ? d : JSON.parse(v); } catch { return d; } };
  const lsSet = (k, v) => { try { localStorage.setItem(k, JSON.stringify(v)); } catch { } };

  class CallError extends Error { constructor(kind, msg) { super(msg); this.kind = kind; } }

  // LiveKit gives every client Google's and Twilio's STUN servers unless TURN over UDP is on, which would tell them who is
  // calling, and it always says TURN over TLS is on port 443. Both are put right here, before any connection exists.
  // Only addresses change: frame encryption does not depend on the path the media takes.
  let turnPort = null;
  const fixIce = cfg => {
    if (!cfg || !cfg.iceServers) return cfg;
    const servers = cfg.iceServers.map(s => ({ ...s, urls: [].concat(s.urls || s.url || []).filter(u => !/^stuns?:.*(google|twilio)\.com/i.test(u)).map(u => turnPort ? u.replace(/^(turns:[^:?]+):443\b/i, `$1:${turnPort}`) : u) })).filter(s => s.urls.length);
    return { ...cfg, iceServers: servers };
  };
  function useOurIceServers(port) {
    turnPort = port || null;
    if (window.__commonsIce) return;
    window.__commonsIce = true;
    const Native = window.RTCPeerConnection;
    const Ours = function (cfg, ...rest) { return new Native(fixIce(cfg), ...rest); };
    Ours.prototype = Native.prototype; Object.setPrototypeOf(Ours, Native);
    window.RTCPeerConnection = Ours;
    const set = Native.prototype.setConfiguration;
    Native.prototype.setConfiguration = function (cfg) { return set.call(this, fixIce(cfg)); };
  }

  function describeSupport() {
    const w = window;
    const insertable = !!(w.RTCRtpSender && w.RTCRtpSender.prototype && typeof w.RTCRtpSender.prototype.createEncodedStreams === 'function');
    const scriptTransform = typeof w.RTCRtpScriptTransform !== 'undefined';
    const r = {
      secureContext: !!w.isSecureContext, webrtc: !!w.RTCPeerConnection, worker: typeof w.Worker !== 'undefined', subtle: !!(w.crypto && w.crypto.subtle),
      getUserMedia: !!(navigator.mediaDevices && navigator.mediaDevices.getUserMedia), getDisplayMedia: !!(navigator.mediaDevices && navigator.mediaDevices.getDisplayMedia),
      insertableStreams: insertable, scriptTransform, e2ee: insertable ? 'insertable-streams' : (scriptTransform ? 'script-transform' : null),
      outputSelect: !!(w.HTMLMediaElement && 'setSinkId' in w.HTMLMediaElement.prototype), library: !!w.LivekitClient, reasons: [],
    };
    if (!r.library) r.reasons.push('The call library (vendor/livekit-client.umd.js) did not load.');
    if (!r.secureContext) r.reasons.push('This page is not a secure context, so the browser hides camera, microphone and encryption APIs. Use https:// or http://127.0.0.1.');
    if (!r.webrtc) r.reasons.push('This browser or webview has no WebRTC (RTCPeerConnection), so it cannot make calls at all.');
    if (r.webrtc && !r.e2ee) r.reasons.push('This browser or webview cannot encrypt call frames (no insertable streams / RTCRtpScriptTransform). Megatron will not start a call without end-to-end encryption.');
    if (!r.worker || !r.subtle) r.reasons.push('This browser lacks Web Workers or WebCrypto, which the frame encryption needs.');
    if (!r.getUserMedia) r.reasons.push('This browser or webview does not offer microphone access (getUserMedia).');
    r.ok = r.reasons.length === 0;
    r.canShareScreen = r.ok && r.getDisplayMedia;
    return r;
  }

  class Call extends EventTarget {
    static support() { return describeSupport(); }

    constructor(opts) {
      super();
      this.rpc = opts.rpc;
      this.gate = new window.CommonsMicGate(on => this._publishMic(on));
      this.workerUrl = opts.workerUrl || 'vendor/livekit-client.e2ee.worker.js';
      this.LK = opts.LK || window.LivekitClient;
      this.audioHost = null;
      this.reset();
      this.prefs = Object.assign({ noiseSuppression: true, echoCancellation: true, autoGainControl: true, ptt: false, pttKey: 'Backquote', shareAudio: true, shareProfile: 'text', playProfile: '1080p30', forceCanvasPlayback: false, micId: '', outId: '', camId: '' }, lsGet('commons.call.prefs', {}));
      this.volumes = lsGet('commons.call.volumes', {});
      this._keydown = e => this._onKey(e, true); this._keyup = e => this._onKey(e, false);
    }

    reset() {
      this.room = null; this.kp = null; this.gid = null; this.roomName = ''; this.phase = 'idle'; this.error = null;
      this.keyState = null; this.appliedEpoch = null; this._keyed = new Set(); this.members = new Map(); this.byAccount = new Map();
      this.muted = false; this.gameMuted = false; this.gameName = null; this.gameError = ''; this.deafened = false; this.pttHeld = false; this.sharing = false; this.camera = false;
      this.blocked = new Map(); this.decryptFail = new Map(); this.tracks = new Map(); this.audioEls = new Map(); this.speaking = new Set();
      this.play = null; this.playHost = null;
      this.viewQuality = new Map(); this.attempt = 0; this.rotations = 0; this.lastRotation = 0; this.events = [];
      this._leaving = false; this._token = null;
    }

    log(msg) { this.events.push({ ts: Date.now(), msg }); if (this.events.length > 100) this.events.shift(); }
    changed() { if (this._pending) return; this._pending = setTimeout(() => { this._pending = null; this.dispatchEvent(new Event('change')); }, 60); }
    fail(kind, msg) { this.error = { kind, msg }; this.log('error: ' + msg); this.changed(); }
    savePrefs() { lsSet('commons.call.prefs', this.prefs); }

    // ------------------------------------------------------------------ join / leave
    async join(gid, roomName) {
      if (this.phase !== 'idle' && this.phase !== 'error') await this.leave();
      this.reset(); this.gid = gid; this.roomName = roomName || ''; this.phase = 'connecting'; this.changed();
      const sup = describeSupport();
      if (!sup.ok) { this.phase = 'error'; this.fail('unsupported', sup.reasons.join(' ')); throw new CallError('unsupported', sup.reasons.join(' ')); }
      try {
        await this._connect();
        this.attempt = 0;
      } catch (e) {
        this.phase = 'error';
        const ce = e instanceof CallError ? e : this._explain(e);
        this.fail(ce.kind, ce.message);
        await this._teardown();
        throw ce;
      }
      document.addEventListener('keydown', this._keydown); document.addEventListener('keyup', this._keyup);
      this._startPoll();
      this._watchGameKeys();
    }

    // Asks the app about game keys for as long as the call lasts. The app answers at once when something changes, and every few
    // seconds regardless, so silence means the app is gone, and then the keys hold nothing. An app without the feature just says
    // so, and is left alone.
    async _watchGameKeys() {
      const mine = this._gameWatch = {};
      const wait = ms => new Promise(r => setTimeout(r, ms));
      let seq = 0;
      while (this._gameWatch === mine) {
        try {
          const st = await Promise.race([this.rpc('game_mute_wait', { gid: this.gid, since: seq }), wait(9000).then(() => { throw new Error('the app stopped answering'); })]);
          if (this._gameWatch !== mine) return;
          seq = st.seq;
          this.gameName = st.game || null; this.gameError = st.error || '';
          if (st.muted !== this.gameMuted) { this.gameMuted = !!st.muted; this.log(st.muted ? 'muted by game key' : 'game key let go'); await this.gate.setGame(st.muted).catch(e => this.log('mic: ' + e.message)); }
          this.changed();
        } catch (e) {
          if (this._gameWatch !== mine) return;
          if (this.gameMuted) this.log('game keys lost: ' + (e.message || e));
          this.gameMuted = false; this.gameName = null;
          await this.gate.lost().catch(() => { });
          this.changed();
          await wait(/unknown method|not allowed/i.test(String(e.message || e)) ? 30000 : 1000);
        }
      }
    }
    // Unmute pressed while a game key has the mic: a toggled key gives way, a held one goes on until it is let go.
    async clearGameKey() { try { await this.rpc('game_mute_clear', { gid: this.gid }); } catch { } }

    _explain(e) {
      const m = (e && e.message) || String(e);
      if (/no call server configured/.test(m)) return new CallError('no-server', 'This relay has no call server configured. Ask whoever runs it to set up LiveKit (docs/DEPLOY.md).');
      if (/not a current member|not in this room|no longer a member/.test(m)) return new CallError('not-member', 'You are not a current member of this room, so you cannot join its call.');
      if (/NotAllowed|Permission|denied/i.test(m)) return new CallError('permission', 'Microphone permission was denied. Allow microphone access for Megatron in your system or browser settings, then try again.');
      if (/NotFound|Requested device not found/i.test(m)) return new CallError('no-device', 'No microphone was found. Plug one in or pick another device in call settings.');
      if (/websocket|could not establish|signal|timeout|ECONN|Failed to fetch|NetworkError|connect/i.test(m)) return new CallError('network', 'Could not reach the call server. Check your connection; if you run the server, check that its address and ports (TCP 443/7881, UDP 7882 or the TURN ports) are reachable. (' + m + ')');
      return new CallError('other', m);
    }

    async _fetchCredentials() {
      const [st, tok] = await Promise.all([this.rpc('call_state', { gid: this.gid }), this.rpc('call_token', { gid: this.gid })]);
      return { st, tok };
    }

    async _connect() {
      const LK = this.LK;
      const { st, tok } = await this._fetchCredentials();
      this._token = tok;
      this.me = st.me;
      // Per-sender keys: every device in the group gets its own key, so no two senders ever share a key/nonce space.
      const kp = new LK.BaseKeyProvider({ sharedKey: false, ratchetWindowSize: 0, failureTolerance: -1 });
      this.kp = kp;
      const worker = new Worker(this.workerUrl); this._worker = worker;
      const p = this.prefs;
      useOurIceServers(tok.turn_port);
      const room = new LK.Room({
        adaptiveStream: false, dynacast: true,
        e2ee: { keyProvider: kp, worker },
        audioCaptureDefaults: { deviceId: p.micId || undefined, echoCancellation: p.echoCancellation, noiseSuppression: p.noiseSuppression, autoGainControl: p.autoGainControl },
        videoCaptureDefaults: { deviceId: p.camId || undefined, resolution: LK.VideoPresets.h360 },
        audioOutput: p.outId ? { deviceId: p.outId } : undefined,
        stopLocalTrackOnUnpublish: true,
        // One quick resume (same session, ICE restart) is left to the SDK. If that fails the SDK would republish tracks into the old
        // frame-encryption pipeline, which broke decryption for some pairs after a server restart in testing; so a lost session ends
        // here and a fresh room, worker, keys and token are built (_autoRejoin).
        reconnectPolicy: { nextRetryDelayInMs: ctx => (ctx.retryCount === 0 ? 300 : null) },
      });
      this.room = room;
      this._wire(room);
      await this._applyKeys(st);          // keys are in place before the room is allowed to carry any media
      await room.setE2EEEnabled(true);    // marks everything we will publish as encrypted
      if (!room.localParticipant.isE2EEEnabled) throw new CallError('no-e2ee', 'Frame encryption could not be switched on, so the call was not started.');
      await room.connect(tok.url, tok.token, { autoSubscribe: false });
      // The worker confirms once it holds the keys and has switched our sender to encrypt. Nothing is published before this.
      for (let i = 0; i < 50 && !room.isE2EEEnabled; i++) await sleep(100);
      if (!room.isE2EEEnabled) throw new CallError('no-e2ee', 'The call connected but frame encryption did not start, so it was closed. Nothing was sent.');
      this.phase = 'connected'; this.log('connected, epoch ' + st.epoch);
      // microphone: join muted when push-to-talk is on
      this.muted = p.ptt ? true : this.muted;
      this.gate.manual = this.muted; this.gate.game = this.gameMuted;
      try { await this.gate.open(); }
      catch (e) { this.log('microphone unavailable: ' + (e.message || e)); const ce = this._explain(e); this.muted = true; this.gate.manual = true; this.fail(ce.kind === 'other' ? 'mic' : ce.kind, ce.message + ' You are in the call muted and can still listen.'); }
      room.remoteParticipants.forEach(rp => this._reviewParticipant(rp));
      this.changed();
    }

    async _publishMic(on) {
      const lp = this.room.localParticipant;
      const pub = await lp.setMicrophoneEnabled(on);
      if (on && pub && !pub.isEncrypted) { await lp.setMicrophoneEnabled(false); throw new CallError('no-e2ee', 'The server did not mark your microphone track as encrypted. The microphone was switched off.'); }
    }

    async leave() {
      this._leaving = true;
      this._gameWatch = null;
      clearInterval(this._poll); clearTimeout(this._rejoinTimer);
      document.removeEventListener('keydown', this._keydown); document.removeEventListener('keyup', this._keyup);
      await this._teardown();
      this.reset(); this.changed();
    }

    async _teardown() {
      this.gate.close();
      this._releasePlayback();                 // stop the movie's capture, audio graph and preview, even if the room is already gone
      const room = this.room; this.room = null;
      for (const el of this.audioEls.values()) { try { el.remove(); } catch { } }
      this.audioEls.clear(); this.tracks.clear();
      if (room) { try { room.removeAllListeners(); await room.disconnect(true); } catch { } }
      if (this._worker) { try { this._worker.terminate(); } catch { } this._worker = null; }   // each join/rejoin makes a new worker; do not leak the old one and its keys
    }

    // ------------------------------------------------------------------ keys from the MLS group
    async _applyKeys(st) {
      const LK = this.LK, kp = this.kp;
      const idx = Number(BigInt(st.epoch) % BigInt(KEY_RING));
      const prevEpoch = this.appliedEpoch;
      this.members = new Map(st.members.map(m => [m.device, m]));
      this.byAccount = new Map(); st.members.forEach(m => { if (!this.byAccount.has(m.account)) this.byAccount.set(m.account, []); this.byAccount.get(m.account).push(m.device); });
      const mats = new Map();
      this._keyed = this._keyed || new Set();
      for (const [device, key] of Object.entries(st.keys)) {
        const material = await LK.createKeyMaterialFromBuffer(b64d(key));
        mats.set(device, material); this._keyed.add(device);
        kp.onSetEncryptionKey(material, device, idx);
      }
      if (prevEpoch !== null && prevEpoch !== st.epoch) {
        this.rotations++; this.lastRotation = Date.now();
        const oldIdx = Number(BigInt(prevEpoch) % BigInt(KEY_RING));
        if (oldIdx !== idx) {
          // The previous epoch's key stays for a moment so frames in flight still play, then its slot is overwritten
          // with random bytes, so that a removed member who still holds that key cannot be decoded either.
          // Setting a key at an index also makes it the sender's current index, so the current keys are set again last.
          const members = [...this._keyed].filter(d => d !== this.me);   // never our own slot: setting a slot also selects it for SENDING, and the worker applies these asynchronously, so a late junk key could win the race and silence us
             // every device ever keyed, including ones that just left: their old slot must die too
          const gen = this.rotations;
          setTimeout(async () => {
            if (this.rotations !== gen || !this.kp) return;
            for (const d of members) { const junk = crypto.getRandomValues(new Uint8Array(32)); kp.onSetEncryptionKey(await LK.createKeyMaterialFromBuffer(junk.buffer), d, oldIdx); }
            for (const [d, m] of mats) kp.onSetEncryptionKey(m, d, idx);
            this.log('old key slot ' + oldIdx + ' wiped');
          }, OLD_KEY_GRACE_MS);
        }
        this.log(`keys rotated: epoch ${prevEpoch} to ${st.epoch}`);
      }
      this._mats = mats; this._idx = idx;
      this.appliedEpoch = st.epoch; this.keyState = { epoch: st.epoch, index: idx, devices: Object.keys(st.keys).length };
      if (this.room) this.room.remoteParticipants.forEach(rp => this._reviewParticipant(rp));
    }

    // The frame-encryption worker forgets a participant's keys when that participant drops out and comes back (server restart,
    // network loss). Keys are therefore installed again whenever a participant appears or one of its tracks is subscribed.
    _rekey(identity) {
      const m = this._mats && this._mats.get(identity);
      if (m && this.kp) this.kp.onSetEncryptionKey(m, identity, this._idx);
    }

    _startPoll() {
      clearInterval(this._poll);
      this._polling = false;
      this._poll = setInterval(async () => {
        if (this._polling || !this.room || this.phase === 'idle') return;
        this.audioEls.forEach(e => { if (e.paused && !this.deafened) this._play(e); });
        this._polling = true;
        try {
          const st = await this.rpc('call_state', { gid: this.gid });
          if (st.epoch !== this.appliedEpoch || st.members.length !== this.members.size) { await this._applyKeys(st); this.changed(); }
        } catch (e) {
          const m = (e && e.message) || String(e);
          if (/no longer a member|not joined|out of sync/.test(m)) {
            // The core says we are not in the room any more: stop media at once.
            const msg = /no longer a member/.test(m) ? 'You were removed from this room, so the call was ended on this device.' : m;
            await this.leave(); this.phase = 'error'; this.fail('removed', msg);
          }
        } finally { this._polling = false; }
      }, POLL_MS);
    }

    // ------------------------------------------------------------------ approval of remote tracks
    _reviewParticipant(rp) { rp.trackPublications.forEach(pub => this._reviewPub(rp, pub)); }
    _reviewPub(rp, pub) {
      const id = rp.identity;
      let why = null;
      if (!this.members.has(id) && !this._testTrustAll) why = 'not a current member of this room';   // _testTrustAll exists only so tests can prove the cryptography alone excludes a removed member
      else if (!pub.isEncrypted) why = 'not marked as encrypted';
      if (why) {
        if (pub.isSubscribed) pub.setSubscribed(false);
        this.blocked.set(id + '/' + pub.trackSid, { identity: id, source: pub.source, why });
        this.log(`blocked ${pub.source} from ${id.slice(0, 8)}: ${why}`);
      } else {
        this.blocked.delete(id + '/' + pub.trackSid);
        if (!pub.isSubscribed && !pub.isDesired) pub.setSubscribed(true);
        const q = this.viewQuality.get(pub.trackSid); if (q !== undefined && pub.kind === 'video') pub.setVideoQuality(q);
      }
      this.changed();
    }

    _wire(room) {
      const E = this.LK.RoomEvent, LK = this.LK;
      room.on(E.ParticipantConnected, rp => { this._rekey(rp.identity); this._reviewParticipant(rp); this.changed(); });
      room.on(E.ParticipantDisconnected, rp => { this.speaking.delete(rp.identity); for (const k of [...this.tracks.keys()]) if (k.startsWith(rp.identity + '/')) this.tracks.delete(k); this._dropAudio(rp.identity); this.changed(); });
      room.on(E.TrackPublished, (pub, rp) => this._reviewPub(rp, pub));
      room.on(E.TrackSubscribed, (track, pub, rp) => {
        // second line of defence: re-check before anything is attached
        if ((!this.members.has(rp.identity) && !this._testTrustAll) || !pub.isEncrypted) { pub.setSubscribed(false); return; }
        this._rekey(rp.identity);
        if (track.kind === 'audio') this._attachAudio(rp.identity, pub, track);
        else this.tracks.set(rp.identity + '/' + pub.trackSid, { identity: rp.identity, pub, track, source: pub.source });
        this.changed();
      });
      room.on(E.TrackUnsubscribed, (track, pub, rp) => { this.tracks.delete(rp.identity + '/' + pub.trackSid); if (track.kind === 'audio') this._dropAudio(rp.identity, pub.trackSid); this.changed(); });
      room.on(E.TrackUnpublished, (pub, rp) => { const k = rp.identity + '/' + pub.trackSid; this.tracks.delete(k); this.blocked.delete(k); this.viewQuality.delete(pub.trackSid); this._dropAudio(rp.identity, pub.trackSid); this.changed(); });
      room.on(E.TrackMuted, () => this.changed()); room.on(E.TrackUnmuted, () => this.changed());
      room.on(E.ActiveSpeakersChanged, sp => { this.speaking = new Set(sp.map(s => s.identity)); this.changed(); });
      room.on(E.Reconnecting, () => { this.phase = 'reconnecting'; this.log('connection lost; reconnecting'); this.changed(); });
      room.on(E.Reconnected, () => { this.phase = 'connected'; this.log('reconnected'); this._pollNow(); this.changed(); });
      room.on(E.SignalReconnecting, () => { this.log('signal reconnecting'); });
      room.on(E.Disconnected, reason => { this.log('disconnected: ' + reason); if (!this._leaving && this.room === room) this._autoRejoin(reason); });
      room.on(E.EncryptionError, (err, rp) => {
        const id = rp ? rp.identity : 'unknown';
        // self-heal: the worker may have lost this member's key (it forgets keys when a participant drops out and returns)
        this._rk = this._rk || new Map(); if (rp && Date.now() - (this._rk.get(id) || 0) > 2000) { this._rk.set(id, Date.now()); this._rekey(id); }
        const rec = this.decryptFail.get(id) || { n: 0, last: 0, msg: '' }; rec.n++; rec.last = Date.now(); rec.msg = (err && err.message) || ''; this.decryptFail.set(id, rec); this.changed();
      });
      room.on(E.MediaDevicesError, e => { const ce = this._explain(e); this.fail(ce.kind === 'other' ? 'device' : ce.kind, ce.message); });
      room.on(E.LocalTrackPublished, pub => { if (!pub.isEncrypted && this.room === room) { this.log('local track not encrypted: unpublishing'); room.localParticipant.unpublishTrack(pub.track); this.fail('no-e2ee', 'A track of yours was not encrypted and was stopped.'); } this.changed(); });
      room.on(E.LocalTrackUnpublished, pub => { if (pub.source === LK.Track.Source.ScreenShare) { this.sharing = false; } if (pub.source === LK.Track.Source.Camera) this.camera = false; this.changed(); });
      room.on(E.ParticipantEncryptionStatusChanged, () => this.changed());
    }

    _pollNow() { if (this._poll) { this._polling = false; } this.rpc('call_state', { gid: this.gid }).then(st => this._applyKeys(st)).catch(() => { }); }

    _audioHostEl() {
      if (!this.audioHost) { this.audioHost = document.createElement('div'); this.audioHost.id = 'call-audio'; this.audioHost.hidden = true; document.body.appendChild(this.audioHost); }
      return this.audioHost;
    }
    _attachAudio(identity, pub, track) {
      const el = track.attach(); el.autoplay = true; el.muted = this.deafened; this._audioHostEl().appendChild(el); this._play(el);
      this.audioEls.set(identity + '/' + pub.trackSid, el);
      const m = this.members.get(identity);
      const vol = m ? this.volumes[m.account] : undefined;
      if (vol !== undefined) track.setVolume ? track.setVolume(vol) : (el.volume = vol);
      this.tracks.set(identity + '/' + pub.trackSid, { identity, pub, track, source: pub.source, el });
      if (this.prefs.outId && el.setSinkId) el.setSinkId(this.prefs.outId).catch(() => { });
    }
    // Browsers may refuse to start sound until the user has clicked in the page; a paused element would mean silence for the user
    // (and for WebAudio taps of the track). Retry, and if the browser keeps refusing say so and resume on the next click.
    _play(el) {
      const p = el.play && el.play();
      if (p && p.catch) p.catch(() => {
        if (this._audioBlocked) return;
        this._audioBlocked = true; this.fail('audio-blocked', 'Your browser is holding back the sound until you click somewhere in this page. Click once and it will start.');
        const go = () => { document.removeEventListener('pointerdown', go, true); this._audioBlocked = false; if (this.error && this.error.kind === 'audio-blocked') this.error = null; try { this.room && this.room.startAudio(); } catch { } this.audioEls.forEach(e => this._play(e)); this.changed(); };
        document.addEventListener('pointerdown', go, true);
      });
    }
    _dropAudio(identity, sid) {
      for (const [k, el] of [...this.audioEls]) if (k.startsWith(identity + '/') && (!sid || k.endsWith('/' + sid))) { try { el.remove(); } catch { } this.audioEls.delete(k); }
    }

    // ------------------------------------------------------------------ automatic rejoin after the connection is lost for good
    _autoRejoin(reason) {
      if (this._rejoining) return; this._rejoining = true;
      this.phase = 'reconnecting'; this.changed();
      const run = async () => {
        const delays = [1000, 2000, 4000, 8000, 15000];
        const started = Date.now();
        for (let i = 0; !this._leaving; i++) {
          await this._teardown();
          try {
            this.keyState = null; this.appliedEpoch = null; this._keyed = new Set(); this.blocked.clear(); this.tracks.clear();
            await this._connect(); this.attempt = 0; this.error = null; this.log('rejoined automatically'); this._rejoining = false; this.changed(); return;
          } catch (e) {
            const ce = e instanceof CallError ? e : this._explain(e);
            this.log('rejoin attempt ' + (i + 1) + ' failed: ' + ce.message);
            if (ce.kind === 'not-member' || /no longer a member/.test(ce.message) || ce.kind === 'unsupported') { this._rejoining = false; await this.leave(); this.phase = 'error'; this.fail(ce.kind, ce.message); return; }
            this.attempt = i + 1; this.error = { kind: 'network', msg: 'Connection lost; trying again (attempt ' + (i + 1) + ')…' }; this.changed();
            if (Date.now() - started > 10 * 60 * 1000) { this._rejoining = false; this.phase = 'error'; this.fail('network', 'Could not get back into the call after 10 minutes. ' + ce.message); await this._teardown(); return; }
            await sleep(delays[Math.min(i, delays.length - 1)]);
          }
        }
        this._rejoining = false;
      };
      run();
    }

    // ------------------------------------------------------------------ controls
    async setMuted(m) {
      this.muted = m;
      try { await this.gate.setManual(m); } catch (e) { this.muted = true; this.gate.manual = true; const ce = this._explain(e); this.fail(ce.kind, ce.message); }
      this.changed();
    }
    async setDeafened(d) {
      this.deafened = d;
      this.audioEls.forEach(el => { el.muted = d; });
      if (d) await this.setMuted(true);   // deafen implies mute
      this.changed();
    }
    setPtt(on) { this.prefs.ptt = on; this.savePrefs(); if (on) this.setMuted(true); this.changed(); }
    setPttKey(code) { this.prefs.pttKey = code; this.savePrefs(); this.changed(); }
    _onKey(e, down) {
      if (!this.prefs.ptt || !this.room || e.code !== this.prefs.pttKey) return;
      const t = e.target, typing = t && (/^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName) || t.isContentEditable);
      if (typing && down) return;          // never steal keystrokes meant for a text field
      if (e.repeat) return;
      if (down && this.deafened) return;
      this.pttHeld = down; this.setMuted(!down);
    }
    pttButton(down) { if (!this.prefs.ptt || this.deafened) return; this.pttHeld = down; this.setMuted(!down); }

    setVolume(identity, v) {
      const m = this.members.get(identity); if (m) { this.volumes[m.account] = v; lsSet('commons.call.volumes', this.volumes); }
      for (const t of this.tracks.values()) if (t.identity === identity && t.track.kind === 'audio') { t.track.setVolume ? t.track.setVolume(v) : (t.el.volume = Math.min(1, v)); }
      this.changed();
    }
    volumeOf(identity) { const m = this.members.get(identity); const v = m ? this.volumes[m.account] : undefined; return v === undefined ? 1 : v; }

    async setNoiseSuppression(on) {
      this.prefs.noiseSuppression = on; this.prefs.echoCancellation = on || this.prefs.echoCancellation; this.savePrefs();
      const lp = this.room && this.room.localParticipant; const pub = lp && lp.getTrackPublication(this.LK.Track.Source.Microphone);
      if (pub && pub.track && pub.track.restartTrack) { try { await pub.track.restartTrack({ noiseSuppression: on, echoCancellation: this.prefs.echoCancellation, autoGainControl: this.prefs.autoGainControl, deviceId: this.prefs.micId || undefined }); } catch (e) { this.fail('device', 'Could not restart the microphone: ' + e.message); } }
      this.changed();
    }
    async listDevices() {
      const LK = this.LK; const out = { mic: [], out: [], cam: [] };
      try { out.mic = await LK.Room.getLocalDevices('audioinput', false); } catch { }
      try { out.cam = await LK.Room.getLocalDevices('videoinput', false); } catch { }
      try { if (describeSupport().outputSelect) out.out = await LK.Room.getLocalDevices('audiooutput', false); } catch { }
      return out;
    }
    async pickDevice(kind, id) {
      const key = { audioinput: 'micId', audiooutput: 'outId', videoinput: 'camId' }[kind]; this.prefs[key] = id; this.savePrefs();
      if (this.room) { try { await this.room.switchActiveDevice(kind, id); } catch (e) { this.fail('device', 'Could not switch to that device: ' + e.message); } }
      if (kind === 'audiooutput') this.audioEls.forEach(el => el.setSinkId && el.setSinkId(id).catch(() => { }));
      this.changed();
    }

    // ------------------------------------------------------------------ screen share and camera
    shareProfiles() { const P = this.LK.ScreenSharePresets; return { text: { label: 'Sharp text (1080p, low frame rate)', preset: P.h1080fps15, layers: [P.h360fps15, P.h720fps15] }, motion: { label: 'Smooth motion (720p, 30 fps)', preset: P.h720fps30, layers: [P.h360fps15] } }; }
    async startShare(opts = {}) {
      if (!this.room) throw new CallError('idle', 'Join the call first.');
      if (this.play) { const m = 'Stop playing the video before sharing your screen.'; this.fail('busy', m); throw new CallError('busy', m); }
      const sup = describeSupport();
      if (!sup.getDisplayMedia) { const m = 'This browser or webview cannot share a screen (no getDisplayMedia). Open the call in a Chromium or Firefox browser tab instead.'; this.fail('no-screen', m); throw new CallError('no-screen', m); }
      const prof = this.shareProfiles()[opts.profile || this.prefs.shareProfile] || this.shareProfiles().text;
      const wantAudio = opts.audio !== undefined ? opts.audio : this.prefs.shareAudio;
      try {
        const pub = await this.room.localParticipant.setScreenShareEnabled(true,
          { audio: wantAudio, resolution: { width: prof.preset.width, height: prof.preset.height, frameRate: prof.preset.encoding.maxFramerate }, contentHint: (opts.profile || this.prefs.shareProfile) === 'motion' ? 'motion' : 'detail', surfaceSwitching: 'include', systemAudio: 'include' },
          { screenShareEncoding: prof.preset.encoding, screenShareSimulcastLayers: prof.layers, videoCodec: 'vp8' });
        if (pub && !pub.isEncrypted) { await this.room.localParticipant.setScreenShareEnabled(false); throw new CallError('no-e2ee', 'The screen share was not encrypted and was stopped.'); }
        this.sharing = true;
        const audioPub = this.room.localParticipant.getTrackPublication(this.LK.Track.Source.ScreenShareAudio);
        this.shareHasAudio = !!audioPub;
        if (wantAudio && !audioPub) this.log('no audio came with the screen share (the chosen surface or this browser has no shareable audio)');
        // when the user stops from the browser's own "stop sharing" bar
        if (pub && pub.track) pub.track.mediaStreamTrack.addEventListener('ended', () => { this.sharing = false; this.changed(); });
      } catch (e) {
        this.sharing = false;
        if (e instanceof CallError) { this.fail(e.kind, e.message); throw e; }
        const m = (e && e.message) || String(e);
        const msg = /NotAllowed|Permission|denied/i.test(m) ? 'Screen sharing was cancelled or not allowed. If you did not cancel it, check your system\'s screen-recording permission for the browser or app.' : 'Could not start screen share: ' + m;
        this.fail('screen', msg); throw new CallError('screen', msg);
      }
      this.changed();
    }
    async stopShare() { if (this.room) await this.room.localParticipant.setScreenShareEnabled(false); this.sharing = false; this.changed(); }
    async setCamera(on) {
      if (!this.room) return;
      try {
        const pub = await this.room.localParticipant.setCameraEnabled(on);
        if (on && pub && !pub.isEncrypted) { await this.room.localParticipant.setCameraEnabled(false); throw new CallError('no-e2ee', 'The camera track was not encrypted and was stopped.'); }
        this.camera = on;
      } catch (e) { this.camera = false; const ce = e instanceof CallError ? e : this._explain(e); this.fail(ce.kind === 'other' ? 'camera' : ce.kind, ce.message.replace('microphone', 'camera')); }
      this.changed();
    }
    // ------------------------------------------------------------------ play into the call (a video with sound, for everyone)
    // One movie at a time per device, and never at the same time as a screen share: both use the screen-share source slots.
    playbackProfiles() { return PLAY_PROFILES; }
    playbackState() {
      const p = this.play; if (!p) return null;
      const v = p.video;
      return {
        title: p.title, kind: p.kind, profile: p.profile, paused: p.paused, volume: p.volume,
        position: v && isFinite(v.currentTime) ? v.currentTime : 0,
        duration: v && isFinite(v.duration) ? v.duration : 0,
        capture: p.capture, width: p.width || 0, height: p.height || 0,
      };
    }
    _playHostEl() {
      let host = document.getElementById(PLAY_HOST_ID);
      if (!host) {
        host = document.createElement('div'); host.id = PLAY_HOST_ID;
        // kept in the page (not display:none) so Chromium keeps decoding and captureStream keeps producing frames
        host.style.cssText = 'position:fixed;left:-10000px;top:0;width:320px;height:180px;overflow:hidden;pointer-events:none;opacity:.01';
        document.body.appendChild(host);
      }
      return host;
    }

    // ---- Jellyfin: configured per device, in this browser's localStorage only. The key never leaves this device.
    jellyfinConfig() {
      const c = lsGet('commons.call.jellyfin', null) || {};
      return { url: String(c.url || '').trim().replace(/\/+$/, ''), key: String(c.key || '').trim() };
    }
    saveJellyfinConfig(url, key) {
      lsSet('commons.call.jellyfin', { url: String(url || '').trim().replace(/\/+$/, ''), key: String(key || '').trim() });
      this.log('Jellyfin settings saved on this device');
      this.changed();
    }
    async jellyfinSearch(term) {
      const c = this.jellyfinConfig();
      if (!c.url) throw new CallError('jellyfin', 'Set your Jellyfin address in call Settings first.');
      if (!c.key) throw new CallError('jellyfin', 'Set your Jellyfin API key in call Settings first.');
      let u; try { u = new URL(c.url + '/Items'); } catch { throw new CallError('jellyfin', 'The Jellyfin address is not a valid URL.'); }
      if (!/^https?:$/.test(u.protocol)) throw new CallError('jellyfin', 'The Jellyfin address must start with http:// or https://.');
      u.searchParams.set('IncludeItemTypes', 'Movie,Episode');
      u.searchParams.set('Recursive', 'true');
      u.searchParams.set('Limit', '40');
      u.searchParams.set('Fields', 'ProductionYear,RunTimeTicks,PrimaryImageAspectRatio,SeriesName');
      const t = String(term || '').trim();
      if (t) { u.searchParams.set('searchTerm', t); u.searchParams.set('SortBy', 'SortName'); u.searchParams.set('SortOrder', 'Ascending'); }
      else { u.searchParams.set('SortBy', 'DateCreated,SortName'); u.searchParams.set('SortOrder', 'Descending'); }
      let r;
      try { r = await fetch(u.href, { headers: { 'X-Emby-Token': c.key }, mode: 'cors', credentials: 'omit' }); }
      catch (e) { throw new CallError('jellyfin', 'Could not reach Jellyfin at ' + c.url + '. Check the address and that Jellyfin is running, and that Jellyfin allows this page\'s origin (CORS). (' + (e.message || e) + ')'); }
      if (r.status === 401 || r.status === 403) throw new CallError('jellyfin', 'Jellyfin refused the API key. Check the key in call Settings.');
      if (!r.ok) throw new CallError('jellyfin', 'Jellyfin answered ' + r.status + ' ' + r.statusText + '.');
      let j; try { j = await r.json(); } catch { throw new CallError('jellyfin', 'Jellyfin sent an answer that is not JSON. Is the address really a Jellyfin server?'); }
      const items = (j && Array.isArray(j.Items)) ? j.Items : null;
      if (!items) throw new CallError('jellyfin', 'Jellyfin sent an unexpected answer.');
      return items.map(it => ({
        id: it.Id, name: it.Name || 'Untitled', type: it.Type || '', year: it.ProductionYear || null,
        series: it.SeriesName || '', ticks: it.RunTimeTicks || 0, hasImage: !!(it.ImageTags && it.ImageTags.Primary),
      }));
    }
    jellyfinImageUrl(item) {
      const c = this.jellyfinConfig();
      if (!c.url || !item || !item.hasImage) return '';
      // images cannot carry the X-Emby-Token header, so the key goes in the query; Jellyfin accepts api_key there
      return c.url + '/Items/' + encodeURIComponent(item.id) + '/Images/Primary?maxHeight=360&quality=80&api_key=' + encodeURIComponent(c.key);
    }
    async startJellyfinPlayback(item, opts = {}) {
      const c = this.jellyfinConfig();
      if (!c.url || !c.key) throw new CallError('jellyfin', 'Set up Jellyfin in call Settings first.');
      const prof = PLAY_PROFILES[opts.profile || this.prefs.playProfile] || PLAY_PROFILES['1080p30'];
      const q = encodeURIComponent;
      const urls = [
        // direct play first: no transcoding load on the server and the best picture
        c.url + '/Videos/' + q(item.id) + '/stream?static=true&api_key=' + q(c.key),
        // h264/aac mp4 transcode when the container or codec will not play in this browser (MKV/HEVC/DTS and friends)
        c.url + '/Videos/' + q(item.id) + '/stream.mp4?Static=false&VideoCodec=h264&AudioCodec=aac&AudioChannels=2&Container=mp4&MaxWidth=' + prof.width + '&MaxHeight=' + prof.height + '&api_key=' + q(c.key),
      ];
      return this._startPlayback({ title: item.name || 'Video', kind: 'jellyfin', urls, crossOrigin: true, profile: opts.profile });
    }
    async startFilePlayback(file, opts = {}) {
      const url = URL.createObjectURL(file);
      try { return await this._startPlayback({ title: file.name || 'Video file', kind: 'file', urls: [url], revoke: url, crossOrigin: false, profile: opts.profile }); }
      catch (e) { try { URL.revokeObjectURL(url); } catch { } throw e; }
    }
    async togglePlayback() { return this.play && this.play.paused ? this.resumePlayback() : this.pausePlayback(); }

    async _startPlayback(o) {
      if (!this.room) throw new CallError('idle', 'Join the call first.');
      if (this.sharing) throw new CallError('busy', 'Stop sharing your screen before playing a video into the call.');
      if (this.play) throw new CallError('busy', 'A video is already playing into the call. Stop it first.');
      const LK = this.LK;
      const profKey = PLAY_PROFILES[o.profile || this.prefs.playProfile] ? (o.profile || this.prefs.playProfile) : '1080p30';
      const prof = PLAY_PROFILES[profKey];
      const video = document.createElement('video');
      video.playsInline = true; video.preload = 'auto'; video.volume = 1;
      if (o.crossOrigin) video.crossOrigin = 'anonymous';   // Jellyfin: needed or the canvas is tainted and Web Audio goes silent
      this._playHostEl().appendChild(video);
      const done = [];
      let actx = null, vpub = null, apub = null, canvasStop = null;
      try {
        actx = new (window.AudioContext || window.webkitAudioContext)({ latencyHint: 'playback' });
        // The movie's audio never goes through getUserMedia, so echo cancellation, noise suppression and AGC cannot touch it.
        // One gain feeds both the published stereo track and the sender's own speakers.
        const srcNode = actx.createMediaElementSource(video);
        const gain = actx.createGain(); gain.gain.value = 1;
        const dest = actx.createMediaStreamDestination();
        dest.channelCount = 2; dest.channelCountMode = 'explicit'; dest.channelInterpretation = 'speakers';
        srcNode.connect(gain); gain.connect(dest); gain.connect(actx.destination);
        await actx.resume().catch(() => { });
        await this._openMedia(video, o.urls, o.crossOrigin);
        // Capture: the fast native path when the webview has HTMLMediaElement.captureStream and the source is not larger
        // than the chosen top layer; otherwise draw into a canvas (WebKit has no element capture). Large sources are
        // downscaled by the canvas path so the default really means 1080p.
        let vtrack = null, capture = 'element';
        const native = typeof video.captureStream === 'function' && !this.prefs.forceCanvasPlayback;
        const tooBig = (video.videoWidth || 0) > prof.width + 8 || (video.videoHeight || 0) > prof.height + 8;
        if (!native || tooBig) {
          capture = 'canvas';
          const vw = video.videoWidth || prof.width, vh = video.videoHeight || prof.height;
          let w = Math.min(prof.width, vw), h = Math.round(w * vh / vw);
          if (h > prof.height) { h = prof.height; w = Math.round(h * vw / vh); }
          w = Math.max(2, w - (w % 2)); h = Math.max(2, h - (h % 2));
          const canvas = document.createElement('canvas'); canvas.width = w; canvas.height = h;
          const g2d = canvas.getContext('2d', { alpha: false, desynchronized: true });
          const draw = () => { if (video.readyState >= 2) { try { g2d.drawImage(video, 0, 0, w, h); } catch { } } };
          draw();
          // requestVideoFrameCallback fires once per decoded frame and stops while paused, so a paused movie costs nothing;
          // rAF is the WebKit fallback (a paused movie keeps repeating its last frame, which is what viewers should see).
          if (typeof video.requestVideoFrameCallback === 'function') {
            let id = 0, stopped = false;
            const tick = () => { if (stopped) return; draw(); id = video.requestVideoFrameCallback(tick); };
            id = video.requestVideoFrameCallback(tick);
            canvasStop = () => { stopped = true; try { video.cancelVideoFrameCallback(id); } catch { } };
          } else {
            let raf = 0, stopped = false;
            const tick = () => { if (stopped) return; draw(); raf = requestAnimationFrame(tick); };
            raf = requestAnimationFrame(tick);
            canvasStop = () => { stopped = true; try { cancelAnimationFrame(raf); } catch { } };
          }
          const vstream = canvas.captureStream(prof.fps);
          vtrack = vstream.getVideoTracks()[0];
          done.push(...vstream.getTracks());
          this._playCanvas = canvas;
        } else {
          const vstream = video.captureStream();
          vtrack = vstream.getVideoTracks()[0];
          vstream.getAudioTracks().forEach(t => { try { t.stop(); } catch { } });   // audio comes from Web Audio, not the element's stream
          if (vtrack) done.push(vtrack);
        }
        if (!vtrack) throw new CallError('play', 'This video could not be captured as a stream in this browser.');
        vtrack.contentHint = 'motion';
        const atrack = dest.stream.getAudioTracks()[0];
        if (!atrack) throw new CallError('play', 'The audio of this video could not be captured.');
        atrack.contentHint = 'music';
        done.push(atrack);
        await video.play();
        const layers = prof.layers.map(([w, h, maxBitrate, maxFramerate]) => new LK.VideoPreset({ width: w, height: h, maxBitrate, maxFramerate }));
        vpub = await this.room.localParticipant.publishTrack(vtrack, {
          name: 'movie', source: LK.Track.Source.ScreenShare, simulcast: true,
          screenShareEncoding: { maxBitrate: prof.bitrate, maxFramerate: prof.fps },
          screenShareSimulcastLayers: layers, videoCodec: 'vp8', degradationPreference: 'maintain-framerate',
        });
        apub = await this.room.localParticipant.publishTrack(atrack, {
          name: 'movie-audio', source: LK.Track.Source.ScreenShareAudio,
          audioPreset: { maxBitrate: PLAY_AUDIO_BITRATE, priority: 'high' },
          forceStereo: true, dtx: false, red: true,     // no DTX: a quiet passage must not be gated away
        });
        for (const pub of [vpub, apub]) {
          if (!pub || !pub.isEncrypted) {
            throw new CallError('no-e2ee', 'The movie was not encrypted, so it was stopped. Nothing was sent.');
          }
        }
        const st = {
          title: o.title || 'Video', kind: o.kind || 'file', profile: profKey, paused: false, volume: 1,
          video, actx, gain, dest, vpub, apub, vtrack, atrack, capture, revoke: o.revoke || null, width: video.videoWidth, height: video.videoHeight,
        };
        this.play = st;
        this._playCanvas = capture === 'canvas' ? this._playCanvas : null;
        this._playCanvasStop = canvasStop;
        video.addEventListener('timeupdate', () => { const now = Date.now(); if (now - (this._playPaintAt || 0) >= 500) { this._playPaintAt = now; this.changed(); } });
        video.addEventListener('ended', () => { if (this.play && this.play.video === video) this.pausePlayback(); });
        video.addEventListener('error', () => { if (this.play && this.play.video === video) { const m = video.error ? ' ' + video.error.message : ''; this.fail('play', 'The video stopped because the browser could not decode it.' + m); this.stopPlayback(); } });
        this.log('playing "' + st.title + '" into the call (' + capture + ', ' + profKey + ')');
        this.changed();
        return this.playbackState();
      } catch (e) {
        for (const pub of [apub, vpub]) { try { if (pub && pub.track) await this.room.localParticipant.unpublishTrack(pub.track); } catch { } }
        for (const t of done) { try { t.stop(); } catch { } }
        if (canvasStop) { try { canvasStop(); } catch { } }
        if (actx) { try { actx.close(); } catch { } }
        this._playCanvas = null; this._playCanvasStop = null;
        try { video.pause(); video.removeAttribute('src'); video.load(); video.remove(); } catch { }
        if (o.revoke) { try { URL.revokeObjectURL(o.revoke); } catch { } }
        const ce = e instanceof CallError ? e : new CallError('play', 'Could not play this video: ' + ((e && e.message) || e));
        this.fail(ce.kind, ce.message);
        throw ce;
      }
    }

    // Loads the first URL that the browser accepts. Jellyfin's static stream is tried before its transcode.
    async _openMedia(video, urls, crossOrigin) {
      let last = null;
      for (const url of urls) {
        try {
          await new Promise((res, rej) => {
            let timer = 0;
            const fin = err => { clearTimeout(timer); video.removeEventListener('loadeddata', ok); video.removeEventListener('error', bad); err ? rej(err) : res(); };
            const ok = () => fin();
            const bad = () => { const me = video.error; fin(new Error(me ? (me.message || ('media error ' + me.code)) : 'the media could not be loaded')); };
            timer = setTimeout(() => { this.log('play: no loadeddata after ' + PLAY_MAX_LOAD_MS + ' ms (readyState ' + video.readyState + ', networkState ' + video.networkState + ', src ' + (video.currentSrc || '').slice(0, 40) + ')'); fin(new Error('it took too long to start')); }, PLAY_MAX_LOAD_MS);
            video.addEventListener('loadeddata', ok);
            video.addEventListener('error', bad);
            video.src = url; video.load();
          });
          return url;
        } catch (e) {
          last = e;
          this.log('play: a source failed: ' + e.message);
          try { video.pause(); video.removeAttribute('src'); video.load(); } catch { }
        }
      }
      const hint = crossOrigin ? ' If this is a CORS problem, Jellyfin must allow this page\'s origin (Jellyfin\'s allowed origins / a reverse proxy that adds Access-Control-Allow-Origin).' : '';
      throw new CallError('play', 'Could not load the video' + (last ? ': ' + last.message : '') + '.' + hint);
    }

    async pausePlayback() {
      const p = this.play; if (!p || p.paused) return;
      p.paused = true;
      try { p.video.pause(); } catch { }
      // Video: the track keeps its last frame (no black frame); audio: mute the Opus track, so silence, not stray samples.
      if (p.apub && p.apub.track && p.apub.track.mute) { try { await p.apub.track.mute(); } catch { } }
      this.changed();
    }
    async resumePlayback() {
      const p = this.play; if (!p || !p.paused) return;
      try { await p.video.play(); } catch (e) { this.fail('play', 'Could not resume the video: ' + (e.message || e)); return; }
      p.paused = false;
      if (p.actx && p.actx.state === 'suspended') { try { await p.actx.resume(); } catch { } }
      if (p.apub && p.apub.track && p.apub.track.unmute) { try { await p.apub.track.unmute(); } catch { } }
      this.changed();
    }
    seekPlayback(sec) {
      const p = this.play; if (!p) return;
      const d = p.video && isFinite(p.video.duration) ? p.video.duration : sec;
      try { p.video.currentTime = Math.max(0, Math.min(Number(sec) || 0, d)); } catch { }
      this.changed();
    }
    setPlaybackVolume(v) {
      const p = this.play; if (!p || !p.gain) return;
      p.volume = Math.max(0, Math.min(1, Number(v) || 0));
      try { p.gain.gain.setTargetAtTime(p.volume, p.actx.currentTime, 0.03); } catch { p.gain.gain.value = p.volume; }
      this.changed();
    }
    async stopPlayback() {
      const p = this.play; this.play = null;
      const lp = this.room && this.room.localParticipant;
      for (const pub of [p && p.vpub, p && p.apub]) { try { if (lp && pub && pub.track) await lp.unpublishTrack(pub.track); } catch { } }
      this._releasePlayback(p);
      if (p) this.log('stopped playing "' + p.title + '"');
      this.changed();
    }
    // Stops the capture, the audio graph and the hidden preview. Safe to call when nothing is playing.
    _releasePlayback(p = this.play) {
      if (!p) return;
      if (this._playCanvasStop) { try { this._playCanvasStop(); } catch { } this._playCanvasStop = null; }
      for (const t of [p.vtrack, p.atrack]) { try { t && t.stop(); } catch { } }
      try { p.actx && p.actx.close(); } catch { }
      try { if (p.video) { p.video.pause(); p.video.removeAttribute('src'); p.video.load(); p.video.remove(); } } catch { }
      if (p.revoke) { try { URL.revokeObjectURL(p.revoke); } catch { } }
      this._playCanvas = null;
      if (this.play === p) this.play = null;
    }

    // viewer-side quality: 'auto' | 'high' | 'medium' | 'low'
    setViewQuality(trackSid, q) {
      const Q = this.LK.VideoQuality; const v = { high: Q.HIGH, medium: Q.MEDIUM, low: Q.LOW }[q];
      const t = [...this.tracks.values()].find(x => x.pub.trackSid === trackSid); if (!t) return;
      if (q === 'auto') { this.viewQuality.delete(trackSid); t.pub.setVideoQuality && t.pub.setVideoQuality(Q.HIGH); } else { this.viewQuality.set(trackSid, v); t.pub.setVideoQuality(v); }
      this.changed();
    }
    qualityOf(trackSid) { const Q = this.LK.VideoQuality; const v = this.viewQuality.get(trackSid); return v === undefined ? 'auto' : ({ [Q.HIGH]: 'high', [Q.MEDIUM]: 'medium', [Q.LOW]: 'low' }[v] || 'auto'); }

    // ------------------------------------------------------------------ a plain snapshot for the UI
    snapshot() {
      const LK = this.LK, now = Date.now();
      const peers = [];
      const add = (identity, p, me) => {
        const m = this.members.get(identity);
        const fail = this.decryptFail.get(identity);
        peers.push({
          identity, me, name: m ? m.name : 'Unknown device ' + identity.slice(0, 8), account: m ? m.account : null, member: !!m,
          speaking: me ? this.speaking.has(identity) && !this.muted && !this.gameMuted : this.speaking.has(identity),
          muted: me ? this.muted || this.gameMuted : (p ? !p.isMicrophoneEnabled : true), volume: me ? 1 : this.volumeOf(identity),
          cantDecrypt: !!(fail && now - fail.last < 6000), decryptMsg: fail ? fail.msg : '',
          quality: p && !me ? p.connectionQuality : undefined,
        });
      };
      if (this.room) {
        add(this.me, this.room.localParticipant, true);
        this.room.remoteParticipants.forEach(rp => add(rp.identity, rp, false));
      }
      const shares = [];
      if (this.room) {
        // A muted video (a camera switched off is muted, not unpublished) has no tile: it would only be a black box.
        for (const t of this.tracks.values()) if (t.track.kind === 'video' && !t.pub.isMuted) shares.push({ key: t.identity + '/' + t.pub.trackSid, sid: t.pub.trackSid, identity: t.identity, name: (this.members.get(t.identity) || {}).name || t.identity.slice(0, 8), source: t.source, movie: t.pub.trackName === 'movie', track: t.track, quality: this.qualityOf(t.pub.trackSid), local: false });
        const lp = this.room.localParticipant;
        for (const src of [LK.Track.Source.ScreenShare, LK.Track.Source.Camera]) { const pub = lp.getTrackPublication(src); if (pub && pub.track && !pub.isMuted) shares.push({ key: 'me/' + pub.trackSid, sid: pub.trackSid, identity: this.me, name: 'You', source: src, movie: pub.trackName === 'movie', track: pub.track, quality: 'auto', local: true }); }
      }
      return {
        phase: this.phase, error: this.error, gid: this.gid, roomName: this.roomName, muted: this.muted, gameMuted: this.gameMuted, gameName: this.gameName, gameError: this.gameError, deafened: this.deafened, ptt: this.prefs.ptt, pttKey: this.prefs.pttKey, pttHeld: this.pttHeld,
        sharing: this.sharing, camera: this.camera, playing: !!this.play, playback: this.playbackState(), peers, shares, prefs: this.prefs, attempt: this.attempt,
        encryption: { active: !!(this.room && this.room.isE2EEEnabled), backend: describeSupport().e2ee, epoch: this.keyState && this.keyState.epoch, keyIndex: this.keyState && this.keyState.index, rotations: this.rotations, lastRotation: this.lastRotation, devices: this.keyState && this.keyState.devices },
        blocked: [...this.blocked.values()], events: this.events.slice(-12),
      };
    }

    // test hook: measure what a remote participant's audio really sounds like (spectral share of a known tone)
    async probeTone(identity, hz, ms = 1500, source = 'microphone') {
      const t = [...this.tracks.values()].find(x => x.identity === identity && x.track.kind === 'audio' && x.source === source); if (!t) return { present: false };
      const ctx = (() => { try { return new AudioContext({ sinkId: { type: 'none' } }); } catch { return new AudioContext(); } })(); await ctx.resume();   // 'none': measure without any audio device
      const src = ctx.createMediaStreamSource(new MediaStream([t.track.mediaStreamTrack])); const an = ctx.createAnalyser(); an.fftSize = 8192; src.connect(an);
      const buf = new Float32Array(an.frequencyBinCount); let best = 0, rmsMax = 0; const end = Date.now() + ms;
      while (Date.now() < end) {
        await sleep(100); an.getFloatFrequencyData(buf);
        const bin = Math.round(hz / (ctx.sampleRate / an.fftSize)); let peak = -200, tot = 0, n = 0;
        for (let i = 0; i < buf.length; i++) { const p = Math.pow(10, buf[i] / 10); tot += p; n++; if (Math.abs(i - bin) <= 2 && buf[i] > peak) peak = buf[i]; }
        const share = Math.pow(10, peak / 10) / (tot || 1); if (share > best) best = share;
        const td = new Float32Array(an.fftSize); an.getFloatTimeDomainData(td); let s = 0; for (const v of td) s += v * v; rmsMax = Math.max(rmsMax, Math.sqrt(s / td.length));
      }
      await ctx.close();
      return { present: true, toneShare: best, rms: rmsMax };
    }
  }

  window.CommonsCall = Call;
  window.CommonsCallError = CallError;
})();
