'use strict';
const $ = s => document.querySelector(s);
const $$ = s => [...document.querySelectorAll(s)];
const esc = s => String(s ?? '').replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));
const fmt = n => (n ?? 0).toLocaleString('zh-CN');
const ago = t => {
  if (!t) return '—';
  const d = (Date.now() - new Date(t)) / 1000;
  if (d < 60) return '刚刚';
  if (d < 3600) return `${d / 60 | 0} 分钟前`;
  if (d < 86400) return `${d / 3600 | 0} 小时前`;
  return `${d / 86400 | 0} 天前`;
};

let busy = 0;
const setBusy = d => { busy = Math.max(0, busy + d); $('#progress').dataset.on = String(busy > 0); };
async function api(path, opts) {
  setBusy(1);
  try {
    const r = await fetch(path, opts);
    if (!r.ok) throw new Error((await r.json().catch(() => ({}))).error || r.statusText);
    return r.status === 204 ? null : r.json();
  } finally { setBusy(-1); }
}
const post = (p, body) => api(p, body === undefined
  ? { method: 'POST' }
  : { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });

let toastTimer;
function toast(msg) {
  const el = $('#toast'); el.textContent = msg; el.dataset.show = 'true'; delete el.dataset.act;
  clearTimeout(toastTimer); toastTimer = setTimeout(() => { el.dataset.show = 'false'; }, 3400);
}

function toastAction(msg, label, fn) {
  const el = $('#toast');
  el.innerHTML = `<span>${esc(msg)}</span><button class="toast-act">${esc(label)}</button>`;
  el.dataset.show = 'true'; el.dataset.act = 'true';
  el.querySelector('button').onclick = () => { el.dataset.show = 'false'; delete el.dataset.act; fn(); };
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { el.dataset.show = 'false'; delete el.dataset.act; }, 9000);
}
const openDlg = html => { $('#dlgBody').innerHTML = html; $('#dlgBody').classList.remove('wide'); $('#dlg').dataset.open = 'true'; };
const closeDlg = () => { $('#dlg').dataset.open = 'false'; };

function ask(msg, { ok = '确定', cancel = '取消', danger = false } = {}) {
  return new Promise(resolve => {
    openDlg(`<div class="ask-msg">${esc(msg).replace(/\n/g, '<br>')}</div>
      <div class="dfoot"><button class="btn btn-outline" id="askNo">${esc(cancel)}</button>
      <button class="btn ${danger ? 'btn-danger' : 'btn-brand'}" id="askOk">${esc(ok)}</button></div>`);
    const done = v => { closeDlg(); document.removeEventListener('keydown', key, true); resolve(v); };
    const key = e => {
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); done(false); }
      if (e.key === 'Enter' && !e.isComposing) { e.preventDefault(); e.stopPropagation(); done(true); }
    };
    document.addEventListener('keydown', key, true);
    $('#askNo').onclick = () => done(false);
    $('#askOk').onclick = () => done(true);
    $('#askOk').focus();
  });
}

function askText(msg, value = '', { ok = '确定' } = {}) {
  return new Promise(resolve => {
    openDlg(`<div class="ask-msg">${esc(msg)}</div><input id="askIn" class="input" value="${esc(value)}">
      <div class="dfoot"><button class="btn btn-outline" id="askNo">取消</button><button class="btn btn-brand" id="askOk">${esc(ok)}</button></div>`);
    const done = v => { closeDlg(); resolve(v); };
    const inp = $('#askIn');
    inp.onkeydown = e => {
      if (e.key === 'Enter' && !e.isComposing) { e.preventDefault(); done(inp.value); }
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); done(null); }
    };
    $('#askNo').onclick = () => done(null);
    $('#askOk').onclick = () => done(inp.value);
    inp.focus(); inp.select();
  });
}

const ACT = {
  awaiting_approval: '等待审批', errored: '出错', completed: '已完成',
  running: '运行中', idle: '空闲',
};
const AGENT_LABEL = { claude_cli: 'Claude Code', codex: 'Codex', claude: 'Claude Code', grok: 'Grok Build', deepcode: 'Deep Code', dsh: 'DeepSeek dsh',
  gemini: 'Gemini CLI', copilot: 'GitHub Copilot CLI', opencode: 'OpenCode', qwen: 'Qwen Code' };
