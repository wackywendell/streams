//! Integration reads and actions stay within one already-admitted catalog and owner host.

use super::{OpenedRecords, Result};
use crate::{
    integration::{
        ConversationId, LinkKind, ReadReply, Reference, ResolvedTarget, SessionView, WorkScope,
    },
    model::{Record, RecordKind, Session, Status, Stream, ValidationError},
    operations::{decay_cutoff_ms, project_stream_status, stream_activity_ms},
    storage::records::{RecordAddress, RecordRead, RecordReader, RecordStore, UnreadableRecord},
};

pub struct Streams<'a> {
    records: &'a OpenedRecords,
    scope: WorkScope,
    lifecycle_cutoff: Option<u64>,
}

impl<'a> Streams<'a> {
    pub fn new(records: &'a OpenedRecords) -> Self {
        Self {
            records,
            scope: WorkScope {
                catalog: records.source_id().into(),
                host: records.host().as_str().into(),
            },
            lifecycle_cutoff: None,
        }
    }
    pub fn with_lifecycle(mut self, decay_days: u32, now_ms: u64) -> Self {
        self.lifecycle_cutoff = decay_cutoff_ms(decay_days, now_ms);
        self
    }
    /// A supplied scope is an assertion, never a selector for another catalog or host.
    pub fn admit(records: &'a OpenedRecords, expected: &WorkScope) -> Result<Self> {
        expected.validate()?;
        let application = Self::new(records);
        if &application.scope != expected {
            return Err(ValidationError::new(
                "integration scope does not match the configured catalog and host",
            )
            .into());
        }
        Ok(application)
    }
    pub fn scope(&self) -> &WorkScope {
        &self.scope
    }

    pub fn require_stream(&self, id: &str) -> Result<Stream> {
        self.records
            .records()
            .with_reader(|reader| self.stream(reader, id))
    }

    /// Select current local work before native observation or effects.
    pub fn select_open(
        &self,
        stream_id: &str,
        selection: &crate::integration::SessionSelection,
    ) -> Result<(Stream, Vec<Session>)> {
        use crate::integration::SessionSelection;
        let stream = self.require_stream(stream_id)?;
        let sessions = match selection {
            SessionSelection::AllEligible => {
                let mut sessions: Vec<_> = self
                    .records
                    .host_sessions()?
                    .complete()?
                    .into_iter()
                    .filter_map(|value| {
                        let Record::Session(session) = value.record else {
                            unreachable!("Session partition")
                        };
                        (session.stream_id == stream.id && session.status.is_open_eligible())
                            .then_some(session)
                    })
                    .collect();
                sessions.sort_by(|left, right| left.id.cmp(&right.id));
                sessions
            }
            SessionSelection::Only { value } => {
                self.records
                    .records()
                    .with_reader(|reader| -> Result<Vec<Session>> {
                        let mut seen = std::collections::BTreeSet::new();
                        value
                            .as_slice()
                            .iter()
                            .map(|id| {
                                if !seen.insert(id) {
                                    return Err(
                                        ValidationError::new("duplicate selected Session").into()
                                    );
                                }
                                let session = self.session(reader, id)?.ok_or_else(|| {
                                    ValidationError::new("selected Session is unavailable")
                                })?;
                                if session.stream_id != stream.id {
                                    return Err(ValidationError::new(
                                        "selected Session is not a current Stream member",
                                    )
                                    .into());
                                }
                                Ok(session)
                            })
                            .collect()
                    })?
            }
        };
        Ok((stream, sessions))
    }

    pub fn companion_settings(
        &self,
        config: &crate::config::Config,
        conversation: &ConversationId,
    ) -> Result<crate::runtime::card_settings::CardSettings> {
        Ok(crate::runtime::card_settings::CardSettings::new(
            config,
            self.records.root(),
            &self.scope.host,
            conversation,
        )?)
    }

    pub fn read_session(&self, session: &str) -> Result<ReadReply<SessionView>> {
        self.records.records().with_reader(|reader| {
            let session = self.session(reader, session)?.ok_or_else(|| {
                crate::runtime::RuntimeError::NotFound(
                    "saved Session is unavailable on this host".into(),
                )
            })?;
            Ok(ReadReply {
                view: SessionView::from(&session),
                viewer_actions: Vec::new(),
                warnings: Vec::new(),
            })
        })
    }

