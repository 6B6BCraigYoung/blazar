use super::*;
use futures::StreamExt;

async fn local_state(root: &std::path::Path) -> Shared {
    let db = blazar_db::Db::open_in_memory().await.unwrap();
    AppState::with_services(
        db,
        "local".into(),
        None,
        crate::mesh::MeshCtx::new(None, root.to_path_buf()),
        crate::services::Services::Isolated,
    )
}

async fn workspace(st: &Shared, path: &std::path::Path) -> String {
    let created = create_workspace(
        State(st.clone()),
        Json(CreateWorkspace {
            node: "local".into(),
            path: path.to_string_lossy().into_owned(),
            name: None,
            project: None,
        }),
    )
    .await
    .map_err(|e| e.message())
    .unwrap();
    created.0["id"].as_str().unwrap().to_owned()
}

fn query(path: &str, dir: bool) -> DownloadQuery {
    DownloadQuery {
        path: path.into(),
        dir,
    }
}

/// 不可压缩的伪随机字节，保证 gzip 后依然很大，节点进程会被背压卡住。
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

fn header(r: &Response, name: axum::http::HeaderName) -> Option<String> {
    r.headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_owned())
}

/// 把响应体逐帧读完；返回读到的字节、第一帧的大小，以及是否以错误结束。
async fn drain(r: Response) -> (Vec<u8>, usize, bool) {
    let mut body = r.into_body().into_data_stream();
    let (mut all, mut first, mut failed) = (Vec::new(), 0, false);
    while let Some(frame) = body.next().await {
        match frame {
            Ok(b) => {
                if all.is_empty() {
                    first = b.len();
                }
                all.extend_from_slice(&b);
            }
            Err(_) => {
                failed = true;
                break;
            }
        }
    }
    (all, first, failed)
}

async fn text(r: Response) -> String {
    let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn untar(gz: &[u8]) -> tempfile::TempDir {
    let out = tempfile::tempdir().unwrap();
    let file = out.path().join("x.tar.gz");
    std::fs::write(&file, gz).unwrap();
    let ok = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&file)
        .arg("-C")
        .arg(out.path())
        .status()
        .unwrap()
        .success();
    assert!(ok, "打包结果解不开");
    std::fs::remove_file(file).unwrap();
    out
}

fn sha256(path: &std::path::Path) -> String {
    let out = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .or_else(|_| std::process::Command::new("sha256sum").arg(path).output())
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// 本测试进程派生出来、命令行满足条件的进程。
fn our_processes(wanted: impl Fn(&str) -> bool) -> Vec<u32> {
    let out = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,command="])
        .output()
        .unwrap();
    let rows: Vec<(u32, u32, String)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let ppid = it.next()?.parse().ok()?;
            Some((pid, ppid, it.collect::<Vec<_>>().join(" ")))
        })
        .collect();
    let mut family = vec![std::process::id()];
    loop {
        let more: Vec<u32> = rows
            .iter()
            .filter(|(pid, ppid, _)| family.contains(ppid) && !family.contains(pid))
            .map(|(pid, _, _)| *pid)
            .collect();
        if more.is_empty() {
            break;
        }
        family.extend(more);
    }
    rows.iter()
        .filter(|(pid, _, cmd)| *pid != std::process::id() && family.contains(pid) && wanted(cmd))
        .map(|(pid, _, _)| *pid)
        .collect()
}

fn alive(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .is_ok_and(|o| {
            let stat = String::from_utf8_lossy(&o.stdout);
            let stat = stat.trim();
            !stat.is_empty() && !stat.starts_with('Z')
        })
}

async fn wait_gone(pids: &[u32]) -> Vec<u32> {
    for _ in 0..50 {
        let left: Vec<u32> = pids.iter().copied().filter(|p| alive(*p)).collect();
        if left.is_empty() {
            return left;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    pids.iter().copied().filter(|p| alive(*p)).collect()
}

#[tokio::test]
async fn files_stream_with_their_exact_size_and_name() {
    let root = tempfile::tempdir().unwrap();
    let st = local_state(root.path()).await;
    let dir = tempfile::tempdir().unwrap();
    let name = "报告 \"v2\" 100%.bin";
    let data = noise(3 * 1024 * 1024 + 17, 7);
    std::fs::create_dir(dir.path().join("子 目录")).unwrap();
    std::fs::write(dir.path().join("子 目录").join(name), &data).unwrap();
    std::fs::write(dir.path().join("empty"), b"").unwrap();
    let ws = workspace(&st, dir.path()).await;

    let r = serve_download(&st, &ws, &query(&format!("子 目录/{name}"), false), 1 << 30).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        header(&r, axum::http::header::CONTENT_LENGTH).as_deref(),
        Some(data.len().to_string().as_str())
    );
    assert_eq!(
        header(&r, axum::http::header::CONTENT_DISPOSITION).unwrap(),
        "attachment; filename=\"__ _v2_ 100%.bin\"; filename*=UTF-8''%E6%8A%A5%E5%91%8A%20%22v2%22%20100%25.bin"
    );
    let (got, first, failed) = drain(r).await;
    assert!(!failed);
    assert!(got == data, "下载内容和原文件不一致");
    assert!(
        first <= 64 * 1024,
        "第一帧 {first} 字节：响应应该边读边发，而不是读完整个文件再发"
    );

    let r = serve_download(&st, &ws, &query("empty", false), 1 << 30).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        header(&r, axum::http::header::CONTENT_LENGTH).as_deref(),
        Some("0")
    );
    let (got, _, failed) = drain(r).await;
    assert!(got.is_empty() && !failed);
}

