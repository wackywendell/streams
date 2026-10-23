# ADR: Record-local operations with one writer per file

Status: decided, not implemented. This replaces the lookup-only proposal in [Bounded, event-driven work](bounded-work-and-card-events.md) and the authority/replay design in [Git sync](catalog-git-sync.md). Local durability remains separate from publication. The [effect-planning ADR](effect-planning.md) still governs native effects. The single-host rule below supersedes cross-host participation and ownership-transfer proposals in earlier records.

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
- Capture takes a Herdr workspace snapshot and reads each Pi pane's Session by its native-identity key. Its budget grows with that workspace, not the Stream or catalog.
- `open` and `resume` read Session IDs from the saved arrangement, which capture alone writes. They read those Sessions directly and skip any whose current membership no longer matches. An explicit Session request can name its record directly. Without either source, they must not discover candidates implicitly.
- `show` and listing a Stream's Sessions are explicit user discovery. They may enumerate host-scoped Session files, like `list` and human title resolution, but never on an event or report path.

Any new automatic operation that needs a roster must first identify a record that already owns its answer, rather than introducing duplicated membership and coordination.

## Read, apply, write

Each operation type declares a small typed input set: its target and captured dependencies. The target record also holds its retry receipts. It does not take an arbitrary `Catalog` plus unrestricted inputs. The shell reads and strictly decodes those records once. Pure apply compares captured intent with current records and returns a changed target, no change, superseded intent or conflict. Storage atomically replaces that one target, including its receipt, and returns the resulting values without reopening the catalog. Unrelated revisions cannot reject the operation. Actions do not coordinate writers, retry effects or recover partial transactions; if one needs that machinery, change its model instead.

Apply assesses conversation-local and Stream-shared proposals separately. A Session report checks its host, captured branch/context generation and the current Stream's same-host relationship, not its pre-move membership. A Stream report checks the target Stream and the captured Session's current Stream ID and membership period. Each move creates a new membership period, including a return to the same Stream, so old Stream prose cannot become valid again or be retargeted to the destination. These report guards do not change conversation identity or native card lifetime. Relevant harness lifecycle events invalidate branch/context generations before admitting old model work. Manual-text protection and reuse-of-operation-ID checks remain.

Strict decoding includes supported schema versions, unique object keys, regular files, safe paths, physical identity and relationships among the named records. A malformed record is not absence. Creation distinguishes a missing key from unreadable or conflicting state. Whole-catalog validation belongs to explicit inspection and cutover, not opening a backend handle. `list` and `check` may enumerate current records; neither replays history.

Human title and substring matching stays in the CLI shell. A human-supplied title triggers explicit discovery using the same enumeration as `list`, resolves to exactly one host-qualified ID or fails on ambiguity, and then invokes the operation by exact key. This is user-requested discovery before the operation, not a fallback inside it. Exact-ID CLI requests, the Pi child and cards do not enumerate records. Discovery of a foreign Stream does not grant permission to modify it.

### Membership move contract

A move reads exactly 3 catalog inputs: the Session, the old Stream and the new Stream. The Session supplies the old Stream key; verified destination input supplies the new key. The domain entry point takes those typed records and verified move input, not a complete `Catalog`. All 3 records must belong to the current host. If a destination Stream must be created, that is a separate local creation before the move, not a multi-file transaction or permission to scan for another Stream.

Pure apply returns only the updated Session, or no change/conflict. It changes membership and its period while preserving work text, conversation identity, checkout URL and directory intent. Storage writes the Session and its receipt in one atomic file replacement. Neither Stream is written; their revisions remain unchanged. There is no roster, multi-file write set, recovery fence, rollback or other Session input.

The catalog layer owns this function and persistence. Native reconciliation supplies verified movement input and calls it; it does not implement catalog reconciliation. Native relationship/evidence changes remain separate effect plans owned by the child/executor, preserving external-effect uncertainty. They cannot expand this catalog action into a coordinated transaction.

### Durability without history as a journal

Local saves acknowledge durable record changes, not publication. They do not create or verify Git commits or run inline sync. They may emit a separate sync trigger; a due publication check cannot hide a catalog scan inside a report response. Sync failure cannot turn an accepted local save into a clean failure inviting a duplicate action.

Stable operation IDs and exact-intent fingerprints prevent double application and changed-intent retries after lost acknowledgements. The retry key is `(record identity, operation ID)`. Keep its fingerprint and accepted outcome in the target record's envelope, written atomically with the record. There is no second receipt file, host-wide receipt map, separate journal or history lookup. Retain enough receipt identities to recognize earlier retries after later edits; a last-operation-only marker would lose that guarantee. Receipts are deduplication evidence, not replayable operations.

