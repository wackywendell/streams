use std::path::Path;
use std::sync::{Arc, Mutex};

use streams_temper::sync::{
    git::GitCatalog,
    git_io::{Git, GitError, GitHook},
};

// Isolate author/signing and index caches per subprocess, never user config.
pub fn install_hook(
    catalog: &mut GitCatalog,
    mut hook: impl FnMut(
        &Git,
        &[&str],
        Option<&[u8]>,
        Option<&Path>,
    ) -> Option<Result<Vec<u8>, GitError>>
    + Send
    + 'static,
) {
    let hook: GitHook = Arc::new(Mutex::new(
        move |git: &Git, args: &[&str], input: Option<&[u8]>, index: Option<&Path>| {
            hook(git, args, input, index).or_else(|| Some(test_command(git, args, input, index)))
        },
    ));
    catalog.set_git_hook(hook);
}

pub fn test_command(
    git: &Git,
    args: &[&str],
    input: Option<&[u8]>,
    index: Option<&Path>,
) -> Result<Vec<u8>, GitError> {
    let mut configured = vec![
        "-c",
        "user.name=Streams test",
        "-c",
        "user.email=streams@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.untrackedCache=false",
    ];
    configured.extend_from_slice(args);
    // The hook API does not carry the caller's allowed codes. Only config's
    // absent-key result and merge-base's negative answer are expected here.
    let allowed: &[i32] = if args.first() == Some(&"config") || args.first() == Some(&"merge-base")
    {
        &[0, 1]
    } else {
        &[0]
    };
    git.run_raw(&configured, input, index, allowed)
}
