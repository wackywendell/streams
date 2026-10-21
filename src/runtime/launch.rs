//! Launch only an exact, verified saved conversation; no title/cwd-based discovery.

use std::path::{Path, PathBuf};

use super::{
    Result, RuntimeError,
    conversations::{ConversationRoots, Harness, conversation_file},
    invalid, text,
};
use crate::model::{Session, Status, Stream};

/// Verified filesystem input, not ownership authority or a native-effect permit.
#[derive(Debug, Clone)]
pub struct SessionCommand {
    harness: Harness,
    argv: Vec<String>,
    locator: PathBuf,
}
impl SessionCommand {
    pub fn verified(
        session: &Session,
        locator: Option<&Path>,
        roots: &ConversationRoots,
    ) -> Result<Self> {
        let harness: Harness = session
            .harness
            .parse()
            .map_err(|_| invalid(format!("unsupported session harness {:?}", session.harness)))?;
        let locator = conversation_file(harness, &session.harness_session_id, locator, roots)?;
        let target = if harness == Harness::Pi {
            locator
                .to_str()
                .ok_or_else(|| invalid("conversation path must be UTF-8"))?
        } else {
            &session.harness_session_id
        };
        let argv = vec![
            harness.to_string(),
            match harness {
                Harness::Pi => "--session",
                Harness::Codex => "resume",
                Harness::Claude => "--resume",
            }
            .into(),
            target.into(),
        ];
        for arg in &argv {
            text(arg)?;
        }
        Ok(Self {
            harness,
            argv,
            locator,
        })
    }
    pub fn harness(&self) -> Harness {
        self.harness
    }
    pub fn argv(&self) -> &[String] {
        &self.argv
    }
    pub fn locator(&self) -> &Path {
        &self.locator
    }
    pub fn command_text(&self) -> String {
        self.argv
            .iter()
            .map(|arg| shell_quote(arg))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

/// Explicit references may select done/parked sessions; implicit selection never does.
pub fn select_session<'a>(
    stream: &Stream,
    sessions: &'a [Session],
    host: &str,
    reference: Option<&str>,
) -> Result<Option<&'a Session>> {
    let eligible: Vec<_> = sessions
        .iter()
        .filter(|s| s.stream_id == stream.id && s.host_id == host)
        .collect();
    let matches: Vec<_> = if let Some(reference) = reference {
        let folded = icu_casemap::CaseMapper::new().fold_string(reference);
        let ids: Vec<_> = eligible
            .iter()
            .copied()
            .filter(|s| s.id == reference)
            .collect();
        let exact: Vec<_> = eligible
            .iter()
            .copied()
            .filter(|s| icu_casemap::CaseMapper::new().fold_string(&s.title) == folded)
            .collect();
        if !ids.is_empty() {
            ids
        } else if !exact.is_empty() {
            exact
        } else {
            eligible
                .iter()
                .copied()
                .filter(|s| {
                    !reference.trim().is_empty()
                        && icu_casemap::CaseMapper::new()
                            .fold_string(&s.title)
                            .contains(folded.as_ref())
                })
                .collect()
        }
    } else {
        eligible
            .into_iter()
            .filter(|s| !matches!(s.status, Status::Done | Status::Parked))
            .collect()
    };
    match (matches.as_slice(), reference) {
        ([], Some(reference)) => Err(RuntimeError::NotFound(format!(
            "no eligible local session matches {reference:?}"
        ))),
        ([], None) => Ok(None),
        ([session], _) => Ok(Some(*session)),
        _ => Err(RuntimeError::Ambiguous {
            message: "ambiguous local sessions; choose --session".into(),
            candidates: matches.iter().map(|s| s.id.clone()).collect(),
        }),
    }
}
