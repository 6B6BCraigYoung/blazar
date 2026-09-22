use super::*;

#[derive(Debug, Deserialize)]
pub struct TerminalQuery {
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,

    #[serde(default)]
    pub tab: u32,
}

pub(crate) const fn default_cols() -> u16 {
    100
}
pub(crate) const fn default_rows() -> u16 {
    30
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum TerminalClientMsg {
    Input { data: String },
    Resize { cols: u16, rows: u16 },
}

pub async fn terminal_ws(
    ws: WebSocketUpgrade,
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<TerminalQuery>,
) -> Response {
    ws.on_upgrade(move |socket| async move {
        let Ok((node, path)) = locate(&st, &id).await else {
            return;
        };

        let tmux_session = Some(match q.tab {
            0 => blazar_terminal::session_name(&id),
            n => format!("{}-{n}", blazar_terminal::session_name(&id)),
        });
        let target = if node == "local" {
            blazar_terminal::TerminalTarget::Local {
                cwd: path,
                tmux_session,
            }
        } else {
            blazar_terminal::TerminalTarget::Ssh {
                host: node,
                cwd: path,
                tmux_session,
            }
        };

        let session = match blazar_terminal::TerminalSession::open(&target, q.cols, q.rows) {
            Ok(s) => s,
            Err(err) => {
                let (mut tx, _) = socket.split();
                let _ = tx
                    .send(ws::Message::Text(
                        format!("\r\n打开终端失败: {err}\r\n").into(),
                    ))
                    .await;
                return;
            }
        };

        let (mut tx, mut rx) = socket.split();
        let (handle, mut output) = session.split();

        let pump = tokio::spawn(async move {
            while let Some(chunk) = output.recv().await {
                if tx.send(ws::Message::Binary(chunk.into())).await.is_err() {
                    break;
                }
            }
        });

        while let Some(Ok(msg)) = rx.next().await {
            match msg {
                ws::Message::Text(t) => match serde_json::from_str::<TerminalClientMsg>(&t) {
                    Ok(TerminalClientMsg::Input { data }) => {
                        if handle.write(data.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Ok(TerminalClientMsg::Resize { cols, rows }) => {
                        let _ = handle.resize(cols, rows).await;
                    }
                    Err(_) => {}
                },
                ws::Message::Close(_) => break,
                _ => {}
            }
        }

        handle.close().await;
        pump.abort();
    })
}

pub async fn list_agents() -> Json<Vec<serde_json::Value>> {
    Json(
        blazar_runtime::spec::BUILTIN
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id,
                    "label": s.label,
                    "program": s.program,
                    "resume": s.resume.is_supported(),

                    "interactive": s.interactive,

                    "remote_hands": blazar_runtime::supports_remote_hands(s.id),
                    "structured": !matches!(s.output, blazar_runtime::OutputFormat::PlainLines),
                })
            })
            .collect(),
    )
}
