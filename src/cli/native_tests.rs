use super::*;
use streams::{config::Strategy, runtime::conversations::Harness};

#[test]
fn current_identity_never_suppresses_configuration_errors() {
    let temp = tempfile::TempDir::new().unwrap();
    let cwd = fs::canonicalize(temp.path()).unwrap();
    let environment = BTreeMap::from([
        ("HOME".into(), cwd.to_str().unwrap().into()),
        ("STREAMS_STREAM_ID".into(), "effort".into()),
        ("HERDR_ENV".into(), "1".into()),
        ("HERDR_PANE_ID".into(), "w:p1".into()),
        (
            "HERDR_SOCKET_PATH".into(),
            cwd.join("unavailable.sock").to_str().unwrap().into(),
        ),
    ]);
    let app = App {
        config: Config::new(&environment, &cwd).unwrap(),
        environment,
        cwd,
        input: Input::Stdin,
        herdr: None,
    };
    let opened =
        application::initialize_records(&app.config, Strategy::None, None, "main").unwrap();
    assert_eq!(app.current_stream_id(&opened, None).unwrap(), "effort");
    fs::write(app.config.config_path(), "invalid config").unwrap();
    // An already-opened handle keeps the settings it was opened with; a new command
    // fails when it opens records and loads the invalid configuration.
    assert_eq!(app.current_stream_id(&opened, None).unwrap(), "effort");
    assert!(
        app.show_records(
            &Target {
                reference: None,
                current: true,
                frontend: None,
            },
            false,
        )
        .is_err()
    );
}

#[test]
fn exact_context_does_not_connect_or_read_native_placement_to_read_its_session() {
    let temp = tempfile::TempDir::new().unwrap();
    let cwd = fs::canonicalize(temp.path()).unwrap();
    let environment = BTreeMap::from([
        ("HOME".into(), cwd.to_str().unwrap().into()),
        ("HERDR_ENV".into(), "1".into()),
        ("HERDR_PANE_ID".into(), "w:p1".into()),
        (
            "HERDR_SOCKET_PATH".into(),
            cwd.join("unavailable.sock").to_str().unwrap().into(),
        ),
        (
            "CMUX_SOCKET_PATH".into(),
            cwd.join("also-unavailable.sock").to_str().unwrap().into(),
        ),
        (
            "STREAMS_STREAM_ID".into(),
            "stale-workspace-selector".into(),
        ),
    ]);
    let app = App {
        config: Config::new(&environment, &cwd).unwrap(),
        environment,
        cwd,
        input: Input::Stdin,
        herdr: None,
    };
    let opened =
        application::initialize_records(&app.config, Strategy::None, None, "main").unwrap();
    let stream = Stream::new("actual", "Actual", None, opened.host().as_str()).unwrap();
    let session = Session::new(
        Session::id_for(opened.host().as_str(), "pi", "native"),
        opened.host().as_str(),
        &stream.id,
        "pi",
        "native",
        "Lane",
    )
    .unwrap();
    let Record::Stream(stream) =
        application::create_record(opened.records(), &Record::Stream(stream.clone()))
            .unwrap()
            .current
            .record
    else {
        unreachable!()
    };
    let Record::Session(session) =
        application::create_record(opened.records(), &Record::Session(session.clone()))
            .unwrap()
            .current
            .record
    else {
        unreachable!()
    };
    let state = opened.runtime_state(&app.config).unwrap();
    fs::create_dir_all(state.root()).unwrap();
    fs::write(
        state.root().join("placements"),
        "native inputs must not be inspected",
    )
    .unwrap();
    let observed = app.agent_observation(Harness::Pi, "native", None).unwrap();
    assert_eq!(observed.context.session, Some(session));
    assert_eq!(observed.context.stream, Some(stream));
    let output = observed.output().unwrap();
    assert!(output.value.get("native").is_none());
    assert!(output.value.get("native_inventory").is_none());
}

#[test]
fn operation_uses_the_settings_that_opened_the_records() {
    let temp = tempfile::TempDir::new().unwrap();
    let cwd = fs::canonicalize(temp.path()).unwrap();
    let environment = BTreeMap::from([
        ("HOME".into(), cwd.to_str().unwrap().into()),
        ("HERDR_ENV".into(), "1".into()),
        ("HERDR_PANE_ID".into(), "w:p1".into()),
        (
            "HERDR_SOCKET_PATH".into(),
            cwd.join("unavailable.sock").to_str().unwrap().into(),
        ),
    ]);
    let app = App {
        config: Config::new(&environment, &cwd).unwrap(),
        environment,
        cwd: cwd.clone(),
        input: Input::Stdin,
        herdr: None,
    };
    app.config.set(ConfigKey::FrontendMode, "none").unwrap();
    app.config.set(ConfigKey::StreamNaming, "manual").unwrap();
    let _ = application::initialize_records(&app.config, Strategy::None, None, "main").unwrap();
    let opened = application::open_records(&app.config).unwrap();

    // Mutate configuration on disk after opening: change catalog root, frontend mode and naming,
    // then make the file unreadable TOML. Operations on `opened` must keep using the settings
    // it was opened with and never reload config.toml.
    let other_root = cwd.join("other-catalog");
    app.config
        .set(ConfigKey::CatalogRoot, other_root.to_str().unwrap())
        .unwrap();
    app.config.set(ConfigKey::FrontendMode, "herdr").unwrap();
    app.config
        .set(ConfigKey::StreamNaming, "automatic")
        .unwrap();

    assert!(app.frontend(&opened, None, 5.0).unwrap().is_none());
    let err = app.current_stream_id(&opened, None).unwrap_err();
    assert!(
        err.to_string()
            .contains("--current requires an explicit Stream key or a frontend"),
        "{err}"
    );
    assert_eq!(
        opened.settings().naming.stream,
        streams::model::NamingMode::Manual
    );

    fs::write(app.config.config_path(), "invalid toml [").unwrap();
    assert!(app.frontend(&opened, None, 5.0).unwrap().is_none());
}
