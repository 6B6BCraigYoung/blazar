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
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|error| anyhow::anyhow!("生成 hub 会话失败：{error}"))?;
        let token = bytes.iter().map(|b| format!("{b:02x}")).collect();
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
    let path = request.uri().path();
    let webhook = request.method() == Method::POST
        && path
            .strip_prefix("/api/webhooks/")
            .is_some_and(|token| !token.is_empty() && !token.contains('/'));
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
