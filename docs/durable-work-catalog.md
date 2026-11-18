# ADR: Durable streams without scratch notes

Status: implemented. This record owns field meaning and lifecycle.
[Record-local operations](record-local-operations.md) owns fixed-host storage and
transport; [native cards](native-herdr-work-cards.md) owns presentation. Examples
illustrate logical fields, not the stored JSON.

## Decision and rationale

Adapt Streams into a personal work catalog that helps the user understand each effort's goal, current state and remaining work. Keep its harness-neutral CLI and workspace integrations, but replace Markdown scratch notes with structured stream and session records. Plans, design decisions and detailed explanations belong in referenced project documents. Resumability follows from understanding the work; it is not the organising principle for its description.

Scratch notes mixed status information with rationale and grew into long, largely unreviewed documents. Stronger brevity instructions did not solve that problem. Removing the free-form body makes the boundary explicit, at the cost of requiring useful explanations to be written in project repositories.

Streams already provides discovery, lifecycle commands and session resumption. Extending it avoids rebuilding those capabilities in Work Cards. Retain Work Cards' distinction between shared intent and individual contributions, but not a second authoritative catalog. Tickets and pull requests remain references: parking personal work need not change a team-facing ticket.

## The unit of work

A stream represents an effort the user prioritises, parks, finishes and returns to as a unit. Generally, one Herdr workspace represents one stream; its tabs and sessions contribute to that effort. This redesign is one stream, even though several sessions handle different strands. Completing a session's assignment does not complete the stream.

The Stream survives closing its workspace and can be read on other hosts, but its writing host is immutable. Its ID is independent of its title and native workspace identity. Registration is automatic once meaningful work context exists, with an explicit-registration mode available. Agents may refine names and goals while preserving the effort's scope and manual overrides.

Retain `active`, `waiting`, `blocked`, `parked` and `done`. Parked work is unfinished but off the user's current priority list. Archive is an independent visibility flag: any status can be archived, and restoring visibility preserves that status. Archived work remains searchable. Neither archiving nor changing status necessarily closes workspaces or deletes checkouts, although it is expected those will often happen together.

Agents may report progress, but personal priority, parking, completion and archival require user direction. An explicit instruction in any attached conversation is sufficient; the advisor is not an exclusive gatekeeper or a reason to ask for redundant confirmation.

## Logical data model

The logical model defines streams, sessions, their fields and relationships, and valid operations on them. A stream holds the whole effort; each session describes its contribution and names its stream. Do not also store a session list in the stream, since the two memberships could disagree.

Implement these definitions and rules independently of serialization, filesystem layout and Git. Storage adapters depend on the logical model, not vice versa. They map physical representations to and from logical records; the CLI, advisor and Work Cards consume the logical model. The examples below use YAML as notation, not as a requirement of that model.

### Stream record

The stream holds the whole effort's goal, lifecycle, owning host and references. Progress descriptions belong to sessions, avoiding a second account that must be kept in sync. This example illustrates the domain fields:

```yaml
id: str_catalog
title: Durable personal work catalog
title_mode: automatic
goal: Keep a clear view of ongoing work, independent of running sessions.
goal_short: Understand ongoing work
goal_mode: automatic
status: active
archived: false
revision: 4
updated_at_ms: 1767225600000

owner:
  host_id: host_laptop

repositories:
  streams:
    url: https://github.com/wackywendell/streams

references:
  - name: Catalog model
    purpose: Define lifecycle and data-model decisions
    role: brief
    state: active
    target:
      kind: document
      repository_url: https://github.com/wackywendell/streams
      path: docs/durable-work-catalog.md
      ref: lifecycle-design
      commit: immutable-commit-id
      source_session_id: bc26a2a9-d12f-59cc-9a80-b9efbf910ff1
```

The naming modes distinguish automatic text from protected manual choices. `goal` describes the intended outcome and may be null while that outcome is unspecified. In that case `goal_short` is also null; creation must not invent placeholder text. An overall progress view can be assembled from session descriptions and reference documents when needed, rather than maintained as another record. Scope not assigned to a session belongs in the goal or a referenced plan.