    pub fn read_stream(&self, stream: &str) -> Result<ReadReply<crate::integration::StreamView>> {
        use crate::artifacts::ArtifactStore;
        let sessions = self.records.host_sessions()?;
        let members = sessions_by_id(sessions.records);
        self.records.artifacts().with_artifacts(|reader| {
            Ok(ReadReply {
                view: self.stream_view(reader, stream, &members, &sessions.unreadable)?,
                viewer_actions: vec![],
                warnings: vec![],
            })
        })
    }

    /// A batch shares one host inspection and one producer listing per distinct Stream.
    pub fn read_saved_views(
        &self,
        selections: &[crate::integration::ViewSelection],
    ) -> Result<Vec<crate::integration::WireResult<crate::integration::ModReadValue>>> {
        use crate::{
            artifacts::{ArtifactReader, ArtifactStore},
            integration::{ModReadValue, ViewSelection, WireError, WireResult},
        };
        use std::collections::{BTreeMap, BTreeSet};
        let streams: BTreeSet<_> = selections
            .iter()
            .filter_map(|selection| match selection {
                ViewSelection::SavedStream { stream } => Some(stream.as_str()),
                _ => None,
            })
            .collect();
        let needs_host_streams = selections
            .iter()
            .any(|selection| matches!(selection, ViewSelection::HostStreams));
        // A failed read is reported with its own error, never as an empty or generic view.
        let unavailable =
            |error: super::ApplicationError| WireError::unavailable(error.to_string());
        let sessions = if streams.is_empty() && !needs_host_streams {
            None
        } else {
            Some(
                self.records
                    .host_sessions()
                    .map(|listing| (sessions_by_id(listing.records), listing.unreadable))
                    .map_err(unavailable),
            )
        };
        let host_streams_result = match &sessions {
            Some(Ok((sessions, unreadable))) if needs_host_streams => Some(
                self.host_listing(sessions, unreadable.clone())
                    .map_err(unavailable),
            ),
            Some(Err(error)) if needs_host_streams => Some(Err(error.clone())),
            _ => None,
        };
        let read = |reader: &dyn RecordReader,
                    artifacts: Option<&dyn ArtifactReader>|
         -> Result<_> {
            let mut stream_views = BTreeMap::new();
            for stream in &streams {
                let result = match (&sessions, artifacts) {
                    (Some(Ok((sessions, unreadable))), Some(artifacts)) => self
                        .stream_view(artifacts, stream, sessions, unreadable)
                        .map(|value| WireResult::Ok(ModReadValue::Stream { value }))
                        .unwrap_or_else(|error| WireResult::Error(unavailable(error))),
                    (Some(Err(error)), _) => WireResult::Error(error.clone()),
                    _ => WireResult::Error(WireError::unavailable(
                        "saved Stream view is unavailable",
                    )),
                };
                stream_views.insert(*stream, result);
            }
            let mut session_views = BTreeMap::new();
            Ok(selections
                .iter()
                .map(|selection| match selection {
                    ViewSelection::SavedStream { stream } => stream_views[stream.as_str()].clone(),
                    ViewSelection::SavedSession { session } => session_views
                        .entry(session.as_str())
                        .or_insert_with(|| {
                            let value = match &sessions {
                                Some(Ok((sessions, unreadable)))
                                    if unreadable.is_empty() || sessions.contains_key(session) =>
                                {
                                    Ok(sessions.get(session).cloned())
                                }
                                // An addressed read stays independent of unrelated Sessions and
                                // names this Session's own decode failure.
                                _ => self.session(reader, session).map_err(unavailable),
                            };
                            match value {
                                Ok(Some(session)) => WireResult::Ok(ModReadValue::Session {
                                    value: SessionView::from(&session),
                                }),
                                Ok(None) => WireResult::Error(WireError::unavailable(
                                    "saved Session view is unavailable",
                                )),
                                Err(error) => WireResult::Error(error),
                            }
                        })
                        .clone(),
                    ViewSelection::HostStreams => match &host_streams_result {
                        Some(Ok(value)) => WireResult::Ok(ModReadValue::HostStreams {
                            value: value.clone(),
                        }),
                        Some(Err(error)) => WireResult::Error(error.clone()),
                        None => unreachable!("HostStreams selections read host Streams"),
                    },
                    _ => WireResult::Error(WireError::unavailable(
                        "native integration is not enabled",
                    )),
                })
                .collect())
        };
        if sessions.is_some() {
            self.records
                .artifacts()
                .with_artifacts(|reader| read(reader, Some(reader)))
        } else {
            self.records
                .records()
                .with_reader(|reader| read(reader, None))
        }
    }

