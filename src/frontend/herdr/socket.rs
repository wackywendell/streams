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
    cancellation: Option<crate::cancellation::Cancellation>,
}

impl SocketClient {
    pub fn connect(path: PathBuf, timeout: f64) -> Result<Self, FrontendError> {
        Self::connect_inner(path, timeout, None)
    }

    pub fn connect_cancellable(
        path: PathBuf,
        timeout: f64,
        cancellation: crate::cancellation::Cancellation,
    ) -> Result<Self, FrontendError> {
        Self::connect_inner(path, timeout, Some(cancellation))
    }

    fn connect_inner(
        path: PathBuf,
        timeout: f64,
        cancellation: Option<crate::cancellation::Cancellation>,
    ) -> Result<Self, FrontendError> {
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
            cancellation,
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
        self.exchange(method, params).1
    }

    /// One ordinary request plus the raw reply bytes, retained as effect evidence
    /// even when the reply is partial, malformed or reports an error.
    pub fn exchange(&self, method: &str, params: Value) -> (Vec<u8>, Result<Value, FrontendError>) {
        let mut frame = Vec::new();
        let result = self
            .send(method, params, &mut frame)
            .and_then(|(_, id)| decode(&id, &frame));
        (frame, result)
    }

    /// Return the reader too, retaining bytes received after the acknowledgement.
    pub fn start(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(BufReader<UnixStream>, Value), FrontendError> {
        let mut frame = Vec::new();
        let (reader, id) = self.send(method, params, &mut frame)?;
        Ok((reader, decode(&id, &frame)?))
    }

    fn send(
        &self,
        method: &str,
        params: Value,
        frame: &mut Vec<u8>,
    ) -> Result<(BufReader<UnixStream>, String), FrontendError> {
        let deadline = Instant::now() + self.timeout;
        let mut stream =
            connect(&self.path, deadline, self.cancellation.as_ref()).map_err(socket_error)?;
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
        read_frame_into(
            &mut reader,
            Some(deadline),
            frame,
            self.cancellation.as_ref(),
        )
        .map_err(socket_error)?;
        Ok((reader, id))
    }
}

fn decode(id: &str, frame: &[u8]) -> Result<Value, FrontendError> {
    // Strict: a duplicated identity field must not silently pick a winner.
    let mut response = std::str::from_utf8(frame)
        .map_err(|e| e.to_string())
        .and_then(|text| crate::codec::load_json(text).map_err(|e| e.to_string()))
        .map_err(|e| FrontendError::invalid(format!("invalid Herdr response: {e}")))?;
    if response["id"].as_str() != Some(id) {
        return Err(FrontendError::invalid("Herdr response ID mismatch"));
    }
    if let Some(error) = response.get("error") {
        return Err(FrontendError::invalid(format!(
            "Herdr request failed: {error}"
        )));
    }
    match response.get_mut("result") {
        Some(result) if result.is_object() => Ok(result.take()),
        _ => Err(FrontendError::invalid("invalid Herdr result shape")),
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
    read_frame_into(reader, deadline, &mut frame, None)?;
    Ok(frame)
}

/// Appends to `frame`, so a caller keeps the partial bytes of a failed read.
fn read_frame_into(
    reader: &mut BufReader<UnixStream>,
    deadline: Option<Instant>,
    frame: &mut Vec<u8>,
    cancellation: Option<&crate::cancellation::Cancellation>,
) -> io::Result<()> {
    loop {
        if let Some(cancellation) = cancellation {
            cancellation.check()?;
        }
        if let Some(deadline) = deadline
            && reader.buffer().is_empty()
        {
            use std::os::fd::AsRawFd;
            wait(
                reader.get_ref().as_raw_fd(),
                libc::POLLIN,
                deadline,
                cancellation,
            )?;
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
            return Ok(());
        }
    }
}

// A full local listen backlog can block connect too. Use a nonblocking socket
// and wait within the same absolute deadline, including interrupted polls.
fn connect(
    path: &Path,
    deadline: Instant,
    cancellation: Option<&crate::cancellation::Cancellation>,
) -> io::Result<UnixStream> {
    if let Some(cancellation) = cancellation {
        cancellation.check()?;
    }
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
        wait(stream.as_raw_fd(), libc::POLLOUT, deadline, cancellation)?;
        if let Some(error) = stream.take_error()? {
            return Err(error);
        }
    }
    stream.set_nonblocking(false)?;
    Ok(stream)
}

fn wait(
    fd: std::os::fd::RawFd,
    events: libc::c_short,
    deadline: Instant,
    cancellation: Option<&crate::cancellation::Cancellation>,
) -> io::Result<()> {
    let mut descriptors = [
        libc::pollfd {
            fd,
            events,
            revents: 0,
        },
        libc::pollfd {
            fd: cancellation.map_or(-1, |cancel| cancel.descriptor()),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    loop {
        if let Some(cancellation) = cancellation {
            cancellation.check()?;
        }
        let duration = remaining(deadline).map_err(io::Error::other)?;
        let millis = duration.as_millis().saturating_add(1).min(i32::MAX as u128) as i32;
        match unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as _, millis) } {
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
                    "Herdr request timed out",
                ));
            }
            _ => {
                if let Some(cancellation) = cancellation {
                    cancellation.check()?;
                }
                return Ok(());
            }
        }
    }
}
