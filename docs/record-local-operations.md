# ADR: Record-local operations with one writer per file

Status: implemented and activated. Local durability is separate from publication.
[Observe and act](native-observe-and-act.md) governs native lifetime, targeting and
failures, not catalog saves. Each Stream and its Sessions have one immutable
writing host.

## Make the records sufficient

A Session report reads its named records, applies its input as a pure function and
persists one target. Reconciliation belongs inside apply, not in a history-driven
coordinator around every save. Ordinary operations do not scan the catalog,
traverse history or consult a derived cache, including at process startup. Git
sync alone consults history for transport; it does not replay operations or merge
domain records.

The domain and pure apply function must not depend on the file format, Git or a frontend. Storage reads and writes typed records; sync transports the files. Herdr, cmux and future frontends supply observations and execute their own action types behind interfaces. Frontend and sync choices do not change local operation semantics.

This is a data and interaction model change, not a response-time target. Supplied measurements put `streams --json check` at 0.19–0.24 seconds wall time, about 0.1 seconds CPU, on the current live catalog. Event-driven cards avoid repeated idle reads, and Pi invokes the CLI directly per command. The reason to change storage is that an operation should depend only on its own records as the catalog grows, not that these measurements justify a cache.

## One host writes each file

Each Stream belongs to exactly one host. That host alone writes the Stream record and all of its Sessions. Other hosts hold read-only replicas. Moving a Stream to another machine, transferring its ownership and joining it with another host's Session are unsupported. A future explicit command might duplicate an effort into a new local Stream; this decision does not add that command or make duplication automatic.

Partition portable catalog files by host. The host in the physical location is part of record identity, not a mutable owner field. Typed records carry that host identity; decoding checks it against the path, and the write boundary checks it against the configured producer before any replacement. Creation selects the current host's partition. Updates and moves cannot select another host or change a record's host.

There are no effective-authority tokens, ownership periods, pending ownership transitions, handover or stale-owner reconciliation. The Stream's host is fixed at creation. Native workspace associations and card launch context cannot grant write permission to another partition. Reading synchronized records remains allowed, but local reports and native effects for foreign Streams must refuse before changing records or dispatching effects.

`Session.stream_id` is the only membership authority. There is no Stream roster or separate membership index to keep in sync. A move changes one Session file atomically; both Stream records stay unchanged. Point each query at the records that already hold its answer:

- A card reads its Session, then the Stream named by `stream_id`.
- Explicit diagnostic capture takes a fresh workspace snapshot and reads observed Sessions by their native-identity keys. It saves no workspace layout or contributor roster.
- Explicit Stream reopening discovers same-host Sessions once and selects current membership with status `active`. It opens one tab per selected Session, without preserving order or layout. Single-Session resume uses the selected record directly. Events, reports and card refreshes never discover a Stream's Sessions.
- `show` and listing a Stream's Sessions are explicit user discovery. They may enumerate host-scoped Session files, like `list` and human title resolution, but never on an event or report path.

Any new automatic operation that needs a roster must first identify a record that already owns its answer, rather than introducing duplicated membership and coordination.

## Read, apply, write

Each operation type declares a small typed input set: its target and captured dependencies. It does not take an arbitrary `Catalog` plus unrestricted inputs. The shell reads and strictly decodes those records once. Pure apply compares captured intent with current records and returns a changed target, no change, superseded intent or conflict. Storage atomically replaces that one target when it changed, sets its `revision` and `updated_at_ms`, and returns the resulting values without reopening the catalog. An unchanged result writes nothing. Unrelated revisions cannot reject the operation. Actions do not coordinate writers, retry effects or recover partial transactions; if one needs that machinery, change its model instead.

Apply assesses conversation-local and Stream-shared proposals separately. A Session report checks its host, captured conversation-local fields and the current Stream's same-host relationship, not its pre-move membership. Pi alone owns branch and lifecycle admission: its in-process fence checks conversation, source and branch, cancels old inference on fork or compact, and rechecks before dispatching a write. The CLI does not store a second generation or write lifecycle invalidations. A Stream report checks the target Stream and the captured Session's current Stream ID and `membership_epoch`. Each move increments the epoch, including a return to the same Stream, so old Stream prose cannot become valid again or be retargeted to the destination. These report guards do not change conversation identity or native card lifetime. Manual-text protection remains.

