'use strict';
// Profile pictures, prepared on this device before anything is sent: decode (with the photo's EXIF orientation applied,
// so a phone picture is the right way up), crop with drag and zoom, then shrink to a 256x256 image under 40 KB.
// The core encrypts the resulting bytes like an attachment; the relay never sees the picture.
(function () {
  const SIZE = 256;
  const MAX_BYTES = 40 * 1024;

  /// Decode an image file the way it is meant to be seen. Browsers apply EXIF orientation to an <img> by default, and
  /// createImageBitmap does it when asked; the fallback exists for older web views.
  async function decode(file) {
    if (!file || !/^image\//.test(file.type || '')) throw new Error('That file is not a picture.');
    // An <img> applies the EXIF orientation by default in every web view Megatron runs in, so a phone photo is
    // already the right way up by the time it reaches the canvas.
    const url = URL.createObjectURL(file);
    try {
      const img = new Image();
      img.src = url;
      if (img.decode) await img.decode(); else await new Promise((res, rej) => { img.onload = res; img.onerror = () => rej(new Error('That file is not a picture.')); });
      if (img.naturalWidth && img.naturalHeight) return img;
    } catch { }
    finally { setTimeout(() => URL.revokeObjectURL(url), 1000); }
    // Older web views: the bitmap API, asked explicitly for the picture as it is meant to be seen.
    if (window.createImageBitmap) {
      try { const b = await createImageBitmap(file, { imageOrientation: 'from-image' }); if (b.width && b.height) return b; } catch { }
      try { const b = await createImageBitmap(file); if (b.width && b.height) return b; } catch { }
    }
    throw new Error('That file is not a picture.');
  }

  const toBlob = (canvas, type, q) => new Promise(res => {
    try { canvas.toBlob(b => res(b), type, q); } catch { res(null); }
  });

  /// The cropper: a square 256x256 stage where the picture can be dragged and zoomed. It always covers the stage, so
  /// there are no empty corners to send.
  class Cropper {
    constructor(canvas, source) {
      this.canvas = canvas;
      this.source = source;
      this.w = source.width || source.naturalWidth;
      this.h = source.height || source.naturalHeight;
      this.base = SIZE / Math.min(this.w, this.h);
      this.zoom = 1;
      this.x = 0;
      this.y = 0;
      this.shape = 'circle';
      canvas.width = SIZE; canvas.height = SIZE;
      this._drag = null;
      canvas.addEventListener('pointerdown', e => this._down(e));
      canvas.addEventListener('pointermove', e => this._move(e));
      canvas.addEventListener('pointerup', e => this._up(e));
      canvas.addEventListener('pointercancel', e => this._up(e));
      canvas.addEventListener('wheel', e => { e.preventDefault(); this.setZoom(this.zoom * (e.deltaY < 0 ? 1.1 : 1 / 1.1)); }, { passive: false });
      this.render();
    }
    setShape(shape) { this.shape = shape === 'square' ? 'square' : 'circle'; this.render(); }
    setZoom(z) {
      this.zoom = Math.max(1, Math.min(4, Number(z) || 1));
      this._clamp(); this.render();
    }
    _scaled() { return [this.w * this.base * this.zoom, this.h * this.base * this.zoom]; }
    _clamp() {
      const [sw, sh] = this._scaled();
      const mx = Math.max(0, (sw - SIZE) / 2), my = Math.max(0, (sh - SIZE) / 2);
      this.x = Math.max(-mx, Math.min(mx, this.x));
      this.y = Math.max(-my, Math.min(my, this.y));
    }
    _down(e) {
      if (e.button != null && e.button !== 0) return;
      this._drag = { id: e.pointerId, sx: e.clientX, sy: e.clientY, x: this.x, y: this.y };
      try { this.canvas.setPointerCapture(e.pointerId); } catch { }
      e.preventDefault();
    }
    _move(e) {
      if (!this._drag || e.pointerId !== this._drag.id) return;
      const r = this.canvas.getBoundingClientRect();
      const k = SIZE / (r.width || SIZE);
      this.x = this._drag.x + (e.clientX - this._drag.sx) * k;
      this.y = this._drag.y + (e.clientY - this._drag.sy) * k;
      this._clamp(); this.render();
    }
    _up(e) { if (this._drag && e.pointerId === this._drag.id) this._drag = null; }
    _draw(g) {
      const [sw, sh] = this._scaled();
      g.drawImage(this.source, (SIZE - sw) / 2 + this.x, (SIZE - sh) / 2 + this.y, sw, sh);
    }
    render() {
      const g = this.canvas.getContext('2d');
      g.clearRect(0, 0, SIZE, SIZE);
      this._draw(g);
      if (this.shape === 'circle') {
        g.globalCompositeOperation = 'destination-in';
        g.beginPath(); g.arc(SIZE / 2, SIZE / 2, SIZE / 2, 0, Math.PI * 2); g.fill();
        g.globalCompositeOperation = 'source-over';
      }
    }
    /// The square picture, before the chosen frame is applied. Encoding starts from this.
    output() {
      const c = document.createElement('canvas'); c.width = SIZE; c.height = SIZE;
      this._draw(c.getContext('2d'));
      return c;
    }
  }

  function framed(square, shape) {
    if (shape !== 'circle') return square;
    const c = document.createElement('canvas'); c.width = c.height = SIZE;
    const g = c.getContext('2d');
    g.drawImage(square, 0, 0);
    g.globalCompositeOperation = 'destination-in';
    g.beginPath(); g.arc(SIZE / 2, SIZE / 2, SIZE / 2, 0, Math.PI * 2); g.fill();
    return c;
  }

  /// Shrink to WebP (transparent outside the circle) or, where the web view cannot encode WebP, JPEG. Smaller sizes are
  /// tried as a last resort so a busy photo still fits in 40 KB.
  async function encode(square, shape) {
    const circle = shape === 'circle';
    const webp = circle ? framed(square, 'circle') : square;
    for (const q of [0.85, 0.75, 0.65, 0.55, 0.45]) {
      const b = await toBlob(webp, 'image/webp', q);
      if (b && b.type === 'image/webp' && b.size <= MAX_BYTES) return { blob: b, shape };
    }
    for (const q of [0.85, 0.7, 0.55]) {
      const b = await toBlob(square, 'image/jpeg', q);
      if (b && b.type === 'image/jpeg' && b.size <= MAX_BYTES) return { blob: b, shape };
    }
    for (const s of [192, 160, 128]) {
      const small = document.createElement('canvas'); small.width = small.height = s;
      small.getContext('2d').drawImage(square, 0, 0, s, s);
      for (const q of [0.7, 0.5]) {
        const b = await toBlob(small, 'image/jpeg', q);
        if (b && b.type === 'image/jpeg' && b.size <= MAX_BYTES) return { blob: b, shape };
      }
    }
    // A web view that can encode neither format still gets a tiny picture rather than a refusal.
    for (const s of [128, 96, 64]) {
      const small = document.createElement('canvas'); small.width = small.height = s;
      small.getContext('2d').drawImage(square, 0, 0, s, s);
      const b = await toBlob(small, 'image/png');
      if (b && b.size <= MAX_BYTES) return { blob: b, shape };
    }
    throw new Error('This picture could not be shrunk below 40 KB. Try another one.');
  }

  window.CommonsAvatar = { decode, Cropper, encode, SIZE, MAX_BYTES };
})();
