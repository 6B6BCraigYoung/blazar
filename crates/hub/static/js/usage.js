async function pageUsage() {
  $('#page').innerHTML = `
    <div class="phead"><button class="btn btn-ghost btn-icon menu-btn" style="display:none" aria-label="菜单"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"/></svg></button>
      <h1>用量</h1><div class="grow"></div></div>
    <div class="scroll" id="ub"><div class="empty">加载中…</div></div>`;
  wireHeader();
  try {
    const u = await api('/api/usage');
    const t = u.total;
    const tbl = (title, obj) => {
      const rows = Object.entries(obj);
      return rows.length ? `<div class="card"><h3>${title}</h3>
        <table class="tb"><thead><tr><th>名称</th><th>调用</th><th>输入</th><th>输出</th>
          <th>缓存读</th><th>费用</th></tr></thead><tbody>
        ${rows.sort((a, b) => b[1].cost_usd - a[1].cost_usd).map(([k, v]) => `<tr>
          <td>${esc(AGENT_LABEL[k] || k)}</td><td class="m num">${fmt(v.calls)}</td>
          <td class="m num">${fmt(v.input)}</td><td class="m num">${fmt(v.output)}</td>
          <td class="m num">${fmt(v.cache_read)}</td>
          <td class="m num">$${v.cost_usd.toFixed(4)}</td></tr>`).join('')}</tbody></table></div>` : '';
    };
    $('#ub').innerHTML = `<div id="anBox"></div>
      <div class="kpi" style="margin-bottom:12px">
        <div class="k"><div class="lbl">总费用</div><div class="val">$${t.cost_usd.toFixed(2)}</div></div>
        <div class="k"><div class="lbl">调用次数</div><div class="val num">${fmt(t.calls)}</div></div>
        <div class="k"><div class="lbl">输入 token</div><div class="val num">${fmt(t.input)}</div></div>
        <div class="k"><div class="lbl">输出 token</div><div class="val num">${fmt(t.output)}</div></div>
        <div class="k"><div class="lbl">缓存读取</div><div class="val num">${fmt(t.cache_read)}</div></div>
      </div>
      ${tbl('按 Agent', u.by_agent)}
      ${tbl('按机器', u.by_node)}
      ${u.by_workspace.length ? `<div class="card"><h3>按工作区</h3>
        <table class="tb"><thead><tr><th>工作区</th><th>调用</th><th>输出</th><th>费用</th></tr></thead>
        <tbody>${u.by_workspace.sort((a, b) => b.usage.cost_usd - a.usage.cost_usd).slice(0, 30)
          .map(w => `<tr><td>${esc(w.workspace)}</td><td class="m num">${fmt(w.usage.calls)}</td>
            <td class="m num">${fmt(w.usage.output)}</td>
            <td class="m num">$${w.usage.cost_usd.toFixed(4)}</td></tr>`).join('')}</tbody></table></div>` : ''}
      ${t.calls ? '' : '<div class="empty">还没有用量数据 —— 跑一次 agent 会话就有了</div>'}`;
    drawAnalytics();
  } catch (e) { $('#ub').innerHTML = errPage(e); }
}