Repository aliases configure the Stream's repositories. Document references identify an exact repository URL and portable relative path. PR, Issue and Link share exact URL identity. Name, purpose, role, state and recovery metadata remain outside that identity. Each Stream holds one relationship per exact resource key.

Optional ref, commit and source Session hints support explicit document recovery. Missing hints, files or local checkout mappings leave the saved reference intact. Non-repository work may have an empty repository map.

Roles describe Deliverable, Dependency, Brief and Background relationships. Active and Fulfilled references remain visible; Dropped retains identity and blocks automatic restoration. Delete forgets a relationship and permits rediscovery. The [reference contract](stream-references.md) defines those meanings and their conservative conversation-evidence rules. Runtime accepts the required references array and rejects former collection fields.

### Session record

A session belongs to a host and references one stream. Its native identity is `(host_id, harness, harness_session_id)`; the catalog ID locates its record. Resuming the same session on that host reuses the record. A fork with a new native session ID gets another record.

```yaml
id: bc26a2a9-d12f-59cc-9a80-b9efbf910ff1
host_id: host_laptop
stream_id: str_catalog
membership_epoch: 0
harness: pi
harness_session_id: native-session-uuid
title: Lifecycle design
title_mode: automatic
status: active
recap: Designed the lifecycle model and drafted its ADR.
recap_short: Lifecycle model
state: Reviewing the record fields.
state_short: Field review
remaining: Agree the record fields.
remaining_short: Agree fields
revision: 7
updated_at_ms: 1767225600000

checkout:
  repository: streams
  repository_url: https://github.com/wackywendell/streams
  ref: lifecycle-design
  directory_within_checkout: "."
```

`title` concisely names the contribution. `recap` describes this conversation's cumulative scope and achievements in a sentence or two. Rewrite it only when the scope or main outcome changes. `state`, labelled Status, describes what it is doing right now in one short sentence. `remaining` describes unfinished work; reports select one concrete next action or blocking decision. These are descriptions, not a growing journal or task backlog. Session text is always report-writable; title modes protect only titles. New fields default to empty text or absent short forms, so older records remain readable without a reset. The session's status concerns its assignment, not whether its process is running or the stream is complete. Recovery uses the named remote ref's pushed state; it does not require a pinned commit.

Session IDs derive deterministically from host, harness and native ID, as in this example. `stream_id` is current membership; `membership_epoch` counts moves, so it distinguishes a return to the same Stream. A move changes only this Session and preserves its work and checkout. A historical membership grants no write authority.

Portable `checkout` can be null, especially after automatic enrollment. When supplied it includes an explicit URL and ref. Host-local directory selection is separate and does not populate it. Each Session normally updates only its own record; progress reporting does not rewrite the Stream.

### Mapping to Work Cards

| Streams source | Current Work Cards field | Surface |
| --- | --- | --- |
| `stream.title` | workspace `title` | Herdr workspace title |
| `stream.goal` | workspace `goal.text` | Long shared goal in the card |
| `stream.goal_short` | workspace `goal.short` | Short workspace description |
| `session.title` | agent `title` | Pi session name and eligible agent-tab name |
| Stream and Session titles | card caption | `Stream title: Session title` |
| `session.recap` | Session block | Cumulative scope and achievements |
| `session.recap_short` | `strm_recap_short` | Optional Pi-pane sidebar hint |
| `session.state` | agent `now` | Long current-state description |
| `session.state_short` | `strm_state_short` | Optional Pi-pane sidebar hint |
| `session.remaining` | agent `remaining` | Remaining work in the card |
| `session.remaining_short` | `strm_remaining_short` | Optional Pi-pane sidebar hint |
| `session.status` | agent `status` | Agent status indicator |
| Stream references | workspace references | Non-Dropped Document, PR, Issue and Link rows, resolved through explicit checkout bindings |

