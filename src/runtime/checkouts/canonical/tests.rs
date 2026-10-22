use super::*;

#[test]
fn scanning_fallback_preserves_stored_names_and_physical_aliases() {
    let temp = tempfile::tempdir().unwrap();
    let parent = fs::canonicalize(temp.path()).unwrap();
    let root = parent.join("MiXeD");
    fs::create_dir(&root).unwrap();
    std::os::unix::fs::symlink(&root, parent.join("alias")).unwrap();
    assert_eq!(scanned_entry(&parent, OsStr::new("MiXeD")).unwrap(), root);
    if parent.join("mixed").is_dir() {
        assert_eq!(scanned_entry(&parent, OsStr::new("mixed")).unwrap(), root);
    }
    assert!(scanned_entry(&parent, OsStr::new("missing")).is_err());
    for reported in ["Different-volume-name", "alias"] {
        assert_eq!(
            named_entry(&parent, OsStr::new("MiXeD"), OsStr::new(reported)).unwrap(),
            root
        );
    }
    fs::create_dir(parent.join("Different-volume-name")).unwrap();
    assert_eq!(
        named_entry(
            &parent,
            OsStr::new("MiXeD"),
            OsStr::new("Different-volume-name")
        )
        .unwrap(),
        root
    );
}

#[cfg(target_os = "macos")]
mod attributes {
    use super::*;
    use std::mem::offset_of;

    fn attribute(bytes: &[u8]) -> NameAttribute {
        let mut attribute = NameAttribute {
            length: (offset_of!(NameAttribute, data) + bytes.len()) as u32,
            reference: libc::attrreference_t {
                attr_dataoffset: (offset_of!(NameAttribute, data)
                    - offset_of!(NameAttribute, reference)) as i32,
                attr_length: bytes.len() as u32,
            },
            data: [0; libc::PATH_MAX as usize],
        };
        attribute.data[..bytes.len()].copy_from_slice(bytes);
        attribute
    }

    #[test]
    fn native_lookup_returns_stored_spelling_without_fallback() {
        let expected = Some(OsStr::new("Library").to_os_string());
        assert_eq!(stored_name(Path::new("/System/Library")).unwrap(), expected);
        let alias = Path::new("/System/library");
        if alias.is_dir() {
            assert_eq!(stored_name(alias).unwrap(), expected);
        }
    }

    #[test]
    fn names_preserve_bytes_and_attribute_relative_padding() {
        assert_eq!(
            attribute("MiXeD-É\0".as_bytes()).decode().unwrap(),
            OsStr::new("MiXeD-É")
        );
        let mut padded = attribute(b"repo\0");
        padded.data.copy_within(..5, 4);
        padded.reference.attr_dataoffset += 4;
        padded.length += 4;
        assert_eq!(padded.decode().unwrap(), OsStr::new("repo"));
    }

    #[test]
    fn malformed_or_truncated_name_attributes_are_errors() {
        for offset in [-1, 0, 7, i32::MAX] {
            let mut name = attribute(b"repo\0");
            name.reference.attr_dataoffset = offset;
            assert!(name.decode().is_err(), "offset {offset}");
        }
        for length in [0, 4, u32::MAX] {
            let mut name = attribute(b"repo\0");
            name.reference.attr_length = length;
            assert!(name.decode().is_err(), "length {length}");
        }
        for length in [0, 12, 16, u32::MAX] {
            let mut name = attribute(b"repo\0");
            name.length = length;
            assert!(name.decode().is_err(), "buffer length {length}");
        }
        for bytes in [
            b"\0".as_slice(),
            b".\0",
            b"..\0",
            b"/repo\0",
            b"repo/\0",
            b"one/two\0",
            b"a\0b\0",
            b"repo",
        ] {
            assert!(attribute(bytes).decode().is_err(), "{bytes:?}");
        }
    }
}
