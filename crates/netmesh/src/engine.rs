use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use blazar_transport::{ExecSpec, LocalTransport, NodeTransport};
use serde::Serialize;

use crate::invite::Invite;
use crate::{CliInvocation, MeshAdmin, MeshError, NodeStatus, Result};

pub const ENGINE_VERSION: &str = "2.6.4";

pub const SERVICE_NAME: &str = "blazar-mesh";

pub const RPC_PORTAL: &str = "127.0.0.1:15898";

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

#[derive(Debug, Clone)]
pub struct EngineLayout {
    pub root: PathBuf,
}

impl EngineLayout {
    #[must_use]
    pub fn system() -> Self {
        let root = if cfg!(target_os = "macos") {
            PathBuf::from("/Library/Application Support/Blazar/mesh")
        } else if cfg!(windows) {
            let base = std::env::var_os("ProgramData")
                .map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
            base.join("Blazar").join("mesh")
        } else {
            PathBuf::from("/opt/blazar/mesh")
        };
        Self { root }
    }

    #[must_use]
    pub fn bin_dir(&self) -> PathBuf {
        self.root.join("bin")
    }
    #[must_use]
    pub fn core(&self) -> PathBuf {
        self.bin_dir().join(exe("easytier-core"))
    }
    #[must_use]
    pub fn cli(&self) -> PathBuf {
        self.bin_dir().join(exe("easytier-cli"))
    }
    #[must_use]
    pub fn config(&self) -> PathBuf {
        self.root.join("config.toml")
    }
    #[must_use]
    pub fn log_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    #[must_use]
    pub fn joined(&self) -> bool {
        self.config().exists()
    }

    #[must_use]
    pub fn invocation(&self) -> CliInvocation {
        CliInvocation::Direct {
            program: self.cli().display().to_string(),
            rpc: Some(RPC_PORTAL.to_owned()),
        }
    }

    #[must_use]
    pub fn admin(&self) -> MeshAdmin {
        MeshAdmin::new(Arc::new(LocalTransport), self.invocation())
    }
}

#[derive(Debug, Clone)]
pub struct BundledEngine {
    pub dir: PathBuf,
}

