use super::*;
use crate::cli::Input;
use std::{collections::BTreeMap, sync::Mutex};
use streams::config::{Config, Strategy};

#[test]
fn a_pathless_file_rescan_reinstalls_the_watch_before_filtering_paths() {
    let home = tempfile::tempdir().unwrap();
    let cwd = home.path().to_path_buf();
    let environment = BTreeMap::from([("HOME".into(), cwd.to_str().unwrap().into())]);
    let config = Config::new(&environment, &cwd).unwrap();
    let records = application::initialize_records(&config, Strategy::None, None, "main").unwrap();
    let app = App {
        config,
        environment,
        cwd,
        input: Input::Stdin,
        herdr: None,
    };
    let (sender, _receiver) = mpsc::channel();
    let mut controller = Controller::new(
        app,
        uuid::Uuid::new_v4().to_string(),
        home.path().join("plugin"),
        sender.clone(),
        Arc::new(Mutex::new(Some(std::io::stdout()))),
        Cancellation::new().unwrap(),
    );
    controller.files = Some(Files::new(&records, &controller.session_id, sender).unwrap());
    let event = notify::Event::new(notify::EventKind::Other).set_flag(notify::event::Flag::Rescan);
    assert!(event.paths.is_empty());
    controller
        .file_event(0, Ok(event), Arc::new(AtomicBool::new(false)))
        .unwrap();
    assert!(controller.files.as_ref().unwrap().current(1));
    assert!(!controller.files.as_mut().unwrap().prepare().unwrap());
}
