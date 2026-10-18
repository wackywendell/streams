# ADR: Durable streams without scratch notes

Status: implemented in this checkout's catalog, CLI, Pi extension and skill. The examples describe the domain, not a storage API. Work Cards integration remains pending; the installed runtime has not switched.

## Decision and rationale

Adapt Streams into a personal work catalog that helps the user understand each effort's goal, current state and remaining work. Keep its harness-neutral CLI and workspace integrations, but replace Markdown scratch notes with structured stream and session records. Plans, design decisions and detailed explanations belong in referenced project documents. Resumability follows from understanding the work; it is not the organising principle for its description.

Scratch notes mixed status information with rationale and grew into long, largely unreviewed documents. Stronger brevity instructions did not solve that problem. Removing the free-form body makes the boundary explicit, at the cost of requiring useful explanations to be written in project repositories.

Streams already provides discovery, lifecycle commands and session resumption. Extending it avoids rebuilding those capabilities in Work Cards. Retain Work Cards' distinction between shared intent and individual contributions, but not a second authoritative catalog. Tickets and pull requests remain references: parking personal work need not change a team-facing ticket.

## The unit of work

A stream represents an effort the user prioritises, parks, finishes and returns to as a unit. Generally, one Herdr workspace represents one stream; its tabs and sessions contribute to that effort. This redesign is one stream, even though several sessions handle different strands. Completing a session's assignment does not complete the stream.

The stream survives closing its workspace and changing machines. Its ID is independent of its title and native workspace identity. Registration is automatic once meaningful work context exists, with an explicit-registration mode available. Agents may refine names and goals while preserving the effort's scope and manual overrides.

Retain `active`, `waiting`, `blocked`, `parked` and `done`. Parked work is unfinished but off the user's current priority list. Archive is an independent visibility flag: any status can be archived, and restoring visibility preserves that status. Archived work remains searchable. Neither archiving nor changing status necessarily closes workspaces or deletes checkouts, although it is expected those will often happen together.

Agents may report progress, but personal priority, parking, completion and archival require user direction. An explicit instruction in any attached conversation is sufficient; the advisor is not an exclusive gatekeeper or a reason to ask for redundant confirmation.

## Logical data model

The logical model defines streams, sessions, their fields and relationships, and valid operations on them. A stream holds the whole effort; each session describes its contribution and names its stream. Do not also store a session list in the stream, since the two memberships could disagree.

Implement these definitions and rules independently of serialization, filesystem layout and Git. Storage adapters depend on the logical model, not vice versa. They map physical representations to and from logical records; the CLI, advisor and Work Cards consume the logical model. The examples below use YAML as notation, not as a requirement of that model.

### Stream record

The stream holds the whole effort's goal, lifecycle, owning host and references. Progress descriptions belong to sessions, avoiding a second account that must be kept in sync. This example shows the proposed domain fields:

```yaml
id: str_catalog
title: Durable personal work catalog
title_mode: automatic
goal: Keep a clear view of ongoing work, independent of running sessions.
goal_short: Understand ongoing work
goal_mode: automatic
status: active
archived: false

owner:
  host_id: host_laptop

repositories:
  streams:
    url: https://github.com/wackywendell/streams

documents:
  - repository: streams
    ref: lifecycle-design
    path: docs/durable-work-catalog.md
    description: Lifecycle and data-model decisions

issues: []
prs: []
```

The naming modes distinguish automatic text from protected manual choices. `goal` describes the intended outcome and may be null while that outcome is unspecified. In that case `goal_short` is also null; creation must not invent placeholder text. An overall progress view can be assembled from session descriptions and reference documents when needed, rather than maintained as another record. Scope not assigned to a session belongs in the goal or a referenced plan.

Repository aliases belong to the stream. Documents identify a repository, relevant remote branch/bookmark, relative path and description. Issue and PR references contain URLs and descriptions. Missing local checkouts must not erase these references. Non-repository work may have an empty repository map.

### Session record

A session belongs to a host and references one stream. Its native identity is `(host_id, harness, harness_session_id)`; the catalog ID locates its record. Resuming the same session on that host reuses the record. A fork with a new native session ID gets another record.

```yaml
id: ses_lifecycle
host_id: host_laptop
stream_id: str_catalog
harness: pi
harness_session_id: native-session-uuid
title: Lifecycle design
title_mode: automatic
status: active
state: Draft ADR written.
state_short: Model drafted
remaining: Agree record fields and management operations.

checkout:
  repository: streams
  ref: lifecycle-design
  directory_within_checkout: "."
```

`title` concisely names the contribution; there is no separate long assignment description. `state` describes its substantive progress and present condition, and `remaining` its unfinished scope, not just the next action. These are short descriptions, not a growing journal or task backlog. The session's status concerns its assignment, not whether its process is running or the stream is complete. Recovery uses the named remote ref's pushed state; it does not require a pinned commit.

Each session normally updates only its own record. Persistence must preserve accepted update order, without requiring a per-session counter in the logical model. Stream attachment and ownership checks remain separate from progress reporting. Optional placement observations can preserve last-known tab inclusion and order without storing native pane identities in the catalog.

### Mapping to Work Cards

| Streams source | Current Work Cards field | Surface |
| --- | --- | --- |
| `stream.title` | workspace `title` | Herdr workspace title and card-pane caption |
| `stream.goal` | workspace `goal.text` | Long shared goal in the card |
| `stream.goal_short` | workspace `goal.short` | Short workspace description |
| `session.title` | agent `title` | Pi session name, eligible agent-tab name and card heading |
| `session.state` | agent `now` | Long current-state description |
| `session.state_short` | agent `statusShort` | Short agent description |
| `session.remaining` | agent `remaining` | Remaining work in the card |
| `session.status` | agent `status` | Agent status indicator |
| Stream document, issue and PR references | workspace references | Card links, resolved through local checkout bindings |

