//! Native watches on the parents of named files, shared by the Pi child and the cards host.
//!
//! A parent watch survives atomic replacement of the file. A missing parent is watched at
//! its nearest existing ancestor until it appears. A dropped subscription is recreated once.

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

type Handler = Arc<dyn Fn(u8, notify::Result<Event>) + Send + Sync>;

pub(super) struct FileWatch {
    watcher: RecommendedWatcher,
    handler: Handler,
    parents: BTreeSet<PathBuf>,
    generation: u8,
}

fn watcher(handler: Handler, generation: u8) -> Result<RecommendedWatcher, String> {
    // RecommendedWatcher is native (FSEvents/inotify); never substitute a polling backend.
    notify::recommended_watcher(move |event| handler(generation, event))
        .map_err(|error| format!("required file subscription: {error}"))
}

impl FileWatch {
    /// `handler` receives each event with the generation of the subscription that saw it.
    pub fn new(
        handler: impl Fn(u8, notify::Result<Event>) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let handler: Handler = Arc::new(handler);
        Ok(Self {
            watcher: watcher(handler.clone(), 0)?,
            handler,
            parents: BTreeSet::new(),
            generation: 0,
        })
    }

    /// Watch the parent of each file and each directory; see [`subscribe`].
    pub fn watch<'a>(
        &mut self,
        files: impl IntoIterator<Item = &'a Path>,
        directories: impl IntoIterator<Item = &'a Path>,
    ) -> Result<(), String> {
        let requested = files
            .into_iter()
            .map(|path| path.parent().ok_or("watched file has no parent"))
            .chain(directories.into_iter().map(Ok))
            .collect::<Result<Vec<_>, _>>()?;
        subscribe(&mut self.watcher, &mut self.parents, &requested)
    }

    #[cfg(test)]
    pub fn parents(&self) -> &BTreeSet<PathBuf> {
        &self.parents
    }

    /// Whether `generation` is the live subscription's; older events are stale.
    pub fn current(&self, generation: u8) -> bool {
        self.generation == generation
    }

    /// Replace a dropped subscription once. The caller watches its paths again.
    pub fn recover(&mut self) -> Result<(), String> {
        if self.generation != 0 {
            return Err("required file subscription dropped again".into());
        }
        self.generation = 1;
        self.watcher = watcher(self.handler.clone(), self.generation)?;
        self.parents.clear();
        Ok(())
    }
}

/// Directory watch registration, separate from `notify` so tests can change the filesystem
/// while a watch is being registered.
trait Subscriptions {
    fn watch(&mut self, path: &Path) -> notify::Result<()>;
    fn unwatch(&mut self, path: &Path) -> notify::Result<()>;
}

impl Subscriptions for RecommendedWatcher {
    fn watch(&mut self, path: &Path) -> notify::Result<()> {
        Watcher::watch(self, path, RecursiveMode::NonRecursive)
    }

    fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        Watcher::unwatch(self, path)
    }
}

/// Watch the nearest existing directory for each of `requested`, registering new watches
/// before releasing old ones. A missing directory can appear after its ancestor is resolved
/// but before that watch exists, so its creation event is lost: resolve again until the
/// watched set no longer changes.
fn subscribe(
    subscriptions: &mut impl Subscriptions,
    watched: &mut BTreeSet<PathBuf>,
    requested: &[&Path],
) -> Result<(), String> {
    loop {
        let parents = existing_parents(requested)?;
        if parents == *watched {
            return Ok(());
        }
        for path in parents.difference(watched) {
            subscriptions
                .watch(path)
                .map_err(|e| format!("required file subscription at {}: {e}", path.display()))?;
        }
        for path in watched.difference(&parents) {
            subscriptions
                .unwatch(path)
                .map_err(|e| format!("required file unsubscribe: {e}"))?;
        }
        *watched = parents;
    }
}

/// The nearest existing directory at or above each requested directory.
fn existing_parents(requested: &[&Path]) -> Result<BTreeSet<PathBuf>, String> {
    let mut parents = BTreeSet::new();
    for &requested in requested {
        let mut parent = requested;
        loop {
            match fs::metadata(parent) {
                Ok(metadata) if metadata.is_dir() => break,
                Ok(_) => {
                    return Err(format!(
                        "required watch parent is not a directory: {}",
                        parent.display()
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    parent = parent.parent().ok_or("no existing watch parent")?;
                }
                Err(error) => return Err(format!("{}: {error}", parent.display())),
            }
        }
        parents.insert(parent.to_path_buf());
    }
    Ok(parents)
}

/// Whether `event` may change one of `paths`: it names the file, or it creates, removes or
/// renames one of its ancestors. Access notifications never count.
pub(super) fn relevant<'a>(paths: impl IntoIterator<Item = &'a PathBuf>, event: &Event) -> bool {
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
    paths.into_iter().any(|target| {
        event
            .paths
            .iter()
            .any(|path| target == path || structural && target.starts_with(path))
    })
}

/// A watched file's bytes, or `None` when it is absent. Symlinks and non-files are errors.
pub(super) fn version(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("watched input must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    enum Call {
        Watch(PathBuf),
        Unwatch(PathBuf),
    }

    /// Records registrations; registering `race.0` creates directory `race.1`, as a
    /// concurrent writer can between resolving a parent and its watch taking effect.
    struct Racing {
        calls: Vec<Call>,
        race: Option<(PathBuf, PathBuf)>,
    }

    impl Subscriptions for Racing {
        fn watch(&mut self, path: &Path) -> notify::Result<()> {
            self.calls.push(Call::Watch(path.to_path_buf()));
            if let Some((_, created)) = self.race.take_if(|(at, _)| at == path) {
                fs::create_dir_all(created).unwrap();
            }
            Ok(())
        }

        fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
            self.calls.push(Call::Unwatch(path.to_path_buf()));
            Ok(())
        }
    }

    #[test]
    fn a_parent_created_while_its_ancestor_watch_registers_is_watched_before_returning() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().to_path_buf();
        let parent = root.join("sessions/pi");
        let mut subscriptions = Racing {
            calls: Vec::new(),
            race: Some((root.clone(), parent.clone())),
        };
        let mut watched = BTreeSet::new();

        subscribe(&mut subscriptions, &mut watched, &[&parent]).unwrap();

        assert_eq!(watched, BTreeSet::from([parent.clone()]));
        assert_eq!(
            subscriptions.calls,
            [
                Call::Watch(root.clone()),
                Call::Watch(parent),
                Call::Unwatch(root)
            ]
        );
    }
}
