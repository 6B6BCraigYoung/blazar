const START3 = [0, 0, 1];

function nalType(b) { return b & 0x1f; }

function splitNals(buf) {
  const out = [];
  let i = 0, start = -1;
  const n = buf.length;
  while (i + 2 < n) {
    if (buf[i] === 0 && buf[i + 1] === 0 && buf[i + 2] === 1) {
      if (start >= 0) {
        let end = i;
        if (end > start && buf[end - 1] === 0) end--;
        out.push(buf.subarray(start, end));
      }
      i += 3;
      start = i;
      continue;
    }
    i++;
  }
  return { nals: out, rest: start >= 0 ? buf.subarray(start) : buf };
}

function withLength(nals) {
  let total = 0;
  for (const nal of nals) total += 4 + nal.length;
  const out = new Uint8Array(total);
  let o = 0;
  for (const nal of nals) {
    out[o++] = (nal.length >>> 24) & 0xff;
    out[o++] = (nal.length >>> 16) & 0xff;
    out[o++] = (nal.length >>> 8) & 0xff;
    out[o++] = nal.length & 0xff;
    out.set(nal, o);
    o += nal.length;
  }
  return out;
}

function avcc(sps, pps) {
  const out = new Uint8Array(11 + sps.length + pps.length);
  out.set([1, sps[1], sps[2], sps[3], 0xff, 0xe1, sps.length >>> 8, sps.length & 0xff], 0);
  out.set(sps, 8);
  let o = 8 + sps.length;
  out[o++] = 1;
  out[o++] = pps.length >>> 8;
  out[o++] = pps.length & 0xff;
  out.set(pps, o);
  return out;
}

function same(a, b) {
  if (!a || !b || a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

export class Stream {
  constructor(canvas, url, onState) {
    this.canvas = canvas;
    this.ctx = canvas.getContext('2d', { alpha: false, desynchronized: true });
    this.onState = onState;
    this.disposed = false;
    this.pending = new Uint8Array(0);
    this.au = [];
    this.sps = null;
    this.pps = null;
    this.configured = false;
    this.waitKey = true;
    this.frames = 0;
    this.bytes = 0;
    this.lastTick = performance.now();
    this.decoder = null;
    if (typeof VideoDecoder === 'undefined') {
      onState('error:这个浏览器不支持 WebCodecs 视频解码');
      return;
    }
    const sock = new WebSocket(url);
    sock.binaryType = 'arraybuffer';
    this.sock = sock;
    sock.onopen = () => onState('open');
    sock.onmessage = e => {
      if (typeof e.data === 'string') {
        let m = {};
        try { m = JSON.parse(e.data); } catch (_) { return; }
        if (m.error) onState('error:' + m.error);
        else if (m.ready) onState('ready');
        else if (m.ended) onState('ended:' + (m.stderr || ''));
        return;
      }
      this.feed(new Uint8Array(e.data));
    };
    sock.onclose = () => { if (!this.disposed) onState('closed'); };
    sock.onerror = () => { if (!this.disposed) onState('error:连接失败'); };
    this.timer = setInterval(() => this.tick(), 1000);
  }

  tick() {
    const now = performance.now();
    const dt = (now - this.lastTick) / 1000;
    if (dt <= 0) return;
    const fps = this.frames / dt;
    const kbps = (this.bytes * 8) / dt / 1000;
    this.frames = 0; this.bytes = 0; this.lastTick = now;
    if (this.configured) this.onState(`stats:${fps.toFixed(0)} fps · ${(kbps / 1000).toFixed(1)} Mbps`);
  }

  feed(chunk) {
    this.bytes += chunk.length;
    let buf;
    if (this.pending.length) {
      buf = new Uint8Array(this.pending.length + chunk.length);
      buf.set(this.pending, 0);
      buf.set(chunk, this.pending.length);
    } else {
      buf = chunk;
    }
    const { nals, rest } = splitNals(buf);
    this.pending = rest.slice();
    for (const nal of nals) this.push(nal);
  }

  push(nal) {
    if (!nal.length) return;
    const t = nalType(nal[0]);
    const slice = t === 1 || t === 5;
    const firstMb = slice && nal.length > 1 && (nal[1] & 0x80) !== 0;
    if (this.au.length && (!slice || firstMb) && this.au.some(x => { const k = nalType(x[0]); return k === 1 || k === 5; })) {
      this.flush();
    }
    if (t === 7) { this.sps = nal; return; }
    if (t === 8) { this.pps = nal; return; }
    if (t === 9 || t === 6) return;
    this.au.push(nal);
  }

  flush() {
    const au = this.au;
    this.au = [];
    if (!this.sps || !this.pps) return;
    const key = au.some(x => nalType(x[0]) === 5);
    if (!this.configured || !same(this.cfgSps, this.sps) || !same(this.cfgPps, this.pps)) {
      if (!key) return;
      this.configure();
    }
    if (this.waitKey && !key) return;
    this.waitKey = false;
    if (this.decoder.decodeQueueSize > 6 && !key) { this.waitKey = true; return; }
    try {
      this.decoder.decode(new EncodedVideoChunk({ type: key ? 'key' : 'delta', timestamp: this.frames * 1000, data: withLength(au) }));
    } catch (e) {
      this.onState('error:' + (e && e.message ? e.message : '解码失败'));
    }
  }

  configure() {
    const profile = this.sps[1].toString(16).padStart(2, '0') + this.sps[2].toString(16).padStart(2, '0') + this.sps[3].toString(16).padStart(2, '0');
    if (this.decoder) { try { this.decoder.close(); } catch (_) {} }
    this.decoder = new VideoDecoder({
      output: frame => this.draw(frame),
      error: e => this.onState('error:' + (e && e.message ? e.message : '解码器出错')),
    });
    this.decoder.configure({ codec: 'avc1.' + profile, description: avcc(this.sps, this.pps), optimizeForLatency: true });
    this.cfgSps = this.sps; this.cfgPps = this.pps;
    this.configured = true;
    this.waitKey = true;
    this.onState('playing');
  }

  draw(frame) {
    try {
      const w = frame.displayWidth, h = frame.displayHeight;
      if (this.canvas.width !== w || this.canvas.height !== h) { this.canvas.width = w; this.canvas.height = h; }
      this.ctx.drawImage(frame, 0, 0, w, h);
      this.frames++;
    } finally {
      frame.close();
    }
  }

  snapshot() {
    const a = document.createElement('a');
    a.href = this.canvas.toDataURL('image/png');
    a.download = 'blazar-' + new Date().toISOString().replace(/[:.]/g, '-') + '.png';
    a.click();
  }

  fullscreen() {
    const el = this.canvas;
    if (el.requestFullscreen) el.requestFullscreen().catch(() => {});
    else if (el.webkitRequestFullscreen) el.webkitRequestFullscreen();
  }

  dispose() {
    this.disposed = true;
    clearInterval(this.timer);
    try { this.sock && this.sock.close(); } catch (_) {}
    try { this.decoder && this.decoder.close(); } catch (_) {}
  }
}

export function openStream(canvas, url, onState) {
  return new Stream(canvas, url, onState);
}
