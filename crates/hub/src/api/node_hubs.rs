use super::*;

use blazar_transport::{ExecSpec, shell_quote};

const SESSION_FILES: &str = concat!(
    r#""$HOME/Library/Application Support/ai.blazar.Blazar/hub/hub.session" "#,
    r#""$HOME/Library/Application Support/ai.blazar.Blazar/profiles"/*/hub.session "#,
    r#""${XDG_DATA_HOME:-$HOME/.local/share}/blazar/hub/hub.session" "#,
    r#""${XDG_DATA_HOME:-$HOME/.local/share}/blazar/profiles"/*/hub.session"#,
);

const HUB_CALL: &str = r#"
hub_url() { sed -n 's/.*"url":"\([^"]*\)".*/\1/p' "$1" | head -1; }
hub_call() {
  local u t
  u=$(hub_url "$1")
  t=$(sed -n 's/.*"token":"\([0-9a-f]\{64\}\)".*/\1/p' "$1" | head -1)
  [ -n "$u" ] && [ -n "$t" ] || return 1
  printf 'header = "Authorization: Bearer %s"\n' "$t" | curl -sS -m 8 -K - -X "$2" -H 'Content-Type: application/json' ${4:+--data "$4"} "$u$3"
}
"#;

fn list_script() -> String {
    format!(
        r#"{HUB_CALL}
for f in {SESSION_FILES}; do
  [ -f "$f" ] || continue
  body=$(hub_call "$f" GET /api/accounts) || continue
  printf 'HUB\t%s\t%s\n%s\nEND\n' "$f" "$(hub_url "$f")" "$body"
done
exit 0"#
    )
}

fn mode_script(session: &str, body: &serde_json::Value) -> String {
    format!(
        r#"{HUB_CALL}
f={}
[ -f "$f" ] || {{ echo '{{"error":"那个 hub 的会话文件不见了"}}'; exit 0; }}
hub_call "$f" PUT /api/accounts/mode {}"#,
        shell_quote(session),
        shell_quote(&body.to_string())
    )
}

#[derive(Debug, Serialize, PartialEq)]
pub struct PeerHub {
    pub session: String,
    pub url: String,
    pub label: String,
    pub accounts: serde_json::Value,
    pub modes: serde_json::Value,
}

fn label_of(session: &str) -> String {
    let parts: Vec<&str> = session.split('/').collect();
    match parts.len().checked_sub(2).map(|i| parts[i]) {
        Some("hub") => "后台服务".to_owned(),
        Some(profile) if parts.len() >= 3 && parts[parts.len() - 3] == "profiles" => {
            format!("桌面 · {profile}")
        }
        _ => "Blazar".to_owned(),
    }
}

#[must_use]
pub fn parse_hubs(out: &str) -> Vec<PeerHub> {
    let mut hubs = Vec::new();
    let mut head: Option<(String, String)> = None;
    let mut body = String::new();
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("HUB\t") {
            let mut it = rest.splitn(2, '\t');
            head = Some((
                it.next().unwrap_or_default().to_owned(),
                it.next().unwrap_or_default().to_owned(),
            ));
            body.clear();
        } else if line == "END" {
            if let Some((session, url)) = head.take()
                && let Ok(v) = serde_json::from_str::<serde_json::Value>(body.trim())
                && v["accounts"].is_array()
            {
                hubs.push(PeerHub {
                    label: label_of(&session),
                    session,
                    url,
                    accounts: v["accounts"].clone(),
                    modes: v["modes"].clone(),
                });
            }
        } else if head.is_some() {
            body.push_str(line);
            body.push('\n');
        }
    }
    hubs
}

async fn discover(st: &Shared, node: &str) -> ApiResult<Vec<PeerHub>> {
    let out = st
        .transport(node)
        .exec(ExecSpec::new("bash").arg("-lc").arg(list_script()))
        .await?;
    let mine = (node == "local")
        .then(|| st.auth.get().map(|s| s.path.display().to_string()))
        .flatten();
    let my_url = crate::office::HUB_URL.get().cloned();
    Ok(parse_hubs(&out.stdout)
        .into_iter()
        .filter(|h| mine.as_deref() != Some(h.session.as_str()))
        .filter(|h| node != "local" || my_url.as_deref() != Some(h.url.as_str()))
        .collect())
}

const READ_ONLY: &str = "这是开发实例，别的 Blazar 的账号只能看，要改请到正式的 Blazar 里改";

fn listing(st: &AppState, hubs: Vec<PeerHub>) -> serde_json::Value {
    serde_json::json!({ "hubs": hubs, "read_only": st.dev_instance })
}

