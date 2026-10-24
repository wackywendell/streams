# One native controller for each Pi session

Accepted version 2 contract. Version 1 remains implemented until both sides switch together.

## Why change the protocol

Version 1 sends CLI argument vectors over the child's pipe. Pi receives native
scope, turns it into `--expect`, and requests projection, capture and opening.
The child also watches native events and refreshes that scope. Both sides decide
when native work should run, and the same observation crosses the pipe only to
be checked again.

The child should own native observation, planning and execution for its Pi
session. Pi should send user intent, not a saved native context. This removes
the second control loop without moving model calls or branch admission into Rust.

## Typed application requests

Version 2 should carry typed requests for lifecycle changes, user actions and
catalog operations. It should not carry `argv`, CLI flags or `--expect`. Each
request's fields should be the inputs to its application operation.

For example, the proposed envelope is:

```json
{"type":"request","id":7,"request":{"kind":"open_card"}}
{"type":"request","id":8,"request":{"kind":"session_started"}}
{"type":"request","id":9,"request":{"kind":"attach"}}
```

The child is already bound to the Pi session ID supplied at launch. An open
request should not supply another session ID, pane address or caller proof.
The child must establish caller origin itself. It should resolve only the owner
and pass the typed proof directly to the application operation, not read every
Pi transcript to reduce an inventory to that owner.

The one-shot CLI should parse its arguments into the same application requests.
Neither frontend should reconstruct CLI syntax inside the application layer.
Catalog context, capture inputs, preview, enrollment, apply, listing, workspace
capture and sync should have typed payloads too. Existing domain request and
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
and apply carry their domain request object, not serialized stdin. Workspace
capture names its target Stream. Sync carries the domain scheduling event.

A response has an ID and an outcome: `success`, `saved_with_warnings` or `error`.
Result payloads are objects. Successful catalog results retain their existing
domain shapes. Accepted saves with warnings retain the same result and a
nonempty list of warning messages; they are not failed or rolled-back saves.
Errors carry a kind and message. Native opening returns its own `display`
outcome (`opened`, `existing` or `suppressed`), and closing returns `closed: true`,
without a status confirmation read.

An active `attach` returns `{"native":"active"}`, with no display outcome.
An active `session_started` returns `{"native":"active","display":"..."}`
with the automatic-open outcome, including `suppressed` when automatic opening
is disabled. Either lifecycle request succeeds with
`{"native":"inactive","reason":"outside_herdr"}` outside Herdr, or
`{"native":"inactive","reason":"not_enrolled"}` when the addressed Session
does not exist. These are expected inactive states, not string-matched errors.
Malformed or unreadable records, unproved caller origin and native failures
remain errors. Explicit open or close requests report a refusal in inactive
states rather than claiming an effect succeeded.

Background work can emit `view` with `desired_pi_name`, or a `warning` message.
Request-scoped warnings belong to the response; the child must not emit them
again as unsolicited warnings. The shared fixtures are independent frames,
not a transcript with several responses to one request.
A null desired name means no Pi rename. A non-null name is the current saved
Session title when session-name synchronization is enabled. Pi applies it with
its checked setter. Neither event supplies caller proof or selects another
projection, capture or observation pass. Unknown fields, duplicate JSON keys,
unsupported versions and positional struct arrays are rejected. Version 1
remains active until the coordinated transport cutover.

## One native queue

The child should serialize native user requests and selected native or catalog
events through one execution queue. It should own startup, projection, native
layout capture, verified owner movement and card-closure cleanup. An event
selects records to read; its payload is not proof of current topology.

Each handler should collect its concerned inputs, make a pure plan and execute
that plan through the frontend adapter. The [effect-planning contract](effect-planning.md)
applies: compare normalized desired and observed values, without an applied-action
history or a no-repeat map. Use authoritative replies rather than confirmation
polling. An acknowledgement alone is enough only when the adapter models that
field's normalization exactly.

Catalog work that cannot mutate native state need not wait behind native work.
A catalog change that affects projections should select work on the native
queue, not start another controller. This is one controller per Pi session,
not a new host daemon. One-shot CLI actions should use the same handlers and
host-local native lock, without a separate automatic control loop.

## Lifecycle is not transport startup

The extension should still start one child per Pi session with the selected
binary, environment, working directory and plugin root. A session switch ends
the old child. The first line should be `{"type":"ready","protocol":2}`.
Readiness means the transport can accept requests; it should not itself trigger
native observation, projection, capture or opening.

Pi should send `session_started` for an actual Pi session start. Only that
lifecycle input may consider configured automatic opening. After a child crash,
Pi should send `attach` instead. Attachment may restore subscriptions and fresh
observations and refresh projections. It must not repeat automatic opening or
perform startup catch-up follow.

The extension should retain the existing limit of one automatic child restart
per Pi session. It should reject requests whose child died, warn and reattach,
without replaying those requests. A second failure stops Streams until reload.
Closing stdin should end the child and its subscriptions after bounded shutdown.
A restarted child has no inherited caller proof; an unprovable moved origin
must still refuse, even if the previous child could attest it.

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
view change calls for another native projection or capture. Enabled title sync
uses the authoritative catalog title; the protocol should have no naming
provenance or reclaim operation.

## Cancellation does not undo an effect

Each request ID should be a positive integer unique within the child's lifetime.
A `{"type":"cancel","id":7}` message should cancel that request, not kill the
child. Cancellation before dispatch should prevent the operation from starting.
Running work should stop at its next cancellable boundary.

If an unsafe call may already have happened, cancellation cannot prove that it
did not. The executor must preserve its evidence and any uncertainty fence.
It must not roll back, retry or replace the request. The child should still
finish its response; Pi may discard a response to an already-cancelled request.
Unknown or completed cancellation IDs should have no effect.

## Cut over both sides together

Framing remains one JSON object per newline. Invalid messages and an unsupported
protocol version should fail the connection, not select a fallback. Version 2
must replace the CLI-vector transport and the scope-to-`--expect` loop together,
including their fakes and tests. There should be no version-1 adapter.

This decision does not settle whether ready evidence stores a launch receipt
or current location. It also does not settle whether a later explicit open
finishes an acknowledged but incompletely published card. Those choices remain
open before the dependent card planner changes. Neither can relax pending-launch
uncertainty, caller origin or the rule against replacement during follow.
