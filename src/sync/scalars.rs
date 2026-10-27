//! Validated scalar syntax, independent of history and publication evidence.

use serde::{Deserialize, Serialize};

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
