# One native controller for each Pi session

Active version-2 transport, switched together in Rust and Pi. There is no version-1
adapter. The child owns the native queue, lifecycle, observation, membership
reconciliation and projection. [Observe and act](native-observe-and-act.md) owns
effects; [reconciliation](native-context-reconciliation.md) distinguishes matching
workspace-move events from refreshes that preserve membership.

## Why change the protocol

Version 1 sent CLI argument vectors over the child's pipe. Pi received native
scope, turned it into `--expect`, and requested projection, capture and opening.
The child also watched native events and refreshed that scope. Both sides decided
when native work should run, and the same observation crossed the pipe only to
be checked again.

The child should own native observation, planning and execution for its Pi
session. Pi should send user intent, not a saved native context. This removes
the second control loop without moving model calls or branch admission into Rust.

## Typed application requests

Version 2 should carry typed requests for lifecycle changes, user actions and
catalog operations. It should not carry `argv`, CLI flags or `--expect`. Each
request's fields should be the inputs to its application operation.

For example:

```json
{"type":"request","id":7,"request":{"kind":"open_card"}}
{"type":"request","id":8,"request":{"kind":"session_started"}}
{"type":"request","id":9,"request":{"kind":"attach"}}
```

The child is already bound to the Pi session ID supplied at launch. An open
request should not supply another session ID, pane address or caller proof.
The child trusts that bound identity and locates the owner from one native
observation. It does not prove request origin or search Pi transcripts.
Missing or ambiguous identity stops the affected native action, not independent
catalog work.

The one-shot CLI should parse its arguments into the same application requests.
Neither frontend should reconstruct CLI syntax inside the application layer.
Catalog context, capture inputs, preview, enrollment, apply, listing and sync
have typed payloads too. Existing domain request and
result schemas remain the contracts for those operations.

Responses should retain request IDs and distinguish success, failure and a save
accepted with warnings. Each operation should return its own result, rather
than requiring another native query to discover what execution just did. The
concrete request and response types must be shared before either side activates
version 2; the examples above do not define the remaining payloads.

## Concrete wire schema

[`child_protocol`](../src/child_protocol.rs) defines the version-2 frames and
application request fields. [Shared fixtures](../tests/fixtures/pi-child-v2.json)
cover both directions. Their catalog objects illustrate the envelope, not valid
domain submissions; the existing domain codecs still admit those packets.
Context is bound to the child's Pi session, with an optional Stream selector.
Capture-input selectors explicitly distinguish inventory, Stream and workspace.
Preview carries its catalog, action and request as separate fields. Enrollment
and apply carry their domain request object, not serialized stdin.
Sync is simply `{"kind":"sync"}`, without an event or scheduling policy.
`lifecycle` takes `"action": "resume" | "park"` and transitions the addressed Pi
Session between `parked`/`closed` and `active` (skipping `done` Sessions),
returning `{"session_changed": bool, "saves": [...], "session": ...}` and
emitting save notifications when the Session changes.
`{"kind":"list_streams"}` returns `{"streams": [...], "unreadable": [...]}`:
every Stream the catalog listing could read, and each record file it could not
decode as `{"kind": "host" | "stream" | "session", "host": string, "path": string,
"cause": string, "repair": string}`. `repair` is chosen by kind in Rust; the
extension only displays it. The chooser offers this host's readable Streams and
names this host's unreadable records, since `/streams reset` cannot repair a replica.
`{"kind":"discard_unreadable_card"}` runs before a reset captures anything. It
discards whichever records of the child's card cannot be decoded and returns
`{"session": string | null, "stream": string | null}`, the decode error of each
record it discarded. It emits a save notification when it discarded anything.
`set_title_default` accepts only a `stream` or `session` target and an
`automatic` or `manual` mode. It saves the corresponding global naming setting
and returns `{"title_defaults":{"stream":"...","session":"..."}}`.
Context includes current title defaults. This request does not generate a title,
project a frontend name or permit changing the live child's catalog root.

A response has an ID and an outcome: `success`, `saved_with_warnings` or `error`.
Result payloads are objects. Successful catalog results retain their existing
domain shapes. Domain refusals, repair requirements and publication failures
remain in those results, even when the one-shot CLI exits nonzero.
An error means the operation could not produce its domain result.
Accepted saves with warnings retain the same result and a
nonempty list of warning messages; they are not failed or rolled-back saves.
Errors carry a kind and message. Explicit opening returns its own `display`
outcome (`opened` or `existing`), and closing returns `closed: true`, without a
status confirmation read.

An active `attach` returns `{"native":"active"}`, with no display outcome.
An active `session_started` returns `{"native":"active","display":"..."}`
with the automatic-open outcome, including `suppressed` when automatic opening
is disabled. Either lifecycle request succeeds with
`{"native":"inactive","reason":"outside_herdr"}` outside Herdr, or
`{"native":"inactive","reason":"not_enrolled"}` when the addressed Session
does not exist. These are expected inactive states, not string-matched errors.
Malformed or unreadable records, unavailable or ambiguous native identity and
native failures remain errors. Explicit open or close requests report a refusal
in inactive states rather than claiming an effect succeeded.

