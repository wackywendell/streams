use std::collections::BTreeMap;
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
    lines: Vec<String>,
    widths: Vec<usize>,
    documents: Vec<(char, TargetDto)>,
    links: Vec<String>,
}
#[derive(Deserialize)]
struct SnapshotDto {
    scope: ScopeDto,
    native: OwnerDto,
    stream: Stream,
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
#[derive(Deserialize)]
struct TargetDto {
    scope: ScopeDto,
    #[serde(rename = "repositoryUrl")]
    repository_url: String,
    r#ref: String,
    added_at: Option<String>,
    #[serde(flatten)]
    location: LocationDto,
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
    fn snapshot(self) -> CardSnapshot {
        assert_eq!(self.scope.host_id, self.session.host_id);
        assert_eq!(self.scope.stream_id, self.stream.id);
        assert_eq!(self.scope.session_id, self.session.id);
        assert_eq!(
            self.scope.native_session_id,
            self.session.harness_session_id
        );
        CardSnapshot::new(
            self.stream,
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
struct WidthDifference {
    fixture: String,
    remove_line: Option<usize>,
    replace_line: Option<(usize, String)>,
}

#[test]
fn active_renderer_goldens_and_approved_rust_unicode_layout() {
    // The migration explicitly accepts standard Rust layout for these five cases;
    // retain the original JS goldens and assert the exact approved line edits.
    let evidence: Vec<WidthDifference> =
        serde_json::from_str(include_str!("fixtures/unicode-width-differences.json")).unwrap();
    assert_eq!(evidence.len(), 5);
    assert_eq!(
        UnicodeWidthStr::width("ൎ"),
        0,
        "Recheck the recorded Unicode-width evidence"
    );
    let mut observed = 0;
    let mut differences = Vec::new();
    for fixture in fixtures() {
        let snapshot = fixture.snapshot.map(SnapshotDto::snapshot);
        let rendered = super::render::render(
            snapshot.as_ref(),
            Rect::new(0, 0, fixture.width, fixture.height),
            fixture.notice.as_deref(),
        );
        let mut expected = fixture.lines.clone();
        if let Some(difference) = evidence
            .iter()
            .find(|difference| difference.fixture == fixture.name)
        {
            observed += 1;
            assert_ne!(
                lines(&rendered.buffer),
                fixture.lines,
                "Parity now holds: recheck the approved Rust layout difference"
            );
            if let Some(line) = difference.remove_line {
                assert_eq!(expected.remove(line), "ൎ");
                expected.push(String::new());
            }
            if let Some((line, text)) = &difference.replace_line {
                assert_eq!(expected[*line], format!("{text}ൎ"));
                expected[*line] = text.clone();
            }
        }
        if lines(&rendered.buffer) != expected {
            differences.push(format!(
                "{}\nRust: {:?}\nExpected: {:?}",
                fixture.name,
                lines(&rendered.buffer),
                expected
            ));
        }
        assert!(fixture.widths.iter().all(|&w| w <= fixture.width as usize));
        assert_eq!(
            rendered
                .links
                .iter()
                .map(|(_, url, _)| url.as_str())
                .collect::<Vec<_>>(),
            fixture.links,
            "{} links",
            fixture.name
        );
        let targets: BTreeMap<_, _> = fixture
            .documents
            .into_iter()
            .map(|(key, dto)| {
                let snapshot = snapshot.as_ref().unwrap();
                assert_eq!(dto.scope.catalog_source, snapshot.scope.catalog_source);
                assert_eq!(dto.scope.host_id, snapshot.scope.host_id);
                assert_eq!(dto.scope.stream_id, snapshot.scope.stream_id);
                assert_eq!(dto.scope.session_id, snapshot.scope.session_id);
                assert_eq!(
                    dto.scope.native_session_id,
                    snapshot.scope.native_session_id
                );
                let document = snapshot
                    .stream
                    .documents
                    .iter()
                    .find(|d| {
                        d.repository == dto.location.repository && d.path == dto.location.path
                    })
                    .unwrap()
                    .clone();
                assert_eq!(dto.r#ref, document.r#ref);
                assert_eq!(dto.added_at, document.added_at);
                (
                    key,
                    DocumentTarget {
                        scope: snapshot.scope.clone(),
                        owner: snapshot.context.owner.clone(),
                        document,
                        repository_url: dto.repository_url,
                        location: dto.location.resolved(),
                    },
                )
            })
            .collect();
        assert_eq!(
            rendered.documents, targets,
            "{} document targets",
            fixture.name
        );
    }
    assert_eq!(observed, evidence.len());
    assert!(differences.is_empty(), "{}", differences.join("\n"));
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
    other.scope.catalog_source = "different".into();
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
        "ref", "added", "url", "root", "file", "error", "order", "removed", "progress",
    ] {
        let mut changed = snapshot();
        let action = view.paint(Rect::new(0, 0, 100, 12)).documents[&'a'].clone();
        match field {
            "ref" => changed.stream.documents[0].r#ref = "other".into(),
            "added" => changed.stream.documents[0].added_at = Some("new-commit".into()),
            "url" => {
                changed.stream.repositories.get_mut("lib").unwrap().url =
                    "https://example.com/other".into()
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
                changed.stream.documents.insert(
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
                changed.stream.documents.clear();
                changed.context.documents.clear();
            }
            _ => changed.session.state = "Fresh progress".into(),
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
    let mut target = snapshot.target(&snapshot.stream.documents[0]);
    let identity = target.editor_identity().unwrap();
    assert_eq!(identity.file, PathBuf::from("/code/library/docs/design.md"));
    target.document.r#ref = "recovery-branch".into();
    target.document.added_at = Some("a1".into());
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
    let mut bad = s.session.clone();
    bad.stream_id = "another".into();
    assert!(build(s.stream.clone(), bad, s.context.clone()).is_err());
    let mut bad = s.session.clone();
    bad.checkout = Some(Checkout::new("unknown", "main", ".").unwrap());
    assert!(build(s.stream.clone(), bad, s.context.clone()).is_err());
    let mut bad = s.context.clone();
    bad.documents.clear();
    assert!(build(s.stream.clone(), s.session.clone(), bad).is_err());
    let mut bad = s.context.clone();
    bad.checkouts.push(bad.checkouts[0].clone());
    assert!(build(s.stream.clone(), s.session.clone(), bad).is_err());
    let mut bad = s.context.clone();
    bad.documents[0].location = ResolvedDocument::File {
        selection: bad.checkouts[0].selection.clone(),
        file: "/another/checkout/design.md".into(),
    };
    assert!(build(s.stream.clone(), s.session.clone(), bad).is_err());
    assert!(build(s.stream, s.session, s.context).is_ok()); // nonexistent fixture roots are data
}

#[test]
fn gitlab_artifact_labels_use_the_first_artifact_not_a_suffix_path() {
    for (kind, pull_request) in [("issues", false), ("merge_requests", true)] {
        let mut snapshot = snapshot();
        let url = format!("https://gitlab.example/group/nested/project/-/{kind}/42/-/{kind}/7");
        let link = Link::new(&url, "artifact").unwrap();
        snapshot.stream.issues.clear();
        snapshot.stream.prs.clear();
        if pull_request {
            snapshot.stream.prs.push(link);
        } else {
            snapshot.stream.issues.push(link);
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
    s.stream.goal = Some("界😀é\u{1b}[31m\u{202e} literal | pipe 👩‍💻".into());
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
