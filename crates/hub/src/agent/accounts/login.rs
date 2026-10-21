use axum::extract::{Path, Query, State, WebSocketUpgrade, ws};
use axum::http::StatusCode;
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;

use crate::api::Shared;

use super::probe::{check, shell};
use super::{fail, get};

fn login_ssh_args(node: &str, remote: &str) -> Result<Vec<String>, String> {
    blazar_transport::validate_ssh_target(node).map_err(|e| e.to_string())?;
    Ok(vec![
        "-tt".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=15".into(),
        "--".into(),
        node.into(),
        remote.into(),
    ])
}

pub async fn node_login_ws(
    ws: WebSocketUpgrade,
    State(st): State<Shared>,
    Path((node, runtime)): Path<(String, String)>,
    Query(q): Query<LoginQuery>,
) -> Response {
    if let Err(error) = blazar_transport::validate_ssh_target(&node) {
        return fail(StatusCode::BAD_REQUEST, error.to_string());
    }
    if runtime != "codex" {
        return fail(
            StatusCode::BAD_REQUEST,
            "远端只需要登录 Codex；Claude 用长期 token，经凭据代理转过去",
        );
    }
    let known: Option<String> = sqlx::query_scalar("SELECT name FROM nodes WHERE name = ?1")
        .bind(&node)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten();
    if known.is_none() || node == "local" {
        return fail(StatusCode::NOT_FOUND, "没有这台机器");
    }
    let exports: String = crate::proxy::node_net_env(&st, &node)
        .await
        .iter()
        .map(|(k, v)| format!("export {k}='{}'\n", v.replace('\'', "")))
        .collect();
    let script = format!("{exports}{}", shell("codex", "login --device-auth"));
    let remote = format!("bash -lc '{}'", script.replace('\'', r"'\''"));
    let home = directories::BaseDirs::new()
        .map_or_else(std::env::temp_dir, |b| b.home_dir().to_path_buf());
    let args = match login_ssh_args(&node, &remote) {
        Ok(args) => args,
        Err(error) => return fail(StatusCode::BAD_REQUEST, error),
    };
    let target = blazar_terminal::TerminalTarget::Command {
        program: "ssh".into(),
        args,
        env: Vec::new(),
        cwd: home.display().to_string(),
    };
    ws.on_upgrade(move |socket| install_then_login(st, node, socket, target, q))
}

async fn install_then_login(
    st: Shared,
    node: String,
    mut socket: ws::WebSocket,
    target: blazar_terminal::TerminalTarget,
    q: LoginQuery,
) {
    if crate::remote_cli::remote_version(&st, &node, "codex")
        .await
        .is_none()
    {
        let version = crate::remote_cli::target_version("codex")
            .await
            .map(|v| format!(" {v}"))
            .unwrap_or_default();
        let _ = socket
            .send(ws::Message::Text(
                format!(
                    "{node} 上还没有 Codex，正在把官方发布的 Codex{version} 装进 ~/.blazar …\r\n"
                )
                .into(),
            ))
            .await;
        match crate::remote_cli::install(&st, &node, "codex").await {
            Ok((_, msg)) => {
                let _ = socket
                    .send(ws::Message::Text(format!("{msg}\r\n\r\n").into()))
                    .await;
            }
            Err(e) => {
                let _ = socket
                    .send(ws::Message::Text(format!("安装失败：{e}\r\n").into()))
                    .await;
                let _ = socket.close().await;
                return;
            }
        }
    }
    pty_bridge(socket, target, q).await;
}

