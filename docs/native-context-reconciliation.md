# ADR: Verify reconciliation across Pi and Herdr

Status: proposed. This is a design decision, not an implemented relaxation of the current runtime guards. It extends the [native Herdr ADR](native-herdr-work-cards.md).

## Context

Streams and work cards should remain consistent and usable during ordinary Pi and Herdr use. The Streams library reconciles native changes into its Session, Stream and card state. Temper verifies these transitions, including changes during model work. Native actions are supported inputs, not protocol violations. Verification must address continued operation when evidence is sufficient, not just safety through refusing changes.

## Decision

Separate the conversation, its current Stream membership and its native location. A work card describes the conversation's own work across Stream moves. A stream card describes the shared effort. Names and pane addresses identify neither.

The conversation follows the workspace. When it moves into a workspace associated with Stream B, the CLI changes its membership to B. Other conversations remain where they are; the Streams are not merged or relocated. If the destination has no associated Stream, create one. The conversation need not wait for the rest of its old working arrangement to move.

Every verified owner move brings its existing owned card below the owner, as at launch, even if the user previously moved the card independently. The Pi child performs this reconciliation; the card only observes. This does not reopen a closed card or replace a missing one automatically.

Retain previous membership relationships as participation history, with one current membership. This records that a conversation participated in a Stream, not what its work card said then. Do not duplicate or freeze work cards. A historical relationship grants no authority to update that Stream.

Changing membership preserves the conversation's repository, checkout and working-directory knowledge. These belong to the conversation, not its current Stream's repository aliases. Do not reinterpret them through the destination or add repositories to that Stream as a side effect.

## Card lifetime and native evidence

