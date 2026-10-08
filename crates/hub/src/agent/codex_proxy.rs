use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures::TryStreamExt;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::api::Shared;

const UPSTREAM: &str = "https://chatgpt.com";
const REFRESH_URL: &str = "https://auth.openai.com/oauth/token";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTH_CLAIM: &str = "https://api.openai.com/auth";
const MAX_BODY: usize = 64 * 1024 * 1024;
const PLACEHOLDER_SECS: i64 = 10 * 365 * 24 * 3600;
pub const PROVIDER: &str = "blazar";

static REFRESH: Mutex<()> = Mutex::const_new(());

pub fn is_codex_path(path: &str) -> bool {
    path.starts_with("/backend-api/") || path == "/oauth/token"
}

pub fn ca_params() -> Result<rcgen::CertificateParams, String> {
    let mut p = rcgen::CertificateParams::new(Vec::<String>::new()).map_err(|e| e.to_string())?;
    p.distinguished_name
        .push(rcgen::DnType::CommonName, "Blazar Local Proxy CA");
    p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    p.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
    ];
    Ok(p)
}

pub struct Tls {
    pub acceptor: tokio_rustls::TlsAcceptor,
    pub ca_pem: String,
}

pub fn tls(ca_key_pem: &str) -> Result<Tls, String> {
    let ca_key = rcgen::KeyPair::from_pem(ca_key_pem).map_err(|e| e.to_string())?;
    let params = ca_params()?;
    let ca_pem = params
        .self_signed(&ca_key)
        .map_err(|e| e.to_string())?
        .pem();
    let issuer = rcgen::Issuer::new(params, ca_key);
    let mut leaf =
        rcgen::CertificateParams::new(vec!["localhost".to_owned()]).map_err(|e| e.to_string())?;
    leaf.subject_alt_names.push(rcgen::SanType::IpAddress(
        std::net::Ipv4Addr::LOCALHOST.into(),
    ));
    leaf.distinguished_name
        .push(rcgen::DnType::CommonName, "127.0.0.1");
    leaf.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    let leaf_key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = leaf
        .signed_by(&leaf_key, &issuer)
        .map_err(|e| e.to_string())?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(leaf_key.serialize_der()).into(),
        )
        .map_err(|e| e.to_string())?;
    Ok(Tls {
        acceptor: tokio_rustls::TlsAcceptor::from(Arc::new(config)),
        ca_pem,
    })
}

pub fn home_of(config_dir: Option<&str>) -> Option<PathBuf> {
    config_dir
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::accounts::default_home("codex"))
}

fn claims(jwt: &str) -> Option<Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn jwt(payload: &Value) -> String {
    let header = URL_SAFE_NO_PAD.encode(json!({ "alg": "none", "typ": "JWT" }).to_string());
    let body = URL_SAFE_NO_PAD.encode(payload.to_string());
    format!("{header}.{body}.blazar")
}

async fn read_auth(home: &Path) -> Result<Value, String> {
    let raw = tokio::fs::read(home.join("auth.json"))
        .await
        .map_err(|_| "这个 Codex 账号还没登录（找不到 auth.json）".to_owned())?;
    serde_json::from_slice(&raw).map_err(|_| "这个 Codex 账号的 auth.json 格式不对".to_owned())
}

fn token<'a>(auth: &'a Value, key: &str) -> Option<&'a str> {
    auth["tokens"][key].as_str().filter(|s| !s.is_empty())
}

pub fn placeholder(secret: &str, auth: &Value) -> Result<Value, String> {
    let account = token(auth, "account_id").ok_or("这个 Codex 账号不是 ChatGPT 登录")?;
    let plan = token(auth, "id_token").and_then(claims).and_then(|c| {
        c[AUTH_CLAIM]["chatgpt_plan_type"]
            .as_str()
            .map(str::to_owned)
    });
    let now = chrono::Utc::now();
    let exp = now.timestamp() + PLACEHOLDER_SECS;
    let claim = json!({
        "chatgpt_account_id": account,
        "chatgpt_plan_type": plan,
        "chatgpt_user_id": "blazar",
        "user_id": "blazar",
    });
    let id_token = jwt(&json!({
        "email": "blazar@localhost",
        "sub": "blazar",
        "iat": now.timestamp(),
        "exp": exp,
        AUTH_CLAIM: claim,
    }));
    let access_token = jwt(&json!({
        "iat": now.timestamp(),
        "exp": exp,
        "blazar": secret,
        AUTH_CLAIM: claim,
    }));
    Ok(json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": {
            "id_token": id_token,
            "access_token": access_token,
            "refresh_token": format!("blazar:{secret}"),
            "account_id": account,
        },
        "last_refresh": now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    }))
}

