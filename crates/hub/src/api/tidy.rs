use std::collections::{BTreeMap, BTreeSet, HashMap};

use blazar_core_types::place;

use super::*;

const COMMON: [&str; 9] = [
    "workspace",
    "workspaces",
    "root",
    "home",
    "tmp",
    "src",
    "code",
    "repo",
    "project",
];

const CHECK_SECS: u64 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub node: String,
    pub path: String,
    pub name: String,
    pub parent: Option<String>,
    pub project: Option<String>,
    pub kind: String,
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Step {
    Nest {
        key: String,
        id: String,
        name: String,
        under: String,
        under_name: String,
        project: Option<String>,
    },
    Project {
        key: String,
        ids: Vec<String>,
        names: Vec<String>,
        project: String,
    },
    Archive {
        key: String,
        id: String,
        name: String,
        why: String,
    },
}

impl Step {
    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Self::Nest { key, .. } | Self::Project { key, .. } | Self::Archive { key, .. } => key,
        }
    }
}

fn depth(path: &str) -> usize {
    place::trim(path).matches('/').count()
}

fn descends(parent: &HashMap<String, Option<String>>, from: &str, target: &str) -> bool {
    let mut seen = BTreeSet::new();
    let mut at = Some(from.to_owned());
    while let Some(id) = at {
        if id == target {
            return true;
        }
        if !seen.insert(id.clone()) {
            return false;
        }
        at = parent.get(&id).cloned().flatten();
    }
    false
}

#[must_use]
pub fn plan(rows: &[Item], brain: &str) -> Vec<Step> {
    let mut live: Vec<&Item> = rows
        .iter()
        .filter(|r| r.kind != super::assistant::KIND)
        .collect();
    live.sort_by_key(|r| (depth(&r.path), r.path.clone()));
    let by_id: HashMap<&str, &Item> = live.iter().map(|r| (r.id.as_str(), *r)).collect();
    let mut parent: HashMap<String, Option<String>> = live
        .iter()
        .map(|r| (r.id.clone(), r.parent.clone()))
        .collect();
    let mut project: HashMap<String, Option<String>> = live
        .iter()
        .map(|r| (r.id.clone(), r.project.clone()))
        .collect();
    let mut steps = Vec::new();
    let mut archived = BTreeSet::new();
    for r in &live {
        if r.node == "local" && place::inside(&r.path, brain) && !r.running {
            archived.insert(r.id.clone());
            steps.push(Step::Archive {
                key: format!("archive:{}", r.id),
                id: r.id.clone(),
                name: r.name.clone(),
                why: "本机中转目录（看图暂存），远程会话已经能直接看图".into(),
            });
        }
    }
    let spots: Vec<(&str, &str, &str)> = live
        .iter()
        .filter(|r| !archived.contains(&r.id))
        .map(|r| (r.id.as_str(), r.node.as_str(), r.path.as_str()))
        .collect();
    for r in &live {
        if archived.contains(&r.id) {
            continue;
        }
        let Some(under) = place::container(spots.iter().copied(), &r.node, &r.path) else {
            continue;
        };
        if parent.get(&r.id).cloned().flatten().as_deref() == Some(under) {
            continue;
        }
        let mine = project.get(&r.id).cloned().flatten();
        let theirs = project.get(under).cloned().flatten();
        if mine.is_some() && mine != theirs {
            continue;
        }
        if descends(&parent, under, &r.id) {
            continue;
        }
        parent.insert(r.id.clone(), Some(under.to_owned()));
        if mine.is_none() {
            project.insert(r.id.clone(), theirs.clone());
        }
        steps.push(Step::Nest {
            key: format!("nest:{}", r.id),
            id: r.id.clone(),
            name: r.name.clone(),
            under: under.to_owned(),
            under_name: by_id.get(under).map(|u| u.name.clone()).unwrap_or_default(),
            project: theirs.filter(|_| mine.is_none()),
        });
    }
    let mut groups: BTreeMap<String, Vec<&Item>> = BTreeMap::new();
    for r in &live {
        if archived.contains(&r.id) || parent.get(&r.id).cloned().flatten().is_some() {
            continue;
        }
        let f = place::folder(&r.path);
        if f.is_empty() || COMMON.contains(&f.to_ascii_lowercase().as_str()) {
            continue;
        }
        groups.entry(f.to_owned()).or_default().push(r);
    }
    for (folder, members) in groups {
        let nodes: BTreeSet<&str> = members.iter().map(|r| r.node.as_str()).collect();
        if nodes.len() < 2 {
            continue;
        }
        let named: BTreeSet<String> = members
            .iter()
            .filter_map(|r| project.get(&r.id).cloned().flatten())
            .collect();
        let target = match named.len() {
            0 => folder,
            1 => named.into_iter().next().unwrap_or(folder),
            _ => continue,
        };
        let missing: Vec<&Item> = members
            .into_iter()
            .filter(|r| project.get(&r.id).cloned().flatten().as_deref() != Some(target.as_str()))
            .collect();
        if missing.is_empty() {
            continue;
        }
        steps.push(Step::Project {
            key: format!("project:{target}"),
            ids: missing.iter().map(|r| r.id.clone()).collect(),
            names: missing.iter().map(|r| r.name.clone()).collect(),
            project: target,
        });
    }
    steps
}

