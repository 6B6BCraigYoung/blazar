use super::*;

pub async fn refresh_mesh(State(st): State<Shared>) -> ApiResult<Json<serde_json::Value>> {
    let probe = st.clone();
    tokio::spawn(async move { super::probe_ssh_nodes(&probe).await });
    if !st.mesh_ctx.discovery_enabled() {
        return Ok(Json(
            serde_json::json!({ "discovered": 0, "skipped": "discovery_off" }),
        ));
    }
    let peers = st.mesh().peers().await?;
    let out = record_peers(&st, &peers, st.mesh_is_hub_host()).await?;
    st.emit(ServerEvent::NodesChanged);
    Ok(Json(out))
}

async fn record_peers(
    st: &Shared,
    peers: &[blazar_netmesh::MeshPeer],
    hub_host: bool,
) -> ApiResult<serde_json::Value> {
    let me = peers.iter().find(|p| hub_host && p.is_self());
    let peers: Vec<_> = peers
        .iter()
        .filter(|p| !(hub_host && p.is_self()))
        .collect();
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

    let me = match me {
        Some(p) => Some(forget_self(st, p, &now).await?),
        None => None,
    };

    Ok(serde_json::json!({
        "discovered": peers.len(),
        "upserted": added,
        "offline": offline,
        "self": me,
    }))
}

