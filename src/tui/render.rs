use std::collections::BTreeMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use url::Url;

use super::{CardSnapshot, LinkTarget, Target};
use crate::model::{ReferenceKind, ReferenceRole, ReferenceStatus};

pub(super) const DOCUMENT_KEYS: &str = "abcdefghijklmnoprstuvwxyz";
const DIM: Style = Style::new().add_modifier(Modifier::DIM);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const CYAN: Style = Style::new().fg(Color::Cyan);
const GREEN: Style = Style::new().fg(Color::Green);

pub(super) fn plain(text: &str) -> String {
    text.chars()
        .filter(|&c| {
            !matches!(
                get_general_category(c),
                GeneralCategory::Control | GeneralCategory::Format
            )
        })
        .collect()
}

fn cells(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}
fn prefix(text: &str, width: usize) -> String {
    let mut used = 0;
    text.graphemes(true)
        .take_while(|g| {
            used += cells(g);
            used <= width
        })
        .collect()
}
fn fit(text: &str, width: usize) -> String {
    if width == 0 {
        String::new()
    } else if cells(text) <= width {
        text.into()
    } else {
        prefix(text, width - 1) + "…"
    }
}
fn boundary(text: &str, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }
    text.grapheme_indices(true)
        .map(|(i, g)| i + g.len())
        .find(|&end| end >= offset)
        .unwrap_or(text.len())
}

/// Ratatui owns wrapping and grapheme painting. Oversized glyph replacement preserves
/// the active renderer's one-cell-pane behavior instead of silently dropping glyphs.
fn wrap(text: &str, width: usize, limit: usize) -> Vec<String> {
    if text.trim().is_empty() || width == 0 || limit == 0 {
        return Vec::new();
    }
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = normalized
        .graphemes(true)
        .map(|g| if cells(g) > width { "…" } else { g })
        .collect::<String>();
    let area = Rect::new(0, 0, width as u16, limit.min(u16::MAX as usize) as u16);
    let mut buffer = Buffer::empty(area);
    Paragraph::new(normalized)
        .wrap(Wrap { trim: true })
        .render(area, &mut buffer);
    (0..area.height)
        .map(|y| buffer_line(&buffer, y))
        .take_while(|s| !s.is_empty())
        .collect()
}

/// Only the first goal row reserves the lifecycle marker; work continuations hang by two cells.
fn wrap_block(
    text: &str,
    width: usize,
    first_width: usize,
    indent: usize,
    limit: usize,
) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let first = prefix(&normalized, first_width);
    let end = if first.len() < normalized.len()
        && !normalized[first.len()..].starts_with(' ')
        && let Some(space) = first.rfind(' ')
    {
        space
    } else {
        first.len()
    };
    let (end, first) = if end == 0 && !normalized.is_empty() {
        (normalized.graphemes(true).next().unwrap().len(), "…".into())
    } else {
        (end, first[..end].trim_end().to_owned())
    };
    let mut rows = vec![first];
    let indent = indent.min(width.saturating_sub(1));
    rows.extend(
        wrap(normalized[end..].trim_start(), width - indent, limit - 1)
            .into_iter()
            .map(|line| format!("{}{line}", " ".repeat(indent))),
    );
    rows
}

pub(super) fn buffer_line(buffer: &Buffer, y: u16) -> String {
    let mut line = String::new();
    let mut x = buffer.area.x;
    while x < buffer.area.right() {
        let symbol = buffer[(x, y)].symbol();
        line.push_str(symbol);
        x += (cells(symbol).max(1) as u16).min(buffer.area.right() - x);
    }
    line.trim_end().to_owned()
}

pub(super) fn web_url(value: &str) -> Option<Url> {
    crate::model::reference_url(value)
}
#[derive(Clone)]
struct Entry {
    label: String,
    target: Target,
    key: Option<char>,
}
#[derive(Clone)]
enum Row {
    Text(String, Style),
    State {
        text: String,
        heading: usize,
        goal: bool,
        status: Option<(String, Style)>,
    },
    Documents {
        entries: Vec<Entry>,
        widths: Vec<usize>,
    },
    Link {
        label: String,
        link: Option<LinkTarget>,
        index: usize,
    },
}
impl Row {
    fn items(&self) -> usize {
        match self {
            Self::Documents { entries, .. } => entries.len(),
            _ => 1,
        }
    }
}

pub(super) struct Rendering {
    pub buffer: Buffer,
    pub documents: BTreeMap<char, Target>,
    pub links: Vec<(usize, LinkTarget, Rect)>,
}

struct ReferenceBlock {
    header: Row,
    purpose: Vec<String>,
}

