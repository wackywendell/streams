//! Owner-only read validation and a secure stdin pager; filenames never reach less.
use super::{CliError, Result};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    process::{Command, Stdio},
};
use streams::{config::ViewerMode, runtime::launch::resolve_executable};
#[allow(dead_code)]
const DOCUMENT_BYTES: u64 = 4 * 1024 * 1024;
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct FileIdentity {
    device: u64,
    inode: u64,
}
#[allow(dead_code)]
pub(super) fn observe_file(path: &Path) -> Result<FileIdentity> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(CliError::message("document viewer requires a regular file"));
    }
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}
#[allow(dead_code)]
pub(super) fn read_document(path: &Path, expected: FileIdentity) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || expected
            != (FileIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
    {
        return Err(CliError::message("document file changed during resolution"));
    }
    if metadata.len() > DOCUMENT_BYTES {
        return Err(CliError::message("document exceeds 4 MiB"));
    }
    read_bounded(file)
}
#[allow(dead_code)]
fn read_bounded(file: File) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(DOCUMENT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > DOCUMENT_BYTES {
        return Err(CliError::message("document exceeds 4 MiB"));
    }
    Ok(bytes)
}
pub(super) fn show(program: &Path, content: &[u8], kind: &str) -> Result<()> {
    if !program.is_absolute() {
        return Err(CliError::message("pager executable must be absolute"));
    }
    let mut pager = Command::new(program)
        .arg("--")
        .env("LESSSECURE", "1")
        .env("LESS", "")
        .env_remove("LESSOPEN")
        .env_remove("LESSCLOSE")
        .stdin(Stdio::piped())
        .spawn()?;
    let written = pager
        .stdin
        .take()
        .expect("piped pager input")
        .write_all(content);
    let status = pager.wait()?;
    if !status.success() {
        return Err(CliError::message(format!(
            "{kind} viewer exited with {status}"
        )));
    }
    if let Err(error) = written
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(error.into());
    }
    Ok(())
}

/// Open an existing regular file in the configured terminal viewer (`editor`, `less`, or `hunk`).
pub(super) fn view_file(
    mode: ViewerMode,
    environment: &BTreeMap<String, String>,
    cwd: &Path,
    path: &Path,
) -> Result<()> {
    match mode {
        ViewerMode::Less | ViewerMode::Native => {
            let program = resolve_executable("less", environment, cwd)
                .map_err(|_| CliError::message("less is unavailable"))?;
            let bytes = std::fs::read(path).map_err(|error| {
                CliError::message(format!("cannot read {}: {error}", path.display()))
            })?;
            show(&program, &bytes, "document")
        }
        ViewerMode::Editor => {
            let editor = environment
                .get("VISUAL")
                .or_else(|| environment.get("EDITOR"))
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or("vi");
            let parts: Vec<&str> = editor.split_whitespace().collect();
            let program = resolve_executable(parts[0], environment, cwd)
                .map_err(|_| CliError::message(format!("editor {:?} is unavailable", parts[0])))?;
            let status = Command::new(program).args(&parts[1..]).arg(path).status()?;
            if !status.success() {
                return Err(CliError::message(format!("editor exited with {status}")));
            }
            Ok(())
        }
        ViewerMode::Hunk => {
            let program = resolve_executable("hunk", environment, cwd)
                .map_err(|_| CliError::message("hunk is unavailable"))?;
            let status = Command::new(program).arg(path).status()?;
            if !status.success() {
                return Err(CliError::message(format!("hunk exited with {status}")));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::{ffi::OsStrExt, fs::symlink},
    };
    #[test]
    fn verified_identity_and_exact_size_are_required_before_reading() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("document");
        fs::write(&path, b"saved working bytes").unwrap();
        let identity = observe_file(&path).unwrap();
        assert_eq!(
            read_document(&path, identity).unwrap(),
            b"saved working bytes"
        );
        fs::rename(&path, directory.path().join("original")).unwrap();
        fs::write(&path, b"replacement").unwrap();
        assert!(read_document(&path, identity).is_err());
        let file = File::options().write(true).open(&path).unwrap();
        file.set_len(DOCUMENT_BYTES).unwrap();
        assert_eq!(
            read_document(&path, observe_file(&path).unwrap())
                .unwrap()
                .len() as u64,
            DOCUMENT_BYTES
        );
        file.set_len(DOCUMENT_BYTES + 1).unwrap();
        assert!(read_document(&path, observe_file(&path).unwrap()).is_err());
        assert!(
            read_bounded(File::open(&path).unwrap()).is_err(),
            "bounded read also rejects growth beyond a previously checked size"
        );
    }
    #[test]
    fn symlink_and_fifo_replacements_fail_without_following_or_blocking() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("document");
        fs::write(&path, b"original").unwrap();
        let identity = observe_file(&path).unwrap();
        let original = directory.path().join("original");
        fs::rename(&path, &original).unwrap();
        symlink(&original, &path).unwrap();
        assert!(read_document(&path, identity).is_err());
        fs::remove_file(&path).unwrap();
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(read_document(&path, identity).is_err());
    }
    #[test]
    fn executable_resolution_preserves_symlink_spelling_and_avoids_template_names() {
        use std::{collections::BTreeMap, os::unix::fs::PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert!(
            streams::runtime::launch::resolve_executable("sh", &BTreeMap::new(), root)
                .unwrap()
                .is_absolute()
        );
        assert!(
            streams::runtime::launch::resolve_executable(
                "sh",
                &BTreeMap::from([("PATH".into(), String::new())]),
                root
            )
            .is_err(),
            "an explicitly empty PATH is not the system default"
        );
        let program = root.join("launcher");
        fs::write(&program, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        symlink(&program, root.join("pi")).unwrap();
        let environment = BTreeMap::from([("PATH".into(), root.display().to_string())]);
        assert_eq!(
            streams::runtime::launch::resolve_executable("pi", &environment, root).unwrap(),
            root.join("pi")
        );
        assert!(
            streams::runtime::launch::resolve_executable("missing", &environment, root).is_err()
        );
        fs::set_permissions(&program, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(streams::runtime::launch::resolve_executable("pi", &environment, root).is_err());
    }
}