Each save replaces and flushes one file. Storage reports post-replacement uncertainty honestly, but actions do not construct a recovery protocol around it. A subsequent explicit invocation can read the same record and its receipt. There is no multi-file transaction, pending-transaction scan or generic recovery fence.

Enrollment creates a missing local Stream first, then its Session as separate single-record operations. Interruption may leave an empty generated Stream; that is acceptable in alpha software and needs no rollback. Session and Stream report proposals are admitted and saved independently, not as an atomic batch. Neither case may claim that a second save succeeded merely because the first did.

## Direct identities and paths

Session identity remains `(host, harness, native session ID)`. Derive its deterministic ID using the existing `agent::session_id_for` identity preimage. Membership is not part of its path. A concrete layout is `hosts/<encoded host>/streams/<stream ID>.yaml`, `hosts/<encoded host>/conversations/<derived Session ID>.yaml` and `hosts/<encoded host>/host.yaml`. Encode path components and verify each decoded identity against its location. A host plus an explicit or arrangement-saved Session ID locates that Session directly. Host-qualified keys also locate read-only replicas without searching for their owner.

Change legacy IDs at explicit cutover instead of keeping aliases or a native-ID index. Captured requests carry full identity and schema/context version. A missing Session means unenrolled only in the completed new format; invalid or incomplete migration means repair. No stale key or child startup falls back to discovery.

Keep portable Stream/Session records, the host-wide checkout registry and host-local per-Session directory bindings separate. Read the registry once and pass its decoded value through directory and document resolution. Stream-only directory defaults remain separately Stream-keyed. Each Session checkout always carries its resolved portable repository URL, populated at attach or creation. A Stream-local alias is only a label, so repository edits cannot orphan unread Sessions and moves cannot reinterpret aliases through the destination.

Native workspace relationships also need direct addresses. Represent reservations and relationship phases at their enforcement keys: native workspace and host-local Stream association. Those few authoritative records must establish the existing exclusivity rules without enumerating every association, claim and attempt. Disagreement or unresolved reservation blocks the action; a pane address cannot repair it by inference.

## Native actions

A native operation reads its own records and takes one fresh snapshot under the native lock, then applies observations and plans actions as pure data. The executor preserves record-intent → effect → record-outcome. Mismatch or uncertainty stops the plan and retains its fence. Display, observation and effect permissions remain distinct; cards do not become catalog writers.

The [native reconciliation ADR](native-context-reconciliation.md) owns card lifetime and native evidence identity: catalog-scoped conversation identity plus launch nonce. Current membership and owner location are observations, not lifetime identity. The child acts; the card observes. This decision preserves inherited-ID bootstrap checks and the offline `streams native migrate-evidence` contract: archive old bytes, quarantine unattributable attempts for explicit resolution, and install the completed-format marker only after legacy records and quarantined uncertainty are resolved. Normal operations read one conversation-keyed evidence record, never legacy keys or a mixed layout. Name ledgers stay direct workspace/tab records.

## Sync transports files

Sync will use Git commit/pull/push over the host-partitioned files. Before committing, it rejects local edits outside the current host's partition. Different hosts' changes have disjoint paths by construction, so Git can combine those paths without interpreting record fields. Concurrent changes to the same file are an error: fail closed, retain both sides and report the conflict. Do not use text, field, set or domain merge to choose a result, even if Git could combine different lines in that file automatically.

Local apply enforces actor and captured-input guards once. Sync does not replay operations, re-evaluate report captures, derive authority or preserve one-operation-envelope-per-commit history. A later input change can leave accepted generated prose stale until the next report; that is acceptable for alpha software. Retry receipts travel with their target record envelopes. Checkout selections, registry files and native evidence remain host-local and outside Git.

Sync owns its Git interaction and reports publication or installation failures without silently fixing invalid records. It may consult history and validate incoming schema, host paths and relationships. It does not promise a cross-file atomic catalog snapshot, implement a generic recovery transaction, or make ordinary reads investigate transport state. If a named dependency is missing or invalid during installation, that operation refuses it; a later record event says what to re-read. Local saves remain durable if transport fails.

## Explicit cutover

Run one offline deploy migration after backup, with writers/event consumers stopped and the owning locks held. This is alpha software: change formats, discard and regenerate derived prose when needed, and do not build compatibility layers. Populate fixed host identities, deterministic Session IDs, host-partitioned paths and checkout URLs. Validate Session uniqueness and same-host membership. Do not migrate or add a Stream roster. Ambiguous old ownership or pending transfer state requires an explicit host choice or recreation, not conversion into new write permission. Ownership transfer is not retained as a feature.

