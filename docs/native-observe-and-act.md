# ADR: Observe native state and test behaviour under outside changes

Status: implemented in Rust and activated. This replaces native caller attestation, pair proofs and durable launch fences, not catalog save or report-admission rules.

The [card preference decision](card-preferences-and-control.md) extends this behaviour with outside-attach opening and explicit open/close preferences. It replaces the automatic-opening and closure-policy details below, not observation or native targeting.

## Make ordinary native actions ordinary

Opening a card, moving Pi and reopening work previously passed through caller attestation, reciprocal ownership markers and recorded launch phases. Every feature had to preserve that protocol. The user wants simple, straightforward and predictable behaviour instead: outside changes are normal inputs, and rare duplicate launches are preferable to a recovery system.

Streams will observe current native state, compare it with the requested behaviour and act once. Correctness will be demonstrated by tests that change the outside world during execution, not by running a proof protocol before each effect. Small runtime checks remain where they directly prevent a wrong action. Tests do not make an unsafe frontend operation safe.

## Behaviour

| Operation or outside change | Required response |
| --- | --- |
| Open a card | Reuse an observed card for this Session, or open one beside Pi. Brief duplicate panes are acceptable; duplicate cards exit themselves. |
| Close a card | `q` exits the card. Explicit external close targets only that Session's card. Neither closes Pi nor changes catalog records. |
| Pi starts or resumes | Open a card if automatic opening is enabled. A child restart is not a Pi start. |
| Card closes during a running Pi session | Leave it closed until an explicit open. Reporting and membership reconciliation continue. |
| Pi or its card moves | On the next pass, bring the existing card into Pi's tab, below Pi, if it is elsewhere. Do not create a missing card. |
| Pi exits or changes conversation | The old card exits when it observes that change. It does not close another pane. |
| Open a document | Reuse an observed Streams editor for the exact resolved document; otherwise open one. Rare duplicates are acceptable. Never automatically close or move editors. |
| Reopen a Stream | Open active same-host Sessions in separate tabs. Skip Sessions already observed running and report their locations. Restore no saved layout. |
| Rename a native surface | Keep the [name ownership policy](name-ownership.md): enabled synchronization projects saved titles; disabled synchronization leaves the native name alone. |

Reuse must be cheap. Match reported identities in the operation's snapshot; do not search transcripts or build a discovery protocol to avoid a duplicate. A missing saved transcript locator still prevents resuming that Session: allowing duplicate launches does not permit inventing a conversation. Missing launch inputs produce a Session-specific diagnostic.

Editors, reopened Pi sessions and workspaces have best-effort duplicate avoidance, not an exactly-once guarantee. A lost launch reply reports that the resource may have opened and ends the affected pass. There is no automatic retry. A later explicit request observes again. Two requests can race, or a launched Pi can register late; the user may need to close a duplicate. Streams must not automatically terminate a Pi or editor to repair this.

## Small runtime, no launch history

Keep one native controller per Pi session and the typed [Pi child interface](pi-child-protocol.md). Trust the Session identity supplied over that private channel. The terminal ID is the one retained pane identity: it locates Pi after moves, without a second pane-address identity or owner-location baseline. The native Session ID identifies the conversation, not another pane; keep it to detect replacement before an old card continues. A fresh child can select the reported native ID or an exact retained transcript locator. It reads only the selected transcript, never neighboring files to find a match. Missing or ambiguous observations stop the affected native action with a diagnostic; they do not disable independent catalog reporting. Do not infer identity from a title, working directory or GUI focus.

Each relevant event or explicit request gets one pass: read the concerned records, take at most one native snapshot, make a pure plan, then execute it. Lower layers receive those inputs rather than observing again. An event says what to re-read, not which effect to perform. Compare normalized values, accept checked action replies without confirmation reads, and stop on failure. A later event or user request is a new pass, not replay of a saved action. There is no polling or retry timer.

Streams will persist no workspace, tab or pane addresses. Addresses belong to the current pass and are discarded afterwards. Delete `placements/`, including its saved workspace binding, nonce and pending-attempt state. Explicit reopening will find a workspace by its ordinary catalog and Stream marker, or create one. The marker describes the workspace; it does not authenticate a caller or promise exclusive ownership.

Keep a simple identity marker on managed cards and editors, sufficient to identify their role and Session or document. It is ordinary metadata, not a secret, caller credential or reciprocal pair handshake. Each marker has one writer. Remove per-launch nonces, pending/reserved/ready/closed records, immutable launch receipts, pair publication and predecessor retirement. The native writer queue may serialize work, but it does not make a snapshot immune to user changes.

