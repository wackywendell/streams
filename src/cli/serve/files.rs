//! Native directory watches, narrowed to the owner's logical records before any read.

use super::{
    super::{App, CliError, Result, application},
    Wake,
};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::mpsc::Sender,
};
use streams_temper::{
    agent,
    model::Record,
    runtime::{
        conversations::Harness,
        state::{RuntimeState, herdr_scope},
    },
    yaml_store::{YamlStore, encode_component},
};

type Versions = BTreeMap<&'static str, std::result::Result<Option<Vec<u8>>, String>>;

pub(super) struct Files {
    watcher: RecommendedWatcher,
    sender: Sender<Wake>,
    directories: BTreeSet<PathBuf>,
    groups: BTreeMap<&'static str, Vec<PathBuf>>,
    versions: Versions,
    root: PathBuf,
    host: String,
    session: String,
    legacy: Option<PathBuf>,
    stream: Option<String>,
    runtime: RuntimeState,
    scope: Option<String>,
    generation: u8,
}

fn watcher(sender: Sender<Wake>, generation: u8) -> Result<RecommendedWatcher> {
    // RecommendedWatcher is native (FSEvents/inotify); never substitute a polling backend.
    notify::recommended_watcher(move |event| {
        let _ = sender.send(Wake::Files(generation, event));
    })
    .map_err(|e| CliError::message(format!("required file subscription: {e}")))
}

impl Files {
    pub fn new(app: &App, session: &str, sender: Sender<Wake>) -> Result<Self> {
        // Native backends report physical paths (for example /private/var on macOS).
        let root = fs::canonicalize(app.config.load()?.catalog_root)?;
        let host = app.config.host_identity()?.as_str().to_owned();
        let runtime = RuntimeState::new(app.config.runtime_state_dir_for(&root)?, &host)?;
        let scope = app.herdr.as_ref().map(|read| {
            let endpoint = read.client.endpoint();
            herdr_scope(&endpoint.socket, endpoint.session.as_deref())
        });
        let mut files = Self {
            watcher: watcher(sender.clone(), 0)?,
            sender,
            directories: BTreeSet::new(),
            groups: BTreeMap::new(),
            versions: BTreeMap::new(),
            root,
            session: agent::session_id_for(&host, Harness::Pi, session),
            host,
            legacy: None,
            stream: app.environment.get("STREAMS_STREAM_ID").cloned(),
            runtime,
            scope,
            generation: 0,
        };
        files.prepare(
            app.environment
                .get("HERDR_WORKSPACE_ID")
                .map(String::as_str),
        )?;
        Ok(files)
    }

    /// Discover historical IDs/legacy locations once, after the initial watch is active.
    /// The subsequent owner observation covers changes during this bootstrap read.
    pub fn discover(&mut self, app: &App, native_session: &str) -> Result<()> {
        let opened = application::open(&app.config)?;
        let catalog = opened.backend().snapshot()?.catalog;
        if let Some(session) = catalog.sessions().iter().find(|session| {
            session.host_id == self.host
                && session.harness == "pi"
                && session.harness_session_id == native_session
        }) {
            self.session = session.id.clone();
            self.stream = Some(session.stream_id.clone());
            let record = Record::Session(session.clone());
            let store = YamlStore::new(&self.root);
            let relative = store.record_path(&record);
            if relative.starts_with("sessions") {
                self.legacy = Some(self.root.join(relative));
            }
        }
        Ok(())
    }

