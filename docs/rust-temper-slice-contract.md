# Rust + Temper Save/Publication/Recovery Slice Contract

Status: Stage 1 foundation implemented; Stage 2 publication/recovery adoption gate remains open. The Rust library currently executes only save actions through Temper. Publication witnesses and a Git coordinator must be implemented and reviewed together; the earlier disconnected witness scaffolding was removed because it could authorize unrelated attempts.

## Purpose and scope

This contract defines the preservation requirements, verification boundaries, and adoption gate for porting the Streams catalog save, publication, and recovery protocol to Rust with an embedded Temper I/O Automaton (`specs/catalog_protocol.ioa.toml`).

It addresses the five design-review findings before expanding the port to native Herdr workspace effects, the side-pane TUI renderer, or a remote service backend:

1. **Typed evidence, not unanchored boolean bags:** Temper's `TransitionTable` governs the discrete protocol phase transitions, while a private borrow-checked `SavePermit` binds runtime saves to their originating machine and immutable inputs. Publication evidence witnesses (`ConfirmedAttemptWitness`, `CandidateReuseWitness`, `AdoptionWitness`) remain planned for Stage 2 and will bind each publication transition and its side effects to the exact inspected revisions, operation digests, and markers.
2. **Total context projection:** Because Temper's `EvalContext` defaults missing declared booleans to `false` and counters to `0`, the Rust bridge enforces total projection against `table.initial_values` before calling `evaluate_ctx`. Omitting any declared variable is an immediate error.
3. **One unified save/publication/recovery protocol:** Save fencing (`save.json`), local persistence/commits, publication attempts (`publication.json` in `attempting` and `adopting`), confirmation recovery, and `repair-required` blocking are modeled in one automaton (`CatalogProtocol`) rather than split across disconnected specs.
4. **Independent outcome dimensions:** `Result` retains independent `sharing` (`local-only`, `pending`, `confirmed`, `blocked`, `failed`, `uncertain`), `local` (`unchanged`, `persisted`, `committed`, `repair-required`), and `domain` (`changes`, `no-change`, `superseded`, `conflict`) dimensions. In particular, confirmed remote publication with failed local adoption (`sharing="confirmed"`, `local="repair-required"`) remains representable and tested.
5. **Differential parity and fault-injected sequence testing:** Because `VerificationCascade` (L0 Z3 SMT, L1 Stateright BFS, L2 simulation, L3 `proptest`) verifies the abstract IOA specification rather than filesystem or Git subprocess execution, the Rust implementation is also verified by:
   - stateful `proptest` sequences injecting faults at every persistence and Git boundary (`save.json`, record write, `commit-tree`, `update-ref`, `push`, lost push response, `fetch` failure, `read-tree`/`update-ref` during adoption); and
   - a cross-language differential trace harness (`tests/python/test_rust_differential.py`) executing identical multi-step scenarios against Python and Rust and comparing outcomes, catalog snapshots, `.catalog-sync` files, and Git history.

## Preserved on-disk and wire contracts

The Rust implementation preserves record schemas and canonical operation-envelope bytes. Record YAML may be serialized as JSON (a YAML subset) to quote strings consistently for Python's YAML 1.1 reader:

- **Record files:** `streams/<escaped-id>.yaml`, `sessions/<escaped-host>/<escaped-id>.yaml`, `hosts/<escaped-id>.yaml` with `{version: 1, revision, kind, record}` envelopes and percent-encoding of uppercase, non-ASCII, and path-reserved bytes.
- **Process coordination:** `.catalog.lock` opened with `O_CREAT | O_RDWR | O_NOFOLLOW` (`0600`) and held via `flock(LOCK_EX)` without unlinking the inode.
- **Staging:** `.catalog-staging/` for atomic temporary writes + `fsync` of both file and parent directory.
- **Transport state (`.catalog-sync/`):**
  - `cli.json`: `{version: 1, host, strategy: "none" | "git"}`
  - `config.json`: `{version: 1, branch, host, remote}`
  - `local.json`: `{version: 1, creations: {<stream-id>: <canonical-operation-envelope>}}`
  - `receipts.json`: `{version: 1, operations: {<operation-id>: <sha256-hex-digest>}}`
  - `save.json`: `{version: 1, operation, start?}` (or adoption evidence) written before mutating records and cleared only after commit/persistence verification and receipt recording
  - `publication.json`: `{version: 1, phase: "attempting" | "adopting", base, branch, candidate, confirmed_at, local_tip, operations, remote, adoption_from?, adoption_to?}`
  - `sync.json`: `{version: 1, confirmed, fetched_at, last_confirmed_attempt, last_push, last_sync, refresh_error, refresh_retry_after, retry_after}`
- **Git commit envelopes:**
  - Bootstrap root: `"Streams catalog v1\n"` with an empty tree
  - Snapshot baseline root: `"Streams catalog baseline v1\n" + canonical JSON + "\n"`
  - Operation commit: `"Streams operation v1\n\n" + canonical JSON + "\n"`

## Legacy YAML migration boundary

Exact YAML 1.1 scalar resolution is not required for this local migration.
Existing files or Git snapshots with ambiguous unquoted strings (for example,
`title: 1e3`) may need manual quoting before Rust can read them. This is an
accepted limitation, not a reason to change Python's reader or implement a
custom compatibility resolver. New Rust writes quote strings through JSON
serialization. Schemas, operation-envelope bytes, ownership and durability
rules remain preservation requirements.

## Adoption gate criteria

Before proceeding to Stages 3–5 (CLI cutover, Herdr native effects, and TUI port), this slice must demonstrate:

1. `specs/catalog_protocol.ioa.toml` passes all levels of Temper's `VerificationCascade` (L0 Symbolic Z3, L1 Stateright Model Check, L2 Deterministic Simulation, L3 Property Tests), and intentionally broken spec mutations are rejected with counterexamples.
2. The Rust transition bridge rejects incomplete `EvalContext` projections and requires typed evidence witnesses for every guarded transition.
3. Stateful `proptest` fault-injection tests cover lost push acknowledgements, subsequent local saves across restarts, competing push races, reused operation IDs (same vs. mutated intent), and interrupted adoption.
4. Differential traces run against both Python (`_streams.catalog.sync`) and Rust produce identical `Result` / `ReadResult` payloads, catalog records, `.catalog-sync` state, and Git commit graphs.
