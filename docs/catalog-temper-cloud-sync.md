# ADR: Online-first Temper Cloud sync with local outbox and explicit reseed

Status: proposed, not implemented. [Record-local operations](record-local-operations.md)
now governs `none` and `git`. The earlier service sketch assumed catalog-wide
operation replay and transferable ownership; those assumptions are retired.
This proposal needs a new record-local interface design before implementation.
It does not authorize an outbox, scheduler, reseed command or ownership handover.

## Goal and retained trade-offs

A personal [Datadog Temper Cloud](https://github.com/ddoghq/temper-cloud) instance
could transport work records between hosts while local saves remain useful offline.
The intended service behavior is online-first, locally durable when disconnected,
and explicitly recoverable after server storage resets.

A pure online-only client would simplify sync state but break offline reporting.
Rebuilding a Git-style local commit graph against a service would keep complexity
that a server does not need. The earlier FIFO outbox was one proposed alternative,
not an existing runtime interface or a settled record-local design.

All current record types, including Streams, have one immutable writing host.
Cross-host manual edits and handover are not exceptions. A service backend must
preserve those partitions, embedded retry receipts, captured report admission and
the distinction between local persistence and publication. Host-local checkout
registrations, directory bindings, native sockets and process locks never become
shared catalog records.

The service must persist duplicate-intent evidence if it promises retries across
server restarts; an HTTP idempotency cache alone is insufficient. The exact client
queue, acknowledgement and reset model remains to be designed against current
record interfaces. The old `Operation`, ownership-token, standalone receipt and
save-marker layouts are not compatibility contracts to recreate.

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
   - uploads preserved record envelopes, including their embedded retry receipts; and
   - replays the host's pending `outbox.json` operations on top, updating `.catalog-sync/temper.json` to the new `epoch_id`.
3. **Secondary host convergence after a re-seed:** When a second host (such as a remote Cloud Workspace) later syncs and observes a new `epoch_id` created by the first host's `--reseed`, it does not discard its local state. Instead, it reconciles any records present in its own local `confirmed` snapshot and `outbox` that are missing or newer on the server—specifically its immutable `hosts/<second-host-id>/` partition—before adopting the new `epoch_id`. Because producer partitions are disjoint across hosts, the union of the hosts' local copies reconstructs the catalog across a total server wipe.

## Consequences and open questions

- **Positive:** Would provide online sync and offline local work, subject to designing the record-local durability and reset boundaries. It does not add ownership handover or guarantee recovery from a sandbox wipe. Keeping durable local record envelopes would preserve human inspectability.
- **Negative:** Requires maintaining the Temper app specification (`specs/catalog.csdl.xml`, `.ioa.toml`, `.cedar`) alongside the Rust client, and requires user action (`streams sync --reseed`) after a sandbox reset.
- **Open questions to resolve before implementation:**
  - **Authentication Plumbing:** Verify whether the `streams` CLI should invoke `ddtool auth token` directly (with caching) or delegate token acquisition to the installed `temper` CLI config (`ddoghq/temper-cloud`) once `TMPR-44` (broad ISA group admission) is live in staging.
  - **Secondary-host epoch join policy:** Decide whether a secondary host seeing a new `epoch_id` from another host of the same authenticated owner should merge its producer-owned records automatically during `streams sync`, or also require `--reseed` / `--join-epoch`.