The cutover may start a fresh validated Git sync baseline rather than preserve replay compatibility with old operation envelopes. Coordinate that baseline across replicas; old writers and sync processes must reject the new completed format. Do not rewrite old operation history to make ordinary operations work. Translate usable references and per-Session directory selections, separate Stream defaults, and invalidate old captured requests. Reject conflicting selections or clear and reselect them instead of choosing by timestamp or directory order. Install the completion marker only after the usable new state is durable. No runtime fallback or old-ID alias service remains.

Migrate or reset runtime associations, claims and arrangements to directly keyed relationship records. Pending workspace-open and card/editor attempts are the safety exception to discarding generated state: preserve their uncertainty, nonce and resources while an external resource might exist. Follow their explicit offline migration/recovery contract. A report or card read never runs cutover, and this ADR does not authorize touching live state.

## Inventory and operation budgets

The current-source column below describes the implementation before this redesign. Common costs include `application::open` and `agent::context` full snapshots, receipt/history-based authority and `YamlTransaction::save` whole-catalog revision comparison plus pre/post reads. Those dependencies must disappear rather than move behind a keyed API.

Budgets include fixed configuration/producer/schema files, counted separately by name. A save reads its target's embedded receipt, not another file or recovery marker. Single-Session counts are independent of unrelated records and history. Capture declares a per-workspace-pane budget; arrangement-based open/resume declare a per-saved-Session budget. The checkout registry is read once per operation, not once per repository.

| Operation | Current dependencies | Required record set and changes |
| --- | --- | --- |
| Report / save | Full catalogs, captured inputs, host-wide receipts, authority/history, inline sync and post-save reads | Session + current Stream for conversation work; target Stream + captured Session for Stream work; only typed captured dependencies. Pure admission separates branch and membership guards. Save each target independently with its embedded receipt; no Git activity or transaction fence. |
| Card read / ordinary context | Repeated snapshots and authority, all Stream Sessions for inventory, legacy directory discovery, registry rereads and document checks | Session + current Stream + Session binding + registry. Return only this Session. Native evidence/snapshot are separate event inputs. Check only this Stream's document availability when required. |
| Native project | Repeated source contexts, evidence, name ledgers and snapshots | Session + Stream, own evidence, exact workspace relationship and affected name ledgers; one snapshot. No directory or document reads to project titles/summaries. |
| Native capture | Full catalog filtered to workspace members, authority, relationship enumeration and topology | One workspace snapshot → each Pi pane's Session by native-identity key; exact Stream/workspace relationship and arrangement target. Capture alone writes the arrangement. No roster or catalog scan; file count grows only with workspace subjects. |
| Native open | Full source contexts, evidence, assets and repeated absence/launch/confirmation checks | Session + Stream, own evidence, exact relationship, fixed assets and one snapshot followed by planned effects/replies. Persist pending intent before dispatch. Require completed evidence format. Read directory inputs only if launch needs them. |
| Native close | Full source contexts before/after, evidence, markers and confirmation | External: Session + Stream, own evidence, exact relationship, one snapshot, effect and bounded confirmation. Card q/owner-exit: exit only, no evidence or marker writes. Child/executor handles pane.closed/pane.exited with one positive-absence snapshot, or checks on next start; uncertainty remains. No directory/document reads. |
| Enroll / move | Catalog/native-ID scans, authority/history, duplicate caller snapshots, relationship enumeration, directory discovery, separate saves and inline sync | Enrollment creates a local Stream before its Session as separate single-record saves. Move reads Session + old Stream + new Stream, all same-host, and atomically writes only the Session with its receipt. Native relationship updates are separate plans. No roster, fence or other Session writes. |
| Workspace open / resume | Full catalog member selection, relationship/arrangement enumeration and repeated authority checks | Stream + saved arrangement + exactly the listed Sessions, skipping changed membership; or an explicitly requested Session. Exact native relationships and needed checkout/directory inputs; one frontend observation and ordered plan. No implicit member discovery. |
| Stream show | Full catalog, title resolution, all members and optional refresh | Explicit user discovery may enumerate host-scoped Sessions and filter by stream_id. Human title discovery likewise precedes exact-key actions. Neither enumeration runs on events/reports. Refresh is separate sync. |
| Sync | Operation history/replay, receipts, complete catalogs and domain candidate validation | Git commit/pull/push of partitioned files; path/schema/relationship validation. Same-file concurrent changes fail, never merge. No domain replay or generic recovery transaction. No host-local runtime state enters Git. |

Records, embedded receipts and saved arrangements can grow; the contract bounds addressed records, not constant byte size. Counting tests name files and directory listings, including setup and receipt access. Grow unrelated catalogs and history and require identical automatic-operation counts. Cover strict decoding, foreign-partition writes, changed-intent/lost-ack retries after later edits, same-host single-file moves, stale branch/membership captures and interrupted atomic replacement. Timing is not the acceptance test.