#[tokio::test]
async fn folders_stream_as_tar_gz_without_a_length() {
    let root = tempfile::tempdir().unwrap();
    let st = local_state(root.path()).await;
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("资料 夹");
    std::fs::create_dir_all(pkg.join("sub")).unwrap();
    let blob = noise(300_000, 3);
    std::fs::write(pkg.join("a.bin"), &blob).unwrap();
    std::fs::write(pkg.join("sub/b.txt"), "hello").unwrap();
    let ws = workspace(&st, dir.path()).await;

    let r = serve_download(&st, &ws, &query("资料 夹", true), 1 << 30).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(header(&r, axum::http::header::CONTENT_LENGTH), None);
    assert_eq!(
        header(&r, axum::http::header::CONTENT_TYPE).as_deref(),
        Some("application/gzip")
    );
    assert!(
        header(&r, axum::http::header::CONTENT_DISPOSITION)
            .unwrap()
            .ends_with("filename*=UTF-8''%E8%B5%84%E6%96%99%20%E5%A4%B9.tar.gz")
    );
    let (gz, _, failed) = drain(r).await;
    assert!(!failed);
    let out = untar(&gz);
    assert!(std::fs::read(out.path().join("a.bin")).unwrap() == blob);
    assert_eq!(
        std::fs::read_to_string(out.path().join("sub/b.txt")).unwrap(),
        "hello"
    );

    let r = serve_download(&st, &ws, &query("", true), 1 << 30).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(
        header(&r, axum::http::header::CONTENT_DISPOSITION)
            .unwrap()
            .contains(".tar.gz")
    );
    let (gz, _, failed) = drain(r).await;
    assert!(!failed);
    assert!(untar(&gz).path().join("资料 夹/sub/b.txt").exists());
}

#[tokio::test]
async fn git_folders_skip_ignored_files_and_survive_deleted_ones() {
    let root = tempfile::tempdir().unwrap();
    let st = local_state(root.path()).await;
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false"
                ])
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap()
                .status
                .success()
        );
    };
    git(&["init", "-q"]);
    std::fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(dir.path().join("kept.txt"), "kept").unwrap();
    std::fs::write(dir.path().join("gone.txt"), "gone").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "init"]);
    std::fs::remove_file(dir.path().join("gone.txt")).unwrap();
    std::fs::write(dir.path().join("noise.log"), "noise").unwrap();
    std::fs::write(dir.path().join("new.txt"), "new").unwrap();
    let ws = workspace(&st, dir.path()).await;

    let r = serve_download(&st, &ws, &query(".", true), 1 << 30).await;
    assert_eq!(r.status(), StatusCode::OK);
    let (gz, _, failed) = drain(r).await;
    assert!(!failed, "工作区里删掉的已跟踪文件不应让打包失败");
    let out = untar(&gz);
    assert!(out.path().join("kept.txt").exists());
    assert!(out.path().join("new.txt").exists());
    assert!(!out.path().join("noise.log").exists());
    assert!(!out.path().join("gone.txt").exists());
}

