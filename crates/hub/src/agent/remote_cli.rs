use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use blazar_transport::{ExecSpec, LocalTransport, NodeTransport};
use serde_json::json;

use crate::api::Shared;

#[must_use]
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let tok = s.split_whitespace().find(|t| {
        t.trim_start_matches('v')
            .starts_with(|c: char| c.is_ascii_digit())
    })?;
    let mut it = tok
        .trim_start_matches('v')
        .split(|c: char| !c.is_ascii_digit())
        .filter(|x| !x.is_empty())
        .map(|x| x.parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next().flatten().unwrap_or(0)))
}

fn fmt_version(v: (u64, u64, u64)) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

fn sha256_digest(raw: &str) -> Option<&str> {
    let digest = raw.trim();
    (digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())).then_some(digest)
}

fn npm_dist(meta: &serde_json::Value) -> Result<(&str, &str), String> {
    use base64::Engine;

    let url = meta["dist"]["tarball"]
        .as_str()
        .ok_or("npm 发布信息里没有下载地址")?;
    let sri = meta["dist"]["integrity"]
        .as_str()
        .and_then(|s| s.strip_prefix("sha512-"))
        .ok_or("npm 发布信息里没有 sha512")?;
    let parsed = reqwest::Url::parse(url).map_err(|_| "npm 下载地址无效")?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("registry.npmjs.org")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port_or_known_default() != Some(443)
        || parsed.fragment().is_some()
    {
        return Err("npm 下载地址必须来自官方 HTTPS 仓库".into());
    }
    let digest = base64::engine::general_purpose::STANDARD
        .decode(sri)
        .map_err(|_| "npm sha512 格式无效")?;
    if digest.len() != 64 {
        return Err("npm sha512 长度无效".into());
    }
    Ok((url, sri))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

async fn local_version(runtime: &str) -> Option<(u64, u64, u64)> {
    let script = format!("{}\n{runtime} --version", blazar_transport::PATH_PRELUDE);
    let out = LocalTransport
        .exec(ExecSpec::new("bash").arg("-lc").arg(script))
        .await
        .ok()?;
    parse_version(&out.stdout)
}

async fn exports(st: &Shared, node: &str) -> String {
    crate::proxy::node_net_env(st, node)
        .await
        .iter()
        .map(|(k, v)| format!("export {k}={}\n", shell_quote(v)))
        .collect()
}

async fn run(st: &Shared, node: &str, script: &str, secs: u64) -> Result<String, String> {
    let t = st.transport(node);
    let fut = t.exec(ExecSpec::new("bash").arg("-lc").arg(script));
    match tokio::time::timeout(Duration::from_secs(secs), fut).await {
        Ok(Ok(out)) => Ok(format!("{}{}", out.stdout, out.stderr)),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("超时".into()),
    }
}

async fn remote_version(st: &Shared, node: &str, runtime: &str) -> Option<(u64, u64, u64)> {
    let script = format!(
        "{}\n{runtime} --version 2>/dev/null",
        blazar_transport::PATH_PRELUDE
    );
    parse_version(&run(st, node, &script, 30).await.ok()?)
}

async fn other_nodes(st: &Shared, node: &str) -> Vec<String> {
    sqlx::query_scalar("SELECT name FROM nodes WHERE name != ?1 AND name != 'local' AND (role IS NULL OR role != 'personal') AND (network IS NOT 'easytier' OR role = 'dev') ORDER BY name")
        .bind(node)
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default()
}

fn scp_remote_path(target: &str, path: &str) -> Result<String, String> {
    blazar_transport::validate_ssh_target(target).map_err(|e| e.to_string())?;
    let (user, host) = target
        .split_once('@')
        .map_or((None, target), |(user, host)| (Some(user), host));
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    Ok(match user {
        Some(user) => format!("{user}@{host}:{path}"),
        None => format!("{host}:{path}"),
    })
}

async fn scp3(src: &str, src_path: &str, dst: &str, dst_path: &str) -> Result<(), String> {
    let src = scp_remote_path(src, src_path)?;
    let dst = scp_remote_path(dst, dst_path)?;
    let out = tokio::process::Command::new("scp")
        .args([
            "-3",
            "-q",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=15",
            "--",
            &src,
            &dst,
        ])
        .output()
        .await
        .map_err(|e| format!("起不了 scp：{e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr)
            .trim()
            .chars()
            .take(200)
            .collect())
    }
}

async fn update_claude(st: &Shared, node: &str, want: (u64, u64, u64)) -> Result<String, String> {
    blazar_transport::validate_ssh_target(node).map_err(|e| e.to_string())?;
    let v = fmt_version(want);
    let script = format!(
        "{}{}\nclaude install {v} 2>&1 | tail -3",
        exports(st, node).await,
        blazar_transport::PATH_PRELUDE
    );
    let log = run(st, node, &script, 600).await.unwrap_or_else(|e| e);
    if remote_version(st, node, "claude").await == Some(want) {
        return Ok(format!("已用官方安装器更新到 {v}"));
    }
    let file = format!(".local/share/claude/versions/{v}");
    for src in other_nodes(st, node).await {
        if remote_version(st, &src, "claude").await != Some(want) {
            continue;
        }
        let Ok(sum) = run(st, &src, &format!("sha256sum ~/{file} | cut -d' ' -f1"), 60).await
        else {
            continue;
        };
        let Some(sum) = sha256_digest(&sum) else {
            continue;
        };
        let _ = run(
            st,
            node,
            "mkdir -p ~/.local/share/claude/versions ~/.local/bin",
            30,
        )
        .await;
        if let Err(e) = scp3(&src, &file, node, &format!("{file}.part")).await {
            tracing::warn!(target: "blazar::remote_cli", "从 {src} 拷 claude 到 {node} 失败：{e}");
            continue;
        }
        let install = format!(
            "f=~/{file}.part; [ \"$(sha256sum $f | cut -d' ' -f1)\" = {sum} ] || {{ rm -f $f; echo BAD; exit 1; }}; \
             chmod +x $f && mv $f ~/{file} && ln -sfn ~/{file} ~/.local/bin/claude",
            sum = shell_quote(sum),
        );
        if run(st, node, &install, 60)
            .await
            .is_ok_and(|o| !o.contains("BAD"))
            && remote_version(st, node, "claude").await == Some(want)
        {
            return Ok(format!(
                "官方下载没成功，已从 {src} 拷贝 {v}（sha256 校验通过）"
            ));
        }
    }
    Err(format!(
        "没能更新到 {v}：官方安装器失败（{}），也没有别的机器已经装了这个版本可以拷",
        log.lines().last().unwrap_or("").trim()
    ))
}

fn codex_install_script(v: &str, sri_b64: &str, source: &str) -> String {
    format!(
        r#"set -e
T=x86_64-unknown-linux-musl
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
{source}
[ "$(openssl dgst -sha512 -binary "$TMP/pkg.tgz" | base64 -w0)" = {sri_b64} ] || {{ echo 校验失败; exit 1; }}
tar xzf "$TMP/pkg.tgz" -C "$TMP"
R=$HOME/.codex/packages/standalone/releases/{v}-$T
rm -rf "$R"; mkdir -p "$(dirname "$R")" "$HOME/.local/bin"
cp -a "$TMP/package/vendor/$T" "$R"
ln -sfn "$R" "$HOME/.codex/packages/standalone/current"
for b in codex codex-code-mode-host; do ln -sfn "$HOME/.codex/packages/standalone/current/bin/$b" "$HOME/.local/bin/$b"; done
"#,
        sri_b64 = shell_quote(sri_b64),
    )
}

async fn update_codex(st: &Shared, node: &str, want: (u64, u64, u64)) -> Result<String, String> {
    blazar_transport::validate_ssh_target(node).map_err(|e| e.to_string())?;
    let v = fmt_version(want);
    let meta = tokio::process::Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "30",
            &format!("https://registry.npmjs.org/@openai%2fcodex/{v}-linux-x64"),
        ])
        .output()
        .await
        .map_err(|e| format!("起不了 curl：{e}"))?;
    let meta: serde_json::Value = serde_json::from_slice(&meta.stdout)
        .map_err(|_| "从 npm 拿不到 Codex 的发布信息".to_owned())?;
    let (url, sri) = npm_dist(&meta)?;
    let remote_fetch = format!(
        "curl -fsSL --retry 3 -o \"$TMP/pkg.tgz\" -- {}",
        shell_quote(url)
    );
    let script = format!(
        "{}{}",
        exports(st, node).await,
        codex_install_script(&v, sri, &remote_fetch)
    );
    let _ = run(st, node, &script, 900).await;
    if remote_version(st, node, "codex").await == Some(want) {
        return Ok(format!("已从 npm 官方仓库更新到 {v}（sha512 校验通过）"));
    }
    let tmp = std::env::temp_dir().join(format!("blazar-codex-{v}.tgz"));
    let dl = tokio::process::Command::new("curl")
        .args(["-fsSL", "--retry", "3", "-o"])
        .arg(&tmp)
        .arg("--")
        .arg(url)
        .status()
        .await;
    if !dl.is_ok_and(|s| s.success()) {
        return Err(format!("没能更新到 {v}：远端和本机都下载不到安装包"));
    }
    let up = tokio::process::Command::new("scp")
        .args(["-q", "-o", "BatchMode=yes"])
        .arg("--")
        .arg(&tmp)
        .arg(scp_remote_path(
            node,
            &format!("/tmp/blazar-codex-{v}.tgz"),
        )?)
        .status()
        .await;
    let _ = std::fs::remove_file(&tmp);
    if !up.is_ok_and(|s| s.success()) {
        return Err(format!("没能把安装包传到 {node}"));
    }
    let local_src = format!("mv /tmp/blazar-codex-{v}.tgz \"$TMP/pkg.tgz\"");
    let _ = run(st, node, &codex_install_script(&v, sri, &local_src), 600).await;
    if remote_version(st, node, "codex").await == Some(want) {
        Ok(format!(
            "远端下载没成功，已由本机下载后传过去（sha512 校验通过），现在是 {v}"
        ))
    } else {
        Err(format!("没能更新到 {v}"))
    }
}

