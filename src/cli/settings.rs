use streams_temper::{child_protocol::TitleTarget, model::NamingMode};

use super::*;

impl App {
    pub(super) fn set_title_default(
        &self,
        target: TitleTarget,
        mode: NamingMode,
    ) -> Result<Output> {
        let key = match target {
            TitleTarget::Stream => ConfigKey::StreamNaming,
            TitleTarget::Session => ConfigKey::SessionNaming,
        };
        let defaults = self.config.with_lock(|config| {
            let mut defaults = config.settings()?.naming;
            match target {
                TitleTarget::Stream => defaults.stream = mode,
                TitleTarget::Session => defaults.session = mode,
            }
            config.set(key, mode.as_ref())?;
            Ok(defaults)
        })?;
        Ok(Output::value(json!({"title_defaults":defaults})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_defaults_preserve_other_settings_and_create_no_catalog_or_native_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let environment = BTreeMap::from([
            ("HOME".into(), root.to_str().unwrap().into()),
            ("HERDR_ENV".into(), "1".into()),
            (
                "HERDR_SOCKET_PATH".into(),
                root.join("absent.sock").to_str().unwrap().into(),
            ),
        ]);
        let app = App {
            config: Config::new(&environment, &root).unwrap(),
            environment,
            cwd: root.clone(),
            input: Input::Stdin,
            herdr: None,
        };
        app.config.set(ConfigKey::FrontendMode, "none").unwrap();
        assert_eq!(
            app.set_title_default(TitleTarget::Stream, NamingMode::Manual)
                .unwrap()
                .value,
            json!({"title_defaults":{"stream":"manual","session":"automatic"}})
        );
        let other_handle = app.config.clone();
        other_handle
            .set(ConfigKey::SessionNaming, "manual")
            .unwrap();
        assert_eq!(
            app.set_title_default(TitleTarget::Stream, NamingMode::Automatic)
                .unwrap()
                .value,
            json!({"title_defaults":{"stream":"automatic","session":"manual"}})
        );
        assert_eq!(
            app.set_title_default(TitleTarget::Session, NamingMode::Automatic)
                .unwrap()
                .value,
            json!({"title_defaults":{"stream":"automatic","session":"automatic"}})
        );
        assert_eq!(
            app.config.get(ConfigKey::FrontendMode).unwrap(),
            json!("none")
        );
        assert!(!app.config.load().unwrap().catalog_root.exists());
        assert!(!root.join(".local/state").exists());
        assert!(!root.join("absent.sock").exists());
        let config = app.config.read_overrides().unwrap();
        assert_eq!(
            config,
            json!({
                "naming":{"stream":"automatic","session":"automatic"},
                "frontend":{"mode":"none"}
            })
        );
    }
}
