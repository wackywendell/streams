# ADR: Record-local operations with one writer per file

Status: decided, not implemented. This replaces the lookup-only proposal in [Bounded, event-driven work](bounded-work-and-card-events.md) and the authority/replay design in [Git sync](catalog-git-sync.md). Local durability remains separate from publication. [Observe and act](native-observe-and-act.md) now governs native lifetime, targeting and failures; it supersedes the former proof and launch-fence requirements, not catalog receipts. The single-host rule below supersedes cross-host participation and ownership-transfer proposals in earlier records.

## Make the records sufficient

A Session report should read the Session and its Stream, apply the report, and save the changed records. Today that path repeatedly loads the catalog. Git backing also replays history to recover ownership and prove that working files agree with supported commits. A direct final lookup would leave those larger dependencies intact.

An operation will instead read its named records, apply its input as a pure function, and persist the result. Reconciliation belongs inside apply, not in a history-driven coordinator around every save. Ordinary operations will not scan the catalog, traverse history or consult a derived cache, including at process startup. Git sync alone consults history for transport; it will not replay operations or merge domain records.

The domain and pure apply function must not depend on YAML, Git or a frontend. Storage reads and writes typed records; sync transports the files. Herdr, cmux and future frontends supply observations and execute their own action types behind interfaces. Frontend and sync choices do not change local operation semantics.

This is a data and interaction model change, not a response-time target. Supplied measurements put `streams --json check` at 0.19–0.24 seconds wall time, about 0.1 seconds CPU, on the current live catalog. Event-driven cards and the long-lived Pi child remove repeated idle reads and process starts. The reason to change storage is that an operation should depend only on its own records as the catalog grows, not that these measurements justify a cache.

## One host writes each file

Each Stream belongs to exactly one host. That host alone writes the Stream record and all of its Sessions. Other hosts hold read-only replicas. Moving a Stream to another machine, transferring its ownership and joining it with another host's Session are unsupported. A future explicit command might duplicate an effort into a new local Stream; this decision does not add that command or make duplication automatic.

Partition portable catalog files by host. The host in the physical location is part of record identity, not a mutable owner field. Typed records carry that host identity; decoding checks it against the path, and the write boundary checks it against the configured producer before any replacement. Creation selects the current host's partition. Updates and moves cannot select another host or change a record's host.

There will be no effective-authority tokens, ownership periods, pending ownership transitions, handover or stale-owner reconciliation. The Stream's host is fixed at creation. Native workspace associations and card launch context cannot grant write permission to another partition. Reading synchronized records remains allowed, but local reports and native effects for foreign Streams must refuse before changing records or dispatching effects.

`Session.stream_id` is the only membership authority. There is no Stream roster or separate membership index to keep in sync. A move changes one Session file atomically; both Stream records stay unchanged. Point each query at the records that already hold its answer:

- A card reads its Session, then the Stream named by `stream_id`.
- Explicit diagnostic capture takes a fresh workspace snapshot and reads observed Sessions by their native-identity keys. It saves no workspace layout or contributor roster.
- Explicit Stream reopening discovers same-host Sessions once and selects current membership with status `active`. It opens one tab per selected Session, without preserving order or layout. Single-Session resume uses the selected record directly. Events, reports and card refreshes never discover a Stream's Sessions.
- `show` and listing a Stream's Sessions are explicit user discovery. They may enumerate host-scoped Session files, like `list` and human title resolution, but never on an event or report path.

Any new automatic operation that needs a roster must first identify a record that already owns its answer, rather than introducing duplicated membership and coordination.

## Read, apply, write

Each operation type declares a small typed input set: its target and captured dependencies. The target record also holds its retry receipts. It does not take an arbitrary `Catalog` plus unrestricted inputs. The shell reads and strictly decodes those records once. Pure apply compares captured intent with current records and returns a changed target, no change, superseded intent or conflict. Storage atomically replaces that one target, including its receipt, and returns the resulting values without reopening the catalog. Unrelated revisions cannot reject the operation. Actions do not coordinate writers, retry effects or recover partial transactions; if one needs that machinery, change its model instead.

