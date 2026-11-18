//! Owner-local process identity and point liveness checks.
//!
//! A process key names one process across PID reuse. It does not identify a
//! conversation or say anything about the terminal the process runs in.
use serde::{Deserialize, Serialize};
use std::io;
#[cfg(target_os = "linux")]
use std::io::Read;

/// One process on this host. `started` is the kernel start time in the
/// platform's own unit (clock ticks since boot on Linux, microseconds since the
/// epoch on macOS); keys are only compared on the host that observed them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessKey {
    pub boot: String,
    pub pid: u32,
    pub started: u64,
}

/// Implementations must report observation gaps as errors, never as absence.
pub trait ProcessObserver {
    /// Whether the exact process this key names is still running.
    fn alive(&self, key: &ProcessKey) -> io::Result<bool>;
}

pub struct OwnerProcesses {
    boot: String,
}

impl OwnerProcesses {
    pub fn new() -> io::Result<Self> {
        let boot = boot_id()?;
        let parsed = uuid::Uuid::parse_str(&boot).map_err(|_| unavailable())?;
        if parsed.to_string() != boot {
            return Err(unavailable());
        }
        Ok(Self { boot })
    }

    /// The key of the running process with this PID.
    pub fn observe(&self, pid: u32) -> io::Result<ProcessKey> {
        if pid == 0 || pid > i32::MAX as u32 {
            return Err(unavailable());
        }
        Ok(ProcessKey {
            boot: self.boot.clone(),
            pid,
            started: start_time(pid)?,
        })
    }
}

impl ProcessObserver for OwnerProcesses {
    fn alive(&self, key: &ProcessKey) -> io::Result<bool> {
        if key.boot != self.boot {
            return Ok(false);
        }
        match self.observe(key.pid) {
            Ok(observed) => Ok(observed == *key),
            Err(error) => {
                // An observation gap alone is not evidence of exit. ESRCH from
                // the kernel's point check is; EPERM and all other errors are not.
                if key.pid != 0
                    && key.pid <= i32::MAX as u32
                    && unsafe { libc::kill(key.pid as i32, 0) } == -1
                    && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    Ok(false)
                } else {
                    Err(error)
                }
            }
        }
    }
}

fn unavailable() -> io::Error {
    io::Error::other("owner process observation is unavailable")
}

#[cfg(target_os = "linux")]
fn boot_id() -> io::Result<String> {
    let mut value = String::new();
    std::fs::File::open("/proc/sys/kernel/random/boot_id")?
        .take(64)
        .read_to_string(&mut value)?;
    Ok(value.trim().to_owned())
}

#[cfg(target_os = "linux")]
fn start_time(pid: u32) -> io::Result<u64> {
    let mut stat = String::new();
    std::fs::File::open(format!("/proc/{pid}/stat"))?
        .take(64 * 1024)
        .read_to_string(&mut stat)?;
    parse_linux_stat(pid, &stat)
}

/// Field 22 of /proc/<pid>/stat, for a process that has not exited.
#[cfg(any(target_os = "linux", test))]
fn parse_linux_stat(pid: u32, stat: &str) -> io::Result<u64> {
    let (prefix, rest) = stat.rsplit_once(')').ok_or_else(unavailable)?;
    let (observed_pid, _) = prefix.split_once(" (").ok_or_else(unavailable)?;
    if observed_pid.parse::<u32>().ok() != Some(pid) {
        return Err(unavailable());
    }
    let fields: Vec<_> = rest.split_whitespace().collect();
    if fields.len() < 20 || matches!(fields[0], "Z" | "X" | "x") {
        return Err(unavailable());
    }
    fields[19].parse().map_err(|_| unavailable())
}

#[cfg(target_os = "macos")]
fn boot_id() -> io::Result<String> {
    use std::ffi::CStr;
    let mut bytes = [0_u8; 64];
    let mut len = bytes.len();
    let result = unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            bytes.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 || len == 0 || len > bytes.len() {
        return Err(unavailable());
    }
    let text = CStr::from_bytes_with_nul(&bytes[..len]).map_err(|_| unavailable())?;
    Ok(text
        .to_str()
        .map_err(|_| unavailable())?
        .to_ascii_lowercase())
}

#[cfg(target_os = "macos")]
fn start_time(pid: u32) -> io::Result<u64> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&info);
    let got = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size as i32,
        )
    };
    if got != size as i32
        || info.pbi_pid != pid
        || !matches!(
            info.pbi_status,
            libc::SIDL | libc::SRUN | libc::SSLEEP | libc::SSTOP
        )
    {
        return Err(unavailable());
    }
    info.pbi_start_tvsec
        .checked_mul(1_000_000)
        .and_then(|micros| micros.checked_add(info.pbi_start_tvusec))
        .ok_or_else(unavailable)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn boot_id() -> io::Result<String> {
    Err(unavailable())
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn start_time(_: u32) -> io::Result<u64> {
    Err(unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_stat_handles_parentheses_and_rejects_exited_processes() {
        let stat = |state: &str, started: &str| {
            format!("23 (a tricky ) name) {state} 7 12 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 {started} 0")
        };
        assert_eq!(parse_linux_stat(23, &stat("S", "100")).unwrap(), 100);
        assert_eq!(parse_linux_stat(23, &stat("T", "100")).unwrap(), 100);
        for value in [stat("Z", "100"), stat("S", "-1"), "23 (broken) S".into()] {
            assert!(parse_linux_stat(23, &value).is_err());
        }
        assert!(parse_linux_stat(24, &stat("S", "100")).is_err());
    }

    #[test]
    fn owner_reports_a_reaped_child_and_a_reused_pid_as_not_alive() {
        let observer = OwnerProcesses::new().unwrap();
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let key = observer.observe(child.id()).unwrap();
        assert!(observer.alive(&key).unwrap());
        let reused = ProcessKey {
            started: key.started + 1,
            ..key.clone()
        };
        assert!(!observer.alive(&reused).unwrap());
        let other_boot = ProcessKey {
            boot: "08f95cf6-6657-40c4-b5b9-6ee493d934ab".into(),
            ..key.clone()
        };
        assert!(!observer.alive(&other_boot).unwrap());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!observer.alive(&key).unwrap());
    }

    #[test]
    fn owner_observes_the_current_process() {
        let observer = OwnerProcesses::new().unwrap();
        let current = observer.observe(std::process::id()).unwrap();
        assert_eq!(current.pid, std::process::id());
        assert!(observer.alive(&current).unwrap());
        assert!(observer.observe(0).is_err());
        assert!(observer.observe(u32::MAX).is_err());
    }
}
