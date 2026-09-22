const SHORTCUT_KEY = 'blazar.shortcuts';
const SHORTCUTS = [
  { id: 'palette', label: '命令面板', def: 'Mod+K', run: () => openPalette() },
  { id: 'side', label: '收起 / 展开侧栏', def: 'Mod+\\', run: () => setSide(document.body.dataset.side !== 'off') },
  { id: 'explorer', label: '资源管理器', def: 'Mod+B', run: () => toggleRegion('ex') },
  { id: 'chat', label: '对话栏', def: 'Mod+Alt+B', run: () => toggleRegion('aux') },
  { id: 'panel', label: '底部面板', def: 'Mod+J', run: () => toggleRegion('panel') },
  { id: 'newchat', label: '新对话', def: 'Mod+Shift+N', run: () => { if (S.ws) newChat(); } },
  { id: 'diff', label: '差异面板', def: 'Mod+Shift+D', run: () => { if (S.ws) { if (S.lay.hidePanel) toggleRegion('panel'); showPanelTab('diff'); } } },
  { id: 'git', label: 'Git 面板', def: 'Mod+Shift+G', run: () => { if (S.ws) { if (S.lay.hidePanel) toggleRegion('panel'); showPanelTab('git'); } } },
  { id: 'preview', label: '预览面板', def: 'Mod+Shift+P', run: () => { if (S.ws) { if (S.lay.hidePanel) toggleRegion('panel'); showPanelTab('preview'); } } },
  { id: 'inbox', label: '收件箱', def: 'Alt+I', run: () => navigate('#/inbox') },
  { id: 'tasks', label: '任务', def: 'Alt+T', run: () => navigate('#/tasks') },
  { id: 'workspaces', label: '全部工作区', def: 'Alt+W', run: () => navigate('#/workspaces') },
];
function shortcutMap() {
  let saved = {}; try { saved = JSON.parse(localStorage.getItem(SHORTCUT_KEY) || '{}'); } catch (_) {}
  return Object.fromEntries(SHORTCUTS.map(s => [s.id, saved[s.id] ?? s.def]));
}

function comboOf(e) {
  const c = e.code || '';
  let k = /^Key[A-Z]$/.test(c) ? c.slice(3) : /^Digit\d$/.test(c) ? c.slice(5) : ({ Backslash: '\\', Slash: '/', Comma: ',', Period: '.', Semicolon: ';', Quote: "'", BracketLeft: '[', BracketRight: ']', Minus: '-', Equal: '=', Backquote: '`', Space: 'Space', Enter: 'Enter' })[c]
    || (/^F\d{1,2}$/.test(c) ? c : '');
  if (!k) return '';
  return [(e.metaKey || e.ctrlKey) && 'Mod', e.altKey && 'Alt', e.shiftKey && 'Shift', k].filter(Boolean).join('+');
}
const comboText = c => !c ? '未设置' : c.split('+').map(p => ({ Mod: navigator.platform.includes('Mac') ? '⌘' : 'Ctrl', Alt: navigator.platform.includes('Mac') ? '⌥' : 'Alt', Shift: '⇧' })[p] || p).join(' ');
function handleShortcut(e) {
  if (S.recordingShortcut) return false;
  const combo = comboOf(e); if (!combo) return false;

  if (!combo.startsWith('Mod') && e.target.closest?.('input, textarea, select, [contenteditable], .monaco-editor, .xterm')) return false;
  const map = shortcutMap();
  const hit = SHORTCUTS.find(s => map[s.id] === combo);
  if (!hit) return false;
  e.preventDefault(); hit.run(); return true;
}
function dlgShortcuts() {
  const map = shortcutMap();
  openDlg(`<h3>快捷键</h3>
    <div class="t-caption muted" style="margin-bottom:10px">点「改键」后按下新的组合。存在这台机器上；不带 ⌘ / Ctrl 的组合在输入框里不生效。</div>
    <div class="snip-list" style="max-height:52vh">${SHORTCUTS.map(s => `<div class="snip-row"><span style="flex:1">${esc(s.label)}</span>
      <kbd class="sc-kbd" data-k="${s.id}">${esc(comboText(map[s.id]))}</kbd>
      <button class="linkbtn" data-rec="${s.id}">改键</button><button class="linkbtn" data-clr="${s.id}" title="不要这个快捷键">清除</button></div>`).join('')}</div>
    <div class="dfoot"><button class="btn btn-outline" id="scReset">全部恢复默认</button><span class="grow"></span><button class="btn btn-brand" id="scDone">好</button></div>`);
  const save = (id, combo) => { let saved = {}; try { saved = JSON.parse(localStorage.getItem(SHORTCUT_KEY) || '{}'); } catch (_) {} saved[id] = combo; try { localStorage.setItem(SHORTCUT_KEY, JSON.stringify(saved)); } catch (_) {} };
  const stop = () => { S.recordingShortcut = null; document.removeEventListener('keydown', onKey, true); };
  const onKey = e => {
    const id = S.recordingShortcut; if (!id) return;
    e.preventDefault(); e.stopPropagation();
    if (e.key === 'Escape') { stop(); dlgShortcuts(); return; }
    const combo = comboOf(e); if (!combo) return;
    const clash = SHORTCUTS.find(s => s.id !== id && shortcutMap()[s.id] === combo);
    if (clash) { toast(`${comboText(combo)} 已经是「${clash.label}」了`); return; }
    if (!combo.includes('+') && !/^F\d/.test(combo)) { toast('单个字母会和打字冲突，加上 ⌘ / Ctrl / ⌥'); return; }
    save(id, combo); stop(); dlgShortcuts();
  };
  $$('#dlgBody [data-rec]').forEach(b => { b.onclick = () => {
    S.recordingShortcut = b.dataset.rec; $(`#dlgBody [data-k="${b.dataset.rec}"]`).textContent = '按下新的组合…（Esc 取消）';
    document.addEventListener('keydown', onKey, true);
  }; });
  $$('#dlgBody [data-clr]').forEach(b => { b.onclick = () => { save(b.dataset.clr, ''); dlgShortcuts(); }; });
  $('#scReset').onclick = () => { try { localStorage.removeItem(SHORTCUT_KEY); } catch (_) {} dlgShortcuts(); };
  $('#scDone').onclick = () => { stop(); closeDlg(); };
}
