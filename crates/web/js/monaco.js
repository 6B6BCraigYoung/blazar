window.addEventListener('unhandledrejection', event => {
  const reason = event.reason;
  if (reason && reason.name === 'Canceled' && reason.message === 'Canceled') event.preventDefault();
});

let ready = null;

// 编辑器的底色、前景、选区、光标从 CSS 变量取（--bg-code、--code-*），和代码块、终端同一组；
// 换主题时重新读一遍。Monaco 只认十六进制，所以借一个隐藏元素把任意 CSS 颜色折成 #rrggbbaa。
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
  const c = getComputedStyle(probe).color;
  const n = (c.match(/-?[\d.]+/g) || []).map(Number);
  if (n.length < 3) return fallback;
  const [r, g, b] = c.startsWith('color(') ? n.slice(0, 3).map(v => v * 255) : n.slice(0, 3);
  const a = n.length > 3 ? n[3] : 1;
  const hex = v => Math.round(Math.min(255, Math.max(0, v))).toString(16).padStart(2, '0');
  return '#' + hex(r) + hex(g) + hex(b) + (a < 1 ? hex(a * 255) : '');
}
function fade(color, k) {
  if (color.length !== 9) return color + hexAlpha(k);
  return color.slice(0, 7) + hexAlpha(parseInt(color.slice(7), 16) / 255 * k);
}
function hexAlpha(a) { return Math.round(Math.min(1, Math.max(0, a)) * 255).toString(16).padStart(2, '0'); }

// 语法色压到中等饱和度，跟界面的灰阶和终端的柔和配色放在一起；两套主题同一个色相分配。
const SYNTAX = {
  dark: { comment: '7d828a', keyword: 'c49bd6', number: 'd6bd8a', string: 'a8c79a', escape: 'd9a07a', type: '8fb8d8', tag: 'd9a07a', attr: 'b8a4d9', delimiter: '9a9aa0' },
  light: { comment: '6a6e73', keyword: '8a4a9e', number: '8a6116', string: '3f7a3a', escape: '9a5426', type: '2f5f8f', tag: '9a5426', attr: '6a4f9a', delimiter: '4a4a4e' },
};

function themeData(dark) {
  const fallbackBg = dark ? '#202023' : '#f6f6f7';
  const bg = cssColor('--bg-code', fallbackBg).slice(0, 7);
  const fg = cssColor('--code-fg', dark ? '#d1d1d6' : '#1d1d1f').slice(0, 7);
  const sel = cssColor('--code-selection', dark ? '#0a84ff59' : '#007aff38');
  const syn = SYNTAX[dark ? 'dark' : 'light'];
  const ink = (dark ? '#ffffff' : '#000000');
  const rule = (token, foreground, fontStyle) => (fontStyle ? { token, foreground, fontStyle } : { token, foreground });
  return {
    base: dark ? 'vs-dark' : 'vs',
    inherit: true,
    rules: [
      rule('', fg.slice(1)),
      rule('comment', syn.comment, 'italic'),
      rule('keyword', syn.keyword),
      rule('number', syn.number),
      rule('string', syn.string),
      rule('string.escape', syn.escape),
      rule('regexp', syn.escape),
      rule('type', syn.type),
      rule('type.identifier', syn.type),
      rule('tag', syn.tag),
      rule('annotation', syn.tag),
      rule('attribute.name', syn.attr),
      rule('attribute.value', syn.string),
      rule('key', syn.type),
      rule('variable', fg.slice(1)),
      rule('delimiter', syn.delimiter),
    ],
    colors: {
      'editor.background': bg,
      'editor.foreground': fg,
      'editorGutter.background': bg,
      'minimap.background': bg,
      'editorStickyScroll.background': bg,
      'editorLineNumber.foreground': cssColor('--code-line-number', dark ? '#6a6a70' : '#aeaeb2'),
      'editorLineNumber.activeForeground': cssColor('--code-line-number-active', dark ? '#98989f' : '#6e6e73'),
      'editor.lineHighlightBackground': ink + '0a',
      'editor.lineHighlightBorder': '#00000000',
      'editor.selectionBackground': sel,
      'editor.inactiveSelectionBackground': fade(sel, .5),
      'editorCursor.foreground': cssColor('--code-cursor', fg),
      'editorIndentGuide.background1': ink + (dark ? '12' : '0f'),
      'editorIndentGuide.activeBackground1': ink + (dark ? '2a' : '24'),
      'editorWidget.background': cssColor('--bg-overlay', dark ? '#2a2a2d' : '#ffffff').slice(0, 7),
      'editorWidget.border': cssColor('--border', ink + '1f'),
      'scrollbar.shadow': '#00000000',
      'scrollbarSlider.background': ink + '1f',
      'scrollbarSlider.hoverBackground': ink + '33',
      'scrollbarSlider.activeBackground': ink + '47',
    },
  };
}

