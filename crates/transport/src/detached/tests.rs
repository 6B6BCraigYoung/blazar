use crate::LocalTransport;
use futures::StreamExt;

use super::test_support::{TestRun, launch, local, rid, wait_exit, without_login};
use super::*;

#[tokio::test]
async fn explicit_run_root_is_used_without_changing_home() {
    let root = tempfile::tempdir().unwrap();
    let home = std::env::var_os("HOME");
    let run = DetachedRun::launch_in(
        local(),
        root.path(),
        &rid("root"),
        &ExecSpec::new("/usr/bin/printf").arg("isolated"),
        None,
        RunMode::Null,
    )
    .await
    .unwrap();
    assert!(std::path::Path::new(&run.dir).starts_with(root.path()));
    assert_eq!(std::env::var_os("HOME"), home);
    assert!(matches!(
        wait_exit(&run).await,
        RunState::Exited { code: 0, .. }
    ));
    assert!(run.drain(0).await.unwrap().contains("isolated"));
    run.remove().await.unwrap();
}

#[cfg(unix)]
fn input_fixture(transport: Arc<dyn NodeTransport>, initial: &[u8]) -> TestRun {
    use std::os::unix::fs::MetadataExt;
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("in.jsonl");
    std::fs::write(&input, initial).unwrap();
    std::fs::write(
        root.path().join("in.inode"),
        std::fs::metadata(input).unwrap().ino().to_string(),
    )
    .unwrap();
    let run = DetachedRun::attach(transport, root.path().display().to_string(), rid("input"));
    TestRun { run, _root: root }
}

#[cfg(unix)]
struct TruncatedInput;

#[cfg(unix)]
#[async_trait::async_trait]
impl NodeTransport for TruncatedInput {
    fn kind(&self) -> crate::TransportKind {
        crate::TransportKind::Local
    }
    fn target(&self) -> &str {
        "local"
    }
    async fn exec(&self, mut spec: ExecSpec) -> Result<crate::ExecOutput> {
        let input = spec.stdin.as_mut().unwrap();
        input.truncate(input.len() - 3);
        LocalTransport.exec(without_login(spec)).await
    }
    async fn spawn_lines(&self, _spec: ExecSpec) -> Result<LineStream> {
        unreachable!()
    }
}

