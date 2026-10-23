//! A one-shot cancellation signal that wakes blocking socket I/O without a timer.
use std::{
    io::{self, Write},
    os::{
        fd::{AsRawFd, RawFd},
        unix::net::UnixStream,
    },
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone)]
pub struct Cancellation(Arc<Signal>);
struct Signal {
    flag: Arc<AtomicBool>,
    reader: UnixStream,
    writer: UnixStream,
}
impl Cancellation {
    pub fn new() -> io::Result<Self> {
        let (reader, writer) = UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        writer.set_nonblocking(true)?;
        Ok(Self(Arc::new(Signal {
            flag: Arc::new(AtomicBool::new(false)),
            reader,
            writer,
        })))
    }
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.0.flag.clone()
    }
    pub fn cancel(&self) -> io::Result<()> {
        if !self.0.flag.swap(true, Ordering::SeqCst) {
            (&self.0.writer).write_all(&[1])?;
        }
        Ok(())
    }
    pub(crate) fn descriptor(&self) -> RawFd {
        self.0.reader.as_raw_fd()
    }
    pub(crate) fn check(&self) -> io::Result<()> {
        if self.0.flag.load(Ordering::SeqCst) {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "operation cancelled",
            ))
        } else {
            Ok(())
        }
    }
}
