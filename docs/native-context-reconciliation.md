# ADR: Reconcile conversation context in the CLI

Status: proposed. This is a design decision, not an implemented relaxation of the current runtime guards. It extends the [native Herdr ADR](native-herdr-work-cards.md).

## Context

Herdr and Pi change independently of Streams. Moves, renames, conversation switches and display closures are valid inputs. Streams should reconcile them when identity is provable, rather than permanently disable the integration after a topology change.

## Decision

Separate the conversation, its current Stream membership and its native location. A work card describes the conversation's own work across Stream moves. A stream card describes the shared effort. Names and pane addresses identify neither.

The conversation follows the workspace. When it moves into a workspace associated with Stream B, the CLI changes its membership to B. Other conversations remain where they are; the Streams are not merged or relocated. If the destination has no associated Stream, create one. The conversation need not wait for the rest of its old working arrangement to move.

Retain previous membership relationships as participation history, with one current membership. This records that a conversation participated in a Stream, not what its work card said then. Do not duplicate or freeze work cards. A historical relationship grants no authority to update that Stream.

## CLI and Pi boundary

The CLI owns reconciliation and result admission. Pi supplies its session identity and observed Herdr state when requesting cards. The CLI verifies the live context, reconciles membership and returns the cards with a context reference. Ordinary catalog reads remain read-only; requesting reconciled context deliberately permits membership changes.

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
| Fork or create a conversation | Treat it as a new identity requiring its own enrollment choice, not inherited membership or authority. |

Tree navigation preserves conversation identity but invalidates work captured against the old branch. A completed conversation replacement retires the old runtime's work; cancellation leaves it usable.

Every transition preserves pending native-effect evidence. Changing membership or finding a resource later must not erase launch uncertainty or authorize duplicate effects.

## Scope and open questions

This changes the earlier fixed-membership proposal: verified workspace moves intentionally change membership, but cannot change which conversation a result belongs to. Reconciliation still requires current identity and ownership proof; user confirmation cannot substitute for it.

Physical-pairing requirements for separated resource navigation and explicit recovery from uncertain native launches remain open. They must not silently become blanket restrictions on conversation reporting.

The existing [native reconciliation model](../specs/native_reconciliation.ioa.toml) and [tests](../tests/native_reconciliation.rs) cover resource-effect safety under the earlier fixed-membership assumptions. They do not yet establish the membership history and per-card result admission described here. Concrete identity, crash durability and native API safety also require separate verification before activation.
