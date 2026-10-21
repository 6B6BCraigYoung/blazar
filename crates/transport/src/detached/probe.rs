use std::sync::Arc;

use crate::{NodeTransport, Result, TransportError, shell_quote};

use super::{IDENTITY_FNS, PROBE_BODY, RunState, sh};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeItem {
    pub dir: String,
    pub tail_of: Option<String>,
    pub tail_lines: usize,
    pub exists: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probed {
    pub state: RunState,
    pub tail: String,
    pub exited_at: Option<i64>,
    pub present: Vec<Option<bool>>,
}

pub async fn probe_many(
    transport: &Arc<dyn NodeTransport>,
    items: &[ProbeItem],
) -> Result<Vec<Probed>> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let marker = format!(
        "__BLAZAR_PROBE_{}__",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let mut script = format!("{IDENTITY_FNS}\n");
    for (i, item) in items.iter().enumerate() {
        let tail = item
            .tail_of
            .as_deref()
            .map(|f| {
                format!(
                    "echo '{marker} tail'; tail -n {} -- {} 2>/dev/null",
                    item.tail_lines.max(1),
                    shell_quote(f)
                )
            })
            .unwrap_or_else(|| ":".into());
        let exists: String = item
            .exists
            .iter()
            .enumerate()
            .map(|(j, path)| {
                format!(
                    "if [ -e {} ]; then echo 'present={j}:1'; else echo 'present={j}:0'; fi\n",
                    shell_quote(path)
                )
            })
            .collect();
        script.push_str(&format!(
            r#"echo '{marker} run {i}'
{exists}if cd {dir} 2>/dev/null; then
  R=$({PROBE_BODY})
  [ -e exit.code ] && echo "exited_at=$(stat -c %Y exit.code 2>/dev/null || stat -f %m exit.code 2>/dev/null)"
else R=missing; fi
printf '%s\n' "$R"
case "$R" in *alive*) ;; *) {tail} ;; esac
true
"#,
            dir = shell_quote(&item.dir),
        ));
    }
    let out = transport.exec(sh(script)).await?;
    if out.code != 0 {
        return Err(TransportError::Command {
            code: out.code,
            stderr: out.stderr.trim().to_owned(),
        });
    }
    let mut probed = parse_many(&out.stdout, &marker, items.len());
    for (p, item) in probed.iter_mut().zip(items) {
        p.present.resize(item.exists.len(), None);
    }
    Ok(probed)
}

fn parse_present(state: &str) -> Vec<Option<bool>> {
    let mut out: Vec<Option<bool>> = Vec::new();
    for (j, have) in state.lines().filter_map(|l| {
        let (j, have) = l.trim().strip_prefix("present=")?.split_once(':')?;
        Some((j.parse::<usize>().ok()?, have == "1"))
    }) {
        if j >= out.len() {
            out.resize(j + 1, None);
        }
        out[j] = Some(have);
    }
    out
}

fn parse_many(raw: &str, marker: &str, n: usize) -> Vec<Probed> {
    let mut blocks: Vec<(String, String)> = vec![(String::new(), String::new()); n];
    let mut current: Option<(usize, bool)> = None;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix(marker) {
            let rest = rest.trim();
            if let Some(i) = rest
                .strip_prefix("run ")
                .and_then(|i| i.parse::<usize>().ok())
            {
                current = (i < n).then_some((i, false));
                continue;
            }
            if rest == "tail" {
                current = current.map(|(i, _)| (i, true));
                continue;
            }
        }
        if let Some((i, in_tail)) = current {
            let target = if in_tail {
                &mut blocks[i].1
            } else {
                &mut blocks[i].0
            };
            target.push_str(line);
            target.push('\n');
        }
    }
    blocks
        .into_iter()
        .map(|(state, tail)| {
            let exited_at = state
                .lines()
                .find_map(|l| l.trim().strip_prefix("exited_at="))
                .and_then(|v| v.trim().parse().ok());
            let present = parse_present(&state);
            let body: String = state
                .lines()
                .filter(|l| {
                    !l.trim().starts_with("exited_at=") && !l.trim().starts_with("present=")
                })
                .collect::<Vec<_>>()
                .join("\n");
            let state = if body.trim() == "missing" || body.trim().is_empty() {
                RunState::Missing
            } else {
                parse_probe(&body)
            };
            Probed {
                state,
                tail,
                exited_at,
                present,
            }
        })
        .collect()
}

