//! Observe signing policy at its actual Git directory, independently of graph construction.

use super::repair;
use crate::sync::{git_io::Git, state::RepairRequired};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct SigningProfile {
    author: String,
    committer: String,
    policy: BTreeMap<String, String>,
}

impl SigningProfile {
    pub(super) fn observe(git: &Git) -> Result<Self, RepairRequired> {
        let raw = git
            .run(
                &[
                    "config",
                    "--null",
                    "--get-regexp",
                    r"^(user\.|author\.|committer\.|commit\.gpgsign$|gpg\.)",
                ],
                None,
                None,
                &[0, 1],
            )
            .map_err(repair)?;
        let raw = std::str::from_utf8(&raw).map_err(repair)?;
        let mut policy = BTreeMap::new();
        for entry in raw.split('\0').filter(|s| !s.is_empty()) {
            let (key, value) = entry.split_once('\n').unwrap_or((entry, ""));
            policy.insert(key.into(), value.into());
        }
        let identity = |name: &str| -> Result<String, RepairRequired> {
            let value = git.text(&["var", name], None, None, &[0]).map_err(repair)?;
            value
                .rsplitn(3, ' ')
                .nth(2)
                .map(str::to_owned)
                .ok_or_else(|| repair("invalid Git identity"))
        };
        Ok(Self {
            author: identity("GIT_AUTHOR_IDENT")?,
            committer: identity("GIT_COMMITTER_IDENT")?,
            policy,
        })
    }

    pub(super) fn verify(&self, git: &Git) -> Result<(), RepairRequired> {
        if &Self::observe(git)? != self {
            return Err(repair(
                "Git identity or signing policy differs at the final catalog path; inspect save.json",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
