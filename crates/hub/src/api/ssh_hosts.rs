use std::path::{Path as FsPath, PathBuf};

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<String>,
    pub file: String,
}

fn ssh_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map_or_else(std::env::temp_dir, |b| b.home_dir().to_path_buf())
        .join(".ssh")
}

fn config_path() -> PathBuf {
    std::env::var_os("BLAZAR_SSH_CONFIG").map_or_else(|| ssh_dir().join("config"), PathBuf::from)
}

fn glob_match(pat: &str, s: &str) -> bool {
    fn go(p: &[char], s: &[char]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&p[1..], s) || (!s.is_empty() && go(p, &s[1..])),
            (Some('?'), Some(_)) => go(&p[1..], &s[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &s[1..]),
            _ => false,
        }
    }
    go(
        &pat.chars().collect::<Vec<_>>(),
        &s.chars().collect::<Vec<_>>(),
    )
}

fn expand_include(pat: &str, base: &FsPath) -> Vec<PathBuf> {
    let p = match pat.strip_prefix("~/") {
        Some(rest) => directories::BaseDirs::new()
            .map_or_else(std::env::temp_dir, |b| b.home_dir().to_path_buf())
            .join(rest),
        None if FsPath::new(pat).is_absolute() => PathBuf::from(pat),
        None => base.join(pat),
    };
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !name.contains(['*', '?']) {
        return vec![p];
    }
    let dir = p.parent().map(FsPath::to_path_buf).unwrap_or_default();
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| glob_match(&name, &e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    out.sort();
    out
}

fn strip_comment(value: &str) -> &str {
    let mut cut = value.len();
    for (i, c) in value.char_indices() {
        if c == '#' && value[..i].ends_with(char::is_whitespace) {
            cut = i;
            break;
        }
    }
    value[..cut].trim_end()
}

fn parse_file(path: &FsPath, base: &FsPath, depth: u8, out: &mut Vec<SshHost>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let file = path.display().to_string();
    let mut cur: Vec<usize> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, val) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (
                k.to_ascii_lowercase(),
                strip_comment(v.trim().trim_start_matches('=').trim()),
            ),
            None => continue,
        };
        match key.as_str() {
            "host" => {
                cur.clear();
                for a in val.split_whitespace() {
                    let a = a.trim_matches('"');
                    if a.contains(['*', '?', '!']) || out.iter().any(|h| h.alias == a) {
                        continue;
                    }
                    cur.push(out.len());
                    out.push(SshHost {
                        alias: a.to_owned(),
                        hostname: None,
                        user: None,
                        port: None,
                        file: file.clone(),
                    });
                }
            }
            "match" => cur.clear(),
            "include" if depth < 4 => {
                for pat in val.split_whitespace() {
                    for f in expand_include(pat.trim_matches('"'), base) {
                        parse_file(&f, base, depth + 1, out);
                    }
                }
            }
            "hostname" | "user" | "port" => {
                for &i in &cur {
                    let h = &mut out[i];
                    let slot = match key.as_str() {
                        "hostname" => &mut h.hostname,
                        "user" => &mut h.user,
                        _ => &mut h.port,
                    };
                    if slot.is_none() {
                        *slot = Some(val.trim_matches('"').to_owned());
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn parse_config(path: &FsPath) -> Vec<SshHost> {
    let base = path.parent().map(FsPath::to_path_buf).unwrap_or_default();
    let mut out = Vec::new();
    parse_file(path, &base, 0, &mut out);
    out
}

#[derive(Debug, Serialize)]
pub struct SshHostRow {
    #[serde(flatten)]
    pub host: SshHost,
    pub added: bool,
    pub in_mesh: bool,
}

pub async fn list_ssh_hosts(State(st): State<Shared>) -> ApiResult<Json<Vec<SshHostRow>>> {
    let hosts = parse_config(&config_path());
    let nodes: Vec<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT name, endpoint, network FROM nodes")
            .fetch_all(st.db.pool())
            .await?;
    let rows = hosts
        .into_iter()
        .map(|h| {
            let added = nodes.iter().any(|(n, _, _)| *n == h.alias);
            let in_mesh = nodes.iter().any(|(n, ep, net)| {
                net.as_deref() == Some("easytier")
                    && (*n == h.alias || (ep.is_some() && *ep == h.hostname))
            });
            SshHostRow {
                host: h,
                added,
                in_mesh,
            }
        })
        .collect();
    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
pub struct AddSshHosts {
    pub names: Vec<String>,
}

pub async fn add_ssh_hosts(
    State(st): State<Shared>,
    Json(body): Json<AddSshHosts>,
) -> ApiResult<Json<serde_json::Value>> {
    let known = parse_config(&config_path());
    let now = Utc::now().to_rfc3339();
    let mut added = Vec::new();
    for name in &body.names {
        let Some(h) = known.iter().find(|h| h.alias == *name) else {
            return Err(anyhow::anyhow!("~/.ssh/config 里没有 Host {name}").into());
        };
        let res = sqlx::query(
            "INSERT INTO nodes (id, name, transport, endpoint, network, status, created_at)
             VALUES (?1, ?2, 'ssh', ?3, 'ssh', 'offline', ?4) ON CONFLICT (name) DO NOTHING",
        )
        .bind(blazar_core_types::NodeId::new().to_string())
        .bind(&h.alias)
        .bind(h.hostname.as_deref().unwrap_or(&h.alias))
        .bind(&now)
        .execute(st.db.pool())
        .await?;
        if res.rows_affected() > 0 {
            added.push(h.alias.clone());
        }
    }
    st.emit(ServerEvent::NodesChanged);
    let probe = st.clone();
    tokio::spawn(async move { probe_ssh_nodes(&probe).await });
    Ok(Json(serde_json::json!({ "added": added })))
}

pub async fn remove_ssh_node(
    State(st): State<Shared>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    let row: Option<(Option<String>, i64)> = sqlx::query_as(
        "SELECT n.network, (SELECT COUNT(*) FROM workspaces w WHERE w.node_id = n.id)
         FROM nodes n WHERE n.name = ?1",
    )
    .bind(&name)
    .fetch_optional(st.db.pool())
    .await?;
    match row {
        None => Ok(StatusCode::NOT_FOUND),
        Some((net, _)) if net.as_deref() != Some("ssh") => {
            Err(anyhow::anyhow!("{name} 不是从 SSH 配置加的机器").into())
        }
        Some((_, n)) if n > 0 => {
            Err(anyhow::anyhow!("{name} 上还有 {n} 个工作区，先删掉它们").into())
        }
        Some(_) => {
            sqlx::query("DELETE FROM nodes WHERE name = ?1 AND network = 'ssh'")
                .bind(&name)
                .execute(st.db.pool())
                .await?;
            st.emit(ServerEvent::NodesChanged);
            Ok(StatusCode::NO_CONTENT)
        }
    }
}

pub async fn probe_ssh_nodes(st: &Shared) {
    let Ok(names) = sqlx::query_scalar::<_, String>("SELECT name FROM nodes WHERE network = 'ssh'")
        .fetch_all(st.db.pool())
        .await
    else {
        return;
    };
    if names.is_empty() {
        return;
    }
    let mut set = tokio::task::JoinSet::new();
    for name in names {
        set.spawn(async move {
            let t = blazar_transport::SshTransport::new(name.clone()).connect_timeout(8);
            let ok = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                blazar_transport::NodeTransport::exec(&t, blazar_transport::ExecSpec::new("true")),
            )
            .await
            .is_ok_and(|r| r.is_ok_and(|o| o.code == 0));
            (name, ok)
        });
    }
    let now = Utc::now().to_rfc3339();
    let mut changed = false;
    while let Some(Ok((name, ok))) = set.join_next().await {
        let q = if ok {
            sqlx::query(
                "UPDATE nodes SET status = 'online', last_seen_at = ?1
                 WHERE name = ?2 AND network = 'ssh' AND status != 'online'",
            )
            .bind(&now)
            .bind(&name)
        } else {
            sqlx::query(
                "UPDATE nodes SET status = 'offline'
                 WHERE name = ?1 AND network = 'ssh' AND status != 'offline'",
            )
            .bind(&name)
        };
        if q.execute(st.db.pool())
            .await
            .is_ok_and(|r| r.rows_affected() > 0)
        {
            changed = true;
        }
    }
    if changed {
        st.emit(ServerEvent::NodesChanged);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hosts_includes_and_skips_patterns() {
        let dir = std::env::temp_dir().join(format!("blazar-sshcfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config.d")).unwrap();
        std::fs::write(
            dir.join("config"),
            "Include config.d/*\n\
             Host *\n  ServerAliveInterval 30\n\
             # 注释\n\
             Host gpu-a gpu-b\n  HostName 10.0.0.5\n  User me\n  Port 2222\n\
             Host old\n  Hostname=old.example.com\n\
             Match host foo\n  User nobody\n\
             Host !bad db-*\n  User x\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("config.d/work"),
            "Host lab\n    HostName lab.internal\n",
        )
        .unwrap();
        let hosts = parse_config(&dir.join("config"));
        let names: Vec<_> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(names, ["lab", "gpu-a", "gpu-b", "old"]);
        let b = &hosts[2];
        assert_eq!(b.hostname.as_deref(), Some("10.0.0.5"));
        assert_eq!(b.user.as_deref(), Some("me"));
        assert_eq!(b.port.as_deref(), Some("2222"));
        assert_eq!(hosts[3].hostname.as_deref(), Some("old.example.com"));
        assert_eq!(
            hosts[3].user, None,
            "Match 块里的设置不算到上一个 Host 头上"
        );
        assert!(hosts[0].file.ends_with("work"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trailing_comments_stay_out_of_values() {
        assert_eq!(strip_comment("10.0.0.7  # lab box"), "10.0.0.7");
        assert_eq!(strip_comment("10.0.0.7\t# lab"), "10.0.0.7");
        assert_eq!(strip_comment("#tagged-host"), "#tagged-host");
        assert_eq!(strip_comment("plain"), "plain");
        let dir = std::env::temp_dir().join(format!("blazar-sshcmt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config"),
            "Host lab\n  HostName 10.0.0.7  # easytier; public 203.0.113.9\n  User me # ops\n",
        )
        .unwrap();
        let hosts = parse_config(&dir.join("config"));
        assert_eq!(hosts[0].hostname.as_deref(), Some("10.0.0.7"));
        assert_eq!(hosts[0].user.as_deref(), Some("me"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn glob_matches_include_names() {
        assert!(glob_match("*", "a"));
        assert!(glob_match("*.conf", "x.conf"));
        assert!(!glob_match("*.conf", "x.conf.bak"));
        assert!(glob_match("h?st", "host"));
    }
}
