//! Canonical host-partitioned record storage and its strict YAML codec.

pub mod records;

use crate::model::ValidationError;
use saneyaml as serde_yaml;
use serde_json::Value;
use std::collections::BTreeSet;

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

fn yaml_to_json(yaml: serde_yaml::Value) -> Result<Value, ValidationError> {
    match yaml {
        serde_yaml::Value::Null => Ok(Value::Null),
        serde_yaml::Value::Bool(b) => Ok(Value::Bool(b)),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Number(i.into()))
            } else if let Some(u) = n.as_u64() {
                Ok(Value::Number(u.into()))
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .ok_or_else(|| ValidationError::new("non-finite YAML number"))
            } else {
                Err(ValidationError::new("invalid YAML number"))
            }
        }
        serde_yaml::Value::String(s) => Ok(Value::String(s)),
        serde_yaml::Value::Sequence(seq) => {
            let mut out = Vec::with_capacity(seq.len());
            for item in seq {
                out.push(yaml_to_json(item)?);
            }
            Ok(Value::Array(out))
        }
        serde_yaml::Value::Mapping(map) => {
            let mut obj = serde_json::Map::new();
            let mut seen = BTreeSet::new();
            for (k, v) in map {
                let Value::String(key_str) = yaml_to_json(k)? else {
                    return Err(ValidationError::new(
                        "YAML object keys must be unique strings",
                    ));
                };
                if !seen.insert(key_str.clone()) {
                    return Err(ValidationError::new(
                        "YAML object keys must be unique strings",
                    ));
                }
                obj.insert(key_str, yaml_to_json(v)?);
            }
            Ok(Value::Object(obj))
        }
        serde_yaml::Value::Tagged(tagged) => {
            let core = if tagged.tag.handle == "!!" {
                Some(tagged.tag.suffix.as_str())
            } else if tagged.tag.handle == "!" {
                tagged
                    .tag
                    .suffix
                    .strip_prefix("tag:yaml.org,2002:")
                    .or_else(|| tagged.tag.suffix.is_empty().then_some("str"))
            } else {
                None
            };
            match core {
                Some("str" | "bool" | "int" | "float" | "null" | "seq" | "map") => {
                    yaml_to_json(tagged.value)
                }
                _ => Err(ValidationError::new("unsupported YAML tag")),
            }
        }
    }
}
