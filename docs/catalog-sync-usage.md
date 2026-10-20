# Using the catalog sync library

The sync library composes the catalog foundation. It provides application-level sync and read orchestration without starting a background service or configuring a hosted repository. The [CLI](../README.md) uses these coordinators, including for Pi's catalog reports; Work Cards integration remains separate. The [sync ADR](catalog-git-sync.md) explains the policy; the [implementation plan](../plans/catalog-sync.md) defines its scope.

## Local-only storage

`LocalCatalog` validates operations and serializes saves through the YAML adapter. It does not invoke Git or use the network.

```python
from pathlib import Path
from uuid import uuid4

from _streams.catalog.model import Stream
from _streams.catalog.operations import Operation
from _streams.catalog.sync import LocalCatalog
from _streams.catalog.yaml_store import YamlStore

catalog = LocalCatalog(YamlStore(Path("/path/to/new/local-catalog")), host_id="laptop")
stream = Stream(id=str(uuid4()), title="Investigate indexing", goal="Find the source of slow indexing",
                owner_host_id="laptop")
creation = Operation(id=str(uuid4()), before=None, after=stream, actor="user")
result = catalog.save(creation)
assert result.local == "persisted"
assert result.sharing == "local-only"
```

Retain operation IDs and original inputs when retrying. Generate IDs for new intents, not for each attempt. Local receipt fingerprints prevent an accepted operation from being applied again after later edits, including after Git replay retires it as satisfied or superseded. Reusing an accepted ID with different inputs requires repair rather than changing that ID's meaning.

The catalog's host ID must be stable across processes on the same host. Do not infer it from a terminal pane or a temporary process ID.

## Git-backed storage

Git backing does not require a remote. For a fresh local Git catalog:

```sh
streams init --sync git
```

Direct library callers omit `remote` from `GitCatalog.initialize(path, host_id="laptop")`. Saves commit locally and report `sharing="local-only"`, including refused and repair-required outcomes. Reads return the saved snapshot with freshness `local-only`. Explicit refresh, sync, startup and shutdown do not fetch, push or write publication clocks. Remote inspection is unavailable. Adding a remote outside Streams is unsupported and fails closed.

For an existing initialized `none` catalog, conversion must be explicit:

```sh
streams init --sync git --adopt --json
```

Adoption keeps the catalog directory and lock, record bytes and IDs, original creation authority, retry fingerprints and host-local bindings. Its root commit certifies the imported snapshot and retained provenance; it does not invent missing historical operations. Output includes `baseline_revision`. Repeating the command against that matching setup returns the same baseline, even after later saves. `--adopt` rejects `--remote` and cannot create a replacement host identity. The [adoption ADR](catalog-local-git-adoption.md) defines the trust and failure boundaries.

### Configured remote storage

Create a dedicated private remote separately, with no pre-existing files or commits. Normal Git author and signing configuration must work on the host. Authentication uses Git's normal credential mechanisms; Streams does not provision credentials. Do not put secrets in catalog records or remote URLs.

Initialize a new managed directory explicitly:

```python
from _streams.catalog.sync import GitCatalog, Policy

catalog = GitCatalog.initialize(
    Path("/path/to/new/shared-catalog"),
    remote="git@example.org:me/catalog.git",
    host_id="laptop",
    branch="main",
    policy=Policy(stream_seconds=300, session_seconds=3600),
)
```

Initialization creates an empty bootstrap locally if the remote branch does not exist. If another host already published a supported catalog, it fetches and validates that history. It will not adopt arbitrary existing Git contents. Reopen an initialized directory with `GitCatalog(path, host_id="laptop")`; pass the same policy in every process if overriding defaults.

`save(operation)` commits a validated change locally. It does not push automatically. Command handlers call `catalog.sync(event=...)` to request publication after related saves. They must check even after a no-change save: older pending changes may now be eligible. A running integration normally creates a stream, then its session record, then checks publication once:

```python
from _streams.catalog.model import Session

created = catalog.save(creation, origin={"session": "native-session-id"})
if created.accepted:
    authority = catalog.authority(stream.id)
    session = Session(id=str(uuid4()), host_id="laptop", stream_id=stream.id,
                      harness="pi", harness_session_id="native-session-id", title="Investigate indexing")
    joined = catalog.save(Operation(id=str(uuid4()), before=None, after=session,
                                    actor="automatic", ownership_token=authority.token))
    if joined.accepted:
        publication = catalog.sync(event="session")
```

The initial creation operation establishes local ownership without a push. Generic operations cannot transfer ownership or reattach a session. Ownership handover is not implemented.

Each record save is separate. If session creation fails after stream creation, retain and report the successful stream save. Do not treat the pair as a rollback transaction. Integrations must establish user approval before creating user-directed operations.

## Publication events and results

Call `catalog.sync(event=event)` after an event's related saves. With a configured remote, the backend checks timing and pending state under its shared store lock; callers do not implement their own timers. Without a remote, it validates local history and returns local-only state without publication scheduling.

