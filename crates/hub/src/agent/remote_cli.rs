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

pub async fn remote_version(st: &Shared, node: &str, runtime: &str) -> Option<(u64, u64, u64)> {
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

fn ssh_target(st: &Shared, node: &str) -> String {
    st.transport(node).target().to_owned()
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

async fn scp(args: &[&str]) -> Result<(), String> {
    let out = tokio::process::Command::new("scp")
        .args(["-q", "-o", "BatchMode=yes", "-o", "ConnectTimeout=15"])
        .args(args)
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

async fn scp3(src: &str, src_path: &str, dst: &str, dst_path: &str) -> Result<(), String> {
    let src = scp_remote_path(src, src_path)?;
    let dst = scp_remote_path(dst, dst_path)?;
    scp(&["-3", "--", &src, &dst]).await
}

async fn scp_up(local: &std::path::Path, dst: &str, dst_path: &str) -> Result<(), String> {
    let dst = scp_remote_path(dst, dst_path)?;
    scp(&["--", &local.to_string_lossy(), &dst]).await
}

const PLATFORM_SH: &str = r#"uname -s; uname -m
if [ -f /lib/libc.musl-x86_64.so.1 ] || [ -f /lib/libc.musl-aarch64.so.1 ] || ldd /bin/ls 2>&1 | grep -q musl; then echo musl; else echo gnu; fi"#;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Platform {
    os: &'static str,
    arch: &'static str,
    musl: bool,
}

impl Platform {
    fn claude(&self) -> String {
        format!(
            "{}-{}{}",
            self.os,
            self.arch,
            if self.musl { "-musl" } else { "" }
        )
    }

    fn codex(&self) -> (String, String) {
        let triple = match (self.os, self.arch) {
            ("darwin", "arm64") => "aarch64-apple-darwin",
            ("darwin", _) => "x86_64-apple-darwin",
            (_, "arm64") => "aarch64-unknown-linux-musl",
            _ => "x86_64-unknown-linux-musl",
        };
        (format!("{}-{}", self.os, self.arch), triple.to_owned())
    }
}

fn parse_platform(raw: &str) -> Result<Platform, String> {
    let mut lines = raw.lines().map(str::trim).filter(|l| !l.is_empty());
    let os = match lines.next() {
        Some("Linux") => "linux",
        Some("Darwin") => "darwin",
        other => return Err(format!("不支持的系统：{}", other.unwrap_or("未知"))),
    };
    let arch = match lines.next() {
        Some("x86_64" | "amd64") => "x64",
        Some("aarch64" | "arm64") => "arm64",
        other => return Err(format!("不支持的 CPU 架构：{}", other.unwrap_or("未知"))),
    };
    let musl = os == "linux" && lines.next() == Some("musl");
    Ok(Platform { os, arch, musl })
}

async fn platform(st: &Shared, node: &str) -> Result<Platform, String> {
    parse_platform(&run(st, node, PLATFORM_SH, 30).await?)
}

const CLAUDE_DIST: &str = "https://downloads.claude.ai/claude-code-releases";

fn http() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())
}

async fn claude_latest() -> Result<(u64, u64, u64), String> {
    let text = http()?
        .get(format!("{CLAUDE_DIST}/latest"))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("查不到 Claude Code 最新版本：{e}"))?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    parse_version(&text).ok_or_else(|| "Claude Code 最新版本号格式不对".to_owned())
}

async fn claude_checksum(v: &str, platform: &str) -> Result<String, String> {
    let manifest: serde_json::Value = http()?
        .get(format!("{CLAUDE_DIST}/{v}/manifest.json"))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("拿不到 Claude Code {v} 的发布清单：{e}"))?
        .bytes()
        .await
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or_else(|| format!("Claude Code {v} 的发布清单格式不对"))?;
    manifest["platforms"][platform]["checksum"]
        .as_str()
        .and_then(sha256_digest)
        .map(str::to_owned)
        .ok_or_else(|| format!("Claude Code {v} 没有 {platform} 版本"))
}

fn cache_dir(st: &Shared) -> std::path::PathBuf {
    st.mesh_ctx
        .staging_dir
        .parent()
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf)
        .join("agent-cache")
}

