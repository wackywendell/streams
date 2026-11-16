//! Integration reads and actions stay within one already-admitted catalog and owner host.

use super::{OpenedRecords, Result};
use crate::{
    integration::{
        ConversationId, LinkKind, ReadReply, Reference, ResolvedTarget, SessionView, WorkScope,
    },
    model::{Record, RecordKind, Session, Stream, ValidationError},
    storage::records::{RecordAddress, RecordReader, RecordStore},
};

pub struct Streams<'a> {
    records: &'a OpenedRecords,
    scope: WorkScope,
}

impl<'a> Streams<'a> {
    pub fn new(records: &'a OpenedRecords) -> Self {
        Self {
            records,
            scope: WorkScope {
                catalog: records.source_id().into(),
                host: records.host().as_str().into(),
            },
        }
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
                    .into_iter()
                    .filter_map(|value| {
                        let Record::Session(session) = value.record else {
                            unreachable!("Session partition")
                        };
                        (session.stream_id == stream.id
                            && session.status == crate::model::Status::Active)
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
        let sessions = self.sessions_by_id()?;
        self.records.artifacts().with_artifacts(|reader| {
            Ok(ReadReply {
                view: self.stream_view(reader, stream, &sessions)?,
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
        let sessions = if streams.is_empty() {
            None
        } else {
            self.sessions_by_id().ok()
        };
        let read = |reader: &dyn RecordReader,
                    artifacts: Option<&dyn ArtifactReader>|
         -> Result<_> {
            let mut stream_views = BTreeMap::new();
            for stream in &streams {
                let result = match (&sessions, artifacts) {
                    (Some(sessions), Some(artifacts)) => self
                        .stream_view(artifacts, stream, sessions)
                        .map(|value| WireResult::Ok(ModReadValue::Stream { value }))
                        .unwrap_or_else(|_| {
                            WireResult::Error(WireError::unavailable(
                                "saved Stream view is unavailable",
                            ))
                        }),
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
                                Some(sessions) => Ok(sessions.get(session).cloned()),
                                None => self.session(reader, session),
                            };
                            match value {
                                Ok(Some(session)) => WireResult::Ok(ModReadValue::Session {
                                    value: SessionView::from(&session),
                                }),
                                _ => WireResult::Error(WireError::unavailable(
                                    "saved Session view is unavailable",
                                )),
                            }
                        })
                        .clone(),
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

    /// Resolve native selectors first so saved and contextual views share the same read batch.
    pub fn read_views(
        &self,
        selections: &[crate::integration::ViewSelection],
        presentation: crate::integration::Access<&dyn crate::integration::PresentationSnapshot>,
        execution: crate::integration::Access<&dyn crate::integration::ExecutionSnapshot>,
    ) -> Result<Vec<crate::integration::WireResult<crate::integration::ModReadValue>>> {
        use crate::integration::{
            Access, CompanionVisibility, ForegroundConversation, FrameContext, ModReadValue,
            SidebarContext, ViewSelection, WireResult,
        };
        let mut expanded = selections.to_vec();
        let mut contexts = std::collections::BTreeMap::new();
        let mut frames = std::collections::BTreeMap::new();
        for (index, selection) in selections.iter().enumerate() {
            match selection {
                ViewSelection::Sidebar { space, .. } => {
                    let native = match &presentation {
                        Access::Available { value } => value,
                        _ => {
                            contexts.insert(
                                index,
                                SidebarContext::Unavailable {
                                    reason: "native presentation is unavailable".into(),
                                },
                            );
                            continue;
                        }
                    };
                    if !native.contains_space(space) {
                        contexts.insert(
                            index,
                            SidebarContext::Unavailable {
                                reason: "space is unavailable on this endpoint".into(),
                            },
                        );
                        continue;
                    }
                    match native.stream_for_space(space) {
                        Some(stream) => expanded[index] = ViewSelection::SavedStream { stream },
                        None => {
                            contexts.insert(index, SidebarContext::Unassociated);
                        }
                    }
                }
                ViewSelection::Frame { terminal } => {
                    let foreground = match &execution {
                        Access::Available { value } => value.foreground_conversation(terminal),
                        _ => {
                            frames.insert(
                                index,
                                FrameContext::Unavailable {
                                    reason: "frame context is unavailable".into(),
                                },
                            );
                            continue;
                        }
                    };
                    match foreground {
                        ForegroundConversation::Current(conversation) => {
                            let session = Session::id_for(
                                &self.scope.host,
                                &conversation.harness,
                                &conversation.native_id,
                            );
                            let context = self
                                .records
                                .records()
                                .with_reader(|reader| self.session(reader, &session))
                                .map(|found| match found {
                                    Some(value) => FrameContext::Enrolled {
                                        session: Box::new(SessionView::from(&value)),
                                        companion: CompanionVisibility::Shown,
                                    },
                                    None => FrameContext::Unenrolled,
                                })
                                .unwrap_or_else(|_| FrameContext::Unavailable {
                                    reason: "saved Session view is unavailable".into(),
                                });
                            frames.insert(index, context);
                        }
                        ForegroundConversation::NotApplicable => {
                            frames.insert(index, FrameContext::NotApplicable);
                        }
                        ForegroundConversation::Unknown { reason } => {
                            frames.insert(index, FrameContext::Unavailable { reason });
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(self
            .read_saved_views(&expanded)?
            .into_iter()
            .enumerate()
            .map(|(index, read)| match &selections[index] {
                ViewSelection::Sidebar { space, viewing } => {
                    let context = contexts.remove(&index).unwrap_or_else(|| match read {
                        WireResult::Ok(ModReadValue::Stream { value }) => {
                            let viewing_session = self.sidebar_viewing(
                                &value,
                                space,
                                viewing,
                                &presentation,
                                &execution,
                            );
                            SidebarContext::Associated {
                                stream: Box::new(value),
                                viewing_session,
                            }
                        }
                        _ => SidebarContext::Unavailable {
                            reason: "associated Stream view is unavailable".into(),
                        },
                    });
                    WireResult::Ok(ModReadValue::Sidebar { value: context })
                }
                ViewSelection::Frame { .. } => {
                    let context = frames.remove(&index).unwrap_or(match read {
                        WireResult::Ok(ModReadValue::Session { value }) => FrameContext::Enrolled {
                            session: Box::new(value),
                            companion: CompanionVisibility::Shown,
                        },
                        _ => FrameContext::Unenrolled,
                    });
                    WireResult::Ok(ModReadValue::Frame { context })
                }
                _ => read,
            })
            .collect())
    }

    fn sidebar_viewing(
        &self,
        stream: &crate::integration::StreamView,
        space: &crate::integration::NativeRef,
        viewing: &crate::integration::ViewingChoice,
        presentation: &crate::integration::Access<&dyn crate::integration::PresentationSnapshot>,
        execution: &crate::integration::Access<&dyn crate::integration::ExecutionSnapshot>,
    ) -> crate::integration::ViewingSession {
        use crate::integration::{Access, ForegroundConversation, ViewingChoice, ViewingSession};
        let member = |id: &str| stream.sessions.iter().any(|session| session.id == id);
        match viewing {
            ViewingChoice::Pinned { session } => {
                if member(session) {
                    ViewingSession::Selected {
                        session: session.clone(),
                    }
                } else {
                    ViewingSession::Unavailable {
                        reason: "pinned Session is not a current member on this host".into(),
                    }
                }
            }
            ViewingChoice::Follow { last_agent: None } => ViewingSession::None,
            ViewingChoice::Follow {
                last_agent: Some(terminal),
            } => {
                let Access::Available { value: native } = presentation else {
                    return ViewingSession::Unavailable {
                        reason: "native presentation is unavailable".into(),
                    };
                };
                if native.space_for_terminal(terminal).as_ref() != Some(space) {
                    return ViewingSession::None;
                }
                let foreground = match execution {
                    Access::Available { value } => value.foreground_conversation(terminal),
                    _ => {
                        return ViewingSession::Unavailable {
                            reason: "foreground context is unavailable".into(),
                        };
                    }
                };
                match foreground {
                    ForegroundConversation::Current(conversation) => {
                        let session = Session::id_for(
                            &self.scope.host,
                            &conversation.harness,
                            &conversation.native_id,
                        );
                        if member(&session) {
                            ViewingSession::Selected { session }
                        } else {
                            ViewingSession::None
                        }
                    }
                    ForegroundConversation::NotApplicable => ViewingSession::None,
                    ForegroundConversation::Unknown { reason } => {
                        ViewingSession::Unavailable { reason }
                    }
                }
            }
        }
    }

    fn sessions_by_id(&self) -> Result<std::collections::BTreeMap<String, Session>> {
        Ok(self
            .records
            .host_sessions()?
            .into_iter()
            .map(|value| {
                let Record::Session(session) = value.record else {
                    unreachable!("Session partition");
                };
                (session.id.clone(), session)
            })
            .collect())
    }

    fn stream_view(
        &self,
        reader: &dyn crate::artifacts::ArtifactReader,
        id: &str,
        sessions: &std::collections::BTreeMap<String, Session>,
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
                        description: reference.purpose.clone(),
                    });
                }
                crate::model::ReferenceTarget::Issue { url }
                | crate::model::ReferenceTarget::Link { url } => {
                    links.push(LinkReference {
                        key: crate::integration::LinkKey {
                            kind: LinkKind::Issue,
                            url: url.clone(),
                        },
                        description: reference.purpose.clone(),
                    });
                }
                crate::model::ReferenceTarget::Pr { url } => {
                    links.push(LinkReference {
                        key: crate::integration::LinkKey {
                            kind: LinkKind::Pr,
                            url: url.clone(),
                        },
                        description: reference.purpose.clone(),
                    });
                }
            }
        }
        Ok(StreamView {
            id: stream.id,
            title: stream.title,
            goal: stream.goal,
            goal_short: stream.goal_short,
            status: stream.status,
            sessions: members.into_iter().map(SessionSummary::from).collect(),
            documents,
            links,
            artifacts,
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
                    (
                        LinkKind::Issue,
                        crate::model::ReferenceTarget::Issue { url }
                        | crate::model::ReferenceTarget::Link { url },
                    )
                    | (LinkKind::Pr, crate::model::ReferenceTarget::Pr { url }) => url == &link.url,
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