Background work can emit `view` with `desired_pi_name`, or a `warning` message.
Request-scoped warnings belong to the response; the child must not emit them
again as unsolicited warnings. The shared fixtures are independent frames,
not a transcript with several responses to one request.
A null desired name means no Pi rename. A non-null name is the current saved
Session title when session-name synchronization is enabled. Pi applies it with
its checked setter. Neither event supplies caller proof or selects another
projection, capture or observation pass. Unknown fields, duplicate JSON keys,
unsupported versions and positional struct arrays are rejected.

## Card preferences

`card_settings` returns `global`, `session` and resolved `effective` modes. Global and effective modes are `automatic` or `manual`; session mode also accepts `inherit`. `set_card_mode` takes `target` (`global` or `session`) and `mode`. It returns the same settings object after saving. Global `inherit` is rejected.

Only the Rust child reads or writes card preferences. Pi uses these requests over stdio. The card uses the same requests on its private local connection, supplied through `STREAMS_CARD_CONTROL_SOCKET`. That connection accepts no other operations. An explicit `q` sends session `manual`; a failed save warns but does not prevent exit. See [card preferences and control](card-preferences-and-control.md).

## One native queue

The child should serialize native user requests and selected native or catalog
events through one execution queue. It owns startup, projection, observed owner movement and card following. An event
selects records to read; its payload is not proof of current topology.

Each handler should collect its concerned inputs, make a pure plan and execute
that plan through the frontend adapter. The [observe-and-act contract](native-observe-and-act.md#plan-actions-then-execute-once)
applies: compare normalized desired and observed values, without an applied-action
history or a no-repeat map. Use authoritative replies rather than confirmation
polling. An acknowledgement alone is enough only when the adapter models that
field's normalization exactly.

Catalog work that cannot mutate native state need not wait behind native work.
A catalog change that affects projections should select work on the native
queue, not start another controller. This is one controller per Pi session,
not a new host daemon. One-shot CLI actions should use the same handlers,
without a separate automatic control loop or caller-proof protocol.

## Lifecycle is not transport startup

The extension should still start one child per Pi session with the selected
binary, environment, working directory and plugin root. A session switch ends
the old child. The first line should be
`{"type":"ready","protocol":2,"source":"<digest>"}`, where `source` is the
digest of the Rust sources the binary was built from (see `build.rs`).
Readiness means the transport can accept requests; it should not itself trigger
native observation, projection or opening.

An extension loaded from a checkout computes the same digest over its own
sources. Any other first line, or a different digest, means the binary was not
built from this checkout. The extension then refuses that binary with one
error naming the binary, the checkout and both digests, does not restart it,
and rejects every later request with the same error until Pi reloads. A binary
built from other sources would otherwise fail later in its own way, for example
by rejecting a record field it does not know. A packaged extension has no
sources to compare and skips the check.

An eligible interactive Pi session should send `session_started` before startup
sync. That lifecycle input considers configured automatic opening. A binding change in an already-observing child can also open automatically, as defined by the [card preference decision](card-preferences-and-control.md).
Catalog-only and headless callers should send no native lifecycle request,
including after a child restart. Inherited Herdr environment variables must not
enable native work for those callers.

After a crash, a child whose native lifecycle was enabled should send `attach`
before new requests. Attachment may restore subscriptions, observe the owner
and refresh projections. It must not repeat automatic opening. It compares current card and Pi placement
and follows an existing separated card; it keeps no movement baseline. Catalog requests need not await the attachment outcome.
Lifecycle errors should warn without disabling catalog operations.

The extension should retain the existing limit of one automatic child restart
per Pi session; a refused binary is never restarted. It should reject requests whose child died without replaying
them. A second transport failure stops Streams until reload. Closing stdin
should end the child and its subscriptions after bounded shutdown. A restarted
child locates its bound Session from current reported identity. Stale inherited
pane addresses are not proof of absence and do not require origin recovery.

## Pi keeps model and branch admission

Pi should keep helper/model execution, branch checks and admission of model
results. Captured inputs, dependencies and targets remain fixed. A changed
native location or membership must not replan or retarget a captured shared
result, including a move away and back.

Pi should send the resulting typed catalog intent to the child. Rust still
checks catalog and membership conditions when applying it. A fresh child view
is not permission to rewrite the captured target. Discussion retains its
fixed-membership contract.

The child may return display facts and a desired Pi session name. Those are UI
outputs, not caller authority to send back with the next native action. Pi may
apply the desired name through its checked setter, but must not decide that a
view change calls for another native projection. Enabled title sync
uses the authoritative catalog title; the protocol should have no naming
provenance or reclaim operation.

## Cancellation does not undo an effect

Each request ID should be a positive integer unique within the child's lifetime.
A `{"type":"cancel","id":7}` message should cancel that request, not kill the
child. Cancellation before dispatch should prevent the operation from starting.
Running work should stop at its next cancellable boundary.

If a native call may already have happened, cancellation cannot undo it.
Report that uncertainty without a persistent launch fence. Do not roll back,
retry or replace the request. The child should still finish its response;
Pi may discard a response to an already-cancelled request.
Unknown or completed cancellation IDs should have no effect.

## Cut over both sides together

Framing remains one JSON object per newline. Invalid messages and an unsupported
protocol version should fail the connection, not select a fallback. Version 2
must replace the CLI-vector transport and the scope-to-`--expect` loop together,
including their fakes and tests. There should be no version-1 adapter.

Native opening, card lifetime, following and duplicate handling use the
[observe-and-act decision](native-observe-and-act.md), not the retired receipt
and pair-publication protocol. The typed transport does not carry proof objects.
