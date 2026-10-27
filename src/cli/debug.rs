use std::{fs, os::unix::process::CommandExt, path::PathBuf, process::Command};

use super::{CliError, Output, Result};

pub(super) fn forward(mut arguments: Vec<String>, json: bool) -> Result<Output> {
    let runner = std::env::var_os("STREAMS_PROMPT_LAB_RUNNER").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("extensions/stream-checkpoint/lab-cli.mjs")
        },
        PathBuf::from,
    );
    if !runner.is_absolute() {
        return Err(CliError::message(
            "STREAMS_PROMPT_LAB_RUNNER must be an absolute path",
        ));
    }
    if !fs::metadata(&runner).is_ok_and(|metadata| metadata.is_file()) {
        return Err(CliError::message(format!(
            "prompt lab runner is unavailable at {}; set STREAMS_PROMPT_LAB_RUNNER to the matching Pi package's lab-cli.mjs",
            runner.display()
        )));
    }
    if json && !arguments.iter().any(|argument| argument == "--json") {
        arguments.push("--json".into());
    }
    let executable = std::env::current_exe()?;
    // Replacing this process preserves the foreground signal group and exit status.
    // STREAMS_BIN_PATH pins observational calls to this executable, not an installed CLI.
    let error = Command::new("node")
        .arg(runner)
        .args(arguments)
        .env("STREAMS_BIN_PATH", executable)
        .exec();
    Err(CliError::message(format!(
        "cannot start the Node prompt lab: {error}; Node 22 or newer must be on PATH"
    )))
}
