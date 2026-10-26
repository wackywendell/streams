# ADR: Observe native state and test behaviour under outside changes

Status: accepted direction; not implemented. This replaces native caller attestation, pair proofs and durable launch fences, not catalog save or report-admission rules.

## Make ordinary native actions ordinary

Opening a card, moving Pi or reopening work currently passes through caller attestation, reciprocal ownership markers and recorded launch phases. Every feature must preserve that protocol. The user wants simple, straightforward and predictable behaviour instead: outside changes are normal inputs, and rare duplicate launches are preferable to a recovery system.

Streams will observe current native state, compare it with the requested behaviour and act once. Correctness will be demonstrated by tests that change the outside world during execution, not by running a proof protocol before each effect. Small runtime checks remain where they directly prevent a wrong action. Tests do not make an unsafe frontend operation safe.

## Behaviour

| Operation or outside change | Required response |
| --- | --- |
| Open a card | Reuse an observed card for this Session, or open one beside Pi. Brief duplicate panes are acceptable; duplicate cards exit themselves. |
| Close a card | `q` exits the card. Explicit external close targets only that Session's card. Neither closes Pi nor changes catalog records. |
| Pi starts or resumes | Open a card if automatic opening is enabled. A child restart is not a Pi start. |
| Card closes during a running Pi session | Leave it closed until an explicit open. Reporting and membership reconciliation continue. |
| Pi moves | Bring its existing card into Pi's tab, below Pi. Do not create a missing card. |
| User moves only the card | Leave it there until Pi next moves. Separation alone is not a reason to close it. |
| Pi exits or changes conversation | The old card exits when it observes that change. It does not close another pane. |
| Open a document | Reuse an observed Streams editor for the exact resolved document; otherwise open one. Rare duplicates are acceptable. Never automatically close or move editors. |
| Reopen a Stream | Open active same-host Sessions in separate tabs. Skip Sessions already observed running and report their locations. Restore no saved layout. |
| Rename a native surface | Keep the [name ownership policy](name-ownership.md): enabled synchronization projects saved titles; disabled synchronization leaves the native name alone. |

Reuse must be cheap. Match reported identities in the operation's snapshot; do not search transcripts or build a discovery protocol to avoid a duplicate. A missing saved transcript locator still prevents resuming that Session: allowing duplicate launches does not permit inventing a conversation. Missing launch inputs produce a Session-specific diagnostic.

Editors, reopened Pi sessions and workspaces have best-effort duplicate avoidance, not an exactly-once guarantee. A lost launch reply reports that the resource may have opened and ends the affected pass. There is no automatic retry. A later explicit request observes again. Two requests can race, or a launched Pi can register late; the user may need to close a duplicate. Streams must not automatically terminate a Pi or editor to repair this.

## Small runtime, no launch history

Keep one native controller per Pi session and the typed [Pi child interface](pi-child-protocol.md). Trust the Session identity supplied over that private channel. Use reported conversation and terminal identities to locate its current pane, including after movement or child restart. Missing or ambiguous observations stop the affected native action with a diagnostic; they do not disable independent catalog reporting. Do not infer identity from a title, working directory or GUI focus.

Each relevant event or explicit request gets one pass: read the concerned records, take at most one native snapshot, make a pure plan, then execute it. Lower layers receive those inputs rather than observing again. An event says what to re-read, not which effect to perform. Compare normalized values, accept checked action replies without confirmation reads, and stop on failure. A later event or user request is a new pass, not replay of a saved action. There is no polling or retry timer.

Keep a simple identity marker on managed cards and editors, sufficient to identify their role and Session or document. It is ordinary metadata, not a secret, caller credential or reciprocal pair handshake. Each marker has one writer. Remove per-launch nonces, pending/reserved/ready/closed records, immutable launch receipts, pair publication and predecessor retirement. The native writer queue may serialize work, but it does not make a snapshot immune to user changes.

A card will take a non-blocking process lock keyed by catalog and Session before becoming active. A duplicate that cannot acquire it exits. The operating system releases the lock on process death; file existence and saved PIDs are not ownership. This is a lifetime exclusion, not a persisted launch attempt. Cards still exit on `q` or observed owner loss without writing a closure record.

Follow retains the existing owner-move trigger, not continuous enforcement of co-location. In-memory owner location is enough to compare observations. Startup establishes a baseline; a restarted child does not catch up a missed follow or reopen a card. The next observed Pi move brings the existing card along. No pending follow intent or saved layout is needed.

## Keep targeting safe at the frontend boundary

