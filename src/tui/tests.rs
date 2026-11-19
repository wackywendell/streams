use std::path::PathBuf;

use super::*;
use crate::model::{
    Checkout, Reference, ReferenceState, ReferenceTarget, RepositoryDocument, Session, Stream,
};
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
    repository_url: String,
    checkout_id: String,
    root: PathBuf,
}
#[derive(Deserialize)]
struct LocationDto {
    repository_url: String,
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
    fn snapshot(self) -> CardSnapshot {
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
                        repository_url: b.repository_url,
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
                        repository_url: l.repository_url.clone(),
                        path: l.path.clone(),
                        location: l.resolved(),
                    })
                    .collect(),
            },
        )
        .unwrap()
    }
}
fn document_mut(reference: &mut Reference) -> &mut RepositoryDocument {
    match &mut reference.target {
        ReferenceTarget::Document(document) => document,
        _ => panic!("expected document"),
    }
}
fn document_reference(path: &str, name: &str) -> Reference {
    serde_json::from_value(serde_json::json!({
        "name":name, "purpose":"Guide the work", "role":"brief", "state":"active",
        "target":{"kind":"document","repository_url":"https://example.com/lib","path":path,"ref":"topic"}
    })).unwrap()
}
fn url_reference(kind: &str, url: &str, name: &str) -> Reference {
    serde_json::from_value(serde_json::json!({
        "name":name,"purpose":name,"role":"deliverable","state":"active","target":{"kind":kind,"url":url}
    })).unwrap()
}
fn reference_url_text(reference: &Reference) -> &str {
    match &reference.target {
        ReferenceTarget::Pr { url }
        | ReferenceTarget::Issue { url }
        | ReferenceTarget::Link { url } => url,
        _ => panic!(),
    }
}
#[test]
fn reference_minima_reserve_later_headers_before_earlier_purpose_continuations() {
    let mut snapshot = snapshot();
    snapshot.records.stream.goal = Some("Goal".into());
    snapshot.records.session.recap.clear();
    snapshot.records.session.state.clear();
    snapshot.records.session.remaining.clear();
    snapshot.context.documents.clear();
    snapshot.records.stream.references = ["First", "Second", "Third"]
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            let mut reference =
                url_reference("link", &format!("https://example.test/{index}"), name);
            reference.purpose =
                "A long purpose with several wrapped continuations to allocate".into();
            reference
        })
        .collect();
    for (height, visible, hidden) in [(6, 2, 1), (7, 3, 0), (8, 3, 0)] {
        let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, 40, height), None);
        let text = lines(&rendered.buffer);
        assert_eq!(rendered.links.len(), visible);
        for (index, reference) in snapshot.stream().references.iter().enumerate() {
            assert_eq!(
                text.iter()
                    .any(|line| line.contains(&format!("Link {}", reference.name))),
                index < visible
            );
        }
        assert_eq!(
            text.iter()
                .any(|line| line.starts_with(&format!("… {hidden} more"))),
            hidden > 0
        );
        let first = rendered.links[0].2.y as usize;
        let second = rendered.links[1].2.y as usize;
        assert_eq!(second - first, if height == 8 { 3 } else { 2 });
        assert!(text[first + 1].starts_with("    "));
    }
    let mut dropped = snapshot.records.stream.references[0].clone();
    dropped.target = ReferenceTarget::Link {
        url: "https://example.test/dropped".into(),
    };
    dropped.state = ReferenceState::Dropped;
    snapshot.records.stream.references.insert(0, dropped);
    let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, 40, 6), None);
    assert_eq!(rendered.links.len(), 2);
    assert!(
        lines(&rendered.buffer)
            .iter()
            .any(|line| line.starts_with("… 1 more"))
    );
}