async fn account_home(st: &Shared, key: &[u8], secret: &str) -> Option<PathBuf> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT id, config_dir FROM accounts WHERE provider = 'codex' AND disabled = 0",
    )
    .fetch_all(st.db.pool())
    .await
    .ok()?;
    let (_, dir) = rows
        .into_iter()
        .find(|(id, _)| crate::proxy::secret_for(key, id) == secret)?;
    home_of(dir.as_deref())
}

pub async fn remote_auth(st: &Shared, account: &str) -> Result<String, String> {
    let dir: Option<Option<String>> =
        sqlx::query_scalar("SELECT config_dir FROM accounts WHERE id = ?1 AND provider = 'codex'")
            .bind(account)
            .fetch_optional(st.db.pool())
            .await
            .map_err(|e| e.to_string())?;
    let home = home_of(dir.ok_or("没有这个 Codex 账号")?.as_deref())
        .ok_or("找不到这个 Codex 账号的目录")?;
    let auth = read_auth(&home).await?;
    let secret = crate::proxy::secret(st, account).await?;
    Ok(placeholder(&secret, &auth)?.to_string())
}

async fn refresh(client: &reqwest::Client, home: &Path, used: &str) -> Result<(), String> {
    let _guard = REFRESH.lock().await;
    let mut auth = read_auth(home).await?;
    if token(&auth, "access_token") != Some(used) {
        return Ok(());
    }
    let rt = token(&auth, "refresh_token").ok_or("Codex 账号没有 refresh token，需要重新登录")?;
    let resp = client
        .post(REFRESH_URL)
        .header("content-type", "application/json")
        .body(
            json!({
                "client_id": CLIENT_ID,
                "grant_type": "refresh_token",
                "refresh_token": rt,
                "scope": "openid profile email",
            })
            .to_string(),
        )
        .send()
        .await
        .map_err(|e| format!("刷新 Codex 登录失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "刷新 Codex 登录失败（{}），需要在本机重新登录这个账号",
            resp.status()
        ));
    }
    let fresh: Value = resp
        .bytes()
        .await
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .ok_or("刷新 Codex 登录的返回格式不对")?;
    for key in ["id_token", "access_token", "refresh_token"] {
        if let Some(v) = fresh[key].as_str().filter(|v| !v.is_empty()) {
            auth["tokens"][key] = json!(v);
        }
    }
    auth["last_refresh"] =
        json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    write_private(
        &home.join("auth.json"),
        &serde_json::to_vec_pretty(&auth).map_err(|e| e.to_string())?,
    )
    .await
}

async fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("json.blazar-tmp");
    let mut opts = tokio::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    opts.mode(0o600);
    let mut f = opts.open(&tmp).await.map_err(|e| e.to_string())?;
    tokio::io::AsyncWriteExt::write_all(&mut f, data)
        .await
        .map_err(|e| e.to_string())?;
    f.sync_all().await.map_err(|e| e.to_string())?;
    drop(f);
    tokio::fs::rename(&tmp, path)
        .await
        .map_err(|e| e.to_string())
}

fn deny(status: StatusCode, msg: &str) -> Response {
    (status, axum::Json(json!({ "error": { "message": msg } }))).into_response()
}

