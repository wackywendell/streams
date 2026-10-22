//! Recover stored path spelling without enumerating unrelated siblings on macOS.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use super::{Result, invalid, physical};

#[cfg(test)]
mod tests;

pub(super) fn spelling(root: &Path) -> Result<PathBuf> {
    let mut canonical = PathBuf::from("/");
    for component in root.components().skip(1) {
        let name = component.as_os_str();
        #[cfg(target_os = "macos")]
        if let Some(stored) = stored_name(&canonical.join(name))? {
            canonical = named_entry(&canonical, name, &stored)?;
            continue;
        }
        canonical = scanned_entry(&canonical, name)?;
    }
    Ok(canonical)
}

#[cfg(any(target_os = "macos", test))]
fn named_entry(parent: &Path, requested: &OsStr, stored: &OsStr) -> Result<PathBuf> {
    let candidate = parent.join(stored);
    if stored == requested {
        return Ok(candidate);
    }
    // A mounted root may report its volume name, not its parent directory entry.
    // Do not substitute a different object or introduce a symlink alias.
    let target = fs::metadata(parent.join(requested))?;
    if fs::symlink_metadata(&candidate)
        .is_ok_and(|metadata| physical(&metadata) == physical(&target))
    {
        return Ok(candidate);
    }
    scanned_entry(parent, requested)
}

fn scanned_entry(parent: &Path, name: &OsStr) -> Result<PathBuf> {
    let entries = fs::read_dir(parent)?.collect::<std::io::Result<Vec<_>>>()?;
    if let Some(entry) = entries.iter().find(|entry| entry.file_name() == name) {
        return Ok(entry.path());
    }
    let target = fs::metadata(parent.join(name))?;
    entries
        .into_iter()
        .find(|entry| {
            fs::symlink_metadata(entry.path())
                .is_ok_and(|metadata| physical(&metadata) == physical(&target))
        })
        .map(|entry| entry.path())
        .ok_or_else(|| invalid("canonical directory entry not found"))
}

#[cfg(target_os = "macos")]
// getattrlist returns a length, an attribute-relative reference, then the UTF-8 name.
// Keep this ABI layout and bounds together; the syscall may truncate variable data.
#[repr(C)]
struct NameAttribute {
    length: u32,
    reference: libc::attrreference_t,
    data: [u8; libc::PATH_MAX as usize],
}

#[cfg(target_os = "macos")]
impl NameAttribute {
    fn decode(&self) -> std::io::Result<std::ffi::OsString> {
        use std::ffi::{CStr, OsString};
        use std::io::{Error, ErrorKind};
        use std::mem::{offset_of, size_of_val};
        use std::os::unix::ffi::OsStringExt;
        let invalid = || Error::new(ErrorKind::InvalidData, "invalid macOS filename attribute");
        let start = usize::try_from(self.reference.attr_dataoffset)
            .ok()
            .and_then(|offset| {
                offset.checked_sub(
                    offset_of!(NameAttribute, data) - offset_of!(NameAttribute, reference),
                )
            })
            .ok_or_else(invalid)?;
        let end = start
            .checked_add(self.reference.attr_length as usize)
            .ok_or_else(invalid)?;
        if self.length as usize > size_of_val(self)
            || offset_of!(NameAttribute, data)
                .checked_add(end)
                .is_none_or(|total| total > self.length as usize)
        {
            return Err(invalid());
        }
        let bytes = CStr::from_bytes_with_nul(self.data.get(start..end).ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        let stored = OsString::from_vec(bytes.to_bytes().to_vec());
        let mut components = Path::new(&stored).components();
        if !matches!(components.next(), Some(std::path::Component::Normal(component)) if component == stored.as_os_str())
            || components.next().is_some()
        {
            return Err(invalid());
        }
        Ok(stored)
    }
}

#[cfg(target_os = "macos")]
fn stored_name(path: &Path) -> std::io::Result<Option<std::ffi::OsString>> {
    use std::ffi::CString;
    use std::io::Error;
    use std::mem::size_of;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes())?;
    let mut attributes = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: libc::ATTR_CMN_NAME,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut name = NameAttribute {
        length: 0,
        reference: libc::attrreference_t {
            attr_dataoffset: 0,
            attr_length: 0,
        },
        data: [0; libc::PATH_MAX as usize],
    };
    // SAFETY: Both structures have their C ABI layout; path is NUL-terminated and
    // the writable buffer remains valid for its entire explicitly supplied size.
    if unsafe {
        libc::getattrlist(
            path.as_ptr(),
            (&mut attributes as *mut libc::attrlist).cast(),
            (&mut name as *mut NameAttribute).cast(),
            size_of::<NameAttribute>(),
            0,
        )
    } == -1
    {
        let error = Error::last_os_error();
        return if matches!(
            error.raw_os_error(),
            Some(libc::ENOTSUP | libc::EOPNOTSUPP | libc::ENOSYS | libc::EINVAL)
        ) {
            Ok(None)
        } else {
            Err(error)
        };
    }
    name.decode().map(Some)
}