    fn projected_status(
        &self,
        stream_status: Status,
        sessions: &[&Session],
        last_updated_ms: u64,
    ) -> Status {
        project_stream_status(
            stream_status,
            sessions,
            last_updated_ms,
            self.lifecycle_cutoff,
        )
    }

    /// A row's counts, status and recency reflect its readable Sessions only. An unreadable
    /// Session's Stream is unknown, so it is named among the listing's unreadable records and
    /// counted nowhere.
    fn host_listing(
        &self,
        sessions: &std::collections::BTreeMap<String, Session>,
        mut unreadable: Vec<UnreadableRecord>,
    ) -> Result<crate::integration::HostListing> {
        use crate::integration::{HostListing, StreamSummary};
        let stream_listing = self.records.host_streams()?;
        unreadable.extend(stream_listing.unreadable);
        unreadable.sort_by(|left, right| left.path.cmp(&right.path));
        let stream_reads = stream_listing.records;
        let mut counts: std::collections::BTreeMap<String, (usize, usize)> =
            std::collections::BTreeMap::new();
        let mut by_stream: std::collections::BTreeMap<&str, Vec<&Session>> =
            std::collections::BTreeMap::new();
        for session in sessions.values() {
            let entry = counts.entry(session.stream_id.clone()).or_insert((0, 0));
            if session.status == crate::model::Status::Active {
                entry.0 += 1;
            }
            entry.1 += 1;
            by_stream
                .entry(session.stream_id.as_str())
                .or_default()
                .push(session);
        }
        let mut summaries = Vec::with_capacity(stream_reads.len());
        for read in stream_reads {
            let Record::Stream(stream) = read.record else {
                unreachable!("Stream partition")
            };
            let (active_sessions, total_sessions) =
                counts.get(&stream.id).copied().unwrap_or((0, 0));
            let members = by_stream
                .get(stream.id.as_str())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let last_updated_ms = stream_activity_ms(&stream, members);
            let status = self.projected_status(stream.status, members, last_updated_ms);
            summaries.push(StreamSummary {
                id: stream.id,
                title: stream.title,
                goal_short: stream.goal_short,
                status,
                active_sessions,
                total_sessions,
                last_updated_ms,
            });
        }
        summaries.sort_by_cached_key(|summary| {
            (
                std::cmp::Reverse(summary.last_updated_ms),
                icu_casemap::CaseMapper::new()
                    .fold_string(&summary.title)
                    .into_owned(),
                summary.id.clone(),
            )
        });
        Ok(HostListing {
            streams: summaries,
            unreadable,
        })
    }

