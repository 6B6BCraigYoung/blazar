#![cfg(unix)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use blazar_hub::{HubConfig, build_router, build_state};
use blazar_netmesh::Invite;
use http_body_util::BodyExt;
use tower::ServiceExt;

const SECRET: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

fn fake_cli(dir: &Path) -> PathBuf {
    let state = dir.join("state");
    std::fs::create_dir_all(&state).unwrap();
    let script = format!(
        r#"#!/bin/sh
S='{state}'
echo "$*" >> "$S/args"
case "$*" in
  "-o json node")
    cat <<'J'
{{"peer_id":1,"ipv4_addr":"10.99.0.1/24","hostname":"hub-host",
 "stun_info":{{"udp_nat_type":5,"public_ip":["203.0.113.10"]}},
 "listeners":["tcp://0.0.0.0:11010"],
 "config":"mapped_listeners = [\"tcp://203.0.113.10:11010\"]\n[network_identity]\nnetwork_name = \"demo-mesh\"\nnetwork_secret = \"NETWORK-SECRET\"\n",
 "version":"2.6.4"}}
J
    ;;
  "-o json peer")
    echo '[{{"ipv4":"10.99.0.1","hostname":"hub-host","cost":"Local"}},
           {{"ipv4":"10.99.0.30","hostname":"gpu-1","cost":"p2p","lat_ms":"3.1","loss_rate":"0.0%"}}]'
    ;;
  "-o json credential generate"*)
    n=$(( $(cat "$S/n" 2>/dev/null || echo 0) + 1 )); echo $n > "$S/n"
    echo "cred-$n" >> "$S/live"
    printf '{{"credential_id":"cred-%s","credential_secret":"{SECRET}"}}\n' "$n"
    ;;
  "-o json credential list")
    printf '{{"credentials":['
    sep=''
    for id in $(cat "$S/live" 2>/dev/null); do
      printf '%s{{"credential_id":"%s","expiry_unix":4000000000,"reusable":false}}' "$sep" "$id"; sep=','
    done
    printf ']}}\n'
    ;;
  "credential revoke "*)
    grep -v "^$3\$" "$S/live" > "$S/live.new" 2>/dev/null; mv "$S/live.new" "$S/live"
    ;;
  *) echo "unexpected: $*" >&2; exit 2 ;;