async fn forget_self(
    st: &Shared,
    me: &blazar_netmesh::MeshPeer,
    now: &str,
) -> ApiResult<serde_json::Value> {
    sqlx::query(
        "INSERT INTO kv_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(crate::mesh::SELF_HOSTNAME_KEY)
    .bind(serde_json::json!(me.hostname).to_string())
    .bind(now)
    .execute(st.db.pool())
    .await?;

    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT id, name,
                (SELECT COUNT(*) FROM workspaces WHERE node_id = n.id)
              + (SELECT COUNT(*) FROM agent_configs WHERE node_id = n.id)
              + (SELECT COUNT(*) FROM claims WHERE node_id = n.id)
              + (SELECT COUNT(*) FROM jobs WHERE node_id = n.id)
         FROM nodes n
         WHERE network = 'easytier' AND (name = ?1 OR (?2 <> '' AND endpoint = ?2))",
    )
    .bind(&me.hostname)
    .bind(&me.ipv4)
    .fetch_all(st.db.pool())
    .await?;

    let mut removed = Vec::new();
    let mut kept = Vec::new();
    for (id, name, refs) in rows {
        if refs == 0 {
            sqlx::query("DELETE FROM nodes WHERE id = ?1")
                .bind(&id)
                .execute(st.db.pool())
                .await?;
            removed.push(name);
        } else {
            tracing::warn!(target: "blazar::mesh", "节点 {name} 就是这台 hub，但还有 {refs} 项挂在它上面；请把它们挪到 local 后再删掉这个节点");
            kept.push(serde_json::json!({ "name": name, "refs": refs }));
        }
    }

    Ok(serde_json::json!({
        "hostname": me.hostname,
        "ipv4": me.ipv4,
        "removed": removed,
        "kept": kept,
    }))
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

    fn peer(hostname: &str, ipv4: &str, cost: &str) -> blazar_netmesh::MeshPeer {
        blazar_netmesh::MeshPeer {
            hostname: hostname.into(),
            ipv4: ipv4.into(),
            cost: cost.into(),
            latency_ms: Some(1.0),
            loss_rate: Some(0.0),
            nat_type: String::new(),
            tunnel_proto: String::new(),
            version: String::new(),
            peer_id: String::new(),
        }
    }

    async fn mesh_hub(via: &str) -> (Shared, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let st = crate::state::AppState::with_services(
            blazar_db::Db::open_in_memory().await.unwrap(),
            via.into(),
            None,
            crate::mesh::MeshCtx::new(None, dir.path().join("mesh")),
            crate::services::Services::Isolated,
        );
        for (id, name, transport, endpoint) in [
            ("n-local", "local", "local", ""),
            ("n-mini", "mini", "ssh", "10.0.0.1"),
            ("n-renamed", "mini-old", "ssh", "10.0.0.1"),
            ("n-gpu", "gpu-1", "ssh", "10.0.0.2"),
        ] {
            sqlx::query(
                "INSERT INTO nodes (id, name, transport, endpoint, network, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, IIF(?3 = 'ssh', 'easytier', NULL), 'online', '0')",
            )
            .bind(id)
            .bind(name)
            .bind(transport)
            .bind(endpoint)
            .execute(st.db.pool())
            .await
            .unwrap();
        }
        (st, dir)
    }

    async fn names(st: &Shared) -> Vec<(String, String)> {
        sqlx::query_as("SELECT name, status FROM nodes ORDER BY name")
            .fetch_all(st.db.pool())
            .await
            .unwrap()
    }

    async fn stored_self(st: &Shared) -> Option<String> {
        sqlx::query_scalar("SELECT value FROM kv_settings WHERE key = ?1")
            .bind(crate::mesh::SELF_HOSTNAME_KEY)
            .fetch_optional(st.db.pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_hub_host_is_not_kept_as_a_mesh_node() {
        let (st, _dir) = mesh_hub("local").await;
        assert!(st.mesh_is_hub_host());
        let peers = [
            peer("mini", "10.0.0.1", "Local"),
            peer("gpu-1", "10.0.0.2", "p2p"),
        ];
        let out = record_peers(&st, &peers, st.mesh_is_hub_host())
            .await
            .ok()
            .unwrap();
        let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(
            names(&st).await,
            vec![pair("gpu-1", "online"), pair("local", "online")]
        );
        assert_eq!(out["discovered"], 1);
        assert_eq!(out["self"]["hostname"], "mini");
        assert_eq!(
            out["self"]["removed"],
            serde_json::json!(["mini", "mini-old"])
        );
        assert_eq!(out["self"]["kept"], serde_json::json!([]));
        assert_eq!(stored_self(&st).await.as_deref(), Some("\"mini\""));

        record_peers(&st, &peers, true).await.ok().unwrap();
        assert_eq!(names(&st).await.len(), 2);
    }

    #[tokio::test]
    async fn every_refresh_marks_online_peers_as_seen_now() {
        let (st, _dir) = mesh_hub("gpu-1").await;
        sqlx::query("UPDATE nodes SET last_seen_at = '2026-09-01T00:00:00+00:00'")
            .execute(st.db.pool())
            .await
            .unwrap();
        let peers = [
            peer("gpu-1", "10.0.0.2", "Local"),
            peer("mini", "10.0.0.1", "p2p"),
        ];
        record_peers(&st, &peers, false).await.ok().unwrap();
        let seen = |name: &'static str| {
            let st = st.clone();
            async move {
                sqlx::query_scalar::<_, String>("SELECT last_seen_at FROM nodes WHERE name = ?1")
                    .bind(name)
                    .fetch_one(st.db.pool())
                    .await
                    .unwrap()
            }
        };
        for n in ["gpu-1", "mini"] {
            assert!(!seen(n).await.starts_with("2026-09"), "{n}");
        }
        assert!(seen("mini-old").await.starts_with("2026-09"));
    }

    #[tokio::test]
    async fn a_self_row_still_in_use_goes_offline_instead() {
        let (st, _dir) = mesh_hub("local").await;
        sqlx::query(
            "INSERT INTO workspaces (id, node_id, name, path, created_at)
             VALUES ('w1', 'n-mini', 'app', '/srv/app', '0')",
        )
        .execute(st.db.pool())
        .await
        .unwrap();
        let peers = [peer("mini", "10.0.0.1", "Local")];
        let out = record_peers(&st, &peers, true).await.ok().unwrap();
        assert_eq!(out["self"]["removed"], serde_json::json!(["mini-old"]));
        assert_eq!(
            out["self"]["kept"],
            serde_json::json!([{ "name": "mini", "refs": 1 }])
        );
        let rows = names(&st).await;
        assert!(
            rows.contains(&("mini".to_owned(), "offline".to_owned())),
            "{rows:?}"
        );
        let ws: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workspaces WHERE node_id = 'n-mini'")
                .fetch_one(st.db.pool())
                .await
                .unwrap();
        assert_eq!(ws, 1);
    }

    #[tokio::test]
    async fn a_remote_mesh_source_keeps_its_own_row() {
        let (st, _dir) = mesh_hub("gpu-1").await;
        assert!(!st.mesh_is_hub_host());
        let peers = [
            peer("gpu-1", "10.0.0.2", "Local"),
            peer("mini", "10.0.0.1", "p2p"),
        ];
        let out = record_peers(&st, &peers, st.mesh_is_hub_host())
            .await
            .ok()
            .unwrap();
        assert!(out["self"].is_null());
        assert_eq!(stored_self(&st).await, None);
        let rows = names(&st).await;
        for n in ["gpu-1", "mini"] {
            assert!(
                rows.contains(&(n.to_owned(), "online".to_owned())),
                "{rows:?}"
            );
        }
    }
}
