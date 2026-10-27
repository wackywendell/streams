use std::path::PathBuf;

use super::*;
use crate::model::{Checkout, Document, Link, Session, Stream};
use ratatui::style::{Color, Modifier};
use serde::Deserialize;
use unicode_width::UnicodeWidthStr;

#[derive(Deserialize)]
struct Fixture {
    name: String,
    snapshot: Option<SnapshotDto>,
    width: u16,
    height: u16,
    notice: Option<String>,
}
#[derive(Deserialize)]
struct SnapshotDto {
    scope: ScopeDto,
    native: OwnerDto,
    stream: serde_json::Value,
    session: Session,
    checkouts: Vec<BindingDto>,
    #[serde(rename = "documentLocations")]
    locations: Vec<LocationDto>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScopeDto {
    catalog_source: String,
    host_id: String,
    stream_id: String,
    session_id: String,
    native_session_id: String,
}
#[derive(Deserialize)]
struct OwnerDto {
    pane_id: String,
    tab_id: String,
    workspace_id: String,
}
#[derive(Deserialize)]
struct BindingDto {
    repository: String,
    checkout_id: String,
    root: PathBuf,
}
#[derive(Deserialize)]
struct LocationDto {
    repository: String,
    path: String,
    checkout_id: Option<String>,
    root: Option<PathBuf>,
    file: Option<PathBuf>,
    error: Option<String>,
}
impl LocationDto {
    fn resolved(self) -> ResolvedDocument {
        let selection = self.checkout_id.map(|checkout_id| CheckoutSelection {
            checkout_id,
            root: self.root.unwrap(),
        });
        match self.error.as_deref() {
            None => ResolvedDocument::File {
                selection: selection.unwrap(),
                file: self.file.unwrap(),
            },
            Some(error) => ResolvedDocument::Unavailable {
                selection,
                problem: match error {
                    "Document not present in this checkout" => DocumentProblem::MissingDocument,
                    "No registered checkout" => DocumentProblem::MissingCheckout,
                    "Ambiguous viewing-session checkout binding" => {
                        DocumentProblem::AmbiguousCheckout
                    }
                    _ => DocumentProblem::Other(error.into()),
                },
            },
        }
    }
}
impl SnapshotDto {
    fn snapshot(mut self) -> CardSnapshot {
        // Frozen JavaScript fixtures predate the field rename; production accepts only commit.
        for document in self.stream["documents"].as_array_mut().unwrap() {
            let document = document.as_object_mut().unwrap();
            if let Some(commit) = document.remove("added_at") {
                document.insert("commit".into(), commit);
            }
        }
        let stream: Stream = serde_json::from_value(self.stream).unwrap();
        assert_eq!(self.scope.host_id, self.session.host_id);
        assert_eq!(self.scope.stream_id, stream.id);
        assert_eq!(self.scope.session_id, self.session.id);
        assert_eq!(
            self.scope.native_session_id,
            self.session.harness_session_id
        );
        CardSnapshot::new(
            stream,
            self.session,
            DisplayContext {
                catalog_source: self.scope.catalog_source,
                owner: NativeOwnerObservation {
                    pane_id: self.native.pane_id,
                    tab_id: self.native.tab_id,
                    workspace_id: self.native.workspace_id,
                },
                checkouts: self
                    .checkouts
                    .into_iter()
                    .map(|b| CheckoutBinding {
                        repository: b.repository,
                        selection: CheckoutSelection {
                            checkout_id: b.checkout_id,
                            root: b.root,
                        },
                    })
                    .collect(),
                documents: self
                    .locations
                    .into_iter()
                    .map(|l| DocumentLocation {
                        repository: l.repository.clone(),
                        path: l.path.clone(),
                        location: l.resolved(),
                    })
                    .collect(),
            },
        )
        .unwrap()
    }
}
fn fixtures() -> Vec<Fixture> {
    serde_json::from_str(include_str!("fixtures/active-renderer.json")).unwrap()
}
fn snapshot() -> CardSnapshot {
    fixtures().remove(0).snapshot.unwrap().snapshot()
}
fn view(snapshot: CardSnapshot) -> CardView {
    let mut view = CardView::new();
    let token = view.begin_refresh();
    assert!(view.complete_refresh(token, Ok(snapshot)));
    view
}
fn lines(buffer: &Buffer) -> Vec<String> {
    if buffer.area.is_empty() {
        return Vec::new();
    }
    (buffer.area.y..buffer.area.bottom())
        .map(|y| super::render::buffer_line(buffer, y))
        .collect()
}

#[derive(Deserialize)]
struct CardGolden {
    name: String,
    lines: Vec<String>,
    documents: Vec<char>,
    links: Vec<String>,
}

type CardCase = (String, Option<CardSnapshot>, u16, u16, Option<String>);

fn card_text_cases() -> Vec<CardCase> {
    let mut cases: Vec<_> = fixtures()
        .into_iter()
        .map(|f| {
            (
                f.name,
                f.snapshot.map(SnapshotDto::snapshot),
                f.width,
                f.height,
                f.notice,
            )
        })
        .collect();
    let mut card = snapshot();
    card.records.stream.goal =
        Some("Help people track work across sessions and use their cards.".into());
    card.records.session.title = "Naming".into();
    card.records.session.recap = "Built naming settings and Pi name import.".into();
    card.records.session.state = "Waiting on Argon review.".into();
    card.records.session.remaining = "Deploy and reload.".into();
    card.records.stream.documents.clear();
    card.context.documents.clear();
    card.records.stream.issues.clear();
    card.records.stream.prs =
        vec![Link::new("https://example.com/org/repo/pull/12", "Naming settings").unwrap()];
    for (name, width, height) in [
        ("wide", 44, 14),
        ("narrow", 20, 16),
        ("short-1", 44, 1),
        ("short-2", 44, 2),
        ("short-4", 44, 4),
        ("short-6", 44, 6),
    ] {
        cases.push((
            format!("card-text-{name}"),
            Some(card.clone()),
            width,
            height,
            None,
        ));
    }
    for status in [
        crate::model::Status::Active,
        crate::model::Status::Waiting,
        crate::model::Status::Blocked,
        crate::model::Status::Parked,
        crate::model::Status::Done,
    ] {
        let mut card = card.clone();
        card.records.session.status = status;
        cases.push((format!("card-text-{status}"), Some(card), 44, 14, None));
    }
    for field in ["recap", "state", "remaining", "all", "goal"] {
        let mut card = card.clone();
        match field {
            "recap" => card.records.session.recap.clear(),
            "state" => card.records.session.state.clear(),
            "remaining" => card.records.session.remaining.clear(),
            "goal" => card.records.stream.goal = None,
            _ => {
                card.records.session.recap.clear();
                card.records.session.state.clear();
                card.records.session.remaining.clear();
            }
        }
        cases.push((format!("card-text-empty-{field}"), Some(card), 44, 14, None));
    }
    cases
}

#[test]
fn link_actions_require_fresh_membership_under_the_painters_url_policy() {
    let mut original = snapshot();
    original.records.stream.issues = vec![Link::new("https://example.com", "Issue").unwrap()];
    original.records.stream.prs.clear();
    let url = url::Url::parse("https://example.com/").unwrap();
    assert!(original.contains_link(&url));
    assert!(!original.contains_link(&url::Url::parse("https://forged.example/").unwrap()));
    let mut refreshed = original.clone();
    refreshed.records.stream.issues.clear();
    assert!(!refreshed.contains_link(&url));
    refreshed.records.stream.prs = vec![Link::new("file:///tmp/private", "").unwrap()];
    assert!(!refreshed.contains_link(&url::Url::parse("file:///tmp/private").unwrap()));
}
#[test]
fn card_text_goldens_cover_layout_actions_lifecycles_and_empty_blocks() {
    let goldens: Vec<CardGolden> =
        serde_json::from_str(include_str!("fixtures/card-text-renderer.json")).unwrap();
    let cases = card_text_cases();
    assert_eq!(cases.len(), goldens.len());
    for ((name, snapshot, width, height, notice), expected) in cases.into_iter().zip(goldens) {
        assert_eq!(name, expected.name);
        let rendered = super::render::render(
            snapshot.as_ref(),
            Rect::new(0, 0, width, height),
            notice.as_deref(),
        );
        assert_eq!(lines(&rendered.buffer), expected.lines, "{name}");
        assert!(
            expected
                .lines
                .iter()
                .all(|line| UnicodeWidthStr::width(line.as_str()) <= width as usize)
        );
        assert_eq!(
            rendered.documents.keys().copied().collect::<Vec<_>>(),
            expected.documents,
            "{name} keys"
        );
        assert_eq!(
            rendered
                .links
                .iter()
                .map(|(_, url, _)| url.to_string())
                .collect::<Vec<_>>(),
            expected.links,
            "{name} links"
        );
        for (key, target) in rendered.documents {
            let snapshot = snapshot.as_ref().unwrap();
            let index = super::render::DOCUMENT_KEYS.find(key).unwrap();
            assert_eq!(
                target,
                snapshot.target(&snapshot.records.stream.documents[index]),
                "{name} target {key}"
            );
        }
    }
}

#[test]
fn frame_actions_expire_at_refresh_start_not_completion() {
    let mut view = view(snapshot());
    let frame = view.paint(Rect::new(0, 0, 100, 12)).clone();
    let action = frame.documents[&'a'].clone();
    assert!(view.is_current(&action));
    assert!(!frame.documents.contains_key(&'1'));
    let token = view.begin_refresh();
    assert!(!view.is_current(&action));
    let stale = view.paint(Rect::new(0, 0, 100, 12));
    assert_eq!(stale.freshness, Freshness::Refreshing);
    assert!(stale.documents.is_empty() && stale.links.is_empty());
    assert!(
        lines(&stale.buffer)
            .iter()
            .any(|l| l.starts_with("Refreshing…"))
    );
    assert!(view.complete_refresh(token, Ok(snapshot())));
    assert!(!view.is_current(&action));
    let next = view.paint(Rect::new(0, 0, 100, 12)).documents[&'a'].clone();
    assert!(view.is_current(&next));
    assert_ne!(next.frame(), action.frame());
}

#[test]
fn view_lifetimes_never_share_refresh_tokens_or_actions() {
    let mut first = CardView::new();
    let mut second = CardView::default();
    let first_token = first.begin_refresh();
    let second_token = second.begin_refresh();
    assert_ne!(first_token, second_token);
    assert!(!second.complete_refresh(first_token, Ok(snapshot())));
    assert!(first.complete_refresh(first_token, Ok(snapshot())));
    assert!(second.complete_refresh(second_token, Ok(snapshot())));
    let first_action = first.paint(Rect::new(0, 0, 100, 12)).documents[&'a'].clone();
    let second_action = second.paint(Rect::new(0, 0, 100, 12)).documents[&'a'].clone();
    assert!(first.is_current(&first_action));
    assert!(second.is_current(&second_action));
    assert!(!first.is_current(&second_action));
    assert!(!second.is_current(&first_action));
}

#[test]
fn membership_and_owner_moves_keep_the_view_but_invalidate_old_actions() {
    let original = snapshot();
    let mut view = view(original.clone());
    let area = Rect::new(0, 0, 120, 12);
    let old = view.paint(area).documents[&'a'].clone();
    let token = view.begin_quiet_refresh();
    assert!(!view.is_current(&old));
    assert!(
        !lines(&view.paint(area).buffer)
            .iter()
            .any(|line| line.contains("Refreshing"))
    );
    let mut moved = original;
    moved.records.stream.id = "destination".into();
    moved.records.session.stream_id = moved.records.stream.id.clone();
    moved.records.scope.stream_id = moved.records.stream.id.clone();
    moved.records.owner.tab_id = "new-tab".into();
    moved.records.owner.workspace_id = "new-workspace".into();
    assert!(view.complete_refresh(token, Ok(moved.clone())));
    let frame = view.paint(area);
    assert_eq!(frame.freshness, Freshness::Current);
    let new = frame.documents[&'a'].clone();
    let Target::Document(target) = new.target() else {
        panic!("missing document");
    };
    assert_eq!(target.scope, *moved.scope());
    assert_eq!(target.owner, *moved.owner());
    assert!(view.is_current(&new));
    assert!(!view.is_current(&old));
}

#[test]
fn stale_completion_failure_and_scope_switch_never_authorize_actions() {
    let mut view = view(snapshot());
    let first = view.begin_refresh();
    let second = view.begin_refresh();
    assert!(!view.complete_refresh(first, Ok(snapshot())));
    assert!(view.complete_refresh(second, Err("Malformed record".into())));
    let failed = view.paint(Rect::new(0, 0, 80, 5));
    assert_eq!(failed.freshness, Freshness::Failed);
    assert!(failed.documents.is_empty() && failed.links.is_empty());
    assert!(
        lines(&failed.buffer)
            .last()
            .unwrap()
            .starts_with("Card unavailable: Malformed record. Refresh to retry.")
    );
    let mut other = snapshot();
    other.records.scope.catalog_source = "different".into();
    let token = view.begin_refresh();
    assert!(view.complete_refresh(token, Ok(other)));
    let failed = view.paint(Rect::new(0, 0, 120, 3));
    assert_eq!(failed.freshness, Freshness::Failed);
    assert!(failed.documents.is_empty());
    let token = view.begin_refresh();
    assert!(!view.complete_refresh(token, Ok(snapshot())));
    let still_failed = view.paint(Rect::new(0, 0, 120, 3));
    assert_eq!(still_failed.freshness, Freshness::Failed);
    assert!(still_failed.documents.is_empty() && still_failed.links.is_empty());
    assert!(
        lines(&still_failed.buffer)
            .iter()
            .any(|line| line.contains("Streams scope changed"))
    );
}

#[test]
fn document_revision_label_uses_optional_commit_not_source_session() {
    let mut snapshot = snapshot();
    let path = snapshot.records.stream.documents[0].path.clone();
    snapshot.records.stream.documents[0].source_session_id = Some("source-session".into());
    for commit in [None, Some("current-123456")] {
        snapshot.records.stream.documents[0].commit = commit.map(str::to_owned);
        let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, 160, 30), None);
        let rendered_lines = lines(&rendered.buffer);
        assert!(rendered_lines.iter().any(|line| line.contains(&path)));
        assert_eq!(
            rendered_lines
                .iter()
                .any(|line| line.contains(&format!("{path} (added current-)"))),
            commit.is_some()
        );
        if commit.is_none() {
            assert!(!rendered_lines.iter().any(|line| line.contains(" (added ")));
        }
        assert_eq!(
            rendered.documents[&'a'].document,
            snapshot.records.stream.documents[0]
        );
    }
}

