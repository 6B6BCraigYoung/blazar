use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use blazar_transport::ExecSpec;
use serde_json::{Value, json};

use crate::api::Shared;

use super::ports::{forget_listen, listening};
use super::preview::{choose_address, close_tunnel, ensure_tunnel, handed, remember};
use super::{cd_target, changed, fail, load, place, preview_port};

const LISTEN_AFTER_SECS: u64 = 5;

pub(super) fn dev_dir(ws: &str) -> String {
    let safe: String = ws
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    format!("${{TMPDIR:-/tmp}}/blazar-dev-{safe}")
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            if it.peek() == Some(&'[') {
                it.next();
                for n in it.by_ref() {
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[must_use]
pub fn detect_url(log: &str) -> Option<(u16, String, bool)> {
    let clean = strip_ansi(log);
    let mut found = None;
    for (at, _) in clean.match_indices("http") {
        let rest = &clean[at..];
        let (https, rest) = if let Some(r) = rest.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = rest.strip_prefix("http://") {
            (false, r)
        } else {
            continue;
        };
        let host_end = rest
            .find([' ', '\n', '\r', '\t', '"', '\'', ')', '>'])
            .unwrap_or(rest.len());
        let url = &rest[..host_end];
        let (hostport, path) = url.split_once('/').map_or((url, ""), |(h, p)| (h, p));
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h, p),
            None => continue,
        };
        if !matches!(
            host,
            "localhost" | "127.0.0.1" | "0.0.0.0" | "[::]" | "[::1]"
        ) {
            continue;
        }
        let Ok(port) = port.parse::<u16>() else {
            continue;
        };
        if port < 80 {
            continue;
        }
        found = Some((
            port,
            format!("/{}", path.trim_end_matches(['.', ','])),
            https,
        ));
    }
    found
}

struct Running {
    pid: String,
    age: u64,
}

fn running_of(head: &str) -> Option<Running> {
    let mut it = head.split_whitespace();
    (it.next()? == "__RUNNING__").then(|| Running {
        pid: it.next().unwrap_or_default().to_owned(),
        age: it.next().and_then(|a| a.parse().ok()).unwrap_or(u64::MAX),
    })
}

async fn dev_status(st: &Shared, ws: &str) -> Value {
    let v = probe_dev(st, ws).await;
    remember(ws, &v);
    v
}

pub async fn loopback_ports(State(st): State<Shared>, headers: HeaderMap) -> Response {
    if crate::bots::caller_of(&headers).is_some() {
        return fail(StatusCode::FORBIDDEN, "bot 不能列出本机端口");
    }
    let known: Vec<String> = handed().lock().unwrap().keys().cloned().collect();
    for ws in &known {
        dev_status(&st, ws).await;
    }
    let mut ports: Vec<Value> = handed()
        .lock()
        .unwrap()
        .iter()
        .map(
            |(ws, (port, purpose))| json!({ "port": port, "workspace_id": ws, "purpose": purpose }),
        )
        .collect();
    ports.sort_by_key(|p| p["port"].as_u64());
    Json(json!({ "ports": ports })).into_response()
}

async fn probe_dev(st: &Shared, ws: &str) -> Value {
    let Some(p) = place(st, ws).await else {
        return json!({ "running": false, "reason": "没有这个工作区" });
    };
    let script = format!(
        r#"D={dir}
if [ -f "$D/pid" ] && kill -0 "$(cat "$D/pid")" 2>/dev/null; then
  M=$(stat -c %Y "$D/pid" 2>/dev/null || stat -f %m "$D/pid" 2>/dev/null || echo 0)
  echo "__RUNNING__ $(cat "$D/pid") $(( $(date +%s) - M ))"
else echo "__STOPPED__"; fi
[ -f "$D/log" ] && tail -c 24000 "$D/log""#,
        dir = dev_dir(ws)
    );
    let out = match st
        .transport(&p.node)
        .exec(ExecSpec::new("bash").arg("-lc").arg(script))
        .await
    {
        Ok(o) => o.stdout,
        Err(e) => return json!({ "running": false, "reason": format!("连不上 {}：{e}", p.node) }),
    };
    let (head, log) = out.split_once('\n').unwrap_or((&out, ""));
    let running = running_of(head);
    let log = strip_ansi(log);
    let manual = preview_port(st, ws).await;
    let from_log = detect_url(&log);
    let ports = match (&manual, &from_log, &running) {
        (None, None, Some(r)) if r.age >= LISTEN_AFTER_SECS => {
            listening(st, &p.node, ws, &r.pid).await
        }
        _ => Vec::new(),
    };
    let mut v = json!({ "running": running.is_some(), "node": p.node, "log": log });
    if let Some(a) = choose_address(manual, from_log, &ports) {
        v["port"] = json!(a.port);
        v["path"] = json!(a.path);
        v["source"] = json!(a.source);
        if a.source == "listen" {
            v["ports"] = json!(ports);
        }
        if running.is_some() {
            let (port, path) = (a.port, a.path);
            let scheme = if a.https { "https" } else { "http" };
            if p.node == "local" {
                v["preview_url"] = json!(format!("{scheme}://localhost:{port}{path}"));
            } else {
                match ensure_tunnel(ws, &p.node, port).await {
                    Ok(lp) => {
                        v["preview_url"] = json!(format!("{scheme}://127.0.0.1:{lp}{path}"));
                        v["tunnel"] = json!(lp);
                    }
                    Err(e) => v["tunnel_error"] = json!(e),
                }
            }
        }
    }
    if running.is_none() {
        forget_listen(ws);
        close_tunnel(ws).await;
    }
    v
}

pub async fn dev_get(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    Json(dev_status(&st, &id).await).into_response()
}

pub async fn dev_start(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let cmd = load(&st, &id).await["dev"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if cmd.trim().is_empty() {
        return fail(StatusCode::CONFLICT, "还没配置 dev server 的启动命令");
    }
    let Some(p) = place(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个工作区");
    };

    let script = format!(
        r#"D={dir}; mkdir -p "$D" || exit 1
if [ -f "$D/pid" ] && kill -0 "$(cat "$D/pid")" 2>/dev/null; then echo "已经在跑了"; exit 0; fi
cat > "$D/run.sh" <<'BLAZAR_DEV_CMD'
cd {cwd} || exit 97
{cmd}
BLAZAR_DEV_CMD
: > "$D/log"
if command -v setsid >/dev/null 2>&1; then
  nohup setsid bash -l "$D/run.sh" >> "$D/log" 2>&1 < /dev/null &
elif command -v perl >/dev/null 2>&1; then
  nohup perl -MPOSIX -e 'POSIX::setsid(); exec @ARGV' bash -l "$D/run.sh" >> "$D/log" 2>&1 < /dev/null &
else
  nohup bash -l "$D/run.sh" >> "$D/log" 2>&1 < /dev/null &
fi
echo $! > "$D/pid"
echo started"#,
        dir = dev_dir(&id),
        cwd = cd_target(&p.path),
        cmd = cmd
            .lines()
            .filter(|l| l.trim() != "BLAZAR_DEV_CMD")
            .collect::<Vec<_>>()
            .join("\n"),
    );
    match st
        .transport(&p.node)
        .exec(
            ExecSpec::new("bash")
                .arg("-l")
                .arg("-s")
                .stdin(script.into_bytes()),
        )
        .await
    {
        Ok(o) if o.code == 0 => {
            changed(&st, &id);

            tokio::time::sleep(Duration::from_millis(1500)).await;
            Json(dev_status(&st, &id).await).into_response()
        }
        Ok(o) => fail(
            StatusCode::BAD_GATEWAY,
            format!("{}{}", o.stdout, o.stderr).trim().to_owned(),
        ),
        Err(e) => fail(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

pub async fn dev_stop(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(p) = place(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个工作区");
    };
    let script = format!(
        r#"D={dir}; [ -f "$D/pid" ] || exit 0
P=$(cat "$D/pid")
kill -TERM -- "-$P" 2>/dev/null || kill -TERM "$P" 2>/dev/null
for i in 1 2 3 4 5 6; do kill -0 "$P" 2>/dev/null || break; sleep 0.5; done
kill -KILL -- "-$P" 2>/dev/null; kill -KILL "$P" 2>/dev/null
rm -f "$D/pid"; echo "[blazar] 已停止" >> "$D/log""#,
        dir = dev_dir(&id)
    );
    let _ = st
        .transport(&p.node)
        .exec(ExecSpec::new("bash").arg("-lc").arg(script))
        .await;
    forget_listen(&id);
    close_tunnel(&id).await;
    changed(&st, &id);
    Json(dev_status(&st, &id).await).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripts::{Address, ScriptsBody, preview_port_of, put};

    #[test]
    fn dev_server_urls_are_found_in_real_world_logs() {
        let vite = "\u{1b}[32m  VITE v5.4.1\u{1b}[0m  ready in 312 ms\n\n  \u{1b}[32m➜\u{1b}[0m  Local:   \u{1b}[36mhttp://localhost:\u{1b}[1m5173\u{1b}[22m/\u{1b}[0m\n  ➜  Network: use --host to expose\n";
        assert_eq!(detect_url(vite), Some((5173, "/".into(), false)));
        assert_eq!(
            detect_url("- Local:        http://localhost:3000\n- Environments: .env"),
            Some((3000, "/".into(), false))
        );
        assert_eq!(
            detect_url("Serving HTTP on 0.0.0.0 port 8000 (http://0.0.0.0:8000/) ..."),
            Some((8000, "/".into(), false))
        );
        assert_eq!(
            detect_url("listening on https://127.0.0.1:8443/app."),
            Some((8443, "/app".into(), true))
        );

        assert_eq!(
            detect_url("http://localhost:3000 in use\nLocal: http://localhost:3001/"),
            Some((3001, "/".into(), false))
        );

        assert_eq!(
            detect_url("docs at https://vitejs.dev:443/guide and http://example.com:8080/"),
            None
        );
        assert_eq!(detect_url("no url here"), None);
    }

    async fn ports(st: &Shared, headers: HeaderMap) -> (StatusCode, Value) {
        let r = loopback_ports(State(st.clone()), headers).await;
        let status = r.status();
        let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    async fn preview_fixture(dev: &str) -> (Shared, String, tempfile::TempDir) {
        let db = blazar_db::Db::open_in_memory().await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let st = crate::state::AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, root.path().to_path_buf()),
            crate::services::Services::Isolated,
        );
        let ws = uuid::Uuid::now_v7().to_string();
        for q in [
            "INSERT INTO nodes (id, name, transport, status, created_at) VALUES ('n-local', 'local', 'local', 'online', '0')".to_owned(),
            format!(
                "INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES ('{ws}', 'n-local', 'pv', '{}', '0')",
                root.path().display()
            ),
            format!(
                "INSERT INTO workspace_scripts (workspace_id, setup, cleanup, dev, copy_files, updated_at)
                 VALUES ('{ws}', '', '', '{dev}', '', '0')"
            ),
        ] {
            sqlx::query(&q).execute(st.db.pool()).await.unwrap();
        }
        (st, ws, root)
    }

    async fn wait_for_address(st: &Shared, ws: &str) -> Value {
        for _ in 0..30 {
            let v = dev_status(st, ws).await;
            if v["preview_url"].is_string() {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!("the preview never got an address");
    }

    #[test]
    fn a_manual_port_beats_the_log_and_the_log_beats_listening_ports() {
        let logged = Some((3000, "/app".to_owned(), true));
        assert_eq!(
            choose_address(Some(4000), logged.clone(), &[8000]),
            Some(Address {
                port: 4000,
                path: "/".into(),
                https: false,
                source: "manual"
            })
        );
        assert_eq!(
            choose_address(None, logged, &[8000]),
            Some(Address {
                port: 3000,
                path: "/app".into(),
                https: true,
                source: "log"
            })
        );
        assert_eq!(
            choose_address(None, None, &[8001, 8000, 35729]),
            Some(Address {
                port: 8000,
                path: "/".into(),
                https: false,
                source: "listen"
            })
        );
        assert_eq!(choose_address(None, None, &[]), None);
        assert_eq!(preview_port_of(None), Ok(None));
        assert_eq!(preview_port_of(Some(3000)), Ok(Some(3000)));
        assert!(preview_port_of(Some(0)).is_err());
        assert!(preview_port_of(Some(70000)).is_err());
        assert!(running_of("__STOPPED__").is_none());
        let r = running_of("__RUNNING__ 4242 7").unwrap();
        assert_eq!((r.pid.as_str(), r.age), ("4242", 7));
        assert_eq!(running_of("__RUNNING__ 4242").unwrap().age, u64::MAX);
    }

    #[tokio::test]
    async fn a_server_that_never_logs_its_address_is_found_by_its_listening_port() {
        let (st, ws, _root) =
            preview_fixture("exec python3 -m http.server 0 --bind 127.0.0.1").await;
        dev_start(State(st.clone()), Path(ws.clone())).await;
        let v = wait_for_address(&st, &ws).await;
        assert_eq!(v["source"], "listen");
        let port = v["port"].as_u64().unwrap();
        assert!(v["ports"].as_array().unwrap().contains(&json!(port)));
        assert_eq!(v["preview_url"], format!("http://localhost:{port}/"));
        let (_, list) = ports(&st, HeaderMap::new()).await;
        assert!(
            list["ports"]
                .as_array()
                .unwrap()
                .contains(&json!({ "port": port, "workspace_id": ws, "purpose": "preview" }))
        );
        dev_stop(State(st.clone()), Path(ws.clone())).await;
        let (_, after) = ports(&st, HeaderMap::new()).await;
        assert!(!after.to_string().contains(&ws));
    }

    #[tokio::test]
    async fn a_manual_preview_port_is_used_without_guessing() {
        let (st, ws, _root) = preview_fixture("exec sleep 30").await;
        let bad = put(
            State(st.clone()),
            Path(ws.clone()),
            Json(ScriptsBody {
                preview_port: Some(70000),
                ..ScriptsBody::default()
            }),
        )
        .await;
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
        put(
            State(st.clone()),
            Path(ws.clone()),
            Json(ScriptsBody {
                dev: "exec sleep 30".into(),
                preview_port: Some(4321),
                ..ScriptsBody::default()
            }),
        )
        .await;
        assert_eq!(load(&st, &ws).await["preview_port"], 4321);
        dev_start(State(st.clone()), Path(ws.clone())).await;
        let v = dev_status(&st, &ws).await;
        assert_eq!(v["source"], "manual");
        assert_eq!(v["preview_url"], "http://localhost:4321/");
        let (_, list) = ports(&st, HeaderMap::new()).await;
        assert!(
            list["ports"]
                .as_array()
                .unwrap()
                .contains(&json!({ "port": 4321, "workspace_id": ws, "purpose": "preview" }))
        );
        dev_stop(State(st.clone()), Path(ws.clone())).await;
    }

    #[tokio::test]
    async fn loopback_ports_list_running_previews_and_refuse_bots() {
        let (st, ws, _root) =
            preview_fixture("exec python3 -u -m http.server 0 --bind 127.0.0.1").await;
        let (_, before) = ports(&st, HeaderMap::new()).await;
        assert!(!before.to_string().contains(&ws));

        dev_start(State(st.clone()), Path(ws.clone())).await;
        let v = wait_for_address(&st, &ws).await;
        assert_eq!(v["source"], "log");
        let port = v["port"].as_u64().unwrap();

        let (status, list) = ports(&st, HeaderMap::new()).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            list["ports"]
                .as_array()
                .unwrap()
                .contains(&json!({ "port": port, "workspace_id": ws, "purpose": "preview" }))
        );

        let mut bot = HeaderMap::new();
        bot.insert(
            crate::bots::CALLER_HEADER,
            json!({"bot": "b1", "handle": "lead", "role": "lead"})
                .to_string()
                .parse()
                .unwrap(),
        );
        let (status, body) = ports(&st, bot).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.get("ports").is_none());

        dev_stop(State(st.clone()), Path(ws.clone())).await;
        let (_, after) = ports(&st, HeaderMap::new()).await;
        assert!(!after.to_string().contains(&ws));
    }
}