The safety rule is narrower than “never touch a pane Streams did not create.” Streams intentionally names Pi's tab and its associated workspace when synchronization is enabled. It must never close or move an unrelated pane, close Pi or an editor as cleanup, or select another resource through a recycled address. Name projection retains its binding and actual-first-pane eligibility rules.

A marker check in a snapshot cannot protect an address reused between observation and execution. The frontend adapter must provide non-reused resource handles for the action's lifetime, or a single identity precondition checked atomically with the effect. That requirement includes the destination of a move and targets of naming. If an operation lacks both, do not claim the invariant or build a local attestation protocol around it: add the small frontend guard before enabling that operation.

The current Herdr adapter sends public pane addresses. [Herdr 0.9.5 documents](https://raw.githubusercontent.com/herdrdev/herdr/v0.9.5/docs/next/website/src/content/docs/socket-api.mdx) that cross-workspace moves change those addresses while preserving the terminal. This review has not established non-reuse guarantees across server replacement or an atomic identity precondition for all affected methods. That is an implementation dependency, not a claim that today's adapter already meets this decision. No particular new Herdr API is prescribed here.

This is protection against ordinary changes and failures, not hostile local processes forging Streams metadata. A snapshot failure means unknown state, not proof that Pi exited. Card cleanup is eventual after a usable observation, not instantaneous during an outage.

## Demonstrate safety and progress in tests

Extend the existing fake Herdr to inject outside changes before observation, between observation and each action, and after an effect but before its reply. Exercise the real controller and adapter, not only a separate abstract protocol. Generated sequences should include owner and card moves, closes, renames, delayed registration, duplicate opens, conversation replacement, process crashes, lost replies, subscription loss and recycled addresses after server restart.

The fake must retain the actual resource identities independently of Streams' observations. Assert against the resource that received an effect, not merely the pane ID Streams requested. A stale-address test must fail if a wrong pane was affected, even when the reply later makes the controller report an error.

Check safety after every step: no unrelated close or move, no automatic editor or Pi cleanup, no background reopening of a closed card, and no report retargeted by movement. Check progress after outside changes stop and observations and process scheduling resume: duplicates settle to at most one active card, a card with no owner exits, and a successful owner-move pass places the existing card correctly. A permanently unavailable frontend cannot provide those liveness guarantees.

Count work as well as outcomes. A settled projection produces no effects; a pass takes at most one snapshot and never re-reads a transcript to attest its caller. Run duplicate-exit tests with real processes and OS locks as well as modelled lock ownership. Preserve shrunk failure sequences as regression cases. Property or bounded model exploration may supplement these tests; native operations will not execute the old proof state machine at runtime.

## What this supersedes

The [caller-attestation proposal](native-caller-attestation.md) is retired. Native lifetime and effect-proof requirements in [reconciliation](native-context-reconciliation.md), [effect planning](effect-planning.md), [bounded work](bounded-work-and-card-events.md), [Pi child transport](pi-child-protocol.md), [native enrollment](native-enrollment-boundary.md) and [record-local operations](record-local-operations.md) yield to this decision. The original [native-card ADR](native-herdr-work-cards.md) remains the UI and import rationale, not a requirement to preserve its recovery protocol.

Keep record-local catalog saves, host ownership, membership periods, captured report targets, document path checks and naming policy. Removing native launch receipts does not remove catalog save receipts or Pi branch admission. Activation and the [manual catalog cutover](native-catalog-reset.md) remain coordinator work; this ADR changes no live state and does not authorize discarding evidence while the old runtime is active.

## Appendix: measured retirement footprint

At `c41d1c47`, `src/runtime/native*.rs` and recursive `src/runtime/native_cards/` contain 7,959 physical lines: 4,424 outside test modules and 3,535 in test modules. The non-test portions declare 51 structs/enums, plus one trait and one type alias. Counts include comments and blank lines; they are not executable-line counts.

These modules implement machinery the new design will retire:

| Module | Non-test lines | Structs/enums |
| --- | ---: | ---: |
| `src/runtime/native_state.rs` | 555 | 8 |
| `src/runtime/native_cards/handover.rs` | 199 | 1 |
| `src/runtime/native_cards/facts.rs` | 162 | 1 |
| `src/runtime/native_cards/linking.rs` | 113 | 1 |
| `src/runtime/open_protocol.rs` | 99 | 3 |
| Total | 1,128 | 14 |

The last module is outside the native-file baseline above. These are gross retirement footprints, not a forecast net deletion: the small card lock and ordinary resource data still need homes. Repeated guards in opening, documents, projection and caller observation, plus pair classification in `src/frontend/herdr/ownership.rs`, add further removal not included in that total. Existing tests that only enforce the retired protocol will be replaced with behavioural tests; the measured baseline does not predict the size of that replacement suite.