fn reference_blocks(snapshot: &CardSnapshot, width: usize) -> Vec<ReferenceBlock> {
    let mut visible: Vec<_> = snapshot
        .session()
        .references
        .iter()
        .filter(|(_, entry)| entry.status != ReferenceStatus::Dropped)
        .collect();
    // Cards and the app list entries by role, then name, then key.
    visible.sort_by(|(key_a, a), (key_b, b)| {
        a.role
            .cmp(&b.role)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| key_a.cmp(key_b))
    });
    let mut blocks = Vec::new();
    let mut document_index = 0;
    let mut url_index = 0;
    for (ref_key, entry) in visible {
        let kind = match entry.kind {
            ReferenceKind::Document => "Document",
            ReferenceKind::Pr => "PR",
            ReferenceKind::Issue => "Issue",
            ReferenceKind::Link => "Link",
        };
        let role = match entry.role {
            ReferenceRole::Deliverable => "Deliverable",
            ReferenceRole::Dependency => "Dependency",
            ReferenceRole::Brief => "Brief",
            ReferenceRole::Background => "Background",
        };
        let status = match entry.status {
            ReferenceStatus::Active => "Active",
            ReferenceStatus::Fulfilled => "Fulfilled",
            ReferenceStatus::Dropped => unreachable!(),
        };
        let label = plain(&format!("{kind} {} · {role} · {status}", entry.name));
        let header = match entry.kind {
            ReferenceKind::Document => {
                let key = DOCUMENT_KEYS.chars().nth(document_index);
                document_index += 1;
                Row::Documents {
                    entries: vec![Entry {
                        label,
                        key,
                        target: Target::Document(Box::new(snapshot.target(ref_key, entry))),
                    }],
                    widths: vec![width],
                }
            }
            ReferenceKind::Pr | ReferenceKind::Issue | ReferenceKind::Link => {
                url_index += 1;
                Row::Link {
                    label: format!("[{url_index}] {label}"),
                    link: web_url(ref_key).map(|_| LinkTarget {
                        url: ref_key.clone(),
                        entry: entry.clone(),
                    }),
                    index: url_index,
                }
            }
        };
        let purpose_text = plain(&entry.purpose);
        let mut purpose = wrap(
            &purpose_text,
            width.saturating_sub(4).max(1),
            purpose_text.len().saturating_add(1).min(u16::MAX as usize),
        );
        if purpose.is_empty() {
            purpose.push(String::new());
        }
        blocks.push(ReferenceBlock { header, purpose });
    }
    blocks
}

