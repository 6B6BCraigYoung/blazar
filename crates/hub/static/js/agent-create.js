async function pageAgentCreate() {
  const qs = new URLSearchParams(location.hash.split('?')[1] || '');
  if (!S.runtimes) { try { S.runtimes = (await api('/api/runtimes')).runtimes; } catch (_) { S.runtimes = []; } }
  const rts = S.runtimes.filter(r => r.installed);
  let tpl = null;
  if (qs.get('template')) { try { tpl = await api(`/api/agent-profiles/${encodeURIComponent(qs.get('template'))}`); } catch (_) {} }
  const base = tpl ? { ...tpl, name: `${tpl.name}（副本）`, env: {} } : { runtime: qs.get('runtime') || rts[0]?.id || 'claude', starters: [] };

  let rtReset = false;
  if (tpl && !rts.some(r => r.id === base.runtime) && rts.length) { base.runtime = rts[0].id; base.model = null; base.thinking_level = null; base.permission_mode = null; rtReset = true; }
  let starters = (base.starters || []).map(x => ({ ...x }));
  $('#page').innerHTML = `
    <div class="phead"><a href="#/agents" class="muted t-label">← 返回</a><span class="faint">/</span><h1 class="truncate">${tpl ? `复制 ${esc(tpl.name)}` : '创建智能体'}</h1></div>
    <div class="scroll"><div style="max-width:760px;margin:0 auto">
      ${tpl ? `<div class="notice">指令、开场建议和命令行参数会被复制。环境变量不会复制。</div>` : ''}
      ${rtReset ? `<div class="notice warn">原运行时当前不可用，副本已改用其他运行时。模型、思考等级只在所选运行时上有效，因此已被清空。</div>` : ''}
      <div class="set-sec"><div class="set-h"><h3>身份</h3></div><div class="card set-card">
        <div class="set-row"><label>头像</label><input id="cAv" class="input" style="width:90px;text-align:center;font-size:20px" value="${esc(base.avatar || '')}" placeholder="🤖" maxlength="4"></div>
        <div class="set-row"><label>名称</label><input id="cName" class="input" value="${esc(base.name || '')}" placeholder="例如：深度研究智能体" maxlength="40"></div>
        <div class="set-row"><label>描述</label><div><textarea id="cDesc" class="input" rows="2" placeholder="这个智能体做什么？">${esc(base.description || '')}</textarea><div class="t-caption faint num" id="cDescN"></div></div></div>
      </div></div>
      <div class="set-sec"><div class="set-h"><h3>行为与能力</h3></div><div class="card set-card">
        <div class="set-row top"><label>指令</label><textarea id="cIns" class="input" rows="7" placeholder="写下这个智能体该做什么、关注什么、要避开什么…">${esc(base.instructions || '')}</textarea></div>
        <div class="set-row top"><label>对话开场建议</label><div><div class="t-caption faint" style="margin-bottom:6px">最多三条。点击后填入输入框，不会发送。</div><div id="cStarters"></div></div></div>
      </div></div>
      <div class="set-sec"><div class="set-h"><h3>执行配置</h3><span class="t-caption faint">未指定的选项沿用运行时默认值。</span></div><div class="card set-card">
        <div class="set-row"><label>运行时</label>${rts.length ? `<select id="cRt" class="input">${rts.map(r => `<option value="${esc(r.id)}" ${r.id === base.runtime ? 'selected' : ''}>${esc(r.label)}${r.authed === false ? '（未登录）' : ''}</option>`).join('')}</select>` : '<span class="muted">暂无可用运行时 —— 先去「运行时」页安装或登录一个 CLI</span>'}</div>
        <div class="set-row"><label>模型</label><input id="cModel" class="input mono" list="cModelList" value="${esc(base.model || '')}" placeholder="默认（提供方）"><datalist id="cModelList"></datalist></div>
        <div class="set-row"><label>思考</label><select id="cThink" class="input"><option value="">跟随 CLI 配置</option>${Object.entries(THINK_LABEL).map(([v, t]) => `<option value="${v}" ${base.thinking_level === v ? 'selected' : ''}>${t}</option>`).join('')}</select></div>
        <div class="set-row"><label>权限模式</label><select id="cPermA" class="input">${permOpts(base.runtime, base.permission_mode)}</select></div>
      </div></div>
      <div class="ag-savebar" style="position:sticky;bottom:0"><span class="grow"></span><a class="btn btn-ghost btn-sm" href="#/agents">放弃</a><button class="btn btn-brand btn-sm" id="cCreate" ${rts.length ? '' : 'disabled'}>创建并打开</button></div>
    </div></div>`;
  wireHeader();
  const drawSt = () => {
    $('#cStarters').innerHTML = starters.map((x, i) => `<div class="st-item"><input class="input input-sm" data-sl="${i}" placeholder="审查拉取请求" value="${esc(x.label)}" maxlength="80">
      <textarea class="input" rows="2" data-sp="${i}" placeholder="审查当前拉取请求，总结风险并建议下一步。" maxlength="4000">${esc(x.prompt)}</textarea>
      <button class="btn btn-ghost btn-xs" data-rm="${i}">移除</button></div>`).join('') + (starters.length < 3 ? '<button class="btn btn-outline btn-xs" id="cStAdd">＋ 添加建议</button>' : '');
    $$('#cStarters [data-sl]').forEach(i => { i.oninput = () => { starters[+i.dataset.sl].label = i.value; }; });
    $$('#cStarters [data-sp]').forEach(i => { i.oninput = () => { starters[+i.dataset.sp].prompt = i.value; }; });
    $$('#cStarters [data-rm]').forEach(b => { b.onclick = () => { starters.splice(+b.dataset.rm, 1); drawSt(); }; });
    $('#cStAdd')?.addEventListener('click', () => { starters.push({ label: '', prompt: '' }); drawSt(); });
  };
  drawSt();
  const count = () => { const n = $('#cDesc').value.length; $('#cDescN').textContent = `${n} / 255${n > 255 ? ` · 超出 ${n - 255} 字` : ''}`; };
  count(); $('#cDesc').oninput = count;
  const fillModels = async () => { if (!$('#cRt')) return; const list = await modelsFor($('#cRt').value); $('#cModelList').innerHTML = list.filter(m => m.id).map(m => `<option value="${esc(m.id)}">${esc(m.label)}</option>`).join(''); };
  fillModels();
  $('#cRt')?.addEventListener('change', () => { $('#cPermA').innerHTML = permOpts($('#cRt').value, ''); $('#cModel').value = ''; $('#cThink').value = ''; fillModels(); });
  $('#cName').focus();
  $('#cCreate').onclick = async () => {
    const name = $('#cName').value.trim();
    if (!name) { toast('名称必填'); $('#cName').focus(); return; }
    if ($('#cDesc').value.length > 255) { toast('描述不超过 255 字'); return; }
    const clean = starters.map(x => ({ label: x.label.trim(), prompt: x.prompt.trim() })).filter(x => x.label || x.prompt);
    if (clean.some(x => !x.label || !x.prompt)) { toast('请填写完整或移除每条建议后再保存。'); return; }
    const body = { name, avatar: $('#cAv').value.trim(), description: $('#cDesc').value.trim(), instructions: $('#cIns').value, starters: clean,
      runtime: $('#cRt').value, model: $('#cModel').value.trim() || null, thinking_level: $('#cThink').value || null, permission_mode: $('#cPermA').value || null,
      custom_args: tpl?.custom_args || [], max_concurrent: tpl?.max_concurrent || 1, env: {}, color: '' };
    const btn = $('#cCreate'); btn.disabled = true; btn.textContent = '正在创建…';
    try {
      const r = await api('/api/agent-profiles', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
      toast(`已创建 ${r.name}`); await loadProfiles(); navigate(`#/agent/${encodeURIComponent(r.id)}`);
    } catch (e) { toast(/已经有叫/.test(e.message) ? '已存在同名智能体。' : (e.message || '无法创建智能体。')); btn.disabled = false; btn.textContent = '创建并打开'; }
  };
}

function dlgAgent(a = {}) { navigate('#/agents/new' + (a.runtime ? `?runtime=${encodeURIComponent(a.runtime)}` : '')); }

async function pageAgentDetail(id) {
  const spec = S.agents.find(a => a.id === id);
  if (!spec) { $('#page').innerHTML = '<div class="empty">未知 agent</div>'; return; }
  const configs = await api('/api/agent-configs');
  const mine = configs.filter(c => c.agent_id === id);

  const merged = (node, patch) => {
    const base = mine.find(c => (c.node || '') === (node || '')) || {};
    return {
      node: node || null,
      agent_id: id,
      program_path: base.program_path ?? null,
      model: base.model ?? null,
      permission_mode: base.permission_mode ?? null,
      custom_args: base.custom_args ?? [],
      custom_env: base.custom_env ?? {},
      max_concurrent: base.max_concurrent ?? 1,
      enabled: base.enabled ?? true,
      ...patch,
    };
  };
  const view = new URLSearchParams(location.hash.split('?')[1] || '').get('view') || 'general';

  $('#page').innerHTML = `
    <div class="phead">
      <button class="btn btn-ghost btn-icon menu-btn" style="display:none" aria-label="菜单"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"/></svg></button>
      <a href="#/runtimes" class="muted t-label">运行时</a><span class="faint">/</span>
      <h1>${esc(spec.label)} · 设置</h1>
      <span class="badge badge-muted mono">${esc(spec.program)}</span>
      ${spec.structured ? '' : '<span class="badge badge-warn">纯文本输出</span>'}
      ${spec.resume ? '' : '<span class="badge badge-muted">不支持续接</span>'}
    </div>
    <div style="flex:1;display:flex;min-height:0">
      <nav class="vtabs">
        <button class="vtab" data-v="general">配置</button>
        <button class="vtab" data-v="env">环境变量</button>
        <button class="vtab" data-v="args">自定义参数</button>
      </nav>
      <div class="scroll" id="agentBody"></div>
    </div>`;

  const nodeOptions = ['', ...S.nodes.map(n => n.name)];
  const drawGeneral = () => {
    $('#agentBody').innerHTML = `
      <div class="card">
        <h3>配置</h3>
        <div class="desc">节点级配置优先于全局默认。留空节点即为全局默认。</div>
        <div class="grid2">
          <div class="field"><label>适用机器</label>
            <select id="cNode" class="input">
              ${nodeOptions.map(n => `<option value="${esc(n)}">${esc(n) || '（全局默认）'}</option>`).join('')}
            </select></div>
          <div class="field"><label>可执行文件路径（装在非标准位置时）</label>
            <input id="cPath" class="input" placeholder="${esc(spec.program)}"></div>
          <div class="field"><label>模型</label><input id="cModel" class="input" placeholder="默认"></div>
          <div class="field"><label>最大并发</label>
            <input id="cConc" class="input" type="number" min="1" max="16" value="2"></div>
        </div>
        <div class="field"><label>权限模式</label>
          <select id="cPerm" class="input">${permOpts(spec.id, '')}</select>
          ${modesFor(spec.id).length ? '' :
            '<div class="hint">这个运行时没有权限模式概念，设了也不会生效</div>'}</div>
        <div class="actions"><button class="btn btn-brand btn-sm" id="cSave">保存</button></div>
      </div>
      ${mine.length ? `<div class="card"><h3>已有配置</h3>
        <table class="tb"><thead><tr><th>机器</th><th>路径</th><th>模型</th><th>并发</th><th></th></tr></thead>
        <tbody>${mine.map(c => `<tr>
          <td>${esc(c.node || '（全局默认）')}</td>
          <td class="m">${esc(c.program_path || '—')}</td>
          <td class="m">${esc(c.model || '—')}</td>
          <td class="m">${c.max_concurrent}</td>
          <td style="text-align:right"><button class="btn btn-danger btn-xs"
            data-del="${c.id}">删除</button></td></tr>`).join('')}</tbody></table></div>` : ''}`;
    $('#cSave').onclick = async () => {
      try {
        await post('/api/agent-configs', merged($('#cNode').value, {
          program_path: $('#cPath').value.trim() || null,
          model: $('#cModel').value.trim() || null,
          permission_mode: $('#cPerm').value || null,
          max_concurrent: +$('#cConc').value || 2, enabled: true,
        }));
        toast('已保存'); pageAgentDetail(id);
      } catch (e) { toast('保存失败: ' + e.message); }
    };
    $$('#agentBody [data-del]').forEach(b => {
      b.onclick = async () => {

        try {
          await api(`/api/agent-configs/${b.dataset.del}`, { method: 'DELETE' });
          toast('已删除'); pageAgentDetail(id);
        } catch (e) { toast('删除失败: ' + e.message); }
      };
    });
  };
  const drawEnv = () => {
    const c = mine[0];
    const text = Object.entries(c?.custom_env || {}).map(([k, v]) => `${k}=${v}`).join('\n');
    $('#agentBody').innerHTML = `
      <div class="card">
        <h3>环境变量</h3>
        <div class="desc">注入 agent 进程。<strong>出口代理走这里</strong> ——
          连不上 AI API 的机器需要 <code>HTTPS_PROXY</code>。凭据请用机器本机的登录态，不要写在这里。</div>
        <div class="field"><label>每行一个 KEY=value</label>
          <textarea id="envText" class="input" style="min-height:160px;font-family:var(--font-mono)"
            placeholder="HTTPS_PROXY=http://127.0.0.1:9527">${esc(text)}</textarea></div>
        <div class="field"><label>适用机器</label>
          <select id="eNode" class="input">
            ${nodeOptions.map(n => `<option value="${esc(n)}"${c && c.node === n ? ' selected' : ''}>${esc(n) || '（全局默认）'}</option>`).join('')}
          </select></div>
        <div class="actions"><button class="btn btn-brand btn-sm" id="eSave">保存</button></div>
      </div>`;
    $('#eSave').onclick = async () => {
      const env = {};
      for (const line of $('#envText').value.split('\n')) {
        const t = line.trim();
        if (!t || t.startsWith('#')) continue;
        const i = t.indexOf('=');
        if (i > 0) env[t.slice(0, i).trim()] = t.slice(i + 1).trim();
      }
      try {
        await post('/api/agent-configs',
          merged($('#eNode').value, { custom_env: env, enabled: true }));
        toast(`已保存 ${Object.keys(env).length} 个变量`);
      } catch (e) { toast('保存失败: ' + e.message); }
    };
  };
  const drawArgs = () => {
    const c = mine[0];
    $('#agentBody').innerHTML = `
      <div class="card">
        <h3>自定义参数</h3>
        <div class="desc">追加到 CLI 命令行，每行一个。会插在提示词之前。</div>
        <div class="field"><textarea id="argsText" class="input"
          style="min-height:120px;font-family:var(--font-mono)"
          placeholder="--verbose">${esc((c?.custom_args || []).join('\n'))}</textarea></div>
        <div class="field"><label>适用机器</label>
          <select id="aNode" class="input">
            ${nodeOptions.map(n => `<option value="${esc(n)}">${esc(n) || '（全局默认）'}</option>`).join('')}
          </select></div>
        <div class="actions"><button class="btn btn-brand btn-sm" id="aSave">保存</button></div>
      </div>`;
    $('#aSave').onclick = async () => {
      const args = $('#argsText').value.split('\n').map(s => s.trim()).filter(Boolean);
      try {
        await post('/api/agent-configs',
          merged($('#aNode').value, { custom_args: args, enabled: true }));
        toast(`已保存 ${args.length} 个参数`);
      } catch (e) { toast('保存失败: ' + e.message); }
    };
  };
  const draws = { general: drawGeneral, env: drawEnv, args: drawArgs };
  const select = v => {
    $$('.vtab').forEach(t => { t.dataset.active = String(t.dataset.v === v); });
    (draws[v] || drawGeneral)();
  };
  $$('.vtab').forEach(t => { t.onclick = () => select(t.dataset.v); });
  select(view);
  wireHeader();
}
