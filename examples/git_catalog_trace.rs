//! Bounded JSON-lines Git fixture adapter; not a user-facing CLI.

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};
use streams_temper::{
    codec::record_to_data,
    model::Catalog,
    sync::{
        envelope::decode_operation,
        git::GitCatalog,
        git_io::{Git, GitError, GitHook},
        outcomes::Result as SyncResult,
        policy::{Policy, SyncEvent},
    },
};

#[derive(Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
enum Fault {
    #[default]
    None,
    LostPushReply,
    LostReplyFailedAdoption,
    UncertainUnaccepted,
    FailedAdoption,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
enum Command {
    Init {
        remote: Option<String>,
    },
    Save {
        message: String,
    },
    Read {
        #[serde(default)]
        refresh: bool,
    },
    Snapshot {
        #[serde(default)]
        confirmed: bool,
    },
    Authority {
        stream: String,
    },
    Sync {
        event: SyncEvent,
        #[serde(default)]
        fault: Fault,
        now: Option<f64>,
    },
}

fn command(
    git: &Git,
    args: &[&str],
    input: Option<&[u8]>,
    index: Option<&Path>,
) -> Result<Vec<u8>, GitError> {
    let mut configured = vec![
        "-c",
        "user.name=Streams differential",
        "-c",
        "user.email=streams@example.invalid",
        "-c",
        "commit.gpgsign=false",
    ];
    configured.extend_from_slice(args);
    let allowed: &[i32] = if matches!(args.first(), Some(&"config" | &"merge-base")) {
        &[0, 1]
    } else {
        &[0]
    };
    git.run_raw(&configured, input, index, allowed)
}

fn catalog(root: &str, host: &str, fault: Fault, now: f64) -> GitCatalog {
    let mut catalog = GitCatalog::new(root, host, Policy::default()).unwrap();
    catalog.set_clock(Arc::new(move || now));
    let mut fired = false;
    let hook: GitHook = Arc::new(Mutex::new(
        move |git: &Git, args: &[&str], input: Option<&[u8]>, index: Option<&Path>| {
            // After an ambiguous push, suppress confirmation only for this request.
            // A new handle has normal transport again, modelling a process restart.
            if fired
                && matches!(fault, Fault::LostPushReply | Fault::UncertainUnaccepted)
                && matches!(args.first(), Some(&"ls-remote" | &"fetch"))
            {
                return Some(Err(GitError::new("fixture cannot confirm", None)));
            }
            if !fired
                && args.first() == Some(&"push")
                && matches!(
                    fault,
                    Fault::LostPushReply
                        | Fault::LostReplyFailedAdoption
                        | Fault::UncertainUnaccepted
                )
            {
                fired = true;
                if matches!(fault, Fault::LostPushReply | Fault::LostReplyFailedAdoption) {
                    return Some(
                        command(git, args, input, index)
                            .and_then(|_| Err(GitError::new("fixture lost push reply", None))),
                    );
                }
                return Some(Err(GitError::new("fixture unaccepted push", None)));
            }
            if matches!(
                fault,
                Fault::FailedAdoption | Fault::LostReplyFailedAdoption
            ) && index.is_none()
                && args.starts_with(&["read-tree", "-m", "-u"])
            {
                fired = true;
                return Some(Err(GitError::new("fixture adoption failure", Some(1))));
            }
            Some(command(git, args, input, index))
        },
    ));
    catalog.set_git_hook(hook);
    catalog
}

fn records(catalog: &Catalog) -> Value {
    let mut records = catalog.records();
    records.sort_by_key(|record| record.key());
    json!(
        records
            .iter()
            .map(|record| { json!({"kind": record.kind(), "record": record_to_data(record)}) })
            .collect::<Vec<_>>()
    )
}

fn outcome(result: SyncResult) -> Value {
    let domain = result.domain.as_ref().map(|domain| {
        json!({
            "operation_id": domain.operation_id, "outcome": domain.outcome,
            "record": domain.record.as_ref().map(record_to_data),
            "conflicts": domain.conflicts, "reason": domain.reason,
        })
    });
    json!({
        "sharing": result.sharing, "local": result.local, "domain": domain,
        "local_revision": result.local_revision, "remote_revision": result.remote_revision,
        "conflicts": result.conflicts, "accepted": result.accepted(),
        "requires_repair": result.requires_repair(), "needs_attention": result.needs_attention(),
    })
}

fn execute(root: &str, host: &str, line: &str) -> Value {
    let request: Command = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(_) => return json!({"error": "request"}),
    };
    // No handle or in-memory cache survives a logical command.
    let fault = match &request {
        Command::Sync { fault, .. } => *fault,
        _ => Fault::None,
    };
    let now = match &request {
        Command::Sync { now: Some(now), .. } => *now,
        _ => 1000.0,
    };
    let catalog = catalog(root, host, fault, now);
    match request {
        Command::Init { remote } => match catalog.initialize(remote, "main") {
            Ok(_) => json!({"initialized": true}),
            Err(_) => json!({"error": "repair-required"}),
        },
        Command::Save { message } => match decode_operation(&message) {
            Err(_) => json!({"error": "envelope"}),
            Ok((operation, origin)) => match catalog.save(&operation, Some(&origin)) {
                Ok(result) => json!({"result": outcome(result)}),
                Err(_) => json!({"error": "validation"}),
            },
        },
        Command::Sync { event, .. } => json!({"result": outcome(catalog.sync(event))}),
        Command::Snapshot { confirmed } => {
            let snapshot = if confirmed {
                catalog.confirmed_snapshot()
            } else {
                catalog.snapshot()
            };
            match snapshot {
                Ok(snapshot) => json!({"records": records(&snapshot.catalog)}),
                Err(_) => json!({"error": "repair-required"}),
            }
        }
        Command::Authority { stream } => match catalog.authority(&stream) {
            Ok(authority) => json!({"authority": authority}),
            Err(_) => json!({"error": "repair-required"}),
        },
        Command::Read { refresh } => match catalog.read(refresh) {
            Ok(read) => json!({
                "records": records(&read.catalog), "local": records(&read.local.catalog),
                "confirmed": read.confirmed.as_ref().map(|s| records(&s.catalog)),
                "freshness": read.freshness, "fetched_at": read.fetched_at,
                "conflicts": read.conflicts,
            }),
            Err(_) => json!({"error": "repair-required"}),
        },
    }
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [root, host] = args.as_slice() else {
        panic!("expected root and host")
    };
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let response = match line {
            Ok(line) => execute(root, host, &line),
            Err(_) => json!({"error": "input"}),
        };
        if serde_json::to_writer(&mut stdout, &response).is_err()
            || writeln!(stdout).is_err()
            || stdout.flush().is_err()
        {
            break;
        }
    }
}
