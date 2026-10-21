use super::*;

pub(super) async fn ensure_installed(st: &Shared, node: &str, agent_id: &str) -> ApiResult<bool> {
    let missing = |list: &[blazar_runtime::DiscoveredAgent]| {
        list.iter()
            .find(|a| a.id == agent_id)
            .is_some_and(|a| a.path.is_none())
    };
    let cached: Option<String> = sqlx::query_scalar(
        "SELECT json_extract(capabilities, '$.agents') FROM nodes WHERE name = ?1",
    )
    .bind(node)
    .fetch_optional(st.db.pool())
    .await?
    .flatten();
    let cached = cached
        .and_then(|c| serde_json::from_str::<Vec<blazar_runtime::DiscoveredAgent>>(&c).ok())
        .unwrap_or_default();
    if !missing(&cached) {
        return Ok(false);
    }
    let Ok(fresh) = blazar_runtime::discover(&st.transport(node)).await else {
        return Ok(false);
    };
    crate::remote_cli::store_agents(st, node, &fresh).await;
    if !missing(&fresh) {
        return Ok(false);
    }
    if crate::remote_cli::installable(agent_id) {
        return Ok(true);
    }
    let label = blazar_runtime::spec::find(agent_id).map_or(agent_id, |s| s.label);
    let usable: Vec<&str> = fresh
        .iter()
        .filter(|a| a.path.is_some())
        .map(|a| a.label.as_str())
        .collect();
    let hint = if usable.is_empty() {
        "这台机器上一个智能体 CLI 都没找到".to_owned()
    } else {
        format!("这台机器上能用的是 {}", usable.join("、"))
    };
    Err(ApiError(anyhow::anyhow!(
        "{node} 上没有装 {label}，Blazar 也没法自动装它。{hint}；换一个智能体，或手动装上再试"
    )))
}

pub(super) async fn install_agent(
    st: &Shared,
    sid: SessionId,
    ws: WorkspaceId,
    node: &str,
    agent_id: &str,
    next_seq: &mut u64,
) -> ApiResult<()> {
    let label = blazar_runtime::spec::find(agent_id).map_or(agent_id, |s| s.label);
    let version = crate::remote_cli::target_version(agent_id)
        .await
        .map(|v| format!(" {v}"))
        .unwrap_or_default();
    let tool = blazar_core_types::ToolId(format!("blazar-install-{agent_id}"));
    let mut append = async |kind: blazar_core_types::EntryKind| -> ApiResult<()> {
        let entry = NormalizedEntry {
            seq: *next_seq,
            ts: Utc::now(),
            parent_tool_use_id: None,
            kind,
        };
        st.db.append_event(sid, ws, &entry).await?;
        *next_seq += 1;
        st.emit(ServerEvent::Entry {
            workspace_id: ws,
            session_id: sid,
            entry: Box::new(entry),
        });
        Ok(())
    };
    append(blazar_core_types::EntryKind::ToolUse {
        id: tool.clone(),
        name: "Install".into(),
        input: serde_json::json!({
            "description": format!("{label}{version} → {node}:~/.blazar"),
        }),
    })
    .await?;
    let result = crate::remote_cli::install(st, node, agent_id).await;
    let (ok, content) = match &result {
        Ok((_, msg)) => (true, msg.clone()),
        Err(e) => (false, e.clone()),
    };
    append(blazar_core_types::EntryKind::ToolResult {
        id: tool,
        ok,
        content,
        structured: None,
    })
    .await?;
    result.map(|_| ()).map_err(|e| ApiError(anyhow::anyhow!(e)))
}