Apply assesses conversation-local and Stream-shared proposals separately. A Session report checks its host, captured conversation-local fields and the current Stream's same-host relationship, not its pre-move membership. Pi alone owns branch and lifecycle admission: its in-process fence checks conversation, source and branch, cancels old inference on fork or compact, and rechecks before dispatching a write. The CLI does not store a second generation or write lifecycle invalidations. A Stream report checks the target Stream and the captured Session's current Stream ID and membership period. Each move creates a new membership period, including a return to the same Stream, so old Stream prose cannot become valid again or be retargeted to the destination. These report guards do not change conversation identity or native card lifetime. Manual-text protection and reuse-of-operation-ID checks remain.

Strict decoding includes supported schema versions, unique object keys, regular files, safe paths, record-address identity and relationships among the named records. A malformed record is not absence. Creation distinguishes a missing key from unreadable or conflicting state. Whole-catalog validation belongs to explicit inspection and cutover, not opening a backend handle. `list` and `check` may enumerate current records; neither replays history.

A positional CLI `REF` requests explicit human discovery by ID, title or substring, using the same enumeration as `list`. It must resolve to exactly one host-qualified ID or fail on ambiguity before invoking the operation. UUID-looking strings are not reserved, and there is no new `--id` flag. Automation, the Pi child, cards and Pi keep their existing exact-key paths, including `--stream-id`, `--session-id` and `--current`. Those paths use point reads, never catalog enumeration. Discovery of a foreign Stream does not grant permission to modify it.

### Record selection is not native permission

An explicit record key will select a read without proving native readiness. A known Stream key, including `STREAMS_STREAM_ID` for `--current`, needs no frontend connection merely to read that record. An exact Session query reads that Session and its saved Stream; workspace metadata cannot replace its membership. This keeps record inspection independent of missing or incomplete native projection.

Without an explicit key, Herdr workspace metadata may select one Stream. The reader must verify that selector against the named canonical placement and the same snapshot's complete binding fields, endpoint and calling workspace. Missing, incomplete or conflicting evidence refuses this metadata-only selection. CMUX has no workspace metadata capability, so it requires an explicit key rather than a reverse lookup or fabricated Herdr proof.

Selecting a record does not select a native effect target. Effects use the same current native observation and the frontend's safe target-identity contract. A missing or conflicting binding is a diagnostic, not a reason to guess another workspace. There is no caller-attestation object or fallback from a failed native observation to a saved pane address.

### Membership move contract

A move reads exactly 3 catalog inputs: the Session, the old Stream and the new Stream. The Session supplies the old Stream key; verified destination input supplies the new key. The domain entry point takes those typed records and verified move input, not a complete `Catalog`. All 3 records must belong to the current host. If a destination Stream must be created, that is a separate local creation before the move, not a multi-file transaction or permission to scan for another Stream.

Pure apply returns only the updated Session, or no change/conflict. It changes membership and its period while preserving work text, conversation identity, checkout URL and directory intent. Storage writes the Session and its receipt in one atomic file replacement. Neither Stream is written; their revisions remain unchanged. There is no roster, multi-file write set, recovery fence, rollback or other Session input.

The catalog layer owns this function and persistence. Native reconciliation supplies verified movement input and calls it; it does not implement catalog reconciliation. Native relationship changes remain separate effect plans owned by the child/executor. They warn on failed effects without recording native launch history. They cannot expand this catalog action into a coordinated transaction.

### Durability without history as a journal

Local saves acknowledge durable record changes, not publication. They do not create or verify Git commits or run inline sync. They may emit a separate sync trigger; a due publication check cannot hide a catalog scan inside a report response. Sync failure cannot turn an accepted local save into a clean failure inviting a duplicate action.

Stable operation IDs and exact-intent fingerprints prevent double application and changed-intent retries after lost acknowledgements. The retry key is `(record identity, operation ID)`. Keep its fingerprint and accepted outcome in the target record's envelope, written atomically with the record. There is no second receipt file, host-wide receipt map, separate journal or history lookup. Retain enough receipt identities to recognize earlier retries after later edits; a last-operation-only marker would lose that guarantee. Receipts are deduplication evidence, not replayable operations. Store compact decisions and target IDs, never captured or historical record snapshots. Work text belongs only in the current record.

Each save replaces and flushes one file. Storage reports post-replacement uncertainty honestly, but actions do not construct a recovery protocol around it. A subsequent explicit invocation can read the same record and its receipt. There is no multi-file transaction, pending-transaction scan or generic recovery fence.

