use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(path)?;
        directory.set_permissions(std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    if !std::fs::symlink_metadata(path)?.is_dir() {
        return Err(io::Error::other("数据目录不能是符号链接"));
    }
    Ok(())
}

pub(crate) fn prepare_database(path: &Path) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(file) => restrict_file(file)?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            restrict_file(open_existing(path)?)?;
        }
        Err(error) => return Err(error),
    }
    restrict_sidecars(path)
}

pub(crate) fn restrict_sidecars(path: &Path) -> io::Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        match open_existing(Path::new(&sidecar)) {
            Ok(file) => restrict_file(file)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn open_existing(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(io::Error::other("数据库文件不能是符号链接或特殊文件"));
    }
    options.open(path)
}

fn restrict_file(file: File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("数据库文件必须是普通文件"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.nlink() != 1 {
            return Err(io::Error::other("数据库文件不能包含硬链接"));
        }
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn newly_created_app_directories_are_private() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("blazar");
        let profile = root.join("profiles").join("default");
        ensure_private_dir(&profile).unwrap();
        for path in [&root, &root.join("profiles"), &profile] {
            assert_eq!(mode(path), 0o700);
        }
    }

    #[test]
    fn linked_directory_is_rejected_without_changing_target_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("shared");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = temp.path().join("app");
        symlink(&target, &link).unwrap();
        assert!(ensure_private_dir(&link).is_err());
        assert_eq!(mode(&target), 0o755);
    }

    #[test]
    fn linked_files_are_rejected_without_changing_target_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("fixture");
        std::fs::write(&target, "fixture only").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        let link = temp.path().join("db.sqlite");
        symlink(&target, &link).unwrap();
        assert!(prepare_database(&link).is_err());
        std::fs::remove_file(&link).unwrap();
        std::fs::hard_link(&target, &link).unwrap();
        assert!(prepare_database(&link).is_err());
        assert_eq!(mode(&target), 0o644);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "fixture only");
    }

    #[test]
    fn linked_sidecars_are_rejected_without_changing_target_permissions() {
        for suffix in ["-wal", "-shm", "-journal"] {
            let temp = tempfile::tempdir().unwrap();
            let target = temp.path().join("fixture");
            std::fs::write(&target, "fixture only").unwrap();
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
            symlink(&target, temp.path().join(format!("db.sqlite{suffix}"))).unwrap();
            assert!(prepare_database(&temp.path().join("db.sqlite")).is_err());
            assert_eq!(mode(&target), 0o644);
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "fixture only");
        }
    }
}