pub async fn forward(st: &Shared, client: &reqwest::Client, key: &[u8], req: Request) -> Response {
    let path = req
        .uri()
        .path_and_query()
        .map_or("/", |x| x.as_str())
        .to_owned();
    let method = req.method().clone();
    let mut headers = req.headers().clone();
    let presented = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(claims)
        .and_then(|c| c["blazar"].as_str().map(str::to_owned));
    let body = match axum::body::to_bytes(req.into_body(), MAX_BODY).await {
        Ok(b) => b,
        Err(e) => return deny(StatusCode::PAYLOAD_TOO_LARGE, &e.to_string()),
    };
    if path == "/oauth/token" {
        return renew(st, key, &body).await;
    }
    let Some(secret) = presented else {
        return deny(
            StatusCode::UNAUTHORIZED,
            "Blazar 凭据代理没认出这个 Codex 账号",
        );
    };
    let Some(home) = account_home(st, key, &secret).await else {
        return deny(
            StatusCode::UNAUTHORIZED,
            "Blazar 不认识这个账号暗号（账号可能已删除或停用）",
        );
    };
    for h in [
        "host",
        "authorization",
        "content-length",
        "connection",
        "proxy-authorization",
        "transfer-encoding",
        "chatgpt-account-id",
    ] {
        headers.remove(h);
    }
    let url = format!("{UPSTREAM}{path}");
    let mut retried = false;
    loop {
        let auth = match read_auth(&home).await {
            Ok(a) => a,
            Err(e) => return deny(StatusCode::UNAUTHORIZED, &e),
        };
        let (Some(access), Some(account)) =
            (token(&auth, "access_token"), token(&auth, "account_id"))
        else {
            return deny(StatusCode::UNAUTHORIZED, "这个 Codex 账号不是 ChatGPT 登录");
        };
        let up = client
            .request(method.clone(), &url)
            .headers(headers.clone())
            .bearer_auth(access)
            .header("chatgpt-account-id", account)
            .body(body.clone())
            .send()
            .await;
        let up = match up {
            Ok(r) => r,
            Err(e) => {
                return deny(
                    StatusCode::BAD_GATEWAY,
                    &format!("Blazar 连不上 ChatGPT：{e}"),
                );
            }
        };
        if up.status() == StatusCode::UNAUTHORIZED && !retried {
            retried = true;
            if let Err(e) = refresh(client, &home, access).await {
                tracing::warn!(target: "blazar::proxy", "{e}");
                return deny(StatusCode::UNAUTHORIZED, &e);
            }
            continue;
        }
        let mut out = Response::builder().status(up.status());
        for (k, v) in up.headers() {
            if !matches!(
                k.as_str(),
                "connection" | "transfer-encoding" | "content-length"
            ) {
                out = out.header(k, v);
            }
        }
        let stream = up.bytes_stream().map_err(std::io::Error::other);
        return out
            .body(Body::from_stream(stream))
            .unwrap_or_else(|_| deny(StatusCode::BAD_GATEWAY, "转发响应失败"));
    }
}

async fn renew(st: &Shared, key: &[u8], body: &[u8]) -> Response {
    let presented = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v["refresh_token"].as_str().map(str::to_owned))
        .and_then(|t| t.strip_prefix("blazar:").map(str::to_owned));
    let Some(secret) = presented else {
        return deny(
            StatusCode::UNAUTHORIZED,
            "Blazar 凭据代理没认出这个 Codex 账号",
        );
    };
    let Some(home) = account_home(st, key, &secret).await else {
        return deny(StatusCode::UNAUTHORIZED, "Blazar 不认识这个账号暗号");
    };
    let fresh = match read_auth(&home)
        .await
        .and_then(|auth| placeholder(&secret, &auth))
    {
        Ok(v) => v,
        Err(e) => return deny(StatusCode::UNAUTHORIZED, &e),
    };
    axum::Json(json!({
        "id_token": fresh["tokens"]["id_token"],
        "access_token": fresh["tokens"]["access_token"],
        "refresh_token": fresh["tokens"]["refresh_token"],
    }))
    .into_response()
}

pub struct Remote {
    pub env: Vec<(String, String)>,
    pub args: Vec<String>,
}

