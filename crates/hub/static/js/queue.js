function sendOptions() {
  return {
    resume: $('#resumeChk').checked,
    model: modelSel().model || null, effort: modelSel().effort || null,

    permission_mode: $('#permMode').value || null,
    agent: $('#agentSel').value.startsWith('r:') ? $('#agentSel').value.slice(2) : null,
    profile: $('#agentSel').value.startsWith('p:') ? $('#agentSel').value.slice(2) : null,
    brain: 'local',
    ...prefBody(),
  };
}
async function loadQueue() {
  if (!S.ws) return;
  const ws = S.ws.id;
  try { const q = await api(`/api/workspaces/${ws}/queue`); if (S.ws?.id === ws) S.queue = q; } catch (_) { S.queue = S.queue || []; }
  drawQueue();
}
const queuedHere = () => (S.queue || []).find(q => (q.thread_id || null) === (S.viewSession || null));
function drawQueue() {
  const el = $('#cbQueue'); if (!el) return;
  const mine = queuedHere();
  const others = (S.queue || []).length - (mine ? 1 : 0);
  el.hidden = !mine && !others;
  if (el.hidden) return;
  const running = wsRunning();
  el.innerHTML = mine ? `<div class="cq-h"><span class="cq-dot" data-held="${!!mine.held}"></span>
      <b>${mine.held ? '排着，没有自动发出' : '已排队'}</b>
      <span class="faint">${esc(mine.held || '这一轮结束后自动发出')}</span><span class="grow"></span>
      ${running ? '<button class="linkbtn" data-q="steer" title="不等这一轮结束，现在就插进去（Steer）">插话</button>' : '<button class="linkbtn" data-q="send">发送</button>'}
      <button class="linkbtn" data-q="edit">编辑</button><button class="linkbtn" data-q="drop">撤回</button></div>
    <div class="cq-t">${esc(mine.text)}${mine.images ? ` <span class="faint">［附图 ${mine.images} 张］</span>` : ''}</div>${
      others ? `<div class="cq-o faint">另有 ${others} 条排在别的对话里</div>` : ''}`
    : `<div class="cq-o faint">有 ${others} 条消息排在别的对话里，轮到时自动发出</div>`;
  el.querySelectorAll('[data-q]').forEach(b => { b.onclick = () => queueAct(b.dataset.q, mine); });
}
async function queueAct(act, q) {
  const base = `/api/workspaces/${S.ws.id}/queue/${q.id}`;
  try {
    if (act === 'drop') { await api(base, { method: 'DELETE' }); toast('已撤回'); }
    else if (act === 'edit') {
      await api(base, { method: 'DELETE' });
      const p = $('#prompt'); p.value = q.text + (p.value.trim() ? '\n\n' + p.value : ''); growPrompt(); p.focus();
    }
    else if (act === 'send') { const r = await post(base + '/send'); noteSent(r); }
    else if (act === 'steer') {
      const sid = S.running?.sid || S.session;
      if (!sid) { toast('找不到正在跑的会话'); return; }
      const i = await post(`/api/sessions/${sid}/input`, { text: q.text, thinking: currentRuntime() === 'claude' ? prefs().thinking : null });
      if (!i.accepted) { toast(i.reason || '插不进去：这个运行时不支持中途插话，等这一轮结束会自动发出'); return; }
      await api(base, { method: 'DELETE' });
      toast('已插话，agent 读到后会打勾');
    }
  } catch (e) { toast(e.message); }
  loadQueue();
}

async function enqueue(body, r) {
  S.running = { sid: r.running_session_id, interactive: !!r.running_interactive };
  S.queue = await api(`/api/workspaces/${S.ws.id}/queue`, { method: 'PUT', headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ thread_id: S.viewSession || null, request: body, append: true }) });
  drawQueue();
}

function editUserMessage(u) {
  if (u.querySelector('.cc-edit')) return;
  const tx = u.querySelector('.cc-ut'); const old = tx.textContent;
  tx.hidden = true;
  const ed = document.createElement('div'); ed.className = 'cc-edit';
  ed.innerHTML = `<textarea class="input" rows="3"></textarea><div class="row"><span class="faint t-micro">重试会把文件恢复到这条消息之前，并丢弃它之后的对话</span><span class="grow"></span>
    <button class="btn btn-outline btn-xs" data-x="no">取消</button><button class="btn btn-brand btn-xs" data-x="ok">重试</button></div>`;
  tx.after(ed);
  const ta = ed.querySelector('textarea'); ta.value = old.replace(/\n［附图 \d+ 张］$/, ''); ta.focus();
  const close = () => { ed.remove(); tx.hidden = false; };
  ed.querySelector('[data-x="no"]').onclick = close;
  ed.querySelector('[data-x="ok"]').onclick = () => { const t = ta.value.trim(); if (!t) return; close(); retryMessage(u, t); };
  ta.onkeydown = e => {
    if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); close(); }
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); ed.querySelector('[data-x="ok"]').click(); }
  };
}
async function retryMessage(u, text) {
  if (wsRunning()) { toast('agent 还在运行，先中断再重试'); return; }
  const later = [...$$('#log > .cc-user[data-first="true"]')].filter(x => x.compareDocumentPosition(u) & Node.DOCUMENT_POSITION_PRECEDING).length;
  if (!await ask(`${text == null ? '重新生成这一轮' : '用改过的内容重试'}？\n\n· 工作区的文件会恢复到这条消息发出之前（现在的状态会先存一份）\n· 这条消息${later ? `和它之后的 ${later} 轮对话` : ''}会标成「已回退」，agent 不再记得\n· 不是 git 仓库的目录没有检查点，只重来对话、不动文件`, { ok: '重试', danger: true })) return;
  try {
    const r = await post(`/api/workspaces/${S.ws.id}/retry`, { session_id: u.dataset.sid, text, options: { ...sendOptions(), wait_secs: 20 } });
    if (r.admitted === false) { toast(r.reason || '工作区正忙'); return; }
    if (r.session_id) { S.session = r.session_id; noteSent(r); }
    if (r.files_restored) reloadWorkspaceFiles();
    toast(r.files_restored ? '文件已恢复，正在从这里重来' : '正在从这里重来（这个目录没有检查点，文件没动）');
  } catch (e) { toast('重试失败: ' + e.message); loadHistory(); }
}

