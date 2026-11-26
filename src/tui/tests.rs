use std::path::PathBuf;

use super::*;
use crate::model::{
    ArtifactEntry, Goal, ReferenceEntry, ReferenceKind, ReferenceRole, ReferenceStatus, Session,
    Stream, TextPair, Title,
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
    stream: Stream,
    session: Session,
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
struct LocationDto {
    path: String,
    root: Option<PathBuf>,
    file: Option<PathBuf>,
    error: Option<String>,
}
impl LocationDto {
    fn resolved(self) -> ResolvedDocument {
        match self.error {
            None => ResolvedDocument::File {
                root: self.root.unwrap(),
                file: self.file.unwrap(),
            },
            Some(reason) => ResolvedDocument::Unavailable { reason },
        }
    }
}
impl SnapshotDto {
    fn snapshot(self) -> CardSnapshot {
        assert_eq!(self.scope.host_id, self.session.host_id);
        assert_eq!(self.scope.stream_id, self.stream.id);
        assert_eq!(self.scope.session_id, self.session.id);
        assert_eq!(self.scope.native_session_id, self.session.id);
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
                documents: self
                    .locations
                    .into_iter()
                    .map(|l| DocumentLocation {
                        path: l.path.clone(),
                        location: l.resolved(),
                    })
                    .collect(),
            },
        )
        .unwrap()
    }
}

fn doc_entry(name: &str) -> ReferenceEntry {
    ReferenceEntry {
        kind: ReferenceKind::Document,
        name: name.into(),
        purpose: "Guide the work".into(),
        role: ReferenceRole::Brief,
        status: ReferenceStatus::Active,
        manual: false,
    }
}