#[test]
fn every_repaint_geometry_notice_and_target_change_invalidates_old_actions() {
    let mut view = view(snapshot());
    let action = view.paint(Rect::new(0, 0, 100, 12)).documents[&'a'].clone();
    view.geometry_changed();
    assert!(!view.is_current(&action));
    let action = view.paint(Rect::new(0, 0, 100, 12)).documents[&'a'].clone();
    view.set_notice(Some("Projection unavailable".into()));
    assert!(!view.is_current(&action));
    let action = view.paint(Rect::new(0, 0, 100, 12)).documents[&'a'].clone();
    view.paint(Rect::new(0, 0, 100, 12));
    assert!(!view.is_current(&action));
    for field in [
        "ref",
        "commit",
        "source-session",
        "url",
        "root",
        "file",
        "error",
        "order",
        "removed",
        "progress",
    ] {
        let mut changed = snapshot();
        let action = view.paint(Rect::new(0, 0, 100, 12)).documents[&'a'].clone();
        match field {
            "ref" => changed.records.stream.documents[0].r#ref = "other".into(),
            "commit" => changed.records.stream.documents[0].commit = Some("latest-commit".into()),
            "source-session" => {
                changed.records.stream.documents[0].source_session_id = Some("source".into())
            }
            "url" => {
                changed
                    .records
                    .stream
                    .repositories
                    .get_mut("lib")
                    .unwrap()
                    .url = "https://example.com/other".into()
            }
            "root" => {
                let selection = CheckoutSelection {
                    checkout_id: "replacement".into(),
                    root: "/relocated".into(),
                };
                changed.context.checkouts[0].selection = selection.clone();
                changed.context.documents[0].location = ResolvedDocument::File {
                    selection,
                    file: "/relocated/docs/design.md".into(),
                };
            }
            "file" => {
                if let ResolvedDocument::File { file, .. } =
                    &mut changed.context.documents[0].location
                {
                    *file = "/code/library/actual.md".into();
                }
            }
            "error" => {
                changed.context.documents[0].location = ResolvedDocument::Unavailable {
                    selection: None,
                    problem: DocumentProblem::MissingCheckout,
                }
            }
            "order" => {
                changed.records.stream.documents.insert(
                    0,
                    Document::new("lib", "topic", "first.md", "", None).unwrap(),
                );
                changed.context.documents.insert(
                    0,
                    DocumentLocation {
                        repository: "lib".into(),
                        path: "first.md".into(),
                        location: ResolvedDocument::Unavailable {
                            selection: None,
                            problem: DocumentProblem::MissingCheckout,
                        },
                    },
                );
            }
            "removed" => {
                changed.records.stream.documents.clear();
                changed.context.documents.clear();
            }
            _ => changed.records.session.state = "Fresh progress".into(),
        }
        let token = view.begin_refresh();
        assert!(!view.is_current(&action));
        assert!(view.complete_refresh(token, Ok(changed)));
        let next = view.paint(Rect::new(0, 0, 100, 12));
        let target = next.documents.get(&'a').map(CapturedAction::target);
        match field {
            "removed" => assert!(target.is_none()),
            "progress" => assert_eq!(target, Some(action.target())),
            _ => assert!(
                target.is_some_and(|target| target != action.target()),
                "{field}"
            ),
        }
        assert!(!view.is_current(&action));
        let token = view.begin_refresh();
        view.complete_refresh(token, Ok(snapshot()));
    }
}

