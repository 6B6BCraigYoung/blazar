use std::collections::BTreeMap;
use std::time::Duration;

use blazar_transport::detached::{ProbeItem, RunState};
use chrono::Utc;

use crate::api::Shared;

use super::announce::{announce, took};
use super::launch::finish;
use super::{EVERY, Job, SELECT, SUMMARY_LINES, changed, get, job_of};

const PROBE_TIMEOUT: Duration = Duration::from_secs(25);

pub async fn watch(st: Shared) {
    loop {
        sweep(&st).await;
        tokio::time::sleep(EVERY).await;
    }
}

pub async fn sweep(st: &Shared) {
    let running: Vec<Job> = sqlx::query(&format!("{SELECT} WHERE j.status = 'running'"))
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default()
        .iter()
        .map(job_of)
        .collect();
    st.bots
        .jobs
        .missing
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|id, _| running.iter().any(|j| &j.id == id));
    let mut by_node: BTreeMap<String, Vec<Job>> = BTreeMap::new();
    for j in running.into_iter().filter(|j| j.run_dir.is_some()) {
        by_node.entry(j.node.clone()).or_default().push(j);
    }
    let checks = by_node
        .into_iter()
        .map(|(node, jobs)| check_node(st, node, jobs));
    let touched = futures::future::join_all(checks).await;
    if touched.into_iter().any(|t| t) {
        changed(st);
    }
}

#[must_use]
pub fn verdict(job: &Job, state: &RunState) -> Option<(&'static str, Option<i32>)> {
    match state {
        RunState::Alive { .. } => None,
        RunState::Exited { code: 0, .. } => Some(("done", Some(0))),
        RunState::Exited { code, .. } => Some(("failed", Some(*code))),
        RunState::Lost { .. } if job.kind == "register" => Some(("done", None)),
        RunState::Lost { .. } | RunState::Missing => Some(("lost", None)),
    }
}

async fn check_node(st: &Shared, node: String, jobs: Vec<Job>) -> bool {
    let items: Vec<ProbeItem> = jobs
        .iter()
        .map(|j| ProbeItem {
            dir: j.run_dir.clone().unwrap_or_default(),
            tail_of: j.log_file(),
            tail_lines: SUMMARY_LINES,
            exists: j.wait_paths(),
        })
        .collect();
    let transport = st.transport(&node);
    let now = Utc::now().to_rfc3339();
    let probed = match tokio::time::timeout(
        PROBE_TIMEOUT,
        blazar_transport::detached::probe_many(&transport, &items),
    )
    .await
    {
        Ok(Ok(states)) => states,
        Ok(Err(e)) => return note_error(st, &jobs, &format!("连不上 {node}：{e}")).await,
        Err(_) => {
            return note_error(
                st,
                &jobs,
                &format!("{node} {} 秒内没有回应", PROBE_TIMEOUT.as_secs()),
            )
            .await;
        }
    };
    note_missing(st, &jobs, &probed);
    let mut touched = false;
    for (job, probed) in jobs.iter().zip(probed) {
        match verdict(job, &probed.state) {
            None => {
                let _ = sqlx::query(
                    "UPDATE jobs SET last_checked_at = ?2, last_error = NULL WHERE id = ?1",
                )
                .bind(&job.id)
                .bind(&now)
                .execute(st.db.pool())
                .await;
                touched |= job.last_error.is_some();
                touched |= past_deadline(st, job).await;
            }
            Some((status, code)) => {
                if finish(st, &job.id, status, code, probed.exited_at).await {
                    touched = true;
                    if let Some(done) = get(st, &job.id).await {
                        announce(st, &done, &probed.tail).await;
                    }
                }
            }
        }
    }
    touched
}

fn note_missing(st: &Shared, jobs: &[Job], probed: &[blazar_transport::detached::Probed]) {
    let mut missing = st
        .bots
        .jobs
        .missing
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for (job, p) in jobs.iter().zip(probed) {
        let gone: Vec<String> = job
            .wait_paths()
            .into_iter()
            .zip(&p.present)
            .filter(|(_, have)| **have == Some(false))
            .map(|(path, _)| path)
            .collect();
        if gone.is_empty() {
            missing.remove(&job.id);
        } else {
            missing.insert(job.id.clone(), gone);
        }
    }
}

#[must_use]
pub fn missing_paths(st: &Shared, id: &str) -> Vec<String> {
    st.bots
        .jobs
        .missing
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(id)
        .cloned()
        .unwrap_or_default()
}

async fn note_error(st: &Shared, jobs: &[Job], error: &str) -> bool {
    let mut touched = false;
    for j in jobs {
        touched |= j.last_error.as_deref() != Some(error);
        let _ = sqlx::query("UPDATE jobs SET last_error = ?2 WHERE id = ?1 AND status = 'running'")
            .bind(&j.id)
            .bind(error)
            .execute(st.db.pool())
            .await;
    }
    touched
}

async fn past_deadline(st: &Shared, job: &Job) -> bool {
    let Some(at) = job.deadline_at.as_deref() else {
        return false;
    };
    let now = Utc::now();
    if job.deadline_noted_at.is_some() || at > now.to_rfc3339().as_str() {
        return false;
    }
    let first = sqlx::query(
        "UPDATE jobs SET deadline_noted_at = ?2, updated_at = ?2
         WHERE id = ?1 AND deadline_noted_at IS NULL",
    )
    .bind(&job.id)
    .bind(now.to_rfc3339())
    .execute(st.db.pool())
    .await
    .is_ok_and(|r| r.rows_affected() == 1);
    if first {
        crate::inbox::push(
            st,
            crate::inbox::Item {
                kind: "job",
                title: format!("后台任务「{}」过了截止时间还在跑", job.label),
                body: format!(
                    "机器 {}，发起方 {}，已经跑了 {}。要结束就到任务页的「后台任务」里点「结束」。",
                    job.node,
                    job.owner_label(),
                    took(&job.created_at, None)
                ),
                workspace_id: job.workspace_id.clone(),
                thread_id: None,
                session_id: None,
                ref_id: Some(job.id.clone()),
            },
        )
        .await;
    }
    first
}