esac
"#,
        state = state.display()
    );
    let p = dir.join("easytier-cli");
    std::fs::write(&p, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

struct Harness {
    app: axum::Router,
    dir: tempfile::TempDir,
}

impl Harness {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cfg = HubConfig {
            db_path: dir.path().join("blazar.sqlite"),
            bind: ([127, 0, 0, 1], 0).into(),
            mesh_via: "local".into(),
            mesh_container: None,
            engine_dir: None,
        };
        let st = build_state(&cfg).await.unwrap();
        let h = Self {
            app: build_router(st),
            dir,
        };
        let cli = fake_cli(h.dir.path());
        let (code, _) = h
            .call(
                "PUT",
                "/api/mesh/issuer",
                Some(serde_json::json!({ "via": "local", "cli": cli.display().to_string() })),
            )
            .await;
        assert_eq!(code, StatusCode::OK);
        h
    }

    async fn send(&self, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let res = self.app.clone().oneshot(req).await.unwrap();
        let code = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let v = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&bytes).into()));
        (code, v)
    }

    async fn call(
        &self,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "127.0.0.1:7777");
        let body = match body {
            Some(v) => {
                b = b.header(header::CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        self.send(b.body(body).unwrap()).await
    }

    fn state_file(&self, name: &str) -> PathBuf {
        self.dir.path().join("state").join(name)
    }
}

#[tokio::test]
async fn issue_invite_end_to_end() {
    let h = Harness::new().await;

    let (code, iss) = h.call("GET", "/api/mesh/issuer", None).await;
    assert_eq!(code, StatusCode::OK, "{iss}");
    assert_eq!(iss["reachable"], true);
    assert_eq!(iss["network_name"], "demo-mesh");
    assert_eq!(iss["entry_points"][0], "tcp://203.0.113.10:11010");
    assert!(
        !iss.to_string().contains("NETWORK-SECRET"),
        "签发节点的 network_secret 不能经 API 流出"
    );

    let (code, r) = h
        .call(
            "POST",
            "/api/mesh/invites",
            Some(serde_json::json!({ "member": "Zhang San", "days": 30 })),
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{r}");
    assert_eq!(r["file_name"], "zhang-san.blazar");
    let inv = Invite::parse(
        r["content"].as_str().unwrap(),
        chrono::Utc::now().timestamp(),
    )
    .expect("签出的文件必须能被对方的 Blazar 接受");
    assert_eq!(inv.network_name, "demo-mesh");

    assert_eq!(inv.ipv4.as_deref(), Some("10.99.0.31/24"));
    assert_eq!(inv.peers, vec!["tcp://203.0.113.10:11010"]);
    assert!(!r["summary"].to_string().contains(SECRET));

    let args = std::fs::read_to_string(h.state_file("args")).unwrap();
    let gen_line = args
        .lines()
        .find(|l| l.contains("credential generate"))
        .unwrap();
    assert!(
        gen_line.contains("--reusable false"),
        "凭据必须不可复用：{gen_line}"
    );
    assert!(gen_line.contains(&format!("--ttl {}", 30 * 86_400)));

    let (code, r2) = h
        .call(
            "POST",
            "/api/mesh/invites",
            Some(serde_json::json!({ "member": "Li Si" })),
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{r2}");
    assert_eq!(r2["summary"]["ipv4"], "10.99.0.32/24");

    let (code, _) = h
        .call(
            "POST",
            "/api/mesh/invites",
            Some(serde_json::json!({ "member": "Zhang San" })),
        )
        .await;
    assert_eq!(code, StatusCode::CONFLICT);

    let (code, _) = h
        .call(
            "POST",
            "/api/mesh/invites",
            Some(serde_json::json!({ "member": "x", "hostname": "gpu-1" })),
        )
        .await;
    assert_eq!(code, StatusCode::CONFLICT);

    let (code, _) = h
        .call(
            "POST",
            "/api/mesh/invites",
            Some(serde_json::json!({ "member": "王五" })),
        )
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);

    for f in ["blazar.sqlite", "blazar.sqlite-wal"] {
        if let Ok(bytes) = std::fs::read(h.dir.path().join(f)) {
            assert!(
                !String::from_utf8_lossy(&bytes).contains(SECRET),
                "{f} 里出现了凭据私钥"
            );
        }
    }

    let (_, list) = h.call("GET", "/api/mesh/invites", None).await;
    let states: Vec<&str> = list["invites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, vec!["waiting", "waiting"]);

    std::fs::write(h.state_file("live"), "cred-2\n").unwrap();
    let (_, list) = h.call("GET", "/api/mesh/invites", None).await;
    let by_host = |host: &str| {
        list["invites"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["hostname"] == host)
            .unwrap()["state"]
            .clone()
    };
    assert_eq!(by_host("zhang-san"), "lost");
    assert_eq!(by_host("li-si"), "waiting");

    let (code, _) = h.call("DELETE", "/api/mesh/invites/cred-2", None).await;
    assert_eq!(code, StatusCode::NO_CONTENT);
    let live = std::fs::read_to_string(h.state_file("live")).unwrap();
    assert!(!live.contains("cred-2"), "签发节点上的凭据要真的被吊销");
    let (_, list) = h.call("GET", "/api/mesh/invites", None).await;
    assert_eq!(
        list["invites"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == "cred-2")
            .unwrap()["state"],
        "revoked"
    );
    let (code, _) = h.call("DELETE", "/api/mesh/invites/nope", None).await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn preview_shows_summary_and_rejects_garbage() {
    let h = Harness::new().await;
    let (_, r) = h
        .call(
            "POST",
            "/api/mesh/invites",
            Some(serde_json::json!({ "member": "Zhang San" })),
        )
        .await;
    let content = r["content"].as_str().unwrap().to_owned();

    let (code, p) = h
        .call(
            "POST",
            "/api/mesh/join/preview",
            Some(serde_json::json!({ "invite": content })),
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{p}");
    assert_eq!(p["summary"]["hostname"], "zhang-san");
    assert!(!p.to_string().contains(SECRET), "预览不能把私钥回显给页面");

    let (code, e) = h
        .call(
            "POST",
            "/api/mesh/join/preview",
            Some(serde_json::json!({ "invite": "{\"hello\":1}" })),
        )
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(
        e["error"]
            .as_str()
            .unwrap()
            .contains("不是 Blazar 邀请文件")
    );
}

#[tokio::test]
async fn join_is_refused_from_other_machines() {
    let h = Harness::new().await;
    let mut req = Request::builder()
        .method("POST")
        .uri("/api/mesh/join")
        .header(header::HOST, "10.99.0.20:7777")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let remote: SocketAddr = "10.99.0.5:40000".parse().unwrap();
    req.extensions_mut().insert(ConnectInfo(remote));
    let (code, _) = h.send(req).await;
    assert_eq!(
        code,
        StatusCode::FORBIDDEN,
        "别的机器不能让这台机器入网 / 退网"
    );
}

#[tokio::test]
async fn cross_site_and_rebinding_requests_are_blocked() {
    let h = Harness::new().await;
    let req = |host: &str, origin: Option<&str>| {
        let mut b = Request::builder()
            .uri("/api/mesh/invites")
            .header(header::HOST, host);
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        b.body(Body::empty()).unwrap()
    };

    let (code, _) = h
        .send(req("127.0.0.1:7777", Some("http://127.0.0.1:7777")))
        .await;
    assert_eq!(code, StatusCode::OK);

    let (code, _) = h.send(req("localhost:7777", None)).await;
    assert_eq!(code, StatusCode::OK);

    let (code, _) = h
        .send(req("127.0.0.1:7777", Some("https://evil.example")))
        .await;
    assert_eq!(code, StatusCode::FORBIDDEN);

    let (code, _) = h
        .send(req("evil.example:7777", Some("http://evil.example:7777")))
        .await;
    assert_eq!(code, StatusCode::MISDIRECTED_REQUEST);

    let (code, _) = h
        .send(req("10.99.0.20:7777", Some("http://10.99.0.20:7777")))
        .await;
    assert_eq!(code, StatusCode::OK);
}

#[tokio::test]
async fn easytier_config_import_preview_and_guards() {
    let h = Harness::new().await;
    let toml = r#"
hostname = "gpu-new"
ipv4 = "10.99.0.40/24"
[network_identity]
network_name = "demo-mesh"
network_secret = "TOPSECRET"
[[peer]]
uri = "tcp://203.0.113.10:11010"
[[port_forward]]
bind_addr = "0.0.0.0:8080"
dst_addr = "10.99.0.11:80"
proto = "tcp"
"#;
    let (code, p) = h
        .call(
            "POST",
            "/api/mesh/config/preview",
            Some(serde_json::json!({ "toml": toml })),
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{p}");
    assert_eq!(p["summary"]["auth"], "network_secret");
    assert_eq!(p["summary"]["features"][0], "端口转发 ×1");
    assert!(!p.to_string().contains("TOPSECRET"), "预览不能回显密钥");

    let (code, e) = h
        .call(
            "POST",
            "/api/mesh/config/preview",
            Some(serde_json::json!({ "toml": "lisenters = []\n" })),
        )
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(e["error"].as_str().unwrap().contains("lisenters"));

    for (method, body) in [
        ("PUT", format!("{{\"toml\":{}}}", serde_json::json!(toml))),
        ("GET", String::new()),
    ] {
        let mut req = Request::builder()
            .method(method)
            .uri("/api/mesh/config")
            .header(header::HOST, "10.99.0.20:7777")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let remote: SocketAddr = "10.99.0.5:40000".parse().unwrap();
        req.extensions_mut().insert(ConnectInfo(remote));
        let (code, _) = h.send(req).await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{method} 应拒绝远端调用");
    }
}