// 只定义当前这一套：CSS 变量此刻就是这套主题的值。切主题时再定义另一套。
function defineThemes(m) {
  const name = editorTheme();
  m.editor.defineTheme(name, themeData(name === 'blazar-dark'));
}

function load() {
  if (ready) return ready;
  ready = new Promise((resolve, reject) => {
    window.require.config({ paths: { vs: '/vendor/vs' } });
    window.require(['vs/editor/editor.main'], () => {
      const m = window.monaco;
      // 对话里的代码块只走 colorize、不一定开着编辑器，所以主题在这里全局跟着切。
      const retheme = () => { defineThemes(m); m.editor.setTheme(editorTheme()); };
      retheme();
      new MutationObserver(retheme).observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
      matchMedia('(prefers-color-scheme: dark)').addEventListener('change', retheme);
      resolve(m);
    }, reject);
  });
  return ready;
}

const LANG = { rs: 'rust', ts: 'typescript', tsx: 'typescript', js: 'javascript', jsx: 'javascript', mjs: 'javascript',
  py: 'python', go: 'go', java: 'java', c: 'c', h: 'c', cpp: 'cpp', hpp: 'cpp', cc: 'cpp', cu: 'cpp', json: 'json',
  toml: 'ini', ini: 'ini', yaml: 'yaml', yml: 'yaml', md: 'markdown', html: 'html', css: 'css', sh: 'shell',
  bash: 'shell', zsh: 'shell', sql: 'sql', lua: 'lua', rb: 'ruby', php: 'php', swift: 'swift', kt: 'kotlin',
  vue: 'html', xml: 'xml' };
export function langOf(path) {
  return LANG[(path.split('.').pop() || '').toLowerCase()] || 'plaintext';
}

function preferences() {
  const defaults = { fontSize: 13, minimap: false, wordWrap: true };
  try {
    const saved = JSON.parse(localStorage.getItem('blazar.ui') || '{}') || {};
    if (saved.v !== 2) { delete saved.minimap; delete saved.wordWrap; }
    const prefs = { ...defaults, ...saved };
    prefs.fontSize = Math.min(18, Math.max(11, Math.round(Number(prefs.fontSize) || defaults.fontSize)));
    return prefs;
  } catch (_) { return defaults; }
}
function editorTheme() {
  const selected = document.documentElement.dataset.theme;
  return (selected === 'light' || ((!selected || selected === 'system') && !matchMedia('(prefers-color-scheme: dark)').matches)) ? 'blazar-light' : 'blazar-dark';
}

export class Editor {
  constructor(m, host, onSave, onDirty) {
    this.m = m;
    this.models = new Map();
    this.onDirty = onDirty;
    const prefs = preferences();
    this.ed = m.editor.create(host, {
      value: '', language: 'plaintext', theme: editorTheme(), automaticLayout: true,
      fontSize: prefs.fontSize, wordWrap: prefs.wordWrap ? 'on' : 'off', wrappingIndent: 'same', lineHeight: 22, fontFamily: '"JetBrains Mono", "SF Mono", Menlo, monospace',
      minimap: { enabled: !!prefs.minimap }, scrollBeyondLastLine: false, renderWhitespace: 'selection',
      padding: { top: 12, bottom: 12 }, smoothScrolling: !matchMedia('(prefers-reduced-motion: reduce)').matches, mouseWheelScrollSensitivity: 1,
      scrollbar: { horizontalScrollbarSize: 10, verticalScrollbarSize: 10, useShadows: false },
      overviewRulerBorder: false, hideCursorInOverviewRuler: true, renderLineHighlight: 'line', renderLineHighlightOnlyWhenFocus: true,
      guides: { indentation: true },
    });
    this.updatePrefs = () => {
      const p = preferences();
      this.ed.updateOptions({ fontSize: p.fontSize, minimap: { enabled: !!p.minimap }, wordWrap: p.wordWrap ? 'on' : 'off' });
      defineThemes(m);
      m.editor.setTheme(editorTheme());
    };
    this.host = host;
    this.sizeObserver = new ResizeObserver(() => this.fit());
    this.sizeObserver.observe(host);
    this.themeObserver = new MutationObserver(this.updatePrefs);
    this.themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    this.systemTheme = matchMedia('(prefers-color-scheme: dark)');
    this.systemTheme.addEventListener('change', this.updatePrefs);
    window.addEventListener('blazar:preferences', this.updatePrefs);
    window.addEventListener('storage', this.updatePrefs);
    this.ed.addCommand(m.KeyMod.CtrlCmd | m.KeyCode.KeyS, () => {
      const p = this.current();
      if (p) onSave(p);
    });
  }