    fn session_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![
            self.root
                .join("conversations")
                .join(encode_component(&self.host))
                .join(format!("{}.yaml", encode_component(&self.session))),
        ];
        paths.extend(self.legacy.clone());
        paths
    }

    /// Baseline before observation, not after it: a write racing the read must
    /// remain a future trigger rather than being silently marked as observed.
    pub fn prepare(&mut self, workspace: Option<&str>) -> Result<bool> {
        let session_paths = self.session_paths();
        if let Ok(Some(bytes)) = version(&session_paths)
            && let Some(path) = session_paths.iter().find(|path| path.exists())
            && let Ok(relative) = path.strip_prefix(&self.root)
            && let Ok(Record::Session(session)) =
                YamlStore::decode_record(&relative.to_string_lossy(), &bytes)
        {
            self.stream = Some(session.stream_id);
        }
        let mut groups = BTreeMap::from([("session", session_paths)]);
        if let Some(stream) = &self.stream {
            groups.insert(
                "stream",
                vec![
                    self.root
                        .join("streams")
                        .join(format!("{}.yaml", encode_component(stream))),
                ],
            );
        }
        if let (Some(scope), Some(workspace)) = (&self.scope, workspace.filter(|id| !id.is_empty()))
        {
            let [association, claim] = self.runtime.workspace_binding_paths(scope, workspace)?;
            groups.insert("association", vec![association]);
            groups.insert("claim", vec![claim]);
        }
        self.groups = groups;
        self.watch_parents()?;
        let versions = self
            .groups
            .iter()
            .map(|(key, paths)| (*key, version(paths)))
            .collect();
        let changed = self.versions != versions;
        self.versions = versions;
        Ok(changed)
    }

    fn watch_parents(&mut self) -> Result<()> {
        let mut directories = BTreeSet::new();
        for path in self.groups.values().flatten() {
            let mut parent = path.parent().expect("absolute record parent");
            loop {
                match fs::metadata(parent) {
                    Ok(metadata) if metadata.is_dir() => break,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        parent = parent
                            .parent()
                            .ok_or_else(|| CliError::message("no existing watch parent"))?;
                    }
                    _ => {
                        return Err(CliError::message(format!(
                            "required watch parent is not a directory: {}",
                            parent.display()
                        )));
                    }
                }
            }
            directories.insert(parent.to_path_buf());
        }
        // Register replacements first. Parent watches survive atomic file replacement.
        for path in directories.difference(&self.directories) {
            self.watcher
                .watch(path, RecursiveMode::NonRecursive)
                .map_err(|e| {
                    CliError::message(format!(
                        "required file subscription at {}: {e}",
                        path.display()
                    ))
                })?;
        }
        for path in self.directories.difference(&directories) {
            self.watcher
                .unwatch(path)
                .map_err(|e| CliError::message(format!("required file unsubscribe: {e}")))?;
        }
        self.directories = directories;
        Ok(())
    }

    pub fn relevant(&self, event: &Event) -> bool {
        if !matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
        ) {
            return false;
        }
        event.paths.iter().any(|path| {
            self.groups.values().flatten().any(|target| {
                // Missing ancestor creation/deletion only repairs this narrow watch;
                // unchanged logical file bytes still produce no observation.
                target == path
                    || target.starts_with(path)
                        && matches!(
                            event.kind,
                            EventKind::Create(_)
                                | EventKind::Remove(_)
                                | EventKind::Modify(notify::event::ModifyKind::Name(_))
                        )
            })
        })
    }

    pub fn current(&self, generation: u8) -> bool {
        self.generation == generation
    }

    pub fn recover(&mut self) -> Result<()> {
        if self.generation != 0 {
            return Err(CliError::message(
                "required file subscription dropped again",
            ));
        }
        self.generation = 1;
        self.watcher = watcher(self.sender.clone(), self.generation)?;
        self.directories.clear();
        self.watch_parents()
    }
}

fn version(paths: &[PathBuf]) -> std::result::Result<Option<Vec<u8>>, String> {
    for path in paths {
        match fs::read(path) {
            Ok(bytes) => return Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, ModifyKind, RenameMode};
    use std::sync::mpsc;

    #[test]
    fn exact_paths_filter_before_reads_and_atomic_echoes_coalesce() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let environment = BTreeMap::from([("HOME".into(), root.to_string_lossy().into_owned())]);
        let app = App {
            config: super::super::super::Config::new(&environment, root).unwrap(),
            environment,
            cwd: root.into(),
            input: super::super::super::Input::Stdin,
            herdr: None,
        };
        application::initialize(
            &app.config,
            streams_temper::config::Strategy::None,
            None,
            "main",
            false,
        )
        .unwrap();
        let (sender, _) = mpsc::channel();
        let mut files = Files::new(&app, "pi", sender).unwrap();
        let path = files.session_paths().remove(0);
        let unrelated = Event::new(EventKind::Create(CreateKind::File))
            .add_path(path.with_file_name("unrelated.yaml"));
        assert!(!files.relevant(&unrelated));
        let mut observations = 0;
        for _ in 0..32 {
            if files.relevant(&unrelated) && files.prepare(None).unwrap() {
                observations += 1;
            }
        }
        assert_eq!(observations, 0);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let temporary = path.with_file_name("save.tmp");
        fs::write(&temporary, b"changed record").unwrap();
        fs::rename(temporary, &path).unwrap();
        let rename = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::To))).add_path(path);
        for _ in 0..32 {
            if files.relevant(&rename) && files.prepare(None).unwrap() {
                observations += 1;
            }
        }
        assert_eq!(
            observations, 1,
            "duplicate save notifications must coalesce"
        );
        files.recover().unwrap();
        assert!(
            !files.current(0),
            "queued errors from the dropped watcher must be ignored"
        );
        assert!(files.current(1));
        assert!(files.recover().is_err(), "a second actual drop is fatal");
    }
}