The short fields are optional display derivatives. `goal_short` condenses the goal; `state_short` summarises the session's status, state and remaining work. Generate them alongside the full descriptions, not by truncation or separate per-surface inference. Regenerate or clear a short field when its source content changes. The short workspace description is purpose, not aggregate progress.

These naming mappings are defaults, not permission to overwrite manual names. Work Cards retains independent workspace, tab and Pi naming controls and overrides in local runtime state. A tab rename does not redefine the catalog title or Pi session name. Editor-tab names continue to come from document identity. The stream's lifecycle status has no equivalent in the current shared Work Cards record; it must not replace the agent-status indicator.

### Freshness and host state

Storage may report when a record was last edited. Display this as “record last updated,” not “progress last verified.” A description may combine information from several times, so there is no single evidence timestamp for the prose.

Runtime inspection returns process state with its check time, separately from the session description. An unavailable host means unknown, not stopped. A host check-in contains host identity, display label and check-in time: a concrete observation that does not freshen its sessions' descriptions. Publication, fetch and user priority-review times likewise remain distinct from record updates.

## Initial physical data model

The first storage adapter maps these logical records to versioned YAML files in a Git repository with one shared branch:

```text
<catalog Git repository>/
  streams/<stream-id>.yaml
  sessions/<host-id>/<session-record-id>.yaml
  hosts/<host-id>.yaml

$XDG_STATE_HOME/streams/                 # host-local, not synchronised
  host.yaml
  bindings/<session-record-id>.yaml
  workspaces/<stream-id>.yaml
  sync/
```

Encoding versions, directory conventions and Git commits belong to this adapter, not the logical records. Stable IDs determine filenames, so renaming and archiving do not move files. Git history supplies edit metadata; Git ancestry and locally serialised operations supply save ordering. Filesystem timestamps are not durable across clones and checkouts. Local files hold installation identity, absolute paths, saved-session locations, native ownership/layout information and sync bookkeeping.

The adapter must preserve logical identities, relationships and operation semantics, and expose revisions and save outcomes without leaking storage mechanics into domain rules. JSON files, database tables or ticket-system fields could provide another physical mapping. A ticket-backed catalog would still need independent personal lifecycle state, not equate it with a team ticket's status. Only YAML/Git is in scope now.

## Host ownership and resumption

A stream has one owning host at a time. Delayed reports from a previous period of ownership must not become current state, even if the stream has since returned to that same host. The operations therefore need a way to distinguish ownership changes; the exact version token is an internal consistency detail. Old session records remain useful historical context, but their native session IDs are not resumable on another host.

Ownership governs execution and reporting, not where the user may manage the catalog. An advisor elsewhere may request a handover. The handover must check the expected previous owner and activate the new owner only after shared storage confirms the change. With the Git backend, it attempts immediate publication rather than waiting for batched sync. A failed or offline request can remain saved and visibly pending, but grants no execution authority.

This is authority over accepted reports, not a distributed process lock. An unreachable old host may still be running. Handover must expose that uncertainty; it cannot claim to have stopped the old processes. Once the old host learns of the transfer, it stops stream-owned automatic effects. Returning ownership to it requires reattachment under the new ownership.

Resumption should reconstruct the previous working arrangement where practical, not always select the last recorded session. Tools expose known tabs, sessions and recovery options so the user or advisor can restore the set, choose a subset or start replacements. Reuse a verified live workspace or saved sessions on the owning host. After cross-host handover, start fresh sessions from the stream's goal and references, its sessions' descriptions, and the last pushed branches or bookmarks. Session titles and descriptions explain each contribution's context.

Recovery does not promise uncommitted edits, synchronised transcripts or exact pane geometry. Launch results must identify what actually opened, including partial failures, rather than encouraging retries that duplicate resources.

## Management and persistence contract

The `streams` CLI is the primary interface for listing, searching, inspecting and managing work, with human-readable and structured output. Pi tools and any future picker use the same operations. Catalog access does not require a live workspace; runtime actions use the relevant harness or frontend adapter. A custom browser is not needed initially.

The domain layer validates operations against logical records. Operations carry stable IDs, exact targets and relevant preconditions. Independent changes may combine; conflicting intent must be surfaced. Adapters provide revision-checked persistence; domain rules decide whether changed data conflicts. Git sync replays domain operations rather than merging arbitrary YAML, preserving operation identity and rejecting superseded ownership or stale updates. Generated descriptions still need input checks before saving, since save order alone cannot prevent an old inference from finishing late. Session progress updates do not rewrite the stream record.

For the initial Git backend, ordinary saves succeed after a local commit. Publication is separately pending, failed or confirmed, and unpublished changes remain for later reconciliation. Sync is session-driven or explicitly requested, with batched reports and check-ins; it does not require a host daemon. Cached reads and local work continue offline. Handover is the exception to delayed activation because an unpublished ownership claim cannot authorise another host.

## Remaining work

This decision establishes the record boundaries and lifecycle, not a complete CLI specification. The [README](../README.md) describes implemented commands. Handover, Work Cards projections and multi-session reconstruction remain follow-on work. Git hosting, batching intervals and retry mechanics belong to the sync design.

No automatic migration or legacy-format compatibility is required. Existing scratch material may be kept or migrated manually. The [Pi work-card design](pi-work-card.md) replaces scratch-generating checkpoints with catalog reporting. The [scratch format contract](../src/_streams/FORMAT.md) and [checkpoint specification](pi-stream-checkpoint-spec.md) describe the historical integration, not the current catalog workflow.