fn remote_setup_script(account: &str) -> String {
    format!(
        r#"set -e
umask 077
D="$HOME/.blazar/codex/{account}"
mkdir -p "$D"
IFS= read -r A; IFS= read -r C
printf '%s' "$A" | base64 --decode > "$D/auth.json.tmp"
mv -f "$D/auth.json.tmp" "$D/auth.json"
printf '%s' "$C" | base64 --decode > "$D/blazar-ca.pem"
for f in config.toml AGENTS.md skills prompts rules sessions; do
  if [ -e "$HOME/.codex/$f" ] && [ ! -e "$D/$f" ]; then ln -s "$HOME/.codex/$f" "$D/$f"; fi
done
printf '__BLAZAR_CODEX_HOME__ %s\n' "$D"
"#
    )
}

pub async fn prepare_remote(
    st: &Shared,
    node: &str,
    account: &str,
    port: u16,
) -> Result<Remote, String> {
    if account.is_empty()
        || !account
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(format!("Codex 账号 id 不合法：{account:?}"));
    }
    let auth = remote_auth(st, account).await?;
    let ca = crate::proxy::ca_pem(st).await?;
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    let out = st
        .transport(node)
        .exec(
            blazar_transport::ExecSpec::new("bash")
                .arg("-lc")
                .arg(remote_setup_script(account))
                .stdin(format!("{}\n{}\n", b64(&auth), b64(&ca))),
        )
        .await
        .map_err(|e| format!("在 {node} 上准备 Codex 账号失败：{e}"))?;
    let home = out
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("__BLAZAR_CODEX_HOME__ "))
        .map(str::trim)
        .filter(|d| d.starts_with('/'))
        .ok_or_else(|| {
            format!(
                "在 {node} 上准备 Codex 账号失败：{}",
                out.stderr.trim().chars().take(200).collect::<String>()
            )
        })?
        .to_owned();
    Ok(Remote {
        env: vec![
            ("CODEX_HOME".into(), home.clone()),
            (
                "CODEX_CA_CERTIFICATE".into(),
                format!("{home}/blazar-ca.pem"),
            ),
            (
                "CODEX_REFRESH_TOKEN_URL_OVERRIDE".into(),
                format!("https://127.0.0.1:{port}/oauth/token"),
            ),
        ],
        args: launch_args(port),
    })
}

