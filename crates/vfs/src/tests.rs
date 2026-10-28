use super::*;
use crate::browse::parse_listing;
use crate::files::parse_file;
use crate::search::parse_search;
use crate::tree::{parse_tree, unquote_git_path};

struct TestLocal;

fn isolated(mut spec: ExecSpec) -> ExecSpec {
    if spec.program == "bash" && spec.args.first().is_some_and(|arg| arg == "-lc") {
        spec.args[0] = "-c".into();
    }
    spec.env("BASH_ENV", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
}

#[async_trait::async_trait]
impl NodeTransport for TestLocal {
    fn kind(&self) -> blazar_transport::TransportKind {
        blazar_transport::TransportKind::Local
    }
    fn target(&self) -> &str {
        "local"
    }
    async fn exec(&self, spec: ExecSpec) -> blazar_transport::Result<blazar_transport::ExecOutput> {
        blazar_transport::LocalTransport.exec(isolated(spec)).await
    }
    async fn spawn_lines(
        &self,
        spec: ExecSpec,
    ) -> blazar_transport::Result<blazar_transport::LineStream> {
        blazar_transport::LocalTransport
            .spawn_lines(isolated(spec))
            .await
    }
}

struct PartialWrite;

#[async_trait::async_trait]
impl NodeTransport for PartialWrite {
    fn kind(&self) -> blazar_transport::TransportKind {
        blazar_transport::TransportKind::Local
    }
    fn target(&self) -> &str {
        "local"
    }
    async fn exec(
        &self,
        mut spec: ExecSpec,
    ) -> blazar_transport::Result<blazar_transport::ExecOutput> {
        if spec.stdin.is_some() {
            spec.args[1] = format!("cat() {{ printf partial; return 1; }}\n{}", spec.args[1]);
        }
        TestLocal.exec(spec).await
    }
    async fn spawn_lines(
        &self,
        spec: ExecSpec,
    ) -> blazar_transport::Result<blazar_transport::LineStream> {
        TestLocal.spawn_lines(spec).await
    }
}

fn tar_names(b64: &str) -> Vec<String> {
    let mut child = std::process::Command::new("sh")
        .args(["-c", "base64 -d | tar -tzf -"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), b64.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let mut names: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim_start_matches("./").to_owned())
        .filter(|l| !l.is_empty() && !l.ends_with('/'))
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn folder_archive_skips_git_ignored_files() {
    let root = tempfile::tempdir().unwrap();
    let sub = root.path().join("pkg");
    std::fs::create_dir_all(sub.join("src")).unwrap();
    std::fs::write(sub.join("src/a.rs"), "a").unwrap();
    std::fs::write(sub.join("build.log"), "noise").unwrap();
    std::fs::write(root.path().join(".gitignore"), "*.log\n").unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(root.path())
            .output()
            .unwrap()
    };
    git(&["init", "-q"]);
    let v = Vfs::new(Arc::new(TestLocal), root.path().to_string_lossy());
    let b64 = v.archive_base64("pkg", 1 << 20).await.unwrap();
    assert_eq!(tar_names(&b64), ["src/a.rs"]);
}

#[tokio::test]
async fn files_and_folders_round_trip_between_roots() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(src.path().join("pkg/src")).unwrap();
    std::fs::write(src.path().join("pkg/src/a.rs"), "fn a() {}").unwrap();
    std::fs::write(src.path().join("pkg/README.md"), "# pkg").unwrap();
    std::fs::write(src.path().join("note.bin"), [0u8, 159, 146, 150]).unwrap();
    let from = Vfs::new(Arc::new(TestLocal), src.path().to_string_lossy());
    let to = Vfs::new(Arc::new(TestLocal), dst.path().to_string_lossy());
    let archive = from.archive_base64("pkg", 1 << 20).await.unwrap();
    to.extract_base64("vendor/pkg", &archive).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(dst.path().join("vendor/pkg/src/a.rs")).unwrap(),
        "fn a() {}"
    );
    assert_eq!(
        std::fs::read_to_string(dst.path().join("vendor/pkg/README.md")).unwrap(),
        "# pkg"
    );
    let file = from.read_base64("note.bin", 1 << 20).await.unwrap();
    to.write_base64("in/note.bin", &file).await.unwrap();
    assert_eq!(
        std::fs::read(dst.path().join("in/note.bin")).unwrap(),
        [0u8, 159, 146, 150]
    );
    assert!(to.write_base64("../escape.bin", &file).await.is_err());
    assert!(to.extract_base64("../out", &archive).await.is_err());
    assert!(to.extract_base64("in/note.bin", &archive).await.is_err());
    assert!(!dst.path().join("escape.bin").exists());
}