    /// Resolve native selectors first so saved and contextual views share the same read batch.
    pub fn read_views(
        &self,
        selections: &[crate::integration::ViewSelection],
        terminals: crate::integration::Access<&mut dyn crate::integration::TerminalObservation>,
    ) -> Result<Vec<crate::integration::WireResult<crate::integration::ModReadValue>>> {
        use crate::integration::{
            Access, CompanionVisibility, FrameContext, ModReadValue, TabContext, ViewSelection,
            WireError, WireResult,
        };
        let mut terminals = terminals;
        let mut enrolled = std::collections::HashMap::new();
        let mut expanded = selections.to_vec();
        let mut frames = std::collections::BTreeMap::new();
        let mut tab_errors = std::collections::BTreeMap::new();
        for (index, selection) in selections.iter().enumerate() {
            match (selection, &mut terminals) {
                (ViewSelection::Frame { terminal }, Access::Available { value: native }) => {
                    let context = match self.enrolled_session(&mut enrolled, *native, terminal) {
                        Enrollment::Enrolled(session) => FrameContext::Enrolled {
                            session: Box::new(SessionView::from(&*session)),
                            companion: CompanionVisibility::Shown,
                        },
                        Enrollment::Unenrolled => FrameContext::Unenrolled,
                        Enrollment::NotApplicable => FrameContext::NotApplicable,
                        Enrollment::Unavailable(reason) => FrameContext::Unavailable { reason },
                    };
                    frames.insert(index, context);
                }
                (ViewSelection::Tab { terminals: tab }, Access::Available { value: native }) => {
                    match self.tab_stream(&mut enrolled, *native, tab) {
                        Ok(Some(stream)) => expanded[index] = ViewSelection::SavedStream { stream },
                        Ok(None) => expanded[index] = ViewSelection::HostStreams,
                        Err(reason) => {
                            tab_errors.insert(index, WireError::unavailable(reason));
                        }
                    }
                }
                (ViewSelection::Frame { .. }, _) => {
                    frames.insert(
                        index,
                        FrameContext::Unavailable {
                            reason: "frame context is unavailable".into(),
                        },
                    );
                }
                (ViewSelection::Tab { .. }, _) => {
                    tab_errors.insert(index, WireError::unavailable("tab context is unavailable"));
                }
                _ => {}
            }
        }
        Ok(self
            .read_saved_views(&expanded)?
            .into_iter()
            .enumerate()
            .map(|(index, read)| match &selections[index] {
                ViewSelection::Frame { .. } => WireResult::Ok(ModReadValue::Frame {
                    context: frames.remove(&index).expect("every frame is resolved"),
                }),
                ViewSelection::Tab { .. } => {
                    if let Some(error) = tab_errors.remove(&index) {
                        return WireResult::Error(error);
                    }
                    match read {
                        WireResult::Ok(ModReadValue::Stream { value }) => {
                            WireResult::Ok(ModReadValue::Tab {
                                context: TabContext::Stream {
                                    stream: Box::new(value),
                                },
                            })
                        }
                        WireResult::Ok(ModReadValue::HostStreams { value }) => {
                            WireResult::Ok(ModReadValue::Tab {
                                context: TabContext::Unassociated { streams: value },
                            })
                        }
                        WireResult::Ok(_) => unreachable!("a tab expands to a Stream or host read"),
                        WireResult::Error(error) => WireResult::Error(error),
                    }
                }
                _ => read,
            })
            .collect())
    }

    /// The tab's Stream: an enrolled conversation's Session names it, else frontend routing does.
    /// A Session that cannot be read is skipped here; the view shown names it as unreadable.
    fn tab_stream(
        &self,
        enrolled: &mut std::collections::HashMap<crate::integration::NativeRef, Enrollment>,
        native: &mut dyn crate::integration::TerminalObservation,
        terminals: &[crate::integration::NativeRef],
    ) -> std::result::Result<Option<String>, String> {
        use crate::integration::Access;
        for terminal in terminals {
            if let Enrollment::Enrolled(session) = self.enrolled_session(enrolled, native, terminal)
            {
                return Ok(Some(session.stream_id));
            }
        }
        for terminal in terminals {
            match native.routed_stream(terminal) {
                Access::Available {
                    value: Some(stream),
                } => return Ok(Some(stream)),
                Access::Available { value: None } | Access::Unsupported => {}
                Access::Unavailable { reason } => return Err(reason),
            }
        }
        Ok(None)
    }

    /// One foreground observation and Session read per terminal, shared by every selection.
    fn enrolled_session(
        &self,
        enrolled: &mut std::collections::HashMap<crate::integration::NativeRef, Enrollment>,
        native: &mut dyn crate::integration::TerminalObservation,
        terminal: &crate::integration::NativeRef,
    ) -> Enrollment {
        use crate::integration::ForegroundConversation;
        enrolled
            .entry(terminal.clone())
            .or_insert_with(|| match native.foreground_conversation(terminal) {
                ForegroundConversation::Current(conversation) => {
                    let session = Session::id_for(
                        &self.scope.host,
                        &conversation.harness,
                        &conversation.native_id,
                    );
                    match self
                        .records
                        .records()
                        .with_reader(|reader| self.session(reader, &session))
                    {
                        Ok(Some(session)) => Enrollment::Enrolled(Box::new(session)),
                        Ok(None) => Enrollment::Unenrolled,
                        Err(_) => {
                            Enrollment::Unavailable("saved Session view is unavailable".into())
                        }
                    }
                }
                ForegroundConversation::NotApplicable => Enrollment::NotApplicable,
                ForegroundConversation::Unknown { reason } => Enrollment::Unavailable(reason),
            })
            .clone()
    }