| Event | Eligibility |
| --- | --- |
| `session`, `host` | N since the last confirmed push, or M if stream edits are pending |
| `stream` | M since the last full sync |
| `start`, `attach` | M since the last full sync |
| `shutdown`, `explicit`, `force` | Attempt regardless of M/N |

Defaults are M = 300 seconds, N = 3,600 seconds. Each Git subprocess has a 30-second timeout. A pass makes at most 3 competing-push attempts. Failed automatic attempts share a 60-second cooldown; shutdown and explicit/forced requests bypass it. These are caller-driven checks, not delivery deadlines or a scheduled task. A backward clock change conservatively delays interval-based attempts.

### CLI settings and Pi lifecycle

Set the intervals in host-local CLI configuration, not in shared catalog records:

```sh
streams config set sync.stream_seconds 300
streams config set sync.session_seconds 3600
streams config --json
```

Both settings accept finite nonnegative numbers. Zero removes that interval's delay, but not the failed-attempt cooldown or safety checks. Every CLI process reads the current values, including initialization and later opens. Direct library callers still pass their own `Policy`.

Pi startup runs `streams sync --event start --json`. It respects M and the cooldown without enrolling a session or invoking a model. Shutdown cancels pending inference and awaits `streams sync --event shutdown --json`. This attempts publication regardless of timing; it does not generate a final report. Each lifecycle command has a 10-second total deadline, including the spawned CLI and its child process group. Ordinary CLI commands retain the library limits above.

Failed results, transport errors and timeouts produce a warning and a durable `stream-catalog-sync` Pi entry. A timeout does not prove whether a push succeeded. Publication markers and retained commits remain available; interrupted adoption requires manual inspection. Run ordinary `streams sync` to establish publication or report repair before changing that evidence. Neither lifecycle hook undoes an accepted local save. The `none` strategy invokes no Git or network commands. Git without a remote uses local Git validation but makes no network calls.

### Scheduled and explicit read refresh

List/show command handlers call `catalog.read(refresh=False)`. A Git read fetches when N has elapsed since the last successful refresh; `refresh=True` requests an explicit blocking fetch. A successful fetch in any write sync counts toward this read clock, even if its later push fails. There is no daemon and no asynchronous refresh to wait for later.

Reads never push or reset the M/N publication clocks. They share a separate 60-second failed-read cooldown; explicit refresh bypasses it. Fetch success clears the prior read error. A failure returns the cached view with `freshness='failed'` and diagnostic detail. During the cooldown, cached reads retain that detail rather than hiding the failure. Local-only reads make no network calls, including explicit refresh requests.

`ReadResult` separates the displayed catalog from persistence:

- `catalog` is the saved local catalog without a remote, or the logical view produced by replay onto confirmed remote state
- `local` is the actual local persistence snapshot, not that projected view
- `confirmed` is the last verified remote snapshot, or None if unavailable/local-only
- `freshness`, `fetched_at` and `detail` describe refresh evidence and any fetch failure
- `conflicts` identifies pending decisions that cannot safely combine

On a genuine domain conflict, the primary view remains the complete local catalog and the confirmed alternative remains available separately. This avoids inventing a partially merged, potentially invalid record graph. CLI output must expose those conflicts and the alternative rather than label local state as reconciled. The projected catalog is not a persistence revision or permission to overwrite records. Writes must use the actual local snapshot. In the initial CLI, run `streams sync` to adopt records visible only in the refreshed view before editing them or their dependent relationships.

Reads do not change live record files, index, HEAD or pending publication markers. Pending projected state never grants ownership; authority still comes from validated confirmed history or genuine local creation provenance. Unsupported history or repair-required state fails closed rather than becoming an ordinary offline warning. With a configured remote, `inspect_remote()` remains an unconditional low-level fetch returning only a confirmed snapshot; `confirmed_snapshot()` reads that snapshot without network access. No-remote mode refuses `inspect_remote()` before any network command.

A `Result` separates the domain outcome, local save and sharing state. `accepted` is true only for an accepted domain save without a local repair condition; a publication result alone is never an accepted save. `requires_repair` stops follow-on catalog writes. `needs_attention` also includes ordinary publication failures and uncertainty: these warn without undoing local acceptance. CLI receipts can therefore contain accepted saves and a repair-required publication result together. Callers must stop dependent effects while retaining every save receipt.

In particular:

- `committed` with `local-only` is a successful local Git save without a publication destination
- `committed` with `pending` is a successful local save, not a failed edit
- `blocked` with conflict details means the entire outgoing push stopped; local commits remain available
- `uncertain` means the push result could not be established; the next attempt checks remote history before replay
- `confirmed` with local `repair-required` means publication succeeded but the local checkout needs inspection
- `repair-required` is never permission to reset or overwrite the catalog

