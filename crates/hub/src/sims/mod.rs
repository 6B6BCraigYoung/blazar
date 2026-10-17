use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use blazar_transport::detached::{DetachedRun, RunMode, RunState};
use blazar_transport::{ExecSpec, NodeTransport};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::api::{ApiError, ApiResult, Shared};
use crate::state::ServerEvent;

pub mod display;
pub mod ffmpeg;
pub mod install;
pub mod stream;

pub struct Software {
    pub id: &'static str,
    pub label: &'static str,
    pub desc: &'static str,
    pub standalone: &'static [&'static str],
}

pub const SOFTWARE: &[Software] = &[
    Software {
        id: "isaacsim",
        label: "Isaac Sim",
        desc: "在工作区所在的机器上启动 Isaac Sim，在这里看高清画面。",
        standalone: &["isaac-sim.sh", "isaac-sim.selector.sh"],
    },
    Software {
        id: "mujoco",
        label: "MuJoCo",
        desc: "在工作区所在的机器上打开 MuJoCo 查看器，在这里看画面。",
        standalone: &[],
    },
];

#[derive(Default)]
pub struct SimsState {
    launches: Mutex<()>,
}

pub fn find(id: &str) -> Option<&'static Software> {
    SOFTWARE.iter().find(|s| s.id == id)
}

const LAUNCHES_KEY: &str = "sims.launches";

fn path_key(sw: &str, node: &str) -> String {
    format!("sims.path.{sw}.{node}")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub software: String,
    pub source: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub version: String,
}