fn url_entry(kind: ReferenceKind, name: &str) -> ReferenceEntry {
    ReferenceEntry {
        kind,
        name: name.into(),
        purpose: name.into(),
        role: ReferenceRole::Deliverable,
        status: ReferenceStatus::Active,
        manual: false,
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

#[test]
fn reference_minima_reserve_later_headers_before_earlier_purpose_continuations() {
    let mut snapshot = snapshot();
    snapshot.records.stream.goal = Some(Goal::new("Goal", None, false));
    snapshot.records.session.recap = None;
    snapshot.records.session.next = None;
    snapshot.context.documents.clear();
    snapshot.records.session.references.clear();
    for (index, name) in ["First", "Second", "Third"].into_iter().enumerate() {
        let mut entry = url_entry(ReferenceKind::Link, name);
        entry.purpose = "A long purpose with several wrapped continuations to allocate".into();
        snapshot
            .records
            .session
            .references
            .insert(format!("https://example.test/{index}"), entry);
    }
    for (height, visible, hidden) in [(6, 2, 1), (7, 3, 0), (8, 3, 0)] {
        let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, 40, height), None);
        let text = lines(&rendered.buffer);
        assert_eq!(rendered.links.len(), visible);
        for (index, name) in ["First", "Second", "Third"].into_iter().enumerate() {
            assert_eq!(
                text.iter()
                    .any(|line| line.contains(&format!("Link {name}"))),
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
    let mut dropped = url_entry(ReferenceKind::Link, "AaaDropped");
    dropped.status = ReferenceStatus::Dropped;
    snapshot
        .records
        .session
        .references
        .insert("https://example.test/dropped".into(), dropped);
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
    snapshot
        .records
        .session
        .references
        .get_mut("docs/design.md")
        .unwrap()
        .status = ReferenceStatus::Dropped;
    snapshot.context.documents.clear();
    snapshot.records.session.artifacts.insert(
        "result.md".into(),
        ArtifactEntry {
            description: "Independent Session output".into(),
        },
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
    assert!(snapshot.document_target("docs/design.md").is_none());
}

#[test]
fn references_order_by_role_then_name_then_key() {
    let mut snapshot = snapshot();
    snapshot
        .records
        .session
        .references
        .get_mut("https://example.com/org/repo/issues/42")
        .unwrap()
        .status = ReferenceStatus::Fulfilled;
    let mut link = url_entry(ReferenceKind::Link, "Dashboard");
    link.role = ReferenceRole::Background;
    snapshot
        .records
        .session
        .references
        .insert("https://example.test/background".into(), link);
    let rendered =
        lines(&super::render::render(Some(&snapshot), Rect::new(0, 0, 160, 30), None).buffer)
            .join("\n");
    // Order: Deliverable (PR Review) -> Dependency (Issue Track work) -> Brief (Document Design) -> Background (Link Dashboard)
    assert!(rendered.find("PR Review").unwrap() < rendered.find("Issue Track work").unwrap());
    assert!(rendered.find("Issue Track work").unwrap() < rendered.find("Document Design").unwrap());
    assert!(rendered.find("Document Design").unwrap() < rendered.find("Link Dashboard").unwrap());
    assert!(rendered.contains("Document Design · Brief · Active"));
    assert!(rendered.contains("Dependency · Fulfilled"));
    assert!(rendered.contains("Background · Active"));
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
    card.records.stream.goal = Some(Goal::new(
        "Help people track work across sessions and use their cards.",
        None,
        false,
    ));
    card.records.session.title = Title::new("Naming", false);
    card.records.session.recap = Some(TextPair::new(
        "Built naming settings and Pi name import.",
        None,
    ));
    card.records.session.next = Some(TextPair::new("Deploy and reload.", None));
    card.records.session.references.clear();
    card.context.documents.clear();
    card.records.session.references.insert(
        "https://example.com/org/repo/pull/12".into(),
        url_entry(ReferenceKind::Pr, "Naming settings"),
    );
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
    for field in ["recap", "next", "all", "goal"] {
        let mut card = card.clone();
        match field {
            "recap" => card.records.session.recap = None,
            "next" => card.records.session.next = None,
            "goal" => card.records.stream.goal = None,
            _ => {
                card.records.session.recap = None;
                card.records.session.next = None;
            }
        }
        cases.push((format!("card-text-empty-{field}"), Some(card), 44, 14, None));
    }
    cases
}

#[test]
fn link_actions_require_fresh_membership_under_the_painters_url_policy() {
    let mut original = snapshot();
    original.records.session.references.clear();
    original.context.documents.clear();
    let entry_a = url_entry(ReferenceKind::Issue, "Issue");
    let entry_b = url_entry(ReferenceKind::Issue, "Issue");
    original
        .records
        .session
        .references
        .insert("https://EXAMPLE.com/work".into(), entry_a.clone());
    original
        .records
        .session
        .references
        .insert("https://example.com/work".into(), entry_b.clone());
    let rendered = super::render::render(Some(&original), Rect::new(0, 0, 100, 30), None);
    assert_eq!(rendered.links.len(), 2);
    let painted = rendered.links[0].1.clone();
    assert_eq!(painted.url, "https://EXAMPLE.com/work");
    let equivalent = LinkTarget {
        url: "https://example.com/work".into(),
        entry: entry_b,
    };
    assert!(original.contains_link(&painted));
    assert!(original.contains_link(&equivalent));
    let mut refreshed = original.clone();
    refreshed
        .records
        .session
        .references
        .get_mut("https://EXAMPLE.com/work")
        .unwrap()
        .status = ReferenceStatus::Dropped;
    assert!(!refreshed.contains_link(&painted));
    assert!(refreshed.contains_link(&equivalent));
    for field in ["name", "purpose", "kind", "role", "status"] {
        let mut changed = original.clone();
        let entry = changed
            .records
            .session
            .references
            .get_mut("https://EXAMPLE.com/work")
            .unwrap();
        match field {
            "name" => entry.name = "New name".into(),
            "purpose" => entry.purpose = "New purpose".into(),
            "kind" => entry.kind = ReferenceKind::Link,
            "role" => entry.role = ReferenceRole::Background,
            _ => entry.status = ReferenceStatus::Fulfilled,
        }
        assert!(!changed.contains_link(&painted), "{field}");
    }
    let unsafe_link = LinkTarget {
        url: "file:///tmp/private".into(),
        entry: url_entry(ReferenceKind::Link, "Private"),
    };
    refreshed
        .records
        .session
        .references
        .insert(unsafe_link.url.clone(), unsafe_link.entry.clone());
    assert!(!refreshed.contains_link(&unsafe_link));
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
                    .map(|(_, link, _)| link.url.as_str())
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
            "{name} documents"
        );
        assert_eq!(
            rendered
                .links
                .iter()
                .map(|(_, link, _)| link.url.clone())
                .collect::<Vec<_>>(),
            expected.links,
            "{name} links"
        );
    }
}

#[test]
fn artifacts_render_from_session_metadata_with_exact_fresh_shortcuts() {
    let mut snapshot = snapshot();
    snapshot.records.session.references.clear();
    snapshot.context.documents.clear();
    snapshot.records.session.artifacts.insert(
        "assessment.md".into(),
        ArtifactEntry {
            description: "Investigation findings".into(),
        },
    );
    let expected = crate::artifacts::summaries(&snapshot.records.session).remove(0);
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
    // Moving a Session keeps its artifacts.
    snapshot.records.session.stream_id = "destination".into();
    snapshot.records.stream.id = "destination".into();
    let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, 100, 20), None);
    assert!(lines(&rendered.buffer).join("\n").contains("assessment.md"));
}

