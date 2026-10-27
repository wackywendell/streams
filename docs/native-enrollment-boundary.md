# Enrollment and native placement from one observation

Status: implemented and activated. The native driver collects one observation and the concerned records, then passes them to record-only catalog work without another frontend lookup. [Observe and act](native-observe-and-act.md) owns effects and targeting.

## Separate catalog work from native binding

The driver owns native observation and ordinary workspace markers. Streams persists no workspace, tab or pane addresses. It passes its admitted `OpenedRecords`, parsed `EnrollmentRequest` and already-collected `RecordContext` to a record-only App helper. App saves the Stream and Session and selects their host-local directories. It does not resolve a native caller, collect another context, reopen the catalog or reload defaults. No native callback or display DTO crosses this boundary.

The local result must include its save receipts, the actual current Session, any matching Stream dependency already available, and whether directory selection completed. These are inputs to native completion, not a claim that native setup succeeded. A partial result may contain an accepted Stream without a Session; that does not establish the caller's membership.

An explicit Stream target or new-effort proposal keeps its captured target. A different native association is a reason to refuse, not to retarget that request. Native creation of a destination effort after an owner move is a separate operation, not permission to reinterpret an earlier model proposal.

## Reuse current records, not historical receipts

A fresh Session save returns the Stream dependency it already read, in memory. It does not serialize a second copy into the receipt or read it again to construct a response. A retained retry must preserve its target-only read budget. Its receipt describes the original result; `save.current` describes the Session now.

The driver may reuse its earlier Stream only when that record matches the returned Session's actual current membership and immutable host. If no matching dependency is available, the result must say so and native completion must stop. It must not combine the current Session with the historical receipt destination or perform a hidden context refresh.

## Observe workspace markers

A Herdr workspace carries ordinary catalog, host and Stream markers. From a workspace, the same snapshot's complete markers select the exact Stream to read. The child already knows its Session; no caller-proof object, placement read or reverse index is needed. Conflicting markers refuse selection rather than guessing from labels or working directories.

Explicit reopening finds a workspace by those markers in its snapshot, or creates one. Reuse is best effort, not an exclusive one-Stream-to-one-workspace claim. Use observed addresses only within the pass. An unavailable frontend is unknown, not positive absence; report it without a durable launch fence.

The already-collected Session and matching Stream can supply missing markers after enrollment. Do not overwrite markers that name a different catalog, host or Stream. Stream reopening follows the [active-Session workflow](record-local-operations.md#reopen-active-sessions), not saved layouts. Only the explicit user command discovers active Sessions; owner refresh never needs that set. Exact transcript locators live with host-local Session directory bindings, written through `DirectoryBindings` from the native driver's observed input.

There are no saved placements, associations or enrollment claims, nonces or pending-attempt records. Do not replace them with hidden scans. The [pass-local targeting policy](native-observe-and-act.md#bound-targeting-to-the-current-pass) owns the subscription-ended check and accepted restart risk.

## Native completion is best effort

Do not save native attempts, resource addresses or launch receipts. Workspace, Pi, card and editor launches follow the [observe-and-act failure policy](native-observe-and-act.md#behaviour): reuse observed resources cheaply, report uncertain results and never replay automatically. A later explicit request observes again.

After accepted local enrollment and directory selection, the driver projects nonconflicting workspace markers. A failed projection retains the accepted enrollment, warns and ends the pass. The catalog writer returns the value it saved; checked replies may supply handles to later preplanned actions within this pass. Neither result fabricates a newer snapshot. The native executor remains the single writer of these markers; presentation helpers cannot publish competing markers.

## Respect frontend capabilities

Herdr supplies observable workspace markers. CMUX currently provides no such metadata. An explicit Stream or current Session still selects its catalog records; supported native operations use current observations and checked replies, not saved placement. A workspace-only current lookup without such a selector must refuse clearly. It must not infer ownership from a label, fabricate Herdr capabilities or fall back to a private reverse index.

Activation used a supervised offline reset. This model reads no old associations, claims or layouts and has no migration/reset component.

## Keep accepted saves separate from completion

A directory or native completion failure after a local save retains the accepted result with a warning. It does not imply rollback or retry the operation. The driver must release its native writer before publication or network sync. Publication has its own outcome and cannot revoke an accepted local save.