function askCard(k, resolved) {
  const qs = Array.isArray(k.request?.input?.questions) ? k.request.input.questions : [];
  CC.ask.set(k.id, qs);
  return `<div class="cc-appr cc-ask" data-appr="${esc(k.id)}" data-done="${resolved ? 'true' : 'false'}">
    <div class="ah">agent 有问题要问你</div>
    ${qs.map((q, qi) => `<div class="askq" data-q="${qi}" data-multi="${!!q.multiSelect}">
      <div class="askh">${q.header ? `<span class="badge badge-muted">${esc(q.header)}</span> ` : ''}${esc(q.question || '')}${q.multiSelect ? ' <span class="faint">（可多选）</span>' : ''}</div>
      <div class="askopts">${(q.options || []).map((o, oi) => `<button class="askopt" data-o="${oi}" aria-pressed="false"><b>${esc(o.label)}</b>${o.description ? `<span>${esc(o.description)}</span>` : ''}</button>`).join('')}</div>
      <input class="input input-sm askother" placeholder="其他（自己写）">
    </div>`).join('')}
    <div class="aopts"><button class="aopt" data-ask-ok>提交回答</button><button class="aopt" data-no>不回答，告诉 agent 该怎么做 <span class="faint">(esc)</span></button></div>
    <div class="adeny"><input class="input input-sm" placeholder="说明，回车发送（可留空）"></div>
    <div class="apstate">${resolved ? esc(resolved) : ''}</div>
  </div>`;
}
async function submitAsk(card) {
  const qs = CC.ask.get(card.dataset.appr) || [];
  const answers = {};
  for (const box of card.querySelectorAll('.askq')) {
    const q = qs[+box.dataset.q]; if (!q) continue;
    const picked = [...box.querySelectorAll('.askopt[aria-pressed="true"]')].map(b => q.options[+b.dataset.o]?.label).filter(Boolean);
    const other = box.querySelector('.askother').value.trim();
    if (other) picked.push(other);
    if (!picked.length) { toast(`还有问题没回答：${q.header || q.question}`); return; }
    answers[q.question] = picked.join(', ');
  }
  try {
    const r = await post(`/api/approvals/${card.dataset.appr}`, { allow: true, answers });
    if (r.delivered) markApproval(card.dataset.appr, '已回答：' + Object.values(answers).join('；'));
    else { toast(r.reason || '没能送达'); markApproval(card.dataset.appr, r.reason || '没能送达'); }
  } catch (e) { toast('提交失败: ' + e.message); }
}

async function send() {
  let text = $('#prompt').value.trim();
  const rv = reviewList();
  if (!S.ws || (!text && !(S.attach || []).length && !rv.length)) return;
  if (!text) text = rv.length ? '请逐条处理下面的审阅意见。' : '看一下这张图';

  const wire = text + reviewBlock(rv);
  $('#prompt').value = ''; growPrompt();
  if (S.lay.hideAux) toggleRegion('aux');
  showAuxTab('chat');
  const btn = $('#btnSend');
  btn.disabled = true; btn.dataset.busy = 'true';
  try {

    const body = { text: wire, resume_session: S.viewSession || null, images: takeImages(), ...sendOptions(), context_file: ctxFile(), wait_secs: 20 };
    const r = await post(`/api/workspaces/${S.ws.id}/prompt`, body);
    if (r.admitted === false) {

      try {
        await enqueue(body, r);
        if (rv.length) { saveReview([]); drawDiff(); }
        S.attach = []; drawAttach();
        toast('已排队：这一轮结束后自动发出');
      } catch (e) { restorePrompt(text); toast(`没排上：${e.message}`); }
      return;
    }
    if (r.session_id) {
      S.session = r.session_id;
      noteSent(r);
      if (rv.length) { saveReview([]); drawDiff(); }
    }
    refreshCpSoon();
    setFresh(false);
    S.attach = []; drawAttach();
    const a = r.activity || {};
    if (a.started === false) {
      toast(a.reason || 'agent 未能启动');
      const log = $('#log');
      if (log) {
        log.querySelector('.cc-empty, .empty')?.remove();
        log.insertAdjacentHTML('beforeend',
          `<div class="cc-row cc-err"><span class="cc-dot"></span><div class="cc-main">agent 没能启动：${
            esc(a.reason || '')}${a.failure_class ? `<div class="faint">分类：${esc(a.failure_class)}</div>` : ''}</div></div>`);
        log.scrollTop = log.scrollHeight;
      }
    }
  } catch (e) { toast('发送失败: ' + e.message); }
  finally { btn.disabled = false; delete btn.dataset.busy; drawStatus(); }
}
function cycleMode() {
  const list = modeList(); if (!list.length) return;

  const i = list.findIndex(x => x[0] === effectiveMode()?.[0]);
  setMode(list[(i + 1) % list.length][0]);
}
async function stopSession() {

  const sid = S.session || S.ws?.sessions?.[0]?.id;
  if (!sid) { toast('该工作区没有运行中的会话'); return; }
  try {
    const r = await post(`/api/sessions/${sid}/interrupt`);
    toast(r.interrupted ? '已中断' : (r.reason || '未能中断'));
  } catch (e) { toast('中断失败: ' + e.message); }
}
