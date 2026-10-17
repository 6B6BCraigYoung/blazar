use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use blazar_transport::ExecSpec;
use blazar_transport::detached::{DetachedRun, RunMode, RunState};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::{ApiError, ApiResult, Shared};
use crate::remote_cli;
use crate::state::ServerEvent;

pub const DEFAULT_ISAACSIM: &str = "5.1.0.0";
pub const ISAACSIM_MIN_FREE_GB: u64 = 30;
pub const EULA_KEY: &str = "sims.eula.isaacsim";

fn record_key(sw: &str, node: &str) -> String {
    format!("sims.install.{sw}.{node}")
}

pub fn env_dir(sw: &str) -> String {
    format!("$HOME/.blazar/envs/{sw}")
}

const UV_RELEASES: &str = "https://api.github.com/repos/astral-sh/uv/releases/latest";

pub fn pick_uv(release: &Value, arch: &str) -> Option<(String, String, String)> {
    let triple = match arch {
        "x64" => "x86_64-unknown-linux-gnu",
        "arm64" => "aarch64-unknown-linux-gnu",
        _ => return None,
    };
    let name = format!("uv-{triple}.tar.gz");
    let a = release["assets"]
        .as_array()?
        .iter()
        .find(|a| a["name"].as_str() == Some(name.as_str()))?;
    let sha = a["digest"]
        .as_str()
        .and_then(|d| d.strip_prefix("sha256:"))
        .and_then(remote_cli::sha256_digest)?;
    let url = a["browser_download_url"].as_str()?;
    let ok = url
        .parse::<reqwest::Url>()
        .is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some("github.com"));
    ok.then(|| (name, url.to_owned(), sha.to_owned()))
}

