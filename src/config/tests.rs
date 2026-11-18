use super::*;
use strum::IntoEnumIterator;

fn config() -> Config {
    Config::new(
        &BTreeMap::from([("HOME".into(), "/home/test".into())]),
        Path::new("/work"),
    )
    .unwrap()
}

#[test]
fn config_keys_roundtrip_exact_wire_names() {
    let keys = [
        (ConfigKey::CatalogRoot, "catalog.root"),
        (ConfigKey::FrontendMode, "frontend.mode"),
        (ConfigKey::StreamNaming, "naming.stream"),
        (ConfigKey::SessionNaming, "naming.session"),
        (ConfigKey::LifecycleDecayDays, "lifecycle.decay_days"),
        (ConfigKey::HerdrViewerMode, "herdr.viewer.mode"),
        (ConfigKey::HerdrViewerPlacement, "herdr.viewer.placement"),
        (ConfigKey::HerdrNamingTab, "herdr.naming.tab"),
        (ConfigKey::HerdrNamingWorkspace, "herdr.naming.workspace"),
        (ConfigKey::HerdrCardsAutoOpen, "herdr.cards.auto_open"),
        (ConfigKey::HerdrTimeoutMs, "herdr.timeout_ms"),
        (
            ConfigKey::HerdrPaneHeightFraction,
            "herdr.pane_height_fraction",
        ),
        (ConfigKey::PiNamingSession, "pi.naming.session"),
        (ConfigKey::PiNamingImportEdits, "pi.naming.import_edits"),
        (ConfigKey::PiReportingMode, "pi.reporting.mode"),
        (ConfigKey::PiReportingEnrollment, "pi.reporting.enrollment"),
        (ConfigKey::PiReportingFrequency, "pi.reporting.frequency"),
        (ConfigKey::PiReportingContext, "pi.reporting.context"),
        (ConfigKey::PiReportingModel, "pi.reporting.model"),
        (
            ConfigKey::PiReportingModelProvider,
            "pi.reporting.model.provider",
        ),
        (ConfigKey::PiReportingModelId, "pi.reporting.model.id"),
        (
            ConfigKey::PiReportingSessionLogging,
            "pi.reporting.session_logging",
        ),
        (
            ConfigKey::PiReportingDebugCommands,
            "pi.reporting.debug_commands",
        ),
    ];
    assert_eq!(
        ConfigKey::iter().collect::<Vec<_>>(),
        keys.map(|(key, _)| key)
    );
    for (key, wire) in keys {
        assert_eq!(wire.parse::<ConfigKey>().unwrap(), key);
        assert_eq!(key.to_string(), wire);
        assert_eq!(key.as_ref(), wire);
        assert_eq!(key.to_string().parse::<ConfigKey>().unwrap(), key);
        assert_eq!(wire.parse::<ConfigPath>().unwrap(), ConfigPath::Key(key));
    }
    for section in [
        "catalog",
        "frontend",
        "naming",
        "lifecycle",
        "herdr",
        "herdr.viewer",
        "herdr.naming",
        "herdr.cards",
        "pi",
        "pi.naming",
        "pi.reporting",
    ] {
        assert!(matches!(
            section.parse::<ConfigPath>().unwrap(),
            ConfigPath::Section(_)
        ));
    }
    for wire in [
        "unknown",
        "cards.auto_open",
        "notes.root",
        "sync.stream_seconds",
        "sync.session_seconds",
        "CatalogRoot",
        "catalog.Root",
        "catalog.root ",
        "",
    ] {
        assert!(wire.parse::<ConfigKey>().is_err(), "{wire:?}");
        assert!(wire.parse::<ConfigPath>().is_err(), "{wire:?}");
    }
}

#[test]
fn unknown_toml_keys_preserve_diagnostics() {
    for (source, message) in [
        (
            "[catalog]\nunknown = \"x\"\n",
            "unknown streams config key: catalog.unknown",
        ),
        ("[sync]\nattempts = 1\n", "invalid config section: sync"),
        (
            "[frontend]\nMode = \"none\"\n",
            "unknown streams config key: frontend.Mode",
        ),
        ("[notes]\n", "invalid config section: notes"),
        (
            "[herdr]\nunknown = 1\n",
            "unknown streams config key: herdr.unknown",
        ),
        (
            "[pi.reporting.model]\nprovider = \"a\"\nid = \"b\"\nextra = \"c\"\n",
            "unknown streams config key: pi.reporting.model.extra",
        ),
    ] {
        let table: toml::Table = toml::from_str(source).unwrap();
        assert_eq!(
            config()
                .validate_overrides_toml(table)
                .unwrap_err()
                .to_string(),
            message
        );
    }
}

