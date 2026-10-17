use super::*;

pub(crate) fn yes() -> bool {
    true
}

pub async fn probe_node(
    State(st): State<Shared>,
    Path(name): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let env: std::collections::BTreeMap<String, String> = if name == "local" {
        Default::default()
    } else {
        crate::proxy::node_net_env(&st, &name)
            .await
            .into_iter()
            .collect()
    };
    let h = st.transport(&name).health_with(&env).await?;

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

    let now = Utc::now().to_rfc3339();
    let probed = serde_json::json!({
        "os": h.os, "arch": h.arch, "cpus": h.cpus, "mem_gb": h.mem_gb,
        "disk_free_gb": h.disk_free_gb, "load1": h.load1,
        "gpu_vram_mb": h.gpus.iter().map(|g| g.vram_mb).collect::<Vec<_>>(),
        "gpus": h.gpus.iter().map(|g| g.name.clone()).collect::<Vec<_>>(),
        "has_ai_egress": has_egress,
        "egress": h.egress,
        "egress_env": env.keys().collect::<Vec<_>>(),
        "labels": labels,
        "probed_at": now,
    });
    let existing: Option<String> =
        sqlx::query_scalar("SELECT capabilities FROM nodes WHERE name = ?1")
            .bind(&name)
            .fetch_optional(st.db.pool())
            .await?;
    let caps = merge_capabilities(
        existing
            .as_deref()
            .and_then(|c| serde_json::from_str(c).ok())
            .unwrap_or(serde_json::Value::Null),
        probed,
    );

    sqlx::query("UPDATE nodes SET capabilities = ?1, last_seen_at = ?2 WHERE name = ?3")
        .bind(caps.to_string())
        .bind(&now)
        .bind(&name)
        .execute(st.db.pool())
        .await?;

    st.emit(ServerEvent::NodesChanged);
    Ok(Json(caps))
}

pub(crate) fn merge_capabilities(
    existing: serde_json::Value,
    probed: serde_json::Value,
) -> serde_json::Value {
    let mut out = match existing {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    if !out.contains_key("max_concurrent") {
        out.insert("max_concurrent".into(), serde_json::json!(2));
    }
    if let serde_json::Value::Object(p) = probed {
        for (k, v) in p {
            out.insert(k, v);
        }
    }
    serde_json::Value::Object(out)
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    #[test]
    fn probing_keeps_fields_the_probe_does_not_own() {
        let before = serde_json::json!({
            "agents": [{ "id": "claude" }], "max_concurrent": 4, "cpus": 8,
            "devices": ["can1"], "constraints": { "tls_intercept": true }
        });
        let after =
            merge_capabilities(before, serde_json::json!({ "cpus": 16, "labels": ["gpu"] }));
        assert_eq!(after["cpus"], 16);
        assert_eq!(after["max_concurrent"], 4);
        assert_eq!(after["agents"][0]["id"], "claude");
        assert_eq!(after["devices"][0], "can1");
        assert_eq!(after["constraints"]["tls_intercept"], true);
        let fresh = merge_capabilities(serde_json::Value::Null, serde_json::json!({ "cpus": 2 }));
        assert_eq!(fresh["max_concurrent"], 2);
    }
}