#[test]
fn dropped_documents_need_no_location_or_shortcut_and_artifacts_follow_visible_references() {
    let mut snapshot = snapshot();
    snapshot.records.stream.references[0].state = ReferenceState::Dropped;
    snapshot.context.documents.clear();
    snapshot.records.session.artifacts.insert(
        snapshot.records.stream.id.clone(),
        vec![crate::artifacts::ArtifactEntry {
            name: "result.md".into(),
            description: "Independent Session output".into(),
        }],
    );
    let snapshot = CardSnapshot::new(
        snapshot.records.stream,
        snapshot.records.session,
        snapshot.context,
    )
    .unwrap();
    let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, 100, 20), None);
    let text = lines(&rendered.buffer).join("\n");
    assert!(!text.contains("Document Design"));
    assert!(text.find("PR Review").unwrap() < text.find("Issue Track work").unwrap());
    assert!(text.find("Issue Track work").unwrap() < text.find("result.md").unwrap());
    assert_eq!(rendered.documents.len(), 1);
    assert!(
        matches!(&rendered.documents[&'a'], Target::Artifact(artifact) if artifact.key.name == "result.md")
    );
    assert!(
        snapshot
            .document_target("https://example.com/lib", "docs/design.md")
            .is_none()
    );
}

#[test]
fn reference_groups_labels_states_and_document_qualifiers_use_saved_values() {
    let mut snapshot = snapshot();
    snapshot.records.stream.references.reverse();
    snapshot.records.stream.references[0].state = ReferenceState::Fulfilled;
    let mut link = url_reference("link", "https://example.test/background", "Dashboard");
    link.role = crate::model::ReferenceRole::Background;
    snapshot.records.stream.references.insert(0, link);
    let text = |snapshot: &CardSnapshot| {
        lines(&super::render::render(Some(snapshot), Rect::new(0, 0, 160, 30), None).buffer)
            .join("\n")
    };
    let rendered = text(&snapshot);
    assert!(rendered.contains("Document Design (https://example.com/lib) · Brief · Active"));
    assert!(rendered.find("Document Design").unwrap() < rendered.find("PR Review").unwrap());
    assert!(rendered.find("PR Review").unwrap() < rendered.find("Issue Track work").unwrap());
    assert!(rendered.find("Issue Track work").unwrap() < rendered.find("Link Dashboard").unwrap());
    assert!(rendered.contains("Dependency · Fulfilled"));
    assert!(rendered.contains("Background · Active"));
    snapshot.records.session.checkout =
        Some(Checkout::new("https://example.com/lib", "main", ".").unwrap());
    assert!(text(&snapshot).contains("Document Design · Brief · Active"));
    let mut other = document_reference("other.md", "Other");
    document_mut(&mut other).repository_url = "ssh://another/repository".into();
    snapshot.records.stream.references.push(other);
    snapshot.context.documents.push(DocumentLocation {
        repository_url: "ssh://another/repository".into(),
        path: "other.md".into(),
        location: ResolvedDocument::Unavailable {
            selection: None,
            problem: DocumentProblem::MissingCheckout,
        },
    });
    let rendered = text(&snapshot);
    assert!(rendered.contains("Document Design (https://example.com/lib)"));
    assert!(rendered.contains("Document Other (ssh://another/repository)"));
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
    card.records.stream.references.clear();
    card.context.documents.clear();
    card.records.stream.references = vec![url_reference(
        "pr",
        "https://example.com/org/repo/pull/12",
        "Naming settings",
    )];
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
    let painted = url_reference("issue", "https://EXAMPLE.com/work", "Issue");
    let equivalent = url_reference("issue", "https://example.com/work", "Issue");
    original.records.stream.references = vec![painted.clone(), equivalent.clone()];
    let rendered = super::render::render(Some(&original), Rect::new(0, 0, 100, 30), None);
    assert_eq!(rendered.links.len(), 2);
    let painted = rendered.links[0].1.clone();
    assert_eq!(reference_url_text(&painted), "https://EXAMPLE.com/work");
    assert!(original.contains_link(&painted));
    assert!(original.contains_link(&equivalent));
    let mut refreshed = original.clone();
    refreshed.records.stream.references[0].state = ReferenceState::Dropped;
    assert!(!refreshed.contains_link(&painted));
    assert!(refreshed.contains_link(&equivalent));
    for field in ["name", "purpose", "kind", "role", "state"] {
        let mut changed = original.clone();
        let reference = &mut changed.records.stream.references[0];
        match field {
            "name" => reference.name = "New name".into(),
            "purpose" => reference.purpose = "New purpose".into(),
            "kind" => {
                reference.target = ReferenceTarget::Link {
                    url: "https://EXAMPLE.com/work".into(),
                }
            }
            "role" => reference.role = crate::model::ReferenceRole::Background,
            _ => reference.state = ReferenceState::Fulfilled,
        }
        assert!(!changed.contains_link(&painted), "{field}");
    }
    let mut unsafe_reference = painted;
    unsafe_reference.target = ReferenceTarget::Link {
        url: "file:///tmp/private".into(),
    };
    refreshed
        .records
        .stream
        .references
        .push(unsafe_reference.clone());
    assert!(!refreshed.contains_link(&unsafe_reference));
}