#[test]
fn malformed_settings_schema_is_rejected() {
    for source in [
        "catalog = \"bad\"\n",
        "[catalog]\nroot = 1\n",
        "[catalog]\nroot = \"relative\"\n",
        "[frontend]\nmode = true\n",
        "[frontend]\nmode = \" \"\n",
        "[herdr.viewer]\nmode = \"nano\"\n",
        "[herdr.viewer]\nplacement = \"pane-left\"\n",
        "[herdr.naming]\ntab = \"true\"\n",
        "[herdr]\ntimeout_ms = 0\n",
        "[herdr]\ntimeout_ms = \"3000\"\n",
        "[herdr]\npane_height_fraction = 1.0\n",
        "[herdr]\npane_height_fraction = 0.0\n",
        "[pi.reporting]\nfrequency = 0\n",
        "[pi.reporting]\ncontext = \"raw\"\n",
        "[pi.reporting.model]\nprovider = \"openai\"\n",
        "[pi.reporting.model]\nid = \"gpt-5\"\n",
        "[pi.reporting.model]\nprovider = \" openai\"\nid = \"gpt-5\"\n",
    ] {
        let table: toml::Table = toml::from_str(source).unwrap();
        assert!(config().validate_overrides_toml(table).is_err(), "{source}");
    }
}

#[test]
fn invalid_setting_text_is_rejected() {
    for (key, value) in [
        (ConfigKey::CatalogRoot, "relative"),
        (ConfigKey::CatalogRoot, ""),
        (ConfigKey::CatalogRoot, "/bad\0path"),
        (ConfigKey::FrontendMode, ""),
        (ConfigKey::HerdrViewerMode, "vim"),
        (ConfigKey::HerdrViewerPlacement, "pane-left"),
        (ConfigKey::HerdrNamingTab, "yes"),
        (ConfigKey::HerdrTimeoutMs, "0"),
        (ConfigKey::HerdrPaneHeightFraction, "1.2"),
        (ConfigKey::PiReportingFrequency, "0"),
        (ConfigKey::PiReportingModel, "noslash"),
        (ConfigKey::PiReportingModel, "/missing-provider"),
        (ConfigKey::PiReportingModel, "provider/"),
    ] {
        assert!(
            config().parse_toml_value(key, value).is_err(),
            "{key} {value:?}"
        );
    }
}

#[test]
fn title_defaults_and_card_defaults_validate_strictly() {
    for key in [
        ConfigKey::StreamNaming,
        ConfigKey::SessionNaming,
        ConfigKey::PiReportingMode,
        ConfigKey::PiReportingEnrollment,
    ] {
        for mode in ["automatic", "manual"] {
            assert_eq!(
                config().parse_toml_value(key, mode).unwrap(),
                toml::Value::String(mode.into())
            );
        }
        for mode in ["inherit", "", "Automatic", "manual ", "null"] {
            assert!(
                config().parse_toml_value(key, mode).is_err(),
                "{key}: {mode}"
            );
        }
    }
    for mode in ["automatic", "manual"] {
        assert_eq!(
            config()
                .parse_toml_value(ConfigKey::HerdrCardsAutoOpen, mode)
                .unwrap(),
            toml::Value::String(mode.into())
        );
    }
    for mode in ["inherit", "", "Automatic", "manual ", "null"] {
        assert!(
            config()
                .parse_toml_value(ConfigKey::HerdrCardsAutoOpen, mode)
                .is_err()
        );
    }
}

#[test]
fn overrides_normalize_paths_and_preserve_explicit_frontend() {
    let table: toml::Table = toml::from_str(
        "[catalog]\nroot = \"~/Catalog\"\n[frontend]\nmode = \"third-party-adapter\"\n",
    )
    .unwrap();
    let validated = config().validate_overrides_toml(table).unwrap();
    assert_eq!(
        toml_table_to_json(&validated).unwrap(),
        json!({
            "catalog": {"root": "/home/test/Catalog"},
            "frontend": {"mode": "third-party-adapter"}
        })
    );
}