async fn cached_download(path: &std::path::Path, url: &str, sha256: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;

    let hex = |d: &[u8]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
    if let Ok(bytes) = tokio::fs::read(path).await
        && hex(&Sha256::digest(&bytes)) == sha256
    {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| e.to_string())?;
    }
    let part = path.with_extension("part");
    let mut resp = http()?
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("本机下载失败：{e}"))?;
    let mut file = tokio::fs::File::create(&part)
        .await
        .map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("本机下载中断：{e}"))?
    {
        hash.update(&chunk);
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    if hex(&hash.finalize()) != sha256 {
        let _ = tokio::fs::remove_file(&part).await;
        return Err("本机下载的文件校验不通过".into());
    }
    tokio::fs::rename(&part, path)
        .await
        .map_err(|e| e.to_string())
}

pub const MANAGED_BIN: &str = ".blazar/bin";

fn claude_install_script(v: &str, sha256: &str, source: &str) -> String {
    format!(
        r#"set -e
umask 022
R="$HOME/.blazar/agents/claude/{v}"
F="$R/claude.part"
mkdir -p "$R" "$HOME/{MANAGED_BIN}"
{source}
S=$( (sha256sum "$F" 2>/dev/null || shasum -a 256 "$F") | cut -d' ' -f1)
[ "$S" = {sum} ] || {{ rm -f "$F"; echo "校验失败" >&2; exit 1; }}
chmod +x "$F" && mv -f "$F" "$R/claude"
ln -sfn "$R/claude" "$HOME/{MANAGED_BIN}/claude"
"#,
        sum = shell_quote(sha256),
    )
}

async fn install_claude(st: &Shared, node: &str, want: (u64, u64, u64)) -> Result<String, String> {
    let v = fmt_version(want);
    let p = platform(st, node).await?;
    let plat = p.claude();
    let sum = claude_checksum(&v, &plat).await?;
    let url = format!("{CLAUDE_DIST}/{v}/{plat}/claude");
    let fetch = format!(
        "curl -fsSL --connect-timeout 15 --retry 2 -o \"$F\" -- {u} 2>/dev/null || wget -q -T 15 -t 2 -O \"$F\" -- {u}",
        u = shell_quote(&url)
    );
    let script = format!(
        "{}{}",
        exports(st, node).await,
        claude_install_script(&v, &sum, &fetch)
    );
    let remote_log = run(st, node, &script, 900).await.unwrap_or_else(|e| e);
    if remote_version(st, node, "claude").await == Some(want) {
        return Ok(format!(
            "已把 Claude Code {v} 装到 {node} 的 ~/.blazar（官方发布，sha256 校验通过）"
        ));
    }

    let target = ssh_target(st, node);
    let dst = format!(".blazar/agents/claude/{v}/claude.part");
    let local = cache_dir(st)
        .join("claude")
        .join(&v)
        .join(&plat)
        .join("claude");
    let pushed = match cached_download(&local, &url, &sum).await {
        Ok(()) => {
            let _ = run(
                st,
                node,
                &format!("mkdir -p ~/.blazar/agents/claude/{v}"),
                30,
            )
            .await;
            scp_up(&local, &target, &dst).await
        }
        Err(e) => Err(e),
    };
    if pushed.is_ok() {
        let _ = run(st, node, &claude_install_script(&v, &sum, ":"), 120).await;
        if remote_version(st, node, "claude").await == Some(want) {
            return Ok(format!(
                "{node} 下载不了，已由本机下载后传过去，装在 ~/.blazar（sha256 校验通过），现在是 {v}"
            ));
        }
    }

    for src in other_nodes(st, node).await {
        if remote_version(st, &src, "claude").await != Some(want)
            || platform(st, &src).await.ok().as_ref() != Some(&p)
        {
            continue;
        }
        let Ok(path) = run(
            st,
            &src,
            &format!(
                "{}\nreadlink -f \"$(command -v claude)\"",
                blazar_transport::PATH_PRELUDE
            ),
            30,
        )
        .await
        else {
            continue;
        };
        let path = path.trim();
        if !path.starts_with('/') {
            continue;
        }
        let _ = run(
            st,
            node,
            &format!("mkdir -p ~/.blazar/agents/claude/{v}"),
            30,
        )
        .await;
        if let Err(e) = scp3(&ssh_target(st, &src), path, &target, &dst).await {
            tracing::warn!(target: "blazar::remote_cli", "从 {src} 拷 claude 到 {node} 失败：{e}");
            continue;
        }
        let _ = run(st, node, &claude_install_script(&v, &sum, ":"), 120).await;
        if remote_version(st, node, "claude").await == Some(want) {
            return Ok(format!(
                "下载都没成功，已从 {src} 拷贝 {v} 到 ~/.blazar（sha256 校验通过）"
            ));
        }
    }
    Err(format!(
        "没能在 {node} 上装好 Claude Code {v}：远端下载失败（{}），本机{}，也没有别的机器装了这个版本可以拷",
        remote_log.lines().last().unwrap_or("").trim(),
        match pushed {
            Ok(()) => "传过去后校验没通过".to_owned(),
            Err(e) => format!("兜底也失败（{e}）"),
        }
    ))
}