    fn stream_view(
        &self,
        reader: &dyn crate::artifacts::ArtifactReader,
        id: &str,
        sessions: &std::collections::BTreeMap<String, Session>,
        unreadable: &[UnreadableRecord],
    ) -> Result<crate::integration::StreamView> {
        use crate::integration::{
            DocumentKey, DocumentReference, LinkReference, SessionSummary, StreamArtifacts,
            StreamView,
        };
        let stream = self.stream(reader, id)?;
        let mut members: Vec<_> = sessions
            .values()
            .filter(|session| session.stream_id == id)
            .collect();
        members.sort_by_cached_key(|session| {
            (
                icu_casemap::CaseMapper::new()
                    .fold_string(&session.title)
                    .into_owned(),
                session.id.clone(),
            )
        });
        let mut artifacts = vec![];
        for producer in reader.stream_producers(&self.scope.host, id)? {
            if let Some(session) = sessions.get(&producer) {
                let mut entries = crate::artifacts::summaries(session, id);
                entries.sort_by(|left, right| left.key.cmp(&right.key));
                if !entries.is_empty() {
                    artifacts.push(StreamArtifacts { producer, entries });
                }
            }
        }
        artifacts.sort_by(|left, right| left.producer.cmp(&right.producer));
        let mut documents = Vec::new();
        let mut links = Vec::new();
        for reference in stream
            .references
            .iter()
            .filter(|r| r.state != crate::model::ReferenceState::Dropped)
        {
            match &reference.target {
                crate::model::ReferenceTarget::Document(document) => {
                    let repository = stream
                        .repositories
                        .iter()
                        .find_map(|(alias, repo)| {
                            (repo.url == document.repository_url).then(|| alias.clone())
                        })
                        .unwrap_or_else(|| document.repository_url.clone());
                    documents.push(DocumentReference {
                        key: DocumentKey {
                            repository,
                            path: document.path.clone(),
                        },
                        name: reference.name.clone(),
                        description: reference.purpose.clone(),
                    });
                }
                crate::model::ReferenceTarget::Issue { url } => {
                    links.push(LinkReference {
                        key: crate::integration::LinkKey {
                            kind: LinkKind::Issue,
                            url: url.clone(),
                        },
                        name: reference.name.clone(),
                        description: reference.purpose.clone(),
                    });
                }
                crate::model::ReferenceTarget::Link { url } => {
                    links.push(LinkReference {
                        key: crate::integration::LinkKey {
                            kind: LinkKind::Link,
                            url: url.clone(),
                        },
                        name: reference.name.clone(),
                        description: reference.purpose.clone(),
                    });
                }
                crate::model::ReferenceTarget::Pr { url } => {
                    links.push(LinkReference {
                        key: crate::integration::LinkKey {
                            kind: LinkKind::Pr,
                            url: url.clone(),
                        },
                        name: reference.name.clone(),
                        description: reference.purpose.clone(),
                    });
                }
            }
        }
        let last_updated_ms = stream_activity_ms(&stream, &members);
        let status = self.projected_status(stream.status, &members, last_updated_ms);
        Ok(StreamView {
            id: stream.id,
            title: stream.title,
            goal: stream.goal,
            goal_short: stream.goal_short,
            status,
            sessions: members.into_iter().map(SessionSummary::from).collect(),
            documents,
            links,
            artifacts,
            unreadable: unreadable.to_vec(),
        })
    }

    /// Two addressed records, without a roster, artifact listing or native inspection.
    pub fn conversation_records(
        &self,
        conversation: &ConversationId,
    ) -> Result<Option<(Stream, Session)>> {
        self.records.records().with_reader(|reader| {
            let id = Session::id_for(
                &self.scope.host,
                &conversation.harness,
                &conversation.native_id,
            );
            let Some(session) = self.session(reader, &id)? else {
                return Ok(None);
            };
            let key = RecordAddress::new(&self.scope.host, RecordKind::Stream, &session.stream_id)?;
            let Some(value) = reader.read(&key)? else {
                return Err(ValidationError::new("current Stream is missing").into());
            };
            let Record::Stream(stream) = value.record else {
                unreachable!("addressed Stream record")
            };
            Ok(Some((stream, session)))
        })
    }

    /// Resolve saved keys directly without foreground process-group re-verification.
    pub fn open_link(
        &self,
        stream: &str,
        link: &crate::integration::LinkKey,
        _context: &crate::integration::ActivationContext,
    ) -> crate::integration::ReferenceOpenReport {
        use crate::integration::{
            NonEmpty, ReferenceOpenReport, ReferenceOpenResult, ViewerAction,
        };
        let result = match self.saved_link(stream, link) {
            Ok(url) => ReferenceOpenResult::NeedsViewer {
                value: NonEmpty::new(ViewerAction::OpenUrl { url }, []),
            },
            Err(_) => ReferenceOpenResult::NotApplied {
                reason: "saved link reference is unavailable".into(),
            },
        };
        ReferenceOpenReport {
            known: vec![],
            result,
        }
    }