Enrollment creates a missing local Stream first, then its Session as separate single-record operations. Interruption may leave an empty generated Stream; that is acceptable in alpha software and needs no rollback. Session and Stream report proposals are admitted and saved independently, not as an atomic batch. Neither case may claim that a second save succeeded merely because the first did.

## Direct identities and paths

Session identity remains `(host, harness, native session ID)`. Derive its deterministic ID using the existing `agent::session_id_for` identity preimage. Membership is not part of its path. A concrete layout is `hosts/<encoded host>/streams/<stream ID>.yaml`, `hosts/<encoded host>/conversations/<derived Session ID>.yaml` and `hosts/<encoded host>/host.yaml`. Encode path components and verify each decoded identity against its location. A host plus an explicit or native-derived Session ID locates that Session directly. Host-qualified keys also locate read-only replicas without searching for their owner.

After the explicit reset, re-enrollment creates Sessions under their native-identity keys. Do not keep old-ID aliases or a native-ID index. Captured requests carry full identity and schema version. A missing Session means unenrolled only in the completed new format; an incomplete reset means repair. No stale key or child startup falls back to discovery.

`catalog_source` identifies the logical catalog by saved host UUID and canonical catalog path. Do not include device numbers, inodes or another UUID. Replacing contents at the same path does not create a different logical catalog. Ordinary updates retain their captured-field and target-local receipt guards; explicit offline reset stops writers and retires obsolete runtime references instead of asking normal actions to detect directory replacement.

Keep portable Stream/Session records, the host-wide checkout registry and host-local per-Session directory bindings separate. Read the registry once and pass its decoded value through directory and document resolution. Stream-only directory defaults remain separately Stream-keyed. Each Session checkout always carries its resolved portable repository URL, populated at attach or creation. A Stream-local alias is only a label, so repository edits cannot orphan unread Sessions and moves cannot reinterpret aliases through the destination.

Normal Session construction will derive its ID from immutable native identity, and checkout construction will require the portable URL. These domain types will change with the atomic catalog switch: tightening them while the legacy backend remains active would reject its existing data. The archived catalog stays opaque; no legacy catalog decoder or replay model remains. There will be no parallel runtime domain models or public validated wrapper around a mutable record.

A host-local checkout is the registry's saved checkout ID and canonical directory path, not a filesystem object. Device numbers can change across reboots; a directory recreated at the same path remains the same registered checkout. Do not persist device/inode identity or reject a root because those numbers changed. Explicit moves change the saved path while preserving the ID. Repository URL bindings, explicit defaults, directory existence and path-containment checks remain. During offline cutover, archive the old registry and remove its obsolete stamps while preserving IDs, paths, bindings and defaults; normal reads do not migrate it.

Native workspace relationships also need direct addresses. The [native placement contract](native-enrollment-boundary.md) keeps one canonical placement record per host-local Stream; workspace metadata is its observed projection and selector, not a second authority or reverse claim index. Exact endpoint, workspace and binding checks replace association and claim enumeration. Native creation retains no attempt journal; current observations drive best-effort reuse. Missing metadata does not prove that an existing resource is safe to adopt.

## Native actions

### Reopen active Sessions

Stream reopening will use current Session records and one fresh native observation. Discover active same-host Sessions for this explicit user command, create or reuse the Stream workspace, and open each missing Session in its own tab. Already-running Sessions stay where they are; report their locations rather than move or duplicate them. There is no saved tab order, pane sizing, topology, open/closed cache or layout restoration. Remove layout capture and saved arrangements instead of maintaining another source of work state.

Each Session's host-local binding will hold its exact transcript locator beside its directory intent, at `session-directories/<digest(Session.id)>.json` under the existing runtime host root. `DirectoryBindings` owns persistence. The native driver supplies the verified locator in memory; enrollment or local binding save records it, and directory-only edits preserve it. No new index, portable catalog path or transcript search is needed. Missing launch inputs produce a Session-specific diagnostic. Definite failures need not prevent other Sessions opening; an uncertain native effect warns and stops the affected execution without a durable fence. Reuse is best effort: skip observed running Sessions without transcript searches, and tolerate rare duplicates when launches race or registration is late. The canonical workspace handle remains a resource address, not evidence that the workspace is currently open.

