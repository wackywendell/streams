//! File events select rereads of admitted named records, never compute membership.

use super::{
    super::{CliError, Result, application},
    Wake,
};
use notify::Event;
use std::{collections::BTreeMap, path::PathBuf, sync::mpsc::Sender};
use streams::{
    file_store::records::relative_record_path,
    model::{RecordKind, Session},
    storage::records::RecordAddress,
};

use crate::cli::file_watch::{self, FileWatch};

type Versions = BTreeMap<&'static str, std::result::Result<Option<Vec<u8>>, String>>;
pub(super) struct Files {
    watch: FileWatch,
    groups: BTreeMap<&'static str, PathBuf>,
    versions: Versions,
    root: PathBuf,
    host: String,
    streams: PathBuf,
}
impl Files {
    pub fn new(
        records: &application::OpenedRecords,
        native_session: &str,
        sender: Sender<Wake>,
    ) -> Result<Self> {
        let host = records.host().as_str();
        let address = RecordAddress::new(
            host,
            RecordKind::Session,
            Session::id_for(host, "pi", native_session),
        )
        .map_err(application::ApplicationError::from)?;
        let root = std::fs::canonicalize(records.root())?;
        let session =
            root.join(relative_record_path(&address).map_err(application::ApplicationError::from)?);
        let streams = session
            .parent()
            .expect("conversation directory")
            .parent()
            .expect("host partition")
            .join("streams");
        let watch = FileWatch::new(move |generation, event| {
            let _ = sender.send(Wake::Files(generation, event));
        })
        .map_err(CliError::message)?;
        let mut files = Self {
            watch,
            groups: BTreeMap::from([("session", session)]),
            versions: BTreeMap::new(),
            root,
            host: host.into(),
            streams,
        };
        files.prepare()?;
        Ok(files)
    }

    /// The collector supplies the current key. Do not baseline a newly selected file
    /// after observation: an already-queued write must still cause its next reread.
    pub fn set_stream(&mut self, stream: Option<&str>) -> Result<()> {
        let path =
            if let Some(stream) = stream {
                let address = RecordAddress::new(&self.host, RecordKind::Stream, stream)
                    .map_err(application::ApplicationError::from)?;
                Some(self.root.join(
                    relative_record_path(&address).map_err(application::ApplicationError::from)?,
                ))
            } else {
                None
            };
        self.set_path("stream", path)
    }

    fn set_path(&mut self, group: &'static str, path: Option<PathBuf>) -> Result<()> {
        if self.groups.get(group) == path.as_ref() {
            return Ok(());
        }
        match path {
            Some(path) => {
                self.groups.insert(group, path);
            }
            None => {
                self.groups.remove(group);
            }
        }
        self.versions.remove(group);
        self.watch_parents()
    }

    /// Register before rechecking, then baseline before collecting domain records.
    pub fn prepare(&mut self) -> Result<bool> {
        self.watch_parents()?;
        let versions = self
            .groups
            .iter()
            .map(|(key, path)| (*key, file_watch::version(path)))
            .collect();
        let changed = self.versions != versions;
        self.versions = versions;
        Ok(changed)
    }

    fn watch_parents(&mut self) -> Result<()> {
        self.watch
            .watch(
                self.groups.values().map(PathBuf::as_path),
                [self.streams.as_path()],
            )
            .map_err(CliError::message)
    }

    /// Missing ancestor creation or deletion only repairs this narrow watch;
    /// unchanged logical file bytes still produce no observation.
    pub fn relevant(&self, event: &Event) -> bool {
        file_watch::relevant(self.groups.values(), event)
    }

    pub fn current(&self, generation: u8) -> bool {
        self.watch.current(generation)
    }

