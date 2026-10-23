use super::events::{Change, Events};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

type Version = Result<Option<Vec<u8>>, String>;
fn version(path: &Path) -> Version {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("watched input must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok(Some(bytes))
}
fn relevant(paths: &BTreeSet<PathBuf>, event: &Event) -> bool {
    if !matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    ) {
        return false;
    }
    let structural = matches!(
        event.kind,
        EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(notify::event::ModifyKind::Name(_))
    );
    event.paths.iter().any(|path| {
        paths.contains(path) || structural && paths.iter().any(|target| target.starts_with(path))
    })
}
fn watcher(
    events: Events,
    paths: Arc<Mutex<BTreeSet<PathBuf>>>,
    generation: u8,
) -> Result<RecommendedWatcher, String> {
    // RecommendedWatcher is native (FSEvents/inotify), with no polling fallback.
    notify::recommended_watcher(move |event: notify::Result<Event>| {
        if event.as_ref().map_or(true, |event| {
            event.need_rescan() || relevant(&paths.lock().expect("card watch paths"), event)
        }) {
            events.push(Change::File(generation, event));
        }
    })
    .map_err(|e| e.to_string())
}

pub(super) struct Files {
    watcher: RecommendedWatcher,
    events: Events,
    paths: Arc<Mutex<BTreeSet<PathBuf>>>,
    parents: BTreeSet<PathBuf>,
    versions: BTreeMap<PathBuf, Version>,
    evidence: PathBuf,
    generation: u8,
}
pub(super) struct FileChanges {
    pub changed: bool,
    pub recovered: bool,
}
impl Files {
    pub fn new(
        paths: BTreeSet<PathBuf>,
        evidence: PathBuf,
        events: Events,
    ) -> Result<Self, String> {
        let shared = Arc::new(Mutex::new(BTreeSet::new()));
        let mut files = Self {
            watcher: watcher(events.clone(), shared.clone(), 0)?,
            events,
            paths: shared,
            parents: BTreeSet::new(),
            versions: BTreeMap::new(),
            evidence,
            generation: 0,
        };
        files.set_paths(paths)?;
        Ok(files)
    }
    pub fn set_paths(&mut self, mut paths: BTreeSet<PathBuf>) -> Result<(), String> {
        let observed = paths.clone();
        paths.insert(self.evidence.clone());
        *self.paths.lock().expect("card watch paths") = paths.clone();
        self.watch_parents(&paths)?;
        self.versions.retain(|path, _| observed.contains(path));
        for path in observed {
            self.versions
                .entry(path.clone())
                .or_insert_with(|| version(&path));
        }
        Ok(())
    }
    fn watch_parents(&mut self, paths: &BTreeSet<PathBuf>) -> Result<(), String> {
        let mut parents = BTreeSet::new();
        for path in paths {
            let mut parent = path.parent().ok_or("watched file has no parent")?;
            loop {
                match fs::metadata(parent) {
                    Ok(metadata) if metadata.is_dir() => break,
                    Ok(_) => {
                        return Err(format!(
                            "watch parent is not a directory: {}",
                            parent.display()
                        ));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        parent = parent.parent().ok_or("no existing watch parent")?
                    }
                    Err(error) => return Err(error.to_string()),
                }
            }
            parents.insert(parent.to_path_buf());
        }
        // Register replacements before releasing old parents: atomic renames and
        // newly created record directories must not introduce a subscription gap.
        for parent in parents.difference(&self.parents) {
            self.watcher
                .watch(parent, RecursiveMode::NonRecursive)
                .map_err(|e| e.to_string())?;
        }
        for parent in self.parents.difference(&parents) {
            self.watcher.unwatch(parent).map_err(|e| e.to_string())?;
        }
        self.parents = parents;
        Ok(())
    }
    pub fn consume(
        &mut self,
        events: Vec<(u8, notify::Result<Event>)>,
    ) -> Result<FileChanges, String> {
        let paths = self.paths.lock().expect("card watch paths").clone();
        let mut touched = BTreeSet::new();
        let mut recovered = false;
        for (generation, event) in events {
            if generation != self.generation {
                continue;
            }
            let rescan = event.as_ref().map_or(true, Event::need_rescan);
            if rescan {
                if self.generation != 0 {
                    return Err("required card file subscription dropped again".into());
                }
                self.generation = 1;
                self.watcher = watcher(self.events.clone(), self.paths.clone(), self.generation)?;
                self.parents.clear();
                touched.extend(paths.iter().cloned());
                recovered = true;
            } else if let Ok(event) = event
                && relevant(&paths, &event)
            {
                for path in event.paths {
                    touched.extend(
                        paths
                            .iter()
                            .filter(|target| *target == &path || target.starts_with(&path))
                            .cloned(),
                    );
                }
            }
        }
        if !touched.is_empty() {
            self.watch_parents(&paths)?;
        }
        let mut changed = recovered;
        for path in touched {
            // Evidence is a wake-only dependency, read once by DisplayLifetime.
            if path == self.evidence {
                continue;
            }
            let next = version(&path);
            if self.versions.get(&path) != Some(&next) {
                changed = true;
                self.versions.insert(path, next);
            }
        }
        Ok(FileChanges { changed, recovered })
    }
    pub fn bytes(&self, path: &Path) -> Result<Option<&[u8]>, String> {
        self.versions
            .get(path)
            .ok_or_else(|| "input is not watched".into())
            .and_then(|version| match version {
                Ok(Some(bytes)) => Ok(Some(bytes.as_slice())),
                Ok(None) => Ok(None),
                Err(error) => Err(error.clone()),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use streams_temper::tui::application::PaneEvents;
    fn events() -> Events {
        Events::new(PaneEvents::new(|_| {}))
    }
    #[test]
    fn access_notifications_and_evidence_only_changes_do_not_invalidate_catalog_context() {
        let root = tempfile::tempdir().unwrap();
        let own = root.path().join("own.yaml");
        let evidence = root.path().join("evidence.json");
        fs::write(&own, "context").unwrap();
        fs::write(&evidence, "reserved").unwrap();
        let mut files = Files::new([own.clone()].into(), evidence.clone(), events()).unwrap();
        for kind in [
            notify::event::AccessKind::Open(notify::event::AccessMode::Read),
            notify::event::AccessKind::Close(notify::event::AccessMode::Read),
        ] {
            let event = Event::new(EventKind::Access(kind)).add_path(own.clone());
            assert!(!relevant(&[own.clone()].into(), &event));
            assert!(!files.consume(vec![(0, Ok(event))]).unwrap().changed);
        }
        fs::write(&evidence, "ready").unwrap();
        let event = Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(evidence.clone());
        assert!(relevant(&[evidence.clone()].into(), &event));
        assert!(!files.consume(vec![(0, Ok(event))]).unwrap().changed);
        assert!(!files.versions.contains_key(&evidence));
    }
    #[test]
    fn exact_names_atomic_rename_and_duplicate_notifications_cost_one_logical_read() {
        let root = tempfile::tempdir().unwrap();
        let own = root.path().join("own.yaml");
        fs::write(&own, "before").unwrap();
        let mut files = Files::new(
            [own.clone()].into(),
            root.path().join("evidence.json"),
            events(),
        )
        .unwrap();
        let other = Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(root.path().join("other.yaml"));
        assert!(!relevant(&[own.clone()].into(), &other));
        let staging = root.path().join("staging.tmp");
        fs::write(&staging, "after").unwrap();
        fs::rename(&staging, &own).unwrap();
        let event = Event::new(EventKind::Modify(notify::event::ModifyKind::Name(
            notify::event::RenameMode::Both,
        )))
        .add_path(staging)
        .add_path(own.clone());
        assert!(relevant(&[own].into(), &event));
        let mut reads = 0;
        for _ in 0..32 {
            if files.consume(vec![(0, Ok(event.clone()))]).unwrap().changed {
                reads += 1;
            }
        }
        assert_eq!(reads, 1);
        let dropped = || notify::Error::generic("fixture watcher lost");
        assert!(files.consume(vec![(0, Err(dropped()))]).unwrap().recovered);
        assert!(!files.consume(vec![(0, Err(dropped()))]).unwrap().changed);
        assert!(files.consume(vec![(1, Err(dropped()))]).is_err());
    }
}
