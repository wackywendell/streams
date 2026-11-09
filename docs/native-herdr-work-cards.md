# ADR: Native Herdr work cards as Streams projections

Status: implemented and activated. [Observe and act](native-observe-and-act.md)
owns native lifetime and effects; [record-local operations](record-local-operations.md)
owns storage and active-Session reopening.

## Keep the card, not a second catalog

Keep a native card beside each agent, alongside Pi's inline card. The imported
history through `e7d74ce22f95893fa8eed570ef2e6a14d61f74bd` preserves the original
implementation; rebuilding only its appearance would discard tested behavior.

Streams is the sole writable work catalog. The card combines shared Stream purpose
and references with its exact Session's contribution. It neither keeps a separate
account nor runs another summarizer. One Session finishing does not finish the
effort. A workspace association names an effort, not an arbitrary Session in it.
Titles, cwd and GUI focus never establish conversation identity.

Orientation is not an activity log. Purpose, evidenced progress and remaining work
belong in short records; substantive plans and decisions stay in project documents.
Existing summaries are editable drafts, not independent evidence. Helpers receive
runtime-selected evidence and narrow proposal permissions, not filesystem,
executable-selection or native-effect authority. Historical tool outcomes remain
historical; prompt instructions and validation cannot guarantee semantic truth.

## Preserve readable density and exact navigation

Put `Stream title: Session title` in the card-pane caption. Show a muted shared Goal
above the Session recap, current Status and Remaining blocks. Omit empty text
sections and keep lifecycle status distinct from native presence. Spare rows create
quiet section breaks rather than pushing content off-screen for labels or the close hint.

References appear in Document, PR, Issue and Link groups, preserving saved order
within each group. A header shows saved name, role and state; wrapped purpose
follows it. Fulfilled entries stay visible. Dropped entries have no normal-card
row, action or overflow count.

Document letters and URL row numbers identify painted entries. Each block reserves
a header and purpose row before spare purpose lines are allocated. The card keeps
the longest prefix of whole blocks that fits and counts omitted entries once.
A complete painted document key remains active when its descriptive text is
clipped. A resize or refresh invalidates old actions before new input can use them.

HTTP links use the terminal client's link gesture. Activation compares the full
saved Reference, including revision and relationship fields, with current records
before parsing its exact URL for the browser. Display labels and parsed URL
spelling supply no identity. Annotation integrations remain out of scope.

Resizing paints the current snapshot without waiting for I/O. That snapshot is
display data, not authority to mutate or open something. Native and record events
drive reads; there is no polling or retry loop.

## Resolve documents from explicit local mappings

Prefer the viewing Session's working file, including uncommitted edits. Two tabs
in the same Herdr workspace can have different jj workspaces or repositories.
A workspace-wide directory map would conflate them.

Streams owns checkout selection. A stable checkout ID names a canonical root on
this host. Explicit repository URL bindings distinguish intentional mappings from
matching remotes or directory names. A move updates the ID's root rather than
rewriting each Session. The viewing Session's selection wins for that repository;
explicit defaults can supply other repositories. Missing or unbound selections
remain visible errors, not permission to choose another checkout.

A document identifies its exact repository URL and portable relative path.
Optional ref, commit and source Session hints support explicit recovery.
The card opens current working files, including local edits. Missing files or
unavailable checkout mappings leave the portable relationship intact.

Display hints include established URLs from the viewing Session's registered
checkout before any document is saved. A missing selection or valid plain directory
permits explicit repository defaults; broken explicit selections remain unavailable.
Diagnostic captures freeze the same hint arrays for deterministic replay.

Before opening, revalidate the painted reference, viewing scope, selected root and
canonical regular file. Reject traversal and escaping symlinks. Reuse an observed
Streams editor only for the exact resolved identity; matching a relative path or
an unmarked tab is insufficient. Do not automatically move or close editors.
Trusted host configuration selects the editor command, with the filename passed
as data rather than executable text.

## Project saved names and bounded metadata

Workspace names use the Stream title; card captions combine Stream and Session
titles. Pi and eligible agent tabs use the Session title; editor tabs use document
identity. Workspace token `strm_goal_short` and Pi-pane tokens `strm_recap_short`,
`strm_state_short` and `strm_remaining_short` copy their matching saved short forms. Independent
synchronization switches and the actual-first-pane rule select one tab-name writer.
[Name ownership](name-ownership.md) owns manual/automatic policy and Pi name import;
there is no native-name provenance ledger or separate manual override to infer.

Herdr trims metadata, removes controls and caps text at 80 Unicode scalar values.
Streams imposes a stricter limit of fewer than 25 characters. Publish an existing
compact summary only when it meets that limit and Herdr retains it unchanged;
otherwise clear the hint. Keep full text in the catalog and card. Do not truncate
or run another inference just to fill a native field.

## Keep failures and environments separate

Closing the card is display-only. Reporting continues. Native actions compare
current observations and execute once; lost replies warn, with best-effort reuse
on a later request instead of recorded launch phases. Catalog saves and native
effects are not one transaction, and a saved title does not prove a rename worked.

GUI-created terminals inherit the frontend server's environment, not the invoking
CLI's. Launches explicitly carry bounded catalog, host-state, registry and harness
selection paths, including defaults. They do not forward the full environment or
credentials. Otherwise a card could read another catalog despite the same Stream ID.

Frozen renderer fixtures, outside-change fakes and isolated PTYs test distinct
boundaries. Neither a similar screenshot nor a fake endpoint establishes live
platform support. Keep one active writer and matching loaded CLI/Pi versions;
updating installed processes remains separate from repository integration.
