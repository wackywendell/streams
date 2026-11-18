//! Launch only an exact, verified saved conversation; no title/cwd-based discovery.

use std::path::{Path, PathBuf};

use super::{
    Result,
    conversations::{ConversationRoots, Harness, conversation_file},
    invalid, text,
};
use crate::model::Session;

/// Resolve on the owner before hc can interpret argv[0] as a template name.
pub fn resolve_executable(
    program: &str,
    environment: &std::collections::BTreeMap<String, String>,
    cwd: &Path,
) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    text(program)?;
    if !cwd.is_absolute() {
        return Err(invalid("executable base directory must be absolute"));
    }
    let path = Path::new(program);
    let candidates = if path.is_absolute() || path.components().count() > 1 {
        vec![cwd.join(path)]
    } else {
        let search = match environment.get("PATH") {
            Some(path) => path.clone(),
            None => {
                // Match execvp's native search path when the launch environment omits PATH.
                let size = unsafe { libc::confstr(libc::_CS_PATH, std::ptr::null_mut(), 0) };
                if size == 0 {
                    return Err(invalid("default executable path is unavailable"));
                }
                let mut bytes = vec![0u8; size];
                if unsafe { libc::confstr(libc::_CS_PATH, bytes.as_mut_ptr().cast(), size) } != size
                {
                    return Err(invalid("default executable path is unavailable"));
                }
                bytes.pop();
                String::from_utf8(bytes)
                    .map_err(|_| invalid("default executable path must be UTF-8"))?
            }
        };
        std::env::split_paths(&search)
            .map(|directory| cwd.join(directory).join(program))
            .collect()
    };
    candidates
        .into_iter()
        .find(|path| {
            std::fs::metadata(path).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
        .ok_or_else(|| invalid("owner executable is unavailable"))
}

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
