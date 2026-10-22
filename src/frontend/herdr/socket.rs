//! Protocol 22 uses one socket per ordinary response; subscriptions own theirs.

use super::FrontendError;
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[cfg(test)]
pub(crate) mod tests;

pub const PROTOCOL: u64 = 22;
const MAX_FRAME: usize = 16 * 1024 * 1024;

pub struct SocketClient {
    pub path: PathBuf,
    timeout: Duration,
    sequence: AtomicU64,
}

impl SocketClient {
    pub fn connect(path: PathBuf, timeout: f64) -> Result<Self, FrontendError> {
        crate::process::deadline(timeout, "Herdr")
            .map_err(|e| FrontendError::invalid(e.message))?;
        let timeout = Duration::from_secs_f64(timeout);
        if !path.is_absolute() {
            return Err(FrontendError::invalid("HERDR_SOCKET_PATH must be absolute"));
        }
        let client = Self {
            path,
            timeout,
            sequence: AtomicU64::new(1),
        };
        let pong = client.request("ping", json!({}))?;
        if pong["type"] != "pong"
            || pong["protocol"].as_u64() != Some(PROTOCOL)
            || pong["version"].as_str().is_none()
        {
            return Err(FrontendError::invalid(
                "unsupported Herdr protocol; protocol 22 required",
            ));
        }
        Ok(client)
    }

    pub fn request(&self, method: &str, params: Value) -> Result<Value, FrontendError> {
        let (_, result) = self.start(method, params)?;
        Ok(result)
    }

    /// Return the reader too, retaining bytes received after the acknowledgement.
    pub fn start(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(BufReader<UnixStream>, Value), FrontendError> {
        let deadline = Instant::now() + self.timeout;
        let mut stream = connect(&self.path, deadline).map_err(socket_error)?;
        let id = self.sequence.fetch_add(1, Ordering::Relaxed).to_string();
        let request = json!({"id":id,"method":method,"params":params});
        let mut bytes =
            serde_json::to_vec(&request).map_err(|e| FrontendError::invalid(e.to_string()))?;
        bytes.push(b'\n');
        stream
            .set_write_timeout(Some(remaining(deadline)?))
            .map_err(socket_error)?;
        stream.write_all(&bytes).map_err(socket_error)?;
        let mut reader = BufReader::new(stream);
        let bytes = read_frame(&mut reader, Some(deadline)).map_err(socket_error)?;
        let response: Value = serde_json::from_slice(&bytes)
            .map_err(|e| FrontendError::invalid(format!("invalid Herdr response: {e}")))?;
        if response["id"].as_str() != Some(&id) {
            return Err(FrontendError::invalid("Herdr response ID mismatch"));
        }
        if let Some(error) = response.get("error") {
            return Err(FrontendError::invalid(format!(
                "Herdr request failed: {error}"
            )));
        }
        let result = response
            .get("result")
            .filter(|v| v.is_object())
            .ok_or_else(|| FrontendError::invalid("invalid Herdr result shape"))?;
        Ok((reader, result.clone()))
    }
}

fn remaining(deadline: Instant) -> Result<Duration, FrontendError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|v| !v.is_zero())
        .ok_or_else(|| FrontendError::invalid("Herdr request timed out"))
}

pub(super) fn socket_error(error: io::Error) -> FrontendError {
    FrontendError::invalid(format!("Herdr socket: {error}"))
}

/// Bounded framing without losing coalesced frames. Idle subscriptions have no
/// timeout; ordinary replies use an absolute deadline, including partial reads.
pub(super) fn read_frame(
    reader: &mut BufReader<UnixStream>,
    deadline: Option<Instant>,
) -> io::Result<Vec<u8>> {
    let mut frame = Vec::new();
    loop {
        if let Some(deadline) = deadline
            && reader.buffer().is_empty()
        {
            use std::os::fd::AsRawFd;
            let duration = remaining(deadline).map_err(io::Error::other)?;
            let mut poll = libc::pollfd {
                fd: reader.get_ref().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let millis = duration.as_millis().saturating_add(1).min(i32::MAX as u128) as i32;
            match unsafe { libc::poll(&mut poll, 1, millis) } {
                -1 => {
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(error);
                }
                0 => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Herdr response timed out",
                    ));
                }
                _ => {}
            }
        }
        let buffer = match reader.fill_buf() {
            Ok(buffer) => buffer,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if buffer.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Herdr socket closed before a complete frame",
            ));
        }
        let length = buffer
            .iter()
            .position(|v| *v == b'\n')
            .map(|i| i + 1)
            .unwrap_or(buffer.len());
        if frame.len() + length > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Herdr frame exceeds 16 MiB",
            ));
        }
        let complete = buffer[length - 1] == b'\n';
        frame.extend_from_slice(&buffer[..length]);
        reader.consume(length);
        if complete {
            return Ok(frame);
        }
    }
}

// A full local listen backlog can block connect too. Use a nonblocking socket
// and wait within the same absolute deadline, including interrupted polls.
fn connect(path: &Path, deadline: Instant) -> io::Result<UnixStream> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.contains(&0) || bytes.len() >= address.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid Herdr socket path",
        ));
    }
    address.sun_family = libc::AF_UNIX as _;
    for (target, byte) in address.sun_path.iter_mut().zip(bytes) {
        *target = *byte as _;
    }
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let stream = UnixStream::from(fd);
    stream.set_nonblocking(true)?;
    let result = unsafe {
        libc::connect(
            stream.as_raw_fd(),
            &address as *const _ as *const libc::sockaddr,
            std::mem::size_of_val(&address) as _,
        )
    };
    if result < 0 {
        let error = io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(libc::EINPROGRESS | libc::EINTR)) {
            return Err(error);
        }
        let mut poll = libc::pollfd {
            fd: stream.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        loop {
            let timeout = remaining(deadline).map_err(io::Error::other)?;
            let millis = timeout.as_millis().saturating_add(1).min(i32::MAX as u128) as i32;
            match unsafe { libc::poll(&mut poll, 1, millis) } {
                -1 => {
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(error);
                }
                0 => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Herdr connect timed out",
                    ));
                }
                _ => break,
            }
        }
        if let Some(error) = stream.take_error()? {
            return Err(error);
        }
    }
    stream.set_nonblocking(false)?;
    Ok(stream)
}
