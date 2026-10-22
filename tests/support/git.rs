use std::path::Path;
use std::sync::{Arc, Mutex};

use streams_temper::sync::{
    git::GitCatalog,
    git_io::{Git, GitError, GitHook},
};

// Seed an existing repository; tests of initialization itself use production initialize.
#[allow(dead_code)] // The library Git harness imports only the process hooks.
pub fn seed_bootstrap(catalog: &GitCatalog) {
    use streams_temper::{
        storage::{StorageError, Transaction},
        sync::{
            envelope::BOOTSTRAP,
            state::{GitConfig, State},
        },
    };
    std::fs::create_dir_all(catalog.root()).unwrap();
    let git = catalog.git();
    git.run(
        &["init", "--initial-branch=main", "--template="],
        None,
        None,
        &[0],
    )
    .unwrap();
    let input = format!(
        "commit refs/heads/main\ncommitter Streams test <streams@example.invalid> 1 +0000\ndata {}\n{}\ndeleteall\ndone\n",
        BOOTSTRAP.len(),
        BOOTSTRAP,
    );
    git.run(
        &["fast-import", "--quiet"],
        Some(input.as_bytes()),
        None,
        &[0],
    )
    .unwrap();
    git.run(&["read-tree", "--reset", "-u", "HEAD"], None, None, &[0])
        .unwrap();
    catalog
        .store()
        .with_transaction::<_, StorageError>(|tx| {
            State::from_journal(tx.journal())
                .write_config(&GitConfig::new(None, "main", catalog.host_id()).unwrap())
                .unwrap();
            Ok(())
        })
        .unwrap();
}

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