#[test]
fn card_text_goldens_cover_layout_actions_lifecycles_and_empty_blocks() {
    let goldens: Vec<CardGolden> =
        serde_json::from_str(include_str!("fixtures/card-text-renderer.json")).unwrap();
    let cases = card_text_cases();
    if std::env::var_os("STREAMS_UPDATE_CARD_GOLDENS").is_some() {
        let mut values: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("fixtures/card-text-renderer.json")).unwrap();
        for ((name, snapshot, width, height, notice), value) in cases.iter().zip(&mut values) {
            assert_eq!(value["name"], *name);
            let rendered = super::render::render(
                snapshot.as_ref(),
                Rect::new(0, 0, *width, *height),
                notice.as_deref(),
            );
            value["lines"] = serde_json::json!(lines(&rendered.buffer));
            value["documents"] = serde_json::json!(rendered.documents.keys().collect::<Vec<_>>());
            value["links"] = serde_json::json!(
                rendered
                    .links
                    .iter()
                    .map(|(_, reference, _)| reference_url_text(reference))
                    .collect::<Vec<_>>()
            );
        }
        std::fs::write(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/tui/fixtures/card-text-renderer.json"),
            serde_json::to_string_pretty(&values).unwrap() + "\n",
        )
        .unwrap();
        return;
    }
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
                .map(|(_, reference, _)| reference_url_text(reference).to_owned())
                .collect::<Vec<_>>(),
            expected.links,
            "{name} links"
        );
        for (key, target) in rendered.documents {
            let snapshot = snapshot.as_ref().unwrap();
            let index = super::render::DOCUMENT_KEYS.find(key).unwrap();
            assert_eq!(
                target,
                Target::Document(Box::new(
                    snapshot.target(&snapshot.records.stream.references[index])
                )),
                "{name} target {key}"
            );
        }
    }
}

#[test]
fn artifacts_render_from_session_metadata_with_exact_fresh_shortcuts() {
    let mut snapshot = snapshot();
    snapshot.records.stream.references.clear();
    snapshot.context.documents.clear();
    snapshot.records.session.artifacts.insert(
        snapshot.records.stream.id.clone(),
        vec![crate::artifacts::ArtifactEntry {
            name: "assessment.md".into(),
            description: "Investigation findings".into(),
        }],
    );
    let expected =
        crate::artifacts::summaries(&snapshot.records.session, &snapshot.records.stream.id)
            .remove(0);
    let mut view = view(snapshot.clone());
    let frame = view.paint(Rect::new(0, 0, 100, 20)).clone();
    let text = lines(&frame.buffer).join("\n");
    assert!(text.contains("assessment.md"));
    assert!(text.contains("Investigation findings"));
    let action = frame.documents[&'a'].clone();
    assert_eq!(action.target(), &Target::Artifact(Box::new(expected)));
    assert!(view.is_current(&action));
    view.begin_refresh();
    assert!(!view.is_current(&action));
    assert!(view.paint(Rect::new(0, 0, 100, 20)).documents.is_empty());
    snapshot.records.session.membership_epoch += 1;
    snapshot.records.session.stream_id = "destination".into();
    snapshot.records.stream.id = "destination".into();
    let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, 100, 20), None);
    assert!(!lines(&rendered.buffer).join("\n").contains("assessment.md"));
}

