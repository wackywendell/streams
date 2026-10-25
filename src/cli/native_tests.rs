use super::*;
use streams_temper::{config::Strategy, runtime::conversations::Harness};

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
    assert_eq!(
        app.current_stream_id(opened.host().as_str(), opened.source_id(), None)
            .unwrap(),
        "effort"
    );
    fs::write(app.config.config_path(), "invalid config").unwrap();
    assert!(
        app.current_stream_id(opened.host().as_str(), opened.source_id(), None)
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
    application::create_record(opened.records(), "stream", &Record::Stream(stream.clone()))
        .unwrap();
    application::create_record(
        opened.records(),
        "session",
        &Record::Session(session.clone()),
    )
    .unwrap();
    let state = app.state(opened.root(), opened.host().as_str()).unwrap();
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
