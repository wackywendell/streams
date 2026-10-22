//! A fresh controlling terminal per child; never changes the test runner's terminal.
use std::{
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

pub struct Pty {
    master: File,
    slave: File,
    before: libc::termios,
    child: Child,
    parser: vt100::Parser,
    output: Vec<u8>,
}

fn attributes(file: &File) -> libc::termios {
    let mut value = std::mem::MaybeUninit::uninit();
    // SAFETY: tcgetattr initializes the termios on success; file owns a live PTY fd.
    assert_eq!(
        unsafe { libc::tcgetattr(file.as_raw_fd(), value.as_mut_ptr()) },
        0,
        "tcgetattr: {}",
        io::Error::last_os_error()
    );
    unsafe { value.assume_init() }
}

impl Pty {
    pub fn spawn(mut command: Command, raw: bool) -> Self {
        let (mut master, mut slave) = (-1, -1);
        let mut size = libc::winsize {
            ws_row: 20,
            ws_col: 100,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: openpty initializes both descriptors on success; optional pointers are null.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut size,
                )
            },
            0
        );
        // SAFETY: each fresh descriptor is transferred to exactly one File.
        let (master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
        for file in [&master, &slave] {
            // openpty does not promise close-on-exec. Only dup'd stdin/out/err may reach the child.
            assert_ne!(
                unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) },
                -1
            );
        }
        let mut before = attributes(&slave);
        if raw {
            // SAFETY: before is initialized; only this disposable slave is modified.
            unsafe { libc::cfmakeraw(&mut before) };
            assert_eq!(
                unsafe { libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &before) },
                0
            );
        }
        command
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave.try_clone().unwrap()));
        // SAFETY: pre_exec calls only async-signal-safe syscalls, on the child's dup'd stdin.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1
                    || libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY as _, 0) == -1
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
        assert_ne!(flags, -1);
        assert_ne!(
            unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
            -1
        );
        Self {
            master,
            slave,
            before,
            child,
            parser: vt100::Parser::new(20, 100, 0),
            output: Vec::new(),
        }
    }

    fn pump(&mut self, render: bool) {
        let mut bytes = [0; 8192];
        loop {
            match self.master.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => {
                    if render {
                        self.parser.process(&bytes[..n]);
                    }
                    self.output.extend_from_slice(&bytes[..n]);
                }
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.raw_os_error() == Some(libc::EIO) =>
                {
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => panic!("PTY read: {e}"),
            }
        }
    }

    pub fn until(&mut self, mut ready: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump(true);
            if ready(self) {
                return;
            }
            assert!(
                self.child.try_wait().unwrap().is_none() && Instant::now() < deadline,
                "PTY condition unmet; screen:\n{}\noutput: {:?}",
                self.screen(),
                String::from_utf8_lossy(&self.output)
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn screen(&self) -> String {
        self.parser.screen().contents()
    }
    pub fn send(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).unwrap();
    }
    pub fn signal(&self, signal: libc::c_int) {
        assert_eq!(
            unsafe { libc::kill(self.child.id() as libc::pid_t, signal) },
            0
        );
    }
    pub fn resize(&mut self, rows: u16, columns: u16) {
        self.pump(true);
        self.parser.set_size(rows, columns);
        let size = libc::winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // The kernel sends SIGWINCH to this terminal's foreground process group.
        assert_eq!(
            unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size) },
            0
        );
    }
    pub fn restored(&self) -> bool {
        let after = attributes(&self.slave);
        after.c_iflag == self.before.c_iflag
            && after.c_oflag == self.before.c_oflag
            && after.c_cflag == self.before.c_cflag
            && after.c_lflag == self.before.c_lflag
            && after.c_cc == self.before.c_cc
            && unsafe {
                libc::cfgetispeed(&after) == libc::cfgetispeed(&self.before)
                    && libc::cfgetospeed(&after) == libc::cfgetospeed(&self.before)
            }
    }
    pub fn assert_cleanup(&self) {
        assert!(self.restored(), "slave termios was not restored");
        for sequence in [
            b"\x1b[?1049h".as_slice(),
            b"\x1b[?1049l",
            b"\x1b[?25h",
            b"\x1b[?7h",
            b"\x1b[?2004l",
            b"\x1b[?1000l",
            b"\x1b[?1002l",
            b"\x1b[?1003l",
            b"\x1b[?1006l",
            b"\x1b[?1015l",
            b"\x1b[?7;25;1000;1002;1003;1006;1015;1049;2004r",
        ] {
            assert!(
                self.output
                    .windows(sequence.len())
                    .any(|window| window == sequence),
                "missing cleanup/setup sequence: {sequence:?}"
            );
        }
    }
    pub fn finish(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            // Card run() has returned. Drain libtest's output, not another card frame.
            self.pump(false);
            if let Some(status) = self.child.try_wait().unwrap() {
                self.pump(false);
                assert!(
                    status.success(),
                    "child failed: {status}; output: {:?}",
                    String::from_utf8_lossy(&self.output)
                );
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "PTY child did not exit; screen: {}",
                self.screen()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Pty {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            // The child owns this session/process group. Kill it on assertion failure too.
            unsafe {
                libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL);
            }
            let _ = self.child.wait();
        }
    }
}