#[test]
fn artifact_styles_do_not_interpret_slashes_in_descriptions_as_paths() {
    let mut snapshot = snapshot();
    snapshot.records.session.artifacts.insert(
        snapshot.records.stream.id.clone(),
        vec![
            crate::artifacts::ArtifactEntry {
                name: "brief.md".into(),
                description: "Use artifacts instead of /tmp files".into(),
            },
            crate::artifacts::ArtifactEntry {
                name: "evidence.md".into(),
                description: "Startup evidence without a slash".into(),
            },
        ],
    );
    let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, 160, 20), None);
    let text = lines(&rendered.buffer);
    let position = |needle: &str| {
        text.iter()
            .enumerate()
            .find_map(|(y, line)| line.find(needle).map(|x| (x as u16, y as u16)))
            .unwrap_or_else(|| panic!("Missing {needle:?} in {text:?}"))
    };
    for key in ["[b]", "[c]"] {
        let (x, y) = position(key);
        for dx in 0..3 {
            assert_eq!(rendered.buffer[(x + dx, y)].fg, Color::Green);
        }
    }
    for name in ["brief.md", "evidence.md"] {
        let (x, y) = position(name);
        for dx in 0..name.len() as u16 {
            assert_eq!(rendered.buffer[(x + dx, y)].fg, Color::Reset);
            assert!(
                !rendered.buffer[(x + dx, y)]
                    .modifier
                    .intersects(Modifier::DIM | Modifier::BOLD)
            );
        }
    }
    for description in [
        "Use artifacts instead of /tmp files",
        "Startup evidence without a slash",
    ] {
        let (x, y) = position(description);
        for dx in 0..description.len() as u16 {
            assert!(
                rendered.buffer[(x + dx, y)]
                    .modifier
                    .contains(Modifier::DIM)
            );
        }
    }
    let (x, y) = position("[a]");
    assert_eq!(rendered.buffer[(x, y)].fg, Color::Cyan);
    let (x, y) = position("Document Design");
    assert_eq!(rendered.buffer[(x, y)].fg, Color::Cyan);
    assert!(!rendered.buffer[(x + 5, y)].modifier.contains(Modifier::DIM));
    assert!(!text.join("\n").contains("Artifact "));
}

#[test]
fn artifact_descriptions_use_available_height_instead_of_a_count_cutoff() {
    for count in [1, 3, 4, 7] {
        let mut snapshot = snapshot();
        snapshot.records.session.artifacts.insert(
            snapshot.records.stream.id.clone(),
            (0..count)
                .map(|i| crate::artifacts::ArtifactEntry {
                    name: format!("file{i}.md"),
                    description: format!("Detail {i}"),
                })
                .collect(),
        );
        snapshot.records.session.artifacts.insert(
            "former-stream".into(),
            (0..4)
                .map(|i| crate::artifacts::ArtifactEntry {
                    name: format!("old{i}.md"),
                    description: "Old output".into(),
                })
                .collect(),
        );
        for width in [16, 40, 120] {
            let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, width, 30), None);
            let text = lines(&rendered.buffer);
            let joined = text.join("\n");
            assert!(!joined.contains("old0.md"));
            assert_eq!(rendered.documents.len(), count + 1);
            for i in 0..count {
                assert!(joined.contains(&format!("file{i}.md")));
                if width >= 40 {
                    assert!(joined.contains(&format!("Detail {i}")));
                }
                let key = super::render::DOCUMENT_KEYS.chars().nth(i + 1).unwrap();
                let Target::Artifact(artifact) = &rendered.documents[&key] else {
                    panic!("Wrong target")
                };
                assert_eq!(artifact.key.name, format!("file{i}.md"));
            }
            assert_eq!(
                text.iter().filter(|line| line.contains("file")).count(),
                count
            );
            assert!(
                text.iter()
                    .all(|line| UnicodeWidthStr::width(line.as_str()) <= width as usize)
            );
        }
    }
}