struct WithHome(String);

#[async_trait::async_trait]
impl NodeTransport for WithHome {
    fn kind(&self) -> blazar_transport::TransportKind {
        blazar_transport::TransportKind::Local
    }
    fn target(&self) -> &str {
        "local"
    }
    async fn exec(&self, spec: ExecSpec) -> blazar_transport::Result<blazar_transport::ExecOutput> {
        TestLocal.exec(spec.env("HOME", &self.0)).await
    }
    async fn spawn_lines(
        &self,
        spec: ExecSpec,
    ) -> blazar_transport::Result<blazar_transport::LineStream> {
        TestLocal.spawn_lines(spec.env("HOME", &self.0)).await
    }
}

#[tokio::test]
async fn browsing_home_leaves_protected_folders_unopened() {
    let home = tempfile::tempdir().unwrap();
    for dir in ["Desktop/inner", "Desktop/.git", "proj/src", "proj/.git"] {
        std::fs::create_dir_all(home.path().join(dir)).unwrap();
    }
    let v = Vfs::new(
        Arc::new(WithHome(home.path().to_string_lossy().into_owned())),
        "",
    );
    let listed = v.browse("").await.unwrap();
    assert_eq!(listed.home.as_deref(), Some(listed.path.as_str()));
    let entry = |name: &str| listed.entries.iter().find(|e| e.name == name).unwrap();
    assert!(entry("proj").is_repo);
    assert_eq!(entry("proj").children, Some(1));
    if cfg!(target_os = "macos") {
        assert!(!entry("Desktop").is_repo);
        assert_eq!(entry("Desktop").children, None);
    } else {
        assert!(entry("Desktop").is_repo);
        assert_eq!(entry("Desktop").children, Some(1));
    }
}

#[tokio::test]
async fn a_home_workspace_tree_skips_protected_folders() {
    let home = tempfile::tempdir().unwrap();
    for (dir, file) in [("Desktop", "secret.txt"), ("proj", "a.txt")] {
        std::fs::create_dir_all(home.path().join(dir)).unwrap();
        std::fs::write(home.path().join(dir).join(file), "x").unwrap();
    }
    let root = home.path().to_string_lossy().into_owned();
    let v = Vfs::new(Arc::new(WithHome(root.clone())), root);
    let paths: Vec<String> = v
        .tree(None)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.path)
        .collect();
    assert!(paths.iter().any(|p| p == "proj/a.txt"), "{paths:?}");
    assert_eq!(
        paths.iter().any(|p| p == "Desktop/secret.txt"),
        !cfg!(target_os = "macos"),
        "{paths:?}"
    );
}

#[tokio::test]
async fn mkdir_creates_a_visible_folder_and_rejects_bad_names() {
    let root = tempfile::tempdir().unwrap();
    let here = root.path().to_string_lossy().into_owned();
    let v = Vfs::new(Arc::new(TestLocal), "");
    let created = v.mkdir(&here, " new project ").await.unwrap();
    assert_eq!(
        std::path::PathBuf::from(&created),
        root.path().canonicalize().unwrap().join("new project")
    );
    assert!(root.path().join("new project").is_dir());
    let listed = v.browse(&here).await.unwrap();
    assert!(listed.entries.iter().any(|e| e.name == "new project"));
    let again = v.mkdir(&here, "new project").await.unwrap_err();
    assert!(again.to_string().contains("同名"), "{again}");
    for bad in ["", ".", "..", ".hidden", "a/b", "a\\b", "tab\there"] {
        assert!(v.mkdir(&here, bad).await.is_err(), "{bad:?} 应被拒绝");
    }
    assert!(v.mkdir(&format!("{here}/missing"), "x").await.is_err());
    assert!(!root.path().join("missing").exists());
}

#[tokio::test]
async fn folder_archive_rejects_files_and_oversized_output() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("d")).unwrap();
    std::fs::write(root.path().join("d/x.bin"), vec![7u8; 4096]).unwrap();
    std::fs::write(root.path().join("f.txt"), "f").unwrap();
    let v = Vfs::new(Arc::new(TestLocal), root.path().to_string_lossy());
    assert_eq!(
        tar_names(&v.archive_base64("d", 1 << 20).await.unwrap()),
        ["x.bin"]
    );
    assert!(v.archive_base64("f.txt", 1 << 20).await.is_err());
    assert!(v.archive_base64("d", 8).await.is_err());
    assert!(v.archive_base64("../d", 1 << 20).await.is_err());
}