Card identity is the conversation identity plus its launch nonce, within the catalog source. Current Stream membership and the owner tuple are observations, not immutable identity or write authority. Only the Pi child/native executor writes card evidence and owner markers. On `q` or owner exit the card just exits, without writes. This replaces the earlier [card-written self-close proposal](bounded-work-and-card-events.md#closing-a-card). A card's launch scope identifies its original lifetime; it never grants authority to write the current or former Stream.

The canonical conversation key is lowercase SHA256 of the compact UTF-8 JSON array `["streams-native-conversation",2,catalog_source,host_id,harness,native_session_id]`, in that order. Use this key for both native pair identity and version-2 evidence lookup. It excludes the catalog Session ID, Stream, socket and owner location, so a catalog ID migration does not rekey the lifetime. Evidence retains the endpoint and resource observations separately; a different endpoint does not establish continuity.

The child subscribes to `pane.closed` and `pane.exited` for its own card. An event says to re-read: after one fresh positive-absence check, the child records closure and clears the stale owner marker. If no child is running, its next start observes and records closure. Missing or uncertain observations retain uncertainty.

At startup and after every event batch, the card reads its own evidence by conversation key and launch nonce. If the record is absent or its nonce differs, the card exits without writing. Legitimate launch intent exists before dispatch. An orphan therefore cannot outlive the removal or replacement of its evidence, including after offline reset.

Use a versioned, conversation-keyed evidence record carrying that lifetime's nonce, phase and resources. Normal operations use direct keyed reads and require the completed new format. They never read legacy keys, replay history or scan for evidence. If migration is required, fail clearly and name the migration command.

### Offline reset

The alpha cutover resets native evidence instead of attributing old records. After backup, the coordinator stops Streams Pi children and their serve processes, then runs `streams native migrate-evidence` under the native lock. Card panes will restart; preserving their old nonce or lifetime through deploy is not required. Catalog identity migration can run in either order.

For this first deploy, the coordinator also stops every pre-rule card process by command line, including `~/code/streams/target/debug/streams native pane`. They inspect a snapshot for leftover card panes and close them by hand. Replacing the executable does not upgrade running readers. Late launch requests must use the deployed binary at their captured `STREAMS_RUST_BIN` path. These are deploy steps, not runtime attribution.

The reset takes one Herdr snapshot and runs one ordered plan, without retries:

1. Close known card resources named by version-1 evidence, matching their nonce, resource tuple and card markers against the snapshot. The record shape alone cannot distinguish cards from editors. Leave editor and unclassified panes open.
2. Clear Streams pair markers on every pane, guarding each target against its snapshot identity.
3. Delete all version-1 card and editor evidence, then install the completed-version-2 marker.

A late card from an in-flight version-1 launch starts the deployed binary, finds no matching version-2 evidence and exits. Unmarked or otherwise unclassified cards follow the same rule. The reset needs neither plugin provenance in the snapshot nor a frontend dispatch barrier. If an effect reply fails or mismatches, stop before deleting evidence or installing completion.

Editor evidence is discarded even when a launch was uncertain. Editors may contain unsaved work, so the reset leaves them open. A later request may create a duplicate editor pane; this is an accepted alpha cost. This exception does not cover workspace-open attempts.

After reset, automatic open or `/streams pane` can launch fresh cards. Normal operations never perform this reset, read version-1 evidence or support a mixed layout. There is no attribution, quarantine, migration manifest or editor evidence upgrade.

## Cross-host participation

Each Stream belongs to exactly one host. Stream authority is that host; cross-host moves and handover are unsupported. A native move into a workspace associated with another host's Stream must fail before creating records, binding workspaces or dispatching native effects. Do not silently duplicate the Stream, change its host or redirect participation. Reading synchronized records remains allowed.

Session identity is `(host, agent, native Session ID)`, where agent means the coding harness, such as Pi. Resuming a copied conversation on another host creates a separate Session and work card, even if its native ID is unchanged. It does not join or gain authority over the original host's Stream.

## CLI and Pi boundary

The library owns reconciliation and result admission, exposed through the CLI. Pi supplies its session identity and observed Herdr state when requesting cards. The library verifies the live context, reconciles membership and returns the cards with a context reference. Ordinary catalog reads remain read-only; requesting reconciled context deliberately permits membership changes.

Pi runs the model and returns its proposed updates with that original context reference and its latest observed Herdr state. The CLI reconciles again and assesses conversation-local and Stream-shared updates separately. A workspace move need not invalidate useful work-card updates, but results computed for the previous Stream must not be retargeted to the destination.

Pi carries request context without interpreting membership or orchestrating record changes. The CLI owns the checks and returns which updates were accepted. This keeps movement policy and stale-result handling consistent across callers.

## Other native changes

| Change | Intended response |
| --- | --- |
| Rename a native surface | Preserve user-chosen names. Resume automatic naming only after explicit reclaim of that surface. |
| Reorder panes or change pane IDs | Follow verified live conversation and terminal identity, not inherited addresses. Membership changes only if the workspace changes. |
| Move a card independently | Retain its conversation association. The next owner move brings it below the owner, regardless of the card's chosen placement. Do not close it or launch a replacement. |
| Move an editor independently | Retain its conversation association. Do not automatically move it back, close it or launch a replacement. |
| Close a display | Reporting continues. No background respawn; explicit reopen requires positive absence and duplicate checks. |
| Lose observations or restore a workspace | Pause affected effects until fresh proof is available. Missing observations do not prove closure; recycled handles do not prove continuity. |
| Fork or create a conversation | Create a fresh Session and work card and join the verified workspace's Stream automatically. Do not copy the parent's identity or outstanding effect authority. |

Tree navigation preserves conversation identity but invalidates work captured against the old branch. A completed conversation replacement retires the old runtime's work; cancellation leaves it usable.

Reconcile on relevant native events and Streams interactions, following the [event-driven process contract](bounded-work-and-card-events.md). Normal transitions preserve pending native-effect evidence. Membership changes or resource reappearance cannot erase launch uncertainty or authorize duplicate effects. The explicit offline reset above is the deploy exception.

## Work status and native presence

Keep work status separate from observed native presence: running, closed or unknown. The library determines presence from verified observations; the summarizer cannot set it. Closing a card is not closing its conversation, and unavailable evidence is not proof of closure.

Give the model native context alongside conversation evidence and the cards. It may use that information to propose work status. Reopening does not mechanically unpark work, and closure does not mechanically park it. The pane displays both dimensions; the library validates proposals against their captured context.

## Scope and open questions

This changes the earlier fixed-membership proposal: verified workspace moves intentionally change membership, but cannot change which conversation a result belongs to. Reconciliation still requires current identity and ownership proof; user confirmation cannot substitute for it. The [caller-attestation proposal](native-caller-attestation.md) separates request-origin proof from native location.

Physical-pairing requirements for separated resource navigation and explicit recovery from uncertain native launches remain open. They must not silently become blanket restrictions on conversation reporting.

The [resource model](../specs/native_reconciliation.ioa.toml) covers native-effect safety under the earlier fixed-membership assumptions; it does not yet model the card-follow policy. The [membership/admission model](../specs/native_membership.ioa.toml) and [tests](../tests/native_membership.rs) cover conversation-only moves, per-card context checks, membership revisits and observation recovery. Both abstract identity and commit evidence; neither proves concrete caller attestation, ordered-history persistence or crash-safe report receipts.

A retried report must retain its original intent across a move and a lost acknowledgement, rather than becoming a newly retargeted edit. The production admission protocol still needs to establish that guarantee before activation.