    fn saved_link(
        &self,
        stream: &str,
        link: &crate::integration::LinkKey,
    ) -> Result<crate::web_url::SafeHttpUrl> {
        let stream = self
            .records
            .records()
            .with_reader(|reader| self.stream(reader, stream))?;
        let exists = stream.references.iter().any(|reference| {
            reference.state != crate::model::ReferenceState::Dropped
                && match (link.kind, &reference.target) {
                    (LinkKind::Issue, crate::model::ReferenceTarget::Issue { url })
                    | (LinkKind::Pr, crate::model::ReferenceTarget::Pr { url })
                    | (LinkKind::Link, crate::model::ReferenceTarget::Link { url }) => {
                        url == &link.url
                    }
                    _ => false,
                }
        });
        if !exists {
            return Err(ValidationError::new("saved link reference is unavailable").into());
        }
        Ok(link.url.parse()?)
    }

    pub fn resolve_saved_reference(
        &self,
        reference: &Reference,
        registry: &crate::runtime::CheckoutRegistry,
        locations: &crate::runtime::DirectoryBindings,
    ) -> Result<ResolvedTarget> {
        match reference {
            Reference::Document {
                stream,
                document,
                viewing_session,
            } => {
                let (stream, session) =
                    self.records.records().with_reader(|reader| -> Result<_> {
                        let stream = self.stream(reader, stream)?;
                        let session = self.session(reader, viewing_session)?.ok_or_else(|| {
                            ValidationError::new("viewing Session is unavailable")
                        })?;
                        if session.stream_id != stream.id {
                            return Err(ValidationError::new(
                                "viewing Session is not a current member of this Stream",
                            )
                            .into());
                        }
                        Ok((stream, session))
                    })?;
                let resolved =
                    super::documents::resolve(registry, locations, &stream, &session, document)?;
                Ok(ResolvedTarget::Document {
                    stream: stream.id,
                    document: document.clone(),
                    viewing_session: session.id,
                    path: resolved.file,
                    checkout_id: resolved.checkout.id,
                    checkout_root: resolved.checkout.directory,
                })
            }
            Reference::Artifact { key } => {
                key.validate()?;
                if key.host != self.scope.host {
                    return Err(ValidationError::new("artifact belongs to another host").into());
                }
                self.records.records().with_reader(|reader| -> Result<()> {
                    let session = self
                        .session(reader, &key.session)?
                        .ok_or_else(|| ValidationError::new("artifact producer is unavailable"))?;
                    crate::integration::resolve_artifact_summary(key, &session)?;
                    Ok(())
                })?;
                Ok(ResolvedTarget::Artifact { key: key.clone() })
            }
            Reference::Link { stream, link } => Ok(ResolvedTarget::Link {
                url: self.saved_link(stream, link)?,
            }),
        }
    }

    fn stream(&self, reader: &dyn RecordReader, id: &str) -> Result<Stream> {
        let key = RecordAddress::new(&self.scope.host, RecordKind::Stream, id)?;
        let value = reader
            .read(&key)?
            .ok_or_else(|| ValidationError::new("saved Stream is unavailable"))?;
        let Record::Stream(stream) = value.record else {
            unreachable!("addressed Stream record")
        };
        Ok(stream)
    }

    fn session(&self, reader: &dyn RecordReader, id: &str) -> Result<Option<Session>> {
        let key = RecordAddress::new(&self.scope.host, RecordKind::Session, id)?;
        Ok(reader.read(&key)?.map(|value| {
            let Record::Session(session) = value.record else {
                unreachable!("addressed Session record")
            };
            session
        }))
    }
}

/// What a terminal's foreground observation says about its enrolled Session.
#[derive(Clone)]
enum Enrollment {
    Enrolled(Box<Session>),
    Unenrolled,
    NotApplicable,
    Unavailable(String),
}

fn sessions_by_id(reads: Vec<RecordRead>) -> std::collections::BTreeMap<String, Session> {
    reads
        .into_iter()
        .map(|value| {
            let Record::Session(session) = value.record else {
                unreachable!("Session partition");
            };
            (session.id.clone(), session)
        })
        .collect()
}
