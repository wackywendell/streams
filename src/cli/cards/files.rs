use super::events::{Change, Events};
use crate::cli::file_watch::{self, FileWatch};
use notify::Event;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use streams::tui::application::Refresh;

type Version = Result<Option<Vec<u8>>, String>;

pub(super) struct Files {
    watch: FileWatch,
    paths: Arc<Mutex<BTreeSet<PathBuf>>>,
    versions: BTreeMap<PathBuf, Version>,
}
pub(super) struct FileChanges {
    pub changed: bool,
    pub recovered: bool,
}
impl Files {
    pub fn new(paths: BTreeSet<PathBuf>, events: Events) -> Result<Self, String> {
        let shared = Arc::new(Mutex::new(BTreeSet::new()));
        let filter = shared.clone();
        let watch = FileWatch::new(move |generation, event: notify::Result<Event>| {
            if event.as_ref().map_or(true, |event| {
                event.need_rescan()
                    || file_watch::relevant(&*filter.lock().expect("card watch paths"), event)
            }) {
                events.push(Change::File(generation, event));
            }
        })?;
        let mut files = Self {
            watch,
            paths: shared,
            versions: BTreeMap::new(),
        };
        files.set_paths(paths)?;
        Ok(files)
    }
    pub fn set_paths(&mut self, paths: BTreeSet<PathBuf>) -> Result<(), String> {
        let observed = paths.clone();
        *self.paths.lock().expect("card watch paths") = paths.clone();
        self.watch_parents(&paths)?;
        self.versions.retain(|path, _| observed.contains(path));
        for path in observed {
            self.versions
                .entry(path.clone())
                .or_insert_with(|| file_watch::version(&path));
        }
        Ok(())
    }
    fn watch_parents(&mut self, paths: &BTreeSet<PathBuf>) -> Result<(), String> {
        self.watch.watch(paths.iter().map(PathBuf::as_path), [])
    }
    pub fn consume(
        &mut self,
        events: Vec<(u8, notify::Result<Event>)>,
        refresh: Refresh,
    ) -> Result<FileChanges, String> {
        let paths = self.paths.lock().expect("card watch paths").clone();
        let mut touched = if refresh == Refresh::Full {
            paths.clone()
        } else {
            BTreeSet::new()
        };
        let mut recovered = false;
        for (generation, event) in events {
            if !self.watch.current(generation) {
                continue;
            }
            let rescan = event.as_ref().map_or(true, Event::need_rescan);
            if rescan {
                self.watch.recover()?;
                touched.extend(paths.iter().cloned());
                recovered = true;
            } else if let Ok(event) = event
                && file_watch::relevant(&paths, &event)
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
            let next = file_watch::version(&path);
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
    use notify::EventKind;
    use std::fs;
    use streams::tui::application::PaneEvents;
    fn events() -> Events {
        Events::new(PaneEvents::new(|_| {}))
    }
    #[test]
    fn access_notifications_do_not_invalidate_catalog_context() {
        let root = tempfile::tempdir().unwrap();
        let own = root.path().join("own.json");
        fs::write(&own, "context").unwrap();
        let mut files = Files::new([own.clone()].into(), events()).unwrap();
        for kind in [
            notify::event::AccessKind::Open(notify::event::AccessMode::Read),
            notify::event::AccessKind::Close(notify::event::AccessMode::Read),
        ] {
            let event = Event::new(EventKind::Access(kind)).add_path(own.clone());
            assert!(!file_watch::relevant(
                &BTreeSet::from([own.clone()]),
                &event
            ));
            assert!(
                !files
                    .consume(vec![(0, Ok(event))], Refresh::Changed)
                    .unwrap()
                    .changed
            );
        }
    }
    #[test]
    fn exact_names_atomic_rename_and_duplicate_notifications_cost_one_logical_read() {
        let root = tempfile::tempdir().unwrap();
        let own = root.path().join("own.json");
        fs::write(&own, "before").unwrap();
        let mut files = Files::new([own.clone()].into(), events()).unwrap();
        let other = Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(root.path().join("other.json"));
        assert!(!file_watch::relevant(
            &BTreeSet::from([own.clone()]),
            &other
        ));
        let staging = root.path().join("staging.tmp");
        fs::write(&staging, "after").unwrap();
        fs::rename(&staging, &own).unwrap();
        let event = Event::new(EventKind::Modify(notify::event::ModifyKind::Name(
            notify::event::RenameMode::Both,
        )))
        .add_path(staging)
        .add_path(own.clone());
        assert!(file_watch::relevant(&BTreeSet::from([own]), &event));
        let mut reads = 0;
        for _ in 0..32 {
            if files
                .consume(vec![(0, Ok(event.clone()))], Refresh::Changed)
                .unwrap()
                .changed
            {
                reads += 1;
            }
        }
        assert_eq!(reads, 1);
        let dropped = || notify::Error::generic("fixture watcher lost");
        assert!(
            files
                .consume(vec![(0, Err(dropped()))], Refresh::Changed)
                .unwrap()
                .recovered
        );
        assert!(
            !files
                .consume(vec![(0, Err(dropped()))], Refresh::Changed)
                .unwrap()
                .changed
        );
        assert!(
            files
                .consume(vec![(1, Err(dropped()))], Refresh::Changed)
                .is_err()
        );
    }
}