impl BundledEngine {
    #[must_use]
    pub fn locate(explicit: Option<&Path>) -> Option<Self> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(p) = explicit {
            candidates.push(p.to_path_buf());
        }
        if let Some(p) = std::env::var_os("BLAZAR_ENGINE_DIR") {
            candidates.push(PathBuf::from(p));
        }
        if let Ok(me) = std::env::current_exe()
            && let Some(dir) = me.parent()
        {
            candidates.push(dir.join("engine"));

            candidates.push(dir.join("../Resources/engine"));

            candidates.push(dir.join("../lib/Blazar/engine"));
        }
        candidates
            .into_iter()
            .map(|dir| Self { dir })
            .find(Self::complete)
    }

    fn complete(&self) -> bool {
        self.dir.join(exe("easytier-core")).is_file()
            && self.dir.join(exe("easytier-cli")).is_file()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalMeshStatus {
    pub joined: bool,

    pub running: bool,
    pub node: Option<NodeStatus>,

    pub peer_count: usize,

    pub engine_bundled: bool,
    pub engine_version: &'static str,

    pub other_engine: Option<String>,

    pub external: Option<ExternalEngine>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExternalEngine {
    pub program: String,

    pub label: String,

    pub interface: Option<String>,
    pub virtual_ipv4: Option<String>,

    pub rpc: Option<String>,

    pub node: Option<NodeStatus>,
    pub peer_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ExternalHints {
    pub mesh_ips: Vec<std::net::Ipv4Addr>,

    pub rpc_candidates: Vec<String>,
}

pub async fn local_status(
    layout: &EngineLayout,
    bundle: Option<&BundledEngine>,
    hints: &ExternalHints,
) -> LocalMeshStatus {
    let joined = layout.joined();
    let (running, node, peer_count) = if joined && layout.cli().is_file() {
        let admin = layout.admin();
        match admin.status().await {
            Ok(node) => {
                let peers = crate::EasyTierMesh::new(Arc::new(LocalTransport), layout.invocation())
                    .peers()
                    .await
                    .map(|p| p.iter().filter(|p| !p.is_self()).count())
                    .unwrap_or(0);
                (true, Some(node), peers)
            }
            Err(_) => (false, None, 0),
        }
    } else {
        (false, None, 0)
    };
    let other = other_engine().await;
    let external = match &other {
        Some(program) if !running => {
            Some(detect_external(program, cli_program(layout, bundle), hints).await)
        }
        _ => None,
    };
    LocalMeshStatus {
        joined,
        running,
        node,
        peer_count,
        engine_bundled: bundle.is_some(),
        engine_version: ENGINE_VERSION,
        other_engine: other,
        external,
    }
}

#[must_use]
pub fn cli_program(layout: &EngineLayout, bundle: Option<&BundledEngine>) -> String {
    if layout.cli().is_file() {
        return layout.cli().display().to_string();
    }
    if let Some(b) = bundle {
        let p = b.dir.join(exe("easytier-cli"));
        if p.is_file() {
            return p.display().to_string();
        }
    }
    exe("easytier-cli")
}

async fn detect_external(program: &str, cli: String, hints: &ExternalHints) -> ExternalEngine {
    let label = if program.contains("easytier-gui") {
        "easytier-gui"
    } else {
        "easytier-core"
    }
    .to_owned();
    let addrs = interface_addrs().await;
    let (interface, virtual_ipv4) = match_mesh_iface(&addrs, &hints.mesh_ips)
        .map_or((None, None), |(i, ip)| (Some(i), Some(ip.to_string())));

    let mut ext = ExternalEngine {
        program: program.to_owned(),
        label,
        interface,
        virtual_ipv4,
        rpc: None,
        node: None,
        peer_count: 0,
    };
    for rpc in &hints.rpc_candidates {
        let inv = CliInvocation::Direct {
            program: cli.clone(),
            rpc: Some(rpc.clone()),
        };
        let admin = MeshAdmin::new(Arc::new(LocalTransport), inv.clone());

        let Ok(Ok(node)) = tokio::time::timeout(Duration::from_secs(3), admin.status()).await
        else {
            continue;
        };
        if node.hostname.is_empty() && node.virtual_ipv4.is_empty() {
            continue;
        }
        ext.peer_count = crate::EasyTierMesh::new(Arc::new(LocalTransport), inv)
            .peers()
            .await
            .map(|p| p.iter().filter(|p| !p.is_self()).count())
            .unwrap_or(0);
        if ext.virtual_ipv4.is_none() && !node.virtual_ipv4.is_empty() {
            ext.virtual_ipv4 = Some(node.virtual_ipv4.clone());
        }
        ext.rpc = Some(rpc.clone());
        ext.node = Some(node);
        break;
    }
    ext
}

async fn interface_addrs() -> Vec<(String, std::net::Ipv4Addr)> {
    let (prog, args): (&str, &[&str]) = if cfg!(target_os = "linux") {
        ("ip", &["-4", "-o", "addr", "show"])
    } else if cfg!(windows) {
        return Vec::new();
    } else {
        ("ifconfig", &[])
    };
    let Ok(out) = LocalTransport
        .exec(ExecSpec::new(prog).args(args.iter().copied()))
        .await
    else {
        return Vec::new();
    };
    parse_iface_addrs(&out.stdout)
}

fn parse_iface_addrs(text: &str) -> Vec<(String, std::net::Ipv4Addr)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();

        if fields.len() >= 4 && fields[0].ends_with(':') && fields[2] == "inet" {
            if let Some(ip) = fields[3].split('/').next().and_then(|s| s.parse().ok()) {
                out.push((fields[1].to_owned(), ip));
            }
            continue;
        }

        if !line.starts_with(char::is_whitespace) {
            if let Some((name, _)) = line.split_once(':') {
                cur = name.to_owned();
            }
            continue;
        }
        if fields.first() == Some(&"inet")
            && let Some(ip) = fields.get(1).and_then(|s| s.parse().ok())
        {
            out.push((cur.clone(), ip));
        }
    }
    out
}

fn match_mesh_iface(
    addrs: &[(String, std::net::Ipv4Addr)],
    mesh_ips: &[std::net::Ipv4Addr],
) -> Option<(String, std::net::Ipv4Addr)> {
    let same24 =
        |a: &std::net::Ipv4Addr, b: &std::net::Ipv4Addr| a.octets()[..3] == b.octets()[..3];
    let physical = |n: &str| {
        [
            "en", "eth", "wl", "lo", "bridge", "docker", "br-", "veth", "awdl", "llw",
        ]
        .iter()
        .any(|p| n.starts_with(p))
    };
    addrs
        .iter()
        .filter(|(n, ip)| !physical(n) && !ip.is_loopback())
        .find(|(_, ip)| mesh_ips.iter().any(|m| m != ip && same24(m, ip)))
        .cloned()
}

async fn other_engine() -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    let out = LocalTransport
        .exec(ExecSpec::new("ps").args(["-axo", "args="]))
        .await
        .ok()?;
    let ours = EngineLayout::system().bin_dir().display().to_string();
    find_other_engine(&out.stdout, &ours)
}

