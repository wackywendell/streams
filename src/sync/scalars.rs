//! Validated scalar syntax, independent of history and publication evidence.

use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Lowercase SHA-1 or SHA-256 syntax; not proof of object existence.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct Oid(String);

impl TryFrom<String> for Oid {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if !matches!(value.len(), 40 | 64)
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("expected a Git object ID".to_string());
        }
        Ok(Self(value))
    }
}

impl FromStr for Oid {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value.to_owned())
    }
}

impl Oid {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Oid {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<Oid> for String {
    fn from(value: Oid) -> Self {
        value.0
    }
}

/// Finite seconds, including negative timestamps and negative zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f64")]
pub struct Seconds(f64);

impl Seconds {
    pub const ZERO: Self = Self(0.0);

    pub fn get(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for Seconds {
    type Error = String;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        if !value.is_finite() {
            return Err("expected a finite timestamp".to_string());
        }
        Ok(Self(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn oid_constructor_and_wire_syntax() {
        for width in [40, 64] {
            let raw = "0123456789abcdef".repeat(4)[..width].to_owned();
            let oid = Oid::try_from(raw.clone()).unwrap();
            assert_eq!(raw.parse::<Oid>().unwrap(), oid);
            assert_eq!(serde_json::from_value::<Oid>(json!(raw)).unwrap(), oid);
            assert_eq!(serde_json::to_value(oid).unwrap(), json!(raw));
        }
        for raw in ["", "abc", &"A".repeat(40), &"g".repeat(64), &"a".repeat(41)] {
            assert_eq!(raw.parse::<Oid>().unwrap_err(), "expected a Git object ID");
            assert!(serde_json::from_value::<Oid>(json!(raw)).is_err());
        }
        for value in [
            json!(null),
            json!(true),
            json!(1),
            json!(1.5),
            json!([]),
            json!({}),
        ] {
            assert!(serde_json::from_value::<Oid>(value).is_err());
        }
    }

    #[test]
    fn seconds_constructor_and_wire_syntax() {
        for raw in [0.0, -0.0, -123.0, f64::MAX, f64::MIN] {
            let seconds = Seconds::try_from(raw).unwrap();
            assert_eq!(seconds.get().to_bits(), raw.to_bits());
            assert_eq!(
                serde_json::from_value::<Seconds>(json!(raw)).unwrap(),
                seconds
            );
            assert_eq!(serde_json::to_value(seconds).unwrap(), json!(raw));
        }
        assert_eq!(Seconds::default(), Seconds::ZERO);
        for raw in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                Seconds::try_from(raw).unwrap_err(),
                "expected a finite timestamp"
            );
        }
        for value in [json!(null), json!(true), json!("1"), json!([]), json!({})] {
            assert!(serde_json::from_value::<Seconds>(value).is_err());
        }
        assert!(serde_json::from_str::<Seconds>("1e999").is_err());
    }
}