#[test]
fn artifacts_switch_to_aligned_name_columns_only_when_description_rows_do_not_fit() {
    let mut snapshot = snapshot();
    snapshot.records.stream.goal = Some("Goal".into());
    snapshot.records.session.recap.clear();
    snapshot.records.session.state.clear();
    snapshot.records.session.remaining.clear();
    snapshot
        .records
        .stream
        .references
        .retain(|r| matches!(r.target, ReferenceTarget::Document(_)));
    snapshot
        .records
        .stream
        .references
        .push(document_reference("docs/longer-guide.md", "Guide"));
    snapshot.context.documents.push(DocumentLocation {
        repository_url: "https://example.com/lib".into(),
        path: "docs/longer-guide.md".into(),
        location: ResolvedDocument::File {
            selection: CheckoutSelection {
                checkout_id: "lib".into(),
                root: "/code/library".into(),
            },
            file: "/code/library/docs/longer-guide.md".into(),
        },
    });
    snapshot.records.session.artifacts.insert(
        snapshot.records.stream.id.clone(),
        (0..4)
            .map(|i| crate::artifacts::ArtifactEntry {
                name: format!("file{i}.md"),
                description: format!("Detail {i}: {}", "long description ".repeat(6)),
            })
            .collect(),
    );
    for (height, notice, descriptions) in [
        (9, None, true),
        (7, None, false),
        (6, None, false),
        (10, None, true),
        (8, Some("Diagnostic"), false),
    ] {
        let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, 100, height), notice);
        let text = lines(&rendered.buffer);
        let joined = text.join("\n");
        assert!(text[0].contains("Goal"));
        assert!(!joined.contains("more; enlarge"));
        assert_eq!(rendered.documents.len(), 6);
        for i in 0..4 {
            assert!(joined.contains(&format!("file{i}.md")));
            assert_eq!(
                joined.contains(&format!("Detail {i}")),
                descriptions,
                "{height}, {notice:?}: {text:?}"
            );
        }
        if descriptions {
            assert_eq!(text.iter().filter(|line| line.contains("file")).count(), 4);
            assert!(joined.contains('…'));
        } else {
            let column = |key| text.iter().find_map(|line| line.find(key)).unwrap();
            assert_eq!(column("[c]"), 0);
            assert_eq!(column("[d]"), "file0.md".len() + 6);
            assert!(
                text.iter()
                    .any(|line| line.contains("file0.md") && line.contains("file3.md"))
            );
        }
    }
    snapshot.records.session.recap = "Contribution".into();
    snapshot.records.session.state = "Current position".into();
    snapshot.records.session.remaining = "Next action".into();
    for (height, descriptions) in [(10, false), (13, true)] {
        let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, 100, height), None);
        let text = lines(&rendered.buffer).join("\n");
        for row in [
            "Goal",
            "Session: Contribution",
            "Status: Current position",
            "Remaining: Next action",
        ] {
            assert!(text.contains(row));
        }
        assert_eq!(text.contains("Detail 0"), descriptions);
        assert!(!text.contains("more; enlarge"));
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
fn document_without_a_recovery_ref_is_rendered_and_opens_the_current_file() {
    let mut snapshot = snapshot();
    document_mut(&mut snapshot.records.stream.references[0]).r#ref = None;
    document_mut(&mut snapshot.records.stream.references[0]).commit = None;
    let snapshot = CardSnapshot::new(
        snapshot.records.stream,
        snapshot.records.session,
        snapshot.context,
    )
    .unwrap();
    let reference = &snapshot.records.stream.references[0];
    let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, 160, 30), None);
    assert!(
        lines(&rendered.buffer)
            .iter()
            .any(|line| line.contains(&reference.name))
    );
    let Target::Document(target) = &rendered.documents[&'a'] else {
        panic!("expected document action")
    };
    assert_eq!(
        target.editor_identity().unwrap().file,
        PathBuf::from("/code/library/docs/design.md")
    );
}