    pub fn recover(&mut self) -> Result<()> {
        self.watch.recover().map_err(CliError::message)?;
        self.watch_parents()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::{
        EventKind,
        event::{CreateKind, ModifyKind, RenameMode},
    };
    use std::{fs, sync::mpsc};
    use streams::{
        config::{Config, Strategy},
        model::{Record, Stream},
        storage::{
            StorageError,
            records::{RecordChange, RecordStore},
        },
    };

    fn records() -> (tempfile::TempDir, application::OpenedRecords) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let env = BTreeMap::from([("HOME".into(), root.to_str().unwrap().to_owned())]);
        let config = Config::new(&env, &root).unwrap();
        let records =
            application::initialize_records(&config, Strategy::None, None, "main").unwrap();
        (directory, records)
    }
    fn save(records: &application::OpenedRecords, record: Record) {
        records
            .records()
            .save::<_, StorageError>(&RecordAddress::for_record(&record), |_, _| {
                Ok(RecordChange {
                    record: record.clone(),
                    outcome: (),
                })
            })
            .unwrap();
    }

    #[test]
    fn exact_paths_filter_before_reads_and_atomic_echoes_coalesce() {
        let (_directory, records) = records();
        let (sender, _) = mpsc::channel();
        let mut files = Files::new(&records, "pi", sender).unwrap();
        let path = files.groups["session"].clone();
        let host = records.host().as_str();
        assert_eq!(
            path,
            records.root().join(
                relative_record_path(
                    &RecordAddress::new(
                        host,
                        RecordKind::Session,
                        Session::id_for(host, "pi", "pi")
                    )
                    .unwrap()
                )
                .unwrap()
            )
        );
        assert_eq!(
            files.groups.len(),
            1,
            "no legacy candidates, environment hint or claim watches"
        );
        let unrelated = Event::new(EventKind::Create(CreateKind::File))
            .add_path(path.with_file_name("unrelated.yaml"));
        assert!(!files.relevant(&unrelated));
        let mut observations = 0;
        for _ in 0..32 {
            if files.relevant(&unrelated) && files.prepare().unwrap() {
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
            if files.relevant(&rename) && files.prepare().unwrap() {
                observations += 1;
            }
        }
        assert_eq!(
            observations, 1,
            "duplicate save notifications must coalesce"
        );
        files.recover().unwrap();
        assert!(!files.current(0));
        assert!(files.current(1));
        assert!(files.recover().is_err(), "a second actual drop is fatal");
    }

    #[test]
    fn observed_keys_retarget_watches_without_computing_membership_or_hiding_a_queued_write() {
        let (_directory, records) = records();
        let host = records.host().as_str();
        for id in ["old", "next"] {
            save(
                &records,
                Record::Stream(Stream::new(id, id, None, host).unwrap()),
            );
        }
        let session = |stream: &str| {
            Record::Session(
                Session::new(
                    Session::id_for(host, "pi", "native"),
                    host,
                    stream,
                    "pi",
                    "native",
                    "Lane",
                )
                .unwrap(),
            )
        };
        save(&records, session("old"));
        let (sender, _) = mpsc::channel();
        let mut files = Files::new(&records, "native", sender).unwrap();
        assert!(
            files.watch.parents().contains(&files.streams),
            "watch all possible addressed Stream events before context collection"
        );
        files.set_stream(Some("old")).unwrap();
        files.prepare().unwrap();
        let old_path = files.groups["stream"].clone();
        save(&records, session("next"));
        assert!(files.prepare().unwrap());
        assert_eq!(
            files.groups["stream"], old_path,
            "raw Session bytes cannot compute desired membership"
        );
        let changed = Stream::new(
            "next",
            "next",
            Some("changed after context collection".into()),
            host,
        )
        .unwrap();
        save(&records, Record::Stream(changed));
        files.set_stream(Some("next")).unwrap();
        let next_path = files.groups["stream"].clone();
        assert_ne!(next_path, old_path);
        assert!(
            !files.versions.contains_key("stream"),
            "retarget must not baseline over the racing write"
        );
        assert!(
            files.relevant(
                &Event::new(EventKind::Modify(ModifyKind::Any)).add_path(next_path.clone())
            )
        );
        assert!(
            !files.relevant(&Event::new(EventKind::Modify(ModifyKind::Any)).add_path(old_path))
        );
        assert!(files.prepare().unwrap());
        assert_eq!(
            files.versions["stream"],
            Ok(Some(fs::read(next_path).unwrap()))
        );
        files.set_stream(None).unwrap();
        assert!(!files.groups.contains_key("stream"));
    }
}
