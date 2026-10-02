// Monaco 的薄封装：一个编辑器实例 + 按路径存的 model。Rust 那边（src/monaco.rs）只通过这里的几个函数跟它打交道。
// Monaco 本身用 hub 自带的 /vendor/vs（AMD 加载器在 index.html 里引入）。

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

export class Editor {
  constructor(m, host, onSave, onDirty) {
    this.m = m;
    this.models = new Map(); // path -> { model, saved }
    this.onDirty = onDirty;
    this.ed = m.editor.create(host, {
      value: '', language: 'plaintext', theme: 'blazar-black', automaticLayout: true,
      fontSize: 13, lineHeight: 22, fontFamily: '"JetBrains Mono", Menlo, monospace',
      minimap: { enabled: false }, scrollBeyondLastLine: false, renderWhitespace: 'selection',
      padding: { top: 12 }, smoothScrolling: true,
    });
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

  // 打开（或切到）一个文件。已经有 model 的直接切过去，`text` 被忽略。
  open(path, text, readonly, line) {
    let e = this.models.get(path);
    if (!e) {
      const model = this.m.editor.createModel(text, readonly ? 'plaintext' : langOf(path));
      e = { model, saved: model.getAlternativeVersionId(), dirty: false };
      // 每次改动都通知（Markdown 预览要跟着刷新），带上现在是不是「未保存」。
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

  // 用磁盘上的新内容替换（agent 改过文件之后）。保持光标和滚动位置。
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
    for (const e of this.models.values()) e.model.dispose();
    this.models.clear();
    this.ed.dispose();
  }
}

export async function createEditor(host, onSave, onDirty) {
  const m = await load();
  return new Editor(m, host, onSave, onDirty);
}

// 给 Markdown 预览 / 对话里的代码块上色，返回 HTML。
export async function colorize(text, lang) {
  const m = await load();
  return m.editor.colorize(text, lang, { tabSize: 2 });
}