#[test]
fn editor_identity_preserves_viewing_file_and_checkout_lifetime_not_recovery_revision() {
    let snapshot = snapshot();
    let mut target = snapshot.target(&snapshot.records.stream.documents[0]);
    let identity = target.editor_identity().unwrap();
    assert_eq!(identity.file, PathBuf::from("/code/library/docs/design.md"));
    target.document.r#ref = "recovery-branch".into();
    target.document.commit = Some("b2".into());
    target.document.source_session_id = Some("source".into());
    assert_eq!(identity, target.editor_identity().unwrap());
    if let ResolvedDocument::File { selection, .. } = &mut target.location {
        selection.checkout_id = "replacement-lifetime".into();
    }
    assert_ne!(identity, target.editor_identity().unwrap());
    for problem in [
        DocumentProblem::MissingDocument,
        DocumentProblem::MissingCheckout,
        DocumentProblem::AmbiguousCheckout,
    ] {
        target.location = ResolvedDocument::Unavailable {
            selection: None,
            problem: problem.clone(),
        };
        assert_eq!(target.editor_identity(), Err(&problem));
    }
}

#[test]
fn domain_and_location_validation_does_not_guess_or_touch_filesystem() {
    let s = snapshot();
    let build = |stream, session, context| CardSnapshot::new(stream, session, context);
    let mut bad = s.records.session.clone();
    bad.stream_id = "another".into();
    assert!(build(s.records.stream.clone(), bad, s.context.clone()).is_err());
    for repository in [
        "unknown",
        s.records
            .stream
            .repositories
            .keys()
            .next()
            .unwrap()
            .as_str(),
    ] {
        let mut bad = s.records.session.clone();
        bad.checkout = Some(Checkout::new(repository, "main", ".").unwrap());
        assert!(build(s.records.stream.clone(), bad, s.context.clone()).is_err());
    }
    let mut bad = s.context.clone();
    bad.documents.clear();
    assert!(build(s.records.stream.clone(), s.records.session.clone(), bad).is_err());
    let mut bad = s.context.clone();
    bad.checkouts.push(bad.checkouts[0].clone());
    assert!(build(s.records.stream.clone(), s.records.session.clone(), bad).is_err());
    let mut bad = s.context.clone();
    bad.documents[0].location = ResolvedDocument::File {
        selection: bad.checkouts[0].selection.clone(),
        file: "/another/checkout/design.md".into(),
    };
    assert!(build(s.records.stream.clone(), s.records.session.clone(), bad).is_err());
    assert!(build(s.records.stream, s.records.session, s.context).is_ok()); // nonexistent fixture roots are data
}

