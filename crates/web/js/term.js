// ANSI 色两套主题同一个思路：中等偏低饱和，色相一致；深色取浅一些的柔和色，浅色取同色相的深色，
// 都在各自的底上保证可读。底色、前景、光标、选区从 CSS 变量取（--terminal-bg、--code-*），和编辑器、代码块同一组。
const DARK = {
  black: '#1b1b1d', brightBlack: '#7c7c82',
  red: '#c98a80', brightRed: '#d9a59c',
  green: '#8fb59a', brightGreen: '#a8c8b0',
  yellow: '#c9b48a', brightYellow: '#d8c7a3',
  blue: '#8fa3c2', brightBlue: '#a9badb',
  magenta: '#a99bbf', brightMagenta: '#bfb3d1',
  cyan: '#8bb0b3', brightCyan: '#a5c4c6',
  white: '#c8c8cc', brightWhite: '#e4e4e7',
};

const LIGHT = {
  black: '#1d1d1f', brightBlack: '#6e6e73',
  red: '#9e5148', brightRed: '#b0645a',
  green: '#4a7656', brightGreen: '#5a8766',
  yellow: '#85692f', brightYellow: '#977a3c',
  blue: '#4d6a99', brightBlue: '#5f7cab',
  magenta: '#7a5f99', brightMagenta: '#8b70aa',
  cyan: '#3f7a7e', brightCyan: '#4f8b8f',
  white: '#6e6e73', brightWhite: '#8e8e93',
};

let probe = null;
function cssColor(name, fallback) {
  const raw = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  if (!raw || !document.body) return fallback;
  if (!probe) {
    probe = document.createElement('span');
    probe.style.display = 'none';
    document.body.appendChild(probe);
  }
  probe.style.color = '';
  probe.style.color = raw;
  return getComputedStyle(probe).color || fallback;
}

function theme() {
  const selected = document.documentElement.dataset.theme;
  const light = selected === 'light' || ((!selected || selected === 'system') && !matchMedia('(prefers-color-scheme: dark)').matches);
  const background = cssColor('--terminal-bg', light ? '#f6f6f7' : '#202023');
  return {
    ...(light ? LIGHT : DARK),
    background,
    foreground: cssColor('--code-fg', light ? '#1d1d1f' : '#d1d1d6'),
    cursor: cssColor('--code-cursor', light ? '#3a3a3c' : '#c7c7cc'),
    cursorAccent: background,
    selectionBackground: cssColor('--code-selection', light ? 'rgba(0,122,255,.22)' : 'rgba(10,132,255,.35)'),
  };
}

export class Term {
  constructor(host, url, onState) {
    this.onState = onState;
    this.term = new window.Terminal({
      fontSize: 12.5, cursorBlink: true, scrollback: 10000, allowTransparency: true,
      macOptionClickForcesSelection: true,
      fontFamily: '"JetBrains Mono", Menlo, monospace',
      theme: theme(),
    });
    this.retheme = () => { this.term.options.theme = theme(); };
    this.themeObserver = new MutationObserver(this.retheme);
    this.themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    this.systemTheme = matchMedia('(prefers-color-scheme: dark)');
    this.systemTheme.addEventListener('change', this.retheme);
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
    this.themeObserver.disconnect();
    this.systemTheme.removeEventListener('change', this.retheme);
    try { this.sock.close(); } catch (_) {}
    this.term.dispose();
  }
}

export function openTerm(host, url, onState) {
  return new Term(host, url, onState);
}