fn uv_install_script(tag: &str, sha256: &str, source: &str) -> String {
    format!(
        r#"set -e
umask 022
R="$HOME/.blazar/agents/uv/{tag}"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
mkdir -p "$R" "$HOME/{bin}"
{source}
S=$( (sha256sum "$T/uv.tar.gz" 2>/dev/null || shasum -a 256 "$T/uv.tar.gz") | cut -d' ' -f1)
[ "$S" = {sum} ] || {{ echo "校验失败" >&2; exit 1; }}
tar -xzf "$T/uv.tar.gz" -C "$T"
cp -f "$T"/*/uv "$R/uv"
chmod +x "$R/uv"
ln -sfn "$R/uv" "$HOME/{bin}/uv"
"#,
        tag = blazar_transport::shell_quote(tag),
        bin = remote_cli::MANAGED_BIN,
        sum = blazar_transport::shell_quote(sha256),
    )
}

async fn has_uv(st: &Shared, node: &str) -> bool {
    remote_cli::run(
        st,
        node,
        &format!(
            "{}\ncommand -v uv >/dev/null 2>&1 && echo FOUND",
            blazar_transport::PATH_PRELUDE
        ),
        30,
    )
    .await
    .is_ok_and(|o| o.contains("FOUND"))
}

pub async fn ensure_uv(st: &Shared, node: &str) -> Result<(), String> {
    if has_uv(st, node).await {
        return Ok(());
    }
    let p = remote_cli::platform(st, node).await?;
    if p.os != "linux" {
        return Err(format!("{node} 不是 Linux，暂不支持一键安装"));
    }
    let release: Value = remote_cli::http()?
        .get(UV_RELEASES)
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "blazar")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("查不到 uv 的发布信息：{e}"))?
        .bytes()
        .await
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .ok_or("uv 发布信息格式不对")?;
    let tag = release["tag_name"].as_str().unwrap_or("latest").to_owned();
    let (name, url, sha) =
        pick_uv(&release, p.arch).ok_or_else(|| format!("没有 {} 的 uv 构建", p.arch))?;
    let fetch = format!(
        "curl -fsSL --connect-timeout 15 --max-time 300 --speed-limit 50000 --speed-time 30 --retry 1 -o \"$T/uv.tar.gz\" -- {u} 2>/dev/null || wget -q -T 30 -t 1 -O \"$T/uv.tar.gz\" -- {u}",
        u = blazar_transport::shell_quote(&url)
    );
    let script = format!(
        "{}{}",
        remote_cli::exports(st, node).await,
        uv_install_script(&tag, &sha, &fetch)
    );
    let _ = remote_cli::run(st, node, &script, 360).await;
    if has_uv(st, node).await {
        return Ok(());
    }
    let local = remote_cli::cache_dir(st).join("uv").join(&tag).join(&name);
    remote_cli::cached_download(&local, &url, &sha).await?;
    let remote_tmp = format!("/tmp/blazar-{name}");
    remote_cli::scp_up(&local, &remote_cli::ssh_target(st, node), &remote_tmp)
        .await
        .map_err(|e| format!("没能把 uv 传到 {node}：{e}"))?;
    let local_src = format!(
        "mv {} \"$T/uv.tar.gz\"",
        blazar_transport::shell_quote(&remote_tmp)
    );
    let out = remote_cli::run(st, node, &uv_install_script(&tag, &sha, &local_src), 120).await;
    if has_uv(st, node).await {
        Ok(())
    } else {
        Err(format!(
            "没能在 {node} 上装好 uv：{}",
            out.unwrap_or_else(|e| e)
                .lines()
                .last()
                .unwrap_or("")
                .trim()
        ))
    }
}

const NVIDIA_INDEX: &str = "https://pypi.nvidia.com/isaacsim/";

pub fn python_tag_for(index_html: &str, version: &str) -> Option<String> {
    let needle = format!("isaacsim-{version}-cp");
    let at = index_html.find(&needle)?;
    let rest = &index_html[at + needle.len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let (major, minor) = digits.split_at(1);
    if major.is_empty() || minor.is_empty() {
        return None;
    }
    Some(format!("{major}.{minor}"))
}

async fn isaacsim_python(version: &str) -> Option<String> {
    let html = remote_cli::http()
        .ok()?
        .get(NVIDIA_INDEX)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .ok()?
        .bytes()
        .await
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())?;
    python_tag_for(&html, version)
}

pub fn install_script(sw: &str, version: &str, python: &str) -> String {
    let env = env_dir(sw);
    let (python, pkg, extra) = match sw {
        "isaacsim" => (
            python.to_owned(),
            format!("isaacsim[all,extscache]=={version}"),
            " --extra-index-url https://pypi.nvidia.com",
        ),
        _ => (
            "python3".to_owned(),
            if version.is_empty() {
                "mujoco".to_owned()
            } else {
                format!("mujoco=={version}")
            },
            "",
        ),
    };
    format!(
        r#"set -e
export PATH="$HOME/.blazar/bin:$PATH"
export UV_PYTHON_PREFERENCE=managed
E="{env}"
echo "== 准备 Python {python}"
if [ "{python}" = python3 ]; then
  PY=$(command -v python3)
else
  uv python install {python}
  PY={python}
fi
if [ -x "$E/bin/python" ]; then
  echo "== 环境已存在：$E"
else
  echo "== 创建环境 $E"
  mkdir -p "$(dirname "$E")"
  uv venv --python "$PY" "$E"
fi
echo "== 安装 {pkg}"
export OMNI_KIT_ACCEPT_EULA=YES
uv pip install --python "$E/bin/python" {pkgq}{extra}
echo "== 完成"
"#,
        pkgq = blazar_transport::shell_quote(&pkg),
    )
}

#[derive(Debug, Deserialize)]
pub struct InstallBody {
    pub node: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub accept_eula: bool,
}

async fn preflight(st: &Shared, node: &str, sw: &str) -> Result<(), String> {
    let out = remote_cli::run(
        st,
        node,
        "uname -s; uname -m; df -Pk \"$HOME\" | awk 'NR==2{print int($4/1048576)}'",
        30,
    )
    .await?;
    let mut lines = out.lines().map(str::trim);
    let os = lines.next().unwrap_or("");
    let arch = lines.next().unwrap_or("");
    let free: u64 = lines.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    if os != "Linux" {
        return Err(format!("{node} 不是 Linux，暂不支持一键安装"));
    }
    if sw == "isaacsim" {
        if arch != "x86_64" {
            return Err(format!("Isaac Sim 只有 x86_64 的包，{node} 是 {arch}"));
        }
        if free < ISAACSIM_MIN_FREE_GB {
            return Err(format!(
                "{node} 的主目录只剩 {free} GB，Isaac Sim 至少要 {ISAACSIM_MIN_FREE_GB} GB"
            ));
        }
    }
    Ok(())
}

pub async fn start(
    State(st): State<Shared>,
    Path(sw): Path<String>,
    Json(body): Json<InstallBody>,
) -> ApiResult<Json<Value>> {
    let software = super::find(&sw).ok_or_else(|| ApiError::bad_request("没有这个软件"))?;
    let node = body.node.trim().to_owned();
    if node.is_empty() || node == "local" {
        return Err(ApiError::bad_request("请选一台远端机器"));
    }
    let version = body.version.trim().to_owned();
    if !version.is_empty() && !version.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return Err(ApiError::bad_request("版本号只能是数字和点"));
    }
    if software.id == "isaacsim" {
        if !body.accept_eula {
            return Err(ApiError::bad_request(
                "安装 Isaac Sim 前要先接受 NVIDIA 的软件许可协议",
            ));
        }
        crate::office::kv_put(&st, EULA_KEY, &json!(true))
            .await
            .map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;
    }
    if let Some(v) = state_view(&st, &sw, &node).await
        && v["state"] == "running"
    {
        return Err(ApiError::bad_request("这台机器上已经在装了"));
    }
    preflight(&st, &node, software.id)
        .await
        .map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;
    ensure_uv(&st, &node)
        .await
        .map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;
    let transport = st.transport(&node);
    let home = super::home_of(&transport)
        .await
        .map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;
    let version = if version.is_empty() && software.id == "isaacsim" {
        DEFAULT_ISAACSIM.to_owned()
    } else {
        version
    };
    let python = if software.id == "isaacsim" {
        isaacsim_python(&version).await.ok_or_else(|| {
            ApiError::bad_request(format!(
                "NVIDIA 的软件源里没有 Isaac Sim {version} 的 Linux x86_64 包"
            ))
        })?
    } else {
        "python3".to_owned()
    };
    let script = format!(
        "{}{}",
        remote_cli::exports(&st, &node).await,
        install_script(software.id, &version, &python)
    );
    let run_id = format!("install-{}", software.id);
    let spec = ExecSpec::new("bash").arg("-lc").arg(script);
    let root = std::path::PathBuf::from(format!("{home}/.blazar/sims"));
    let run = DetachedRun::launch_in(transport, &root, &run_id, &spec, None, RunMode::Null)
        .await
        .map_err(|e| ApiError::from(anyhow::anyhow!("在 {node} 上开始安装失败：{e}")))?;
    crate::office::kv_put(
        &st,
        &record_key(software.id, &node),
        &json!({ "run_dir": run.dir, "run_id": run_id, "version": version, "started_at": Utc::now().to_rfc3339() }),
    )
    .await
    .map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;
    st.emit(ServerEvent::NodesChanged);
    Ok(Json(
        json!({ "ok": true, "node": node, "version": version }),
    ))
}

async fn state_view(st: &Shared, sw: &str, node: &str) -> Option<Value> {
    let rec = crate::office::kv_get(st, &record_key(sw, node)).await;
    let dir = rec["run_dir"].as_str()?.to_owned();
    let run_id = rec["run_id"].as_str().unwrap_or("install").to_owned();
    let run = DetachedRun::attach(st.transport(node), dir, run_id);
    let (state, code) = match run.probe().await {
        Ok(RunState::Alive { .. }) => ("running", None),
        Ok(RunState::Exited { code, .. }) => ("exited", Some(code)),
        Ok(RunState::Lost { .. } | RunState::Missing) => ("lost", None),
        Err(_) => ("unreachable", None),
    };
    let log = run.drain(0).await.unwrap_or_default();
    let lines: Vec<&str> = log
        .lines()
        .filter(|l| !l.starts_with("{\"type\":\"blazar_exit\""))
        .collect();
    let start = lines.len().saturating_sub(40);
    Some(json!({
        "state": state,
        "exit_code": code,
        "version": rec["version"],
        "started_at": rec["started_at"],
        "log": lines[start..].join("\n"),
    }))
}

pub async fn status(
    State(st): State<Shared>,
    Path((sw, node)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    super::find(&sw).ok_or_else(|| ApiError::bad_request("没有这个软件"))?;
    Ok(Json(
        state_view(&st, &sw, &node)
            .await
            .unwrap_or_else(|| json!({ "state": "none" })),
    ))
}

pub async fn cancel(
    State(st): State<Shared>,
    Path((sw, node)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    super::find(&sw).ok_or_else(|| ApiError::bad_request("没有这个软件"))?;
    let rec = crate::office::kv_get(&st, &record_key(&sw, &node)).await;
    let dir = rec["run_dir"]
        .as_str()
        .ok_or_else(|| ApiError::bad_request("这台机器上没有在装"))?
        .to_owned();
    let run_id = rec["run_id"].as_str().unwrap_or("install").to_owned();
    DetachedRun::attach(st.transport(&node), dir, run_id)
        .hard_kill()
        .await
        .map_err(|e| ApiError::from(anyhow::anyhow!("取消失败：{e}")))?;
    st.emit(ServerEvent::NodesChanged);
    Ok(Json(json!({ "ok": true })))
}

pub async fn eula_accepted(st: &Shared) -> bool {
    crate::office::kv_get(st, EULA_KEY).await == json!(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uv_asset_must_match_arch_and_carry_a_digest() {
        let d = format!("sha256:{}", "c".repeat(64));
        let r = json!({ "tag_name": "0.12.24", "assets": [
            { "name": "uv-x86_64-unknown-linux-gnu.tar.gz", "digest": d, "browser_download_url": "https://github.com/astral-sh/uv/releases/download/0.12.24/uv-x86_64-unknown-linux-gnu.tar.gz" },
            { "name": "uv-aarch64-unknown-linux-gnu.tar.gz", "digest": null, "browser_download_url": "https://github.com/x/a.tar.gz" },
        ]});
        let (name, url, sha) = pick_uv(&r, "x64").unwrap();
        assert_eq!(name, "uv-x86_64-unknown-linux-gnu.tar.gz");
        assert!(url.starts_with("https://github.com/astral-sh/uv/"));
        assert_eq!(sha.len(), 64);
        assert!(pick_uv(&r, "arm64").is_none(), "没有校验值的不能用");
        assert!(pick_uv(&r, "riscv").is_none());
    }

    #[test]
    fn install_scripts_pin_python_and_accept_eula_only_for_isaacsim() {
        let s = install_script("isaacsim", "5.1.0.0", "3.11");
        assert!(s.contains("uv python install 3.11"));
        assert!(s.contains(
            "'isaacsim[all,extscache]==5.1.0.0' --extra-index-url https://pypi.nvidia.com"
        ));
        assert!(s.contains("OMNI_KIT_ACCEPT_EULA=YES"));
        assert!(s.contains("$HOME/.blazar/envs/isaacsim"));
        let m = install_script("mujoco", "", "python3");
        assert!(m.contains("PY=$(command -v python3)"));
        assert!(m.contains("uv pip install --python \"$E/bin/python\" 'mujoco'\n"));
        assert!(!m.contains("pypi.nvidia.com"));
        assert!(install_script("mujoco", "3.3.0", "python3").contains("'mujoco==3.3.0'"));
    }

    #[test]
    fn python_version_is_read_from_the_wheel_tag() {
        let html = r#"<a href="isaacsim-5.1.0.0-cp311-none-manylinux_2_35_x86_64.whl#sha256=ad">x</a>
<a href="isaacsim-6.1.0.0-cp312-none-manylinux_2_35_x86_64.whl#sha256=bb">y</a>"#;
        assert_eq!(python_tag_for(html, "5.1.0.0").as_deref(), Some("3.11"));
        assert_eq!(python_tag_for(html, "6.1.0.0").as_deref(), Some("3.12"));
        assert!(python_tag_for(html, "4.5.0.0").is_none());
    }

    #[test]
    fn install_script_is_valid_bash_and_skips_an_existing_env() {
        let home = tempfile::tempdir().unwrap();
        let env = home.path().join(".blazar/envs/mujoco/bin");
        std::fs::create_dir_all(&env).unwrap();
        std::fs::write(env.join("python"), "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(env.join("python"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let bin = home.path().join(".blazar/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("uv"), "#!/bin/sh\necho \"uv $*\"\n").unwrap();
        std::fs::set_permissions(bin.join("uv"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(install_script("mujoco", "", "python3"))
            .env("HOME", home.path())
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{text}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(text.contains("环境已存在"));
        assert!(text.contains("uv pip install"));
        assert!(!text.contains("uv venv"));
    }
}