### Observe and execute once

A native operation reads its concerned records and takes at most one current snapshot. It makes a pure plan and acts once, without caller attestation, repeated observations or a native launch journal. The [observe-and-act ADR](native-observe-and-act.md) owns card lifetime, following, safe targeting and best-effort reuse. Failed effects warn and stop; a later event or explicit request is a new pass.

Membership apply reads the bound Session, source Stream, destination Stream and observed native relationship once. After the save, the native engine consumes the returned current record context and the same native observation. It does not re-read Source, resolve every transcript or construct a new caller proof. Catalog save receipts and captured-result checks remain unchanged.

The [manual catalog cutover](native-catalog-reset.md) archives old native state after stopping old children and viewers. There is no runtime reset component, legacy reader or evidence migration. Do not discard the old implementation's evidence while it is still active.

## Sync transports files

Sync will use Git commit/pull/push over the host-partitioned files. Before committing, it rejects local edits outside the current host's partition. Different hosts' changes have disjoint paths by construction, so Git can combine those paths without interpreting record fields. Concurrent changes to the same file are an error: fail closed, retain both sides and report the conflict. Do not use text, field, set or domain merge to choose a result, even if Git could combine different lines in that file automatically.

Local apply enforces actor and captured-input guards once. Sync does not replay operations, re-evaluate report captures, derive authority or preserve one-operation-envelope-per-commit history. A later input change can leave accepted generated prose stale until the next report; that is acceptable for alpha software. Retry receipts travel with their target record envelopes. Checkout selections, registry files and native evidence remain host-local and outside Git.

Sync owns its Git interaction and reports publication or installation failures without silently fixing invalid records. It may consult history and validate incoming schema, host paths and relationships. It does not promise a cross-file atomic catalog snapshot, implement a generic recovery transaction, or make ordinary reads investigate transport state. If a named dependency is missing or invalid during installation, that operation refuses it; a later record event says what to re-read. Local saves remain durable if transport fails.

## Explicit cutover

Run one explicit offline archive-and-reset with all writers and event consumers stopped and the owning locks held. Keep the configured host UUID. Archive the entire old catalog unchanged, including Git history and bookkeeping, then install a validated record-local catalog with no Streams or Sessions and a fresh transport baseline. Re-enroll work afterward. The archive preserves the old content for manual reference; normal code never reads it.

This replaces the producer-record migration plan. Preserving those records would retain the obsolete Authority and replay model solely to interpret the old format. For this alpha cutover, an opaque archive and re-enrollment are simpler than keeping that machinery. No old authority becomes new write permission, no history is rewritten, and no roster, alias service, automatic import or runtime fallback remains. Coordinate the fresh transport baseline across hosts; old writers must reject the completed new format.

Archive and retire old per-Session directory selections, Stream defaults, associations, claims and arrangements instead of translating catalog references. Preserve the checkout registry's logical intent through the explicit stamp removal described above. Do not discard pending workspace-open attempts or reservation fences while the old native runtime is still active; stop it and archive those files through the manual checklist. Install the completed catalog only after the replacement and required local state are durable; failures after changes begin preserve the archive and report uncertainty, not automatic rollback.

Native evidence and runtime fences follow the [manual catalog-cutover checklist](native-catalog-reset.md), not a reset/preflight component. The coordinator stops all children and viewers, closes card panes, leaves editors open and archives the old native/runtime state. Ordinary actions never migrate or recover it. The coordinator alone performs the live procedure; this ADR authorizes no live-state changes by workers.

## Inventory and operation budgets

The current-source column below describes the implementation before this redesign. Common costs include `application::open` and `agent::context` full snapshots, receipt/history-based authority and `YamlTransaction::save` whole-catalog revision comparison plus pre/post reads. Those dependencies must disappear rather than move behind a keyed API.

Budgets include fixed configuration/producer/schema files, counted separately by name. A save reads its target's embedded receipt, not another file or recovery marker. Single-Session counts are independent of unrelated records and history. Diagnostic capture declares a per-observed-pane budget. Explicit Stream reopening declares its discovery cost and per-selected-Session launch cost; that discovery never runs on idle or event paths. The checkout registry is read once per operation, not once per repository.

