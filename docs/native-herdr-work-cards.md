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

Documents use a responsive row-major grid. Prefer full paths before truncating a
single column. Shortcut assignment stays stable across column layouts, but can
change when references change. Only fully painted shortcuts are active; a resize
or refresh invalidates old actions before new input can use them. Truncation and
display labels must never change the opaque target.

HTTP links use the terminal client's link gesture. Displayed issue/PR numbers are
not removal identities. Keep exact URLs and full descriptions in records. This
editor-first navigation avoids an agent-feedback channel that could accidentally
address a replacement conversation; annotation integrations remain out of scope.

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

A document identifies a Stream repository alias and relative path. Its optional
commit and source Session are provenance, not a pin to old contents. The current
card resolves only working files: it has no active source/revision chooser.
A missing local file never deletes the portable reference.

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
