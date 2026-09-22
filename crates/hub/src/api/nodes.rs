use super::*;

pub async fn refresh_mesh(State(st): State<Shared>) -> ApiResult<Json<serde_json::Value>> {
    let peers = st.mesh().peers().await?;
    let now = Utc::now().to_rfc3339();
    let mut added = 0usize;

    for p in &peers {
        let meta = serde_json::json!({
            "cost": p.cost,
            "latency_ms": p.latency_ms,
            "loss_rate": p.loss_rate,
            "nat_type": p.nat_type,
            "mesh_version": p.version,
        })
        .to_string();

        let res = sqlx::query(
            "INSERT INTO nodes (id, name, transport, endpoint, network, labels, status, last_seen_at, created_at)
             VALUES (?1, ?2, 'ssh', ?3, 'easytier', ?4, 'online', ?5, ?5)
             ON CONFLICT (name) DO UPDATE SET
                endpoint = excluded.endpoint,
                labels = excluded.labels,
                status = 'online',
                last_seen_at = excluded.last_seen_at",
        )
        .bind(blazar_core_types::NodeId::new().to_string())
        .bind(&p.hostname)
        .bind(&p.ipv4)
        .bind(&meta)
        .bind(&now)
        .execute(st.db.pool())
        .await?;
        added += res.rows_affected() as usize;
    }

    let names: Vec<String> = peers.iter().map(|p| p.hostname.clone()).collect();
    let placeholders = if names.is_empty() {
        "''".to_owned()
    } else {
        std::iter::repeat_n("?", names.len())
            .collect::<Vec<_>>()
            .join(",")
    };
    let sql = format!(
        "UPDATE nodes SET status = 'offline'
         WHERE network = 'easytier' AND name NOT IN ({placeholders})"
    );
    let mut q = sqlx::query(&sql);
    for n in &names {
        q = q.bind(n);
    }
    let offline = q.execute(st.db.pool()).await?.rows_affected();

    st.emit(ServerEvent::NodesChanged);
    Ok(Json(serde_json::json!({
        "discovered": peers.len(),
        "upserted": added,
        "offline": offline,
    })))
}

#[derive(Debug, Deserialize)]
pub struct HealthQuery {
    pub node: String,
}

pub async fn node_health(
    State(st): State<Shared>,
    Query(q): Query<HealthQuery>,
) -> ApiResult<Json<blazar_transport::NodeHealth>> {
    Ok(Json(st.transport(&q.node).health().await?))
}