pub fn launch_args(port: u16) -> Vec<String> {
    let base = format!("https://127.0.0.1:{port}/backend-api");
    let q = |s: &str| format!("\"{s}\"");
    [
        format!("chatgpt_base_url={}", q(&format!("{base}/"))),
        format!("model_provider={}", q(PROVIDER)),
        format!("model_providers.{PROVIDER}.name={}", q("OpenAI")),
        format!(
            "model_providers.{PROVIDER}.base_url={}",
            q(&format!("{base}/codex"))
        ),
        format!("model_providers.{PROVIDER}.wire_api={}", q("responses")),
        format!("model_providers.{PROVIDER}.requires_openai_auth=true"),
        format!("model_providers.{PROVIDER}.supports_websockets=false"),
        format!(
            "model_providers.{PROVIDER}.model_catalog_url={}",
            q(&format!("{base}/codex/models"))
        ),
    ]
    .into_iter()
    .flat_map(|kv| ["-c".to_owned(), kv])
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_auth() -> Value {
        let id = jwt(
            &json!({ AUTH_CLAIM: { "chatgpt_plan_type": "pro", "chatgpt_account_id": "acct-1" }, "email": "real@example.com" }),
        );
        json!({
            "auth_mode": "chatgpt",
            "tokens": { "id_token": id, "access_token": "real-access", "refresh_token": "real-refresh", "account_id": "acct-1" },
            "last_refresh": "2026-01-01T00:00:00Z",
        })
    }

    #[test]
    fn placeholder_carries_no_real_credentials() {
        let p = placeholder("blz-abc", &sample_auth()).unwrap();
        let text = p.to_string();
        for leaked in ["real-access", "real-refresh", "real@example.com"] {
            assert!(
                !text.contains(leaked),
                "{leaked} 不能出现在远端的 auth.json 里"
            );
        }
        assert_eq!(p["tokens"]["account_id"], "acct-1");
        assert_eq!(p["tokens"]["refresh_token"], "blazar:blz-abc");
        let at = claims(p["tokens"]["access_token"].as_str().unwrap()).unwrap();
        assert_eq!(at["blazar"], "blz-abc");
        assert_eq!(at[AUTH_CLAIM]["chatgpt_plan_type"], "pro");
        let id = claims(p["tokens"]["id_token"].as_str().unwrap()).unwrap();
        assert_eq!(id[AUTH_CLAIM]["chatgpt_account_id"], "acct-1");
    }

    #[test]
    fn api_key_accounts_have_no_placeholder() {
        assert!(placeholder("blz", &json!({ "OPENAI_API_KEY": "sk-x" })).is_err());
    }

    #[test]
    fn remote_setup_writes_only_into_the_private_home() {
        let home = tempfile::tempdir().unwrap();
        let shared = home.path().join(".codex");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("config.toml"), "model = \"x\"\n").unwrap();
        std::fs::write(shared.join("auth.json"), "remote-own-login").unwrap();
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let run = || {
            let mut child = std::process::Command::new("bash")
                .arg("-c")
                .arg(remote_setup_script("codex-default"))
                .env("HOME", home.path())
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            std::io::Write::write_all(
                child.stdin.as_mut().unwrap(),
                format!("{}\n{}\n", b64("{\"tokens\":{}}"), b64("CA")).as_bytes(),
            )
            .unwrap();
            child.wait_with_output().unwrap()
        };
        let out = run();
        assert!(out.status.success());
        assert!(run().status.success(), "重复准备不能失败");
        let d = home.path().join(".blazar/codex/codex-default");
        assert!(
            String::from_utf8_lossy(&out.stdout)
                .contains(&format!("__BLAZAR_CODEX_HOME__ {}", d.display()))
        );
        assert_eq!(
            std::fs::read_to_string(d.join("auth.json")).unwrap(),
            "{\"tokens\":{}}"
        );
        assert_eq!(
            std::fs::read_to_string(d.join("blazar-ca.pem")).unwrap(),
            "CA"
        );
        assert!(
            std::fs::symlink_metadata(d.join("config.toml"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(shared.join("auth.json")).unwrap(),
            "remote-own-login",
            "远端自己的登录不能被动"
        );
    }

    #[test]
    fn only_chatgpt_paths_go_to_codex() {
        assert!(is_codex_path("/backend-api/codex/responses"));
        assert!(is_codex_path("/oauth/token"));
        assert!(!is_codex_path("/v1/messages"));
    }

    #[test]
    fn launch_args_point_codex_at_the_tunnel() {
        let args = launch_args(4242);
        assert!(args.chunks(2).all(|p| p[0] == "-c"));
        assert!(args.contains(&"model_provider=\"blazar\"".to_owned()));
        assert!(
            args.contains(
                &"model_providers.blazar.base_url=\"https://127.0.0.1:4242/backend-api/codex\""
                    .to_owned()
            )
        );
        assert!(args.contains(&"model_providers.blazar.supports_websockets=false".to_owned()));
    }

    #[tokio::test]
    async fn a_restarted_hub_is_still_trusted_by_running_sessions() {
        use rustls::pki_types::pem::PemObject;
        let key = rcgen::KeyPair::generate().unwrap().serialize_pem();
        let before = tls(&key).unwrap();
        let after = tls(&key).unwrap();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let acceptor = after.acceptor.clone();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut tls = acceptor.accept(tcp).await.unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut tls, b"ok")
                .await
                .unwrap();
            tokio::io::AsyncWriteExt::shutdown(&mut tls).await.unwrap();
        });
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(
                rustls::pki_types::CertificateDer::from_pem_slice(before.ca_pem.as_bytes())
                    .unwrap(),
            )
            .unwrap();
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let name = rustls::pki_types::ServerName::IpAddress(std::net::Ipv4Addr::LOCALHOST.into());
        let mut tls = connector
            .connect(name, tcp)
            .await
            .expect("旧 CA 必须认新证书");
        let mut got = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut tls, &mut got)
            .await
            .unwrap();
        assert_eq!(got, b"ok");
    }
}
