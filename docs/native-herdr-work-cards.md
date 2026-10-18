# ADR: Native Herdr work cards as Streams projections

Status: accepted direction; catalog adaptation and native activation are not complete. The imported implementation retains its original history. This decision replaces its separate catalog and membership model, not its presentation design.

## Context and decision

Keep the native Herdr Work Cards UI alongside Pi's Alt+P card. The user values the work already invested in its layout, navigation, independent agent views and naming protections. Import the existing package and history rather than reconstructing the display from its appearance.

Streams is the only writable work catalog. The native card reads a stream's shared purpose and references together with its owning session's contribution. It does not create another account of the effort or run a second summarizer. The [catalog ADR](durable-work-catalog.md) owns record meaning; the [Pi ADR](pi-work-card.md) owns reporting, editing and initial conversation enrollment.

The imported [orientation](../integrations/herdr-work-cards/docs/design.md), [workspace and display](../integrations/herdr-work-cards/docs/workspace-context.md) and [naming](../integrations/herdr-work-cards/docs/automatic-naming.md) decisions retain the reasoning behind the UI. Their legacy storage APIs, commands and membership transitions are not interfaces to the Streams catalog.

## Preserve the card, replace its authority

Retain a card beside each owning agent, not a new workspace-overview tab. Its shared section comes from the Stream; its title, status, current state and remaining scope come from the exact Session. Finishing that contribution must not imply finishing the effort. The field mapping is recorded in the catalog ADR.

Preserve the existing renderer's density, Unicode handling, clipping, quiet section grouping, responsive document grid and visible-shortcut rules. Keep terminal-link gestures, exact-target document navigation, editor reuse and independent manual names. The port must carry their regression coverage forward rather than treating a similar screenshot as equivalence.

The stream ID, catalog source, host and native conversation identity establish the view's scope. A native workspace association identifies an effort, not an arbitrary session within it. Resolve the owning conversation exactly; do not select a contribution by title, cwd, GUI focus or the number of sessions found. A missing or contradictory association is unavailable state, not permission to enroll or guess.

Catalog reads and validated operations remain behind the Streams application boundary. TypeScript does not write catalog YAML, mirror records into the old Work Cards store or infer ownership from matching record text. Cached display state never grants mutation or native-effect authority. Reading or opening a card does not start reporting, attach a conversation or launch a workspace.

## Keep native effects host-local

Pane ownership, display attempts, terminal identities, naming provenance and editor bindings stay on the execution host. They are not portable catalog fields. Reuse the existing safety policies, but adapt their proofs to Streams identities: an old Work Cards store binding is not proof of a current Streams association.

Closing with `q` remains display-only. It must not stop catalog reporting, delete records, detach the conversation or cause background respawn. Persist uncertain launch attempts before effects; a timeout or missing display after an attempt must not authorize a duplicate launch. Cleanup may affect only positively verified owned resources.

Native workspace names and card captions project `stream.title`; Pi and eligible agent-tab names project `session.title`; editor names come from exact document identity. Compact purpose and progress use `goal_short` and `state_short`, not another inference or text truncation. Preserve the imported naming policy's independent switches, manual protection and actual-first-pane rule. Manual catalog text and manual native labels are separate decisions.

Native effects are not a transaction with catalog persistence. Verify current scope and acknowledgements, report partial effects and preserve recovery evidence. A successfully saved title is not proof that a native name changed.

## Document identity is a port boundary

The old UI opens `(checkout key, relative path)` inside a registered local root. Streams documents name `(repository alias, ref, relative path)`, while absolute directories remain host-local. A native workspace or the viewing agent's cwd cannot identify every referenced checkout, especially with several repositories or worktrees.

Before enabling document shortcuts, define the host-local resolution from that catalog identity to an exact checkout. Revalidate the painted identity and resolved root before opening or reusing an editor. Missing mappings, missing files and ambiguous checkouts must remain visible; none authorizes guessing another file or deleting a portable reference. The old package's automatic missing-document cleanup cannot be carried over: absence on one host is not evidence that a shared reference should disappear.

The precise checkout-binding interface is still open. Until it is settled and tested, document navigation is not a completed part of the native port. A read-only display without it must be described as partial, not presented as preservation of the full UI.

## Alternatives and consequences

Using Alt+P alone would avoid a native port, but the user chose to retain the side pane. Rebuilding only its visual appearance would discard tested behavior and design context. Keeping the old writer and synchronizing two catalogs would introduce competing memberships, summaries and manual overrides. Importing its history and adapting the authority boundary preserves the implementation without that second catalog.

This costs more than changing a record reader. Legacy movement automatically reassigned an agent to its destination workspace; Streams deliberately has no implicit conversation move or fork. The native adapter must suspend conflicting effects rather than revive that behavior. Document resolution and native ownership need explicit adapters before activation.

The import and the functional port are separate changes. Retain original commits and documents, then verify catalog mapping, navigation, close/reopen behavior, naming and partial failures independently. Legacy record and helper tests remain historical coverage; they do not prove the new adapter correct. Run isolated native acceptance before activation, with the old writer and namers quiescent. Do not obtain quiescence by restarting busy agents without direction.
