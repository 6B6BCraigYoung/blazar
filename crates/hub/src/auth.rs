use std::hash::{Hash, Hasher};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::api::Shared;

pub struct Session {
    token: String,
    cookie_name: String,
    pub path: PathBuf,
    dev_origin: Option<String>,
}

impl Session {
    pub fn new(db: &Path, dev_origin: Option<String>) -> anyhow::Result<Self> {
        let token_file = std::env::var_os("BLAZAR_HUB_TOKEN_FILE")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        Self::with_token_file(db, dev_origin, token_file.as_deref())
    }

    pub fn with_token_file(
        db: &Path,
        dev_origin: Option<String>,
        token_file: Option<&Path>,
    ) -> anyhow::Result<Self> {
        if let Some(origin) = &dev_origin {
            let uri: axum::http::Uri = origin.parse()?;
            let host = uri.host().unwrap_or_default();
            anyhow::ensure!(
                uri.scheme_str() == Some("http")
                    && (host == "localhost"
                        || host
                            .parse::<std::net::IpAddr>()
                            .is_ok_and(|ip| ip.is_loopback()))
                    && uri.authority().is_some_and(|a| a.port_u16().is_some())
                    && uri.path() == "/"
                    && uri.query().is_none(),
                "开发源必须是带端口的本机 HTTP 地址"
            );
        }
        let token = match token_file {
            Some(path) => persistent_token(path)?,
            None => random_token()?,
        };
        let mut hash = std::hash::DefaultHasher::new();
        db.hash(&mut hash);
        Ok(Self {
            token,
            cookie_name: format!("blazar_session_{:016x}", hash.finish()),
            path: db.with_file_name("hub.session"),
            dev_origin,
        })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn publish(&self, addr: SocketAddr) -> anyhow::Result<()> {
        let addr = if addr.ip().is_unspecified() {
            SocketAddr::new(
                if addr.is_ipv4() {
                    std::net::Ipv4Addr::LOCALHOST.into()
                } else {
                    std::net::Ipv6Addr::LOCALHOST.into()
                },
                addr.port(),
            )
        } else {
            addr
        };
        blazar_core_types::connection::write(
            &self.path,
            &blazar_core_types::connection::Endpoint {
                url: format!("http://{addr}"),
                token: self.token.clone(),
            },
        )?;
        Ok(())
    }

    fn matches(&self, candidate: &str) -> bool {
        candidate.len() == self.token.len()
            && candidate
                .bytes()
                .zip(self.token.bytes())
                .fold(0u8, |different, (a, b)| different | (a ^ b))
                == 0
    }

    fn authorized(&self, request: &Request) -> bool {
        if let Some(token) = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
        {
            return self.matches(token);
        }
        request
            .headers()
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .filter_map(|v| v.trim().split_once('='))
            .any(|(name, value)| name == self.cookie_name && self.matches(value))
    }

    fn origin_allowed(&self, request: &Request, host: &str) -> bool {
        let Some(origin) = request.headers().get(header::ORIGIN) else {
            return true;
        };
        let Ok(origin) = origin.to_str() else {
            return false;
        };
        if self.dev_origin.as_deref() == Some(origin) && loopback_peer(request) {
            return true;
        }
        origin.parse::<axum::http::Uri>().is_ok_and(|uri| {
            matches!(uri.scheme_str(), Some("http" | "https"))
                && uri
                    .authority()
                    .is_some_and(|a| a.as_str().eq_ignore_ascii_case(host))
                && uri.path() == "/"
                && uri.query().is_none()
        })
    }
}

fn random_token() -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| anyhow::anyhow!("生成 hub 会话失败：{error}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn valid_token(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

fn persistent_token(path: &Path) -> anyhow::Result<String> {
    use anyhow::Context;

    if let Ok(text) = std::fs::read_to_string(path)
        && valid_token(text.trim())
    {
        restrict(path)?;
        return Ok(text.trim().to_ascii_lowercase());
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建令牌目录 {} 失败", parent.display()))?;
    }
    let token = random_token()?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .with_context(|| format!("写不了令牌文件 {}", path.display()))?;
    std::io::Write::write_all(&mut file, format!("{token}\n").as_bytes())
        .with_context(|| format!("写不了令牌文件 {}", path.display()))?;
    restrict(path)?;
    tracing::info!(path = %path.display(), "已生成新的 hub 令牌");
    Ok(token)
}

fn restrict(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn loopback_peer(request: &Request) -> bool {
    request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_some_and(|ConnectInfo(peer)| peer.ip().is_loopback())
}

pub async fn guard(State(st): State<Shared>, request: Request, next: Next) -> Response {
    let Some(session) = st.auth.get() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Some(host) = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
    else {
        return StatusCode::MISDIRECTED_REQUEST.into_response();
    };
    if !host_allowed(host) {
        return StatusCode::MISDIRECTED_REQUEST.into_response();
    }
    if !session.origin_allowed(&request, host) {
        return (StatusCode::FORBIDDEN, "拒绝跨站请求").into_response();
    }
    if request
        .headers()
        .get("sec-fetch-site")
        .is_some_and(|v| v != "none" && v != "same-origin")
        && request.headers().get(header::AUTHORIZATION).is_none()
    {
        return (StatusCode::FORBIDDEN, "拒绝跨站请求").into_response();
    }
    if request.uri().path() == "/api/auth/session" && request.method() == Method::POST {
        if !session.authorized(&request) && !loopback_peer(&request) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        return (
            StatusCode::NO_CONTENT,
            [
                (
                    header::SET_COOKIE,
                    format!(
                        "{}={}; Path=/; HttpOnly; SameSite=Strict",
                        session.cookie_name, session.token
                    ),
                ),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
        )
            .into_response();
    }
    let path = request.uri().path().to_owned();
    if path == crate::fleet::PATH {
        let fleet_token = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|token| st.fleet.grant(token).is_some());
        if !fleet_token && !session.authorized(&request) {
            return (
                StatusCode::UNAUTHORIZED,
                axum::Json(serde_json::json!({"error":"调度令牌无效或已过期"})),
            )
                .into_response();
        }
    }
    let webhook = request.method() == Method::POST
        && path
            .strip_prefix("/api/webhooks/")
            .is_some_and(|token| !token.is_empty() && !token.contains('/'));
    let bearer = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);
    let master = bearer.as_deref().is_some_and(|t| session.matches(t));
    let mut request = request;
    if !master {
        strip_internal(request.headers_mut());
    }
    if path == "/api/facts/report" && request.method() == Method::POST && !master {
        let ok = match bearer.as_deref() {
            Some(t) => crate::bots::facts::report_token_matches(&st, t).await,
            None => false,
        };
        if ok {
            return next.run(request).await;
        }
    }
    if (path.starts_with("/api/") || path.starts_with("/ws/"))
        && !webhook
        && !session.authorized(&request)
    {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({"error":"会话已失效，请重新连接 Blazar"})),
        )
            .into_response();
    }
    next.run(request).await
}