Strict decoding includes supported schema versions, unique object keys, regular files, safe paths, record-address identity and relationships among the named records. A malformed record is not absence. Creation distinguishes a missing key from unreadable or conflicting state. Whole-catalog validation belongs to explicit inspection and reset, not opening a backend handle. `list` and `check` may enumerate current records; neither replays history. A listing (`streams list`, the Pi chooser's `list_streams` and the herdr-app tab read of a tab with no Stream) shows every record it can decode and names each record file it cannot, as an `UnreadableRecord` with its kind, host, catalog path, cause and a repair chosen by kind. A Stream view shows its readable members and names this host's unreadable Sessions the same way, because any of them may be a member it cannot show; its status and recency reflect the readable members only. An action that needs every Session of the host, such as `streams tidy`, still fails on the first unreadable one.

The entity CLI (`stream`, `session`, `artifact`, `config`, `catalog`, plus root aliases `list` and `resume`) resolves an explicit selector by full key (`HOST/ID` or `HOST/HARNESS/ID`), `HARNESS/ID`, exact ID, or unique ID prefix. It never reads Pi environment variables, `hc` labels, or frontend state to select records. When resolution must enumerate records, it reads the strict inventory rather than the partial listing `list` shows: it fails on any unreadable record, so it never selects from a partial set in which an unreadable record might also have matched. Automation, cards, and the Pi extension pass exact keys and use point reads rather than catalog enumeration. Discovery of a foreign Stream does not grant permission to modify it.

### Record selection is not native permission

An explicit record key selects a read without proving native readiness. A known Stream key, including `STREAMS_STREAM_ID` for `--current`, needs no frontend connection merely to read that record. An exact Session query reads that Session and its saved Stream; workspace metadata cannot replace its membership. This keeps record inspection independent of missing or incomplete native projection.

Without an explicit key, the calling Herdr workspace's ordinary catalog, host and Stream markers may select one Stream from the current snapshot. Read that named record and check its immutable host. Missing, incomplete or conflicting markers refuse this metadata-only selection. There is no placement record to consult. CMUX has no workspace metadata capability, so it requires an explicit key rather than a reverse lookup or fabricated Herdr proof.

Selecting a record does not select a native effect target. Effects use addresses from the same current native observation and discard them after the pass. Conflicting markers are a diagnostic, not a reason to guess another workspace. The child checks whether its Herdr subscription has ended before acting; the [targeting decision](native-observe-and-act.md#bound-targeting-to-the-current-pass) records the accepted residual restart risk. There is no caller-attestation object or saved pane address fallback.

### Membership move contract

A move reads exactly 3 catalog inputs: the Session, the old Stream and the new Stream. The Session supplies the old Stream key; verified destination input supplies the new key. The domain entry point takes those typed records and verified move input, not a complete `Catalog`. All 3 records must belong to the current host. If a destination Stream must be created, that is a separate local creation before the move, not a multi-file transaction or permission to scan for another Stream.

Pure apply returns only the updated Session, or no change/conflict. It changes membership and its epoch while preserving work text, conversation identity, checkout URL and directory intent. Storage writes the Session in one atomic file replacement. Neither Stream is written; their revisions remain unchanged. There is no roster, multi-file write set, recovery fence, rollback or other Session input.

The catalog layer owns this function and persistence (`application::move_session_observed`). Only an explicit move calls it; native layout changes never do, in any frontend. No public command exposes it yet. Native relationship changes remain separate effect plans owned by the child/executor. They warn on failed effects without recording native launch history. They cannot expand this catalog action into a coordinated transaction.

### Durability without history as a journal

Local saves acknowledge durable record changes, not publication. They do not create or verify Git commits or run inline sync. Explicit, lifecycle, and write-triggered background sync are separate from local durability: when a non-`none` sync backend is configured, a CLI write may check `.catalog-local/last-sync.json` and spawn a detached `streams catalog sync --background` once `sync.auto_interval_minutes` has elapsed. Sync failure cannot turn an accepted local save into a clean failure inviting a duplicate action.

Each record carries `revision: u64` and `updated_at_ms: u64`. The single save path sets them, and only when the record changes; a save that changes nothing writes nothing. The revision is the only optimistic-concurrency token. User edits, CLI edits, enrollment and lifecycle compare and swap on it. Automatic reports keep their field-level rule: manual text is protected, and fields changed since the captured revision are not overwritten.

A retried write after a lost acknowledgement is recognised from record state, not from a log. A retry whose changes are already present returns no change and writes nothing. A user edit retried after a later edit conflicts; an automatic report retried after a later edit is superseded. There are no operation IDs, retry receipts, fingerprints or history lookup. Work text belongs only in the current record.

Each save replaces and flushes one file. Storage reports post-replacement uncertainty honestly, but actions do not construct a recovery protocol around it. A subsequent explicit invocation can read the same record. There is no multi-file transaction, pending-transaction scan or generic recovery fence.

Enrollment creates a missing local Stream first, then its Session as separate single-record operations. Interruption may leave an empty generated Stream; that is acceptable in alpha software and needs no rollback. Session and Stream report proposals are admitted and saved independently, not as an atomic batch. Neither case may claim that a second save succeeded merely because the first did.

## Direct identities and paths

Session identity is `(host, harness, native session ID)`. Membership is not part of its path. The format-5 layout is `hosts/<host>/streams/<stream ID>.json`, `hosts/<host>/sessions/<harness>/<id>/session.json`, `hosts/<host>/sessions/<harness>/<id>/artifacts/<name>`, and the reserved `hosts/<host>/host.json` path. Each record file holds the record itself as JSON, with no envelope. Encode path components and verify each decoded identity against its location. A host plus a harness and native Session ID locates that Session directly. Host-qualified keys also locate read-only replicas without searching for their owner.

Each Session owns its `stream_id`, `directory`, `references`, and `artifacts` directly; there is no separate checkout registry or `DirectoryBindings` store. Native addresses are not saved records. The [native enrollment boundary](native-enrollment-boundary.md) uses ordinary catalog, host and Stream markers observed on workspaces. Explicit reopening finds a matching workspace in its snapshot, or creates one. There are no saved placements, nonces or pending-attempt records to maintain as another authority or reverse index. Current observations drive best-effort reuse; conflicting markers do not permit adoption.

## Native actions

### Reopen active Sessions

Stream reopening uses current Session records and one fresh native observation. Discover active same-host Sessions for this explicit user command, create or reuse the Stream workspace, and open each missing Session in its own tab. Already-running Sessions stay where they are; report their locations rather than move or duplicate them. There is no saved tab order, pane sizing, topology, open/closed cache, layout capture or arrangement restoration.

Each Session's host-local binding holds its exact transcript locator beside its directory intent, at `session-directories/<digest(Session.id)>.json` under the existing runtime host root. `DirectoryBindings` owns persistence. The native driver supplies the verified locator in memory; enrollment or local binding save records it, and directory-only edits preserve it. No new index, portable catalog path or transcript search is needed. Missing launch inputs produce a Session-specific diagnostic. Definite failures need not prevent other Sessions opening; an uncertain native effect warns and stops the affected execution without a durable fence. Reuse is best effort: skip observed running Sessions without transcript searches, and tolerate rare duplicates when launches race or registration is late. Workspace, tab and pane addresses remain local to this pass; none is persisted.

### Observe and execute once

A native operation reads its concerned records and takes at most one current snapshot. It makes a pure plan and acts once, without caller attestation, repeated observations or a native launch journal. The [observe-and-act ADR](native-observe-and-act.md) owns card lifetime, following, safe targeting and best-effort reuse. Failed effects warn and stop; a later event or explicit request is a new pass.

Membership apply reads the bound Session, source Stream, destination Stream and observed native relationship once. After the save, the native engine consumes the returned current record context and the same native observation. It does not re-read Source, resolve every transcript or construct a new caller proof. Catalog save outcomes and captured-result checks remain unchanged.

The explicit offline reset stops writers before archiving old catalog and native state. There is no runtime reset component, legacy reader or evidence migration.

## Sync transports files

Sync uses Git commit/pull/push over the host-partitioned files. Before committing, it rejects local edits outside the current host's partition. Different hosts' changes have disjoint paths by construction, so Git can combine those paths without interpreting record fields. Concurrent changes to the same file are an error: fail closed, retain both sides and report the conflict. Do not use text, field, set or domain merge to choose a result, even if Git could combine different lines in that file automatically.

Local apply enforces actor and captured-input guards once. Sync does not replay operations, re-evaluate report captures, derive authority or preserve one-operation-envelope-per-commit history. A later input change can leave accepted generated prose stale until the next report; that is acceptable for alpha software. Checkout selections, registry files and native evidence remain host-local and outside Git.

Sync owns its Git interaction and reports publication or installation failures without silently fixing invalid records. It may consult history and validate incoming schema, host paths and relationships. It does not promise a cross-file atomic catalog snapshot, implement a generic recovery transaction, or make ordinary reads investigate transport state. If a named dependency is missing or invalid during installation, that operation refuses it; a later record event says what to re-read. Local saves remain durable if transport fails.

## Explicit reset

A record format change is a reset, not a migration. A binary that cannot decode a
record names it and says to move the catalog directory aside and re-enroll. The
README's "Resetting the catalog" section gives the steps: stop Pi Streams
children and card viewers; close only card panes, leaving editors open; move the
catalog aside as a backup; retain host identity and the checkout registry;
initialize an empty catalog, reload matching Pi code and re-enroll. Handle stray
panes manually. This procedure is not a reset command, migration component or
permission to change live state during ordinary development.

## Inventory and operation budgets

The required record sets are:

| Operation | Addressed inputs and effects |
| --- | --- |
| Session report | Target Session + current Stream; one Session replacement when it changes |
| Automatic Stream report | Target Stream + captured Session; one Stream replacement when it changes |
| Card context | Session + current Stream, plus Session binding and checkout registry when needed for documents |
| Native projection/open/close | Concerned records + at most one native snapshot; directory and launch inputs only when needed |
| Membership move | Session + old Stream + new Stream; replace only the Session |
| Enrollment | Create missing Stream first, then Session as separate local saves |
| Diagnostic capture | One workspace snapshot, then each observed Session by native-identity key |
| Explicit Stream open | Discover active same-host Sessions once; read each selected Session's launch inputs |
| Stream show/member list | Explicit discovery may enumerate host-scoped Session files |
| Sync | Enumerate partitioned files for transport; no domain replay |

Fixed configuration, producer identity and schema inputs count separately by name.
A save reads no journal or receipt. The checkout
registry is read once per operation, not once per repository. Automatic operations
must not hide enumeration in startup or another helper.

The record-adapter observer measures named reads, directory listings, returned content bytes and installed record bytes. The following budgets include its fixed schema marker; configuration, producer identity and native runtime inputs remain separate. Backend admission is not workspace open or resume.

| Record-local operation | Content reads | Listings | Record replacements | Byte budget |
| --- | --- | --- | --- | --- |
| Backend admission | Schema marker | 0 | 0 | Marker bytes only |
| Context | Marker + Session + current Stream | 0 | 0 | Exactly those current records |
| Fresh Session or automatic Stream report | Marker + target + named dependency | 0 | 1 target | Read those records once; write only the resulting target |
| Accepted report retry | Marker + target + named dependency | 0 | 0 | No rewrite |

These are adapter counters, not process-wide metrics. They exclude initialization/transport writes, failed partial I/O, filesystem metadata, configuration/native inputs, process starts and lock duration. Do not infer zero cost outside that boundary. Actual card and native open/resume budgets must exercise their real callers; a library context budget is not a card-read budget.

Records can grow; the contract bounds addressed records, not constant byte size. Counting tests name files and directory listings, including setup access. Grow unrelated catalogs and history and require identical automatic-operation counts. Cover strict decoding, foreign-partition writes, changed-intent/lost-ack retries after later edits, same-host single-file moves, Pi branch fences, stale Stream membership captures and interrupted atomic replacement. Timing is not the acceptance test.
