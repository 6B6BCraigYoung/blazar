const RC_BASE = 500, RC_MAX = 30000, STABLE = 60000;
let wsAttempt = 0;
function connectWs() {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/api/ws`);
  const openedAt = Date.now();
  ws.onopen = () => {
    setTimeout(() => { if (ws.readyState === 1) wsAttempt = 0; }, STABLE);

    if (S.ws) loadHistory();
  };
  ws.onmessage = e => {
    const m = JSON.parse(e.data);
    if (m.kind === 'entry') {

      if (S.ws && m.workspace_id === S.ws.id && m.entry?.kind?.type === 'finished') refreshChangesSoon();
      if (S.ws && m.workspace_id === S.ws.id && S.threadSessions?.has(m.session_id)) appendEvent(m.session_id, m.entry);
      else if (S.ws && m.workspace_id === S.ws.id && S.tabs) refreshThreadsSoon();
    } else if (m.kind === 'queue_changed') {
      if (S.ws && m.workspace_id === S.ws.id) loadQueue();
    } else if (m.kind === 'queue_sent') {

      if (S.ws && m.workspace_id === S.ws.id && (m.queued_thread || null) === (S.viewSession || null)) noteSent(m);
      else if (S.ws && m.workspace_id === S.ws.id) refreshThreadsSoon();
    } else if (m.kind === 'scripts_changed') {
      if (S.ws && m.workspace_id === S.ws.id) { refreshScriptRuns(); if (S.lay.panelTab === 'preview' && !S.devBusy) loadDev(); }
    } else if (m.kind === 'inbox_changed') {
      refreshInboxSoon();
    } else if (m.kind === 'autopilots_changed') {
      refreshAutopilotsSoon();
    } else if (m.kind === 'tasks_changed') {
      refreshTasksSoon();
    } else if (m.kind === 'session_titled') {
      if (S.ws && m.workspace_id === S.ws.id && S.threads) {
        const t = S.threads.find(x => x.id === m.session_id);
        if (t) { t.title = m.title; t.titled = true; drawChatTabs(); }
        else loadThreads().then(drawChatTabs);
      }
    } else {
      refreshState().then(() => {
        renderSidebar();
        if (S.ws) drawQueue();

        if (S.onStateChange) S.onStateChange();
        else if (!S.route.startsWith('#/workspaces/')) render();
      }).catch(() => {});
    }
  };
  ws.onclose = () => {
    if (Date.now() - openedAt < STABLE) wsAttempt += 1;
    setTimeout(connectWs, Math.min(RC_BASE * 2 ** Math.max(0, wsAttempt - 1), RC_MAX));
  };
}

async function refreshState() {
  const s = await api('/api/state');
  S.nodes = s.nodes; S.workspaces = s.workspaces;
  noticeTransitions();

  if (S.ws) markSeen(S.ws.id);
  renderSidebar();
  drawStatus();
}
