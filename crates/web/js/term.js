// xterm.js 的薄封装：一个终端连一条 hub 的 /terminal/ws。xterm 和 fit 插件由 index.html 以全局脚本引入。
// 远端是 tmux 会话：断线重连会回到原处。

export class Term {
  constructor(host, url, onState) {
    this.onState = onState;
    this.term = new window.Terminal({
      fontSize: 12.5, cursorBlink: true, scrollback: 10000, allowTransparency: true,
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
    this.fit = new window.FitAddon.FitAddon();
    this.term.loadAddon(this.fit);
    host.innerHTML = '';
    this.term.open(host);
    this.safeFit();
    const sep = url.includes('?') ? '&' : '?';
    const sock = new WebSocket(`${url}${sep}cols=${this.term.cols}&rows=${this.term.rows}`);
    sock.binaryType = 'arraybuffer';
    this.sock = sock;
    sock.onopen = () => onState('open');
    sock.onmessage = e => this.term.write(e.data instanceof ArrayBuffer ? new Uint8Array(e.data) : e.data);
    sock.onclose = () => {
      if (this.disposed) return;
      this.term.write('\r\n\x1b[90m[已断开 —— 远端会话仍在运行，点「重连」回到原处]\x1b[0m\r\n');
      onState('closed');
    };
    this.term.onData(d => { if (sock.readyState === 1) sock.send(JSON.stringify({ type: 'input', data: d })); });
    // 面板拖动、收起展开、窗口缩放都会改尺寸：盯着宿主节点本身。
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