A card will take a non-blocking process lock keyed by catalog and Session before becoming active. A duplicate that cannot acquire it exits. The operating system releases the lock on process death; file existence and saved PIDs are not ownership. This is a lifetime exclusion, not a persisted launch attempt. Cards still exit on `q` or observed owner loss without writing a closure record.

Follow is a stateless comparison on every pass: if the existing card is outside Pi's tab, move it below Pi. A user-moved card comes back on the next pass, including after a child restart. Keep no previous owner location or move baseline. A missing card stays missing until a Pi start/resume trigger or explicit open; following does not launch it.

## Bound targeting to the current pass

Close and move select only an observed card carrying this Session's marker. Streams will not close Pi or an editor as cleanup. It intentionally names Pi's tab and its associated workspace when synchronization is enabled. Name projection retains its Stream association and actual-first-pane eligibility rules.

Herdr 0.9.5 does not reuse public pane or tab numbers within a workspace. Its [workspace allocator](https://github.com/herdrdev/herdr/blob/v0.9.5/src/workspace.rs) can reuse a closed workspace's ID after a server restart. Pane and tab addresses include that ID, so they can also refer to different resources after restart. Using pass-local addresses removes stale saved targets, but not a restart between observation and action.

Register the Herdr subscription before observation. Long-lived children and cards already have one; a foreground native command keeps a short-lived subscriber for its pass, without reconnecting it. Before each effect, make one cheap check that the captured subscription has not ended. If it has ended, abandon the pass without an effect or retry. A later pass needs a usable subscription and a new observation. This is a local liveness check, not attestation or a new frontend protocol.

The user accepts the remaining restart race: the server can restart after that check, or before the subscription reader notices its end. Herdr does not atomically bind an ordinary effect to the server observed earlier. The check reduces this risk but cannot eliminate it, including for move destinations and naming targets. Do not claim atomic protection against address reuse. Do not add an instance-ID guard, change Herdr's ID model or seek an upstream change for this work.

This protects against ordinary changes and failures, not hostile local processes forging Streams metadata. A snapshot failure means unknown state, not proof that Pi exited. Card cleanup is eventual after a usable observation, not instantaneous during an outage.

## Demonstrate safety and progress in tests

Start with one scripted scenario for each outside change: move, close, rename, crash, lost reply, duplicate open and restart mid-pass. Inject changes before observation, between observation and action, or after an effect but before its reply, as each scenario needs. Exercise the real controller and adapter, not only a separate abstract protocol. Include conversation replacement and subscription loss in the relevant scenarios.

The fake must retain the actual resource identities independently of Streams' observations. Assert against the resource that received an effect, not merely the pane ID Streams requested. Restart Herdr between observation and action, deliver the subscription end before the runtime check, and assert that close or move never touches a pane without this Session's marker. A reply error after mutating the wrong pane is a test failure. This test covers the observed disconnect, not the accepted unobserved restart race.

Check safety after every step: no unrelated close or move, no automatic editor or Pi cleanup, no background reopening of a closed card, and no report retargeted by movement. Check progress after outside changes stop and observations and process scheduling resume: duplicates settle to at most one active card, a card with no owner exits, and a successful pass places the existing card in Pi's tab. A permanently unavailable frontend cannot provide those liveness guarantees.

Count work as well as outcomes. A settled projection produces no effects; a pass takes at most one snapshot and never re-reads a transcript to attest its caller. Run duplicate-exit tests with real processes and OS locks. Do not add generated sequences, model exploration or a shrinking framework unless bugs escape the scripted scenarios. Native operations will not execute the old proof state machine at runtime.

## Plan actions, then execute once

Pure planning combines typed records, the current observation and the request,
then compares desired and observed values to produce actions. Reading and
executing belong to the shell. Catalog changes and frontend effects have separate
action types and executors; no executor holds a transaction across both systems.

Each field has one direction: an observed workspace move can change Session
membership; saved titles feed native display. Display changes do not move Pi or
select membership. An echo event therefore finds no difference instead of
starting a feedback loop.

Trust a checked reply without a confirmation snapshot. Where the reply supplies
identity and values, verify that they match the action. An acknowledgement-only
adapter must model normalization accurately rather than claim an observed value
it did not receive. Handles from checked replies can feed later actions in the
same list. A failed or mismatched reply stops that list; execution does not
re-enter planning or save a retry.

## Catalog and UI boundaries remain

Keep record-local catalog saves, immutable host ownership, membership epochs,
captured report targets, document path checks and naming policy. Removing native
launch receipts does not remove catalog revision guards or Pi branch admission.
[Reconciliation](native-context-reconciliation.md) owns move-versus-refresh
semantics; [enrollment](native-enrollment-boundary.md) owns the record-only helper;
[native cards](native-herdr-work-cards.md) owns UI and document rationale.
