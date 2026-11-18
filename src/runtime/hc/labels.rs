//! Canonical owner-written label keys and JSON values.
use crate::{
    frontend::hc::processes::ProcessKey,
    integration::WorkScope,
    model::{ValidationError, check_text},
};
use sha2::{Digest, Sha256};

/// One label key per helper process, so each helper writes and removes only its own label.
pub fn source_key(helper: &ProcessKey) -> String {
    let bytes = format!(
        "[{},{},{}]",
        crate::codec::ascii_json(&helper.boot),
        helper.pid,
        helper.started
    );
    format!("streams.pi.{:x}", Sha256::digest(bytes.as_bytes()))
}

pub fn scope_key(scope: &WorkScope) -> Result<String, ValidationError> {
    scope.validate()?;
    Ok(scope.notification_key())
}

pub fn work_value(scope: &WorkScope, stream: &str) -> Result<String, ValidationError> {
    scope.validate()?;
    check_text(stream, "Stream", false)?;
    serde_json::to_string(&serde_json::json!({
        "version": 1,
        "scope": scope,
        "stream": stream,
    }))
    .map_err(|_| ValidationError::new("invalid work label"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_key_uses_the_canonical_order_and_decimal_pid() {
        let helper = ProcessKey {
            boot: "08f95cf6-6657-40c4-b5b9-6ee493d934ab".into(),
            pid: 42,
            started: 123,
        };
        let preimage = r#"["08f95cf6-6657-40c4-b5b9-6ee493d934ab",42,123]"#;
        assert_eq!(
            source_key(&helper),
            format!("streams.pi.{:x}", Sha256::digest(preimage.as_bytes()))
        );
    }

    #[test]
    fn label_keys_and_values_follow_canonical_preimages_and_json_shapes() {
        let scope = WorkScope {
            catalog: "café".into(),
            host: "host".into(),
        };
        assert_eq!(scope_key(&scope).unwrap(), scope.notification_key());
        assert_eq!(
            work_value(&scope, "work").unwrap(),
            r#"{"scope":{"catalog":"café","host":"host"},"stream":"work","version":1}"#
        );
    }
}
