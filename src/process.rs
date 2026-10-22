//! Bounded Unix subprocess execution shared by Git and native frontends.
//!
//! The deadline includes concurrent pipe I/O and descendants retaining pipes.
//! Failure terminates the process group and reaps the child; dispatched effects
//! may nevertheless have completed. Callers must not blindly retry mutations.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Captured command output, including nonzero or signal exits.
#[derive(Debug, Clone)]
pub struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Execution failure; partial output remains available for recovery inspection.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct ProcessError {
    pub message: String,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// An explicitly scoped native command. No shell or ambient environment is added.
#[derive(Debug, Clone)]
pub struct ProcessRequest {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub input: Option<Vec<u8>>,
    pub env: BTreeMap<OsString, OsString>,
    /// Only these exit codes are accepted; signals always fail.
    pub allowed: Vec<i32>,
    pub timeout: f64,
}

/// Injectable transport. Implementations must bound execution and never retry.
pub trait ProcessRunner: Send + Sync {
    fn run(&self, request: &ProcessRequest) -> Result<ProcessOutput, ProcessError>;
}

/// Production runner using the same executor as Git.
#[derive(Debug, Default)]
pub struct NativeProcessRunner;

impl ProcessRunner for NativeProcessRunner {
    fn run(&self, request: &ProcessRequest) -> Result<ProcessOutput, ProcessError> {
        let deadline = deadline(request.timeout, "native command")?;
        let mut cmd = Command::new(&request.program);
        cmd.args(&request.args).env_clear().envs(&request.env);
        if let Some(cwd) = &request.cwd {
            cmd.current_dir(cwd);
        }
        let output = execute(
            &mut cmd,
            request.input.as_deref(),
            deadline,
            "native command",
        )?;
        if !output
            .status
            .code()
            .is_some_and(|code| request.allowed.contains(&code))
        {
            return Err(ProcessError {
                message: format!(
                    "native command failed ({}): {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
                code: output.status.code(),
                stdout: output.stdout,
                stderr: output.stderr,
            });
        }
        Ok(output)
    }
}

/// Reject invalid deadlines before dispatch.
pub fn deadline(timeout: f64, label: &str) -> Result<Instant, ProcessError> {
    Duration::try_from_secs_f64(timeout)
        .ok()
        .filter(|timeout| !timeout.is_zero())
        .and_then(|timeout| Instant::now().checked_add(timeout))
        .ok_or_else(|| ProcessError {
            message: format!("{label} timeout must be finite, positive, and representable"),
            code: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
}

// Limit each turn so a continuously writable/readable pipe cannot starve the deadline.
fn drain(pipe: &mut Option<impl Read>, output: &mut Vec<u8>) -> io::Result<()> {
    let Some(reader) = pipe.as_mut() else {
        return Ok(());
    };
    let mut buf = [0; 8192];
    for _ in 0..8 {
        match reader.read(&mut buf) {
            Ok(0) => {
                *pipe = None;
                break;
            }
            Ok(n) => output.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    // These descriptors are owned pipes; retain their existing status flags.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags == -1
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn terminate(child: &mut Child, mut message: String) -> ProcessError {
    // The group remains addressable even if the child exited before a descendant closed its pipes.
    if unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) } == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            message.push_str(&format!("; terminating process group failed: {error}"));
        }
    }
    if let Err(error) = child.wait() {
        message.push_str(&format!("; reaping process failed: {error}"));
    }
    ProcessError {
        message,
        code: None,
        stdout: Vec::new(),
        stderr: Vec::new(),
    }
}

/// Execute a configured command with process-group cleanup and a total deadline.
pub fn execute(
    cmd: &mut Command,
    input: Option<&[u8]>,
    deadline: Instant,
    label: &str,
) -> Result<ProcessOutput, ProcessError> {
    cmd.process_group(0);
    let mut out = Vec::new();
    let mut err = Vec::new();
    if input.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| ProcessError {
        message: format!("{label} could not complete: {e}"),
        code: None,
        stdout: Vec::new(),
        stderr: Vec::new(),
    })?;
    let result = (|| -> io::Result<_> {
        let mut stdin = child.stdin.take();
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        if let Some(pipe) = &stdin {
            nonblocking(pipe)?;
        }
        nonblocking(stdout.as_ref().unwrap())?;
        nonblocking(stderr.as_ref().unwrap())?;
        let mut remaining = input.unwrap_or_default();
        let mut status = None;
        loop {
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("configured {label} timeout elapsed"),
                ));
            }
            drain(&mut stdout, &mut out)?;
            drain(&mut stderr, &mut err)?;
            if let Some(writer) = stdin.as_mut() {
                if !remaining.is_empty() {
                    let len = remaining.len().min(65536);
                    match writer.write(&remaining[..len]) {
                        Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                        Ok(n) => remaining = &remaining[n..],
                        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => remaining = &[],
                        Err(e)
                            if matches!(
                                e.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                            ) => {}
                        Err(e) => return Err(e),
                    }
                }
                if remaining.is_empty() {
                    stdin = None;
                }
            }
            if status.is_none() {
                status = child.try_wait()?;
            }
            if let Some(status) = status {
                stdin = None;
                if stdout.is_none() && stderr.is_none() {
                    return Ok(status);
                }
            }
            std::thread::sleep(
                Duration::from_millis(2).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    })();
    let status = result.map_err(|e| {
        let mut failure = terminate(&mut child, format!("{label} could not complete: {e}"));
        failure.stdout = std::mem::take(&mut out);
        failure.stderr = std::mem::take(&mut err);
        failure
    })?;

    Ok(ProcessOutput {
        status,
        stdout: out,
        stderr: err,
    })
}