pub(super) fn render(
    snapshot: Option<&CardSnapshot>,
    area: Rect,
    notice: Option<&str>,
) -> Rendering {
    let mut result = Rendering {
        buffer: Buffer::empty(area),
        documents: BTreeMap::new(),
        links: Vec::new(),
    };
    let width = area.width as usize;
    let height = area.height as usize;
    if width == 0 || height == 0 {
        return result;
    }
    let content_height = height - usize::from(notice.is_some());
    let footer = if width >= 9 {
        "[q] close"
    } else if width >= 3 {
        "[q]"
    } else {
        ""
    };
    let mut rows = Vec::new();
    if let Some(snapshot) = snapshot.filter(|_| content_height > 0) {
        let blocks = reference_blocks(snapshot, width);
        let document_count = snapshot
            .session()
            .references
            .values()
            .filter(|entry| {
                entry.status != ReferenceStatus::Dropped && entry.kind == ReferenceKind::Document
            })
            .count();
        let artifacts = crate::artifacts::summaries(snapshot.session());
        let mut entries = Vec::new();
        for artifact in artifacts {
            let key = DOCUMENT_KEYS.chars().nth(document_count + entries.len());
            entries.push(Entry {
                label: plain(&artifact.key.name),
                target: Target::Artifact(Box::new(artifact)),
                key,
            });
        }
        let mut columns = 1;
        let mut widths = vec![width];
        for count in (2..=entries.len().min((width + 2) / 2)).rev() {
            let mut candidate = vec![0; count];
            for (i, entry) in entries.iter().enumerate() {
                let length = cells(&entry.label) + if entry.key.is_some() { 4 } else { 0 };
                candidate[i % count] = candidate[i % count].max(length);
            }
            if candidate.iter().sum::<usize>() + 2 * (count - 1) <= width {
                columns = count;
                widths = candidate;
                break;
            }
        }
        let minimum_height = blocks.len() * 2 + entries.len().div_ceil(columns);
        let status = snapshot.session().status;
        let (symbol, style) = match status {
            crate::model::Status::Active => ("●", CYAN),
            crate::model::Status::Waiting => ("○", Style::new().fg(Color::Yellow)),
            crate::model::Status::Blocked => ("!", Style::new().fg(Color::Red)),
            crate::model::Status::Parked => ("○", DIM),
            crate::model::Status::Done => ("✓", Style::new().fg(Color::Green)),
            crate::model::Status::Closed => ("×", DIM),
        };
        let status = format!("{symbol} {status}");
        let status_width = cells(&status);
        let goal = plain(
            snapshot
                .stream()
                .goal
                .as_ref()
                .map(|g| g.text.as_str())
                .unwrap_or_default(),
        )
        .trim()
        .to_owned();
        let recap = plain(
            snapshot
                .session()
                .recap
                .as_ref()
                .map(|p| p.text.as_str())
                .unwrap_or_default(),
        )
        .trim()
        .to_owned();
        let next = plain(
            snapshot
                .session()
                .next
                .as_ref()
                .map(|p| p.text.as_str())
                .unwrap_or_default(),
        )
        .trim()
        .to_owned();
        let mut sections = vec![("", goal, true)];
        for (label, body) in [("Recap", recap), ("Next", next)] {
            if !body.is_empty() {
                sections.push((label, body, false));
            }
        }
        let mut budget = if minimum_height == 0 || content_height < 2 {
            content_height
        } else {
            content_height
                .div_ceil(2)
                .min(content_height.saturating_sub(minimum_height.min(2)).max(1))
        };
        budget = budget.max(
            sections
                .len()
                .min(content_height.saturating_sub(minimum_height.min(2))),
        );
        let mut goal_end = 0;
        let mut work_end = 0;
        for (index, (label, body, goal)) in sections.iter().enumerate() {
            if budget == 0 {
                break;
            }
            let first_width = if index == 0 && width >= status_width + 3 {
                width - status_width - 2
            } else {
                width
            };
            let heading = format!(
                "{label}{}",
                if !label.is_empty() && !body.is_empty() {
                    ":"
                } else {
                    ""
                }
            );
            let text = format!(
                "{heading}{}{body}",
                if !heading.is_empty() && !body.is_empty() {
                    " "
                } else {
                    ""
                }
            );
            let mut state = wrap_block(
                &text,
                width,
                first_width,
                if *goal { 0 } else { 2 },
                budget + 1,
            );
            let reserved = (sections.len() - index - 1).min(budget - 1);
            let visible = state.len().min(budget - reserved);
            if state.len() > visible || (budget == visible && index < sections.len() - 1) {
                let last_width = if visible == 1 { first_width } else { width };
                state[visible - 1] = prefix(&state[visible - 1], last_width - 1)
                    .trim_end()
                    .to_owned()
                    + "…";
            }
            for (row, text) in state.into_iter().take(visible).enumerate() {
                let heading = boundary(
                    &text,
                    if row == 0 {
                        heading.len().min(text.len())
                    } else {
                        0
                    },
                );
                rows.push(Row::State {
                    text,
                    heading,
                    goal: *goal,
                    status: (index == 0 && row == 0 && width >= status_width + 3)
                        .then(|| (status.clone(), style)),
                });
            }
            budget -= visible;
            if *goal {
                goal_end = rows.len();
            } else {
                work_end = rows.len();
            }
        }
        let slots = content_height - rows.len();
        // Select whole minimum blocks before assigning purpose continuation.
        let descriptions = blocks.len() * 2 + entries.len() <= slots;
        let artifact_rows: Vec<_> = if descriptions {
            entries
                .into_iter()
                .map(|mut entry| {
                    if let Target::Artifact(artifact) = &entry.target {
                        entry.label =
                            plain(&format!("{}: {}", artifact.key.name, artifact.description));
                    }
                    Row::Documents {
                        entries: vec![entry],
                        widths: vec![width],
                    }
                })
                .collect()
        } else {
            entries
                .chunks(columns)
                .map(|entries| Row::Documents {
                    entries: entries.to_vec(),
                    widths: widths.clone(),
                })
                .collect()
        };
        let minimum = blocks.len() * 2 + artifact_rows.len();
        let available = slots.saturating_sub(usize::from(minimum > slots));
        let selected_references = blocks.len().min(available / 2);
        let selected_artifacts = if selected_references == blocks.len() {
            artifact_rows.len().min(available - selected_references * 2)
        } else {
            0
        };
        let mut spare = available - selected_references * 2 - selected_artifacts;
        let hidden = blocks.len() - selected_references
            + artifact_rows[selected_artifacts..]
                .iter()
                .map(Row::items)
                .sum::<usize>();
        for block in blocks.into_iter().take(selected_references) {
            rows.push(block.header);
            let count = block.purpose.len().min(1 + spare);
            spare -= count - 1;
            let truncated = count < block.purpose.len();
            for (index, mut purpose) in block.purpose.into_iter().take(count).enumerate() {
                if truncated && index + 1 == count {
                    purpose = prefix(&purpose, width.saturating_sub(4).max(1).saturating_sub(1))
                        .trim_end()
                        .to_owned()
                        + "…";
                }
                rows.push(Row::Text(
                    format!("{}{purpose}", " ".repeat(width.min(4))),
                    Style::new(),
                ));
            }
        }
        rows.extend(artifact_rows.into_iter().take(selected_artifacts));
        if hidden > 0 && rows.len() < content_height {
            rows.push(Row::Text(format!("… {hidden} more; enlarge pane"), DIM));
        }
        let mut boundaries = Vec::new();
        let body_end = work_end.max(goal_end);
        if body_end > 0 && rows.len() > body_end {
            boundaries.push(body_end);
        }
        if goal_end > 0 && work_end > goal_end {
            boundaries.push(goal_end);
        }
        boundaries.truncate(content_height - rows.len());
        boundaries.sort_unstable_by(|a, b| b.cmp(a));
        for boundary in boundaries {
            rows.insert(boundary, Row::Text("─".repeat(width), DIM));
        }
    }
    while rows.len() < content_height {
        rows.push(Row::Text(String::new(), Style::new()));
    }
    if let Some(notice) = notice {
        rows.push(Row::Text(plain(notice), Style::new()));
    }
    for (row, line) in rows.into_iter().enumerate() {
        let hint = if row == height - 1 { footer } else { "" };
        let room = width - cells(hint);
        let y = area.y + row as u16;
        let mut spans = Vec::new();
        match line {
            Row::Text(text, style) => spans.push(Span::styled(prefix(&text, room), style)),
            Row::State {
                text,
                heading,
                goal,
                status,
            } => {
                // The footer can hide status even though the full-width section reserved it.
                let status = status.filter(|(status, _)| room >= cells(status) + 3);
                let text = prefix(
                    &text,
                    room - status.as_ref().map_or(0, |(s, _)| cells(s) + 2),
                );
                let heading = boundary(&text, heading.min(text.len()));
                if goal {
                    spans.push(Span::styled(text.clone(), DIM));
                } else {
                    spans.push(Span::styled(text[..heading].to_owned(), BOLD));
                    spans.push(Span::raw(text[heading..].to_owned()));
                }
                if let Some((status, style)) = status {
                    spans.push(Span::raw(" ".repeat(room - cells(&text) - cells(&status))));
                    spans.push(Span::styled(status, style));
                }
            }
            Row::Documents { entries, widths } => {
                let mut offset = 0;
                let mut painted = 0;
                for (column, entry) in entries.into_iter().enumerate() {
                    if offset >= room {
                        break;
                    }
                    spans.push(Span::raw(" ".repeat(offset - painted)));
                    let available = widths[column].min(room - offset);
                    let key = entry.key.map(|key| format!("[{key}]")).unwrap_or_default();
                    let key_label = prefix(&key, available);
                    let space = if !key_label.is_empty() && available > cells(&key_label) {
                        " "
                    } else {
                        ""
                    };
                    let label = fit(&entry.label, available - cells(&key_label) - space.len());
                    let (key_style, split, leading, trailing) =
                        if let Target::Artifact(artifact) = &entry.target {
                            (
                                GREEN,
                                artifact.key.name.len().min(label.len()),
                                Style::new(),
                                DIM,
                            )
                        } else {
                            (CYAN, 0, CYAN, CYAN)
                        };
                    let split = boundary(&label, split);
                    if !key.is_empty() && key_label == key {
                        result.documents.insert(entry.key.unwrap(), entry.target);
                    }
                    painted = offset + cells(&key_label) + space.len() + cells(&label);
                    spans.extend([
                        Span::styled(key_label, key_style),
                        Span::raw(space),
                        Span::styled(label[..split].to_owned(), leading),
                        Span::styled(label[split..].to_owned(), trailing),
                    ]);
                    offset += widths[column] + 2;
                }
            }
            Row::Link { label, link, index } => {
                let text = fit(&label, room);
                if !text.is_empty()
                    && let Some(link) = link
                {
                    result
                        .links
                        .push((index, link, Rect::new(area.x, y, cells(&text) as u16, 1)));
                }
                spans.push(Span::styled(text, CYAN));
            }
        }
        let painted = spans.iter().map(|s| cells(&s.content)).sum::<usize>();
        if !hint.is_empty() {
            spans.extend([
                Span::raw(" ".repeat(room - painted)),
                Span::styled(hint, DIM),
            ]);
        }
        Line::from(spans).render(Rect::new(area.x, y, area.width, 1), &mut result.buffer);
    }
    result
}
