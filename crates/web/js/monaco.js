


let ready = null;

function load() {
  if (ready) return ready;
  ready = new Promise((resolve, reject) => {
    window.require.config({ paths: { vs: '/vendor/vs' } });
    window.require(['vs/editor/editor.main'], () => {
      const m = window.monaco;
      const bg = '#0c0c0d';
      m.editor.defineTheme('blazar-black', {
        base: 'vs-dark',
        inherit: true,
        rules: [
          { token: '', foreground: 'c8c8cc' },
          { token: 'comment', foreground: '5c5c64', fontStyle: 'italic' },
          { token: 'keyword', foreground: '8fa3c2' },
          { token: 'number', foreground: 'b5904f' },
          { token: 'string', foreground: '8fb59a' },
          { token: 'type', foreground: 'b9c4d6' },
          { token: 'type.identifier', foreground: 'b9c4d6' },
          { token: 'delimiter', foreground: 'a1a1aa' },
        ],
        colors: {
          'editor.background': bg,
          'editor.foreground': '#c8c8cc',
          'editorGutter.background': bg,
          'minimap.background': bg,
          'editorStickyScroll.background': bg,
          'editorLineNumber.foreground': '#4a4a50',
          'editorLineNumber.activeForeground': '#8a8a93',
          'editor.lineHighlightBackground': '#141415',
          'editor.lineHighlightBorder': '#00000000',
          'editor.selectionBackground': '#2f3d52',
          'editor.inactiveSelectionBackground': '#22262d',
          'editorCursor.foreground': '#a1a1aa',
          'editorIndentGuide.background1': '#1f1f22',
          'editorIndentGuide.activeBackground1': '#323237',
          'editorWidget.background': '#1b1b1d',
          'editorWidget.border': '#323237',
          'scrollbar.shadow': '#00000000',
          'scrollbarSlider.background': '#ffffff14',
          'scrollbarSlider.hoverBackground': '#ffffff22',
          'scrollbarSlider.activeBackground': '#ffffff2e',
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
  try { return { fontSize: 12.5, minimap: true, wordWrap: false, ...JSON.parse(localStorage.getItem('blazar.ui') || '{}') }; }
  catch (_) { return { fontSize: 12.5, minimap: true, wordWrap: false }; }
}
function editorTheme() {
  const selected = document.documentElement.dataset.theme;
  return (selected === 'light' || ((!selected || selected === 'system') && !matchMedia('(prefers-color-scheme: dark)').matches)) ? 'vs' : 'blazar-black';
}

export class Editor {
  constructor(m, host, onSave, onDirty) {
    this.m = m;
    this.models = new Map();
    this.onDirty = onDirty;
    const prefs = preferences();
    this.ed = m.editor.create(host, {
      value: '', language: 'plaintext', theme: editorTheme(), automaticLayout: true,
      fontSize: prefs.fontSize, wordWrap: prefs.wordWrap ? 'on' : 'off', lineHeight: 22, fontFamily: '"JetBrains Mono", Menlo, monospace',
      minimap: { enabled: !!prefs.minimap }, scrollBeyondLastLine: false, renderWhitespace: 'selection',
      padding: { top: 12 }, smoothScrolling: true,
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


export async function colorize(text, lang) {
  const m = await load();
  return m.editor.colorize(text, lang, { tabSize: 2 });
}