  fit() {
    const { width, height } = this.host.getBoundingClientRect();
    if (width > 0 && height > 0) this.ed.layout({ width, height });
  }

  current() {
    const cur = this.ed.getModel();
    for (const [p, e] of this.models) if (e.model === cur) return p;
    return null;
  }

  has(path) { return this.models.has(path); }

  open(path, text, readonly, line) {
    let e = this.models.get(path);
    if (!e) {
      const model = this.m.editor.createModel(text, readonly ? 'plaintext' : langOf(path));
      e = { model, saved: model.getAlternativeVersionId(), dirty: false };

      model.onDidChangeContent(() => {
        e.dirty = model.getAlternativeVersionId() !== e.saved;
        this.onDirty(path, e.dirty);
      });
      this.models.set(path, e);
    }
    this.ed.setModel(e.model);
    this.ed.updateOptions({ readOnly: !!readonly });
    this.fit();
    if (line > 0) {
      this.ed.revealLineInCenter(line);
      this.ed.setPosition({ lineNumber: line, column: 1 });
    }
  }

  value(path) { return this.models.get(path)?.model.getValue() ?? ''; }

  replace(path, text) {
    const e = this.models.get(path);
    if (!e || e.model.getValue() === text) { if (e) this.markSaved(path); return; }
    const view = this.ed.getModel() === e.model ? this.ed.saveViewState() : null;
    e.model.setValue(text);
    if (view) this.ed.restoreViewState(view);
    this.markSaved(path);
  }

  // 放回本地存着的草稿：保留“已保存”的基准，所以它显示为没保存；撤销能回到磁盘上的版本。
  restoreDraft(path, text) {
    const e = this.models.get(path);
    if (!e) return false;
    if (e.model.getValue() !== text) {
      e.model.pushEditOperations([], [{ range: e.model.getFullModelRange(), text }], () => null);
    }
    return true;
  }

  markSaved(path) {
    const e = this.models.get(path);
    if (!e) return;
    e.saved = e.model.getAlternativeVersionId();
    if (e.dirty) { e.dirty = false; this.onDirty(path, false); }
  }

  close(path) {
    const e = this.models.get(path);
    if (!e) return;
    if (this.ed.getModel() === e.model) this.ed.setModel(null);
    e.model.dispose();
    this.models.delete(path);
  }

  clear() { this.ed.setModel(null); }

  focus() { this.ed.focus(); }

  dispose() {
    this.sizeObserver.disconnect();
    this.themeObserver.disconnect();
    this.systemTheme.removeEventListener('change', this.updatePrefs);
    window.removeEventListener('blazar:preferences', this.updatePrefs);
    window.removeEventListener('storage', this.updatePrefs);
    for (const e of this.models.values()) e.model.dispose();
    this.models.clear();
    this.ed.dispose();
  }
}

export async function createEditor(host, onSave, onDirty) {
  const m = await load();
  return new Editor(m, host, onSave, onDirty);
}

const colorized = new Map();
const COLORIZE_CACHE = 400;

export async function colorize(text, lang) {
  const key = lang + '\0' + text;
  const hit = colorized.get(key);
  if (hit !== undefined) return hit;
  const m = await load();
  const html = await m.editor.colorize(text, lang, { tabSize: 2 });
  if (colorized.size >= COLORIZE_CACHE) colorized.delete(colorized.keys().next().value);
  colorized.set(key, html);
  return html;
}
