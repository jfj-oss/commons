'use strict';
// Who decides whether the mic is on. Two voices: the person (manual) and a game key (game). Either one can keep it off, and
// neither can overrule the other's "off": a game key letting go never unmutes someone who muted themselves.
// Changes go out one at a time, and only the latest wish counts, so a fast press and release cannot end on the wrong state.
(function (root) {
  class MicGate {
    constructor(publish) { this.publish = publish; this.manual = false; this.game = false; this.sent = null; this.busy = null; this.ready = false; }
    get off() { return this.manual || this.game; }
    setManual(m) { this.manual = !!m; return this.flush(); }
    setGame(g) { this.game = !!g; return this.flush(); }
    // The helper or the app went away: nothing is known about the keys, so they hold nothing.
    lost() { return this.setGame(false); }
    // There is a mic to talk to from now on (joined), or not any more (left).
    open() { this.ready = true; this.sent = null; return this.flush(); }
    close() { this.ready = false; this.game = false; this.sent = null; }
    flush() {
      if (this.busy) return this.busy;
      const run = async () => {
        while (this.ready && this.sent !== this.off) {
          const want = this.off; this.sent = want;
          try { await this.publish(!want); } catch (e) { this.sent = null; throw e; }
        }
      };
      this.busy = run().finally(() => { this.busy = null; if (this.ready && this.sent !== null && this.sent !== this.off) return this.flush(); });
      return this.busy;
    }
  }
  root.CommonsMicGate = MicGate;
})(typeof window !== 'undefined' ? window : globalThis);
