use super::*;
use streams::{config::Strategy, runtime::conversations::Harness};

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
    else {
        unreachable!()
    };
    let Record::Session(session) =
        application::create_record(opened.records(), &Record::Session(session.clone()))
            .unwrap()
            .current
    else {
        unreachable!()
    };
    let state = opened.runtime_state().unwrap();
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
    let host = opened.host().as_str();
    let stream = Stream::new("effort", "Original", None, host).unwrap();
    let Record::Stream(stream) =
        application::create_record(opened.records(), &Record::Stream(stream))
            .unwrap()
            .current
    else {
        unreachable!()
    };
    let session = Session::new(
        Session::id_for(host, "pi", "native"),
        host,
        &stream.id,
        "pi",
        "native",
        "Lane",
    )
    .unwrap();
    let Record::Session(session) =
        application::create_record(opened.records(), &Record::Session(session))
            .unwrap()
            .current
    else {
        unreachable!()
    };

    // Mutate configuration on disk after opening: change catalog root, frontend mode and naming,
    // then make the file invalid TOML. Operations on `opened` must use the settings it was
    // opened with and never reload config.toml.
    let other_root = cwd.join("other-catalog");
    app.config
        .set(ConfigKey::CatalogRoot, other_root.to_str().unwrap())
        .unwrap();
    app.config.set(ConfigKey::FrontendMode, "herdr").unwrap();
    app.config
        .set(ConfigKey::StreamNaming, "automatic")
        .unwrap();
    fs::write(app.config.config_path(), "invalid toml [").unwrap();

    assert!(app.frontend(&opened, None, 5.0).unwrap().is_none());
    let err = app.current_stream_id(&opened, None).unwrap_err();
    assert!(
        err.to_string()
            .contains("--current requires an explicit Stream key or a frontend"),
        "{err}"
    );

    // Stream has TitleMode::Inherit. Disk config was switched to automatic (then broken),
    // so an automatic title edit is refused only because `apply_opened` uses `opened.settings().naming`
    // (manual).
    let request = json!({
        "version": 2,
        "catalog_source": opened.source_id(),
        "actor": "automatic",
        "kind": "stream",
        "before": stream,
        "changes": {"title": "Auto title"},
        "title_default": "automatic",
        "inputs": [{"kind": "session", "record": session}],
    });
    let outcome = catalog::apply_opened(&opened, request).unwrap();
    assert_eq!(
        outcome.value["save"]["domain"]["outcome"], "superseded",
        "{}",
        outcome.value
    );
    assert_eq!(outcome.value["stream"]["title"], "Original");
}