#[test]
fn document_recovery_hints_do_not_become_branch_or_commit_labels() {
    let mut snapshot = snapshot();
    document_mut(&mut snapshot.records.stream.references[0]).source_session_id =
        Some("source-session".into());
    for commit in [None, Some("current-123456")] {
        document_mut(&mut snapshot.records.stream.references[0]).commit = commit.map(str::to_owned);
        let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, 160, 30), None);
        let text = lines(&rendered.buffer).join("\n");
        assert!(text.contains("Document Design"));
        assert!(
            !text.contains("(added ")
                && !text.contains("source-session")
                && !text.contains("topic")
        );
        assert_eq!(
            rendered.documents[&'a'],
            Target::Document(Box::new(
                snapshot.target(&snapshot.records.stream.references[0])
            ))
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
            "ref" => {
                document_mut(&mut changed.records.stream.references[0]).r#ref = Some("other".into())
            }
            "commit" => {
                document_mut(&mut changed.records.stream.references[0]).commit =
                    Some("latest-commit".into())
            }
            "source-session" => {
                document_mut(&mut changed.records.stream.references[0]).source_session_id =
                    Some("source".into())
            }
            "url" => {
                document_mut(&mut changed.records.stream.references[0]).repository_url =
                    "https://example.com/other".into();
                changed.context.checkouts[0].repository_url = "https://example.com/other".into();
                changed.context.documents[0].repository_url = "https://example.com/other".into();
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
                changed
                    .records
                    .stream
                    .references
                    .insert(0, document_reference("first.md", "First"));
                changed.context.documents.insert(
                    0,
                    DocumentLocation {
                        repository_url: "https://example.com/lib".into(),
                        path: "first.md".into(),
                        location: ResolvedDocument::Unavailable {
                            selection: None,
                            problem: DocumentProblem::MissingCheckout,
                        },
                    },
                );
            }
            "removed" => {
                changed.records.stream.references.clear();
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
    let mut target = snapshot.target(&snapshot.records.stream.references[0]);
    let identity = target.editor_identity().unwrap();
    assert_eq!(identity.file, PathBuf::from("/code/library/docs/design.md"));
    document_mut(&mut target.reference).r#ref = Some("recovery-branch".into());
    document_mut(&mut target.reference).commit = Some("b2".into());
    document_mut(&mut target.reference).source_session_id = Some("source".into());
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
        assert!(build(s.records.stream.clone(), bad, s.context.clone()).is_ok());
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
fn credential_bearing_urls_have_no_painted_controls() {
    for url in [
        "https://user@example.test/item",
        "https://user:secret@example.test/item",
        "https://:secret@example.test/item",
        "file:///tmp/private",
    ] {
        let mut snapshot = snapshot();
        let mut reference = url_reference("issue", "https://example.test/item", "Issue");
        reference.target = ReferenceTarget::Issue { url: url.into() };
        snapshot.records.stream.references = vec![reference.clone()];
        assert!(!snapshot.contains_link(&reference), "{url}");
        let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, 100, 30), None);
        assert!(rendered.links.is_empty(), "{url}");
        assert!(super::render::web_url(url).is_none(), "{url}");
    }
}

#[test]
fn resource_labels_use_saved_names_not_numbers_from_url_paths() {
    for (kind, pull_request) in [("issues", false), ("merge_requests", true)] {
        let mut snapshot = snapshot();
        let url = format!("https://gitlab.example/group/nested/project/-/{kind}/42/-/{kind}/7");
        snapshot.records.stream.references = vec![url_reference(
            if pull_request { "pr" } else { "issue" },
            &url,
            "artifact",
        )];
        let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, 100, 30), None);
        let text = lines(&rendered.buffer).join("\n");
        assert!(
            text.contains(if pull_request {
                "PR artifact"
            } else {
                "Issue artifact"
            }),
            "{text}"
        );
        assert_eq!(reference_url_text(&rendered.links[0].1), url);
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
