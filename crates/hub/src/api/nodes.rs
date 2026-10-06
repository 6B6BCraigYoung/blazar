use super::*;

pub async fn refresh_mesh(State(st): State<Shared>) -> ApiResult<Json<serde_json::Value>> {
    let probe = st.clone();
    tokio::spawn(async move { super::probe_ssh_nodes(&probe).await });
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
pub struct NodeRoles {
    pub roles: std::collections::BTreeMap<String, Option<String>>,
}

pub async fn set_node_roles(
    State(st): State<Shared>,
    Json(body): Json<NodeRoles>,
) -> ApiResult<Json<serde_json::Value>> {
    if body
        .roles
        .values()
        .flatten()
        .any(|r| !matches!(r.as_str(), "dev" | "personal"))
    {
        return Err(ApiError::bad_request("机器类型只能是开发机或个人电脑"));
    }
    let mut tx = st.db.pool().begin().await?;
    let mut updated = 0u64;
    for (name, role) in &body.roles {
        if name == "local" {
            continue;
        }
        updated += sqlx::query("UPDATE nodes SET role = ?1 WHERE name = ?2")
            .bind(role)
            .bind(name)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    }
    tx.commit().await?;
    st.emit(ServerEvent::NodesChanged);
    Ok(Json(serde_json::json!({ "updated": updated })))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn machines_keep_the_role_the_user_picked() {
        let dir = tempfile::tempdir().unwrap();
        let st = crate::state::AppState::with_services(
            blazar_db::Db::open_in_memory().await.unwrap(),
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, dir.path().join("mesh")),
            crate::services::Services::Isolated,
        );
        for (name, cost) in [("gpu-1", "p2p"), ("alice-mbp", "relay"), ("me", "Local")] {
            sqlx::query(
                "INSERT INTO nodes (id, name, transport, network, labels, status, created_at)
                 VALUES (?1, ?1, 'ssh', 'easytier', ?2, 'online', '0')",
            )
            .bind(name)
            .bind(serde_json::json!({ "cost": cost }).to_string())
            .execute(st.db.pool())
            .await
            .unwrap();
        }
        let roles = |v: serde_json::Value| Json(serde_json::from_value::<NodeRoles>(v).unwrap());
        assert!(
            set_node_roles(
                State(st.clone()),
                roles(serde_json::json!({ "roles": { "gpu-1": "server" } }))
            )
            .await
            .is_err()
        );
        let _ = set_node_roles(
            State(st.clone()),
            roles(serde_json::json!({ "roles": { "gpu-1": "dev", "alice-mbp": "personal" } })),
        )
        .await
        .ok()
        .unwrap();
        let nodes = super::super::view::snapshot(&st).await.unwrap().nodes;
        let get = |n: &str| nodes.iter().find(|x| x.name == n).unwrap();
        assert!(get("gpu-1").is_usable());
        assert!(get("alice-mbp").is_personal() && !get("alice-mbp").is_usable());
        assert!(get("me").is_self_mesh() && !get("me").is_pending());
    }
}
