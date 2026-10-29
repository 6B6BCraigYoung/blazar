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
        "No agent CLI was found on this machine".to_owned()
    } else {
        format!("Available on this machine: {}", usable.join(", "))
    };
    Err(ApiError(anyhow::anyhow!(
        "{label} isn't installed on {node}, and Blazar can't install it automatically. {hint}. Pick another agent, or install it manually and retry"
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
    // 对话里只写一行人话：装什么、装到哪台机器；进行中的用时由页面按这条的时间戳算。
    append(blazar_core_types::EntryKind::ToolUse {
        id: tool.clone(),
        name: "Install".into(),
        input: serde_json::json!({
            "description": installing_line(label, node),
            "agent": label,
            "node": node,
        }),
    })
    .await?;
    let began = std::time::Instant::now();
    let result = crate::remote_cli::install(st, node, agent_id).await;
    let took = began.elapsed().as_secs();
    let (ok, content) = match &result {
        Ok((version, _)) => (true, installed_line(label, version, node, took)),
        Err(e) => (false, failed_line(label, node, took, e)),
    };
    append(blazar_core_types::EntryKind::ToolResult {
        id: tool,
        ok,
        content: content.clone(),
        structured: None,
        images: Vec::new(),
    })
    .await?;
    result
        .map(|_| ())
        .map_err(|_| ApiError(anyhow::anyhow!(content)))
}

pub(crate) fn installing_line(label: &str, node: &str) -> String {
    format!("Installing {label} on {node} (first time only)…")
}

pub(crate) fn took(secs: u64) -> String {
    if secs < 60 {
        format!("{secs} s")
    } else {
        format!("{} min {} s", secs / 60, secs % 60)
    }
}

pub(crate) fn installed_line(label: &str, version: &str, node: &str, secs: u64) -> String {
    let version = if version.is_empty() {
        String::new()
    } else {
        format!(" {version}")
    };
    format!("Installed {label}{version} on {node} in {}", took(secs))
}

pub(crate) fn failed_line(label: &str, node: &str, secs: u64, reason: &str) -> String {
    let reason = reason.lines().next().unwrap_or("").trim();
    let reason = if reason.is_empty() {
        String::new()
    } else {
        format!(": {reason}")
    };
    format!(
        "Couldn't install {label} on {node} (after {}){reason}. Check that {node} can reach the internet, or install {label} there yourself and try again",
        took(secs)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_lines_are_plain_english_without_paths() {
        let start = installing_line("Claude Code", "gpu-1");
        assert_eq!(start, "Installing Claude Code on gpu-1 (first time only)…");
        let done = installed_line("Claude Code", "2.1.3", "gpu-1", 42);
        assert_eq!(done, "Installed Claude Code 2.1.3 on gpu-1 in 42 s");
        assert_eq!(took(125), "2 min 5 s");
        let failed = failed_line(
            "Codex",
            "gpu-1",
            3,
            "Couldn't download Codex 0.5.0 on gpu-1\nmore",
        );
        assert!(
            failed.starts_with("Couldn't install Codex on gpu-1 (after 3 s): Couldn't download")
        );
        for line in [start, done, failed] {
            assert!(!line.contains("~/.blazar") && !line.contains('→'), "{line}");
            assert!(line.is_ascii() || line.ends_with('…'), "{line}");
        }
    }
}
