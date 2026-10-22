//! Semantic repository fixture, not a Git implementation: immutable validated histories,
//! an observed remote tip, and scripted effect outcomes. No filesystem or subprocess I/O.
//! Fresh coordinators retain journal contents; this does not model disk crash durability.

mod adoption;
mod conformance;
mod policy;
mod recovery;

use super::*;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use crate::codec::record_to_data;
use crate::model::Stream;
use crate::operations::Actor;
use crate::storage::Journal;
use crate::sync::envelope::BOOTSTRAP;
use crate::yaml_store::{YamlStore, record_paths, relative_record_path};
use serde_json::{Map, Value, json};

#[derive(Clone, Copy, PartialEq)]
enum JournalFailure {
    BeforeAdoption,
    BeforeClear,
    AfterClear,
    AfterClearThenWrite,
}

#[derive(Default)]
struct MemoryJournal {
    objects: RefCell<BTreeMap<String, Map<String, Value>>>,
    failure: Cell<Option<JournalFailure>>,
}

impl Journal for MemoryJournal {
    fn read(&self, name: &str) -> Result<Option<Map<String, Value>>, StorageError> {
        Ok(self.objects.borrow().get(name).cloned())
    }
    fn write(&self, name: &str, data: Map<String, Value>) -> Result<(), StorageError> {
        if name == "publication.json"
            && data.get("phase") == Some(&json!("adopting"))
            && self.failure.get() == Some(JournalFailure::BeforeAdoption)
        {
            self.failure.set(None);
            return Err(StorageError::new("adoption marker write failed"));
        }
        self.objects.borrow_mut().insert(name.into(), data);
        Ok(())
    }
    fn clear(&self, name: &str) -> Result<(), StorageError> {
        let failure = if name == "publication.json" {
            self.failure.take()
        } else {
            None
        };
        if failure == Some(JournalFailure::BeforeClear) {
            return Err(StorageError::new("clear failed"));
        }
        self.objects.borrow_mut().remove(name);
        if failure == Some(JournalFailure::AfterClearThenWrite) {
            self.failure.set(Some(JournalFailure::BeforeAdoption));
        }
        if matches!(
            failure,
            Some(JournalFailure::AfterClear | JournalFailure::AfterClearThenWrite)
        ) {
            return Err(StorageError::uncertain("clear took effect but failed"));
        }
        Ok(())
    }
    fn names(&self) -> Result<Vec<String>, StorageError> {
        Ok(self.objects.borrow().keys().cloned().collect())
    }
}

#[derive(Default)]
struct Histories {
    values: BTreeMap<String, History>,
    parents: BTreeMap<String, Option<String>>,
}

impl Histories {
    fn append(&mut self, base: Option<&str>, record: Option<&Record>, message: &str) -> String {
        let history = base.map(|tip| self.values[tip].clone()).unwrap_or_default();
        let mut files = history.files().clone();
        if let Some(record) = record {
            // JSON is a YAML subset; use the production decoder and history validator.
            let bytes = serde_json::to_vec(&json!({"version": 1, "revision": "fixture",
                "kind": record.kind(), "record": record_to_data(record)}))
            .unwrap();
            let previous = history.catalog().get(&record.key()).and_then(|prior| {
                record_paths(&prior)
                    .into_iter()
                    .find(|path| files.contains_key(path.to_str().unwrap()))
            });
            let path = record_paths(record)
                .into_iter()
                .find(|path| files.contains_key(path.to_str().unwrap()))
                .unwrap_or_else(|| relative_record_path(record));
            if let Some(previous) = previous.filter(|previous| previous != &path) {
                files.remove(previous.to_str().unwrap());
            }
            files.insert(path.to_str().unwrap().into(), bytes);
        }
        let tip = format!("{:040x}", self.values.len() + 1);
        let parents: Vec<_> = base.into_iter().collect();
        let history = history.append(&tip, &parents, message, files).unwrap();
        self.parents.insert(tip.clone(), base.map(str::to_owned));
        self.values.insert(tip.clone(), history);
        tip
    }

    fn ancestor(&self, before: &str, after: &str) -> bool {
        let mut tip = Some(after);
        while let Some(current) = tip {
            if current == before {
                return true;
            }
            tip = self.parents[current].as_deref();
        }
        false
    }
}

#[derive(Clone)]
enum Push {
    Accept,
    Reject,
    RejectThenOffline,
    LoseAck { accepted: bool, offline: bool },
    Race(String),
}