#[test]
fn gitlab_artifact_labels_use_the_first_artifact_not_a_suffix_path() {
    for (kind, pull_request) in [("issues", false), ("merge_requests", true)] {
        let mut snapshot = snapshot();
        let url = format!("https://gitlab.example/group/nested/project/-/{kind}/42/-/{kind}/7");
        let link = Link::new(&url, "artifact").unwrap();
        snapshot.records.stream.issues.clear();
        snapshot.records.stream.prs.clear();
        if pull_request {
            snapshot.records.stream.prs.push(link);
        } else {
            snapshot.records.stream.issues.push(link);
        }
        let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, 100, 30), None);
        let text = lines(&rendered.buffer).join("\n");
        assert!(
            text.contains(if pull_request {
                "PR #42: artifact"
            } else {
                "Issue #42: artifact"
            }),
            "{text}"
        );
        assert_eq!(rendered.links[0].1.as_str(), url);
    }
}

#[test]
fn unicode_and_styles_are_cell_bounded_without_terminal_sequences() {
    let mut s = snapshot();
    s.records.stream.goal = Some("界😀é\u{1b}[31m\u{202e} literal | pipe 👩‍💻".into());
    for width in 0..=40 {
        for height in 0..=10 {
            let rendered = super::render::render(
                Some(&s),
                Rect::new(3, 4, width, height),
                Some("Notice 界😀é"),
            );
            for line in lines(&rendered.buffer) {
                assert!(UnicodeWidthStr::width(line.as_str()) <= width as usize);
                assert!(!line.contains('\u{1b}') && !line.contains('\u{202e}'));
            }
            for key in rendered.documents.keys() {
                assert!(
                    lines(&rendered.buffer)
                        .iter()
                        .any(|l| l.contains(&format!("[{key}]")))
                );
            }
        }
    }
    let rendered = super::render::render(Some(&snapshot()), Rect::new(0, 0, 100, 12), None);
    assert!(rendered.buffer[(0, 0)].modifier.contains(Modifier::DIM));
    assert!(
        rendered
            .buffer
            .content()
            .iter()
            .any(|c| c.modifier.contains(Modifier::BOLD))
    );
    assert!(
        rendered
            .buffer
            .content()
            .iter()
            .any(|c| c.fg == Color::Cyan)
    );
}
