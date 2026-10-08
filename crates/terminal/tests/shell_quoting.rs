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
        .env("HOME", "/h")
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let got: Vec<String> = output
        .stdout
        .split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8(part.to_vec()).unwrap())
        .collect();
    let expected = [
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
        ";",
        "set-environment",
        "-t",
        session,
        "PATH",
    ];
    assert_eq!(&got[..expected.len()], &expected[..]);
    let path = &got[expected.len()];
    assert!(path.starts_with("/h/.blazar/bin:"), "{path}");
    assert!(path.ends_with(":/usr/bin:/bin"), "{path}");
    assert_eq!(got.len(), expected.len() + 1);
}
