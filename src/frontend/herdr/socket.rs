//! Protocol 22 uses one socket per ordinary response; subscriptions own theirs.

use super::FrontendError;
use serde_json::{Value, json};
#[cfg(test)]
use std::io::BufRead;
use std::io::{self, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
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
        let mut stream = super::super::socket_io::connect(
            &self.path,
            deadline,
            self.cancellation.as_ref(),
            "Herdr",
        )
        .map_err(socket_error)?;
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
    super::super::socket_io::read_frame_into(
        reader,
        deadline,
        frame,
        cancellation,
        "Herdr",
        MAX_FRAME,
    )
}