#[tokio::test]
async fn atomic_write_failure_keeps_original_and_cleans_staging() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("note.txt"), "original").unwrap();
    let v = Vfs::new(Arc::new(PartialWrite), root.path().to_string_lossy());
    assert!(v.write("note.txt", "replacement", None).await.is_err());
    assert_eq!(
        std::fs::read_to_string(root.path().join("note.txt")).unwrap(),
        "original"
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn atomic_write_rejects_a_stale_content_version() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("note.txt"), "original").unwrap();
    let v = Vfs::new(Arc::new(TestLocal), root.path().to_string_lossy());
    let written = v
        .write_checked("note.txt", "replacement", None, Some(&"0".repeat(64)))
        .await
        .unwrap();
    assert!(!written.saved);
    assert_eq!(
        std::fs::read_to_string(root.path().join("note.txt")).unwrap(),
        "original"
    );
}

#[tokio::test]
async fn atomic_write_detects_changed_content_with_the_same_mtime() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("note.txt"), "original").unwrap();
    sh(root.path(), "touch -t 202001010000 note.txt");
    let v = Vfs::new(Arc::new(TestLocal), root.path().to_string_lossy());
    let original = v.read("note.txt").await.unwrap();
    assert_eq!(original.version.as_ref().unwrap().len(), 64);
    let first = v
        .write_checked(
            "note.txt",
            "first",
            Some(u64::MAX),
            original.version.as_deref(),
        )
        .await
        .unwrap();
    assert!(first.saved);
    assert_ne!(first.version, original.version);
    assert_eq!(v.read("note.txt").await.unwrap().version, first.version);
    sh(root.path(), "touch -t 202001010000 note.txt");
    assert_eq!(v.read("note.txt").await.unwrap().mtime, original.mtime);
    assert!(
        !v.write_checked(
            "note.txt",
            "second",
            Some(original.mtime),
            original.version.as_deref()
        )
        .await
        .unwrap()
        .saved
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("note.txt")).unwrap(),
        "first"
    );
    std::fs::remove_file(root.path().join("note.txt")).unwrap();
    assert!(
        !v.write_checked("note.txt", "second", None, first.version.as_deref())
            .await
            .unwrap()
            .saved
    );
    assert!(!root.path().join("note.txt").exists());
    assert!(
        !v.write("note.txt", "legacy", Some(original.mtime))
            .await
            .unwrap()
            .saved
    );
}

