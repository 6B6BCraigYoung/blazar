


let ready = null;

function load() {
  if (ready) return ready;
  ready = new Promise((resolve, reject) => {
    window.require.config({ paths: { vs: '/vendor/vs' } });
    window.require(['vs/editor/editor.main'], () => {
      const m = window.monaco;
      const scrollbars = (fg) => ({
        'scrollbar.shadow': '#00000000',
        'scrollbarSlider.background': fg + '1f',
        'scrollbarSlider.hoverBackground': fg + '33',
        'scrollbarSlider.activeBackground': fg + '47',
      });
      const dark = '#1e1e20';
      m.editor.defineTheme('blazar-dark', {
        base: 'vs-dark',
        inherit: true,
        rules: [
          { token: '', foreground: 'd1d1d6' },
          { token: 'comment', foreground: '7f8c98', fontStyle: 'italic' },
          { token: 'keyword', foreground: 'fc5fa3' },
          { token: 'number', foreground: 'd0bf69' },
          { token: 'string', foreground: 'fc6a5d' },
          { token: 'type', foreground: '5dd8ff' },
          { token: 'type.identifier', foreground: '5dd8ff' },
          { token: 'delimiter', foreground: 'a1a1a6' },
        ],
        colors: {
          'editor.background': dark,
          'editor.foreground': '#d1d1d6',
          'editorGutter.background': dark,
          'minimap.background': dark,
          'editorStickyScroll.background': dark,
          'editorLineNumber.foreground': '#5a5a60',
          'editorLineNumber.activeForeground': '#a1a1a6',
          'editor.lineHighlightBackground': '#2a2a2d',
          'editor.lineHighlightBorder': '#00000000',
          'editor.selectionBackground': '#0a84ff55',
          'editor.inactiveSelectionBackground': '#0a84ff2a',
          'editorCursor.foreground': '#0a84ff',
          'editorIndentGuide.background1': '#ffffff12',
          'editorIndentGuide.activeBackground1': '#ffffff2a',
          'editorWidget.background': '#2c2c2f',
          'editorWidget.border': '#ffffff1f',
          ...scrollbars('#ffffff'),
        },
      });
      const light = '#ffffff';
      m.editor.defineTheme('blazar-light', {
        base: 'vs',
        inherit: true,
        rules: [
          { token: '', foreground: '1d1d1f' },
          { token: 'comment', foreground: '6c7986', fontStyle: 'italic' },
          { token: 'keyword', foreground: 'ad3da4' },
          { token: 'number', foreground: '272ad8' },
          { token: 'string', foreground: 'c41a16' },
          { token: 'type', foreground: '0b4f79' },
          { token: 'type.identifier', foreground: '0b4f79' },
          { token: 'delimiter', foreground: '3a3a3c' },
        ],
        colors: {
          'editor.background': light,
          'editor.foreground': '#1d1d1f',
          'editorGutter.background': light,
          'minimap.background': light,
          'editorStickyScroll.background': light,
          'editorLineNumber.foreground': '#aeaeb2',
          'editorLineNumber.activeForeground': '#6e6e73',
          'editor.lineHighlightBackground': '#f5f5f7',
          'editor.lineHighlightBorder': '#00000000',
          'editor.selectionBackground': '#007aff33',
          'editor.inactiveSelectionBackground': '#007aff1a',
          'editorCursor.foreground': '#007aff',
          'editorIndentGuide.background1': '#0000000f',
          'editorIndentGuide.activeBackground1': '#00000024',
          'editorWidget.background': '#ffffff',
          'editorWidget.border': '#0000001f',
          ...scrollbars('#000000'),
        },
      });
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
  const defaults = { fontSize: 12.5, minimap: false, wordWrap: true };
  try {
    const saved = JSON.parse(localStorage.getItem('blazar.ui') || '{}') || {};
    if (saved.v !== 2) { delete saved.minimap; delete saved.wordWrap; }
    return { ...defaults, ...saved };
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
      padding: { top: 12, bottom: 12 }, smoothScrolling: true, mouseWheelScrollSensitivity: 1,
      scrollbar: { horizontalScrollbarSize: 10, verticalScrollbarSize: 10, useShadows: false },
      overviewRulerBorder: false, hideCursorInOverviewRuler: true, renderLineHighlight: 'line', guides: { indentation: true },
    });
    this.updatePrefs = () => {
      const p = preferences();
      this.ed.updateOptions({ fontSize: p.fontSize, minimap: { enabled: !!p.minimap }, wordWrap: p.wordWrap ? 'on' : 'off' });
      m.editor.setTheme(editorTheme());
    };
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