pub async fn node_hubs(
    State(st): State<Shared>,
    Path(name): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let hubs = discover(&st, &name).await?;
    Ok(Json(listing(&st, hubs)))
}

#[derive(Debug, Deserialize)]
pub struct PeerMode {
    pub session: String,
    pub provider: String,
    pub mode: String,
}

pub async fn node_hub_mode(
    State(st): State<Shared>,
    Path(name): Path<String>,
    Json(body): Json<PeerMode>,
) -> ApiResult<Json<serde_json::Value>> {
    if st.dev_instance {
        return Err(ApiError::bad_request(READ_ONLY));
    }
    if !crate::accounts::PROVIDERS.contains(&body.provider.as_str()) {
        return Err(ApiError::bad_request(format!(
            "不支持的运行时 {}",
            body.provider
        )));
    }
    let known = discover(&st, &name)
        .await?
        .into_iter()
        .any(|h| h.session == body.session);
    if !known {
        return Err(ApiError::bad_request("这台机器上没有这个 hub"));
    }
    let payload = serde_json::json!({ "provider": body.provider, "mode": body.mode });
    let out = st
        .transport(&name)
        .exec(
            ExecSpec::new("bash")
                .arg("-lc")
                .arg(mode_script(&body.session, &payload)),
        )
        .await?;
    let reply: serde_json::Value = serde_json::from_str(out.stdout.trim()).unwrap_or_default();
    match reply["error"].as_str() {
        Some(e) => Err(ApiError::bad_request(e.to_owned())),
        None if reply["ok"] == serde_json::json!(true) => Ok(Json(reply)),
        None => Err(ApiError(anyhow::anyhow!(
            "那个 hub 没有确认修改：{}",
            out.stdout.trim().chars().take(200).collect::<String>()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_listings_are_parsed_per_session_file() {
        let out = "HUB\t/home/u/.local/share/blazar/hub/hub.session\thttp://127.0.0.1:7777\n\
                   {\"accounts\":[{\"id\":\"a\"}],\"modes\":{\"claude\":\"a\"}}\nEND\n\
                   HUB\t/home/u/.local/share/blazar/profiles/dev/hub.session\thttp://127.0.0.1:5\n\
                   not json\nEND\n";
        let hubs = parse_hubs(out);
        assert_eq!(hubs.len(), 1);
        assert_eq!(hubs[0].label, "后台服务");
        assert_eq!(hubs[0].url, "http://127.0.0.1:7777");
        assert_eq!(hubs[0].modes["claude"], "a");
        assert_eq!(
            label_of(
                "/Users/u/Library/Application Support/ai.blazar.Blazar/profiles/dev/hub.session"
            ),
            "桌面 · dev"
        );
        assert!(parse_hubs("").is_empty());
    }

    #[test]
    fn the_listing_script_is_valid_shell() {
        let script = list_script();
        assert!(script.contains("for f in \"$HOME/Library"));
        let out = std::process::Command::new("bash")
            .arg("-n")
            .arg("-c")
            .arg(&script)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn the_mode_script_quotes_the_session_path_and_body() {
        let s = mode_script(
            "/tmp/it's/hub.session",
            &serde_json::json!({ "provider": "claude", "mode": "auto" }),
        );
        assert!(s.contains("f='/tmp/it'\\''s/hub.session'"));
        assert!(s.contains("PUT /api/accounts/mode '{\"mode\":\"auto\",\"provider\":\"claude\"}'"));
    }

    async fn hub(dev_instance: bool) -> (Shared, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut st = crate::state::AppState::with_services(
            blazar_db::Db::open_in_memory().await.unwrap(),
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, dir.path().join("mesh")),
            crate::services::Services::Isolated,
        );
        std::sync::Arc::get_mut(&mut st).unwrap().dev_instance = dev_instance;
        (st, dir)
    }

    #[tokio::test]
    async fn the_dev_instance_only_looks_at_other_hubs() {
        let (st, _dir) = hub(false).await;
        assert_eq!(listing(&st, Vec::new())["read_only"], false);
        let (st, _dir) = hub(true).await;
        assert_eq!(listing(&st, Vec::new())["read_only"], true);
        let body = PeerMode {
            session: "/tmp/elsewhere/hub.session".into(),
            provider: "claude".into(),
            mode: "auto".into(),
        };
        let err = node_hub_mode(State(st), Path("local".into()), Json(body))
            .await
            .unwrap_err();
        assert_eq!(err.message(), READ_ONLY);
    }
}
