use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use blazar_transport::{ExecSpec, LocalTransport, NodeTransport};
use serde_json::json;

use crate::api::Shared;

// 远端机器上的 Claude Code / Codex 跟本机对齐版本：Blazar 拼的命令行参数是照本机 CLI 来的，远端太旧会不认。

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
        .map(|(k, v)| format!("export {k}='{}'\n", v.replace('\'', "")))
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
    sqlx::query_scalar("SELECT name FROM nodes WHERE name != ?1 AND name != 'local' ORDER BY name")
        .bind(node)
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default()
}

async fn scp3(src: &str, src_path: &str, dst: &str, dst_path: &str) -> Result<(), String> {
    let out = tokio::process::Command::new("scp")
        .args([
            "-3",
            "-q",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=15",
            &format!("{src}:{src_path}"),
            &format!("{dst}:{dst_path}"),
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

// Claude Code 原生安装就是 ~/.local/share/claude/versions/<版本> 一个文件，~/.local/bin/claude 指向它。
async fn update_claude(st: &Shared, node: &str, want: (u64, u64, u64)) -> Result<String, String> {
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
    // 官方下载经代理容易被掐断：从已经是这个版本的另一台机器经本机中转拷过来，按 sha256 校验后再切换。
    let file = format!(".local/share/claude/versions/{v}");
    for src in other_nodes(st, node).await {
        if remote_version(st, &src, "claude").await != Some(want) {
            continue;
        }
        let Ok(sum) = run(st, &src, &format!("sha256sum ~/{file} | cut -d' ' -f1"), 60).await
        else {
            continue;
        };
        let sum = sum.trim().to_owned();
        if sum.len() != 64 {
            continue;
        }
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
            "f=~/{file}.part; [ \"$(sha256sum $f | cut -d' ' -f1)\" = '{sum}' ] || {{ rm -f $f; echo BAD; exit 1; }}; \
             chmod +x $f && mv $f ~/{file} && ln -sfn ~/{file} ~/.local/bin/claude"
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

// Codex 官方的 standalone 布局：~/.codex/packages/standalone/releases/<版本>-<平台>，current 指向在用的那个。
fn codex_install_script(v: &str, sri_b64: &str, source: &str) -> String {
    format!(
        r#"set -e
T=x86_64-unknown-linux-musl
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
{source}
[ "$(openssl dgst -sha512 -binary "$TMP/pkg.tgz" | base64 -w0)" = '{sri_b64}' ] || {{ echo 校验失败; exit 1; }}
tar xzf "$TMP/pkg.tgz" -C "$TMP"
R=$HOME/.codex/packages/standalone/releases/{v}-$T
rm -rf "$R"; mkdir -p "$(dirname "$R")" "$HOME/.local/bin"
cp -a "$TMP/package/vendor/$T" "$R"
ln -sfn "$R" "$HOME/.codex/packages/standalone/current"
for b in codex codex-code-mode-host; do ln -sfn "$HOME/.codex/packages/standalone/current/bin/$b" "$HOME/.local/bin/$b"; done
"#
    )
}

async fn update_codex(st: &Shared, node: &str, want: (u64, u64, u64)) -> Result<String, String> {
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
    let url = meta["dist"]["tarball"]
        .as_str()
        .ok_or("npm 发布信息里没有下载地址")?;
    let sri = meta["dist"]["integrity"]
        .as_str()
        .and_then(|s| s.strip_prefix("sha512-"))
        .ok_or("npm 发布信息里没有 sha512")?;
    let remote_fetch = format!("curl -fsSL --retry 3 -o \"$TMP/pkg.tgz\" '{url}'");
    let script = format!(
        "{}{}",
        exports(st, node).await,
        codex_install_script(&v, sri, &remote_fetch)
    );
    let _ = run(st, node, &script, 900).await;
    if remote_version(st, node, "codex").await == Some(want) {
        return Ok(format!("已从 npm 官方仓库更新到 {v}（sha512 校验通过）"));
    }
    // 远端下载失败：本机下载后传过去。
    let tmp = std::env::temp_dir().join(format!("blazar-codex-{v}.tgz"));
    let dl = tokio::process::Command::new("curl")
        .args(["-fsSL", "--retry", "3", "-o"])
        .arg(&tmp)
        .arg(url)
        .status()
        .await;
    if !dl.is_ok_and(|s| s.success()) {
        return Err(format!("没能更新到 {v}：远端和本机都下载不到安装包"));
    }
    let up = tokio::process::Command::new("scp")
        .args(["-q", "-o", "BatchMode=yes"])
        .arg(&tmp)
        .arg(format!("{node}:/tmp/blazar-codex-{v}.tgz"))
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