fn find_other_engine(ps: &str, ours: &str) -> Option<String> {
    ps.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with(ours))
        .find_map(|l| {
            ["easytier-core", "easytier-gui"].iter().find_map(|name| {
                l.match_indices(name).find_map(|(i, _)| {
                    let end = i + name.len();
                    l[end..]
                        .chars()
                        .next()
                        .is_none_or(char::is_whitespace)
                        .then(|| l[..end].to_owned())
                })
            })
        })
}

fn q(p: impl AsRef<str>) -> String {
    format!("'{}'", p.as_ref().replace('\'', r"'\''"))
}

fn qp(p: &Path) -> String {
    q(p.display().to_string())
}

#[must_use]
pub fn unix_install_script(
    layout: &EngineLayout,
    bundle: &BundledEngine,
    staged_config: &Path,
) -> String {
    let root = qp(&layout.root);
    let bin = qp(&layout.bin_dir());
    let cli = qp(&layout.cli());
    let mut s = String::from("set -eu\numask 022\n");
    s.push_str(&format!(
        "mkdir -p {bin} {logs}\n",
        logs = qp(&layout.log_dir())
    ));

    s.push_str(&format!("chown -R 0:0 {root}\nchmod 755 {root} {bin}\n"));
    for name in ["easytier-core", "easytier-cli"] {
        let src = qp(&bundle.dir.join(name));
        let dst = layout.bin_dir().join(name);
        let tmp = qp(&dst.with_extension("new"));

        s.push_str(&format!(
            "cp -f {src} {tmp}\nchown 0:0 {tmp}\nchmod 755 {tmp}\nmv -f {tmp} {dst}\n",
            dst = qp(&dst)
        ));
    }
    if cfg!(target_os = "macos") {
        s.push_str(&format!(
            "xattr -dr com.apple.quarantine {bin} 2>/dev/null || true\n"
        ));
    }
    let cfg = layout.config();
    let cfg_tmp = qp(&cfg.with_extension("new"));
    s.push_str(&format!(
        "cp -f {staged} {cfg_tmp}\nchown 0:0 {cfg_tmp}\nchmod 600 {cfg_tmp}\nmv -f {cfg_tmp} {cfg}\n",
        staged = qp(staged_config),
        cfg = qp(&cfg),
    ));
    s.push_str(&format!(
        "{cli} service -n {svc} stop >/dev/null 2>&1 || true\n\
         {cli} service -n {svc} uninstall >/dev/null 2>&1 || true\n",
        svc = SERVICE_NAME
    ));

    s.push_str(&format!(
        "{cli} service -n {svc} install --display-name 'Blazar Mesh' \
         --description 'Blazar 团队组网（EasyTier {ver}）' \
         --core-path {core} --service-work-dir {root} -- \
         -c {cfg} --rpc-portal {rpc} --file-log-dir {logs} --file-log-level info >/dev/null\n\
         {cli} service -n {svc} start\n",
        svc = SERVICE_NAME,
        ver = ENGINE_VERSION,
        core = qp(&layout.core()),
        cfg = qp(&cfg),
        rpc = RPC_PORTAL,
        logs = qp(&layout.log_dir()),
    ));
    s
}

