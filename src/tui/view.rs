use std::collections::BTreeMap;

use crate::model::Reference;
use ratatui::{buffer::Buffer, layout::Rect};
use uuid::Uuid;

use super::{CardSnapshot, DocumentTarget, Scope, render};

/// Display freshness is independent of the contribution's domain status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Current,
    Refreshing,
    Failed,
    Empty,
}

/// Identity of one painted frame within a unique view lifetime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameIdentity {
    view: Uuid,
    generation: u64,
    scope: Option<Scope>,
    area: Rect,
}

/// A request token prevents an older completion replacing a newer observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshToken {
    view: Uuid,
    generation: u64,
}

/// A pure action description. Native code must revalidate live authority before effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Document(Box<DocumentTarget>),
    Artifact(Box<crate::artifacts::ArtifactSummary>),
    Link(Box<Reference>),
}

/// An exact target captured against the geometry and scope of a visible current frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedAction {
    frame: FrameIdentity,
    target: Target,
}
impl CapturedAction {
    pub fn frame(&self) -> &FrameIdentity {
        &self.frame
    }
    pub fn target(&self) -> &Target {
        &self.target
    }
}

/// A terminal hyperlink hit region; numeric labels are not keyboard shortcuts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleLink {
    pub index: usize,
    pub area: Rect,
    pub action: CapturedAction,
}

/// Ratatui buffer and only the actions that survived final clipping and footer placement.
#[derive(Debug, Clone)]
pub struct PaintedFrame {
    pub identity: FrameIdentity,
    pub freshness: Freshness,
    pub buffer: Buffer,
    pub documents: BTreeMap<char, CapturedAction>,
    pub links: Vec<VisibleLink>,
}

enum Observation {
    Empty,
    Current(CardSnapshot),
    Refreshing(Option<CardSnapshot>),
    Failed(String),
    Switched,
}

/// Synchronous view reducer. The caller supplies observations and schedules refreshes;
/// this type neither reads a source nor handles terminal input or performs effects.
pub struct CardView {
    view_id: Uuid,
    generation: u64,
    scope: Option<Scope>,
    observation: Observation,
    pending: Option<RefreshToken>,
    quiet_refresh: bool,
    notice: Option<String>,
    painted: Option<PaintedFrame>,
}

impl Default for CardView {
    fn default() -> Self {
        Self::new()
    }
}

impl CardView {
    /// Creates an independent lifetime, including when reopening the same scope.
    pub fn new() -> Self {
        Self {
            view_id: Uuid::new_v4(),
            generation: 0,
            scope: None,
            observation: Observation::Empty,
            pending: None,
            quiet_refresh: false,
            notice: None,
            painted: None,
        }
    }

    fn invalidate(&mut self) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("frame generation exhausted");
        self.painted = None;
    }

    /// Must be called before dispatching any read. Previously captured actions become
    /// invalid immediately, even while a stale snapshot is still being repainted.
    pub fn begin_refresh(&mut self) -> RefreshToken {
        self.quiet_refresh = false;
        self.invalidate();
        self.observation = match std::mem::replace(&mut self.observation, Observation::Empty) {
            Observation::Current(snapshot) => Observation::Refreshing(Some(snapshot)),
            Observation::Refreshing(previous) => Observation::Refreshing(previous),
            Observation::Switched => Observation::Switched,
            _ => Observation::Refreshing(None),
        };
        let token = RefreshToken {
            view: self.view_id,
            generation: self.generation,
        };
        self.pending = Some(token);
        token
    }

    pub(super) fn begin_quiet_refresh(&mut self) -> RefreshToken {
        let token = self.begin_refresh();
        self.quiet_refresh = true;
        token
    }

    /// Rejects superseded completions or another conversation. Verified membership
    /// and placement changes invalidate old actions without retiring this card.
    pub fn complete_refresh(
        &mut self,
        token: RefreshToken,
        result: Result<CardSnapshot, String>,
    ) -> bool {
        if self.pending != Some(token) {
            return false;
        }
        self.pending = None;
        self.invalidate();
        if matches!(self.observation, Observation::Switched) {
            return false;
        }
        self.observation = match result {
            Ok(snapshot)
                if self
                    .scope
                    .as_ref()
                    .is_none_or(|scope| scope.same_conversation(snapshot.scope())) =>
            {
                self.scope = Some(snapshot.scope().clone());
                Observation::Current(snapshot)
            }
            Ok(_) => Observation::Switched,
            Err(message) => Observation::Failed(message),
        };
        true
    }

    pub(super) fn current_snapshot(&self) -> Option<&CardSnapshot> {
        match &self.observation {
            Observation::Current(snapshot) => Some(snapshot),
            _ => None,
        }
    }

    /// Invalidates captured targets before an asynchronous resize/repaint is scheduled.
    pub fn geometry_changed(&mut self) {
        self.invalidate();
    }

    /// Notices reserve a row; changing one invalidates the previously painted targets.
    pub fn set_notice(&mut self, notice: Option<String>) {
        if self.notice != notice {
            self.invalidate();
            self.notice = notice;
        }
    }

    /// Reflows cached data without I/O. Every paint has a new identity, even at the same size.
    pub fn paint(&mut self, area: Rect) -> &PaintedFrame {
        self.invalidate();
        let (snapshot, freshness, failure) = match &self.observation {
            Observation::Empty => (None, Freshness::Empty, None),
            Observation::Current(snapshot) => (Some(snapshot), Freshness::Current, None),
            Observation::Refreshing(previous) => (
                previous.as_ref(),
                Freshness::Refreshing,
                (!self.quiet_refresh).then(|| "Refreshing…".into()),
            ),
            Observation::Failed(message) => (
                None,
                Freshness::Failed,
                Some(format!("Card unavailable: {message}. Refresh to retry.")),
            ),
            Observation::Switched => (
                None,
                Freshness::Failed,
                Some("Card unavailable: Streams scope changed; create a new view for the new attachment".into()),
            ),
        };
        let rendered = render::render(
            snapshot,
            area,
            failure.as_deref().or(self.notice.as_deref()),
        );
        let identity = FrameIdentity {
            view: self.view_id,
            generation: self.generation,
            scope: self.scope.clone(),
            area,
        };
        let current = freshness == Freshness::Current;
        let documents = if current {
            rendered
                .documents
                .into_iter()
                .map(|(key, document)| {
                    (
                        key,
                        CapturedAction {
                            frame: identity.clone(),
                            target: document,
                        },
                    )
                })
                .collect()
        } else {
            BTreeMap::new()
        };
        let links = if current {
            rendered
                .links
                .into_iter()
                .map(|(index, url, area)| VisibleLink {
                    index,
                    area,
                    action: CapturedAction {
                        frame: identity.clone(),
                        target: Target::Link(Box::new(url)),
                    },
                })
                .collect()
        } else {
            Vec::new()
        };
        self.painted = Some(PaintedFrame {
            identity,
            freshness,
            buffer: rendered.buffer,
            documents,
            links,
        });
        self.painted.as_ref().unwrap()
    }

    /// Checks only local frame currency, never native ownership or filesystem freshness.
    /// Call again after any await in the action path, then perform the native guard.
    pub fn is_current(&self, action: &CapturedAction) -> bool {
        self.painted.as_ref().is_some_and(|frame| {
            frame.freshness == Freshness::Current
                && frame.identity == action.frame
                && (frame.documents.values().any(|a| a == action)
                    || frame.links.iter().any(|l| l.action == *action))
        })
    }
}