Conflict details include operation ID, target, reason and available origin attribution. The session triggering sync need not resolve another session's decision. A user may keep remote intent or replace local intent, but that requires explicit repair and revalidation of dependent operations. Do not force-push a resolution.

## Acceptance before activation

The automated tests use isolated XDG directories, a temporary bare Git remote and unsigned test identities. They exercise guarded Pi reports through fresh CLI processes, M-gated startup, shutdown publication, second-host refresh, retained offline saves and retry after restart. A separate native process test checks that the lifecycle deadline stops both the CLI and a child. These tests do not establish that real host credentials or signing work.

Local adoption tests additionally exercise unchanged records, lock and source identities, retained native/checkout evidence, captured Pi reports across adoption, clone-based retry recovery and interrupted installation. Signing failures during preparation leave the active `none` catalog usable. A final-path identity or signing-policy mismatch stops before strategy switching and leaves repair evidence. The tests use synthetic isolated catalogs, not the live catalog.

Live local adoption requires a coordinated window after matching CLI and Pi code is installed. Preserve an approved backup and the existing catalog root. Run the explicit adoption command only with approval, then verify its baseline, existing records, ownership tokens and bindings. Check a subsequent local save, a refreshed read and Pi startup/shutdown. No remote or authentication setup is required; normal local Git author and signing configuration must work. Do not replace the populated catalog with a fresh empty directory.

Before using a remote-backed Git catalog:

1. Integrate matching CLI and Pi extension revisions. Do not mix in the published scratch-based extension.
2. With approval, initialize the selected private catalog using the host's normal Git authentication and signing configuration.
3. Create a test effort and report, then run `streams sync --json`. Confirm both the local save and publication outcomes.
4. On a second host, initialize against the same remote and use `streams show STREAM_ID --refresh --json` to verify the report. This tests visibility, not ownership transfer.

Installed-runtime changes and remote setup require separate approval. Ownership handover and cross-host session resumption remain outside this sync slice.

## Inspecting interrupted saves and manual changes

The first implementation detects and preserves incomplete work; it does not repair it automatically. Back up the entire managed directory, including `.git` and `.catalog-sync`, before manual repair. Do not unlink or replace `.catalog.lock` while callers may be running.

An unfinished local save leaves `.catalog-sync/save.json`. Its original operation and starting commit explain the intended change. Local-only-to-Git adoption uses that same save marker with an `adoption` entry: the baseline, record fingerprints, original private setup and signing profile. A prepared candidate may remain under `.catalog-staging/adoption-*/`. Preserve it alongside the marker.

An unfinished publication or adoption of remote history leaves `.catalog-sync/publication.json`. Retained publication commits are reachable under `refs/catalog/retained/`.

Useful read-only inspection commands in the managed directory are:

```sh
git status --porcelain=v1 --untracked-files=all --ignored
git diff --
git diff --cached --
git log --format=fuller
git for-each-ref refs/catalog/retained/
```

Read the marker files as JSON. Existing configuration, timing and publication records are parsed into immutable supported shapes before effects. Malformed timing, an incomplete marker or an unknown phase is repair-required, not an ordinary offline result; access preserves the existing bookkeeping bytes. Do not repair such metadata by deleting the evidence or assigning a new retry baseline.

Operation messages use the `Streams operation v1` header and can be decoded with `decode_operation(message)`. Compare the marker's starting commit, original operation, changed files, index and resulting commit. A valid record alone is insufficient: every operation commit must explain exactly its intended record change through domain reconciliation.

If the original operation was fully committed, verify its envelope, parent and materialized record before clearing the leftover save marker. The next coordinated history read establishes missing retry receipts before any local operation can retire. Fetched operations are checked against those receipts before becoming the confirmed snapshot.

Local-only mode has no commit history to rebuild receipts from. Before clearing its verified completed save marker, decode the marker's original operation and retain its accepted-intent receipt with `catalog.state.remember(operation)`. Receipt completion is bookkeeping for that verified save, not permission to apply the operation again. Preserve the receipts along with other local state.

If the save was not complete, either finish that exact operation or restore the recorded starting state only under explicit user direction, preserving evidence first. Do not invent a different operation under the old ID. A publication-adoption repair similarly needs to establish the accepted remote tip and preserve later local operations before changing the checkout. Local Git bootstrap has no remote tip: verify its snapshot baseline, original private setup and intended final-path identity/signing policy instead. Do not clear its marker merely because the YAML records remain valid.

After repairing files, index and supported operation history, clear only the marker whose condition was repaired. Merely deleting a marker is not a repair: the next call still validates checkout and history. `snapshot()` refuses uncertain live state; `confirmed_snapshot()` can inspect the last verified remote catalog separately. Dedicated repair and conflict-resolution commands are not part of this library slice.

Keep `.catalog.lock`, `.catalog-staging/` and `.catalog-sync/` untracked. Do not use `git add --all`, folder synchronization or an independent Git writer on this managed directory. Changes to Git remotes, branch selection, published history or record files outside Streams are unsupported and require deliberate inspection.
