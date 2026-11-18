# Catalog saving and sync

Local saves and publication are separate operations. A report reads its addressed
records and saves one target record. It neither commits nor contacts the network. [Record-local operations](record-local-operations.md)
owns this contract; [RecordSync](../src/sync/records.rs) transports files.

## Initialize an empty catalog

```sh
streams init --sync none
# Or choose Git:
streams init --sync git
streams init --sync git --remote git@example.org:me/catalog.git --branch main
```

Choose one setup. Git without a remote records local commits only when sync runs.
A remote must already exist and be dedicated to Streams. An absent branch can be
created; an existing branch must contain supported host-partitioned records.
Authentication uses Git's normal credential mechanisms. Do not put secrets in
records or remote URLs.

Ordinary commands never initialize, convert or adopt a catalog. Changing strategy
or replacing an old format requires an explicit offline reset, not an in-place
import; the README's "Resetting the catalog" section gives the steps. Keep the
host identity stable across processes.

## Save locally

```sh
streams create "Indexing" --goal "Find the source of slow indexing" --json
streams session attach STREAM_ID --harness pi --session-id NATIVE_ID --title "Investigation" --json
```

These are separate single-record saves. If creating the Session fails, keep the
accepted Stream; there is no rollback transaction.

Each record carries its `revision` and `updated_at_ms`. The save path sets both,
and only when the record changes, so a save that changes nothing writes nothing.
A captured edit names the revision it read: a user edit applies only to that
revision, and an automatic report refuses fields changed since then. A retried
save that already landed is recognised from the record itself and returns no
change. There are no operation IDs or retry receipts.

## Sync explicitly or at Pi lifecycle boundaries

```sh
streams sync --json
streams list --refresh
streams show STREAM_ID --refresh --json
```

There are no publication intervals, cooldowns, event flags or background scheduler.
Ordinary reads use saved local records. An explicit `--refresh` runs sync before
reading; it is not a separate remote-only display projection and can publish local
changes as well as install incoming records.

Pi startup and shutdown request the same sync operation through the Rust child.
Shutdown cancels inference first; neither hook generates a report. Each lifecycle
request has a 10-second total deadline, including child-process termination.
Ordinary Git subprocesses use the fixed 30-second library deadline.

With `none`, sync validates local records without Git or network access. With Git
and no remote, it checkpoints this host's changed files and reports local-only
sharing. With a remote, it commits local producer changes, fetches incoming records,
combines disjoint file changes, installs them and pushes. Another host's partition
is read-only. Concurrent changes to the same record file stop sync even if a text
merge could combine different lines. Transport never replays domain operations or
re-evaluates accepted report inputs.

## Interpret outcomes separately

Domain decisions, local persistence and sharing have different meanings:

- A successful local save is durable even if publication later fails.
- `local-only` means there is no publication destination.
- `committed` describes local Git progress, not remote confirmation.
- `confirmed` establishes the push outcome, not fresh verification of report prose.
- `failed` or `uncertain` sharing leaves accepted local records intact.
- `blocked` or `repair-required` needs inspection, not a blind overwrite or retry.

Exit code 2 can include accepted saves and a later warning or failure. A timeout does not prove whether a push succeeded. Lifecycle
failures produce a warning and a durable `stream-catalog-sync` Pi entry.
A later explicit sync observes the current state; it does not resume a saved
publication phase or retry a captured native effect.

## Inspecting interrupted saves and manual changes

Stop writers and preserve a backup before repair. Include the catalog's records,
`.git`, local transport configuration and staging files. Do not unlink or replace
`.catalog.lock` while callers may be running.

Useful read-only Git inspection in a managed Git catalog includes:

```sh
git --no-optional-locks status --porcelain=v1 --untracked-files=all --ignored
git diff --
git diff --cached --
git log --format=fuller
```

There is no `save.json`, `publication.json` or operation-history recovery journal.
Atomic replacement can fail after changing a record. Sync can fail after installing
some files or advancing a local ref. Preserve the diagnostic and inspect actual
records, refs, index and working tree under user direction; do not delete evidence,
force-push, reset or reconstruct old marker files. Sync does not promise a
cross-file atomic catalog snapshot.

Do not use an independent Git writer or folder synchronization on the managed
catalog. Adding a remote or changing branch configuration outside Streams fails
closed rather than silently selecting a new publication destination.

## Acceptance and live setup

Automated acceptance uses isolated homes, temporary Git remotes and fake native
endpoints. It covers two-host file transport, offline local saves, lifecycle
requests and real descendant termination. It does not establish live credentials,
remote access or platform behavior.

Install matching CLI and Pi revisions only with approval. For an approved remote
trial, create and report a test effort, run explicit sync, then refresh it on a
second host. This checks visibility, not ownership transfer. Cross-host Session
moves and handover are unsupported.
