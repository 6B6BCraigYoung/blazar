use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Directory,
    File,
    Parent,
    VerifyFile,
}

#[cfg(any(windows, test))]
#[derive(Debug, serde::Serialize)]
struct Request<'a> {
    path: &'a str,
    kind: Kind,
}

#[cfg(any(windows, test))]
fn protect_with(
    path: &Path,
    kind: Kind,
    runner: &impl Fn(&Request<'_>) -> io::Result<()>,
) -> io::Result<()> {
    let path = path
        .to_str()
        .ok_or_else(|| io::Error::other("私密存储路径不是有效 Unicode"))?;
    runner(&Request { path, kind })
}

#[cfg(any(windows, test))]
fn open_with(
    path: &Path,
    options: &OpenOptions,
    runner: &impl Fn(&Request<'_>) -> io::Result<()>,
) -> io::Result<File> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    protect_with(parent, Kind::Parent, runner).map_err(|error| {
        io::Error::new(error.kind(), format!("私密存储父目录未通过权限校验；请使用 Blazar 专用私密目录，或在 Windows 安全设置中限制为当前用户与 SYSTEM：{error}"))
    })?;
    let exists = match std::fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    if exists {
        protect_with(path, Kind::File, runner)?;
    }
    let file = options.open(path)?;
    if !exists {
        protect_with(path, Kind::File, runner)?;
    }
    Ok(file)
}

pub fn open(path: &Path, options: &OpenOptions) -> io::Result<File> {
    #[cfg(windows)]
    {
        open_with(path, options, &run_powershell)
    }
    #[cfg(not(windows))]
    {
        options.open(path)
    }
}

pub fn protect_file(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        protect_with(path, Kind::File, &run_powershell)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(())
    }
}

pub fn verify_file(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        protect_with(path, Kind::VerifyFile, &run_powershell)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(())
    }
}

pub fn protect_directory(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        protect_with(path, Kind::Directory, &run_powershell)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(())
    }
}

pub fn write(path: &Path, content: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    open(path, &options)?.write_all(content)
}

#[cfg(any(windows, test))]
const POWERSHELL: &str = r#"$ErrorActionPreference = 'Stop'
try {
    [Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false)
    $request = [Console]::In.ReadToEnd() | ConvertFrom-Json
    if ($request.kind -notin @('directory', 'file', 'parent', 'verify_file')) { throw 'Invalid path kind' }
    $path = [System.IO.Path]::GetFullPath([string]$request.path)
    $cursor = $path
    while ($null -ne $cursor) {
        $item = $null
        try { $item = Get-Item -LiteralPath $cursor -Force } catch [System.Management.Automation.ItemNotFoundException] {}
        if ($null -ne $item -and ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Reparse points are not private storage' }
        $parent = [System.IO.Directory]::GetParent($cursor)
        if ($null -eq $parent) { $cursor = $null } else { $cursor = $parent.FullName }
    }
    $user = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
    $system = [System.Security.Principal.SecurityIdentifier]::new('S-1-5-18')
    $directory = $request.kind -in @('directory', 'parent')
    $verifyOnly = $request.kind -in @('parent', 'verify_file')
    if ($directory) {
        $security = [System.Security.AccessControl.DirectorySecurity]::new()
        $inheritance = [System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [System.Security.AccessControl.InheritanceFlags]::ObjectInherit
    } else {
        $security = [System.Security.AccessControl.FileSecurity]::new()
        $inheritance = [System.Security.AccessControl.InheritanceFlags]::None
    }
    $security.SetAccessRuleProtection($true, $false)
    $security.SetOwner($user)
    foreach ($sid in @($user, $system)) {
        $rule = [System.Security.AccessControl.FileSystemAccessRule]::new($sid, [System.Security.AccessControl.FileSystemRights]::FullControl, $inheritance, [System.Security.AccessControl.PropagationFlags]::None, [System.Security.AccessControl.AccessControlType]::Allow)
        $security.AddAccessRule($rule)
    }
    if ($directory -and -not $verifyOnly -and -not (Test-Path -LiteralPath $path)) {
        [System.IO.Directory]::CreateDirectory($path, $security) | Out-Null
    }
    $item = Get-Item -LiteralPath $path -Force
    if ([bool]$item.PSIsContainer -ne $directory) { throw 'Path kind does not match' }
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Reparse points are not private storage' }
    if (-not $verifyOnly) { Set-Acl -LiteralPath $path -AclObject $security }
    $actual = Get-Acl -LiteralPath $path
    if (-not $actual.AreAccessRulesProtected -or $actual.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $user.Value) { throw 'Private ACL verification failed' }
    $seen = @{}
    foreach ($rule in $actual.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier])) {
        $identity = $rule.IdentityReference.Value
        if (($identity -ne $user.Value -and $identity -ne $system.Value) -or $rule.IsInherited -or $rule.AccessControlType -ne [System.Security.AccessControl.AccessControlType]::Allow -or $rule.FileSystemRights -ne [System.Security.AccessControl.FileSystemRights]::FullControl -or $rule.InheritanceFlags -ne $inheritance -or $rule.PropagationFlags -ne [System.Security.AccessControl.PropagationFlags]::None) { throw 'Unexpected private ACL rule' }
        $seen[$identity] = $true
    }
    if (-not $seen.ContainsKey($user.Value) -or -not $seen.ContainsKey($system.Value)) { throw 'Missing private ACL rule' }
    exit 0
} catch {
    exit 1
}"#;

