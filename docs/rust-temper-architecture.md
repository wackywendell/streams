# ADR: Rust runtime and embedded Temper verification

Status: Rust-only source cutover on this branch. The CLI, catalog, workspace runtime, native effects and terminal card use Rust; the Python package and duplicate TypeScript native implementation are retired. This does not change installed tools or establish live-platform readiness. The save, publication and recovery slice passed its [adoption gate](rust-temper-slice-contract.md#gate-result). The [durable work catalog ADR](durable-work-catalog.md), [Git sync ADR](catalog-git-sync.md), [protocol-boundary ADR](catalog-protocol-boundaries.md), [Pi work-card ADR](pi-work-card.md), and [native Herdr ADR](native-herdr-work-cards.md) remain the behavioral authorities during the staged port. A `temper-cloud` synchronization backend is specified separately in the [Temper Cloud sync ADR](catalog-temper-cloud-sync.md).

## Context and decision

Port the Streams CLI, catalog and synchronization engine, checkout and workspace runtime, Herdr native effect client, and side-pane TUI from Python and TypeScript to a single Rust workspace using embedded [Temper](https://github.com/nerdsane/temper) state-machine specifications (`temper-spec` and `temper-jit` in production, `temper-verify` in tests). Retain only the in-process Pi extension (`extensions/stream-checkpoint/`) in TypeScript, because it must execute inside Pi's Node.js process to observe live conversation trees, register slash commands and tools, and render the in-editor card.

This decision responds to a recurring pattern in repository history: state inconsistencies arose not in the immutable domain records or pure reconciliation function in the historical Python implementation, but at protocol boundaries where asynchronous or external effects met durable state. Examples include:

- applying generated reports after the conversation tree, working directory, or catalog attachment changed (`46bc64a3`);
- callers interpreting malformed transport state or combined local-save and publication receipts inconsistently (`d840da8d`);
- distinguishing filesystem failures before and after atomic record replacement (`79940870`);
- recovering from a lost Git push response when later local saves occurred before the next retry (`17d5b268`); and
- preserving both command failure and process-group termination evidence when native Herdr subprocesses time out or are cancelled (`13e3128a`).

Rather than building a custom model checker in Python and TypeScript, or keeping `.ioa.toml` specifications as disconnected sidecars that could drift from handwritten Python and TypeScript handlers, moving the runtime to Rust lets the same I/O Automaton specifications compile into production `TransitionTable` evaluators (`temper-jit`) and run through Temper's verification cascade (`temper-verify`: Z3 SMT, Stateright model checking, deterministic simulation, and `proptest` shrinking) in `cargo test`. Moving the Herdr workspace client to Rust also removes the cross-language lock helper, which spawned Python on every workspace lock acquisition solely to call `fcntl.flock` on an inherited descriptor. Keeping that retired coordinator as a fallback would preserve the Python dependency despite removing the Python CLI.

## Keep typed Rust evidence as the authority around Temper tables

An embedded Temper `TransitionTable` enforces discrete phase transitions, counters, booleans, and string lists, but it does not by itself make identity-dependent protocols or side-effect ordering correct by construction. Two constraints of Temper's evaluator and verifier shape this boundary:

1. **Identity abstraction:** Temper's verification model (`crates/temper-verify/src/model/semantics.rs`) treats arbitrary string and digest comparisons as unknown values rather than modeling full object IDs, commit hashes, or pane tuples. Projecting Rust state into unanchored booleans such as `candidate_confirmed = true` or `pane_attested = true` strips *which* candidate, revision, or pane was verified, and does not prove that the subsequent side effect uses the same identity.
2. **Defaulted context variables:** At runtime, `temper-jit` defaults missing declared booleans in `EvalContext` to `false` (`crates/temper-jit/src/table/guard.rs`). If a caller omits a boolean that appears negated in a guard (such as `!save_marker` or `!adopting_marker`), the transition can silently pass instead of failing closed.

To prevent split-brain logic between Rust and TOML:

- **Private typed evidence witnesses own identity:** Each protocol module defines private Rust state and witness types that carry the concrete verified identities—such as the exact `PublicationAttempt`, confirmed remote OID, expected snapshot revisions, or attested `(pane_id, tab_id, workspace_id, native_session_id)` tuple. Side-effect functions accept these witnesses rather than raw strings or boolean flags.
- **Total, co-located context projection:** Each protocol module projects its own state into `EvalContext` and verifies that every variable declared in `table.initial_values` is explicitly populated before calling `TransitionTable::evaluate_ctx`. A central predicate-bag module that couples catalog, Git, and Herdr state is avoided.
- **Unified durability protocols:** Because `evaluate_ctx` computes a transition without executing or persisting effects, closely coupled durability phases—save markers (`save.json`), record persistence, receipt recording (`receipts.json`), publication attempts (`publication.json`), confirmation recovery, and local branch adoption—are modeled together in `specs/catalog_protocol.ioa.toml` rather than split into independent state machines whose composition Temper would not verify.
- **Orthogonal outcome dimensions:** Rust result types preserve Streams' independent `domain` (`changes`, `no-change`, `superseded`, `conflict`), `local` (`unchanged`, `persisted`, `committed`, `repair-required`), and `sharing` (`local-only`, `pending`, `confirmed`, `blocked`, `failed`, `uncertain`) dimensions in the Rust sync outcome types. In particular, confirmed remote publication combined with failed local adoption (`sharing = "confirmed"`, `local = "repair-required"`) remains a first-class representable state, not collapsed into a single status enum.

## Keep the Pi capture boundary in TypeScript

A Rust CLI cannot observe in-process Pi state while a model request or command is in flight: whether the user switched conversation branches, whether the captured transcript leaf is still on the active branch, or whether the Pi session is shutting down (`extensions/stream-checkpoint/session-scope.ts`, `extensions/stream-checkpoint/native.ts`).

The TypeScript Pi extension therefore remains a safety-critical boundary rather than passive UI glue:

- `SessionScope` and `captureInputs` stay in TypeScript where the live `ExtensionContext` exists, fencing conversation lineage, working directory, and catalog selection before and after inference.
- TypeScript and the Rust `streams` binary exchange versioned JSON requests and receipts carrying the original operation identity, `catalog_source` fingerprint, and captured record preconditions.
- Sequence testing on the TypeScript side continues to cover tree switches, attachment changes, cancellation versus durable completion, and shutdown draining.

## Implementable boundaries, not just separate modules

The Rust rewrite must preserve Streams' modularity: frontend, synchronization, and persistence are choices behind implementable interfaces. Separate modules with concrete dependencies do not meet this requirement. Rust traits provide the extension points; this decision does not require loading binary plugins at runtime.

Application code uses a `CatalogBackend` interface rather than selecting concrete coordinators throughout the CLI. It exposes local snapshots, effective ownership authority, operation saves, read views, and explicit synchronization. An optional read refresh must not publish. Save receipts retain the independent domain, local, and sharing outcomes described above. A projected read view is not a persistence snapshot and cannot serve as a save precondition.

Synchronization remains modular across `none` (local-only), `git`, and service-backed (`temper-cloud`) modes. The common interface does not require every backend to implement Git's commits, candidate construction, or branch adoption. Configuration chooses a backend at the application boundary; the domain model and reconciliation do not make that choice.

### Persistence includes records and recovery bookkeeping

A `Store` provides a serialized transaction scope. Its `Transaction` reads snapshots, conditionally saves one record against the complete captured snapshot, and borrows its recovery journal and local-layout checks under the held lock. This scope does not promise rollback or a multi-record atomic commit. Implementations must detect stale snapshots, including changes that restore the original values, and preserve revisions for no-op saves. A failure after a possible write remains uncertain rather than becoming a definite rejection.

Snapshot revisions are opaque, store-bound tokens. The YAML adapter uses hashes of stored bytes; a table-backed adapter may use counters or database revision tokens. YAML is a persistence format, not the domain model: both adapters use the same validated `Catalog`, `Record`, and `Operation` types.

The transaction supplies the coordinator's recovery journal as a borrowed `&dyn Journal`; neither journal handles nor typed `State` views can escape its serialization scope. Records, creation evidence, retry receipts, and in-flight markers must share one serialization scope and one persistence identity. Abstracting record writes while leaving `.catalog-sync` hard-coded into the coordinator would not make persistence replaceable. The journal interface stores named objects; typed schema validation and save ordering remain in the coordinator/state layer. The YAML transaction privately owns its file journal alongside the catalog lock and maps those objects to the existing versioned JSON files without changing their wire format. Git publication and import evidence borrows the same locked coordinator context, not a long-lived catalog handle or a separately replaceable state view.

These choices are not an unrestricted cross-product. Git requires a versioned file representation, beyond the common store contract. A table-backed local store need not implement Git operations. Combining tables with Git would require a separately designed export/import adapter; it is not implied by implementing `Store`.

### Inspection does not enter a writer protocol

Saved replay inputs need an inspection boundary separate from ordinary catalog reads. A Git snapshot can establish missing retry receipts, and Git status can refresh its index. Neither effect belongs in capture.

`CatalogBackend::inspect` returns durable local records and effective ownership from one read-only scope. Backends and stores must opt into inspection; there is no fallback to an ordinary snapshot or transaction. The YAML adapter shares the existing catalog lock without creating it, rejects record and journal writes, and leaves missing bookkeeping absent. Git inspection validates history and recovery fences without remembering receipts or refreshing the index. The application returns detached inspection data, not the selected backend's write capability.

The proposed [native reconciliation contract](native-context-reconciliation.md) separates logical conversation identity from mutable native locations. Its executable model treats external topology and lifecycle changes as valid input; it is not production reconciliation or permission to bypass existing fences.

Host-local native state keeps endpoint-scoped associations, enrollment claims and pending-open evidence separate from portable catalog records. Endpoint scopes select files; a global per-Stream lock excludes competing opens across scopes. Session and launch command form one optional pair. Enrollment claims survive partial completion, and loaded pending-open records are diagnostic evidence, not resumable effect permits. Runtime orchestration must still establish live ownership and native identity before issuing effects; this storage layer does not attest either.

Enrollment does not open workspaces. For a native caller, it claims the workspace before creating catalog records and records each save separately. Failed directory binding leaves accepted records and the claim intact; the response reports incomplete completion rather than undoing saved work. Automatic Herdr/Pi enrollment checks the conversation before and after acquiring its locks, so a helper cannot enroll its parent's workspace.

Checkout identities use the shared host-wide registry; directory intent and native claims remain catalog-scoped. Enrollment and ordinary checkout commands must resolve the same registrations. Missing remotes are allowed, but malformed discovered locators remain errors, matching Python's explicit-binding validation.

### Frontends control workspaces, not the catalog

A workspace frontend interface identifies, resolves, creates, focuses, and updates native workspaces. Workspace references are scoped to a frontend and endpoint, rather than assumed to be Herdr pane identities. Runtime orchestration owns catalog associations, checkout selection, open/resume policy, and recording partial completion. Native transports do not choose catalog authority or save domain changes.

Capabilities that are not shared stay separate. Herdr pane attestation carries the exact pane, tab, workspace, and native-session identities. CMUX does not need to fabricate that evidence or claim support for arrangement restoration. Unsupported capture or launch capabilities fail explicitly; incomplete inventory is not an empty workspace, and uncertain native effects are not blindly retried. Frontend selection continues to reject ambiguous caller contexts instead of choosing whichever adapter answers first.

Caller inspection and effect authority have different lifetimes. Reporting checks the exact inherited workspace against ready associations, enrollment claims and explicit Stream identity. A partial open blocks reporting, but remains inspectable through `--current`. A claim can identify reporting context before enrollment completes; it cannot establish an open workspace. Neither read creates native-state locks or bindings.

`agent context --attest` additionally requires effective authority for this host, the exact local Pi Session, and fresh matching pane metadata. Optional `--native-inventory` reports every pane tuple, including conversations in other workspaces and panes with no Pi identity. Missing transcripts do not prove process death. These observations cannot authorize a later launch, replacement or cleanup: the native effect engine must obtain its own lock and recheck live evidence. This keeps reporting usable without exporting a reusable effect permit.

### Workspace effects retain uncertainty

Native open rechecks effective backend authority under global per-Stream exclusion and before each resource effect. Catalog owner fields alone cannot grant permission. A fresh attempt writes its journal and marks any existing binding not ready before dispatch. Its one-shot effect boundary persists intent, calls the adapter, then persists the checked response before allowing another effect. The embedded [native-open protocol](../specs/native_open.ioa.toml) prevents dispatch after uncertainty and prevents completion before a ready binding. Loaded journals cannot enter that protocol.

Herdr restore uses exact saved conversations and native layout order. A successful start receipt establishes interactive readiness, not conversation identity. Before advancing to another launch, the coordinator checks fresh host inventory for the just-started conversation in its acknowledged pane and confirms that launch durably. It rechecks the complete restored set before marking the workspace ready. A closed workspace binding can be archived only after complete, identity-verified host inventory establishes its absence. Missing session files do not establish that absence. CMUX retains its single-workspace command launch contract and does not claim multi-tab restoration.

An interrupted open can leave native resources and local intent behind. It keeps the pending attempt and refuses another automatic launch; there is no rollback or speculative recovery. This costs manual inspection after an ambiguous response but avoids creating another copy of a conversation. Terminal `resume` instead keeps the stream lock through `exec`, using an exact verified command. The lock closes at process replacement; it does not prove future process liveness or prevent independent launches outside native inventory.

### Historical documents do not change checkouts

Explicit revision lookup extracts a document into a host-local cache rather than switching branches or snapshotting a jj working copy. Card polling still resolves working files only. Named revisions are resolved again on each request; immutable identities may reuse cached content or missing-file hints. The returned revision retains the caller's spelling, while the cache key follows Python's resolved-revision convention.

The Rust cache requires private, user-owned directories and regular files. It rejects links and nonregular entries instead of following or repairing them. Broadly accessible caches created by Python therefore require a separate permissions audit before cutover; this slice does not silently chmod existing state. Cache extraction and native effect authorization remain separate capabilities.

### Conformance defines substitutability

Adapter tests must exercise the interfaces, not only concrete implementations. An independent non-filesystem store must pass the same storage and catalog laws as YAML, including journal durability/retry behavior. This establishes that the boundary does not depend on paths or byte hashes; it does not by itself establish production database crash durability. Native frontend implementations require their own transport and lifecycle acceptance tests before activation.

Backend conformance is divided into two layers:

1. **Common catalog laws (all backends):** stable operation identity, idempotent retry of identical `(operation_id, digest)` pairs, rejection of reused operation IDs with altered intent, captured-input and manual-field protection, ownership-period token checking, producer partitioning (`Session` and `Host` records writable only by their `host_id`), and honest separation of local persistence from remote sharing.
2. **Backend-specific laws:**
   - `none`: zero network or Git subprocess activity; immediate local persistence and creation authority.
   - `git`: linear operation commit history, temp-index candidate construction, lost-push-response recovery before retry, and fail-closed `adopting` / `repair-required` markers.
   - `temper-cloud`: online-first dispatch with local `confirmed` snapshot and FIFO `outbox` projection, documented in the [Temper Cloud sync ADR](catalog-temper-cloud-sync.md).

## Staged migration and verification gates

The old Python and Herdr suites imported implementation classes and patched their internal methods. Replacing only the executable would leave those suites exercising retired code. The migration therefore used differential traces, frozen wire/rendering fixtures and Rust interface, process and recovery tests before removing the old implementations. Python-only parity runners and tests of the duplicate native coordinator are now retired, not counted as current Rust coverage.

The original migration gates were:

1. **Stage 1 — Preservation contract and differential harness:** Document the exact on-disk YAML/JSON schemas, locking rules, and recovery states ([slice contract](rust-temper-slice-contract.md)), and build a cross-language differential harness that runs identical multi-step operation and fault traces against both Python and Rust.
2. **Stage 2 — Rust + Temper save/publication/recovery slice (adoption gate):** Establish the store, journal, and catalog-backend interfaces and shared conformance tests before extending the concrete coordinators. Implement the core catalog model, reconciliation, `LocalCatalog`, and `GitCatalog` in Rust backed by `specs/catalog_protocol.ioa.toml`. Verify with Temper's L0–L3 cascade, stateful `proptest` fault injection at every filesystem and Git step, and differential parity against Python. Continue with embedded Temper for remaining state machines only if this slice demonstrates clearer invariants and actionable counterexamples without excessive Rust/TOML duplication.
3. **Stage 3 — CLI, checkout, and workspace runtime cutover:** Port the CLI commands, checkout registry, and Herdr/CMUX workspace open/resume/capture orchestration to the `streams` Rust binary, keeping the Pi contract suites green.
4. **Stage 4 — Native Herdr lock and workspace client in Rust:** Move native workspace locking (`flock`), pane attestation, and naming/summary projection into Rust, retiring the Node-to-Python lock helper.
5. **Stage 5 — Native side-pane TUI (`ratatui`):** Extract portable rendering, Unicode width/clipping, shortcut-invalidation, and link/editor fixtures from the active Herdr view tests, then replace `work-cards-pane.js` with `streams-herdr-pane`.

### Core CLI integration

The Rust `streams` binary is this branch's sole catalog and runtime implementation. It supports setup, configuration, discovery, edits, sessions, checkouts and guarded agent requests. Installed tools outside this checkout are unchanged.

[Process tests](../tests/cli.rs) cover receipts, host-qualified sessions, Unicode selection and local-to-Git adoption. Rust wire fixtures preserve prior hashes, schemas and file layouts. [Pi lifecycle acceptance](../tests/stream_sync_test.mjs) uses real Rust processes and isolated Git catalogs, including offline saves and retry after restart.

An accepted save remains usable when ordinary publication fails. The receipt retains that warning; explicit `sync` returns exit 2 for sharing failures. Repair conditions and incomplete command effects also return exit 2 without discarding earlier save receipts.

The Rust stack is rebased onto Python `main` at `a6f11e5c`. Documents retain commit and source-session provenance, session files use stream/host partitions without changing logical identity, and attached sessions use the same host/harness/native UUIDv5 identity as Python. Generated Stream text can merge unrelated concurrent changes; Session inference still guards the whole captured target and explicit supporting inputs.

Only the current partitioned session layout is accepted. All 48 session files on this machine already use it, and the live catalog is local-only, so Rust does not carry the obsolete two-level reader or Git-history migration paths. Agent context retains a full local snapshot behind `CatalogBackend`, rather than copying Python's storage-specific lookup and cache fallbacks.

Native start, open, resume, arrangement capture and the Ratatui card use Rust. Launched cards require protected context and cannot act as their Pi owner. Acknowledged resource tuples remain journalled if later authority checks fail. Every effect re-attests its captured owner; arrangement capture pins the endpoint and checks owner/workspace identity under the Stream lock before persistence.

Pi retains its event lane, commands/tools, inline UI and session-name provenance. Reporting and native calls share one executable selector: an explicit absolute `STREAMS_BIN_PATH`, its compatible `STREAMS_RUST_BIN` alias, or this checkout's debug Cargo target. Conflicting selectors fail. There is no Python or `PATH` fallback; a missing build must not select an older installed implementation.

Saved helper requests use TypeScript preparation and execution with Rust's pure preview boundary. `streams debug` replaces its process with the Node runner before opening configuration or catalog state, retaining signal delivery and exit status. Capture keeps verified conversations when neighboring panes are unresolved, while unmatched historical conversations remain unknown rather than falsely closed.

Fake-service tests cover launch uncertainty, identity checks and authority loss. They do not establish deployment readiness. Isolated PTY and live-platform acceptance remain required before trial; this cutover neither activates native effects nor changes installed tools.

### TUI Unicode compatibility

Use ratatui's standard Unicode layout rather than reproduce every width rule of the JavaScript renderer. The extracted fixtures match except for five trailing U+0D4E Malayalam stress cases, where Rust treats the final grapheme as zero-width and JavaScript counts one column. Accept those five named differences, retain the original fixtures, and test the exact Rust outputs alongside unchanged document targets and links. This avoids a custom Unicode compatibility patch; it does not waive other rendering or shortcut checks. The [fixture notes](../src/tui/fixtures/README.md) record the cases.

## Alternatives considered

- **Keep Python and TypeScript, and add `.ioa.toml` files verified by `temper verify`:** Rejected. Because `temper-codegen` targets only Temper's Rust runtime, `.ioa.toml` files in a Python/TypeScript repository would be verified in isolation with no mechanical guarantee that the Python or TypeScript code matched them.
- **Keep Python and TypeScript, and use Hypothesis and `fast-check` without Rust:** Viable for sequence testing, and much cheaper initially, but keeps the dual Python/Node runtime footprint, interpreter startup overhead in Pi and Herdr hooks, and the Node-spawns-Python `flock` helper.
- **Use `temper-verify` only in `#[cfg(test)]` and hand-code Rust `match` blocks in production:** Avoids calling `temper-jit` at runtime, but reintroduces drift between the verified `.ioa.toml` specification and production transitions. If Stage 2 shows that `temper-jit` adds ceremony without catching faults beyond Rust typestate + `stateright`/`proptest`, this remains the fallback.
- **Run a mandatory local `temper-server` daemon as the primary store for all modes:** Rejected because `none` and `git` modes are explicitly designed to work as direct file-backed stores without a background daemon.

## Consequences

- **Positive:** One static Rust binary replaces the Python CLI, removes `uv`/Python startup overhead on lifecycle hooks, eliminates the Node-to-Python `flock` bridge, and runs both `temper-verify` (Z3, Stateright, simulation, `proptest`) and fault-injected sequence tests directly against production code in `cargo test`.
- **Negative:** Requires porting the active Python and Herdr TypeScript surfaces and extracting differential and rendering fixtures before retiring old suites. Linking `temper-verify` in dev-dependencies requires a local `z3` installation for `cargo test`, while production builds depend only on pure-Rust `temper-spec` and `temper-jit`.
