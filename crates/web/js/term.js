


export class Term {
  constructor(host, url, onState) {
    this.onState = onState;
    this.term = new window.Terminal({
      fontSize: 12.5, cursorBlink: true, scrollback: 10000, allowTransparency: true,
      macOptionClickForcesSelection: true,
      fontFamily: '"JetBrains Mono", Menlo, monospace',
      theme: {
        background: '#101011', foreground: '#c8c8cc', cursor: '#a1a1aa', cursorAccent: '#101011',
        selectionBackground: '#2f3d52',
        black: '#1b1b1d', brightBlack: '#5c5c64',
        red: '#c98a80', brightRed: '#d9a59c',
        green: '#8fb59a', brightGreen: '#a8c8b0',
        yellow: '#c9b48a', brightYellow: '#d8c7a3',
        blue: '#8fa3c2', brightBlue: '#a9badb',
        magenta: '#a99bbf', brightMagenta: '#bfb3d1',
        cyan: '#8bb0b3', brightCyan: '#a5c4c6',
        white: '#c8c8cc', brightWhite: '#e4e4e7',
      },
    });
    this.term.parser.registerOscHandler(52, data => {
      const i = data.indexOf(';');
      const b64 = i < 0 ? '' : data.slice(i + 1);
      if (!b64 || b64 === '?') return true;
      try {
        const bytes = Uint8Array.from(atob(b64), c => c.charCodeAt(0));
        navigator.clipboard?.writeText(new TextDecoder().decode(bytes)).catch(() => {});
      } catch (_) {}
      return true;
    });
    this.fit = new window.FitAddon.FitAddon();
    this.term.loadAddon(this.fit);
    host.innerHTML = '';
    this.term.open(host);
    this.safeFit();
    const sep = url.includes('?') ? '&' : '?';
    const sock = new WebSocket(`${url}${sep}cols=${this.term.cols}&rows=${this.term.rows}`);
    sock.binaryType = 'arraybuffer';
    this.sock = sock;
    sock.onopen = () => { onState('open'); this.resize(); };
    sock.onmessage = e => this.term.write(e.data instanceof ArrayBuffer ? new Uint8Array(e.data) : e.data);
    sock.onclose = () => {
      if (this.disposed) return;
      this.term.write(url.includes('/login/') ? '\r\n\x1b[90m[登录流程已结束]\x1b[0m\r\n' : '\r\n\x1b[90m[已断开 —— 远端会话仍在运行，点「重连」回到原处]\x1b[0m\r\n');
      onState('closed');
    };
    this.term.onData(d => { if (sock.readyState === 1) sock.send(JSON.stringify({ type: 'input', data: d })); });

    this.ro = new ResizeObserver(() => this.resize());
    this.ro.observe(host);
  }

  safeFit() {
    try { this.fit.fit(); return true; } catch (_) { return false; }
  }

  resize() {
    if (this.disposed || !this.safeFit()) return;
    if (this.sock.readyState === 1) {
      this.sock.send(JSON.stringify({ type: 'resize', cols: this.term.cols, rows: this.term.rows }));
    }
  }

  focus() { this.term.focus(); }

  dispose() {
    this.disposed = true;
    this.ro.disconnect();
    try { this.sock.close(); } catch (_) {}
    this.term.dispose();
  }
}

export function openTerm(host, url, onState) {
  return new Term(host, url, onState);
}