The short fields are optional display derivatives. Each condenses only its matching full text. Generate them alongside those descriptions, not through separate per-surface inference. Prefer 1–2 words, shorter is better, preferably fewer than 20 characters and always fewer than 25. Report parsing caps generated hints at 24 Unicode characters; stored overlong hints remain readable but are not projected. Regenerate or clear a short field when its own source changes. The short workspace description is purpose, not aggregate progress.

These naming mappings are authoritative when their synchronization switches are enabled; see [name ownership](name-ownership.md). Disable a surface's synchronization to manage its label independently. A direct tab rename does not redefine the catalog title or Pi session name. Editor-tab names continue to come from document identity. The stream's lifecycle status has no equivalent in the current shared Work Cards record; it must not replace the agent-status indicator.

### Freshness and host state

Each record carries `revision` and `updated_at_ms`, which the save path sets only when the record changes. A Stream's activity time is the latest `updated_at_ms` of the Stream and its member Sessions, computed by one function for listing, the sidebar, cards, decay and tidy. Display this as “record last updated,” not “progress last verified.” A description may combine information from several times, so there is no single evidence timestamp for the prose.

Runtime inspection returns process state with its check time, separately from the session description. An unavailable host means unknown, not stopped. A host check-in contains host identity, display label and check-in time: a concrete observation that does not freshen its sessions' descriptions. Publication, fetch and user priority-review times likewise remain distinct from record updates.

## Physical records and host-local state

Current portable records are host-partitioned:

```text
<catalog>/
  hosts/<host>/host.json
  hosts/<host>/streams/<stream-id>.json
  hosts/<host>/conversations/<deterministic-session-id>.json

<host-local state>/                    # not synchronized
  installation identity
  checkout registry
  per-Session directory bindings and exact transcript locators
```

Each file holds one record as JSON, with no envelope. The physical
host address is part of identity and is checked against the record. Stable IDs
keep renames, archive and membership moves from moving files. No saved native
addresses, arrangement, ownership-transfer period or legacy layout is read.

One immutable host writes a Stream and all of its Sessions. Synchronized copies
are read-only elsewhere. Cross-host participation and handover are unsupported.
A native workspace move cannot grant permission to write another host's partition.

Explicit reopening discovers active same-host Sessions and opens missing ones in
separate tabs, leaving observed running conversations where they are. Single
`resume` replaces the invoking terminal with the selected saved conversation.
Neither restores tab order or exact geometry. Missing launch inputs are errors,
not permission to start a replacement conversation. Native reuse is best effort,
with visible uncertainty and rare duplicates rather than a durable launch journal.

## Management and persistence contract

The `streams` CLI is the primary interface for listing, searching, inspecting and managing work, with human-readable and structured output. Pi tools and any future picker use the same operations. Catalog access does not require a live workspace; runtime actions use the relevant harness or frontend adapter. A custom browser is not needed initially.

Pure apply checks typed captured inputs, manual protection, host and membership. Storage atomically replaces one target when it changes. The record revision guards captured edits, and a retry that already landed is recognised from the record. Generated descriptions still need input checks because save order alone cannot prevent old inference from finishing late.

Both storage modes acknowledge local record persistence without Git or network activity. Explicit sync or Pi lifecycle sync commits and transports files separately. Disjoint host paths combine; same-file concurrent changes fail without text or domain merge. Publication failure cannot undo an accepted save. There are no interval settings, scheduled read projections or background daemon.

## Boundaries

This decision establishes record meaning and lifecycle, not a complete CLI specification or a claim that every recovery path is implemented. The [README](../README.md) owns current commands and setup. [Record-local operations](record-local-operations.md) owns persistence and file transport; the [Pi ADR](pi-work-card.md) owns evidence and guarded reporting; the [native Herdr ADR](native-herdr-work-cards.md) owns the Work Cards adaptation.

The alpha activation used an explicit shutdown, backup and fresh initialization. Runtime code accepts only the current format. There is no migration subsystem, old-record reader or second writable scratch/Work Cards store.