fn codex_install_script(v: &str, triple: &str, sri_b64: &str, source: &str) -> String {
    format!(
        r#"set -e
umask 022
T={triple}
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
{source}
[ "$(openssl dgst -sha512 -binary "$TMP/pkg.tgz" | openssl base64 -A)" = {sri_b64} ] || {{ echo 校验失败; exit 1; }}
tar xzf "$TMP/pkg.tgz" -C "$TMP"
R=$HOME/.blazar/agents/codex/{v}
rm -rf "$R"; mkdir -p "$(dirname "$R")" "$HOME/{MANAGED_BIN}"
cp -a "$TMP/package/vendor/$T" "$R"
for b in "$R"/bin/*; do ln -sfn "$b" "$HOME/{MANAGED_BIN}/$(basename "$b")"; done
"#,
        sri_b64 = shell_quote(sri_b64),
    )
}

async fn codex_latest() -> Result<(u64, u64, u64), String> {
    let meta: serde_json::Value = http()?
        .get("https://registry.npmjs.org/@openai%2fcodex/latest")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("查不到 Codex 最新版本：{e}"))?
        .bytes()
        .await
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or_else(|| "Codex 发布信息格式不对".to_owned())?;
    meta["version"]
        .as_str()
        .and_then(parse_version)
        .ok_or_else(|| "Codex 最新版本号格式不对".to_owned())
}

async fn install_codex(st: &Shared, node: &str, want: (u64, u64, u64)) -> Result<String, String> {
    let v = fmt_version(want);
    let (npm_plat, triple) = platform(st, node).await?.codex();
    let meta: serde_json::Value = http()?
        .get(format!(
            "https://registry.npmjs.org/@openai%2fcodex/{v}-{npm_plat}"
        ))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("从 npm 拿不到 Codex {v} 的发布信息：{e}"))?
        .bytes()
        .await
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or_else(|| "从 npm 拿不到 Codex 的发布信息".to_owned())?;
    let (url, sri) = npm_dist(&meta)?;
    let remote_fetch = format!(
        "curl -fsSL --connect-timeout 15 --retry 2 -o \"$TMP/pkg.tgz\" -- {u} 2>/dev/null || wget -q -T 15 -t 2 -O \"$TMP/pkg.tgz\" -- {u}",
        u = shell_quote(url)
    );
    let script = format!(
        "{}{}",
        exports(st, node).await,
        codex_install_script(&v, &triple, sri, &remote_fetch)
    );
    let _ = run(st, node, &script, 900).await;
    if remote_version(st, node, "codex").await == Some(want) {
        return Ok(format!(
            "已把 Codex {v} 装到 {node} 的 ~/.blazar（npm 官方仓库，sha512 校验通过）"
        ));
    }
    let local = cache_dir(st)
        .join("codex")
        .join(&v)
        .join(&npm_plat)
        .join("pkg.tgz");
    if let Some(dir) = local.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    let downloaded = tokio::process::Command::new("curl")
        .args(["-fsSL", "--retry", "3", "-o"])
        .arg(local.with_extension("part"))
        .arg("--")
        .arg(url)
        .status()
        .await
        .is_ok_and(|s| s.success());
    if !downloaded {
        return Err(format!("没能装好 Codex {v}：远端和本机都下载不到安装包"));
    }
    let _ = tokio::fs::rename(local.with_extension("part"), &local).await;
    let remote_tmp = format!("/tmp/blazar-codex-{v}.tgz");
    scp_up(&local, &ssh_target(st, node), &remote_tmp)
        .await
        .map_err(|e| format!("没能把安装包传到 {node}：{e}"))?;
    let local_src = format!("mv {remote_tmp} \"$TMP/pkg.tgz\"");
    let _ = run(
        st,
        node,
        &codex_install_script(&v, &triple, sri, &local_src),
        600,
    )
    .await;
    if remote_version(st, node, "codex").await == Some(want) {
        Ok(format!(
            "{node} 下载不了，已由本机下载后传过去（sha512 校验通过），装在 ~/.blazar，现在是 {v}"
        ))
    } else {
        Err(format!("没能在 {node} 上装好 Codex {v}"))
    }
}

pub async fn store_agents(st: &Shared, node: &str, agents: &[blazar_runtime::DiscoveredAgent]) {
    let Ok(json) = serde_json::to_string(agents) else {
        return;
    };
    let _ = sqlx::query(
        "UPDATE nodes SET capabilities = json_set(
             CASE WHEN capabilities = '' OR capabilities IS NULL THEN '{}' ELSE capabilities END,
             '$.agents', json(?1))
         WHERE name = ?2",
    )
    .bind(json)
    .bind(node)
    .execute(st.db.pool())
    .await;
    st.emit(crate::state::ServerEvent::NodesChanged);
}

pub fn installable(runtime: &str) -> bool {
    matches!(runtime, "claude" | "codex")
}

pub async fn install(st: &Shared, node: &str, runtime: &str) -> Result<(String, String), String> {
    let want = match local_version(runtime).await {
        Some(v) => v,
        None if runtime == "claude" => claude_latest().await?,
        None if runtime == "codex" => codex_latest().await?,
        None => return Err(format!("不支持自动安装 {runtime}")),
    };
    let msg = if runtime == "claude" {
        install_claude(st, node, want).await?
    } else {
        install_codex(st, node, want).await?
    };
    if let Ok(found) = blazar_runtime::discover(&st.transport(node)).await {
        store_agents(st, node, &found).await;
    }
    Ok((fmt_version(want), msg))
}

pub async fn target_version(runtime: &str) -> Option<String> {
    match local_version(runtime).await {
        Some(v) => Some(fmt_version(v)),
        None if runtime == "claude" => claude_latest().await.ok().map(fmt_version),
        None if runtime == "codex" => codex_latest().await.ok().map(fmt_version),
        None => None,
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
    if !installable(&runtime) {
        return fail(StatusCode::BAD_REQUEST, format!("不支持安装 {runtime}"));
    }
    match install(&st, &node, &runtime).await {
        Ok((version, message)) => {
            Json(json!({ "ok": true, "version": version, "message": message })).into_response()
        }
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
    fn platforms_map_to_release_names() {
        let p = parse_platform("Linux\nx86_64\ngnu\n").unwrap();
        assert_eq!(p.claude(), "linux-x64");
        assert_eq!(p.codex().1, "x86_64-unknown-linux-musl");
        let p = parse_platform("Linux\naarch64\nmusl\n").unwrap();
        assert_eq!(p.claude(), "linux-arm64-musl");
        assert_eq!(
            p.codex(),
            ("linux-arm64".into(), "aarch64-unknown-linux-musl".into())
        );
        let p = parse_platform("Darwin\narm64\nmusl\n").unwrap();
        assert_eq!(p.claude(), "darwin-arm64");
        assert_eq!(p.codex().1, "aarch64-apple-darwin");
        assert!(parse_platform("FreeBSD\nx86_64\n").is_err());
        assert!(parse_platform("Linux\nriscv64\n").is_err());
    }

    #[test]
    fn claude_install_script_verifies_and_links_into_managed_bin() {
        use sha2::{Digest, Sha256};
        let home = tempfile::tempdir().unwrap();
        let src = home.path().join("payload");
        std::fs::write(&src, "#!/bin/sh\necho 9.8.7 '(Claude Code)'\n").unwrap();
        let sum: String = Sha256::digest(std::fs::read(&src).unwrap())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let fetch = format!("cp {} \"$F\"", shell_quote(&src.to_string_lossy()));
        let run = |script: String| {
            std::process::Command::new("bash")
                .arg("-c")
                .arg(script)
                .env("HOME", home.path())
                .output()
                .unwrap()
        };

        let bad = run(claude_install_script("9.8.7", &"0".repeat(64), &fetch));
        assert!(!bad.status.success());
        assert!(!home.path().join(".blazar/bin/claude").exists());
        assert!(
            !home
                .path()
                .join(".blazar/agents/claude/9.8.7/claude.part")
                .exists()
        );

        let ok = run(claude_install_script("9.8.7", &sum, &fetch));
        assert!(
            ok.status.success(),
            "{}",
            String::from_utf8_lossy(&ok.stderr)
        );
        let out = run(format!(
            "{}\nclaude --version",
            blazar_transport::PATH_PRELUDE
        ));
        assert_eq!(
            parse_version(&String::from_utf8_lossy(&out.stdout)),
            Some((9, 8, 7))
        );
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