#[test]
fn artifact_styles_do_not_interpret_slashes_in_descriptions_as_paths() {
    let mut snapshot = snapshot();
    snapshot.records.session.artifacts.insert(
        "brief.md".into(),
        ArtifactEntry {
            description: "Use artifacts instead of /tmp files".into(),
        },
    );
    snapshot.records.session.artifacts.insert(
        "evidence.md".into(),
        ArtifactEntry {
            description: "Startup evidence without a slash".into(),
        },
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
        for i in 0..count {
            snapshot.records.session.artifacts.insert(
                format!("file{i}.md"),
                ArtifactEntry {
                    description: format!("Detail {i}"),
                },
            );
        }
        for width in [16, 40, 120] {
            let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, width, 30), None);
            let text = lines(&rendered.buffer);
            let joined = text.join("\n");
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
    snapshot.records.stream.goal = Some(Goal::new("Goal", None, false));
    snapshot.records.session.recap = None;
    snapshot.records.session.next = None;
    snapshot
        .records
        .session
        .references
        .retain(|_, entry| entry.kind == ReferenceKind::Document);
    snapshot
        .records
        .session
        .references
        .insert("docs/longer-guide.md".into(), doc_entry("Guide"));
    snapshot.context.documents.push(DocumentLocation {
        path: "docs/longer-guide.md".into(),
        location: ResolvedDocument::File {
            root: "/code/library".into(),
            file: "/code/library/docs/longer-guide.md".into(),
        },
    });
    for i in 0..4 {
        snapshot.records.session.artifacts.insert(
            format!("file{i}.md"),
            ArtifactEntry {
                description: format!("Detail {i}: {}", "long description ".repeat(6)),
            },
        );
    }
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
    snapshot.records.session.recap = Some(TextPair::new("Contribution", None));
    snapshot.records.session.next = Some(TextPair::new("Next action", None));
    for (height, descriptions) in [(9, false), (12, true)] {
        let rendered = super::render::render(Some(&snapshot), Rect::new(0, 0, 100, height), None);
        let text = lines(&rendered.buffer).join("\n");
        for row in ["Goal", "Recap: Contribution", "Next: Next action"] {
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
    for field in ["root", "file", "error", "order", "removed", "progress"] {
        let mut changed = snapshot();
        let action = view.paint(Rect::new(0, 0, 100, 12)).documents[&'a'].clone();
        match field {
            "root" => {
                changed.context.documents[0].location = ResolvedDocument::File {
                    root: "/relocated".into(),
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
                    reason: "no directory here".into(),
                }
            }
            "order" => {
                changed
                    .records
                    .session
                    .references
                    .insert("first.md".into(), doc_entry("AaaFirst"));
                changed.context.documents.insert(
                    0,
                    DocumentLocation {
                        path: "first.md".into(),
                        location: ResolvedDocument::Unavailable {
                            reason: "no directory here".into(),
                        },
                    },
                );
            }
            "removed" => {
                changed.records.session.references.clear();
                changed.context.documents.clear();
            }
            _ => changed.records.session.next = Some(TextPair::new("Fresh progress", None)),
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
fn domain_and_location_validation_does_not_guess_or_touch_filesystem() {
    let s = snapshot();
    let build = |stream, session, context| CardSnapshot::new(stream, session, context);
    let mut bad = s.records.session.clone();
    bad.stream_id = "another".into();
    assert!(build(s.records.stream.clone(), bad, s.context.clone()).is_err());
    let mut bad = s.context.clone();
    bad.documents.clear();
    assert!(build(s.records.stream.clone(), s.records.session.clone(), bad).is_err());
    let mut bad = s.context.clone();
    bad.documents[0].location = ResolvedDocument::File {
        root: "/code/library".into(),
        file: "/another/checkout/design.md".into(),
    };
    assert!(build(s.records.stream.clone(), s.records.session.clone(), bad).is_err());
    assert!(build(s.records.stream, s.records.session, s.context).is_ok());
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
        let link = LinkTarget {
            url: url.into(),
            entry: url_entry(ReferenceKind::Issue, "Issue"),
        };
        snapshot.records.session.references.clear();
        snapshot.context.documents.clear();
        snapshot
            .records
            .session
            .references
            .insert(link.url.clone(), link.entry.clone());
        assert!(!snapshot.contains_link(&link), "{url}");
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
        snapshot.records.session.references.clear();
        snapshot.context.documents.clear();
        snapshot.records.session.references.insert(
            url.clone(),
            url_entry(
                if pull_request {
                    ReferenceKind::Pr
                } else {
                    ReferenceKind::Issue
                },
                "artifact",
            ),
        );
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
        assert_eq!(rendered.links[0].1.url, url);
    }
}

#[test]
fn unicode_and_styles_are_cell_bounded_without_terminal_sequences() {
    let mut s = snapshot();
    s.records.stream.goal = Some(Goal::new(
        "界😀é\u{1b}[31m\u{202e} literal | pipe 👩‍💻",
        None,
        false,
    ));
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
