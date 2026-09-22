use std::sync::Arc;

use blazar_core_types::EntryKind;
use blazar_runtime::SessionSpec;
use blazar_runtime::cli_runtime::{CliRuntime, approval_response_json};
use blazar_transport::detached::{DetachedRun, RunState};
use blazar_transport::{ExecSpec, LocalTransport, NodeTransport, SshTransport};
use futures::StreamExt;

#[tokio::test]
async fn a_permission_prompt_can_be_answered_and_the_tool_then_runs() {
    if std::env::var("BLAZAR_TEST_CLAUDE").is_err() {
        return;
    }
    let t: Arc<dyn NodeTransport> = match std::env::var("BLAZAR_TEST_SSH_HOST") {
        Ok(h) => Arc::new(SshTransport::new(h)),
        Err(_) => Arc::new(LocalTransport),
    };
    let tag = format!(
        "t-appr-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let ws = format!("/tmp/{tag}");
    t.exec(ExecSpec::new("mkdir").arg("-p").arg(&ws))
        .await
        .unwrap();

    let rt = CliRuntime::by_id(t.clone(), "claude").unwrap();
    let mut s = SessionSpec::new(
        &ws,
        "用 Bash 工具执行：touch marker.txt 。执行完只回答 done",
    );
    s.permission_mode = Some("default".into());
    let plan = rt.detached_plan(&s, None, Some(&uuid_v4()));
    let run = DetachedRun::launch(
        t.clone(),
        &tag,
        &plan.spec,
        plan.first_input.as_deref(),
        plan.mode,
    )
    .await
    .unwrap();

    let mut off = 0u64;
    let mut answered = false;
    let mut saw_result = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(150);
    'outer: while std::time::Instant::now() < deadline {
        let mut stream = run.follow(off).await.unwrap();
        while let Some(line) = stream.stdout.next().await {
            let at = off;
            off += line.len() as u64 + 1;
            for (k, _) in rt.parse_line(&line) {
                match k {
                    EntryKind::Approval { id, request } if !answered => {
                        let raw = run.line_at(at).await.unwrap().expect("那一行应当还在");
                        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
                        let rid = v["request_id"].as_str().unwrap();
                        assert_eq!(request["tool_name"], "Bash");
                        let msg =
                            approval_response_json(rid, true, Some(&v["request"]["input"]), "");
                        run.append(&format!("a-{id}"), &msg).await.unwrap();
                        answered = true;
                    }
                    EntryKind::Finished(_) => {
                        saw_result = true;
                        run.eof().await.unwrap();
                    }
                    _ => {}
                }
            }
        }
        if matches!(run.probe().await.unwrap(), RunState::Exited { .. }) {
            break 'outer;
        }
    }
    assert!(answered, "agent 应当问过权限");
    assert!(saw_result, "答复之后这一轮应当正常结束");
    let ok = t
        .exec(
            ExecSpec::new("test")
                .arg("-e")
                .arg(format!("{ws}/marker.txt")),
        )
        .await
        .unwrap()
        .code
        == 0;
    let _ = run.remove().await;
    let _ = t.exec(ExecSpec::new("rm").arg("-rf").arg(&ws)).await;
    assert!(ok, "批准之后工具应当真的执行了（marker.txt 要存在）");
}

fn uuid_v4() -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let h = format!("{:032x}", n ^ (u128::from(std::process::id()) << 64));
    format!(
        "{}-{}-4{}-8{}-{}",
        &h[0..8],
        &h[8..12],
        &h[13..16],
        &h[17..20],
        &h[20..32]
    )
}