const INTERNAL_HEADERS: &[&str] = &[
    crate::bots::CALLER_HEADER,
    crate::bots::jobs::SESSION_HEADER,
];

fn strip_internal(headers: &mut axum::http::HeaderMap) {
    for name in INTERNAL_HEADERS {
        headers.remove(*name);
    }
}

fn host_allowed(host: &str) -> bool {
    let Ok(authority) = host.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    if authority.as_str().contains('@') {
        return false;
    }
    let name = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']');
    name.eq_ignore_ascii_case("localhost")
        || name.parse::<std::net::IpAddr>().is_ok()
        || std::env::var("BLAZAR_ALLOWED_HOSTS").is_ok_and(|list| {
            list.split(',')
                .map(str::trim)
                .any(|h| !h.is_empty() && h.eq_ignore_ascii_case(name))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_file_keeps_the_token_across_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("blazar.sqlite");
        let file = dir.path().join("nested/hub.token");
        let first = Session::with_token_file(&db, None, Some(&file)).unwrap();
        let second = Session::with_token_file(&db, None, Some(&file)).unwrap();
        assert!(valid_token(first.token()));
        assert_eq!(first.token(), second.token());
        assert_eq!(
            std::fs::read_to_string(&file).unwrap().trim(),
            first.token()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn invalid_token_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("blazar.sqlite");
        let file = dir.path().join("hub.token");
        std::fs::write(&file, "not a token").unwrap();
        let session = Session::with_token_file(&db, None, Some(&file)).unwrap();
        assert!(valid_token(session.token()));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap().trim(),
            session.token()
        );
    }

    #[tokio::test]
    async fn only_the_master_token_may_carry_internal_identity_headers() {
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let db = blazar_db::Db::open_in_memory().await.unwrap();
        let st = crate::state::AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, dir.path().to_path_buf()),
            crate::services::Services::Isolated,
        );
        let session = Session::with_token_file(&dir.path().join("hub.sqlite"), None, None).unwrap();
        let master = session.token().to_owned();
        let cookie = format!("{}={master}", session.cookie_name);
        assert!(st.auth.set(session).is_ok());
        let app = axum::Router::new()
            .route(
                "/api/echo",
                axum::routing::get(|headers: axum::http::HeaderMap| async move {
                    INTERNAL_HEADERS
                        .iter()
                        .filter(|h| headers.contains_key(**h))
                        .count()
                        .to_string()
                }),
            )
            .layer(axum::middleware::from_fn_with_state(st.clone(), guard))
            .with_state(st);
        let seen = |auth: (header::HeaderName, String)| {
            let app = app.clone();
            async move {
                let request = Request::builder()
                    .uri("/api/echo")
                    .header(header::HOST, "127.0.0.1:7777")
                    .header(auth.0, auth.1)
                    .header(
                        crate::bots::CALLER_HEADER,
                        r#"{"bot":"b","handle":"h","role":"lead"}"#,
                    )
                    .header(crate::bots::jobs::SESSION_HEADER, "some-session")
                    .body(axum::body::Body::empty())
                    .unwrap();
                let response = app.oneshot(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let body = http_body_util::BodyExt::collect(response.into_body())
                    .await
                    .unwrap()
                    .to_bytes();
                String::from_utf8(body.to_vec()).unwrap()
            }
        };
        assert_eq!(
            seen((header::AUTHORIZATION, format!("Bearer {master}"))).await,
            "2"
        );
        assert_eq!(seen((header::COOKIE, cookie)).await, "0");
    }

    #[test]
    fn without_token_file_every_session_is_new() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("blazar.sqlite");
        let first = Session::with_token_file(&db, None, None).unwrap();
        let second = Session::with_token_file(&db, None, None).unwrap();
        assert_ne!(first.token(), second.token());
    }
}