#[must_use]
pub fn unix_leave_script(layout: &EngineLayout) -> String {
    let cli = qp(&layout.cli());
    format!(
        "set -u\n\
         if [ -x {cli} ]; then\n  {cli} service -n {svc} stop >/dev/null 2>&1 || true\n  \
         {cli} service -n {svc} uninstall >/dev/null 2>&1 || true\nfi\n\
         rm -f {cfg}\n",
        svc = SERVICE_NAME,
        cfg = qp(&layout.config()),
    )
}

fn ps(p: impl AsRef<str>) -> String {
    format!("'{}'", p.as_ref().replace('\'', "''"))
}

#[must_use]
pub fn windows_install_script(
    layout: &EngineLayout,
    bundle: &BundledEngine,
    staged_config: &Path,
) -> String {
    let root = ps(layout.root.display().to_string());
    let bin = ps(layout.bin_dir().display().to_string());
    let cli = ps(layout.cli().display().to_string());
    let cfg = ps(layout.config().display().to_string());
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         New-Item -ItemType Directory -Force -Path {bin}, {logs} | Out-Null\n\
         icacls {root} /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' '*S-1-5-32-545:(OI)(CI)RX' | Out-Null\n\
         & {cli} service -n {svc} stop 2>$null\n\
         & {cli} service -n {svc} uninstall 2>$null\n\
         Copy-Item -Force -Path (Join-Path {src} '*') -Destination {bin}\n\
         Copy-Item -Force -Path {staged} -Destination {cfg}\n\
         icacls {cfg} /inheritance:r /grant:r '*S-1-5-18:F' '*S-1-5-32-544:F' | Out-Null\n\
         & {cli} service -n {svc} install --display-name 'Blazar Mesh' --core-path {core} --service-work-dir {root} -- -c {cfg} --rpc-portal {rpc} --file-log-dir {logs} --file-log-level info | Out-Null\n\
         & {cli} service -n {svc} start\n\
         exit $LASTEXITCODE\n",
        logs = ps(layout.log_dir().display().to_string()),
        src = ps(bundle.dir.display().to_string()),
        staged = ps(staged_config.display().to_string()),
        core = ps(layout.core().display().to_string()),
        svc = SERVICE_NAME,
        rpc = RPC_PORTAL,
    )
}

