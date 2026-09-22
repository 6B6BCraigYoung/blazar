let monacoReady = false;
require.config({ paths: { vs: '/vendor/vs' } });
function defineMonacoTheme() {
  const dark = isDark();

  const bg = dark ? '#0e1526' : '#fcfdfe';
  monaco.editor.defineTheme('blazar', {
    base: dark ? 'vs-dark' : 'vs', inherit: true, rules: [],
    colors: { 'editor.background': bg, 'minimap.background': bg, 'editorGutter.background': bg,
      'editorStickyScroll.background': bg, 'editorStickyScrollHover.background': dark ? '#172036' : '#f1f3f7',
      'editorStickyScroll.shadow': dark ? '#00000080' : '#0f172a26',
      'scrollbar.shadow': dark ? '#00000080' : '#0f172a1f',
      'editor.lineHighlightBackground': dark ? '#141c30' : '#f1f3f7',
      'editorLineNumber.foreground': dark ? '#4b5870' : '#a3acbb',
      'editor.selectionBackground': dark ? '#7667ff44' : '#5b4fe033' },
  });
}
require(['vs/editor/editor.main'], () => {
  defineMonacoTheme();
  monacoReady = true;
  if (S.pendingEditor) mountEditor();
});
const LANG = { rs:'rust',ts:'typescript',tsx:'typescript',js:'javascript',jsx:'javascript',mjs:'javascript',
  py:'python',go:'go',java:'java',c:'c',h:'c',cpp:'cpp',hpp:'cpp',cc:'cpp',cu:'cpp',json:'json',
  toml:'ini',ini:'ini',yaml:'yaml',yml:'yaml',md:'markdown',html:'html',css:'css',sh:'shell',
  bash:'shell',zsh:'shell',sql:'sql',lua:'lua',rb:'ruby',php:'php',swift:'swift',kt:'kotlin',
  vue:'html',xml:'xml' };
const langOf = p => LANG[(p.split('.').pop() || '').toLowerCase()] || 'plaintext';
function mountEditor() {
  const host = $('#editor');
  if (!host || !monacoReady) { S.pendingEditor = true; return; }
  S.pendingEditor = false;

  try { S.editor?.dispose(); } catch (_) {}
  S.editor = monaco.editor.create(host, {
    value: '', language: 'plaintext', theme: 'blazar', readOnly: false,
    automaticLayout: true, fontSize: uiPrefs().fontSize, wordWrap: uiPrefs().wordWrap ? 'on' : 'off',
    fontFamily: 'JetBrains Mono, ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
    minimap: { enabled: !!uiPrefs().minimap }, scrollBeyondLastLine: false,
    renderWhitespace: 'selection', padding: { top: 10 },
  });
  S.editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => saveFile());
}