#[must_use]
pub fn tops(rows: &[Item], steps: &[Step]) -> (usize, usize) {
    let live: Vec<&Item> = rows
        .iter()
        .filter(|r| r.kind != super::assistant::KIND)
        .collect();
    let before = live.iter().filter(|r| r.parent.is_none()).count();
    let mut nested = BTreeSet::new();
    let mut gone = BTreeSet::new();
    for s in steps {
        match s {
            Step::Nest { id, .. } => {
                nested.insert(id.as_str());
            }
            Step::Archive { id, .. } => {
                gone.insert(id.as_str());
            }
            Step::Project { .. } => {}
        }
    }
    let after = live
        .iter()
        .filter(|r| {
            r.parent.is_none() && !nested.contains(r.id.as_str()) && !gone.contains(r.id.as_str())
        })
        .count();
    (before, after)
}

async fn rows(st: &AppState) -> sqlx::Result<Vec<Item>> {
    let found = sqlx::query(
        "SELECT w.id, n.name AS node, w.path, w.name, w.parent_id, p.name AS project, w.kind,
                EXISTS (SELECT 1 FROM sessions s WHERE s.workspace_id = w.id AND s.status = 'running')
                  AS running
         FROM workspaces w JOIN nodes n ON n.id = w.node_id
         LEFT JOIN projects p ON p.id = w.project_id
         WHERE w.archived_at IS NULL",
    )
    .fetch_all(st.db.pool())
    .await?;
    Ok(found
        .iter()
        .map(|r| Item {
            id: r.try_get("id").unwrap_or_default(),
            node: r.try_get("node").unwrap_or_default(),
            path: r.try_get("path").unwrap_or_default(),
            name: r.try_get("name").unwrap_or_default(),
            parent: r.try_get("parent_id").unwrap_or(None),
            project: r.try_get("project").unwrap_or(None),
            kind: r.try_get("kind").unwrap_or_default(),
            running: r.try_get::<bool, _>("running").unwrap_or(false),
        })
        .collect())
}

fn brain(st: &AppState) -> String {
    st.mesh_ctx
        .staging_dir
        .parent()
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf)
        .join("brain")
        .display()
        .to_string()
}