type AfterPush = Box<dyn FnOnce(&MemoryRepository)>;
type BeforeRetain = Box<dyn FnOnce(&MemoryRepository) -> Result<(), CatalogError>>;

struct MemoryRepository {
    histories: Rc<RefCell<Histories>>,
    remote: Rc<RefCell<Option<String>>>,
    head: RefCell<String>,
    records: RefCell<Snapshot>,
    journal: MemoryJournal,
    retained: RefCell<Vec<String>>,
    pushes: RefCell<VecDeque<Push>>,
    push_count: Cell<usize>,
    candidate_count: Cell<usize>,
    fetch_unavailable: Cell<bool>,
    fetch_count: Cell<usize>,
    fetch_errors: Cell<usize>,
    candidate_failure: Cell<bool>,
    checkout_failure: Cell<bool>,
    commit_failure: Cell<bool>,
    state_hook: RefCell<Option<crate::sync::state::StateHook>>,
    after_push: RefCell<Option<AfterPush>>,
    before_retain: RefCell<Option<BeforeRetain>>,
    commit_before_failure: Cell<bool>,
    dirty: Cell<bool>,
    host: String,
    remote_enabled: bool,
    now: Cell<f64>,
    clock_script: RefCell<VecDeque<f64>>,
    clock_reads: Cell<usize>,
    policy: Policy,
}

impl MemoryRepository {
    fn new(
        histories: Rc<RefCell<Histories>>,
        remote: Rc<RefCell<Option<String>>>,
        host: &str,
        remote_enabled: bool,
    ) -> Self {
        let head = remote
            .borrow()
            .clone()
            .unwrap_or_else(|| histories.borrow_mut().append(None, None, BOOTSTRAP));
        let records = YamlStore::decode_snapshot(histories.borrow().values[&head].files()).unwrap();
        let repo = Self {
            histories,
            remote,
            head: RefCell::new(head),
            records: RefCell::new(records),
            journal: MemoryJournal::default(),
            retained: RefCell::default(),
            pushes: RefCell::default(),
            push_count: Cell::new(0),
            candidate_count: Cell::new(0),
            fetch_unavailable: Cell::new(false),
            fetch_count: Cell::new(0),
            fetch_errors: Cell::new(0),
            candidate_failure: Cell::new(false),
            checkout_failure: Cell::new(false),
            commit_failure: Cell::new(false),
            state_hook: RefCell::new(None),
            after_push: RefCell::new(None),
            before_retain: RefCell::new(None),
            commit_before_failure: Cell::new(false),
            dirty: Cell::new(false),
            host: host.into(),
            remote_enabled,
            now: Cell::new(1000.0),
            clock_script: RefCell::default(),
            clock_reads: Cell::new(0),
            policy: Policy::default(),
        };
        State::from_journal(&repo.journal)
            .write_config(
                &GitConfig::new(remote_enabled.then(|| "memory:origin".into()), "main", host)
                    .unwrap(),
            )
            .unwrap();
        if remote_enabled {
            repo.run(|core| core.fetch()).unwrap();
        }
        repo
    }

    fn run<T>(&self, f: impl FnOnce(&Coordinator<'_>) -> T) -> T {
        let state =
            State::from_journal(&self.journal).with_optional_hook(self.state_hook.borrow().clone());
        let clock = || {
            self.clock_reads.set(self.clock_reads.get() + 1);
            self.clock_script
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| self.now.get())
        };
        f(&Coordinator {
            io: self,
            state: &state,
            host_id: &self.host,
            policy: &self.policy,
            clock: &clock,
        })
    }
    fn save(&self, operation: &Operation) -> SyncResult {
        self.run(|core| core.save(operation, None).unwrap())
    }
    fn sync(&self) -> SyncResult {
        self.run(|core| core.sync(SyncEvent::Explicit))
    }
    fn snapshot(&self) -> Snapshot {
        self.run(|core| {
            core.ready().unwrap();
            core.saved().unwrap().1
        })
    }
    fn marker(&self) -> Option<PublicationMarker> {
        self.run(|core| core.state.publication().unwrap())
    }
    fn metadata(&self) -> SyncMetadata {
        self.run(|core| core.state.metadata().unwrap())
    }
}