#[cfg(any(windows, test))]
fn powershell_command(program: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        POWERSHELL,
    ]);
    command
}

#[cfg(any(windows, test))]
fn run_powershell(request: &Request<'_>) -> io::Result<()> {
    use std::io::Write;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let system = std::env::var_os("SystemRoot")
        .ok_or_else(|| io::Error::other("无法定位 Windows 系统目录，不能保护私密文件"))?;
    let program =
        std::path::PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut child = powershell_command(&program)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let input = serde_json::to_vec(request)?;
    if let Err(error) = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("无法向权限程序发送路径"))?
        .write_all(&input)
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "无法保护 Windows 私密文件；请检查当前用户权限、路径是否包含链接，以及文件系统是否支持 ACL",
                ))
            };
        }
        if started.elapsed() >= Duration::from_secs(15) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "设置 Windows 私密文件权限超时，已取消写入",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::Write;
    use std::path::PathBuf;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("blazar-private-{}", uuid::Uuid::now_v7()));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn options() -> OpenOptions {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        options
    }

    #[test]
    fn private_storage_denied_directory_does_not_create_a_secret_file() {
        let root = Fixture::new();
        let path = root.0.join("auth.json");
        let result = open_with(&path, &options(), &|_| {
            Err(io::Error::other("fixture ACL denied"))
        });
        assert!(result.is_err());
        assert!(!path.exists());
    }

    #[test]
    fn private_storage_denied_existing_file_does_not_truncate_its_contents() {
        let root = Fixture::new();
        let path = root.0.join("auth.json");
        std::fs::write(&path, "previous synthetic credential").unwrap();
        let result = open_with(&path, &options(), &|request| {
            if request.kind == Kind::File {
                Err(io::Error::other("fixture ACL denied"))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "previous synthetic credential"
        );
    }

    #[test]
    fn private_storage_checks_parent_then_protects_empty_file_before_returning_handle() {
        let root = Fixture::new();
        let path = root.0.join("auth.json");
        let calls = RefCell::new(Vec::new());
        let mut file = open_with(&path, &options(), &|request| {
            calls.borrow_mut().push(request.kind);
            if request.kind == Kind::Parent {
                assert!(!path.exists());
            } else {
                assert_eq!(std::fs::metadata(&path)?.len(), 0);
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(*calls.borrow(), vec![Kind::Parent, Kind::File]);
        file.write_all(b"synthetic credential").unwrap();
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "synthetic credential"
        );
    }

    #[test]
    fn private_storage_new_file_acl_failure_returns_no_writable_handle() {
        let root = Fixture::new();
        let path = root.0.join("auth.json");
        let result = open_with(&path, &options(), &|request| {
            if request.kind == Kind::File {
                Err(io::Error::other("fixture ACL denied"))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert_eq!(std::fs::metadata(path).unwrap().len(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn private_storage_reparse_rejection_preserves_the_link_target() {
        let root = Fixture::new();
        let target = root.0.join("outside.txt");
        let link = root.0.join("auth.json");
        std::fs::write(&target, "synthetic original").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let result = open_with(&link, &options(), &|request| {
            if std::fs::symlink_metadata(request.path)?
                .file_type()
                .is_symlink()
            {
                Err(io::Error::other("fixture reparse point rejected"))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(target).unwrap(),
            "synthetic original"
        );
    }

    #[test]
    fn private_storage_powershell_program_does_not_interpolate_paths() {
        let _runner: fn(&Request<'_>) -> io::Result<()> = run_powershell;
        let command = powershell_command(Path::new("/fixture/powershell"));
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args,
            [
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                POWERSHELL
            ]
        );
    }

    #[test]
    fn private_storage_permission_requests_distinguish_verification_from_changes() {
        for kind in [Kind::Directory, Kind::File, Kind::Parent, Kind::VerifyFile] {
            protect_with(Path::new("/fixture/private"), kind, &|request| {
                assert_eq!(request.kind, kind);
                Ok(())
            })
            .unwrap();
        }
    }

    #[test]
    fn private_storage_path_is_serialized_as_data() {
        let path = Path::new("C:/fixture/quote' $() [x]/token.json");
        protect_with(path, Kind::File, &|request| {
            let encoded = serde_json::to_string(request)?;
            let decoded: serde_json::Value = serde_json::from_str(&encoded)?;
            assert_eq!(decoded["path"], path.to_str().unwrap());
            assert_eq!(decoded["kind"], "file");
            Ok(())
        })
        .unwrap();
    }
}
