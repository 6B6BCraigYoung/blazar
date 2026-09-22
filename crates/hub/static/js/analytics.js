const anDays = () => { try { return +localStorage.getItem('blazar.an.days') || 30; } catch (_) { return 30; } };
async function drawAnalytics() {
  const host = $('#anBox'); if (!host) return;
  const days = anDays();
  let a; try { a = await api(`/api/analytics?days=${days}&tz=${-new Date().getTimezoneOffset()}`); } catch (e) { host.innerHTML = `<div class="card"><div class="empty">${esc(e.message)}</div></div>`; return; }
  const t = a.totals;

  const byDay = new Map(a.daily.map(d => [d.day, d]));
  const series = [];
  for (let i = days - 1; i >= 0; i--) {
    const d = new Date(Date.now() - i * 86400000); const key = `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
    series.push(byDay.get(key) || { day: key, runs: 0, done: 0, failed: 0, interrupted: 0, cost: 0 });
  }
  const maxRuns = Math.max(1, ...series.map(d => d.runs)), maxCost = Math.max(0.0001, ...series.map(d => d.cost));
  const bars = series.map(d => { const h = v => Math.round(v / maxRuns * 100);
    return `<div class="an-col" title="${esc(d.day)}：${d.runs} 次（成 ${d.done} / 败 ${d.failed} / 中断 ${d.interrupted}）· $${d.cost.toFixed(2)}">
      <div class="an-stack"><i class="bad" style="height:${h(d.failed)}%"></i><i class="mut" style="height:${h(d.interrupted)}%"></i><i class="ok" style="height:${h(d.done)}%"></i></div>
      <div class="an-cost" style="height:${Math.round(d.cost / maxCost * 100)}%"></div></div>`; }).join('');
  const heat = Array.from({ length: 7 }, () => Array(24).fill(0)); let hmax = 1;
  for (const [w, h, n] of a.heat) { heat[w][h] = n; hmax = Math.max(hmax, n); }
  host.innerHTML = `
    <div class="row" style="gap:10px;margin-bottom:12px"><h3 style="margin:0;font-size:14px;font-weight:600">运行分析</h3>
      <div class="seg" id="anRange">${[7, 30, 90].map(n => `<button data-d="${n}" data-on="${n === days}">${n} 天</button>`).join('')}</div></div>
    <div class="kpi" style="margin-bottom:12px">
      <div class="k"><div class="lbl">运行次数</div><div class="val num">${fmt(t.runs)}</div></div>
      <div class="k"><div class="lbl">成功率</div><div class="val num">${t.success_rate == null ? '—' : Math.round(t.success_rate * 100) + '%'}</div></div>
      <div class="k"><div class="lbl">失败</div><div class="val num ${t.failed ? 'bad' : ''}">${fmt(t.failed)}</div></div>
      <div class="k"><div class="lbl">费用</div><div class="val">$${t.cost.toFixed(2)}</div></div>
      <div class="k"><div class="lbl">日均</div><div class="val num">${(t.runs / days).toFixed(1)} 次</div></div>
    </div>
    <div class="card"><h3>每天的运行 <span class="faint t-caption" style="font-weight:400">柱：<b class="ok">成功</b> / <b class="bad">失败</b> / 中断 · 细线：费用</span></h3>
      <div class="an-chart">${bars}</div><div class="an-axis"><span>${esc(series[0].day.slice(5))}</span><span>${esc(series[series.length - 1].day.slice(5))}</span></div></div>
    <div class="an-two">
      <div class="card"><h3>谁在干活</h3>${a.board.length ? `<table class="tb"><thead><tr><th>智能体 / 运行时</th><th>运行</th><th>成功率</th><th>平均耗时</th><th>费用</th></tr></thead><tbody>
        ${a.board.map(b => { const fin = b.done + b.failed; return `<tr><td>${b.agent_id ? `<a href="#/agent/${esc(b.agent_id)}">${b.avatar ? esc(b.avatar) + ' ' : ''}${esc(b.who)}</a>` : esc(AGENT_LABEL[b.who] || b.who)}
          <span class="faint t-micro">${b.agent_id ? esc(AGENT_LABEL[b.runtime] || b.runtime) : ''}</span></td><td class="m num">${b.runs}</td>
          <td class="m num">${fin ? Math.round(b.done / fin * 100) + '%' : '—'}</td><td class="m num">${b.avg_secs ? fmtDur(Math.round(b.avg_secs)) : '—'}</td><td class="m num">$${b.cost.toFixed(2)}</td></tr>`; }).join('')}</tbody></table>`
        : '<div class="t-caption faint">这段时间没有运行</div>'}</div>
      <div class="card"><h3>为什么失败</h3>${a.reasons.length ? a.reasons.map(r => `<div class="an-why"><span class="gchip bad num">${r.n}</span><span title="${esc(r.why)}">${esc(r.why)}</span></div>`).join('')
        : '<div class="t-caption faint">这段时间没有失败的运行</div>'}</div>
    </div>
    <div class="card"><h3>什么时候在跑 <span class="faint t-caption" style="font-weight:400">星期 × 小时（本地时间）</span></h3>
      <div class="an-heat">${heat.map((row, w) => `<span class="an-hl">周${WEEK_CN[w]}</span>${row.map((n, h) => `<i style="--v:${(n / hmax).toFixed(2)}" title="周${WEEK_CN[w]} ${pad2(h)}:00 · ${n} 次"></i>`).join('')}`).join('')}
        <span></span>${Array.from({ length: 24 }, (_, h) => `<em>${h % 6 === 0 ? h : ''}</em>`).join('')}</div></div>
    <h3 style="margin:18px 0 10px;font-size:14px;font-weight:600">Token 与费用（累计）</h3>`;
  $$('#anRange [data-d]').forEach(b => { b.onclick = () => { try { localStorage.setItem('blazar.an.days', b.dataset.d); } catch (_) {} drawAnalytics(); }; });
}
