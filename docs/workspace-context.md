# ADR 2 Shared workspace context and per-session agent state

Status: accepted; the workspace and agent runtime is active. General foreground management and the complete automatic naming rollout remain unfinished. See [status and boundaries](#status-and-boundaries). [ADR 1](design.md) defines the product purpose, evidence policy and authority limits.

## Context and problem

One card cannot cleanly represent both shared project intent and several agents' independent assignments. One agent's progress must not replace another's status or rewrite the group's references.

Agents in one Herdr workspace can work in different repositories or worktrees. A shared document must open the same file for each agent, regardless of its working directory. Moving a session to another workspace must change its shared context without copying the source workspace's links.

## Decision

### Separate shared intent from own-agent progress

Use two durable record types:

| Record | Owns |
| --- | --- |
| Workspace | Shared title, plan-derived Goal, checkout registrations, documents, issues, PRs and removal exclusions |
| Agent | Pi session identity, workspace membership, checkout, actual working directory, title, status, Now and Remaining |

Each card pane combines the workspace Goal and references with its own agent's assignment and progress. Several agent/card pairs can share a native workspace. There is no workspace-overview tab.

Goal describes shared intent, not aggregate progress. Status, Now and Remaining describe the agent's assignment, not the whole project's backlog. One agent's Now must not become the workspace summary.

Keep optional compact purpose and progress fields alongside the full text. Generate them in the existing helper requests, not by truncation or separate inference calls. Validate their display-cell limits, clear stale derivatives when their source text changes without a replacement, and leave unknown values absent. Compact progress remains per agent.

The Pi session UUID identifies the agent. Resuming that UUID retains its record; a fork with a new UUID gets another record. A session-file path is only a locator. Titles are presentation, never identity. Runtime code supplies identities, membership and directories; helpers cannot change them.

Records remain authoritative and local. Native metadata binds a live workspace to a durable workspace record and projects display information. It does not become a repository registry or durable project database.

### Give documents explicit checkout identities

A checkout is a canonical directory on the execution host, not a remote URL or repository name. A workspace-local table assigns stable keys to those directories. Two worktrees remain distinct even when they belong to the same repository. Directory aliases reuse the same canonical entry.

A document reference identifies a checkout key and relative path. An agent separately records its checkout and actual working directory, which may be a subdirectory. Neither viewing-agent cwd nor display labels determine document targets.

Do not silently relocate a registered checkout or recycle its key. References, exclusions and agent records retain their checkout identities. Missing files remain removable references; opening reports failure rather than guessing another checkout. Canonical path checks enforce the document boundary described in [ADR 1](design.md#make-local-records-authoritative).

### Follow verified native membership

The live Herdr workspace defines the group, not a shared repository directory or GUI focus. Reconciliation follows verified terminal identity through authoritative native inventories. Public pane IDs can change during moves while terminal identity survives.

Use serialised polling, including while Pi is idle. Pi has no native workspace-movement event. Polling avoids a separate raw-socket subscription client and a higher minimum Herdr version solely for events. Detection is eventual, not instantaneous. Missing or ambiguous identity suspends effects.

Automatic startup and joining wait for meaningful task context. Configurable manual operation waits for explicit open or join; attached summaries still continue. In manual mode, observing a move suspends source-workspace effects until explicit destination joining.

On a verified move, retain the agent identity, assignment and status. Join the destination context and register or reuse the checkout there. Leave source documents, issues and PRs in the source. Invalidate old-membership work before it can affect the destination or continue changing the source. An unbound destination gets fresh context; selecting an existing durable workspace is explicit, not a guessed copy.

### Separate evidence by purpose

| Purpose | Permitted source |
| --- | --- |
| Own-agent title, status, Now and Remaining | Projected compaction plus retained human-readable dialogue, with shared context |
| Shared-reference additions | Latest projected compaction, available dialogue and inline file-operation paths, judged against shared purpose and known references |
| Workspace Goal | Selected registered purpose-bearing Markdown documents only |

Reference collection considers the full projected session context for new and existing workspaces, whether attachment is explicit or automatic. It does not read raw precompaction logs. The helper judges each candidate against the current workspace title, Goal and known references, rather than copying every mention. When shared purpose is unknown, session context can identify useful starting documents; Goal still comes only from their contents.

Reference evidence preserves inline `read`, `write` and `edit` operations as operation-and-path entries. It excludes tool bodies, outputs, shell-command parsing and hidden history. Codemode nested metadata requires a still-visible parent call. Activity suggests candidates, not successful work or automatic admission. Own summaries and document-derived Goal do not receive this activity. Captured evidence may grow during inference, but edits or removals invalidate its result.

Explicit reset is fresh inference, not record deletion or a membership reset. Own and reference helpers receive projected evidence without prior generated own fields, shared title, Goal or reference arrays. Own inference must return every derived field; partial proposals and `no_change` fail without clearing stored text. Manual titles remain protected. Reference additions still merge with all stored references and removal exclusions, because reference origin is not tracked. Reset cannot relabel unprovided existing links. Other agents retain their progress.

Reset bypasses the source-selection and Goal caches but keeps Goal document-only. It does not recover raw precompaction history. Sequential stage publication remains unchanged: a failed later stage does not undo an earlier validated update.

Attachment boundaries detect discontinuities and distinguish fresh task input for automatic startup. They do not limit reference eligibility. Resume, tree navigation, compaction and moves still invalidate stale requests through evidence generations and membership checks. Relevance is a model judgment; explicit removal exclusions prevent historical context from restoring removed references.

Select purpose-bearing designs, ADRs, relevant README files and plans from known document references, not through a recursive repository crawler. No suitable source means no invented Goal. Runtime code reads sources within registered checkouts, with a 256 KiB limit per file. Oversized or unreadable selected sources fail visibly rather than being silently truncated.

Represent Goal as one readonly in-memory value: unevaluated with no provenance, evaluated unknown with nonempty sources and a fingerprint, or known with nonblank full text, optional short and the same required provenance. Constructors enforce text and provenance constraints; one pure transition owns evaluation replacement and invalidation when a selected source is removed. Unchanged full text can retain an omitted short; changed text cannot inherit it. Keep manual title application separate.

Parse the existing flat version-1 YAML into this domain shape at the storage boundary and serialize it back only on a genuine mutation. The persisted fields and public tool/show record shape remain unchanged. Validate wire constraints before normalizing blank purpose or short values. Reads and semantic no-ops preserve original bytes, modification time and revision; there is no eager migration. Domain validation and wire parsing accept only their respective shapes.

Retain selected-source provenance with the Goal, including an evaluated result that establishes no purpose. Changes to candidate references trigger reselection. Scheduled checks reread selected sources and reuse unchanged Goal results; explicit refresh reruns selection and inference. Revalidate source identities and bytes before accepting results so an inference cannot silently use changed files. Ordinary dialogue is not a reason to rewrite shared Goal. A plan establishes intent, never completed work.

The model still interprets whether a document establishes purpose and what it means. Authority checks and provenance do not guarantee correct selection or a useful Goal.

### Make shared additions additive and removals explicit

Session-evidence helpers may update only their own agent, propose shared additions and generate derived labels for exact existing issue/PR URLs. Source-derived updates use the separate source policy above. Neither path may remove references, delete records, register arbitrary roots or write project documents. Separate request contracts enforce these limits rather than trusting a prompt to preserve arrays.

Documents are a small current reading list, not a history of useful files. Admission favours complementary purpose documents, current governing guidance and active plans. Reading or editing a file is insufficient. A soft count guides restraint but does not authorise dropping another agent's reference or a Goal source. No separate cleanup job or automatic pruning is introduced.

Issues mean published issues or tickets, not CI runs or informal problems. PRs must concern work the contributing session actively undertakes or tracks. Prompt guidance governs this semantic admission; URL validation does not prove artifact type or importance.

Derived link labels preserve full descriptions and exact URLs. Label proposals target only links in the request snapshot. Under the shared write lock, skip deleted references or those whose kind or description changed. Description edits invalidate old labels. This narrow annotation permission does not authorise changing reference membership.

Shared additions merge against current references without dropping another agent's additions. Explicit removal records an exclusion in the same workspace write. Old or stale helper evidence cannot resurrect the removed reference. Only an explicit foreground re-add clears its exclusion.

Document reference identity is checkout key plus relative path. Link identity is the exact stored URL. Broken document references remain removable without resolving an existing file. Display letters and link numbers are never removal identities.

Keep `work_card` read-only. The accepted foreground-management design scopes mutations to the current workspace and calling agent. Runtime identities and plan-derived Goal are not arbitrary patch fields. Reference removal is direct and metadata-only: no file deletion, remote issue/PR closure or confirmation dialog.

`/work-card remove <substring>` uses a nonempty literal, case-sensitive substring of a resolved stored document path or stored URL. Exactly one match is required. Missing or ambiguous matches change nothing; ambiguity reports qualified candidates, not a picker. A fresh revision check prevents removing a different target after concurrent edits.

Whole-record deletion is separate. Refuse workspace deletion while any agent refers to it, including the caller. Explicit own-agent deletion may precede workspace deletion. Neither operation closes the native workspace or deletes project files.

### Preserve authority through partial transitions

Use validated records, revisions, cooperating-writer locks and atomic replacement, not a new database. Inference runs outside write locks. Reference additions merge; other edits reject stale whole-record revisions rather than introducing general field-by-field merging.

Membership changes and shared mutations share store-level coordination. Bind shared writes to the current agent membership, not merely a workspace filename. Multiple record replacements and native publications are not one atomic transaction.

Joining reserves one destination identity before creating records, then publishes a ready binding only after membership is committed. Interrupted work can recover its matching reservation rather than allocate competing workspaces. Recovery may leave unused records, but must not leave dangling agent references. If reliable binding evidence is lost, recovery is explicit; labels and cwd are not substitutes for identity.

### Keep lifecycle independent of display presence

Each pair has verified agent, terminal, store and ownership identities. No agent may adopt or close another agent's card. Movement may refresh public pane IDs for the same owned terminals; it does not authorise taking over a restored or unrelated process.

Persist a display-open attempt before launch. Reload can recover an exactly verified existing pair, but absence after an attempt does not permit automatic respawn. `q` closes only the display; summaries continue. Explicit open can create a new attempt. Replacing a display after an agent-only move requires positive evidence of a live owned old display, not its absence.

Pi publishes native purpose and own progress during membership polling and after summary updates, independently of the display loop. Normal `q` closure retains verified agent ownership and does not stop publication. An unmarked native pane cannot prove ownership; publishing fails closed until explicit open establishes it. The card process projects only its caption.

Detach pauses updates before fallible cleanup and preserves enough identity for recovery. Closing panes or native workspaces does not delete records. Cancelled session transitions do not retire a still-live display; successful transitions invalidate stale callbacks. Reload retains a valid pair.

### Group context quietly and navigate exact targets

Put the workspace title in the native card-pane border. Show an unlabelled, muted, full-width Goal above the agent's work. Render the own-agent title in bold with normal-weight Now text, followed by a bold `Remaining:` label and normal body. Status sits beside progress when space permits. Omit empty Goal and Remaining sections.

Optional blank lines separate Goal, work and references only when rows are spare. They must not displace content or the close hint. Documents form a responsive row-major grid. Prefer fewer columns with full paths before truncating a single column. Directories are dim, filenames normal and shortcut letters cyan. Issues and PRs occupy their own rows.

Use positional document letters `a` to `z`, excluding `q`. Assignment remains stable across column layouts, but reference changes can renumber it. Only fully painted shortcuts are active. After a fresh read, check the exact opaque target again before opening; truncation must never change it. HTTP links use the terminal client's link gesture; displayed numbers are inert. Show concise artifact labels and a separate issue or PR number derived from recognized URL paths. Keep full descriptions and exact destinations in the record.

With one checkout, show the relative path. With several, prefix the shortest distinguishing checkout-directory suffix. Compute labels across the workspace, including basename collisions. Labels may change as checkouts are added; identities do not.

Open the exact canonical file in a full editor tab on the execution host. Reuse only a verified matching tab, preserving unsaved state and manual names. Unmarked or ambiguous tabs are not adopted. A metadata-projection failure must not disable independently verified navigation. Partial launch uncertainty reports known identities rather than blindly retrying and creating duplicates.

### Protect manual names on every surface

The accepted complete naming model is:

| Surface | Automatic source |
| --- | --- |
| Native workspace | Workspace title |
| Pi session | Own-agent title |
| Agent tab | Own-agent title, only when its pane is the actual first layout pane |
| Editor tab | Document identity, qualified by checkout and path when needed |

Only the actual first layout pane has tab naming rights. Do not skip an ineligible first pane to find a later agent or editor. Session naming remains per-agent even for non-first panes.

Automatic record titles do not grant permission to overwrite manual native labels. Track naming provenance with the surface, including its last applied automatic name, rather than with whichever agent currently comes first. Transfer of naming authority must retain that history. A changed external label becomes a protected manual override. Without provenance, preserve existing nondefault names. Malformed provenance suspends naming.

Use local surface-scoped provenance where native metadata is insufficient, notably tabs. Scope it to the verified binding lifetime so recreated surfaces cannot inherit unrelated naming rights. This is a transient cache, not project state or a global registry. If a rename succeeds but provenance cannot be saved, preserve the now-unproven label rather than overwrite it on retry. A manual tab label must not be forced onto a separately named Pi session.

The card-pane caption is a separate app-owned projection of workspace title. Its implementation does not establish complete workspace, tab or Pi naming. Do not run competing automatic namers; replace the old naming extension only after the new path is verified.

## Alternatives and tradeoffs

| Alternative | Reason for this decision |
| --- | --- |
| One shared card for all agents | Conflates group intent with individual progress and ownership |
| Duplicate all context per session | References diverge and moves can copy unrelated context |
| Group by cwd or repository | Cannot represent a multi-repository native workspace or distinguish worktrees reliably |
| Derive Goal from dialogue | Lets one agent's task or inherited history redefine shared intent |
| Let helpers replace reference arrays | Can drop concurrent additions or undo explicit removals |
| Add a global checkout registry | Extends authority beyond the local workspace without a demonstrated need |
| Use native focus or labels for recovery | Presentation and focus do not prove identity or ownership |
| Keep a legacy importer and runtime | A small local inventory allowed a backed-up one-off conversion instead |

These choices accept repeated shared context in each pane, conservative recovery and sometimes rejected stale edits. Polling introduces detection delay. Separate requests can delay Goal display. Explicit checkout and naming provenance add local state, but prevent ambiguous file opening and unwanted renaming.

## Consequences

Each agent can update its progress without claiming the whole project. Shared Goal is anchored to registered intent, while own-agent progress remains evidence-based. Workspace-aware relevance guides historical reference proposals, while membership checks and persistent exclusions prevent stale writes and undoing explicit removals.

The system deliberately leaves some recovery and management explicit. Records can survive their native bindings. Native operations and filesystem writes cannot be made one transaction; uncertainty must remain visible rather than trigger destructive cleanup or guessed adoption.

## Status and boundaries

The workspace/agent runtime is active. The old single-card runtime, launch mode and compatibility paths are removed. A backed-up one-off local merge preserved references, manual title and valid plan provenance without importing obsolete progress or modifying session history. There is no runtime importer to rerun.

Implemented behavior includes membership reconciliation, separate evidence paths, removal exclusions, per-agent pairs, exact editor navigation and the card-pane caption. The grouped display, responsive document grid and caption updates passed isolated native verification with a real card process and synthetic shell owner.

Earlier isolated native checks covered independent pairs, exact document targets and reuse, manual editor labels, close-without-respawn, explicit reopening, agent-only moves, sequential pair moves and nested sizing. Host checks used injected SDK callbacks and fake helpers, not complete Pi sessions. They do not establish full Pi-session move/reconnect behavior, model quality or an atomic cross-workspace whole-tab move.

Compact fields, narrow link relabeling, purpose-document selection, reference admission guidance, stage diagnostics and Pi-owned native summary publishing have deterministic coverage. Their live helper/sidebar rollout remains unverified. Sidebar configuration and other integrations do not automatically migrate to the new fields. Diagnostics are read-only, in-memory observations, not source evidence or durable project state.

General foreground CRUD management and complete automatic workspace/tab/Pi naming remain unfinished. The separate `herdr-naming.ts` extension is still enabled. First-pane naming, protected manual names and safe replacement of that extension remain rollout requirements, not claims of completed behavior.

Operational rollout chronology, temporary harness paths and test counts are not part of this decision record. Code, tests and version history retain implementation detail. The [README verification status](../README.md#verification-status) distinguishes demonstrated support from remaining verification.
