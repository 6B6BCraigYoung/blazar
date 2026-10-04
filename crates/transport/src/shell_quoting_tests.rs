use super::ExecSpec;
use std::process::Command;

#[test]
fn shell_execution_preserves_arguments_environment_and_file_values() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir
        .path()
        .join("work ' $(printf expanded) `printf expanded`\n中文");
    std::fs::create_dir(&cwd).unwrap();
    let cwd = cwd.canonicalize().unwrap();
    let filename = "-value ' $(printf expanded) `printf expanded`\n中文";
    let file_value = "synthetic ' \" $(printf expanded) `printf expanded`\nsecond line";
    std::fs::write(cwd.join(filename), file_value).unwrap();
    let env_value = "literal ' \" $HOME $(printf expanded) `printf expanded` ;\n中文";
    let args = [
        "",
        "two words",
        "single'quote",
        "double\"quote",
        "line one\nline two",
        "$(printf expanded)",
        "`printf expanded`",
        "; printf expanded",
        "*?[]",
        "-n",
        "中文",
    ];
    let spec = ExecSpec::new("/bin/sh")
        .args([
            "-c",
            "printf '%s\\0' \"$PWD\" \"$QUOTED\" \"$FILE_VALUE\" \"$@\"",
            "fixture",
        ])
        .args(args)
        .cwd(&cwd)
        .env("QUOTED", env_value)
        .env_file("FILE_VALUE", filename);
    let output = Command::new("/bin/sh")
        .args(["-c", &spec.to_shell().unwrap()])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let cwd = cwd.to_str().unwrap();
    let expected: Vec<u8> = [cwd, env_value, file_value]
        .into_iter()
        .chain(args)
        .flat_map(|value| value.as_bytes().iter().copied().chain([0]))
        .collect();
    assert_eq!(output.stdout, expected);
}
