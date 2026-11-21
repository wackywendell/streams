use super::*;
use crate::cli::Input;
use serde_json::json;
use std::{collections::BTreeMap, fs};
use streams::file_store::records::FileRecordStore;

const HOST: &str = "01234567-89ab-4def-8123-456789abcdef";

#[test]
fn watched_inputs_have_one_canonical_session_and_stream_address_and_both_directory_selections() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = fs::canonicalize(directory.path()).unwrap();
    let environment = BTreeMap::from([("HOME".into(), cwd.to_str().unwrap().into())]);
    let app = App {
        config: Config::new(&environment, &cwd).unwrap(),
        environment,
        cwd: cwd.clone(),
        input: Input::Stdin,
        herdr: None,
    };
    fs::create_dir_all(app.config.host_path().parent().unwrap()).unwrap();
    fs::write(
        app.config.host_path(),
        json!({"version":1,"id":HOST}).to_string(),
    )
    .unwrap();
    let settings = app.config.load().unwrap();
    FileRecordStore::new(&settings.catalog_root, HOST)
        .unwrap()
        .initialize()
        .unwrap();
    let records = application::open_records(&app.config).unwrap();
    let native_id = "11234567-89ab-4def-8123-456789abcdef";
    let scope = Scope {
        catalog_source: records.source_id().into(),
        host_id: HOST.into(),
        stream_id: "project-name".into(),
        session_id: native_id.into(),
        harness: "pi".into(),
        native_session_id: native_id.into(),
    };
    let inputs = Inputs::new(&records, &scope).unwrap();
    let paths = inputs.paths(&scope.stream_id).unwrap();
    assert_eq!(
        paths,
        BTreeSet::from([
            records
                .root()
                .join(format!("hosts/{HOST}/sessions/pi/{native_id}/session.json")),
            records
                .root()
                .join(format!("hosts/{HOST}/streams/project-name.json")),
        ])
    );
    let destination = inputs.paths("destination").unwrap();
    assert!(
        destination.contains(
            &records
                .root()
                .join(format!("hosts/{HOST}/streams/destination.json"))
        )
    );
    assert!(
        !destination.contains(
            &records
                .root()
                .join(format!("hosts/{HOST}/streams/project-name.json"))
        )
    );
    assert!(destination.contains(&inputs.session_path));
    for field in ["source", "host", "session", "harness"] {
        let mut invalid = scope.clone();
        match field {
            "source" => invalid.catalog_source = "other".into(),
            "host" => invalid.host_id = "other".into(),
            "session" => invalid.session_id = "legacy-native-alias".into(),
            "harness" => invalid.harness = "codex".into(),
            _ => unreachable!(),
        };
        assert!(Inputs::new(&records, &invalid).is_err());
    }
    let stream = streams::model::Stream::new(
        HOST,
        &scope.stream_id,
        streams::model::Title::new("Work", false),
        None,
    )
    .unwrap();
    let session = Session::new(
        HOST,
        "pi",
        native_id,
        &scope.stream_id,
        streams::model::Title::new("Conversation", false),
    )
    .unwrap();
    application::create_record(records.records(), &Record::Stream(stream)).unwrap();
    application::create_record(records.records(), &Record::Session(session)).unwrap();
    let mut files = Files::new(paths, Events::new(PaneEvents::new(|_| {}))).unwrap();
    let session = inputs.session(&files).unwrap();
    let stream_path = records
        .root()
        .join(format!("hosts/{HOST}/streams/project-name.json"));
    fs::write(&inputs.session_path, "not a record").unwrap();
    fs::write(stream_path, "not a record").unwrap();
    let observed = inputs
        .records(
            &files,
            session,
            streams::tui::NativeOwnerObservation {
                pane_id: "pane".into(),
                tab_id: "tab".into(),
                workspace_id: "workspace".into(),
            },
        )
        .unwrap();
    assert_eq!(
        observed.scope(),
        &scope,
        "building a frame must reuse watched bytes, not reopen its records"
    );
    files.consume(Vec::new(), Refresh::Full).unwrap();
    assert!(
        inputs.session(&files).is_err(),
        "explicit refresh must reread the files rather than keep a stale frame"
    );
}