#[tokio::test]
async fn atomic_write_allows_only_one_concurrent_content_version() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("note.txt"), "original").unwrap();
    let v = Vfs::new(Arc::new(TestLocal), root.path().to_string_lossy());
    let original = v.read("note.txt").await.unwrap();
    let (first, second) = tokio::join!(
        v.write_checked("note.txt", "first", None, original.version.as_deref()),
        v.write_checked("note.txt", "second", None, original.version.as_deref())
    );
    let mut saved = 0;
    for result in [first, second] {
        match result {
            Ok(written) => saved += usize::from(written.saved),
            Err(VfsError::Transport(blazar_transport::TransportError::Command {
                code: 10,
                ..
            })) => {}
            other => panic!("unexpected save result: {other:?}"),
        }
    }
    assert_eq!(saved, 1);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn atomic_write_reports_an_existing_lock_without_removing_it() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("note.txt"), "original").unwrap();
    let lock = root.path().join(
        ".blazar-write-59700155e034d16d95b0e6916ee23373f40422648201778db4b19ad28398ca6f.lock",
    );
    std::fs::create_dir(&lock).unwrap();
    std::fs::write(lock.join("owner"), "fixture-owner").unwrap();
    let v = Vfs::new(Arc::new(TestLocal), root.path().to_string_lossy());
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        v.write("note.txt", "replacement", None),
    )
    .await
    .unwrap();
    assert!(matches!(
        result,
        Err(VfsError::Transport(
            blazar_transport::TransportError::Command { code: 10, .. }
        ))
    ));
    assert_eq!(
        std::fs::read_to_string(lock.join("owner")).unwrap(),
        "fixture-owner"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("note.txt")).unwrap(),
        "original"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn atomic_write_creates_private_files() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let v = Vfs::new(Arc::new(TestLocal), root.path().to_string_lossy());
    assert!(v.write("new.txt", "new", None).await.unwrap().saved);
    assert_eq!(
        std::fs::metadata(root.path().join("new.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn containment_normalizes_separators_before_validation() {
    let v = vfs();
    for rel in [
        r"..\outside",
        r"src\..\outside",
        r"\absolute",
        r"C:\outside",
        "C:outside",
        r"\\host\share",
    ] {
        assert!(
            matches!(v.safe_rel(rel), Err(VfsError::PathEscape(_))),
            "{rel:?}"
        );
    }
    assert_eq!(v.safe_rel(r"src\file.txt").unwrap(), "src/file.txt");
}

#[cfg(unix)]
#[tokio::test]
async fn containment_rejects_symbolic_links_in_every_file_operation() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("note.txt"), "outside-fixture").unwrap();
    symlink(outside.path(), root.path().join("linked-dir")).unwrap();
    symlink(
        outside.path().join("note.txt"),
        root.path().join("linked-file"),
    )
    .unwrap();
    let v = Vfs::new(Arc::new(TestLocal), root.path().to_string_lossy());
    for path in ["linked-file", "linked-dir/note.txt"] {
        assert!(matches!(
            v.validate_path(path).await,
            Err(VfsError::PathEscape(_))
        ));
        assert!(
            matches!(v.read(path).await, Err(VfsError::PathEscape(_))),
            "{path}"
        );
        assert!(
            matches!(
                v.read_base64(path, 1024).await,
                Err(VfsError::PathEscape(_))
            ),
            "{path}"
        );
        assert!(
            matches!(
                v.write(path, "replacement", None).await,
                Err(VfsError::PathEscape(_))
            ),
            "{path}"
        );
    }
    assert!(v.validate_path("new/file.txt").await.is_ok());
    assert!(v.validate_path("").await.is_ok());
    assert!(matches!(
        v.list_dir("linked-dir").await,
        Err(VfsError::PathEscape(_))
    ));
    assert!(matches!(
        v.write("linked-dir/new.txt", "new", None).await,
        Err(VfsError::PathEscape(_))
    ));
    assert_eq!(
        std::fs::read_to_string(outside.path().join("note.txt")).unwrap(),
        "outside-fixture"
    );
    assert!(!outside.path().join("new.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn containment_accepts_an_explicit_link_root_but_rejects_internal_links() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let alias = fixture.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    let v = Vfs::new(Arc::new(TestLocal), alias.to_string_lossy());
    assert!(
        v.write("folder/file.txt", "fixture", None)
            .await
            .unwrap()
            .saved
    );
    assert_eq!(v.read(r"folder\file.txt").await.unwrap().content, "fixture");
    assert_eq!(v.list_dir("folder").await.unwrap().items.len(), 1);
    std::os::unix::fs::symlink(root.join("folder"), root.join("internal-link")).unwrap();
    assert!(matches!(
        v.read("internal-link/file.txt").await,
        Err(VfsError::PathEscape(_))
    ));
    assert!(v.list_dir("/folder").await.is_err());
    assert!(v.write("./.git/config", "fixture", None).await.is_err());
    assert!(v.write("folder/.git", "fixture", None).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn containment_tree_and_search_do_not_follow_links() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("note.txt"), "outside-only-marker").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("linked-dir")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("note.txt"),
        root.path().join("linked-file"),
    )
    .unwrap();
    let v = Vfs::new(Arc::new(TestLocal), root.path().to_string_lossy());
    assert!(
        v.search("outside-only-marker", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(v.tree(None).await.unwrap().is_empty());
    sh(root.path(), "git init -q && git add linked-file linked-dir");
    assert!(
        v.search("outside-only-marker", 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[test]
fn tree_marks_changes_and_derives_dirs() {
    let raw = "__TRACKED__\nsrc/main.rs\nsrc/lib.rs\nREADME.md\n\
               __STATUS__\n M src/main.rs\n?? src/new.rs\n";
    let tree = parse_tree(raw);

    let by_path = |p: &str| tree.iter().find(|e| e.path == p).cloned();
    assert_eq!(
        by_path("src/main.rs").unwrap().change,
        Some(ChangeKind::Modified)
    );
    assert_eq!(
        by_path("src/new.rs").unwrap().change,
        Some(ChangeKind::Untracked)
    );
    assert_eq!(by_path("README.md").unwrap().change, None);
    assert!(by_path("src").unwrap().is_dir, "目录节点应自动推导出来");
}

#[test]
fn rename_takes_new_path() {
    let raw = "__TRACKED__\na.rs\n__STATUS__\nR  old.rs -> new.rs\n";
    let tree = parse_tree(raw);
    assert!(tree.iter().any(|e| e.path == "new.rs"));
}

#[test]
fn plain_dir_without_git_still_lists() {
    let raw = "__PLAIN__\nnotes/a.md\nnotes/b.md\n";
    let tree = parse_tree(raw);
    assert_eq!(tree.iter().filter(|e| !e.is_dir).count(), 2);
    assert!(tree.iter().any(|e| e.is_dir && e.path == "notes"));
}

#[test]
fn search_parses_rg_and_grep_shape() {
    let raw = "./src/main.rs:42:    let x = 1;\nsrc/lib.rs:7:fn main() {}\n";
    let hits = parse_search(raw);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].path, "src/main.rs");
    assert_eq!(hits[0].line, 42);
    assert_eq!(hits[1].line, 7);
}

#[test]
fn search_ignores_unparseable_lines() {
    let hits = parse_search("--\nbinary file matches\nsrc/a.rs:1:ok\n");
    assert_eq!(hits.len(), 1);
}

fn vfs() -> Vfs {
    Vfs::new(Arc::new(TestLocal), "/tmp/ws")
}

fn out(code: i32, stderr: &str) -> blazar_transport::ExecOutput {
    blazar_transport::ExecOutput {
        code,
        stdout: String::new(),
        stderr: stderr.into(),
    }
}

#[test]
fn unreachable_node_is_an_error_not_an_empty_result() {
    let e = check(
        &out(255, "ssh: connect to host gpu9: No route to host"),
        "/x",
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("机器连不上"), "错误信息要指向机器: {e}");
    assert!(e.contains("No route to host"), "要保留原始原因: {e}");
}

#[test]
fn missing_directory_says_so() {
    let e = check(&out(3, ""), "/home/me/proj").unwrap_err().to_string();
    assert!(e.contains("目录不存在"), "{e}");
    assert!(e.contains("/home/me/proj"), "要说清是哪个目录: {e}");
}

#[test]
fn success_passes_through() {
    assert!(check(&out(0, ""), "/x").is_ok());
}

#[test]
fn file_body_keeps_newlines_and_pipes() {
    let raw = "SIZE|42\nBODY|\nfn main() {\n    let a = b | c;\n}\n";
    let f = parse_file("src/main.rs", raw);
    assert_eq!(f.size, 42);
    assert!(!f.binary && !f.too_large);
    assert!(f.content.contains("let a = b | c;"));
    assert!(f.content.starts_with("fn main()"));
}

#[test]
fn binary_and_oversize_return_no_content() {
    let b = parse_file("a.bin", "SIZE|1024\nBINARY|1\n");
    assert!(b.binary && b.content.is_empty() && b.size == 1024);

    let l = parse_file("big.log", "SIZE|30000000\nTOOLARGE|1\n");
    assert!(l.too_large && l.content.is_empty());
    assert_eq!(l.size, 30_000_000, "要把实际大小告诉用户，而不是只说太大");
}

#[test]
fn empty_file_reads_as_empty_not_error() {
    let f = parse_file("empty.txt", "SIZE|0\nBODY|\n");
    assert_eq!(f.size, 0);
    assert!(f.content.is_empty());
    assert!(!f.binary && !f.too_large);
}

#[test]
fn home_expands_but_paths_stay_quoted() {
    assert_eq!(cd_target(""), "~");
    assert_eq!(cd_target("~"), "~");
    assert_eq!(cd_target("~/my proj"), "~/'my proj'");

    assert_eq!(cd_target("/home/a b"), "'/home/a b'");

    let hostile = cd_target("/tmp; rm -rf /");
    assert_eq!(hostile, "'/tmp; rm -rf /'");

    assert_eq!(cd_target("/tmp/it's"), "'/tmp/it'\\''s'");
}

#[test]
fn listing_puts_repos_first_and_finds_parent() {
    let raw = "PATH|/home/me\nPARENT|/home\n\
               DIR|zebra|/home/me/zebra|0|3\n\
               DIR|Alpha|/home/me/Alpha|0|0\n\
               DIR|myrepo|/home/me/myrepo|1|12\n";
    let l = parse_listing(raw);
    assert_eq!(l.path, "/home/me");
    assert_eq!(l.parent.as_deref(), Some("/home"));
    assert!(!l.is_repo);

    assert_eq!(l.entries[0].name, "myrepo");
    assert!(l.entries[0].is_repo);
    assert_eq!(l.entries[1].name, "Alpha");
    assert_eq!(l.entries[2].name, "zebra");
    assert_eq!(l.entries[0].children, Some(12));
}

#[test]
fn root_has_no_parent() {
    let l = parse_listing("PATH|/\nDIR|tmp|/tmp|0|5\n");
    assert!(l.parent.is_none(), "根目录不应有上一级");
}

#[test]
fn self_repo_is_detected() {
    let l = parse_listing("PATH|/home/me/proj\nPARENT|/home/me\nSELFREPO|1\n");
    assert!(
        l.is_repo,
        "当前目录是仓库时要标出来，否则用户不知道能不能直接选"
    );
}

#[test]
fn path_escape_is_rejected() {
    let v = vfs();
    assert!(v.safe_rel("../../etc/passwd").is_err());
    assert!(v.safe_rel("/etc/passwd").is_err());
    assert!(v.safe_rel("").is_err());
    assert!(v.safe_rel("src/main.rs").is_ok());
}

#[test]
fn nested_parent_dir_still_rejected() {
    let v = vfs();
    assert!(v.safe_rel("src/../../../etc/passwd").is_err());
}

struct Scratch(tempfile::TempDir);

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        self.0.path()
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Self::Target {
        self.0.path()
    }
}

fn scratch(name: &str) -> Scratch {
    Scratch(
        tempfile::Builder::new()
            .prefix(&format!("blazar-vfs-{name}-"))
            .tempdir()
            .unwrap(),
    )
}
#[tokio::test]
async fn list_dir_shows_everything_on_disk_and_flags_ignored() {
    let dir = scratch("list-dir");
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args(args)
            .current_dir(&dir)
            .output()
            .unwrap()
    };
    run(&["init", "-q"]);
    std::fs::write(dir.join(".gitignore"), "target/\n*.log\n").unwrap();
    std::fs::create_dir_all(dir.join("target/debug")).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/a b.rs"), "").unwrap();
    std::fs::write(dir.join("run.log"), "").unwrap();
    std::fs::write(dir.join(".env"), "").unwrap();
    let vfs = Vfs::new(Arc::new(TestLocal), dir.display().to_string());

    let root = vfs.list_dir("").await.unwrap();
    let mut got: Vec<_> = root
        .items
        .iter()
        .map(|i| (i.name.as_str(), i.is_dir, i.ignored))
        .collect();
    got.sort();
    assert_eq!(
        got,
        [
            (".env", false, false),
            (".gitignore", false, false),
            ("run.log", false, true),
            ("src", true, false),
            ("target", true, true),
        ]
    );
    let src = vfs.list_dir("src").await.unwrap();
    assert_eq!(src.items.len(), 1);
    assert_eq!(src.items[0].name, "a b.rs");
    assert!(vfs.list_dir("../").await.is_err());
    assert!(vfs.list_dir("missing").await.is_err());
}

#[tokio::test]
async fn write_round_trips_and_detects_conflicts() {
    let dir = scratch("write-roundtrip");
    let vfs = Vfs::new(Arc::new(TestLocal), dir.display().to_string());

    let body = "第一行 'quoted' $(not run)\nline2\n";
    let w = vfs.write("docs/说明.md", body, None).await.unwrap();
    assert!(w.saved && w.size == body.len() as u64);
    let r = vfs.read("docs/说明.md").await.unwrap();
    assert_eq!(r.content, body);
    assert_eq!(r.mtime, w.mtime);

    let stale = vfs
        .write("docs/说明.md", "mine", Some(w.mtime.saturating_sub(100)))
        .await
        .unwrap();
    assert!(!stale.saved);
    assert_eq!(vfs.read("docs/说明.md").await.unwrap().content, body);

    assert!(
        vfs.write("docs/说明.md", "mine", Some(w.mtime))
            .await
            .unwrap()
            .saved
    );
    assert_eq!(vfs.read("docs/说明.md").await.unwrap().content, "mine");

    assert!(vfs.write("../escape.txt", "x", None).await.is_err());
    assert!(vfs.write("/etc/hosts", "x", None).await.is_err());
    assert!(vfs.write(".git/config", "x", None).await.is_err());
    assert!(
        vfs.write("a/.git/hooks/pre-commit", "x", None)
            .await
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn write_keeps_the_executable_bit() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("write-mode");
    let f = dir.join("run.sh");
    std::fs::write(&f, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
    let vfs = Vfs::new(Arc::new(TestLocal), dir.display().to_string());
    vfs.write("run.sh", "#!/bin/sh\necho hi\n", None)
        .await
        .unwrap();
    assert_eq!(
        std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

fn sh(dir: &std::path::Path, cmd: &str) {
    let st = std::process::Command::new("bash")
        .env("BASH_ENV", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(st.success(), "命令失败: {cmd}");
}

#[test]
fn git_octal_escapes_round_trip() {
    assert_eq!(
        unquote_git_path(r#""\346\226\207\346\241\243/\350\257\264\346\230\216.md""#),
        "文档/说明.md"
    );
    assert_eq!(unquote_git_path("plain.txt"), "plain.txt");
    assert_eq!(unquote_git_path(r#""a b.txt""#), "a b.txt");
    assert_eq!(unquote_git_path(r#""say \"hi\".txt""#), r#"say "hi".txt"#);
    assert_eq!(unquote_git_path(r#""back\\slash""#), r"back\slash");
    assert_eq!(unquote_git_path(r#""tab\there""#), "tab\there");

    assert_eq!(unquote_git_path(r#""\xyz""#), "xyz");
}

#[tokio::test]
async fn chinese_paths_get_their_change_marker() {
    let d = scratch("zhpath");
    sh(
        &d,
        "git init -q . && git config user.email t@t && git config user.name t",
    );
    std::fs::create_dir_all(d.join("文档")).unwrap();
    std::fs::write(d.join("文档/说明.md"), "a\n").unwrap();
    std::fs::write(d.join("plain.txt"), "b\n").unwrap();
    sh(&d, "git add -A && git commit -qm init");
    std::fs::write(d.join("文档/说明.md"), "changed\n").unwrap();

    let v = Vfs::new(Arc::new(TestLocal), d.to_str().unwrap());
    let tree = v.tree(None).await.unwrap();
    let find = |p: &str| tree.iter().find(|e| e.path == p);

    assert_eq!(
        find("文档/说明.md").and_then(|e| e.change),
        Some(ChangeKind::Modified),
        "中文文件要拿到自己的改动标记，树: {:?}",
        tree.iter().map(|e| &e.path).collect::<Vec<_>>()
    );
    assert!(
        !tree.iter().any(|e| e.path.contains("\\3")),
        "不能出现八进制转义的幽灵条目: {:?}",
        tree.iter().map(|e| &e.path).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn utf8_text_is_not_mistaken_for_binary() {
    let d = scratch("utf8");
    let mut text = String::new();
    for i in 0..600 {
        text.push_str(&format!("中文测试内容需要足够长以便被截断 {i}\n"));
    }
    std::fs::write(d.join("zh.md"), &text).unwrap();
    std::fs::write(d.join("bin.dat"), [0u8, 1, 2, 3, 0, 5]).unwrap();

    let v = Vfs::new(Arc::new(TestLocal), d.to_str().unwrap());
    let f = v.read("zh.md").await.unwrap();
    assert!(!f.binary, "中文纯文本不能被判成二进制");
    assert!(f.content.contains("中文测试内容"), "内容要真的读回来");

    let b = v.read("bin.dat").await.unwrap();
    assert!(b.binary, "真的含 NUL 的文件仍要判成二进制");
}

#[tokio::test]
async fn diff_works_before_the_first_commit() {
    let d = scratch("nocommit");
    sh(
        &d,
        "git init -q . && git config user.email t@t && git config user.name t",
    );
    std::fs::write(d.join("a.txt"), "hello\n").unwrap();

    let v = Vfs::new(Arc::new(TestLocal), d.to_str().unwrap());
    let out = v
        .diff("HEAD")
        .await
        .expect("没有提交也要出 diff 而不是报错");
    assert!(out.contains("a.txt"), "新增文件要出现在 diff 里: {out}");
    assert!(out.contains("+hello"), "内容要出现在 diff 里: {out}");
}

#[tokio::test]
async fn diff_on_a_plain_directory_says_so() {
    let d = scratch("nogit");
    std::fs::write(d.join("a.txt"), "x").unwrap();
    let v = Vfs::new(Arc::new(TestLocal), d.to_str().unwrap());
    let err = v.diff("HEAD").await.unwrap_err();
    assert!(
        matches!(err, VfsError::NotARepo(_)),
        "要能结构化地区分出「不是仓库」，调用方才好显示成空态而不是错误: {err}"
    );
}

#[tokio::test]
async fn looking_at_the_diff_does_not_change_the_users_repo() {
    let d = scratch("index-untouched");
    sh(
        &d,
        "git init -q . && git config user.email t@t && git config user.name t \
            && echo a > a && git add -A && git commit -qm i",
    );
    std::fs::write(d.join("新文件.txt"), "new\n").unwrap();
    let status = |d: &std::path::Path| {
        String::from_utf8(
            std::process::Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .args(["status", "--porcelain"])
                .current_dir(d)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
    };
    let before = status(&d);

    let v = Vfs::new(Arc::new(TestLocal), d.to_str().unwrap());
    let diff = v.diff("HEAD").await.unwrap();
    let _ = v.tree_with_stat(None).await.unwrap();

    assert!(
        diff.contains("新文件.txt"),
        "未跟踪的新文件仍要出现在 diff 里: {diff}"
    );
    assert_eq!(
        before,
        status(&d),
        "看 diff / 树不能改变用户仓库的 git status"
    );
}

#[tokio::test]
async fn tree_reports_how_much_changed_in_the_same_round_trip() {
    let d = scratch("diffstat");
    sh(
        &d,
        "git init -q . && git config user.email t@t && git config user.name t \
            && printf 'a\nb\nc\n' > f.txt && git add -A && git commit -qm i",
    );
    std::fs::write(d.join("f.txt"), "a\nB\nc\nd\n").unwrap();
    std::fs::write(d.join("new.txt"), "x\ny\n").unwrap();

    let v = Vfs::new(Arc::new(TestLocal), d.to_str().unwrap());
    let (_, stat) = v.tree_with_stat(None).await.unwrap();
    let stat = stat.expect("git 仓库要有改动量");
    assert_eq!(stat.files, 2, "{stat:?}");
    assert_eq!(stat.added, 4, "f.txt +2、new.txt +2: {stat:?}");
    assert_eq!(stat.removed, 1, "{stat:?}");
}

#[test]
fn stat_section_is_parsed_and_absent_outside_git() {
    assert_eq!(
        parse_stat("__TRACKED__\na\n__STATUS__\n__STAT__\n12 3 4\n"),
        Some(DiffStat {
            added: 12,
            removed: 3,
            files: 4
        })
    );
    assert_eq!(parse_stat("__PLAIN__\na\n"), None, "非 git 目录没有改动量");

    let t = parse_tree("__TRACKED__\na.rs\n__STATUS__\n__STAT__\n1 2 3\n");
    assert!(t.iter().all(|e| e.path != "1 2 3"), "{t:?}");
}

#[tokio::test]
async fn rename_and_remove_stay_inside_the_root() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("src/deep")).unwrap();
    std::fs::write(root.path().join("src/a.rs"), "a").unwrap();
    std::fs::write(root.path().join("src/b.rs"), "b").unwrap();
    std::fs::write(root.path().join("src/deep/x.txt"), "x").unwrap();
    let v = Vfs::new(Arc::new(TestLocal), root.path().to_string_lossy());
    assert_eq!(
        v.rename("src/a.rs", ".env.example").await.unwrap(),
        "src/.env.example"
    );
    assert!(root.path().join("src/.env.example").is_file());
    assert!(!root.path().join("src/a.rs").exists());
    let taken = v.rename("src/b.rs", ".env.example").await.unwrap_err();
    assert!(taken.to_string().contains("同名"), "{taken}");
    for bad in ["", ".", "..", "a/b", "../x"] {
        assert!(v.rename("src/b.rs", bad).await.is_err(), "{bad:?} 应被拒绝");
    }
    assert!(v.rename("../outside", "x").await.is_err());
    v.remove("src/deep").await.unwrap();
    assert!(!root.path().join("src/deep").exists());
    v.remove("src/deep").await.unwrap();
    assert!(v.remove("").await.is_err());
    assert!(v.remove("..").await.is_err());
    assert!(root.path().join("src/b.rs").is_file());
}

#[tokio::test]
async fn a_huge_diff_is_cut_on_the_node_instead_of_shipped_whole() {
    let d = scratch("huge-diff");
    sh(
        &d,
        "git init -q . && git config user.email t@t && git config user.name t \
            && echo a > a && git add -A && git commit -qm i",
    );
    let line = "中文与 ascii 混排的一行，用来检查截断不会切坏字符\n";
    std::fs::write(d.join("big.txt"), line.repeat(40_000)).unwrap();

    let v = Vfs::new(Arc::new(TestLocal), d.to_str().unwrap());
    let full = v.diff("HEAD").await.unwrap();
    let cap = 100_001;
    let cut = v
        .diff_opts(
            "HEAD",
            DiffOpts {
                max_bytes: Some(cap),
                ..DiffOpts::default()
            },
        )
        .await
        .expect("被 head 截断不算出错");
    assert!(full.len() > cap * 10, "测试要真的够大: {}", full.len());
    assert!(cut.len() <= cap + 2, "远端只回传上限内的字节: {}", cut.len());
    let keep = (0..=cap - 1)
        .rev()
        .find(|&i| full.is_char_boundary(i))
        .unwrap();
    assert_eq!(cut[..keep], full[..keep], "截断前的内容要和完整 diff 一字不差");
}