pub(super) fn parse_probe(raw: &str) -> RunState {
    let size = raw
        .lines()
        .find_map(|l| l.strip_prefix("size="))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    for l in raw.lines() {
        let l = l.trim();
        if let Some(c) = l.strip_prefix("exited=") {
            return RunState::Exited {
                code: c.trim().parse().unwrap_or(-1),
                size,
            };
        }
        match l {
            "alive" => {
                return RunState::Alive {
                    size,
                    input_replaced: false,
                };
            }
            "alive-replaced" => {
                return RunState::Alive {
                    size,
                    input_replaced: true,
                };
            }
            "lost" => return RunState::Lost { size },
            _ => {}
        }
    }
    RunState::Lost { size }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExecSpec;
    use crate::detached::test_support::{launch, local, rid, wait_exit};
    use crate::detached::{DetachedRun, RunMode};

    #[tokio::test]
    async fn many_runs_are_probed_in_one_go_with_tails_of_finished_ones() {
        let transport = local();
        let done = launch(
            transport.clone(),
            &rid("pm-done"),
            &ExecSpec::new("bash")
                .arg("-c")
                .arg("echo one; echo two; exit 3"),
            None,
            RunMode::Null,
        )
        .await
        .unwrap();
        let busy = launch(
            transport.clone(),
            &rid("pm-busy"),
            &ExecSpec::new("bash").arg("-c").arg("sleep 30"),
            None,
            RunMode::Null,
        )
        .await
        .unwrap();
        wait_exit(&done).await;
        let item = |run: &DetachedRun| ProbeItem {
            dir: run.dir.clone(),
            tail_of: Some(format!("{}/out.jsonl", run.dir)),
            tail_lines: 20,
            exists: vec![run.dir.clone(), "/nonexistent/blazar-probe".into()],
        };
        let items = [
            item(&done),
            item(&busy),
            ProbeItem {
                dir: "/nonexistent/blazar-probe".into(),
                tail_of: None,
                tail_lines: 20,
                exists: vec![done.dir.clone()],
            },
            ProbeItem {
                dir: done.dir.clone(),
                tail_of: Some("/nonexistent/blazar-probe/log".into()),
                tail_lines: 20,
                ..ProbeItem::default()
            },
        ];
        let got = probe_many(&transport, &items).await.unwrap();
        assert!(
            matches!(got[0].state, RunState::Exited { code: 3, .. }),
            "{got:?}"
        );
        assert!(got[0].tail.contains("one\ntwo\n"), "{got:?}");
        assert!(matches!(got[1].state, RunState::Alive { .. }), "{got:?}");
        assert!(got[1].tail.is_empty());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(
            got[0].exited_at.is_some_and(|t| (now - t).abs() < 60),
            "{got:?}"
        );
        assert_eq!(got[1].exited_at, None);
        assert_eq!(got[2].state, RunState::Missing);
        assert_eq!(got[0].present, [Some(true), Some(false)]);
        assert_eq!(got[1].present, [Some(true), Some(false)]);
        assert_eq!(got[2].present, [Some(true)]);
        assert!(got[3].present.is_empty());
        assert!(
            matches!(got[3].state, RunState::Exited { code: 3, .. }),
            "{got:?}"
        );
        assert!(got[3].tail.is_empty());
        busy.hard_kill().await.unwrap();
    }

    #[test]
    fn probe_output_is_parsed() {
        assert_eq!(
            parse_probe("size=120\nexited=0\n"),
            RunState::Exited { code: 0, size: 120 }
        );
        assert_eq!(
            parse_probe("size=5\nalive\n"),
            RunState::Alive {
                size: 5,
                input_replaced: false
            }
        );
        assert_eq!(parse_probe("size=5\nlost\n"), RunState::Lost { size: 5 });
    }
}
