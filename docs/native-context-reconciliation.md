# ADR: Verify reconciliation across Pi and Herdr

Status: proposed. This is a design decision, not an implemented relaxation of the current runtime guards. It extends the [native Herdr ADR](native-herdr-work-cards.md).

## Context

Streams and work cards should remain consistent and usable during ordinary Pi and Herdr use. The Streams library reconciles native changes into its Session, Stream and card state. Temper verifies these transitions, including changes during model work. Native actions are supported inputs, not protocol violations. Verification must address continued operation when evidence is sufficient, not just safety through refusing changes.

## Decision

Separate the conversation, its current Stream membership and its native location. A work card describes the conversation's own work across Stream moves. A stream card describes the shared effort. Names and pane addresses identify neither.

The conversation follows the workspace. When it moves into a workspace associated with Stream B, the CLI changes its membership to B. Other conversations remain where they are; the Streams are not merged or relocated. If the destination has no associated Stream, create one. The conversation need not wait for the rest of its old working arrangement to move.

Retain previous membership relationships as participation history, with one current membership. This records that a conversation participated in a Stream, not what its work card said then. Do not duplicate or freeze work cards. A historical relationship grants no authority to update that Stream.

Changing membership preserves the conversation's repository, checkout and working-directory knowledge. These belong to the conversation, not its current Stream's repository aliases. Do not reinterpret them through the destination or add repositories to that Stream as a side effect.

## CLI and Pi boundary

The library owns reconciliation and result admission, exposed through the CLI. Pi supplies its session identity and observed Herdr state when requesting cards. The library verifies the live context, reconciles membership and returns the cards with a context reference. Ordinary catalog reads remain read-only; requesting reconciled context deliberately permits membership changes.

Pi runs the model and returns its proposed updates with that original context reference and its latest observed Herdr state. The CLI reconciles again and assesses conversation-local and Stream-shared updates separately. A workspace move need not invalidate useful work-card updates, but results computed for the previous Stream must not be retargeted to the destination.

Pi carries request context without interpreting membership or orchestrating record changes. The CLI owns the checks and returns which updates were accepted. This keeps movement policy and stale-result handling consistent across callers.

## Other native changes

| Change | Intended response |
| --- | --- |
| Rename a native surface | Preserve user-chosen names. Resume automatic naming only after explicit reclaim of that surface. |
| Reorder panes or change pane IDs | Follow verified live conversation and terminal identity, not inherited addresses. Membership changes only if the workspace changes. |
| Move a card or editor independently | Retain its conversation association. Do not automatically move it back, close it or launch a replacement. |
| Close a display | Reporting continues. No background respawn; explicit reopen requires positive absence and duplicate checks. |
| Lose observations or restore a workspace | Pause affected effects until fresh proof is available. Missing observations do not prove closure; recycled handles do not prove continuity. |
| Fork or create a conversation | Create a fresh Session and work card and join the verified workspace's Stream automatically. Do not copy the parent's identity or outstanding effect authority. |

Tree navigation preserves conversation identity but invalidates work captured against the old branch. A completed conversation replacement retires the old runtime's work; cancellation leaves it usable.

Reconcile closures and other changes on the next Streams interaction; no continuing observer is required. Every transition preserves pending native-effect evidence. Membership changes or resource reappearance cannot erase launch uncertainty or authorize duplicate effects.

## Work status and native presence

Keep work status separate from observed native presence: running, closed or unknown. The library determines presence from verified observations; the summarizer cannot set it. Closing a card is not closing its conversation, and unavailable evidence is not proof of closure.

Give the model native context alongside conversation evidence and the cards. It may use that information to propose work status. Reopening does not mechanically unpark work, and closure does not mechanically park it. The pane displays both dimensions; the library validates proposals against their captured context.

## Scope and open questions

This changes the earlier fixed-membership proposal: verified workspace moves intentionally change membership, but cannot change which conversation a result belongs to. Reconciliation still requires current identity and ownership proof; user confirmation cannot substitute for it.

Physical-pairing requirements for separated resource navigation and explicit recovery from uncertain native launches remain open. They must not silently become blanket restrictions on conversation reporting.

The [resource model](../specs/native_reconciliation.ioa.toml) covers native-effect safety under the earlier fixed-membership assumptions. The [membership/admission model](../specs/native_membership.ioa.toml) and [tests](../tests/native_membership.rs) cover conversation-only moves, per-card context checks, membership revisits and observation recovery. Both abstract identity and commit evidence; neither proves concrete caller attestation, ordered-history persistence or crash-safe report receipts.

A retried report must retain its original intent across a move and a lost acknowledgement, rather than becoming a newly retargeted edit. The production admission protocol still needs to establish that guarantee before activation.
