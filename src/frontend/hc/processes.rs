//! Owner-local process identity and fresh process-group observations.
//!
//! A process observation validates a PID and its kernel start time. It does not
//! identify a conversation or prove that a terminal contains no other processes.
use serde::{Deserialize, Serialize};
use std::io;
#[cfg(target_os = "linux")]
use std::io::Read;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessKey {
    pub boot: String,
    pub pid: u32,
    pub started: String,
}

impl ProcessKey {
    pub fn validate(&self) -> io::Result<()> {
        let boot = uuid::Uuid::parse_str(&self.boot).map_err(|_| unavailable())?;
        if boot.to_string() != self.boot || self.pid == 0 {
            return Err(unavailable());
        }
        let valid_decimal = |value: &str| {
            !value.is_empty()
                && value.bytes().all(|byte| byte.is_ascii_digit())
                && (value == "0" || !value.starts_with('0'))
                && value.parse::<u64>().is_ok()
        };
        let valid_start = if let Some(ticks) = self.started.strip_prefix("linux:") {
            valid_decimal(ticks)
        } else if let Some(start) = self.started.strip_prefix("macos:") {
            let mut parts = start.split(':');
            matches!((parts.next(), parts.next(), parts.next()), (Some(seconds), Some(micros), None)
                if valid_decimal(seconds) && valid_decimal(micros) && micros.parse::<u64>().is_ok_and(|value| value < 1_000_000))
        } else {
            false
        };
        if !valid_start {
            return Err(unavailable());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessObservation {
    pub key: ProcessKey,
    pub parent_pid: u32,
    pub process_group: u32,
    pub stopped: bool,
}

/// Implementations must report observation gaps as errors, never as absence.
pub trait ProcessObserver {
    fn observe(&self, pid: u32) -> io::Result<ProcessObservation>;

    /// A different observed process proves that this particular process has exited.
    fn exited(&self, key: &ProcessKey) -> io::Result<bool> {
        key.validate()?;
        let observed = self.observe(key.pid)?;
        observed.key.validate()?;
        if observed.key.pid != key.pid {
            return Err(unavailable());
        }
        Ok(observed.key != *key)
    }
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
}

impl ProcessObserver for OwnerProcesses {
    fn observe(&self, pid: u32) -> io::Result<ProcessObservation> {
        if pid == 0 || pid > i32::MAX as u32 {
            return Err(unavailable());
        }
        observe_process(&self.boot, pid)
    }

    fn exited(&self, key: &ProcessKey) -> io::Result<bool> {
        key.validate()?;
        if key.boot != self.boot {
            return Ok(true);
        }
        match self.observe(key.pid) {
            Ok(observed) => Ok(observed.key != *key),
            Err(error) => {
                // An observation gap alone is not evidence of exit. ESRCH from
                // the kernel's point check is; EPERM and all other errors are not.
                if key.pid <= i32::MAX as u32
                    && unsafe { libc::kill(key.pid as i32, 0) } == -1
                    && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    Ok(true)
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
fn observe_process(boot: &str, pid: u32) -> io::Result<ProcessObservation> {
    let mut stat = String::new();
    std::fs::File::open(format!("/proc/{pid}/stat"))?
        .take(64 * 1024)
        .read_to_string(&mut stat)?;
    parse_linux_stat(boot, pid, &stat)
}

#[cfg(any(target_os = "linux", test))]
fn parse_linux_stat(boot: &str, pid: u32, stat: &str) -> io::Result<ProcessObservation> {
    let (prefix, rest) = stat.rsplit_once(')').ok_or_else(unavailable)?;
    let (observed_pid, _) = prefix.split_once(" (").ok_or_else(unavailable)?;
    if observed_pid.parse::<u32>().ok() != Some(pid) {
        return Err(unavailable());
    }
    let fields: Vec<_> = rest.split_whitespace().collect();
    if fields.len() < 20 {
        return Err(unavailable());
    }
    let stopped = match fields[0] {
        "T" | "t" => true,
        "R" | "S" | "D" | "I" | "W" | "P" => false,
        _ => return Err(unavailable()),
    };
    let parent_pid = fields[1].parse().map_err(|_| unavailable())?;
    let process_group = fields[2].parse().map_err(|_| unavailable())?;
    if process_group == 0 {
        return Err(unavailable());
    }
    let key = ProcessKey {
        boot: boot.to_owned(),
        pid,
        started: format!("linux:{}", fields[19]),
    };
    key.validate()?;
    Ok(ProcessObservation {
        key,
        parent_pid,
        process_group,
        stopped,
    })
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
fn observe_process(boot: &str, pid: u32) -> io::Result<ProcessObservation> {
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
        || info.pbi_pgid == 0
        || !matches!(
            info.pbi_status,
            libc::SIDL | libc::SRUN | libc::SSLEEP | libc::SSTOP
        )
    {
        return Err(unavailable());
    }
    let key = ProcessKey {
        boot: boot.to_owned(),
        pid,
        started: format!("macos:{}:{}", info.pbi_start_tvsec, info.pbi_start_tvusec),
    };
    key.validate()?;
    Ok(ProcessObservation {
        key,
        parent_pid: info.pbi_ppid,
        process_group: info.pbi_pgid,
        stopped: info.pbi_status == libc::SSTOP,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn boot_id() -> io::Result<String> {
    Err(unavailable())
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn observe_process(_: &str, _: u32) -> io::Result<ProcessObservation> {
    Err(unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;
    const BOOT: &str = "08f95cf6-6657-40c4-b5b9-6ee493d934ab";

    #[test]
    fn process_keys_have_canonical_boot_and_kernel_start_values() {
        let key = ProcessKey {
            boot: BOOT.into(),
            pid: 9,
            started: "linux:42".into(),
        };
        assert!(key.validate().is_ok());
        for started in [
            "linux:042",
            "linux:-1",
            "linux:",
            "macos:1:1000000",
            "macos:01:2",
            "macos:1:2:3",
            "unknown:1",
        ] {
            assert!(
                ProcessKey {
                    started: started.into(),
                    ..key.clone()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            ProcessKey {
                started: "macos:1:2".into(),
                ..key.clone()
            }
            .validate()
            .is_ok()
        );
        assert!(
            ProcessKey {
                boot: BOOT.to_uppercase(),
                ..key.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            ProcessKey {
                pid: 0,
                ..key.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_str::<ProcessKey>(&format!(
                "{{\"boot\":\"{BOOT}\",\"pid\":9,\"started\":\"linux:42\",\"extra\":true}}"
            ))
            .is_err()
        );
    }

    #[test]
    fn linux_stat_handles_parentheses_and_distinguishes_stopped_and_reused_processes() {
        let stat = |state: &str, started: &str| {
            format!("23 (a tricky ) name) {state} 7 12 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 {started} 0")
        };
        let first = parse_linux_stat(BOOT, 23, &stat("S", "100")).unwrap();
        assert_eq!(
            (first.parent_pid, first.process_group, first.stopped),
            (7, 12, false)
        );
        let stopped = parse_linux_stat(BOOT, 23, &stat("T", "100")).unwrap();
        assert!(stopped.stopped);
        assert_ne!(
            first.key,
            parse_linux_stat(BOOT, 23, &stat("S", "101")).unwrap().key
        );
        for value in [stat("Z", "100"), stat("S", "0100"), "23 (broken) S".into()] {
            assert!(parse_linux_stat(BOOT, 23, &value).is_err());
        }
        assert!(parse_linux_stat(BOOT, 24, &stat("S", "100")).is_err());
    }

    #[test]
    fn exit_evidence_distinguishes_reuse_and_observation_gaps() {
        struct Fake(Option<ProcessObservation>);
        impl ProcessObserver for Fake {
            fn observe(&self, _: u32) -> io::Result<ProcessObservation> {
                self.0
                    .clone()
                    .ok_or_else(|| io::Error::other("observation unavailable"))
            }
        }
        let key = ProcessKey {
            boot: BOOT.into(),
            pid: 42,
            started: "linux:10".into(),
        };
        let observed = ProcessObservation {
            key: key.clone(),
            parent_pid: 1,
            process_group: 42,
            stopped: false,
        };
        assert!(!Fake(Some(observed.clone())).exited(&key).unwrap());
        let reused = ProcessObservation {
            key: ProcessKey {
                started: "linux:11".into(),
                ..key.clone()
            },
            ..observed
        };
        assert!(Fake(Some(reused)).exited(&key).unwrap());
        assert!(Fake(None).exited(&key).is_err());
        assert!(Fake(None).exited(&ProcessKey { pid: 0, ..key }).is_err());
    }

    #[test]
    fn owner_reports_a_reaped_child_as_exited() {
        let observer = OwnerProcesses::new().unwrap();
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let key = observer.observe(child.id()).unwrap().key;
        assert!(!observer.exited(&key).unwrap());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(observer.exited(&key).unwrap());
    }

    #[test]
    fn owner_observes_the_current_process_without_inventing_identity() {
        let observer = OwnerProcesses::new().unwrap();
        let current = observer.observe(std::process::id()).unwrap();
        current.key.validate().unwrap();
        assert_eq!(current.key.pid, std::process::id());
        assert!(!current.stopped);
        assert!(observer.observe(0).is_err());
        assert!(observer.observe(u32::MAX).is_err());
    }
}
