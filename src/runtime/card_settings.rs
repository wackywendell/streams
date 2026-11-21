//! One host-local preference per catalog conversation, independent of membership.
use super::*;
use crate::{
    config::{CardMode, Config},
    model::Session,
};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationId {
    pub harness: String,
    pub native_id: String,
}

impl ConversationId {
    /// Parse an exact `HARNESS/ID` target with no missing or extra segments.
    pub fn parse_pair(value: &str) -> Result<Self> {
        let Some((harness, native_id)) = value.split_once('/') else {
            return Err(invalid(format!("expected HARNESS/ID, got {value:?}")));
        };
        if native_id.contains('/') {
            return Err(invalid(format!("expected HARNESS/ID, got {value:?}")));
        }
        text(harness)?;
        text(native_id)?;
        Ok(Self {
            harness: harness.into(),
            native_id: native_id.into(),
        })
    }
}

impl From<&Session> for ConversationId {
    fn from(value: &Session) -> Self {
        Self {
            harness: value.harness.clone(),
            native_id: value.harness_session_id.clone(),
        }
    }
}

#[derive(Default, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Preference {
    auto_open: CardMode,
}

pub struct CardSettings {
    path: PathBuf,
}
impl CardSettings {
    pub fn new(
        config: &Config,
        catalog: &Path,
        host: &str,
        conversation: &ConversationId,
    ) -> Result<Self> {
        text(host)?;
        text(&conversation.harness)?;
        text(&conversation.native_id)?;
        Ok(Self {
            path: config
                .runtime_state_dir_for(catalog)
                .map_err(|error| invalid(error.to_string()))?
                .join(host)
                .join("card-settings")
                .join(format!(
                    "{}.json",
                    Session::id_for(host, &conversation.harness, &conversation.native_id)
                )),
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn mode(&self) -> Result<CardMode> {
        Ok(read_json::<Preference>(&self.path)?
            .unwrap_or_default()
            .auto_open)
    }
    pub fn set(&self, auto_open: CardMode) -> Result<()> {
        let _guard = lock(&self.path.with_extension("lock"))?;
        write_json(&self.path, &Preference { auto_open })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn preferences_survive_reopening_and_are_scoped_to_the_catalog_and_conversation() {
        let home = tempfile::tempdir().unwrap();
        let config = Config::new(
            &BTreeMap::from([("HOME".into(), home.path().to_str().unwrap().into())]),
            home.path(),
        )
        .unwrap();
        let store = |catalog: &str, native: &str| {
            CardSettings::new(
                &config,
                &home.path().join(catalog),
                "host",
                &ConversationId {
                    harness: "pi".into(),
                    native_id: native.into(),
                },
            )
            .unwrap()
        };
        let first = store("catalog", "one");
        assert_eq!(first.mode().unwrap(), CardMode::Automatic);
        assert!(!first.path().exists());
        first.set(CardMode::Manual).unwrap();
        assert_eq!(store("catalog", "one").mode().unwrap(), CardMode::Manual);
        assert_eq!(store("catalog", "two").mode().unwrap(), CardMode::Automatic);
        assert_eq!(store("other", "one").mode().unwrap(), CardMode::Automatic);
        let codex = CardSettings::new(
            &config,
            &home.path().join("catalog"),
            "host",
            &ConversationId {
                harness: "codex".into(),
                native_id: "one".into(),
            },
        )
        .unwrap();
        assert_ne!(first.path(), codex.path());
        assert_eq!(codex.mode().unwrap(), CardMode::Automatic);
        first.set(CardMode::Automatic).unwrap();
        assert_eq!(first.mode().unwrap(), CardMode::Automatic);
        assert_eq!(
            first.mode().unwrap().resolve(CardMode::Manual),
            CardMode::Manual
        );
        assert_eq!(
            first.mode().unwrap().resolve(CardMode::Automatic),
            CardMode::Automatic
        );
        first.set(CardMode::Manual).unwrap();
        assert_eq!(
            first.mode().unwrap().resolve(CardMode::Automatic),
            CardMode::Manual
        );
        for bytes in [
            r#"{"auto_open":"other"}"#,
            r#"{"auto_open":"inherit"}"#,
            r#"{}"#,
            r#"{"auto_open":"manual","pane":"old"}"#,
        ] {
            std::fs::write(first.path(), bytes).unwrap();
            assert!(first.mode().is_err());
        }
    }

    #[test]
    fn conversation_id_parse_pair_requires_exactly_two_segments() {
        let parsed = ConversationId::parse_pair("pi/abc-123").unwrap();
        assert_eq!(parsed.harness, "pi");
        assert_eq!(parsed.native_id, "abc-123");
        for bad in ["abc", "host/pi/abc", "/abc", "pi/", ""] {
            assert!(ConversationId::parse_pair(bad).is_err(), "{bad:?}");
        }
    }
}