#[tokio::test]
async fn downloads_refuse_paths_outside_the_workspace_and_wrong_kinds() {
    let root = tempfile::tempdir().unwrap();
    let st = local_state(root.path()).await;
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), "secret").unwrap();
    std::fs::create_dir(dir.path().join("pkg")).unwrap();
    std::fs::write(dir.path().join("a.txt"), "a").unwrap();
    std::fs::write(dir.path().join("big.bin"), noise(4096, 1)).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path().join("secret"), dir.path().join("link")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("dir-link")).unwrap();
    }
    let ws = workspace(&st, dir.path()).await;
    let escape = format!(
        "../{}/secret",
        outside.path().file_name().unwrap().to_string_lossy()
    );

    for (path, folder) in [
        ("../secret", false),
        (escape.as_str(), false),
        ("/etc/passwd", false),
        ("pkg/../../x", true),
        ("link", false),
        ("dir-link/secret", false),
        ("dir-link", true),
    ] {
        let r = serve_download(&st, &ws, &query(path, folder), 1 << 30).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(text(r).await, "路径越界", "{path}");
    }

    for (path, folder, why) in [
        ("pkg", false, "pkg 不是一个文件"),
        ("a.txt", true, "a.txt 不是一个文件夹"),
        ("missing.txt", false, "missing.txt 不是一个文件"),
    ] {
        let r = serve_download(&st, &ws, &query(path, folder), 1 << 30).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(text(r).await, why);
    }

    let r = serve_download(&st, &ws, &query("big.bin", false), 4095).await;
    assert_eq!(r.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(text(r).await.contains("太大了"));
    let r = serve_download(&st, &ws, &query("big.bin", false), 4096).await;
    assert_eq!(r.status(), StatusCode::OK);

    std::fs::write(dir.path().join("pkg/noise.bin"), noise(200_000, 9)).unwrap();
    let r = serve_download(&st, &ws, &query("pkg", true), 64 * 1024).await;
    assert_eq!(r.status(), StatusCode::OK);
    let (got, _, failed) = drain(r).await;
    assert!(failed, "打包超过上限时响应体要以错误结束");
    assert!(got.len() <= 64 * 1024);
}

#[tokio::test]
async fn a_file_that_shrinks_mid_download_fails_instead_of_looking_complete() {
    let root = tempfile::tempdir().unwrap();
    let st = local_state(root.path()).await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("shrinking.bin");
    std::fs::write(&file, noise(24 * 1024 * 1024, 5)).unwrap();
    let ws = workspace(&st, dir.path()).await;

    let r = serve_download(&st, &ws, &query("shrinking.bin", false), 1 << 30).await;
    let mut body = r.into_body().into_data_stream();
    let mut got = body.next().await.unwrap().unwrap().len();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_len(1000)
        .unwrap();
    let mut failed = false;
    while let Some(frame) = body.next().await {
        match frame {
            Ok(b) => got += b.len(),
            Err(e) => {
                assert!(e.to_string().contains("变了"), "{e}");
                failed = true;
            }
        }
    }
    assert!(failed);
    assert!(got < 24 * 1024 * 1024);
}

#[tokio::test]
async fn cancelling_a_download_leaves_no_process_behind() {
    let root = tempfile::tempdir().unwrap();
    let st = local_state(root.path()).await;
    let dir = tempfile::tempdir().unwrap();
    let marker = format!("cancel-{}.bin", std::process::id());
    std::fs::write(dir.path().join(&marker), noise(24 * 1024 * 1024, 11)).unwrap();
    std::fs::create_dir(dir.path().join("cancel-dir")).unwrap();
    std::fs::write(
        dir.path().join("cancel-dir/x.bin"),
        noise(24 * 1024 * 1024, 13),
    )
    .unwrap();
    let ws = workspace(&st, dir.path()).await;

    for (path, folder) in [(marker.as_str(), false), ("cancel-dir", true)] {
        let r = serve_download(&st, &ws, &query(path, folder), 1 << 30).await;
        let mut body = r.into_body().into_data_stream();
        assert!(!body.next().await.unwrap().unwrap().is_empty());
        let running = if folder {
            our_processes(|cmd| cmd.contains("tar") && cmd.contains("-czf -"))
        } else {
            our_processes(|cmd| cmd.contains("head -c") && cmd.contains(&marker))
        };
        assert!(
            !running.is_empty(),
            "{path}：下载中应该能看到节点上的读取进程"
        );
        drop(body);
        let left = wait_gone(&running).await;
        assert!(left.is_empty(), "{path}：取消后还有进程没退出：{left:?}");
    }
}

#[tokio::test]
async fn a_downloaded_file_hashes_the_same_as_the_original() {
    let root = tempfile::tempdir().unwrap();
    let st = local_state(root.path()).await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("same.zip");
    std::fs::write(&file, noise(7_600_000, 21)).unwrap();
    let ws = workspace(&st, dir.path()).await;

    let r = serve_download(&st, &ws, &query("same.zip", false), 1 << 30).await;
    let (got, _, failed) = drain(r).await;
    assert!(!failed);
    let copy = dir.path().join("copy.zip");
    std::fs::write(&copy, got).unwrap();
    assert_eq!(sha256(&copy), sha256(&file));
    assert_eq!(sha256(&file).len(), 64);
}