impl Repository for MemoryRepository {
    fn check_layout(&self) -> Result<(), CatalogError> {
        Ok(())
    }
    fn check_target(&self, config: &GitConfig) -> Result<(), RepairRequired> {
        if config.branch() != "main"
            || config.remote() != self.remote_enabled.then_some("memory:origin")
        {
            return Err(repair("configured target changed"));
        }
        Ok(())
    }
    fn clean(&self) -> Result<(), CatalogError> {
        if self.dirty.get() {
            return Err(repair("working-tree change").into());
        }
        Ok(())
    }
    fn head(&self) -> Result<String, CatalogError> {
        Ok(self.head.borrow().clone())
    }
    fn history(&self, tip: Option<&str>) -> Result<History, CatalogError> {
        Ok(tip
            .map(|tip| self.histories.borrow().values[tip].clone())
            .unwrap_or_default())
    }
    fn ancestor(&self, before: &str, after: &str) -> Result<bool, CatalogError> {
        Ok(self.histories.borrow().ancestor(before, after))
    }
    fn snapshot(&self) -> Result<Snapshot, CatalogError> {
        Ok(self.records.borrow().clone())
    }
    fn fetch(&self, _: &str) -> Result<Option<String>, CatalogError> {
        self.fetch_count.set(self.fetch_count.get() + 1);
        if self.fetch_unavailable.get() || self.fetch_errors.get() > 0 {
            self.fetch_errors
                .set(self.fetch_errors.get().saturating_sub(1));
            return Err(GitError::new("confirmation unavailable", None).into());
        }
        Ok(self.remote.borrow().clone())
    }
    fn candidate(
        &self,
        local: &History,
        remote: &History,
        changes: Vec<ReplayChange>,
    ) -> Result<String, CatalogError> {
        self.candidate_count.set(self.candidate_count.get() + 1);
        if self.candidate_failure.get() {
            return Err(GitError::new("candidate unavailable", Some(1)).into());
        }
        let mut base = remote.tip().or(local.root()).unwrap().to_owned();
        for change in changes {
            base = self.histories.borrow_mut().append(
                Some(&base),
                Some(&change.record),
                &encode_operation(&change.operation, Some(&change.origin))?,
            );
        }
        Ok(base)
    }
    fn retain(&self, revision: &str) -> Result<(), CatalogError> {
        let callback = self.before_retain.borrow_mut().take();
        if let Some(callback) = callback {
            callback(self)?;
        }
        self.retained.borrow_mut().push(revision.into());
        Ok(())
    }
    fn push(&self, candidate: &Oid, _: &str) -> Result<(), CatalogError> {
        // The coordinator must fence and retain the exact attempt before dispatch.
        let marker = self.marker().unwrap();
        assert_eq!(marker.attempt().candidate(), candidate);
        assert!(
            self.retained
                .borrow()
                .iter()
                .any(|tip| tip == candidate.as_str())
        );
        self.push_count.set(self.push_count.get() + 1);
        let outcome = self.pushes.borrow_mut().pop_front().unwrap_or(Push::Accept);
        let result = match outcome {
            Push::Accept => {
                *self.remote.borrow_mut() = Some(candidate.to_string());
                Ok(())
            }
            Push::Reject => Err(GitError::new("push rejected", Some(1)).into()),
            Push::RejectThenOffline => {
                self.fetch_unavailable.set(true);
                Err(GitError::new("push rejected", Some(1)).into())
            }
            Push::LoseAck { accepted, offline } => {
                if accepted {
                    *self.remote.borrow_mut() = Some(candidate.to_string());
                }
                self.fetch_unavailable.set(offline);
                Err(GitError::new("lost push acknowledgement", None).into())
            }
            Push::Race(tip) => {
                *self.remote.borrow_mut() = Some(tip);
                Err(GitError::new("competing publisher", Some(1)).into())
            }
        };
        let callback = self.after_push.borrow_mut().take();
        if let Some(callback) = callback {
            callback(self);
        }
        result
    }
    fn checkout(&self, old: &Oid, new: &Oid) -> Result<(), CatalogError> {
        let PublicationMarker::Adopting(marker) = self.marker().unwrap() else {
            panic!("checkout must be fenced");
        };
        assert_eq!(marker.adoption_from(), old);
        assert_eq!(marker.adoption_to(), new);
        assert!(self.retained.borrow().iter().any(|tip| tip == old.as_str()));
        assert!(self.retained.borrow().iter().any(|tip| tip == new.as_str()));
        if self.checkout_failure.get() {
            return Err(GitError::new("checkout failed", Some(1)).into());
        }
        assert_eq!(self.head.borrow().as_str(), old.as_str());
        *self.records.borrow_mut() =
            YamlStore::decode_snapshot(self.history(Some(new.as_str()))?.files())?;
        *self.head.borrow_mut() = new.to_string();
        Ok(())
    }
    fn commit_record(
        &self,
        record: &Record,
        expected: &Snapshot,
        start: Option<&str>,
        message: &str,
    ) -> Result<String, CatalogError> {
        assert!(
            State::from_journal(&self.journal)
                .read("save.json")?
                .is_some()
        );
        assert_eq!(&*self.records.borrow(), expected);
        assert_eq!(start, Some(self.head.borrow().as_str()));
        if self.commit_before_failure.get() {
            return Err(GitError::new("save failure", Some(1)).into());
        }
        let commit = self
            .histories
            .borrow_mut()
            .append(start, Some(record), message);
        *self.head.borrow_mut() = commit.clone();
        *self.records.borrow_mut() =
            YamlStore::decode_snapshot(self.history(Some(&commit))?.files())?;
        if self.commit_failure.get() {
            return Err(StorageError::uncertain("commit took effect but failed").into());
        }
        Ok(commit)
    }
}

