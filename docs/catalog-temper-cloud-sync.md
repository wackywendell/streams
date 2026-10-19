# ADR: Online-first Temper Cloud sync with local outbox and explicit reseed

Status: proposed; follow-on synchronization backend after the core Rust cutover ([Rust and Temper architecture ADR](rust-temper-architecture.md)). The [durable work catalog ADR](durable-work-catalog.md) owns logical record semantics, and the [Git sync ADR](catalog-git-sync.md) continues to govern `none` and `git` synchronization.

## Context and decision

Add a `temper-cloud` synchronization backend that uses a personal [Datadog Temper Cloud](https://github.com/ddoghq/temper-cloud) instance as the shared catalog authority across hosts (such as a Mac laptop and remote Cloud Workspaces), while retaining a complete local YAML snapshot and a FIFO operation outbox on every host.

Two simpler service models were rejected during design:

- **Pure online-only client:** Making every `streams` mutation a direct network call with no local persistence eliminates client sync state, but breaks catalog edits, session enrollment, and Pi progress reporting whenever a laptop is offline or disconnected from the corporate network.
- **Git-style client commit rebuilding against a server:** Using a server merely as a remote target for batch sync while keeping Git's local commit graph and candidate-adoption pass retains the most error-prone part of Git sync (`AdoptingAttempt` and `read-tree` branch replacement) even though a server actor can serialize and reconcile operations directly.

Instead, the `temper-cloud` backend is **online-first when connected, local-outbox-backed when offline, and explicitly re-seedable if server storage resets**:

1. Every host maintains its full local YAML catalog (`confirmed`, the last state acknowledged by the server) plus an ordered local `outbox` (`.catalog-sync/outbox.json`) of unacknowledged `Operation` envelopes.
2. When online, writes append to `outbox`, dispatch immediately to the `temper-cloud` instance, and—upon acknowledgement—update `confirmed` and remove the operation from `outbox`.
3. When offline, writes append to `outbox` and return `local = "persisted"`, `sharing = "pending"`. Reads and local authority fold `outbox` over `confirmed` through the domain's pure `reconcile` function.
4. If the `temper-cloud` instance storage is reset or replaced, normal sync refuses to overwrite local files and requires an explicit `streams sync --reseed` command to repopulate the server from local state.

## Why offline conflicts remain bounded without Git history

In general distributed systems, offline queues can accumulate complex multi-writer conflicts. Streams' logical data model ([durable work catalog ADR](durable-work-catalog.md)) makes offline conflicts rare by construction:

- **`Session` and `Host` records are producer-partitioned:** A host only ever writes `sessions/<this-host-id>/*` and `hosts/<this-host-id>.yaml`. While a machine is offline, no other machine can access its native conversations or legally write to its producer partition.
- **New `Stream` creations use fresh identities:** A stream created offline receives a fresh UUID and establishes local creation authority immediately; no other host knows of or can mutate that stream until the creating host reconnects.
- **Automatic `Stream` inference is owner-gated:** Background agent refinement of a stream's title or goal requires the active `ownership_token`. While the owning host is offline, another host can only modify that stream if the user explicitly performs a manual edit or ownership handover on the second host.

Consequently, when an offline host reconnects and drains its `outbox` in FIFO order:

- all `Host`, `Session`, and newly created `Stream` operations apply without contention;
- any automatic `Stream` report generated under an `ownership_token` that was superseded while the host was offline is cleanly retired with `outcome = "superseded"`; and
- only a concurrent manual `Stream` edit made offline on one host and online on another produces a domain `conflict`, which pauses that specific operation for user direction without blocking unrelated streams or session progress.

## Client state and the disappearance of local branch adoption

In the `git` backend, a client must rewrite its local Git commit graph (`L1 -> L2`) into the rebased remote commit graph (`R1 -> C1 -> C2`) after pushing, creating a window where remote publication is `confirmed` while local branch adoption is `repair-required`.

With `temper-cloud`, the server's entity actors own the canonical event sequence. Each host stores:

```text
<catalog root>/
  streams/<stream-id>.yaml               # confirmed server snapshot
  sessions/<host-id>/<session-id>.yaml
  hosts/<host-id>.yaml
  .catalog-sync/
    cli.json                             # {version: 1, host, strategy: "temper"}
    temper.json                          # instance URL, tenant, last_sequence, epoch_id
    outbox.json                          # FIFO list of pending Operation envelopes
    receipts.json                        # durable operation-id -> sha256 digest map
    save.json                            # in-flight local file replacement marker
```

The host's effective local catalog at any moment is:

$$\text{effective\_catalog} = \text{fold}(\text{reconcile}, \text{confirmed}, \text{outbox})$$

### Write and retry flow

1. **Local durability first:** Under `.catalog.lock`, the client validates the `Operation` against `effective_catalog`, appends it to `outbox.json` (and records its digest in `receipts.json`), and releases any local caller immediately if the server cannot be reached within a short connection budget.
2. **Immediate online flush:** When reachable, the client drains `outbox.json` in FIFO order by invoking the corresponding Temper OData action for each `Operation`.
3. **Lost response recovery without an adoption phase:** If a network timeout or crash occurs after `temper-cloud` commits an operation but before the client receives the HTTP response, the operation remains at the head of `outbox.json` with its original `operation.id`. On the next flush, the client re-sends the same `Operation`. The server's durable receipt ledger recognizes the `(operation_id, digest)` pair, returns the current entity state without re-applying effects, and the client advances `confirmed` and pops the operation from `outbox.json`.

Host-local runtime state (`$XDG_STATE_HOME/streams/`—checkout registrations, directory bindings, Herdr Unix sockets, pane attestations, and workspace locks) remains strictly on the execution host and is never sent to `temper-cloud`.

## Datadog hosting evaluation and `temper-cloud` constraints

Three internal Datadog hosting options were evaluated using Confluence and Jira (`TMPR` project) documentation as of October 2026:

| Option | How it runs | Trade-offs for Streams |
| --- | --- | --- |
| **`ddoghq/temper-cloud` (Selected)** | Shared `temper-controlplane` (`gizmo.us1.staging.dog`) authenticates callers via Datadog ISA (`ddtool auth token`), binds instance ownership to `authenticated_subject` (`TMPR-38`), and provisions per-owner Temper runtime sandboxes on TensorLake ([Getting started with Temper](https://datadoghq.atlassian.net/wiki/spaces/CSE/pages/7342915850/Getting+started+with+Temper)). | Zero server infrastructure to maintain per user and native Datadog identity. However, sandbox storage does not yet provide production backup or retention guarantees, and TensorLake snapshot lifecycles have undergone active stabilization (`TMPR-46`, `TMPR-47`, `TMPR-50`). |
| **Persistent Cloud Workspace ("Dogpark Hub" pattern)** | Run `temper serve --storage turso` on a persistent Datadog Workspace backed by a local SQLite/Turso file, reached from the Mac via an auto-reconnecting SSH `-L` port forward ([Run it on a workspace](https://datadoghq.atlassian.net/wiki/spaces/~712020c733f6056b6649bfab166cc091fa3cac/pages/7338297304/Dogpark+Run+it+on+a+workspace)). | Persistent workspace disk under direct user control, but requires keeping a dedicated workspace running and managing an SSH tunnel daemon (`keeper`) on the Mac. Remains fully compatible with the same OData client if needed as a fallback URL. |
| **`devkube` (`entei.us1.staging.dog`)** | Personal Kubernetes namespace (`devenv-<user>`) with OrgStore Postgres ([What do devkubes provision?](https://datadoghq.atlassian.net/wiki/spaces/~6318f96f8d88ec800fbffaf6/pages/7341965527/2026W43+What+do+devkubes+provision)). | Requires static `devkube_installation` Bazel targets in `dd-source` and garbage-collects idle namespaces; disproportionate operational overhead for a personal work catalog. |

Inspection of `nerdsane/temper` and `ddoghq/temper-cloud` identified three runtime constraints that the Streams Temper app specification must handle explicitly:

1. **Single-leader actor assumption (Temper ADR-0046):** Each Temper `EntityActor` caches current state and `sequence_nr` in memory; all hosts must talk to one active `temper-cloud` instance rather than writing concurrently to a shared database behind the server's back.
2. **In-memory HTTP `IdempotencyCache` (`crates/temper-server/src/idempotency.rs`):** Temper's built-in HTTP `Idempotency-Key` cache is an in-memory `BTreeMap` capped at 1,000 entries with a 1-hour TTL and does not survive a server restart. To preserve Streams' durable retry guarantee across restarts, operation IDs and payload digests must be persisted inside the Temper entity state/journal rather than relying solely on HTTP idempotency headers.
3. **Closed IOA state-variable types (Temper ADR-0181):** Temper IOA `[[state]]` variables support only `string`, `bool`, `counter`, `int`, and `list` of strings. Structured collection fields (`repositories`, `documents`, `issues`, `prs`, and `Session.checkout`) must either be canonically encoded in string lists or modeled as child entities in `catalog.csdl.xml` with shared validation in the Rust domain codec.

## Detecting server resets and explicit `--reseed` recovery

Because `temper-cloud` explicitly notes that *"production data retention, backups, and recovery are not provided for those sandboxes"*, the client must treat its local `confirmed` YAML files and `outbox.json` as a durable replica capable of rebuilding the remote instance, never as a disposable cache that empties when the server returns an empty list.

### Epoch tracking (`CatalogMeta('root')`)

When a catalog is initialized on `temper-cloud` (or re-seeded), the client creates a singleton entity `CatalogMeta('root')` on the server containing:

- `epoch_id`: a fresh UUID identifying that storage initialization;
- `owner_host_id` and creator identity; and
- the schema/spec version.

The client stores `epoch_id` in `.catalog-sync/temper.json`. Because Streams never deletes `CatalogMeta('root')` during normal operation:

- **Missing `CatalogMeta('root')` (`404`) or missing app specs on a previously initialized catalog:** Proves that the `temper-cloud` sandbox was reset or recreated.
- **Changed `epoch_id`:** Proves that another host owned by the user already re-seeded the server after a reset.

### Fail-closed detection and `streams sync --reseed`

When a normal read refresh or write sync detects that `CatalogMeta('root')` is missing on `temper-cloud`:

1. **Do not erase local records or re-seed silently:** The client keeps all local `confirmed` YAML files and `outbox.json` intact, records a reset diagnostic, and returns `sharing = "blocked"` with instructions to inspect and run `streams sync --reseed`.
2. **Explicit re-seed (`streams sync --reseed`):** When invoked by the user:
   - pushes the Streams `.ioa.toml`, `.csdl.xml`, and `.cedar` specifications to the `temper-cloud` instance if they are absent;
   - creates a new `CatalogMeta('root')` with a fresh `epoch_id`;
   - uploads all records from the host's local `confirmed` snapshot along with recorded creation/ownership tokens and operation fingerprints (`receipts.json`); and
   - replays the host's pending `outbox.json` operations on top, updating `.catalog-sync/temper.json` to the new `epoch_id`.
3. **Secondary host convergence after a re-seed:** When a second host (such as a remote Cloud Workspace) later syncs and observes a new `epoch_id` created by the first host's `--reseed`, it does not discard its local state. Instead, it reconciles any records present in its own local `confirmed` snapshot and `outbox` that are missing or newer on the server—specifically its producer-owned `sessions/<second-host-id>/*` and `hosts/<second-host-id>.yaml` records, plus any streams for which it holds the latest ownership token—before adopting the new `epoch_id`. Because producer partitions are disjoint across hosts, the union of the hosts' local copies reconstructs the catalog across a total server wipe.

## Consequences and open questions

- **Positive:** Gives Streams low-latency online sync and simple single-actor ownership handover without sacrificing offline operation or risking data loss when a `temper-cloud` sandbox is wiped. Keeping `confirmed` as standard YAML files preserves human inspectability on disk and reuses the existing local storage layout.
- **Negative:** Requires maintaining the Temper app specification (`specs/catalog.csdl.xml`, `.ioa.toml`, `.cedar`) alongside the Rust client, and requires user action (`streams sync --reseed`) after a sandbox reset.
- **Open questions to resolve before implementation:**
  - **Authentication Plumbing:** Verify whether the `streams` CLI should invoke `ddtool auth token` directly (with caching) or delegate token acquisition to the installed `temper` CLI config (`ddoghq/temper-cloud`) once `TMPR-44` (broad ISA group admission) is live in staging.
  - **Secondary-host epoch join policy:** Decide whether a secondary host seeing a new `epoch_id` from another host of the same authenticated owner should merge its producer-owned records automatically during `streams sync`, or also require `--reseed` / `--join-epoch`.
