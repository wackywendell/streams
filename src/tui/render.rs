use std::collections::BTreeMap;

use percent_encoding::percent_decode_str;
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

use super::{CardSnapshot, DocumentTarget};

const DOCUMENT_KEYS: &str = "abcdefghijklmnoprstuvwxyz";
const DIM: Style = Style::new().add_modifier(Modifier::DIM);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const CYAN: Style = Style::new().fg(Color::Cyan);

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
    if value.chars().any(|c| {
        c.is_whitespace()
            || c == '\\'
            || matches!(
                get_general_category(c),
                GeneralCategory::Control | GeneralCategory::Format
            )
    }) {
        return None;
    }
    // decodeURIComponent rejects malformed escapes and invalid UTF-8; percent_decode
    // deliberately does not reject malformed escapes, so check the escape shape first.
    let bytes = value.as_bytes();
    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b'%'
            && !(bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit))
        {
            return None;
        }
    }
    let decoded = percent_decode_str(value).decode_utf8().ok()?;
    if decoded.chars().any(|c| {
        matches!(
            get_general_category(c),
            GeneralCategory::Control | GeneralCategory::Format
        )
    }) {
        return None;
    }
    let url = Url::parse(value).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).then_some(url)
}
fn artifact_number(url: &Url, pr: bool) -> Option<&str> {
    let parts: Vec<_> = url.path_segments()?.collect();
    let kind = if pr { "merge_requests" } else { "issues" };
    let index = if parts.len() >= 4
        && !parts[0].is_empty()
        && !parts[1].is_empty()
        && (parts[2] == "issues" && !pr || pr && matches!(parts[2], "pull" | "pulls"))
    {
        2
    } else {
        // GitLab permits nested groups and suffix paths after the artifact number.
        let project_end = parts.windows(2).position(|pair| pair == ["-", kind])?;
        if project_end == 0 || parts[..project_end].iter().any(|part| part.is_empty()) {
            return None;
        }
        project_end + 1
    };
    if parts[index] != kind && !(pr && matches!(parts[index], "pull" | "pulls")) {
        return None;
    }
    let number = *parts.get(index + 1)?;
    (!number.starts_with('0') && !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
        .then_some(number)
}

#[derive(Clone)]
struct Entry {
    label: String,
    target: DocumentTarget,
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
        url: Option<Url>,
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
    pub documents: BTreeMap<char, DocumentTarget>,
    pub links: Vec<(usize, Url, Rect)>,
}

fn root_labels(roots: &[&str]) -> Vec<String> {
    let parts: Vec<_> = roots
        .iter()
        .map(|root| {
            root.split('/')
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .collect();
    let mut depths = vec![1; roots.len()];
    loop {
        let labels: Vec<_> = parts
            .iter()
            .zip(&depths)
            .zip(roots)
            .map(|((parts, &depth), root)| {
                if depth > parts.len() {
                    root.to_string()
                } else {
                    parts[parts.len() - depth..].join("/")
                }
            })
            .collect();
        let collisions: Vec<_> = (0..roots.len())
            .filter(|&i| (0..roots.len()).any(|j| i != j && labels[i] == labels[j]))
            .collect();
        if collisions.is_empty() {
            return labels;
        }
        for i in collisions {
            depths[i] += 1;
        }
    }
}

fn document_entries(snapshot: &CardSnapshot) -> Vec<Entry> {
    let mut roots = Vec::new();
    for binding in &snapshot.context.checkouts {
        let root = binding
            .selection
            .root
            .to_str()
            .expect("validated UTF-8 checkout");
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    let labels = root_labels(&roots);
    snapshot
        .stream
        .documents
        .iter()
        .enumerate()
        .map(|(index, doc)| {
            let target = snapshot.target(doc);
            let root = target.location.selection().and_then(|s| s.root.to_str());
            let qualifier = if snapshot.stream.repositories.len() > 1 || roots.len() > 1 {
                if let Some(root) = root {
                    let label = &labels[roots
                        .iter()
                        .position(|&r| r == root)
                        .expect("validated binding")];
                    if snapshot.context.checkouts.iter().any(|b| {
                        b.repository != doc.repository && b.selection.root.to_str() == Some(root)
                    }) {
                        format!("{}:{label}", doc.repository)
                    } else {
                        label.clone()
                    }
                } else {
                    doc.repository.clone()
                }
            } else {
                String::new()
            };
            let qualifier = if qualifier.is_empty() || qualifier == "/" {
                String::new()
            } else {
                qualifier + "/"
            };
            let added = doc
                .commit
                .as_deref()
                .map(|s| format!(" (added {})", s.chars().take(8).collect::<String>()))
                .unwrap_or_default();
            Entry {
                label: plain(&format!("{qualifier}{}{added}", doc.path)),
                target,
                key: DOCUMENT_KEYS.chars().nth(index),
            }
        })
        .collect()
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
        let entries = document_entries(snapshot);
        let lengths: Vec<_> = entries
            .iter()
            .map(|e| cells(&e.label) + if e.key.is_some() { 4 } else { 0 })
            .collect();
        let mut columns = 1;
        let mut widths = vec![width];
        for count in (2..=entries.len().min((width + 2) / 2)).rev() {
            let mut candidate = vec![0; count];
            for (i, &length) in lengths.iter().enumerate() {
                candidate[i % count] = candidate[i % count].max(length);
            }
            if candidate.iter().sum::<usize>() + 2 * (count - 1) <= width {
                columns = count;
                widths = candidate;
                break;
            }
        }
        let mut references: Vec<Row> = entries
            .chunks(columns)
            .map(|entries| Row::Documents {
                entries: entries.to_vec(),
                widths: widths.clone(),
            })
            .collect();
        for (index, (link, pr)) in snapshot
            .stream
            .issues
            .iter()
            .map(|l| (l, false))
            .chain(snapshot.stream.prs.iter().map(|l| (l, true)))
            .enumerate()
        {
            let url = web_url(&link.url);
            let number = url
                .as_ref()
                .and_then(|url| artifact_number(url, pr))
                .map(|n| format!(" #{n}"))
                .unwrap_or_default();
            let kind = if pr { "PR" } else { "Issue" };
            let description = if link.description.is_empty() {
                &link.url
            } else {
                &link.description
            };
            references.push(Row::Link {
                label: plain(&format!("[{}] {kind}{number}: {description}", index + 1)),
                url,
                index: index + 1,
            });
        }
        let status = snapshot.session.status;
        let (symbol, style) = match status {
            crate::model::Status::Active => ("●", CYAN),
            crate::model::Status::Waiting => ("○", Style::new().fg(Color::Yellow)),
            crate::model::Status::Blocked => ("!", Style::new().fg(Color::Red)),
            crate::model::Status::Parked => ("○", DIM),
            crate::model::Status::Done => ("✓", Style::new().fg(Color::Green)),
        };
        let status = format!("{symbol} {status}");
        let status_width = cells(&status);
        let goal = plain(snapshot.stream.goal.as_deref().unwrap_or_default())
            .trim()
            .to_owned();
        let title = plain(&snapshot.session.title).trim().to_owned();
        let now = plain(&snapshot.session.state).trim().to_owned();
        let remaining = plain(&snapshot.session.remaining).trim().to_owned();
        let mut sections = Vec::new();
        if !goal.is_empty() {
            sections.push((String::new(), goal, false, true));
        }
        sections.push((title, now, true, false));
        if !remaining.is_empty() {
            sections.push(("Remaining".into(), remaining, false, false));
        }
        let mut budget = if references.is_empty() || content_height < 2 {
            content_height
        } else {
            content_height.div_ceil(2).min(
                content_height
                    .saturating_sub(references.len().min(2))
                    .max(1),
            )
        };
        budget = budget.max(
            sections
                .len()
                .min(content_height.saturating_sub(references.len().min(2))),
        );
        let mut goal_end = 0;
        let mut work_end = 0;
        for (index, (label, body, own_status, goal)) in sections.iter().enumerate() {
            if budget == 0 {
                break;
            }
            let section_width = if *own_status && width >= status_width + 3 {
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
            let mut state = wrap(&text, section_width, budget + 1);
            if state.is_empty() && *own_status {
                state.push(String::new());
            }
            let headings = wrap(&heading, section_width, budget + 1);
            let reserved = (sections.len() - index - 1).min(budget - 1);
            let visible = state.len().min(budget - reserved);
            if state.len() > visible || (budget == visible && index < sections.len() - 1) {
                state[visible - 1] = prefix(&state[visible - 1], section_width - 1)
                    .trim_end()
                    .to_owned()
                    + "…";
            }
            for (row, text) in state.into_iter().take(visible).enumerate() {
                let heading = boundary(&text, headings.get(row).map_or(0, String::len));
                rows.push(Row::State {
                    text,
                    heading,
                    goal: *goal,
                    status: (*own_status && row == 0 && width >= status_width + 3)
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
        let visible = references
            .len()
            .min(slots.saturating_sub(usize::from(references.len() > slots)));
        let hidden = references[visible..].iter().map(Row::items).sum::<usize>();
        rows.extend(references.into_iter().take(visible));
        if hidden > 0 && rows.len() < content_height {
            rows.push(Row::Text(format!("… {hidden} more; enlarge pane"), DIM));
        }
        let mut boundaries = Vec::new();
        if work_end > goal_end && rows.len() > work_end {
            boundaries.push(work_end);
        }
        if goal_end > 0 && work_end > goal_end {
            boundaries.push(goal_end);
        }
        boundaries.truncate(content_height - rows.len());
        boundaries.sort_unstable_by(|a, b| b.cmp(a));
        for boundary in boundaries {
            rows.insert(boundary, Row::Text(String::new(), Style::new()));
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
                    let path = fit(&entry.label, available - cells(&key_label) - space.len());
                    let slash = boundary(&path, path.rfind('/').map_or(0, |i| i + 1));
                    if !key.is_empty() && key_label == key {
                        result.documents.insert(entry.key.unwrap(), entry.target);
                    }
                    painted = offset + cells(&key_label) + space.len() + cells(&path);
                    spans.extend([
                        Span::styled(key_label, CYAN),
                        Span::raw(space),
                        Span::styled(path[..slash].to_owned(), DIM),
                        Span::raw(path[slash..].to_owned()),
                    ]);
                    offset += widths[column] + 2;
                }
            }
            Row::Link { label, url, index } => {
                let text = fit(&label, room);
                if !text.is_empty()
                    && let Some(url) = url
                {
                    result
                        .links
                        .push((index, url, Rect::new(area.x, y, cells(&text) as u16, 1)));
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
