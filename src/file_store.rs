//! Canonical host-partitioned record storage. Each record file is the record's JSON.

pub mod records;

/// Encode UTF-8 bytes for a single filename component, including traversal-only dot names.
pub fn encode_component(id: &str) -> String {
    const SAFE: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789-_.~";
    let mut encoded = String::new();
    for &byte in id.as_bytes() {
        if SAFE.contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    if encoded == "." || encoded == ".." {
        encoded.replace('.', "%2E")
    } else {
        encoded
    }
}