#[must_use]
pub fn windows_leave_script(layout: &EngineLayout) -> String {
    let cli = ps(layout.cli().display().to_string());
    format!(
        "if (Test-Path {cli}) {{ & {cli} service -n {svc} stop 2>$null; & {cli} service -n {svc} uninstall 2>$null }}\n\
         Remove-Item -Force -ErrorAction SilentlyContinue {cfg}\n",
        svc = SERVICE_NAME,
        cfg = ps(layout.config().display().to_string()),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Elevation {
    Gui,

    Terminal,
}

async fn run_elevated(script: &Path, how: Elevation, reason: &str) -> Result<()> {
    let path = script.display().to_string();
    let spec = if cfg!(target_os = "macos") && how == Elevation::Gui {
        let lit = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        ExecSpec::new("osascript").arg("-e").arg(format!(
            "do shell script \"/bin/sh \" & quoted form of {} with prompt {} with administrator privileges",
            lit(&path),
            lit(reason)
        ))
    } else if cfg!(windows) {
        ExecSpec::new("powershell").args([
            "-NoProfile".to_owned(),
            "-Command".to_owned(),
            format!(
                "$p = Start-Process powershell -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
                 -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File',{}; exit $p.ExitCode",
                ps(&path)
            ),
        ])
    } else if how == Elevation::Gui
        && (std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some())
    {
        ExecSpec::new("pkexec").args(["/bin/sh", &path])
    } else {
        ExecSpec::new("sudo").args(["/bin/sh", &path])
    };
    let out = LocalTransport.exec(spec).await?;
    if out.code != 0 {
        let msg = out.stderr.trim();

        let cancelled = msg.contains("-128") || msg.contains("User canceled") || out.code == 126;
        return Err(MeshError::Engine(if cancelled {
            "已取消管理员授权".into()
        } else {
            format!("以管理员身份执行失败（退出码 {}）：{msg}", out.code)
        }));
    }
    Ok(())
}

struct Staging(PathBuf);

impl Staging {
    fn new(parent: &Path) -> Result<Self> {
        let dir = parent.join(format!(
            "join-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).map_err(io_err)?;
        restrict(&dir, 0o700)?;
        Ok(Self(dir))
    }

    fn write(&self, name: &str, body: &str) -> Result<PathBuf> {
        let p = self.0.join(name);
        std::fs::write(&p, body).map_err(io_err)?;
        restrict(&p, 0o600)?;
        Ok(p)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
fn restrict(p: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).map_err(io_err)
}

#[cfg(not(unix))]
fn restrict(_p: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

fn io_err(e: std::io::Error) -> MeshError {
    MeshError::Engine(format!("文件操作失败：{e}"))
}

pub async fn join(
    invite: &Invite,
    bundle: &BundledEngine,
    layout: &EngineLayout,
    staging_parent: &Path,
    how: Elevation,
) -> Result<LocalMeshStatus> {
    install(
        &invite.engine_config()?,
        &invite.network_name,
        bundle,
        layout,
        staging_parent,
        how,
    )
    .await
}

pub async fn install(
    config: &str,
    network_name: &str,
    bundle: &BundledEngine,
    layout: &EngineLayout,
    staging_parent: &Path,
    how: Elevation,
) -> Result<LocalMeshStatus> {
    let staging = Staging::new(staging_parent)?;
    let staged = staging.write("config.toml", config)?;
    let script = if cfg!(windows) {
        staging.write("join.ps1", &windows_install_script(layout, bundle, &staged))?
    } else {
        staging.write("join.sh", &unix_install_script(layout, bundle, &staged))?
    };
    run_elevated(
        &script,
        how,
        &format!("Blazar 要把本机加入团队组网「{network_name}」。"),
    )
    .await?;
    drop(staging);
    wait_online(layout, bundle).await
}

pub async fn running_config(layout: &EngineLayout) -> Result<String> {
    let out = LocalTransport
        .exec(layout.invocation().spec(&["-o", "json", "node", "config"]))
        .await?;
    if out.code != 0 {
        return Err(MeshError::Engine(format!(
            "读取运行中的配置失败：{}",
            out.stderr.trim()
        )));
    }
    let raw = out.stdout.trim();

    let start = raw.find('"').unwrap_or(0);
    Ok(serde_json::from_str::<String>(&raw[start..]).unwrap_or_else(|_| raw.to_owned()))
}

pub async fn leave(layout: &EngineLayout, staging_parent: &Path, how: Elevation) -> Result<()> {
    let staging = Staging::new(staging_parent)?;
    let script = if cfg!(windows) {
        staging.write("leave.ps1", &windows_leave_script(layout))?
    } else {
        staging.write("leave.sh", &unix_leave_script(layout))?
    };
    run_elevated(&script, how, "Blazar 要让本机退出团队组网。").await
}

async fn wait_online(layout: &EngineLayout, bundle: &BundledEngine) -> Result<LocalMeshStatus> {
    for _ in 0..30 {
        let st = local_status(layout, Some(bundle), &ExternalHints::default()).await;
        if st.running {
            return Ok(st);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(MeshError::Engine(format!(
        "服务已安装，但引擎 15 秒内没有上线。日志在 {}",
        layout.log_dir().display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> EngineLayout {
        EngineLayout {
            root: PathBuf::from("/Library/Application Support/Blazar/mesh"),
        }
    }

    fn bundle() -> BundledEngine {
        BundledEngine {
            dir: PathBuf::from("/Applications/Blazar.app/Contents/Resources/engine"),
        }
    }

    #[test]
    fn install_copies_engine_out_of_the_user_writable_bundle() {
        let s = unix_install_script(&layout(), &bundle(), Path::new("/tmp/x/config.toml"));

        assert!(
            s.contains("--core-path '/Library/Application Support/Blazar/mesh/bin/easytier-core'")
        );
        assert!(!s.contains("--core-path '/Applications"));
        assert!(s.contains("chown -R 0:0 '/Library/Application Support/Blazar/mesh'"));
        assert!(s.contains("chmod 600 '/Library/Application Support/Blazar/mesh/config.new'"));
        assert!(s.contains(&format!("--rpc-portal {RPC_PORTAL}")));
        assert!(s.contains(&format!("-n {SERVICE_NAME} install")));
    }

    #[test]
    fn install_is_rerunnable() {
        let s = unix_install_script(&layout(), &bundle(), Path::new("/tmp/c"));
        let stop = s.find("stop >/dev/null 2>&1 || true").unwrap();
        let install = s.find(" install ").unwrap();
        assert!(stop < install);
    }

    #[test]
    fn paths_with_quotes_stay_quoted() {
        let evil = BundledEngine {
            dir: PathBuf::from("/Users/o'neil/$(touch pwn)/engine"),
        };
        let s = unix_install_script(&layout(), &evil, Path::new("/tmp/c"));
        assert!(s.contains(r"'/Users/o'\''neil/$(touch pwn)/engine/easytier-core'"));
    }

    #[test]
    fn leave_removes_the_credential() {
        let s = unix_leave_script(&layout());
        assert!(s.contains("rm -f '/Library/Application Support/Blazar/mesh/config.toml'"));
        assert!(s.contains("uninstall"));
    }

    #[test]
    fn windows_script_locks_down_acl() {
        let l = EngineLayout {
            root: PathBuf::from(r"C:\ProgramData\Blazar\mesh"),
        };
        let b = BundledEngine {
            dir: PathBuf::from(r"C:\Program Files\Blazar\engine"),
        };
        let s = windows_install_script(&l, &b, Path::new(r"C:\Users\a\cfg.toml"));
        assert!(s.contains("/inheritance:r"));
        assert!(s.contains("'*S-1-5-32-545:(OI)(CI)RX'"), "普通用户只读");
    }

    #[test]
    fn finds_mesh_address_on_tun_interface() {
        let mac = "en0: flags=8863<UP> mtu 1500\n\tinet 10.0.5.5 netmask 0xfffffe00\n\
                   utun6: flags=8051<UP> mtu 1380\n\tinet 10.99.0.198 --> 10.99.0.198 netmask 0xffffffff\n\
                   utun7: flags=8051<UP> mtu 1500\n\tinet 198.18.0.1 --> 198.18.0.1 netmask 0xffffffff\n";
        let addrs = parse_iface_addrs(mac);
        assert_eq!(addrs.len(), 3);
        let mesh: Vec<std::net::Ipv4Addr> =
            vec!["10.99.0.1".parse().unwrap(), "10.99.0.16".parse().unwrap()];
        let (iface, ip) = match_mesh_iface(&addrs, &mesh).unwrap();
        assert_eq!(iface, "utun6");
        assert_eq!(ip.to_string(), "10.99.0.198");

        let linux = "1: lo    inet 127.0.0.1/8 scope host lo\n2: eth0    inet 10.99.0.77/24 brd x\n5: et0    inet 10.99.0.20/24 brd x\n";
        let addrs = parse_iface_addrs(linux);

        assert_eq!(match_mesh_iface(&addrs, &mesh).unwrap().0, "et0");

        assert!(match_mesh_iface(&addrs, &[]).is_none());
    }

    #[test]
    fn detects_other_easytier_even_inside_app_bundles() {
        let ps = "/sbin/launchd\n\
            /Applications/easytier-gui.app/Contents/MacOS/easytier-gui --x\n\
            /Library/Application Support/Blazar/mesh/bin/easytier-core -c cfg\n";
        let ours = "/Library/Application Support/Blazar/mesh/bin";
        assert_eq!(
            find_other_engine(ps, ours).as_deref(),
            Some("/Applications/easytier-gui.app/Contents/MacOS/easytier-gui")
        );

        let only_ours = "/Library/Application Support/Blazar/mesh/bin/easytier-core -c cfg\n";
        assert_eq!(find_other_engine(only_ours, ours), None);
        assert_eq!(
            find_other_engine("/usr/bin/easytier-core\n", ours).as_deref(),
            Some("/usr/bin/easytier-core")
        );
    }

    #[test]
    fn staging_is_private_and_cleaned_up() {
        let parent = tempfile::tempdir().unwrap();
        let dir;
        {
            let st = Staging::new(parent.path()).unwrap();
            let f = st.write("config.toml", "secret").unwrap();
            dir = st.0.clone();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode(&dir), 0o700);
                assert_eq!(mode(&f), 0o600);
            }
        }
        assert!(!dir.exists(), "含私钥的中转目录必须被删除");
    }
}