| Operation | Current dependencies | Required record set and changes |
| --- | --- | --- |
| Report / save | Full catalogs, captured inputs, host-wide receipts, authority/history, inline sync and post-save reads | Session + current Stream for conversation work; target Stream + captured Session for Stream work; only typed captured dependencies. Pi fences branch liveness; pure apply separates conversation-local and Stream membership guards. Save each target independently with its embedded receipt; no Git activity or transaction fence. |
| Card read / ordinary context | Repeated snapshots and authority, all Stream Sessions for inventory, legacy directory discovery, registry rereads and document checks | Session + current Stream + Session binding + registry. Return only this Session. Native snapshots are separate event inputs; there is no native evidence-file read. Check only this Stream's document availability when required. |
| Native project | Repeated source contexts, evidence, name ledgers and snapshots | Session + Stream and exact workspace relationship; at most one snapshot. No evidence or name ledger. No directory/document reads to project titles/summaries. |
| Diagnostic capture | Full catalog filtered to workspace members, authority, relationship enumeration and topology | One fresh workspace snapshot → each observed Session by native-identity key. No saved arrangement or contributor roster; file count grows with the observed subjects. |
| Native open | Full source contexts, evidence, assets and repeated absence/launch/confirmation checks | Session + Stream, exact relationship, fixed assets and at most one snapshot followed by effects/replies. No pending intent or evidence file. Read directory inputs only if launch needs them. |
| Native close | Full source contexts before/after, evidence, markers and confirmation | External: concerned records, one snapshot, safely targeted effect and reply. Card q/owner-exit: exit only, no closure record. No confirmation snapshot or directory/document reads. |
| Enroll / move | Catalog/native-ID scans, authority/history, duplicate caller snapshots, relationship enumeration, directory discovery, separate saves and inline sync | Enrollment creates a local Stream before its Session as separate single-record saves. Move reads Session + old Stream + new Stream, all same-host, and atomically writes only the Session with its receipt. Native relationship updates are separate plans. No roster, fence or other Session writes. |
| Workspace open / resume | Full catalog member selection, saved arrangements and repeated authority checks | Explicit reopening discovers current active same-host Sessions once; single-Session resume uses its selected record. Read exact placement and needed Session bindings; take one native observation. Open missing Sessions in separate tabs, leaving existing Sessions in place. No saved layout or open-state history. |
| Stream show | Full catalog, title resolution, all members and optional refresh | Explicit user discovery may enumerate host-scoped Sessions and filter by stream_id. Human title discovery likewise precedes exact-key actions. Neither enumeration runs on events/reports. Refresh is separate sync. |
| Sync | Operation history/replay, receipts, complete catalogs and domain candidate validation | Git commit/pull/push of partitioned files; path/schema/relationship validation. Same-file concurrent changes fail, never merge. No domain replay or generic recovery transaction. No host-local runtime state enters Git. |

The record-adapter observer measures named reads, directory listings, returned content bytes and installed record bytes. The following budgets include its fixed schema marker; configuration, producer identity and native runtime inputs remain separate. Backend admission is not workspace open or resume.

| Record-local operation | Content reads | Listings | Record replacements | Byte budget |
| --- | --- | --- | --- | --- |
| Backend admission | Schema marker | 0 | 0 | Marker bytes only |
| Context | Marker + Session + current Stream | 0 | 0 | Exactly those current envelopes |
| Fresh Session or automatic Stream report | Marker + target + named dependency | 0 | 1 target | Read those envelopes once; write only the resulting target envelope |
| Accepted report retry | Marker + target | 0 | 0 | No dependency read or rewrite |

These are adapter counters, not process-wide metrics. They exclude initialization/transport writes, failed partial I/O, filesystem metadata, configuration/native inputs, process starts and lock duration. Do not infer zero cost outside that boundary. Actual card and native open/resume budgets must exercise their real callers; a library context budget is not a card-read budget.

Records and embedded receipts can grow; the contract bounds addressed records, not constant byte size. Counting tests name files and directory listings, including setup and receipt access. Grow unrelated catalogs and history and require identical automatic-operation counts. Cover strict decoding, foreign-partition writes, changed-intent/lost-ack retries after later edits, same-host single-file moves, Pi branch fences, stale Stream membership captures and interrupted atomic replacement. Timing is not the acceptance test.
