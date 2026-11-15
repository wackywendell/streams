//! A private conversation endpoint, carrying the same typed requests as Pi's pipe.
use super::*;
use crate::child_protocol::{self, ClientFrame, Outcome, Request, RequestId, ServerFrame};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::{UnixListener, UnixStream},
    },
    time::Duration,
};

pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(2);
pub const MAX_CONTROL_FRAME: u64 = 16 * 1024;

pub struct ControlEndpoint {
    path: PathBuf,
    _lock: crate::file_io::FileLock,
}
impl ControlEndpoint {
    pub fn bind(preference_path: &Path) -> Result<(Self, UnixListener)> {
        let lock = crate::file_io::try_lock(&preference_path.with_extension("control.lock"))?;
        // macOS Unix socket addresses are short; catalog/config paths need not be.
        let directory =
            fs::canonicalize("/tmp")?.join(format!("strm-{}", unsafe { libc::geteuid() }));
        crate::file_io::ensure_directory(&directory, true)?;
        let metadata = fs::metadata(&directory)?;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(invalid(
                "card control directory must be private to its user",
            ));
        }
        let path = directory.join(format!(
            "{}.sock",
            digest(preference_path.as_os_str().as_encoded_bytes())
        ));
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(&path)?,
            Ok(_) => return Err(invalid("card control path is not a socket")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let listener = UnixListener::bind(&path)?;
        Ok((Self { path, _lock: lock }, listener))
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for ControlEndpoint {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn request(path: &Path, request: Request) -> Result<Outcome> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(CONTROL_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_TIMEOUT))?;
    let id = RequestId::new(1).expect("fixed valid request ID");
    serde_json::to_writer(
        &mut stream,
        &ClientFrame::Request {
            id,
            request,
            pi_context: None,
        },
    )
    .map_err(|error| invalid(error.to_string()))?;
    stream.write_all(b"\n")?;
    let mut line = String::new();
    BufReader::new(stream.take(MAX_CONTROL_FRAME)).read_line(&mut line)?;
    if !line.ends_with('\n') {
        return Err(invalid("incomplete card control response"));
    }
    match child_protocol::decode_server(&line).map_err(|error| invalid(error.to_string()))? {
        ServerFrame::Response {
            id: response_id,
            outcome,
        } if response_id == id => Ok(outcome),
        _ => Err(invalid("unexpected card control response")),
    }
}

pub fn closed(path: &Path) -> Result<()> {
    use crate::{child_protocol::CardTarget, config::CardMode};
    match request(
        path,
        Request::SetCardMode {
            target: CardTarget::Session,
            mode: CardMode::Manual,
        },
    )? {
        Outcome::Success { .. } => Ok(()),
        Outcome::SavedWithWarnings { warnings, .. } => Err(invalid(warnings.messages().join("; "))),
        Outcome::Error { error } => Err(invalid(error.message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_is_exclusive_and_stable_after_release_or_a_stale_socket() {
        let home = tempfile::tempdir().unwrap();
        let key = home.path().join("preference.json");
        let (endpoint, listener) = ControlEndpoint::bind(&key).unwrap();
        let path = endpoint.path().to_owned();
        assert!(ControlEndpoint::bind(&key).is_err());
        drop(listener);
        drop(endpoint);
        assert!(!path.exists());
        let stale = UnixListener::bind(&path).unwrap();
        drop(stale);
        let (next, _listener) = ControlEndpoint::bind(&key).unwrap();
        assert_eq!(next.path(), path);
    }
}
