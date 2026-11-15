//! One host-local preference per catalog conversation, independent of membership.
use super::*;
use crate::{
    config::{CardMode, Config},
    integration::ConversationId,
    model::Session,
};
use std::path::PathBuf;

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
        assert_eq!(first.mode().unwrap(), CardMode::Inherit);
        assert!(!first.path().exists());
        first.set(CardMode::Manual).unwrap();
        assert_eq!(store("catalog", "one").mode().unwrap(), CardMode::Manual);
        assert_eq!(store("catalog", "two").mode().unwrap(), CardMode::Inherit);
        assert_eq!(store("other", "one").mode().unwrap(), CardMode::Inherit);
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
        assert_eq!(codex.mode().unwrap(), CardMode::Inherit);
        first.set(CardMode::Automatic).unwrap();
        assert_eq!(first.mode().unwrap(), CardMode::Automatic);
        first.set(CardMode::Inherit).unwrap();
        assert_eq!(
            first.mode().unwrap().resolve(CardMode::Manual),
            CardMode::Manual
        );
        assert_eq!(
            first.mode().unwrap().resolve(CardMode::Automatic),
            CardMode::Automatic
        );
        for bytes in [
            r#"{"auto_open":"other"}"#,
            r#"{}"#,
            r#"{"auto_open":"manual","pane":"old"}"#,
        ] {
            std::fs::write(first.path(), bytes).unwrap();
            assert!(first.mode().is_err());
        }
    }
}
