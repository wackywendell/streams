//! Strict physical JSON field mapping for records and wire payloads.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;
use serde::de::{self, DeserializeOwned, DeserializeSeed, MapAccess, SeqAccess, Visitor};

use serde_json::{Map, Value};

use crate::model::{Host, Record, RecordKind, Session, Stream, ValidationError};

/// What a user with a catalog from an older format does. The README documents it.
pub(crate) const RESET_HINT: &str =
    "move the catalog directory aside and re-enroll (README: \"Resetting the catalog\")";

// Unit enums must use strings, not Serde's alternative single-key map representation.
macro_rules! deserialize_string_enum {
    ($($ty:ty),+ $(,)?) => {
        $(impl<'de> serde::Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = <String as serde::Deserialize>::deserialize(deserializer)?;
                value.parse::<Self>().map_err(|_| {
                    serde::de::Error::unknown_variant(
                        &value,
                        <Self as strum::VariantNames>::VARIANTS,
                    )
                })
            }
        })+
    };
}
pub(crate) use deserialize_string_enum;

/// Encode the exact Python json.dumps string spelling used in persisted identity hashes.
pub fn ascii_json(value: &str) -> String {
    let encoded = serde_json::to_string(value).expect("string serialization");
    let mut result = String::new();
    for ch in encoded.chars() {
        if ch >= '\u{7f}' {
            for unit in ch.encode_utf16(&mut [0; 2]) {
                result.push_str(&format!("\\u{unit:04x}"));
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Require a field to be present while accepting an explicit JSON null.
pub(crate) fn required_nullable<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// Deserialize named fields without accepting Serde's positional struct arrays.
pub(crate) fn object<'de, T: DeserializeOwned, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<T, D::Error> {
    // A plain Value would overwrite duplicate keys before the struct can reject them.
    let value = StrictJsonSeed.deserialize(deserializer)?;
    if !value.is_object() {
        return Err(de::Error::custom("value must be an object"));
    }
    serde_json::from_value(value).map_err(de::Error::custom)
}

struct Object<T>(T);

impl<'de, T: DeserializeOwned> Deserialize<'de> for Object<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        object(deserializer).map(Self)
    }
}

pub(crate) fn object_map<'de, T: DeserializeOwned, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, T>, D::Error> {
    BTreeMap::<String, Object<T>>::deserialize(deserializer)
        .map(|items| items.into_iter().map(|(key, item)| (key, item.0)).collect())
}

pub(crate) fn optional_object<'de, T: DeserializeOwned, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<Object<T>>::deserialize(deserializer).map(|item| item.map(|item| item.0))
}

/// Emit untagged persisted fields, not the derived serde representation of `Record`.
pub fn record_to_data(record: &Record) -> Value {
    match record {
        Record::Stream(s) => serde_json::to_value(s),
        Record::Session(s) => serde_json::to_value(s),
        Record::Host(h) => serde_json::to_value(h),
    }
    .expect("record fields are JSON-compatible")
}

/// Require named wire fields rather than Serde's positional struct representation.
pub(crate) fn require_object<'a>(
    value: &'a Value,
    name: &str,
) -> Result<&'a Map<String, Value>, ValidationError> {
    value
        .as_object()
        .ok_or_else(|| ValidationError::new(format!("{name} must be an object")))
}

/// Decode a record's wire fields and check its logical constraints before use.
pub fn record_from_data(kind: RecordKind, value: &Value) -> Result<Record, ValidationError> {
    match kind {
        RecordKind::Stream => stream_from_data(value).map(Record::Stream),
        RecordKind::Session => session_from_data(value).map(Record::Session),
        RecordKind::Host => {
            let host = decode::<Host>(value)?;
            host.validate()?;
            Ok(Record::Host(host))
        }
    }
}

pub fn stream_from_data(value: &Value) -> Result<Stream, ValidationError> {
    decode::<Stream>(value)?.validated()
}

pub fn session_from_data(value: &Value) -> Result<Session, ValidationError> {
    decode::<Session>(value)?.validated()
}

fn decode<T: DeserializeOwned>(value: &Value) -> Result<T, ValidationError> {
    object::<T, _>(value.clone()).map_err(|error| ValidationError::new(error.to_string()))
}

struct StrictJsonSeed;

impl<'de> DeserializeSeed<'de> for StrictJsonSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct StrictVisitor;

        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = Value;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("any valid JSON value without duplicate object keys")
            }

            fn visit_bool<E>(self, value: bool) -> Result<Value, E> {
                Ok(Value::Bool(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Value, E> {
                Ok(Value::Number(value.into()))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Value, E> {
                Ok(Value::Number(value.into()))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Value, E>
            where
                E: de::Error,
            {
                serde_json::Number::from_f64(value)
                    .map(Value::Number)
                    .ok_or_else(|| de::Error::custom("non-finite float in JSON"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Value, E> {
                Ok(Value::String(value.to_owned()))
            }

            fn visit_string<E>(self, value: String) -> Result<Value, E> {
                Ok(Value::String(value))
            }

            fn visit_unit<E>(self) -> Result<Value, E> {
                Ok(Value::Null)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut vec = Vec::new();
                while let Some(elem) = seq.next_element_seed(StrictJsonSeed)? {
                    vec.push(elem);
                }
                Ok(Value::Array(vec))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom(format!("duplicate JSON field: {key}")));
                    }
                    let val = map.next_value_seed(StrictJsonSeed)?;
                    values.insert(key, val);
                }
                Ok(Value::Object(values))
            }
        }

        deserializer.deserialize_any(StrictVisitor)
    }
}

/// Parse JSON without silently accepting duplicate keys, including in nested objects.
pub fn load_json(value: &str) -> Result<Value, ValidationError> {
    let mut de = serde_json::Deserializer::from_str(value);
    let parsed = StrictJsonSeed
        .deserialize(&mut de)
        .map_err(|e| ValidationError::new(format!("invalid JSON: {e}")))?;
    de.end()
        .map_err(|e| ValidationError::new(format!("invalid JSON: {e}")))?;
    Ok(parsed)
}
