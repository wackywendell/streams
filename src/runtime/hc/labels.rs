//! Canonical owner-written label keys.
use crate::frontend::hc::processes::ProcessKey;
use sha2::{Digest, Sha256};
use std::io;

pub fn source_key(helper: &ProcessKey) -> io::Result<String> {
    helper.validate()?;
    let bytes = format!(
        "[{},{},{}]",
        crate::codec::ascii_json(&helper.boot),
        helper.pid,
        crate::codec::ascii_json(&helper.started)
    );
    Ok(format!("streams.pi.{:x}", Sha256::digest(bytes.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_key_uses_the_canonical_order_and_decimal_pid() {
        let helper = ProcessKey {
            boot: "08f95cf6-6657-40c4-b5b9-6ee493d934ab".into(),
            pid: 42,
            started: "linux:123".into(),
        };
        assert_eq!(
            source_key(&helper).unwrap(),
            "streams.pi.1b0fc3ebc218ef8530f852ee3c22d1b482933b84a9922619c0f8d71619716db9"
        );
        assert!(source_key(&ProcessKey { pid: 0, ..helper }).is_err());
    }
}
