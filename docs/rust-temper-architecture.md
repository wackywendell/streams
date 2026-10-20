# ADR: Rust runtime and embedded Temper verification

Status: proposed; the Stage 1–2 save, publication, and recovery slice is under evaluation against the [slice contract](rust-temper-slice-contract.md). The [durable work catalog ADR](durable-work-catalog.md), [Git sync ADR](catalog-git-sync.md), [protocol-boundary ADR](catalog-protocol-boundaries.md), [Pi work-card ADR](pi-work-card.md), and [native Herdr ADR](native-herdr-work-cards.md) remain the behavioral authorities during the staged port. A `temper-cloud` synchronization backend is specified separately in the [Temper Cloud sync ADR](catalog-temper-cloud-sync.md).

## Context and decision

Port the Streams CLI, catalog and synchronization engine, checkout and workspace runtime, Herdr native effect client, and side-pane TUI from Python and TypeScript to a single Rust workspace using embedded [Temper](https://github.com/nerdsane/temper) state-machine specifications (`temper-spec` and `temper-jit` in production, `temper-verify` in tests). Retain only the in-process Pi extension (`extensions/stream-checkpoint/`) in TypeScript, because it must execute inside Pi's Node.js process to observe live conversation trees, register slash commands and tools, and render the in-editor card.

This decision responds to a recurring pattern in repository history: state inconsistencies arose not in the immutable domain records or pure reconciliation function (`src/_streams/catalog/operations.py`), but at protocol boundaries where asynchronous or external effects met durable state. Examples include:

- applying generated reports after the conversation tree, working directory, or catalog attachment changed (`46bc64a3`);
- callers interpreting malformed transport state or combined local-save and publication receipts inconsistently (`d840da8d`);
- distinguishing filesystem failures before and after atomic record replacement (`79940870`);
- recovering from a lost Git push response when later local saves occurred before the next retry (`17d5b268`); and
- preserving both command failure and process-group termination evidence when native Herdr subprocesses time out or are cancelled (`13e3128a`).

Rather than building a custom model checker in Python and TypeScript, or keeping `.ioa.toml` specifications as disconnected sidecars that could drift from handwritten Python and TypeScript handlers, moving the runtime to Rust lets the same I/O Automaton specifications compile into production `TransitionTable` evaluators (`temper-jit`) and run through Temper's verification cascade (`temper-verify`: Z3 SMT, Stateright model checking, deterministic simulation, and `proptest` shrinking) in `cargo test`. Moving the Herdr workspace client to Rust also replaces the cross-language lock helper in `integrations/herdr-work-cards/src/herdr/workspace-lock.ts`, where Node.js had to spawn a Python 3 subprocess on every workspace lock acquisition solely to call `fcntl.flock` on an inherited file descriptor.

## Keep typed Rust evidence as the authority around Temper tables

An embedded Temper `TransitionTable` enforces discrete phase transitions, counters, booleans, and string lists, but it does not by itself make identity-dependent protocols or side-effect ordering correct by construction. Two constraints of Temper's evaluator and verifier shape this boundary:

1. **Identity abstraction:** Temper's verification model (`crates/temper-verify/src/model/semantics.rs`) treats arbitrary string and digest comparisons as unknown values rather than modeling full object IDs, commit hashes, or pane tuples. Projecting Rust state into unanchored booleans such as `candidate_confirmed = true` or `pane_attested = true` strips *which* candidate, revision, or pane was verified, and does not prove that the subsequent side effect uses the same identity.
2. **Defaulted context variables:** At runtime, `temper-jit` defaults missing declared booleans in `EvalContext` to `false` (`crates/temper-jit/src/table/guard.rs`). If a caller omits a boolean that appears negated in a guard (such as `!save_marker` or `!adopting_marker`), the transition can silently pass instead of failing closed.

To prevent split-brain logic between Rust and TOML:

- **Private typed evidence witnesses own identity:** Each protocol module defines private Rust state and witness types that carry the concrete verified identities—such as the exact `PublicationAttempt`, confirmed remote OID, expected snapshot revisions, or attested `(pane_id, tab_id, workspace_id, native_session_id)` tuple. Side-effect functions accept these witnesses rather than raw strings or boolean flags.
- **Total, co-located context projection:** Each protocol module projects its own state into `EvalContext` and verifies that every variable declared in `table.initial_values` is explicitly populated before calling `TransitionTable::evaluate_ctx`. A central predicate-bag module that couples catalog, Git, and Herdr state is avoided.
- **Unified durability protocols:** Because `evaluate_ctx` computes a transition without executing or persisting effects, closely coupled durability phases—save markers (`save.json`), record persistence, receipt recording (`receipts.json`), publication attempts (`publication.json`), confirmation recovery, and local branch adoption—are modeled together in `specs/catalog_protocol.ioa.toml` rather than split into independent state machines whose composition Temper would not verify.
- **Orthogonal outcome dimensions:** Rust result types preserve Streams' independent `domain` (`changes`, `no-change`, `superseded`, `conflict`), `local` (`unchanged`, `persisted`, `committed`, `repair-required`), and `sharing` (`local-only`, `pending`, `confirmed`, `blocked`, `failed`, `uncertain`) dimensions (`src/_streams/catalog/sync/outcomes.py`). In particular, confirmed remote publication combined with failed local adoption (`sharing = "confirmed"`, `local = "repair-required"`) remains a first-class representable state, not collapsed into a single status enum.

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

A `Store` provides a serialized transaction scope. Its `Transaction` reads snapshots and conditionally saves one record against the complete captured snapshot. This scope does not promise rollback or a multi-record atomic commit. Implementations must detect stale snapshots, including changes that restore the original values, and preserve revisions for no-op saves. A failure after a possible write remains uncertain rather than becoming a definite rejection.

Snapshot revisions are opaque, store-bound tokens. The YAML adapter uses hashes of stored bytes; a table-backed adapter may use counters or database revision tokens. YAML is a persistence format, not the domain model: both adapters use the same validated `Catalog`, `Record`, and `Operation` types.

The storage boundary also supplies the coordinator's recovery journal. Records, creation evidence, retry receipts, and in-flight markers must share one serialization scope and one persistence identity. Abstracting record writes while leaving `.catalog-sync` hard-coded into the coordinator would not make persistence replaceable. The journal interface stores named objects; typed schema validation and save ordering remain in the coordinator/state layer. The YAML adapter maps those objects to the existing versioned JSON files without changing their wire format.

These choices are not an unrestricted cross-product. Git requires a versioned file representation, beyond the common store contract. A table-backed local store need not implement Git operations. Combining tables with Git would require a separately designed export/import adapter; it is not implied by implementing `Store`.

### Frontends control workspaces, not the catalog

A workspace frontend interface identifies, resolves, creates, focuses, and updates native workspaces. Workspace references are scoped to a frontend and endpoint, rather than assumed to be Herdr pane identities. Runtime orchestration owns catalog associations, checkout selection, open/resume policy, and recording partial completion. Native transports do not choose catalog authority or save domain changes.

Capabilities that are not shared stay separate. Herdr pane attestation carries the exact pane, tab, workspace, and native-session identities. CMUX does not need to fabricate that evidence or claim support for arrangement restoration. Unsupported capture or launch capabilities fail explicitly; incomplete inventory is not an empty workspace, and uncertain native effects are not blindly retried. Frontend selection continues to reject ambiguous caller contexts instead of choosing whichever adapter answers first.

### Conformance defines substitutability

Adapter tests must exercise the interfaces, not only concrete implementations. An independent non-filesystem store must pass the same storage and catalog laws as YAML, including journal durability/retry behavior. This establishes that the boundary does not depend on paths or byte hashes; it does not by itself establish production database crash durability. Native frontend implementations require their own transport and lifecycle acceptance tests before activation.

Backend conformance is divided into two layers:

1. **Common catalog laws (all backends):** stable operation identity, idempotent retry of identical `(operation_id, digest)` pairs, rejection of reused operation IDs with altered intent, captured-input and manual-field protection, ownership-period token checking, producer partitioning (`Session` and `Host` records writable only by their `host_id`), and honest separation of local persistence from remote sharing.
2. **Backend-specific laws:**
   - `none`: zero network or Git subprocess activity; immediate local persistence and creation authority.
   - `git`: linear operation commit history, temp-index candidate construction, lost-push-response recovery before retry, and fail-closed `adopting` / `repair-required` markers.
   - `temper-cloud`: online-first dispatch with local `confirmed` snapshot and FIFO `outbox` projection, documented in the [Temper Cloud sync ADR](catalog-temper-cloud-sync.md).

## Staged migration and verification gates

The existing 651 Python and 1,049 Herdr tests import Python and TypeScript classes directly and monkey-patch internal methods (such as `Git.run` in `tests/python/test_catalog_git.py` and view renderers in `integrations/herdr-work-cards/tests/`). Simply replacing the `streams` binary would leave those suites testing the retired code. Furthermore, as recorded in the [native Herdr ADR](native-herdr-work-cards.md), part of the imported Herdr test suite covers retired legacy Work Cards storage rather than active Streams behavior.

Migration therefore proceeds in gated stages rather than a single cutover:

1. **Stage 1 — Preservation contract and differential harness:** Document the exact on-disk YAML/JSON schemas, locking rules, and recovery states ([slice contract](rust-temper-slice-contract.md)), and build a cross-language differential harness that runs identical multi-step operation and fault traces against both Python and Rust.
2. **Stage 2 — Rust + Temper save/publication/recovery slice (adoption gate):** Establish the store, journal, and catalog-backend interfaces and shared conformance tests before extending the concrete coordinators. Implement the core catalog model, reconciliation, `LocalCatalog`, and `GitCatalog` in Rust backed by `specs/catalog_protocol.ioa.toml`. Verify with Temper's L0–L3 cascade, stateful `proptest` fault injection at every filesystem and Git step, and differential parity against Python. Continue with embedded Temper for remaining state machines only if this slice demonstrates clearer invariants and actionable counterexamples without excessive Rust/TOML duplication.
3. **Stage 3 — CLI, checkout, and workspace runtime cutover:** Port the CLI commands, checkout registry, and Herdr/CMUX workspace open/resume/capture orchestration to the `streams` Rust binary, keeping the Pi contract suites green.
4. **Stage 4 — Native Herdr lock and workspace client in Rust:** Move native workspace locking (`flock`), pane attestation, and naming/summary projection into Rust, retiring the Node-to-Python lock helper.
5. **Stage 5 — Native side-pane TUI (`ratatui`):** Extract portable rendering, Unicode width/clipping, shortcut-invalidation, and link/editor fixtures from the active Herdr view tests, then replace `work-cards-pane.js` with `streams-herdr-pane`.

## Alternatives considered

- **Keep Python and TypeScript, and add `.ioa.toml` files verified by `temper verify`:** Rejected. Because `temper-codegen` targets only Temper's Rust runtime, `.ioa.toml` files in a Python/TypeScript repository would be verified in isolation with no mechanical guarantee that the Python or TypeScript code matched them.
- **Keep Python and TypeScript, and use Hypothesis and `fast-check` without Rust:** Viable for sequence testing, and much cheaper initially, but keeps the dual Python/Node runtime footprint, interpreter startup overhead in Pi and Herdr hooks, and the Node-spawns-Python `flock` helper.
- **Use `temper-verify` only in `#[cfg(test)]` and hand-code Rust `match` blocks in production:** Avoids calling `temper-jit` at runtime, but reintroduces drift between the verified `.ioa.toml` specification and production transitions. If Stage 2 shows that `temper-jit` adds ceremony without catching faults beyond Rust typestate + `stateright`/`proptest`, this remains the fallback.
- **Run a mandatory local `temper-server` daemon as the primary store for all modes:** Rejected because `none` and `git` modes are explicitly designed to work as direct file-backed stores without a background daemon.

## Consequences

- **Positive:** One static Rust binary replaces the Python CLI, removes `uv`/Python startup overhead on lifecycle hooks, eliminates the Node-to-Python `flock` bridge, and runs both `temper-verify` (Z3, Stateright, simulation, `proptest`) and fault-injected sequence tests directly against production code in `cargo test`.
- **Negative:** Requires porting the active Python and Herdr TypeScript surfaces and extracting differential and rendering fixtures before retiring old suites. Linking `temper-verify` in dev-dependencies requires a local `z3` installation for `cargo test`, while production builds depend only on pure-Rust `temper-spec` and `temper-jit`.