#[cfg(unix)]
#[tokio::test]
async fn append_truncated_upload_does_not_modify_input_log() {
    let run = input_fixture(Arc::new(TruncatedInput), b"");
    assert!(run.append("a-test", r#"{"answer":"A"}"#).await.is_err());
    assert_eq!(
        std::fs::read(run._root.path().join("in.jsonl")).unwrap(),
        b""
    );
    let retry = DetachedRun::attach(local(), run.dir.clone(), run.run_id.clone());
    assert_eq!(
        retry.append("a-test", r#"{"answer":"A"}"#).await.unwrap(),
        Appended::Written
    );
    assert_eq!(
        retry.append("a-test", r#"{"answer":"A"}"#).await.unwrap(),
        Appended::Duplicate
    );
    assert_eq!(
        std::fs::read(run._root.path().join("in.jsonl")).unwrap(),
        b"a-test\t{\"answer\":\"A\"}\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn append_completes_matching_partial_line_once() {
    let prior = b"u-1\t{}\n";
    let full = "a-test\t{\"answer\":\"选项甲\"}\n".as_bytes();
    for missing in [1, 3, 4] {
        let mut initial = prior.to_vec();
        initial.extend(&full[..full.len() - missing]);
        let run = input_fixture(local(), &initial);
        assert_eq!(
            run.append("a-test", r#"{"answer":"选项甲"}"#)
                .await
                .unwrap(),
            Appended::Written
        );
        assert_eq!(
            run.append("a-test", r#"{"answer":"选项甲"}"#)
                .await
                .unwrap(),
            Appended::Duplicate
        );
        let mut expected = prior.to_vec();
        expected.extend(full);
        assert_eq!(
            std::fs::read(run._root.path().join("in.jsonl")).unwrap(),
            expected
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn append_refuses_inconsistent_partial_line_without_rewriting_it() {
    let initial = b"a-test\t{\"answer\":\"A";
    let run = input_fixture(local(), initial);
    assert!(run.append("a-test", r#"{"answer":"B"}"#).await.is_err());
    assert_eq!(
        std::fs::read(run._root.path().join("in.jsonl")).unwrap(),
        initial
    );
}

#[cfg(unix)]
#[tokio::test]
async fn append_refuses_reused_id_with_a_different_complete_payload() {
    let initial = b"a-test\t{\"answer\":\"A\"}\n";
    let run = input_fixture(local(), initial);
    assert!(run.append("a-test", r#"{"answer":"B"}"#).await.is_err());
    assert_eq!(
        std::fs::read(run._root.path().join("in.jsonl")).unwrap(),
        initial
    );
}

fn echo_agent() -> ExecSpec {
    ExecSpec::new("bash").arg("-c").arg(
        r#"echo '{"type":"init"}'; while IFS= read -r l; do printf '{"echo":%s}\n' "$l"; done; echo '{"type":"done"}'"#,
    )
}

#[tokio::test]
async fn a_running_run_refuses_to_be_removed() {
    let spec = ExecSpec::new("bash").arg("-c").arg("sleep 30");
    let run = launch(local(), &rid("rm"), &spec, None, RunMode::Null)
        .await
        .unwrap();
    assert!(run.remove().await.is_err(), "还在跑的不能删");
    run.hard_kill().await.unwrap();
    run.remove().await.unwrap();
    assert!(!std::path::Path::new(&run.dir).exists());
}

#[test]
fn base64_matches_the_standard_alphabet() {
    assert_eq!(b64(b""), "");
    assert_eq!(b64(b"hi"), "aGk=");
    assert_eq!(b64(b"hi!"), "aGkh");
    assert_eq!(b64("中文".as_bytes()), "5Lit5paH");
}

#[test]
fn run_ids_that_could_escape_are_refused() {
    assert!(check_run_id("01a0b1f2-bc2a-7172").is_ok());
    for bad in ["", "../x", "a b", "a;rm -rf ~", "A", "a/b", "$HOME"] {
        assert!(check_run_id(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn a_shifted_boot_clock_does_not_lose_a_live_run() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        "{IDENTITY_FNS}\ncd {}\nS=$(start_of $$)\nprintf '%s wrong-boot %s\\n' $$ \"$S\" > pid\nalive && echo alive || echo lost\necho \"$S\"",
        dir.path().display()
    );
    let out = std::process::Command::new("bash")
        .arg("-c")
        .arg(script)
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&out.stdout);
    let mut lines = out.lines();
    let verdict = lines.next().unwrap();
    let start = lines.next().unwrap();
    if start.contains('_') {
        assert_eq!(
            verdict, "alive",
            "带日期的启动时间足以认出进程，开机时钟漂移不能判死"
        );
    } else {
        assert_eq!(verdict, "lost", "相对开机的启动时间必须核对开机标识");
    }
}

#[test]
fn cmd_sh_execs_the_agent_so_signals_reach_it() {
    let c = render_cmd(
        &ExecSpec::new("claude")
            .arg("-p")
            .arg("it's")
            .cwd("/w x")
            .env("K", "v 1"),
    )
    .unwrap();
    assert!(c.contains("cd '/w x' || exit 96"));
    assert!(c.contains("export K='v 1'"));
    assert!(
        c.contains("exec 'claude' '-p' 'it'\\''s'"),
        "要 exec 替换掉 bash，否则 TERM 只杀到 bash: {c}"
    );
    assert!(
        c.find(crate::PATH_PRELUDE)
            .is_some_and(|i| c.find("export K=").is_some_and(|j| i < j)),
        "按名字找的 CLI 先补 PATH，且在显式 env 之前: {c}"
    );
    let abs = render_cmd(&ExecSpec::new("/opt/claude/bin/claude")).unwrap();
    assert!(
        !abs.contains(crate::PATH_PRELUDE),
        "给了绝对路径就不用补 PATH"
    );
}

#[tokio::test]
async fn invalid_environment_names_never_create_a_run_directory() {
    let root = tempfile::tempdir().unwrap();
    for spec in [
        ExecSpec::new("true").env("A-B", "test-value"),
        ExecSpec::new("true").env_file("A-B", root.path().join("absent")),
    ] {
        assert!(render_cmd(&spec).is_err());
        assert!(
            DetachedRun::launch_in(
                local(),
                root.path(),
                "invalid-env",
                &spec,
                None,
                RunMode::Null
            )
            .await
            .is_err()
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn relay_delivers_input_and_eof_ends_the_agent() {
    let run = launch(
        local(),
        &rid("relay"),
        &echo_agent(),
        Some("u-1\t\"first\""),
        RunMode::Relay,
    )
    .await
    .unwrap();
    assert_eq!(
        run.append("u-2", "\"second\"").await.unwrap(),
        Appended::Written
    );
    assert_eq!(
        run.append("u-2", "\"second\"").await.unwrap(),
        Appended::Duplicate,
        "同一个 msg_id 重投不能写第二遍"
    );
    run.eof().await.unwrap();
    assert!(matches!(
        wait_exit(&run).await,
        RunState::Exited { code: 0, .. }
    ));

    let out = run.drain(0).await.unwrap();
    assert_eq!(out.matches("\"first\"").count(), 1, "{out}");
    assert_eq!(
        out.matches("\"second\"").count(),
        1,
        "重投的那条只能出现一次: {out}"
    );
    assert!(out.contains("blazar_exit"), "{out}");
}

#[tokio::test]
async fn a_prompt_line_equal_to_a_heredoc_delimiter_is_just_text() {
    let marker = std::env::temp_dir().join(format!("pwned-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let evil = format!("fix bug\nBLZ_CMD_EOF\ntouch {}\nEOF\n'", marker.display());
    let spec = ExecSpec::new("printf").arg("%s").arg(&evil);
    let run = launch(local(), &rid("inject"), &spec, None, RunMode::Null)
        .await
        .unwrap();
    wait_exit(&run).await;
    assert!(!marker.exists(), "提示词里的文本被当成命令执行了");
    assert!(
        run.drain(0).await.unwrap().contains("BLZ_CMD_EOF"),
        "文本要原样到达 agent"
    );
}

#[tokio::test]
async fn secrets_from_env_files_reach_the_agent_but_not_the_run_dir() {
    let secret = format!("sk-ant-oat01-test-{}", std::process::id());
    let file = std::env::temp_dir().join(format!("blz-secret-{}", std::process::id()));
    std::fs::write(&file, format!("{secret}\n")).unwrap();
    let spec = ExecSpec::new("bash")
        .arg("-c")
        .arg(r#"printf '[%s]' "$TOKEN""#)
        .env_file("TOKEN", &file);
    let run = launch(local(), &rid("secret"), &spec, None, RunMode::Null)
        .await
        .unwrap();
    wait_exit(&run).await;
    let out = run.drain(0).await.unwrap();
    assert!(out.contains(&format!("[{secret}]")), "{out}");
    let cmd = std::fs::read_to_string(std::path::Path::new(&run.dir).join("cmd.sh")).unwrap();
    assert!(cmd.contains("$(cat "), "{cmd}");
    for f in std::fs::read_dir(&run.dir).unwrap() {
        let path = f.unwrap().path();
        if path.ends_with("out.jsonl") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            assert!(
                !text.contains(&secret),
                "{} 里出现了明文 token",
                path.display()
            );
        }
    }
    run.remove().await.unwrap();
    let _ = std::fs::remove_file(&file);
}

#[tokio::test]
async fn the_agent_outlives_whoever_launched_it() {
    let spec = ExecSpec::new("bash").arg("-c").arg("sleep 30");
    let run = launch(local(), &rid("outlive"), &spec, None, RunMode::Null)
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(matches!(run.probe().await.unwrap(), RunState::Alive { .. }));
    run.hard_kill().await.unwrap();
    assert!(matches!(
        run.probe().await.unwrap(),
        RunState::Exited { .. }
    ));
}

#[tokio::test]
async fn following_resumes_exactly_from_the_byte_offset() {
    let run = launch(
        local(),
        &rid("follow"),
        &echo_agent(),
        Some("u-1\t\"a\""),
        RunMode::Relay,
    )
    .await
    .unwrap();
    run.append("u-2", "\"b\"").await.unwrap();
    run.eof().await.unwrap();
    wait_exit(&run).await;

    let mut s = run.follow(0).await.unwrap();
    let mut off = 0u64;
    let mut first = Vec::new();
    for _ in 0..2 {
        let l = s.stdout.next().await.unwrap();
        off += l.len() as u64 + 1;
        first.push(l);
    }
    drop(s);

    let rest = run.drain(off).await.unwrap();
    let all = format!("{}\n{rest}", first.join("\n"));
    assert_eq!(all, run.drain(0).await.unwrap(), "续读要逐字节一致");
}

#[tokio::test]
async fn a_replaced_input_file_is_detected_instead_of_silently_dropping_input() {
    let run = launch(local(), &rid("inode"), &echo_agent(), None, RunMode::Relay)
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let p = std::path::Path::new(&run.dir).join("in.jsonl");
    std::fs::remove_file(&p).unwrap();
    std::fs::write(&p, "").unwrap();
    assert!(
        matches!(
            run.probe().await.unwrap(),
            RunState::Alive {
                input_replaced: true,
                ..
            }
        ),
        "要探测得出输入文件被换过"
    );
    assert!(
        run.append("u-9", "\"x\"").await.is_err(),
        "写进新文件等于没送达，必须报错"
    );
    run.hard_kill().await.unwrap();
}

fn remote() -> Option<Arc<dyn NodeTransport>> {
    let h = std::env::var("BLAZAR_TEST_SSH_HOST").ok()?;
    Some(Arc::new(crate::SshTransport::new(h)))
}

#[tokio::test]
#[ignore = "requires an explicitly authorized remote test machine"]
async fn remote_dedup_holds_on_gnu_grep_hosts() {
    let Some(t) = remote() else { return };
    let run = DetachedRun::launch(t, &rid("rdedup"), &echo_agent(), None, RunMode::Relay)
        .await
        .unwrap();
    for _ in 0..3 {
        run.append("u-7", "\"once\"").await.unwrap();
    }
    run.eof().await.unwrap();
    wait_exit(&run).await;
    let out = run.drain(0).await.unwrap();
    assert_eq!(
        out.matches("\"once\"").count(),
        1,
        "Linux 上重投三次也只能送达一次: {out}"
    );
}

#[tokio::test]
#[ignore = "requires an explicitly authorized remote test machine"]
async fn remote_orphaned_tool_processes_are_reaped_after_an_agent_crash() {
    let Some(t) = remote() else { return };
    let tag = rid("orphan");
    let agent = ExecSpec::new("bash").arg("-c").arg(format!(
        "setsid bash -c 'sleep 300; echo {tag}' & sleep 1; kill -KILL $$"
    ));
    let run = DetachedRun::launch(t.clone(), &tag, &agent, None, RunMode::Null)
        .await
        .unwrap();
    let st = wait_exit(&run).await;
    assert!(matches!(st, RunState::Exited { code: 137, .. }), "{st:?}");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let left = t
        .exec(ExecSpec::new("bash").arg("-c").arg(format!(
            "grep -lz '^BLAZAR_RUN={tag}$' /proc/[0-9]*/environ 2>/dev/null | wc -l"
        )))
        .await
        .unwrap();
    assert_eq!(
        left.stdout.trim(),
        "0",
        "agent 崩溃后被收养的工具进程要被清掉"
    );
}

#[tokio::test]
#[ignore = "requires an explicitly authorized remote test machine"]
async fn remote_survives_launcher_and_resumes_from_offset() {
    let Some(t) = remote() else { return };
    let run = DetachedRun::launch(
        t,
        &rid("rfollow"),
        &echo_agent(),
        Some("u-1\t\"a\""),
        RunMode::Relay,
    )
    .await
    .unwrap();
    assert!(matches!(run.probe().await.unwrap(), RunState::Alive { .. }));
    run.append("u-2", "\"b\"").await.unwrap();
    run.eof().await.unwrap();
    wait_exit(&run).await;
    let mut s = run.follow(0).await.unwrap();
    let first = s.stdout.next().await.unwrap();
    drop(s);
    let rest = run.drain(first.len() as u64 + 1).await.unwrap();
    assert_eq!(format!("{first}\n{rest}"), run.drain(0).await.unwrap());
}
