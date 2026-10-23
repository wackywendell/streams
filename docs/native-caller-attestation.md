# Caller attestation after native moves

Proposal only. A live Pi conversation can remain the same while its workspace-qualified pane address changes. Finding that conversation in Herdr is not proof that it issued a request.

## Separate origin from location

At `0233c41b`, [caller lookup](../src/frontend/herdr.rs) requires the inherited pane ID to match. [Pi verification](../src/runtime/native.rs) checks the pane's registered conversation in the shared snapshot. The [topology](../src/frontend/herdr/topology.rs) contains terminal and agent identities, but those inventory fields do not establish request origin. Removing the inherited-ID check and searching by Pi UUID would lose that boundary.

Reconciliation needs two separate facts:

- the request belongs to this `(host, agent, native Session ID)` and runtime lifetime, in this catalog
- a fresh observation locates that runtime's terminal on this Herdr server, with its current pane, tab and workspace

Neither fact alone authorizes a catalog write or native effect. Stream ownership, captured branch and fields, resource ownership and pending-attempt evidence remain separate checks.

## Options

### Herdr-backed caller witness

Herdr could supply a caller witness tied to the invoking runtime, terminal and server incarnation. Resolving it would return the current location after a move. The server must establish that relationship from the invocation, not merely accept a supplied pane or terminal ID.

This is the preferred complete solution. Such a contract has not been established in the current Streams adapter. A terminal UUID, agent metadata, PID guess or GUI focus is not a substitute. The Herdr API and its restore/reuse guarantees need verification before choosing a concrete representation.

### Continuity through an already-attested Pi child

The [Pi child protocol](pi-child-protocol.md) provides a private, runtime-local request channel. A child could first bind using the existing strict caller check, then retain that verified server/terminal binding for its lifetime. After a move, one fresh observation would locate the same terminal and verify its Pi identity again.

This may cover moves while the child survives without a new Herdr API. It depends on verified terminal/server identity guarantees and channel isolation. The pipe preserves an established origin; it does not bootstrap one. A child first started or restarted after a move still has stale inherited context. A saved binding or matching UUID cannot repair that gap. This option alone therefore does not meet the event-driven contract's restart recovery requirement.

## Proposal and limits

Prefer a Herdr-backed witness. Investigate child continuity as a bounded bridge, not an automatic fallback after failed proof. Require both facts; refuse when either origin or location cannot be established.

Shell code should validate observations into typed evidence before pure reconciliation. Following the [effect-planning ADR](effect-planning.md), information becomes state changes, then separate catalog and Herdr actions. Each executor plans from fresh state under its own lock. Events trigger a fresh pass; they are not authority. Failed proof stops the affected actions without an automatic re-plan loop. Unavailable native location need not invalidate independently verified conversation-local reporting.

Retire the caller lifetime on conversation replacement. Same-conversation tree navigation instead invalidates its captured branch context. The version-1 child launch Session ID is not, by itself, either proof. Any additional lifetime protocol needs an explicit versioned contract. Cancellation or pipe closure cannot undo an accepted save or erase an uncertain native attempt.

Before activation, test moved addresses, duplicate live identities, inherited helper context, child restart after a move, server replacement and workspace restore. Distinguish absence from unknown evidence and reject recycled handles. Cross-host copies remain distinct Sessions; unsupported participation must fail explicitly as specified in the [reconciliation ADR](native-context-reconciliation.md#cross-host-participation).