async fn vanished(
    st: &AppState,
    rows: &[Item],
    skip: &BTreeSet<String>,
) -> (Vec<Step>, Vec<String>) {
    let mut by_node: BTreeMap<String, Vec<&Item>> = BTreeMap::new();
    for r in rows {
        if r.kind != super::assistant::KIND && !r.running && !skip.contains(&r.id) {
            by_node.entry(r.node.clone()).or_default().push(r);
        }
    }
    let checks = by_node.into_iter().map(|(node, members)| {
        let script: String = members
            .iter()
            .map(|r| {
                format!(
                    "[ -e {} ] || echo {}\n",
                    blazar_transport::shell_quote(&r.path),
                    blazar_transport::shell_quote(&r.id)
                )
            })
            .collect();
        let transport = st.transport(&node);
        async move {
            let spec = blazar_transport::ExecSpec::new("bash")
                .arg("-c")
                .arg(script);
            let out = tokio::time::timeout(
                std::time::Duration::from_secs(CHECK_SECS),
                transport.exec(spec),
            )
            .await;
            (node, members, out)
        }
    });
    let mut steps = Vec::new();
    let mut unreachable = Vec::new();
    for (node, members, out) in futures::future::join_all(checks).await {
        match out {
            Ok(Ok(o)) if o.code == 0 => {
                let gone: BTreeSet<&str> = o.stdout.lines().map(str::trim).collect();
                for r in members.iter().filter(|r| gone.contains(r.id.as_str())) {
                    steps.push(Step::Archive {
                        key: format!("archive:{}", r.id),
                        id: r.id.clone(),
                        name: r.name.clone(),
                        why: format!("目录在 {node} 上已经不存在"),
                    });
                }
            }
            _ => unreachable.push(node),
        }
    }
    (steps, unreachable)
}

async fn full_plan(st: &AppState) -> ApiResult<(Vec<Item>, Vec<Step>, Vec<String>)> {
    let rows = rows(st).await?;
    let mut steps = plan(&rows, &brain(st));
    let planned: BTreeSet<String> = steps
        .iter()
        .filter_map(|s| match s {
            Step::Archive { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    let (gone, unreachable) = vanished(st, &rows, &planned).await;
    let gone_ids: BTreeSet<String> = gone
        .iter()
        .filter_map(|s| match s {
            Step::Archive { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    steps.retain(|s| match s {
        Step::Nest { id, under, .. } => !gone_ids.contains(id) && !gone_ids.contains(under),
        _ => true,
    });
    steps.extend(gone);
    Ok((rows, steps, unreachable))
}

pub async fn tidy_preview(State(st): State<Shared>) -> ApiResult<Json<serde_json::Value>> {
    let (rows, steps, unreachable) = full_plan(&st).await?;
    let (before, after) = tops(&rows, &steps);
    Ok(Json(serde_json::json!({
        "steps": steps,
        "top_before": before,
        "top_after": after,
        "unreachable": unreachable,
    })))
}

#[derive(Debug, Default, Deserialize)]
pub struct TidyBody {
    #[serde(default)]
    pub only: Option<Vec<String>>,
}

pub async fn tidy_apply(
    State(st): State<Shared>,
    Json(b): Json<TidyBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let (_, steps, _) = full_plan(&st).await?;
    let mut applied = Vec::new();
    let mut failed = Vec::new();
    for s in steps.iter().filter(|s| {
        b.only
            .as_ref()
            .is_none_or(|o| o.iter().any(|k| k == s.key()))
    }) {
        let result = match s {
            Step::Nest { id, under, .. } => {
                let req = super::workspaces::UpdateWorkspace {
                    parent: Some(under.clone()),
                    ..Default::default()
                };
                super::workspaces::reshape(&st, id, &req).await
            }
            Step::Project { ids, project, .. } => {
                let mut out = Ok(());
                for id in ids {
                    let req = super::workspaces::UpdateWorkspace {
                        project: Some(project.clone()),
                        ..Default::default()
                    };
                    if let Err(e) = super::workspaces::reshape(&st, id, &req).await {
                        out = Err(e);
                        break;
                    }
                }
                out
            }
            Step::Archive { id, .. } => {
                let req = super::workspaces::UpdateWorkspace {
                    archived: Some(true),
                    ..Default::default()
                };
                super::workspaces::reshape(&st, id, &req).await
            }
        };
        match result {
            Ok(()) => applied.push(s.key().to_owned()),
            Err(e) => failed.push(serde_json::json!({ "key": s.key(), "error": e.message() })),
        }
    }
    Ok(Json(
        serde_json::json!({ "applied": applied, "failed": failed }),
    ))
}

#[cfg(test)]
#[path = "tidy_tests.rs"]
mod tests;
