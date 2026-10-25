# ADR: Verify reconciliation across Pi and Herdr

Status: membership and report-admission proposal, not fully implemented. [Observe and act](native-observe-and-act.md) supersedes the former caller-attestation, card-evidence and native launch-fence requirements. This record retains membership, cross-host and captured-result semantics.

## Context

Streams and work cards should remain consistent and usable during ordinary Pi and Herdr use. The Streams library reconciles native changes into its Session, Stream and card state. Native actions are supported inputs, not protocol violations. Tests must show continued operation when observations are usable, not just safety through refusing changes.

## Decision

Separate the conversation, its current Stream membership and its native location. A work card describes the conversation's own work across Stream moves. A stream card describes the shared effort. Names and pane addresses identify neither.

The conversation follows the workspace. When it moves into a workspace associated with Stream B, the CLI changes its membership to B. Other conversations remain where they are; the Streams are not merged or relocated. If the destination has no associated Stream, create one. The conversation need not wait for the rest of its old working arrangement to move.

Each pass compares the existing card's tab with Pi's current tab. If they differ, the Pi child moves the card below Pi with `split: "down"`. A user-moved card comes back on the next pass; no previous location or owner-move baseline is needed. The card only observes. Following never reopens a closed card or replaces a missing one.

A failed or uncertain move warns and ends the pass; a later event or request makes a new comparison. A restarted child follows the same rule. There is no stored follow intent, relaunch or layout reconstruction. The [observe-and-act contract](native-observe-and-act.md) owns targeting's accepted restart risk and duplicate handling.

Retain previous membership relationships as participation history, with one current membership. This records that a conversation participated in a Stream, not what its work card said then. Do not duplicate or freeze work cards. A historical relationship grants no authority to update that Stream.

Changing membership preserves the conversation's repository, checkout and working-directory knowledge. These belong to the conversation, not its current Stream's repository aliases. Do not reinterpret them through the destination or add repositories to that Stream as a side effect.

## Card lifetime

Card identity follows the catalog-scoped Session, not its membership or pane address. The [observe-and-act decision](native-observe-and-act.md) replaces launch nonces, reciprocal pairing, native evidence records and predecessor retirement with ordinary markers and a card process lock. The child acts; the card observes and exits on `q` or observed owner loss. No closure record or marker cleanup is needed to prevent background reopening.

### Offline reset

The alpha catalog cutover still follows the [coordinator's manual checklist](native-catalog-reset.md). Stop old children and viewers before archiving their native state; leave editors open and clean up stray panes manually. There is no runtime migration, evidence recovery or legacy reader. Do not discard evidence while the old implementation is active.

## Cross-host participation

Each Stream belongs to exactly one host. Stream authority is that host; cross-host moves and handover are unsupported. A native move into a workspace associated with another host's Stream must fail before creating records, binding workspaces or dispatching native effects. Do not silently duplicate the Stream, change its host or redirect participation. Reading synchronized records remains allowed.

Session identity is `(host, agent, native Session ID)`, where agent means the coding harness, such as Pi. Resuming a copied conversation on another host creates a separate Session and work card, even if its native ID is unchanged. It does not join or gain authority over the original host's Stream.

## CLI and Pi boundary

The library owns reconciliation and result admission, exposed through the CLI. The Pi child is bound to its Session and collects native observations itself. It reconciles membership and returns cards with their captured context. Ordinary catalog reads remain read-only; requesting reconciled context deliberately permits membership changes.

Pi runs the model and returns its proposed updates with their original captured context. The library assesses conversation-local and Stream-shared updates separately against current records. A workspace move need not invalidate useful work-card updates, but results computed for the previous Stream must not be retargeted to the destination. Pi does not send caller proof or a native snapshot back to the child.

Pi carries request context without interpreting membership or orchestrating record changes. The CLI owns the checks and returns which updates were accepted. This keeps movement policy and stale-result handling consistent across callers.

## Other native changes

| Change | Intended response |
| --- | --- |
| Rename a native surface | Apply the saved Streams title when that surface's synchronization is enabled; see [name ownership](name-ownership.md). |
| Reorder panes or change pane IDs | Follow verified live conversation and terminal identity, not inherited addresses. Membership changes only if the workspace changes. |
| Move a card independently | Retain its conversation association. On the next pass, bring it into Pi's tab if it is elsewhere. Do not close it or launch a replacement. |
| Move an editor independently | Retain its conversation association. Do not automatically move it back, close it or launch a replacement. |
| Close a display | Reporting continues. No background respawn; explicit reopen uses observed reuse and self-exiting duplicates. |
| Lose observations or restore a workspace | Stop affected effects until usable observations are available. Missing observations do not prove closure. Abort a pass when its subscription ends; the within-pass restart race remains an accepted risk. |
| Fork or create a conversation | Create a fresh Session and work card and join the verified workspace's Stream automatically. Do not copy the parent's identity or outstanding effect authority. |

Tree navigation preserves conversation identity but invalidates work captured against the old branch. A completed conversation replacement retires the old runtime's work; cancellation leaves it usable.

Reconcile on relevant native events and Streams interactions, following the [event-driven process contract](bounded-work-and-card-events.md). Failed effects warn and stop, without a persistent launch fence or automatic replay. Catalog retry receipts remain separate from native launch behaviour.

## Work status and native presence

Keep work status separate from observed native presence: running, closed or unknown. The library determines presence from verified observations; the summarizer cannot set it. Closing a card is not closing its conversation, and unavailable evidence is not proof of closure.

Give the model native context alongside conversation evidence and the cards. It may use that information to propose work status. Reopening does not mechanically unpark work, and closure does not mechanically park it. The pane displays both dimensions; the library validates proposals against their captured context.

## Scope and verification

Workspace moves intentionally change membership, but cannot change which conversation a captured result belongs to. The [record-local operations ADR](record-local-operations.md#membership-move-contract) owns the single-Session save and membership-period checks.

The old native resource and membership models are retired with their caller-proof and pending-attempt state. Scripted tests exercise the [native controller](../tests/serve/subscriptions.rs) and [card effects](../src/runtime/native_cards/tests.rs) against actual fake resources. The separate [membership tests](../src/operations/membership/tests.rs), [record-update tests](../tests/record_updates.rs) and Pi branch-admission tests keep moves, membership revisits, captured targets and accepted-save receipts covered without a native proof protocol.

A retried report must retain its original target across a move and a lost acknowledgement. Removing native launch fences does not permit retargeting an accepted or captured catalog operation.