const SCAN_SH: &str = r#"
scan_env() {
  e=$1; src=$2
  [ -d "$e/bin" ] || return 0
  sp=$(ls -d "$e"/lib/python3*/site-packages 2>/dev/null | head -1)
  [ -n "$sp" ] || return 0
  if [ -x "$e/bin/isaacsim" ]; then
    v=$(ls -d "$sp"/isaacsim-*.dist-info 2>/dev/null | head -1 | sed 's/.*isaacsim-//; s/\.dist-info$//')
    printf 'CAND\tisaacsim\t%s\t%s\t\t%s\n' "$src" "$e/bin/isaacsim" "$v"
  fi
  if [ -d "$sp/mujoco" ] && [ -x "$e/bin/python" ]; then
    v=$(ls -d "$sp"/mujoco-*.dist-info 2>/dev/null | head -1 | sed 's/.*mujoco-//; s/\.dist-info$//')
    printf 'CAND\tmujoco\t%s\t%s\t-m mujoco.viewer\t%s\n' "$src" "$e/bin/python" "$v"
  fi
}
for e in "$W/.venv" "$W/venv" "$W/env"; do [ -d "$e" ] && scan_env "$e" workspace; done
for e in "$HOME/.blazar/envs"/*; do [ -d "$e" ] && scan_env "$e" blazar; done
for root in "$HOME/miniforge3/envs" "$HOME/miniconda3/envs" "$HOME/anaconda3/envs" "$HOME/mambaforge3/envs" "$HOME/mambaforge/envs" "$HOME/.conda/envs"; do
  [ -d "$root" ] || continue
  for e in "$root"/*; do [ -d "$e" ] && scan_env "$e" "conda:$(basename "$e")"; done
done
if [ -n "$CFG" ]; then
  if [ -x "$CFG" ]; then
    v=$(cat "$(dirname "$CFG")/VERSION" 2>/dev/null | head -1)
    printf 'CAND\t%s\tconfigured\t%s\t\t%s\n' "$SW" "$CFG" "$v"
  else
    printf 'CFGMISSING\t%s\n' "$CFG"
  fi
fi
"#;

fn parse_scan(raw: &str, sw: &str) -> (Vec<Candidate>, Option<String>) {
    let mut out = Vec::new();
    let mut missing = None;
    for line in raw.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        match parts.as_slice() {
            ["CAND", software, source, program, args, version] if *software == sw => {
                out.push(Candidate {
                    software: (*software).to_owned(),
                    source: (*source).to_owned(),
                    program: (*program).to_owned(),
                    args: args.split_whitespace().map(str::to_owned).collect(),
                    version: (*version).to_owned(),
                });
            }
            ["CFGMISSING", path] => missing = Some((*path).to_owned()),
            _ => {}
        }
    }
    (out, missing)
}

async fn workspace_of(st: &Shared, id: &str) -> ApiResult<(String, String, String)> {
    sqlx::query_as::<_, (String, String, String)>(
        "SELECT n.name, w.path, w.name FROM workspaces w JOIN nodes n ON n.id = w.node_id WHERE w.id = ?1",
    )
    .bind(id)
    .fetch_optional(st.db.pool())
    .await?
    .ok_or_else(|| ApiError::bad_request("工作区不存在"))
}

pub(crate) async fn home_of(transport: &Arc<dyn NodeTransport>) -> Result<String, String> {
    let out = transport
        .exec(
            ExecSpec::new("bash")
                .arg("-lc")
                .arg("printf '__BLAZAR_HOME__ %s\\n' \"$HOME\""),
        )
        .await
        .map_err(|e| e.to_string())?
        .ok()
        .map_err(|e| e.to_string())?;
    out.lines()
        .find_map(|l| l.strip_prefix("__BLAZAR_HOME__ "))
        .map(str::trim)
        .filter(|h| h.starts_with('/'))
        .map(str::to_owned)
        .ok_or_else(|| "拿不到远端的 HOME".to_owned())
}

#[derive(Debug, Deserialize)]
pub struct ScanBody {
    pub workspace: String,
}

pub async fn list(State(st): State<Shared>) -> ApiResult<Json<Value>> {
    let launches = launches_view(&st).await;
    Ok(Json(json!({
        "software": SOFTWARE.iter().map(|s| json!({ "id": s.id, "label": s.label, "desc": s.desc })).collect::<Vec<_>>(),
        "launches": launches,
    })))
}

pub async fn scan(
    State(st): State<Shared>,
    Path(sw): Path<String>,
    Json(body): Json<ScanBody>,
) -> ApiResult<Json<Value>> {
    let software = find(&sw).ok_or_else(|| ApiError::bad_request("没有这个软件"))?;
    let (node, path, _) = workspace_of(&st, &body.workspace).await?;
    let configured = crate::office::kv_get(&st, &path_key(sw.as_str(), &node))
        .await
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let script = format!(
        "W={}\nSW={}\nCFG={}\n{SCAN_SH}",
        blazar_transport::shell_quote(&path),
        blazar_transport::shell_quote(software.id),
        blazar_transport::shell_quote(&configured),
    );
    let out = st
        .transport(&node)
        .exec(ExecSpec::new("bash").arg("-lc").arg(script))
        .await
        .map_err(|e| {
            ApiError::from(anyhow::anyhow!(
                "在 {node} 上查找 {} 失败：{e}",
                software.label
            ))
        })?;
    let (candidates, missing) = parse_scan(&out.stdout, software.id);
    let displays = display::survey(&st, &node).await;
    Ok(Json(json!({
        "node": node,
        "path": path,
        "configured": configured,
        "configured_missing": missing,
        "candidates": candidates,
        "displays": displays.as_ref().map(display::Survey::to_json).unwrap_or(Value::Null),
        "display_error": displays.as_ref().err().cloned(),
        "standalone": software.standalone,
    })))
}

#[derive(Debug, Deserialize)]
pub struct PathBody {
    pub path: String,
}

pub async fn set_path(
    State(st): State<Shared>,
    Path((sw, node)): Path<(String, String)>,
    Json(body): Json<PathBody>,
) -> ApiResult<Json<Value>> {
    find(&sw).ok_or_else(|| ApiError::bad_request("没有这个软件"))?;
    let path = body.path.trim();
    if !path.is_empty() && !path.starts_with('/') {
        return Err(ApiError::bad_request("请填绝对路径"));
    }
    crate::office::kv_put(&st, &path_key(&sw, &node), &json!(path))
        .await
        .map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Launch {
    pub id: String,
    pub software: String,
    pub node: String,
    pub workspace: String,
    pub workspace_name: String,
    pub path: String,
    pub program: String,
    pub args: Vec<String>,
    pub display: String,
    pub run_dir: String,
    pub started_at: String,
    #[serde(default)]
    pub stopped: bool,
}

async fn read_launches(st: &Shared) -> Vec<Launch> {
    serde_json::from_value(crate::office::kv_get(st, LAUNCHES_KEY).await).unwrap_or_default()
}

async fn write_launches(st: &Shared, list: &[Launch]) -> Result<(), String> {
    crate::office::kv_put(st, LAUNCHES_KEY, &json!(list)).await
}

fn run_of(st: &Shared, l: &Launch) -> DetachedRun {
    DetachedRun::attach(st.transport(&l.node), l.run_dir.clone(), l.id.clone())
}

async fn state_of(st: &Shared, l: &Launch) -> (String, Option<i32>) {
    match run_of(st, l).probe().await {
        Ok(RunState::Alive { .. }) => ("running".into(), None),
        Ok(RunState::Exited { code, .. }) => ("exited".into(), Some(code)),
        Ok(RunState::Lost { .. } | RunState::Missing) => ("lost".into(), None),
        Err(_) => ("unreachable".into(), None),
    }
}

async fn launches_view(st: &Shared) -> Vec<Value> {
    let list = read_launches(st).await;
    let states = futures::future::join_all(list.iter().map(|l| state_of(st, l))).await;
    list.into_iter()
        .zip(states)
        .map(|(l, (state, code))| {
            let mut v = serde_json::to_value(&l).unwrap_or(Value::Null);
            v["state"] = json!(state);
            v["exit_code"] = json!(code);
            v["label"] = json!(find(&l.software).map_or(l.software.as_str(), |s| s.label));
            v
        })
        .collect()
}

#[derive(Debug, Deserialize)]
pub struct LaunchBody {
    pub workspace: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub display: String,
}

fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

pub async fn launch(
    State(st): State<Shared>,
    Path(sw): Path<String>,
    Json(body): Json<LaunchBody>,
) -> ApiResult<Json<Value>> {
    let software = find(&sw).ok_or_else(|| ApiError::bad_request("没有这个软件"))?;
    let (node, path, workspace_name) = workspace_of(&st, &body.workspace).await?;
    if node == "local" {
        return Err(ApiError::bad_request(
            "本机工作区暂不支持，请选远端机器上的工作区",
        ));
    }
    let program = body.program.trim().to_owned();
    if !program.starts_with('/') {
        return Err(ApiError::bad_request("启动程序要填绝对路径"));
    }
    let transport = st.transport(&node);
    let home = home_of(&transport)
        .await
        .map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;
    let chosen = display::choose(&st, &node, &body.display)
        .await
        .map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;

    let mut args = body.args.clone();
    if software.id == "isaacsim"
        && let Some((w, h)) = chosen.size
        && !args.iter().any(|a| a.starts_with("--/app/window/"))
    {
        args.push(format!("--/app/window/width={w}"));
        args.push(format!("--/app/window/height={h}"));
    }
    let mut spec = ExecSpec::new(&program)
        .args(args.iter().cloned())
        .cwd(std::path::PathBuf::from(&path))
        .env("DISPLAY", format!(":{}", chosen.number));
    if let Some(auth) = &chosen.xauthority {
        spec = spec.env("XAUTHORITY", auth.clone());
    }
    if software.id == "isaacsim" && install::eula_accepted(&st).await {
        spec = spec.env("OMNI_KIT_ACCEPT_EULA", "YES");
    }
    let id = new_id();
    let root = std::path::PathBuf::from(format!("{home}/.blazar/sims"));
    let run = DetachedRun::launch_in(transport, &root, &id, &spec, None, RunMode::Null)
        .await
        .map_err(|e| {
            ApiError::from(anyhow::anyhow!(
                "在 {node} 上启动 {} 失败：{e}",
                software.label
            ))
        })?;
    let launch = Launch {
        id: id.clone(),
        software: software.id.to_owned(),
        node: node.clone(),
        workspace: body.workspace.clone(),
        workspace_name,
        path,
        program,
        args,
        display: format!(":{}", chosen.number),
        run_dir: run.dir.clone(),
        started_at: Utc::now().to_rfc3339(),
        stopped: false,
    };
    {
        let _g = st.sims.launches.lock().await;
        let mut list = read_launches(&st).await;
        list.retain(|l| l.id != id);
        list.push(launch.clone());
        write_launches(&st, &list)
            .await
            .map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;
    }
    st.emit(ServerEvent::NodesChanged);
    Ok(Json(
        json!({ "id": id, "node": node, "display": launch.display }),
    ))
}

async fn launch_by_id(st: &Shared, id: &str) -> ApiResult<Launch> {
    read_launches(st)
        .await
        .into_iter()
        .find(|l| l.id == id)
        .ok_or_else(|| ApiError::bad_request("没有这个启动记录"))
}

pub async fn stop(State(st): State<Shared>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let l = launch_by_id(&st, &id).await?;
    run_of(&st, &l)
        .hard_kill()
        .await
        .map_err(|e| ApiError::from(anyhow::anyhow!("停止失败：{e}")))?;
    {
        let _g = st.sims.launches.lock().await;
        let mut list = read_launches(&st).await;
        if let Some(x) = list.iter_mut().find(|x| x.id == id) {
            x.stopped = true;
        }
        let _ = write_launches(&st, &list).await;
    }
    st.emit(ServerEvent::NodesChanged);
    Ok(Json(json!({ "ok": true })))
}

pub async fn remove(State(st): State<Shared>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let l = launch_by_id(&st, &id).await?;
    if matches!(state_of(&st, &l).await.0.as_str(), "running") {
        return Err(ApiError::bad_request("还在运行，先停止"));
    }
    {
        let _g = st.sims.launches.lock().await;
        let mut list = read_launches(&st).await;
        list.retain(|x| x.id != id);
        write_launches(&st, &list)
            .await
            .map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;
    }
    st.emit(ServerEvent::NodesChanged);
    Ok(Json(json!({ "ok": true })))
}

pub async fn log(State(st): State<Shared>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let l = launch_by_id(&st, &id).await?;
    let run = run_of(&st, &l);
    let out = run.drain(0).await.unwrap_or_default();
    let err = run.stderr_tail().await.unwrap_or_default();
    let tail = |s: &str| -> String {
        let lines: Vec<&str> = s.lines().collect();
        let start = lines.len().saturating_sub(200);
        lines[start..].join("\n")
    };
    Ok(Json(json!({ "stdout": tail(&out), "stderr": tail(&err) })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_output_is_filtered_by_software() {
        let raw = "CAND\tisaacsim\tconda:behavior\t/h/envs/behavior/bin/isaacsim\t\t5.1.0.0\n\
                   CAND\tmujoco\tworkspace\t/w/.venv/bin/python\t-m mujoco.viewer\t3.2.7\n\
                   CFGMISSING\t/opt/isaac-sim/isaac-sim.sh\nnoise\n";
        let (c, missing) = parse_scan(raw, "mujoco");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].program, "/w/.venv/bin/python");
        assert_eq!(c[0].args, vec!["-m", "mujoco.viewer"]);
        assert_eq!(c[0].version, "3.2.7");
        assert_eq!(missing.as_deref(), Some("/opt/isaac-sim/isaac-sim.sh"));
        let (c, _) = parse_scan(raw, "isaacsim");
        assert_eq!(c[0].source, "conda:behavior");
        assert!(c[0].args.is_empty());
    }

    #[test]
    fn scan_script_finds_workspace_and_conda_envs_without_running_them() {
        let home = tempfile::tempdir().unwrap();
        let ws = home.path().join("proj");
        let mk = |env: &std::path::Path, pkg: &str, bin: &str, ver: &str| {
            let sp = env.join("lib/python3.11/site-packages");
            std::fs::create_dir_all(sp.join(pkg)).unwrap();
            std::fs::create_dir_all(sp.join(format!("{pkg}-{ver}.dist-info"))).unwrap();
            std::fs::create_dir_all(env.join("bin")).unwrap();
            let b = env.join("bin").join(bin);
            std::fs::write(&b, "#!/bin/sh\nexit 1\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        mk(&ws.join(".venv"), "mujoco", "python", "3.2.7");
        mk(
            &home.path().join("miniforge3/envs/behavior"),
            "isaacsim",
            "isaacsim",
            "5.1.0.0",
        );
        let script = format!(
            "W={}\nSW=isaacsim\nCFG={}\n{SCAN_SH}",
            blazar_transport::shell_quote(&ws.to_string_lossy()),
            blazar_transport::shell_quote("/nope/isaac-sim.sh"),
        );
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(script)
            .env("HOME", home.path())
            .output()
            .unwrap();
        let raw = String::from_utf8_lossy(&out.stdout);
        let (isaac, missing) = parse_scan(&raw, "isaacsim");
        assert_eq!(isaac.len(), 1, "{raw}");
        assert_eq!(isaac[0].source, "conda:behavior");
        assert_eq!(isaac[0].version, "5.1.0.0");
        assert_eq!(missing.as_deref(), Some("/nope/isaac-sim.sh"));
        let (mj, _) = parse_scan(&raw, "mujoco");
        assert_eq!(mj.len(), 1);
        assert_eq!(mj[0].source, "workspace");
    }
}
