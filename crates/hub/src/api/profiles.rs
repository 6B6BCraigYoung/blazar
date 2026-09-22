use super::*;

pub(crate) fn yes() -> bool {
    true
}

pub async fn probe_node(
    State(st): State<Shared>,
    Path(name): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let h = st.transport(&name).health().await?;

    let egress_ok = |c: &u16| matches!(c, 200..=299 | 401);
    let has_egress = h
        .egress
        .iter()
        .any(|(k, c)| (k == "openai" || k == "anthropic") && egress_ok(c));

    let mut labels: Vec<String> = Vec::new();
    if !h.gpus.is_empty() {
        labels.push("gpu".into());
    }
    if has_egress {
        labels.push("ai-egress".into());
    }

    let caps = serde_json::json!({
        "os": h.os, "arch": h.arch, "cpus": h.cpus, "mem_gb": h.mem_gb,
        "disk_free_gb": h.disk_free_gb, "load1": h.load1,
        "gpu_vram_mb": h.gpus.iter().map(|g| g.vram_mb).collect::<Vec<_>>(),
        "gpus": h.gpus.iter().map(|g| g.name.clone()).collect::<Vec<_>>(),
        "has_ai_egress": has_egress,
        "egress": h.egress,
        "labels": labels,
        "max_concurrent": 2,
    });

    sqlx::query("UPDATE nodes SET capabilities = ?1, last_seen_at = ?2 WHERE name = ?3")
        .bind(caps.to_string())
        .bind(Utc::now().to_rfc3339())
        .bind(&name)
        .execute(st.db.pool())
        .await?;

    st.emit(ServerEvent::NodesChanged);
    Ok(Json(caps))
}
