//! Bounded Unix framing shared by native protocols; each caller owns its connection.
use crate::cancellation::Cancellation;
use std::{
    io::{self, BufRead, BufReader},
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};

pub(super) fn remaining(deadline: Instant, label: &str) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|value| !value.is_zero())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{label} request timed out"),
            )
        })
}

pub(super) fn read_frame_into(
    reader: &mut BufReader<UnixStream>,
    deadline: Option<Instant>,
    frame: &mut Vec<u8>,
    cancellation: Option<&Cancellation>,
    label: &str,
    max_frame: usize,
) -> io::Result<()> {
    loop {
        if let Some(cancellation) = cancellation {
            cancellation.check()?;
        }
        if reader.buffer().is_empty() && (deadline.is_some() || cancellation.is_some()) {
            use std::os::fd::AsRawFd;
            wait(
                reader.get_ref().as_raw_fd(),
                libc::POLLIN,
                deadline,
                cancellation,
                label,
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
                format!("{label} socket closed before a complete frame"),
            ));
        }
        let length = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(buffer.len());
        if frame.len() + length > max_frame {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{label} frame exceeds {} MiB", max_frame / (1024 * 1024)),
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

pub(super) fn connect(
    path: &Path,
    deadline: Instant,
    cancellation: Option<&Cancellation>,
    label: &str,
) -> io::Result<UnixStream> {
    if let Some(cancellation) = cancellation {
        cancellation.check()?;
    }
    use std::os::{
        fd::{AsRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    };
    let bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.contains(&0) || bytes.len() >= address.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid {label} socket path"),
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
    use std::os::fd::FromRawFd;
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
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
        wait(
            stream.as_raw_fd(),
            libc::POLLOUT,
            Some(deadline),
            cancellation,
            label,
        )?;
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
    deadline: Option<Instant>,
    cancellation: Option<&Cancellation>,
    label: &str,
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
        let millis = match deadline {
            Some(deadline) => remaining(deadline, label)?
                .as_millis()
                .saturating_add(1)
                .min(i32::MAX as u128) as i32,
            None => -1,
        };
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
                    format!("{label} request timed out"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, thread, time::Duration};
    #[test]
    fn newline_counts_towards_the_bound_and_coalesced_frames_are_retained() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"1234567\n12345678\n").unwrap();
        let mut reader = BufReader::new(reader);
        let mut frame = Vec::new();
        let deadline = Some(Instant::now() + Duration::from_secs(1));
        read_frame_into(&mut reader, deadline, &mut frame, None, "hc", 8).unwrap();
        assert_eq!(frame, b"1234567\n");
        frame.clear();
        assert_eq!(
            read_frame_into(&mut reader, deadline, &mut frame, None, "hc", 8)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
    #[test]
    fn partial_eof_retains_evidence() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"partial").unwrap();
        drop(writer);
        let mut frame = Vec::new();
        assert_eq!(
            read_frame_into(&mut BufReader::new(reader), None, &mut frame, None, "hc", 8)
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(frame, b"partial");
    }
    #[test]
    fn cancellation_wakes_an_idle_reader_without_a_deadline() {
        use std::sync::mpsc;
        let (reader, peer) = UnixStream::pair().unwrap();
        let cancellation = Cancellation::new().unwrap();
        let cancel = cancellation.clone();
        let (result, finished) = mpsc::channel();
        let worker = thread::spawn(move || {
            let error = read_frame_into(
                &mut BufReader::new(reader),
                None,
                &mut Vec::new(),
                Some(&cancellation),
                "hc",
                8,
            )
            .unwrap_err();
            result.send(error.kind()).unwrap();
        });
        cancel.cancel().unwrap();
        let outcome = finished.recv_timeout(Duration::from_secs(1));
        drop(peer);
        worker.join().unwrap();
        assert_eq!(outcome.unwrap(), io::ErrorKind::Interrupted);
    }
}