async fn pty_bridge(socket: ws::WebSocket, target: blazar_terminal::TerminalTarget, q: LoginQuery) {
    let (mut tx, mut rx) = socket.split();
    let session = match blazar_terminal::TerminalSession::open(&target, q.cols, q.rows) {
        Ok(s) => s,
        Err(err) => {
            let _ = tx
                .send(ws::Message::Text(
                    format!("\r\n打开终端失败: {err}\r\n").into(),
                ))
                .await;
            return;
        }
    };
    let (handle, mut output) = session.split();
    let pump = tokio::spawn(async move {
        while let Some(chunk) = output.recv().await {
            if tx.send(ws::Message::Binary(chunk.into())).await.is_err() {
                break;
            }
        }
        let _ = tx.close().await;
    });
    let input = async {
        while let Some(Ok(msg)) = rx.next().await {
            match msg {
                ws::Message::Text(t) => {
                    match serde_json::from_str::<crate::api::TerminalClientMsg>(&t) {
                        Ok(crate::api::TerminalClientMsg::Input { data }) => {
                            if handle.write(data.as_bytes()).await.is_err() {
                                break;
                            }
                        }
                        Ok(crate::api::TerminalClientMsg::Resize { cols, rows }) => {
                            let _ = handle.resize(cols, rows).await;
                        }
                        Err(_) => {}
                    }
                }
                ws::Message::Close(_) => break,
                _ => {}
            }
        }
    };
    let stop = pump.abort_handle();
    tokio::select! {
        () = input => stop.abort(),
        _ = pump => {}
    }
}

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    #[serde(default = "crate::api::default_cols")]
    pub cols: u16,
    #[serde(default = "crate::api::default_rows")]
    pub rows: u16,
}

pub async fn login_ws(
    ws: WebSocketUpgrade,
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<LoginQuery>,
) -> Response {
    let Some(a) = get(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个账号");
    };
    if a.kind == "token" {
        return fail(
            StatusCode::CONFLICT,
            "这个账号用的是长期 token，需要时更换 token 即可",
        );
    }
    let cmd = match a.provider.as_str() {
        "claude" => "auth login",
        _ => "login",
    };
    let home = directories::BaseDirs::new()
        .map_or_else(std::env::temp_dir, |b| b.home_dir().to_path_buf());
    let target = blazar_terminal::TerminalTarget::Command {
        program: "bash".into(),
        args: vec!["-lc".into(), shell(&a.provider, cmd)],
        env: a.env().into_iter().collect(),
        cwd: home.display().to_string(),
    };
    ws.on_upgrade(move |socket| async move {
        let (mut tx, mut rx) = socket.split();
        let session = match blazar_terminal::TerminalSession::open(&target, q.cols, q.rows) {
            Ok(s) => s,
            Err(err) => {
                let _ = tx
                    .send(ws::Message::Text(
                        format!("\r\n打开终端失败: {err}\r\n").into(),
                    ))
                    .await;
                return;
            }
        };
        let (handle, mut output) = session.split();
        let pump = tokio::spawn(async move {
            while let Some(chunk) = output.recv().await {
                if tx.send(ws::Message::Binary(chunk.into())).await.is_err() {
                    break;
                }
            }
            let _ = tx.close().await;
        });
        let input = async {
            while let Some(Ok(msg)) = rx.next().await {
                match msg {
                    ws::Message::Text(t) => {
                        match serde_json::from_str::<crate::api::TerminalClientMsg>(&t) {
                            Ok(crate::api::TerminalClientMsg::Input { data }) => {
                                if handle.write(data.as_bytes()).await.is_err() {
                                    break;
                                }
                            }
                            Ok(crate::api::TerminalClientMsg::Resize { cols, rows }) => {
                                let _ = handle.resize(cols, rows).await;
                            }
                            Err(_) => {}
                        }
                    }
                    ws::Message::Close(_) => break,
                    _ => {}
                }
            }
        };
        let stop = pump.abort_handle();
        tokio::select! {
            () = input => stop.abort(),
            _ = pump => {}
        }
        if let Some(a) = get(&st, &a.id).await {
            check(&st, &a).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_targets_are_validated_before_login_commands() {
        for node in ["-invalid", "", "Alice@-invalid", "host name"] {
            assert!(login_ssh_args(node, "true").is_err());
        }
        let args = login_ssh_args("hub-host", "true").unwrap();
        let index = args.iter().position(|arg| arg == "hub-host").unwrap();
        assert_eq!(args[index - 1], "--");
    }
}