fn pair() -> (MemoryRepository, MemoryRepository) {
    let histories = Rc::default();
    let remote = Rc::default();
    let a = MemoryRepository::new(Rc::clone(&histories), Rc::clone(&remote), "a", true);
    confirmed(a.sync());
    let b = MemoryRepository::new(histories, remote, "b", true);
    (a, b)
}

fn create(id: &str, stream: &str, host: &str) -> Operation {
    Operation::new(
        id,
        None,
        Record::Stream(Stream::new(stream, stream, None, host).unwrap()),
        Actor::User,
        None,
        vec![],
    )
    .unwrap()
}
fn accepted(result: SyncResult) {
    assert!(result.accepted(), "{result:?}");
}
fn confirmed(result: SyncResult) {
    assert_eq!(result.sharing, SharingStatus::Confirmed, "{result:?}");
    assert!(!result.requires_repair(), "{result:?}");
}

fn assert_lost_ack_recovery(
    accepted_remote: bool,
    competing_remote: bool,
    interrupted_adoption: bool,
    ids: &[u8],
) {
    let (a, b) = pair();
    accepted(a.save(&create("initial", "initial", "a")));
    a.pushes.borrow_mut().push_back(Push::LoseAck {
        accepted: accepted_remote,
        offline: true,
    });
    assert_eq!(a.sync().sharing, SharingStatus::Uncertain);
    a.fetch_unavailable.set(false);
    let mut expected = BTreeSet::new();
    for id in ids {
        let id = format!("stream-{id}");
        let result = a.save(&create(&id, &id, "a"));
        accepted(result.clone());
        assert_eq!(
            result.local,
            if expected.insert(id) {
                LocalStatus::Committed
            } else {
                LocalStatus::Unchanged
            }
        );
    }
    let id = expected.iter().next().unwrap();
    let changed = Operation::new(
        id,
        None,
        Record::Stream(Stream::new(id, "Changed intent", None, "a").unwrap()),
        Actor::User,
        None,
        vec![],
    )
    .unwrap();
    let snapshot = a.snapshot();
    let head = a.head().unwrap();
    let receipts = a.run(|core| core.state.fingerprints().unwrap());
    let marker = a.marker();
    let retained = a.retained.borrow().clone();
    assert!(a.save(&changed).requires_repair());
    assert_eq!(a.snapshot(), snapshot);
    assert_eq!(a.head().unwrap(), head);
    assert_eq!(a.run(|core| core.state.fingerprints().unwrap()), receipts);
    assert_eq!(a.marker(), marker);
    assert_eq!(*a.retained.borrow(), retained);
    accepted(a.save(&create(id, id, "a")));
    if competing_remote {
        accepted(b.save(&create("competing", "competing", "b")));
        confirmed(b.sync());
    }
    a.checkout_failure.set(interrupted_adoption);
    let result = a.sync();
    if interrupted_adoption {
        assert_eq!(result.sharing, SharingStatus::Confirmed);
        assert!(result.requires_repair());
        assert!(a.run(|core| core.ready()).is_err());
        assert!(a.run(|core| core.sync(SyncEvent::Force)).requires_repair());
        assert!(matches!(a.marker(), Some(PublicationMarker::Adopting(_))));
    } else {
        confirmed(result);
        assert!(a.marker().is_none());
    }
    confirmed(b.sync());
    assert_eq!(
        b.snapshot().catalog.streams().len(),
        expected.len() + 1 + usize::from(competing_remote)
    );
}

