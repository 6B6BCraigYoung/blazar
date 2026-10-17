use std::time::Duration;

use blazar_transport::ExecSpec;

use super::*;

const SHELL_TIMEOUT_SECS: u64 = 120;
const SHELL_MAX_BYTES: usize = 30 * 1024;

#[derive(Debug, Deserialize)]
pub struct ShellBody {
    pub command: String,
}

fn clip(s: &str) -> (String, bool) {
    if s.len() <= SHELL_MAX_BYTES {
        return (s.to_owned(), false);
    }
    let mut cut = s.len() - SHELL_MAX_BYTES;
    while !s.is_char_boundary(cut) {
        cut += 1;
    }
    (s[cut..].to_owned(), true)
}

pub fn shell_script(cwd: &str, command: &str) -> String {
    format!(
        "{}\ncd {} || exit 96\n{command}",
        blazar_transport::PATH_PRELUDE,
        blazar_transport::shell_quote(cwd)
    )
}

pub async fn workspace_shell(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<ShellBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let command = body.command.trim().to_owned();
    if command.is_empty() {
        return Err(ApiError::bad_request("命令是空的"));
    }
    let (node, path) = locate(&st, &id).await?;
    let started = std::time::Instant::now();
    let transport = st.transport(&node);
    let run = transport.exec(
        ExecSpec::new("bash")
            .arg("-lc")
            .arg(shell_script(&path, &command)),
    );
    let (stdout, stderr, code, timed_out) =
        match tokio::time::timeout(Duration::from_secs(SHELL_TIMEOUT_SECS), run).await {
            Ok(Ok(out)) => (out.stdout, out.stderr, out.code, false),
            Ok(Err(e)) => (String::new(), e.to_string(), -1, false),
            Err(_) => (
                String::new(),
                format!("超过 {SHELL_TIMEOUT_SECS} 秒没结束，已放弃等待"),
                -1,
                true,
            ),
        };
    let (stdout, cut_out) = clip(&stdout);
    let (stderr, cut_err) = clip(&stderr);
    Ok(Json(serde_json::json!({
        "command": command,
        "stdout": stdout,
        "stderr": stderr,
        "code": code,
        "timed_out": timed_out,
        "truncated": cut_out || cut_err,
        "ms": started.elapsed().as_millis() as u64,
        "node": node,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_output_keeps_the_tail_on_a_char_boundary() {
        let s = format!("{}{}", "头".repeat(SHELL_MAX_BYTES), "end");
        let (out, cut) = clip(&s);
        assert!(cut);
        assert!(out.len() <= SHELL_MAX_BYTES);
        assert!(out.ends_with("end"));
        assert_eq!(clip("short"), ("short".to_owned(), false));
    }

    #[test]
    fn the_command_runs_inside_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker.txt"), "x").unwrap();
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(shell_script(
                &dir.path().to_string_lossy(),
                "ls; echo oops >&2; exit 3",
            ))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&out.stdout).contains("marker.txt"));
        assert!(String::from_utf8_lossy(&out.stderr).contains("oops"));
    }
}