pub async fn versions(State(st): State<Shared>, Path(node): Path<String>) -> Response {
    let mut out = serde_json::Map::new();
    for rt in ["claude", "codex"] {
        let (local, remote) = tokio::join!(local_version(rt), remote_version(&st, &node, rt));
        out.insert(
            rt.into(),
            json!({
                "local": local.map(fmt_version),
                "remote": remote.map(fmt_version),
                "behind": matches!((local, remote), (Some(l), Some(r)) if r < l),
            }),
        );
    }
    Json(serde_json::Value::Object(out)).into_response()
}

pub async fn update(
    State(st): State<Shared>,
    Path((node, runtime)): Path<(String, String)>,
) -> Response {
    let fail = |s: StatusCode, m: String| (s, Json(json!({ "error": m }))).into_response();
    let known: Option<String> = sqlx::query_scalar("SELECT name FROM nodes WHERE name = ?1")
        .bind(&node)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten();
    if known.is_none() || node == "local" {
        return fail(StatusCode::NOT_FOUND, "没有这台机器".into());
    }
    if !matches!(runtime.as_str(), "claude" | "codex") {
        return fail(StatusCode::BAD_REQUEST, format!("不支持更新 {runtime}"));
    }
    let Some(want) = local_version(&runtime).await else {
        return fail(
            StatusCode::CONFLICT,
            format!("本机没装 {runtime}，不知道该对齐到哪个版本"),
        );
    };
    let r = if runtime == "claude" {
        update_claude(&st, &node, want).await
    } else {
        update_codex(&st, &node, want).await
    };
    match r {
        Ok(msg) => Json(json!({ "ok": true, "version": fmt_version(want), "message": msg }))
            .into_response(),
        Err(e) => fail(StatusCode::BAD_GATEWAY, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_targets_are_validated_before_scp_path_generation() {
        for target in ["-invalid", "", "Alice@-invalid", "host name"] {
            assert!(scp_remote_path(target, "package.tgz").is_err());
        }
        assert_eq!(
            scp_remote_path("hub-host", "package.tgz").unwrap(),
            "hub-host:package.tgz"
        );
        assert_eq!(
            scp_remote_path("Alice@::1", "package.tgz").unwrap(),
            "Alice@[::1]:package.tgz"
        );
        assert_eq!(
            scp_remote_path("[::1]", "package.tgz").unwrap(),
            "[::1]:package.tgz"
        );
    }

    #[test]
    fn remote_checksum_requires_hexadecimal_digits() {
        let valid = "0123456789abcdef".repeat(4);
        assert_eq!(sha256_digest(&format!("{valid}\n")), Some(valid.as_str()));
        assert!(sha256_digest(&"g".repeat(64)).is_none());
        assert!(sha256_digest(&"0".repeat(63)).is_none());
    }

    #[test]
    fn npm_dist_rejects_untrusted_downloads_and_malformed_digests() {
        use base64::Engine;
        let integrity = format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode([0_u8; 64])
        );
        let official = "https://registry.npmjs.org/@openai/codex/-/codex-1.2.3-linux-x64.tgz";
        assert!(npm_dist(&json!({"dist": {"tarball": official, "integrity": integrity}})).is_ok());
        for url in [
            "http://registry.npmjs.org/package.tgz",
            "https://example.com/package.tgz",
            "https://registry.npmjs.org@example.com/package.tgz",
            "--help",
        ] {
            assert!(
                npm_dist(&json!({"dist": {"tarball": url, "integrity": integrity}})).is_err(),
                "{url}"
            );
        }
        for digest in ["sha512-short", "sha512-", "sha256-aabb"] {
            assert!(
                npm_dist(&json!({"dist": {"tarball": official, "integrity": digest}})).is_err()
            );
        }
    }

    #[test]
    fn versions_parse_and_compare() {
        assert_eq!(parse_version("2.1.37 (Claude Code)"), Some((2, 1, 37)));
        assert_eq!(parse_version("codex-cli 0.159.3"), Some((0, 159, 3)));
        assert_eq!(parse_version("v1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("command not found"), None);
        assert!(
            parse_version("2.1.37").unwrap() < parse_version("2.1.283").unwrap(),
            "按数字比，不按字符串"
        );
    }
}
