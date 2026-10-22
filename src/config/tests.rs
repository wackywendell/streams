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
        (ConfigKey::StreamSeconds, "sync.stream_seconds"),
        (ConfigKey::SessionSeconds, "sync.session_seconds"),
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
            "unknown streams config key: sync.attempts",
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
fn durations_are_strict_nonnegative_finite_json_numbers() {
    let config = config();
    for key in [ConfigKey::StreamSeconds, ConfigKey::SessionSeconds] {
        let name = key.as_ref().split_once('.').unwrap().1;
        for text in ["0", "-0.0", "0.25", "12", "1e100"] {
            let expected = text.parse::<f64>().unwrap();
            assert_eq!(
                config.parse_value(key, text).unwrap().as_f64(),
                Some(expected)
            );
            let mut data = json!({"sync":{}});
            data["sync"][name] = load_json(text).unwrap();
            let parsed = config.validate_overrides(data).unwrap();
            assert_eq!(parsed["sync"][name].as_f64(), Some(expected));
        }
        for text in [
            "-1",
            "true",
            "false",
            "null",
            "\"12\"",
            "{}",
            "[]",
            "NaN",
            "Infinity",
            "-Infinity",
            "1e999",
            "",
        ] {
            assert!(config.parse_value(key, text).is_err(), "{key} {text}");
            let document = format!("{{\"sync\":{{\"{name}\":{text}}}}}");
            let parsed = load_json(&document)
                .map_err(|error| invalid(error.to_string()))
                .and_then(|data| config.validate_overrides(data));
            assert!(parsed.is_err(), "{document}");
        }
    }
}

#[test]
fn overrides_normalize_paths_and_preserve_explicit_frontend_and_numbers() {
    assert_eq!(
        config()
            .validate_overrides(json!({
                "catalog":{"root":"~/Catalog"},
                "frontend":{"mode":"third-party-adapter"},
                "sync":{"stream_seconds":0.25,"session_seconds":12}
            }))
            .unwrap(),
        json!({
            "catalog":{"root":"/home/test/Catalog"},
            "frontend":{"mode":"third-party-adapter"},
            "sync":{"stream_seconds":0.25,"session_seconds":12}
        })
    );
}
