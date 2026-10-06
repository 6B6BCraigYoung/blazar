#![cfg(unix)]

use std::process::Command;

#[test]
fn tmux_shell_preserves_workspace_and_session_arguments() {
    let cwd = "/workspace/it' s \"quoted\" $(printf expanded) `printf expanded` ;\n中文";
    let session = "session' \" $(printf expanded) `printf expanded` ;\n中文";
    let production = blazar_terminal::debug_shell_command(cwd, Some(session));
    let script = format!(
        "tmux() {{ return 97; }}\nexec() {{ printf '%s\\0' \"$TERM\" \"$@\"; }}\n{production}"
    );
    let output = Command::new("/bin/bash")
        .args(["--noprofile", "--norc", "-c", &script])
        .env_clear()
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let expected: Vec<u8> = [
        "xterm-256color",
        "tmux",
        "-u",
        "new-session",
        "-A",
        "-s",
        session,
        "-c",
        cwd,
        ";",
        "set-option",
        "-t",
        session,
        "status",
        "off",
        ";",
        "set-option",
        "-t",
        session,
        "mouse",
        "on",
    ]
    .into_iter()
    .flat_map(|value| value.as_bytes().iter().copied().chain([0]))
    .collect();
    assert_eq!(output.stdout, expected);
}
