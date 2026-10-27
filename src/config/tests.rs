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
    }
    for wire in [
        "unknown",
        "notes.root",
        "sync.stream_seconds",
        "sync.session_seconds",
        "CatalogRoot",
        "catalog.Root",
        "catalog.root ",
        "",
    ] {
        assert!(wire.parse::<ConfigKey>().is_err(), "{wire:?}");
    }
}

#[test]
fn unknown_json_keys_preserve_diagnostics() {
    for (data, message) in [
        (
            json!({"catalog":{"unknown":"x"}}),
            "unknown streams config key: catalog.unknown",
        ),
        (
            json!({"sync":{"attempts":1}}),
            "invalid config section: sync",
        ),
        (
            json!({"frontend":{"Mode":"none"}}),
            "unknown streams config key: frontend.Mode",
        ),
        (json!({"notes":{}}), "invalid config section: notes"),
    ] {
        assert_eq!(
            config().validate_overrides(data).unwrap_err().to_string(),
            message
        );
    }
}

#[test]
fn malformed_settings_schema_is_rejected() {
    for data in [
        json!([]),
        Value::Null,
        json!({"catalog":null}),
        json!({"catalog":[]}),
        json!({"catalog":{"root":1}}),
        json!({"catalog":{"root":"relative"}}),
        json!({"catalog":{"root":"/bad\0path"}}),
        json!({"catalog":{"unknown":"x"}}),
        json!({"notes":{}}),
        json!({"sync":{"attempts":1}}),
        json!({"sync":{"stream_seconds":300}}),
        json!({"sync":{"session_seconds":3600}}),
        json!({"frontend":{"mode":true}}),
        json!({"frontend":{"mode":" "}}),
    ] {
        assert!(config().validate_overrides(data.clone()).is_err(), "{data}");
    }
}

#[test]
fn invalid_setting_text_is_rejected() {
    for (key, value) in [
        (ConfigKey::CatalogRoot, "relative"),
        (ConfigKey::CatalogRoot, ""),
        (ConfigKey::CatalogRoot, "/bad\0path"),
        (ConfigKey::FrontendMode, ""),
    ] {
        assert!(config().parse_value(key, value).is_err(), "{key} {value:?}");
    }
}

#[test]
fn title_defaults_accept_only_automatic_or_manual_and_default_independently() {
    for key in [ConfigKey::StreamNaming, ConfigKey::SessionNaming] {
        for mode in ["automatic", "manual"] {
            assert_eq!(config().parse_value(key, mode).unwrap(), json!(mode));
        }
        for mode in ["inherit", "", "Automatic", "manual ", "null"] {
            assert!(config().parse_value(key, mode).is_err(), "{key}: {mode}");
        }
    }
    for invalid in [Value::Null, json!(true), json!(1), json!("inherit")] {
        assert!(
            config()
                .validate_overrides(json!({"naming":{"stream":invalid}}))
                .is_err()
        );
    }
    let defaults: TitleDefaults = serde_json::from_value(json!({"stream":"manual"})).unwrap();
    assert_eq!(defaults.stream, NamingMode::Manual);
    assert_eq!(defaults.session, NamingMode::Automatic);
    assert_eq!(
        TitleDefaults::default(),
        TitleDefaults {
            stream: NamingMode::Automatic,
            session: NamingMode::Automatic,
        }
    );
}

#[test]
fn overrides_normalize_paths_and_preserve_explicit_frontend() {
    assert_eq!(
        config()
            .validate_overrides(json!({
                "catalog":{"root":"~/Catalog"},
                "frontend":{"mode":"third-party-adapter"}
            }))
            .unwrap(),
        json!({
            "catalog":{"root":"/home/test/Catalog"},
            "frontend":{"mode":"third-party-adapter"}
        })
    );
}