macro_rules! lost_ack_mode {
    ($name:ident, $accepted:expr, $competing:expr, $interrupted:expr) => {
        #[test]
        fn $name() {
            assert_lost_ack_recovery($accepted, $competing, $interrupted, &[0, 1, 0, 2]);
        }
    };
}
lost_ack_mode!(lost_ack_unaccepted_alone_completes, false, false, false);
lost_ack_mode!(
    lost_ack_unaccepted_alone_fences_interrupted_adoption,
    false,
    false,
    true
);
lost_ack_mode!(lost_ack_unaccepted_competing_completes, false, true, false);
lost_ack_mode!(
    lost_ack_unaccepted_competing_fences_interrupted_adoption,
    false,
    true,
    true
);
lost_ack_mode!(lost_ack_accepted_alone_completes, true, false, false);
lost_ack_mode!(
    lost_ack_accepted_alone_fences_interrupted_adoption,
    true,
    false,
    true
);
lost_ack_mode!(lost_ack_accepted_competing_completes, true, true, false);
lost_ack_mode!(
    lost_ack_accepted_competing_fences_interrupted_adoption,
    true,
    true,
    true
);

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config { cases: 32, failure_persistence: None, ..Default::default() })]
    #[test]
    fn generated_retries_preserve_every_save(
        ids in proptest::collection::vec(0u8..16, 1..13),
        accepted in proptest::bool::ANY,
        competing in proptest::bool::ANY,
        interrupted in proptest::bool::ANY,
    ) {
        let mut sequence = vec![0, 1, 0, 2];
        sequence.extend(ids);
        assert_lost_ack_recovery(accepted, competing, interrupted, &sequence);
    }
}

#[test]
fn adoption_journal_faults_preserve_confirmation_and_reassert_fence() {
    for stage in [
        JournalFailure::BeforeAdoption,
        JournalFailure::BeforeClear,
        JournalFailure::AfterClear,
    ] {
        let (a, b) = pair();
        accepted(a.save(&create("remote", "remote", "a")));
        confirmed(a.sync());
        accepted(b.save(&create("local", "local", "b")));
        b.now.set(2000.0);
        let old = b.head().unwrap();
        let graphs = b.retained.borrow().len();
        b.journal.failure.set(Some(stage));
        let result = b.sync();
        assert_eq!(result.sharing, SharingStatus::Confirmed, "{result:?}");
        assert_eq!(result.local, LocalStatus::RepairRequired);
        assert!(
            b.journal.failure.get().is_none(),
            "fault must have been exercised"
        );
        let marker = b.marker().unwrap();
        assert!(matches!(marker, PublicationMarker::Adopting(_)));
        assert_eq!(
            marker.attempt().confirmed_at(),
            Some(Seconds::try_from(2000.0).unwrap())
        );
        assert_eq!(b.metadata().last_push, marker.attempt().confirmed_at());
        assert_eq!(
            b.head().unwrap(),
            if stage == JournalFailure::BeforeAdoption {
                old.clone()
            } else {
                marker.attempt().candidate().to_string()
            }
        );
        let refs = b.retained.borrow().clone();
        assert_eq!(refs.len(), graphs + 3);
        assert!(refs.contains(&old));
        assert!(
            refs.iter()
                .any(|tip| tip == marker.attempt().candidate().as_str())
        );
        for tip in refs {
            b.history(Some(&tip)).unwrap();
        }
        b.now.set(3000.0);
        assert!(b.run(|core| core.snapshot()).is_err());
        let retry = b.run(|core| core.sync(SyncEvent::Force));
        assert_eq!(retry.sharing, SharingStatus::Blocked);
        assert!(retry.requires_repair());
        assert_eq!(
            b.metadata().last_push,
            Some(Seconds::try_from(2000.0).unwrap())
        );
        assert_eq!(b.marker(), Some(marker));
    }
}

#[test]
fn uncertain_save_effect_leaves_no_receipt_and_fences_fresh_coordinator() {
    let (a, _) = pair();
    let op = create("new", "new", "a");
    a.commit_failure.set(true);
    assert!(a.save(&op).requires_repair());
    a.run(|core| {
        assert!(core.state.read("save.json").unwrap().is_some());
        assert!(!core.state.seen(&op).unwrap());
        assert!(core.snapshot().is_err());
    });
    assert!(
        a.records.borrow().catalog.stream("new").is_some(),
        "failure was after the save effect"
    );
    assert!(
        a.save(&op).requires_repair(),
        "fresh state wrapper must not blindly retry"
    );
}
