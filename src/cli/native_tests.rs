use super::*;
use streams_temper::config::Strategy;

#[test]
fn current_identity_never_suppresses_configuration_errors() {
    let temp = tempfile::TempDir::new().unwrap();
    let cwd = fs::canonicalize(temp.path()).unwrap();
    let environment = BTreeMap::from([
        ("HOME".into(), cwd.to_str().unwrap().into()),
        ("STREAMS_STREAM_ID".into(), "effort".into()),
    ]);
    let app = App {
        config: Config::new(&environment, &cwd).unwrap(),
        environment,
        cwd,
        input: Input::Stdin,
        herdr: None,
    };
    let opened = application::initialize(&app.config, Strategy::None, None, "main", false).unwrap();
    assert_eq!(app.current_stream_id(&opened, None).unwrap(), "effort");
    fs::write(app.config.config_path(), "invalid config").unwrap();
    assert!(app.current_stream_id(&opened, None).is_err());
}
