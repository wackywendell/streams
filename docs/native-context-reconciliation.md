# ADR: Reconcile native changes without changing conversation membership

Status: proposed. This is a design decision, not an implemented relaxation of the current runtime guards. It extends the [native Herdr ADR](native-herdr-work-cards.md).

## Context

Herdr and Pi can change independently of Streams. Users can rename or move panes, switch conversations, close displays and restore workspaces. These are valid inputs, not violations of a Streams protocol.

Streams should keep working when it can prove that the same conversation continues in a new location. It must not mistake a different conversation, conflicting workspace ownership or a recycled handle for continuity.

## Decision

Separate conversation identity from native location. The enrolled Session and native conversation ID, scoped to a catalog and host, identify the conversation. Workspace, tab and pane addresses describe where it is currently displayed. Names describe neither identity nor membership.

Adopt location changes automatically when fresh observations prove the same conversation and terminal, and ownership remains unambiguous. Otherwise pause the affected native effects. A temporary loss of evidence must not permanently disable the integration: fresh proof can restore operation without a reload.

Herdr may give a moved pane a new ID while its inherited caller context still resolves the old handle. Streams must use the actual live terminal and conversation evidence, not unconditional equality with launch-time addresses. After restart or restore, matching saved handles alone cannot establish continuity.

## Intended responses

| Change | Streams response |
| --- | --- |
| Rename a workspace, tab, pane or Pi session | Keep user-chosen names, including after moves. Resume automatic naming only when the user explicitly reclaims that surface. |
| Move or reorder the owning conversation within its associated workspace | Adopt its proven live location without changing membership. |
| Move across workspaces | Adopt only when the complete bound arrangement has relocated, the source can be retired and destination ownership is compatible. A split arrangement or conflicting destination pauses adoption; it never authorizes taking another Stream's workspace. |
| Lose observations, disconnect, restart or restore | Pause affected effects and seek fresh evidence. Missing information is not proof of closure; saved names, handles and metadata are not proof of live identity. |
| Fork or create a Pi conversation | Treat it as a new identity requiring its own enrollment choice. Do not inherit membership or effect authority from the parent. |
| Move a card or editor independently | Retain its association with the original conversation. Do not automatically move it back, close it or launch a replacement. Unrelated eligible operations may continue; actions requiring the original pairing wait for reunion and verification. |
| Close or reopen a display | Keep membership and reporting independent of display state. Do not respawn closed resources in the background. Explicit reopen requires positive absence and duplicate checks, with no unresolved launch attempt. Reappearance must be verified before reuse. |

Pi tree navigation stays within the same conversation, but invalidates work captured against the old branch. Switching to another enrolled conversation may use that conversation's existing membership. On a completed switch or fork, stop and drain the old runtime's work before the replacement gains authority. A cancelled switch must leave the old conversation usable.

## Safety boundaries

Reconciliation changes location, not authority. Adopt a move only while current identity and both workspaces' ownership still match the evidence used to approve it. User confirmation cannot substitute for those proofs.

Preserve pending launch and ownership evidence across every change. A new address, a closed display or a fresh owner observation must not erase uncertainty or authorize a duplicate launch. An interrupted reopen needs its original recovery proof, not merely a later resource that looks right.

Pause only the operations whose safety is unresolved. A native-workspace conflict need not stop catalog reporting when the exact conversation's membership and reporting authority remain independently provable.

## Consequences

This rejects both permanent failure on every topology change and automatic attachment to whichever conversation occupies a pane. It accepts temporary pauses and explicit recovery where identity or ownership cannot be proved, while allowing ordinary moves to remain ordinary.

The [executable reconciliation model](../specs/native_reconciliation.ioa.toml) and its [tests](../tests/native_reconciliation.rs) explore this safety policy. They do not implement reconciliation or establish concrete identity, crash durability or native API safety. Those remain implementation and isolated acceptance requirements before activation.
