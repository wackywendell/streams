# Enrollment and native placement from one observation

[Observe and act](native-observe-and-act.md) supersedes this record's caller proofs, placement nonces and unsafe-launch fences. The record-only enrollment boundary remains: the native driver collects one observation and the concerned records, then passes them to catalog work without another frontend lookup. The replacement is not yet active.

## Separate catalog work from native binding

The driver will own native admission, the Stream's placement record and its workspace binding projection. It will pass its admitted `OpenedRecords`, parsed `EnrollmentRequest` and already-collected `RecordContext` to a record-only App helper. App will save the Stream and Session and select their host-local directories. It will not resolve a native caller, collect another context, reopen the catalog or reload defaults. No native callback or display DTO crosses this boundary.

The local result must include its save receipts, the actual current Session, any matching Stream dependency already available, and whether directory selection completed. These are inputs to native completion, not a claim that native binding succeeded. A partial result may contain an accepted Stream without a Session; that does not permit a ready binding.

An explicit Stream target or new-effort proposal keeps its captured target. A different native association is a reason to refuse, not to retarget that request. Native creation of a destination effort after an owner move is a separate operation, not permission to reinterpret an earlier model proposal.

## Reuse current records, not historical receipts

A fresh Session save will return the Stream dependency it already read, in memory. It will not serialize a second copy into the receipt or read it again to construct a response. A retained retry must preserve its target-only read budget. Its receipt describes the original result; `save.current` describes the Session now.

The driver may reuse its earlier Stream only when that record matches the returned Session's actual current membership and immutable host. If no matching dependency is available, the result must say so and native completion must stop. It must not combine the current Session with the historical receipt destination or perform a hidden context refresh.

## Keep one placement authority

We will keep one host-local placement record per Stream, carrying its endpoint and workspace binding. The binding identifies the catalog, host and Stream; it is not a per-launch proof. Workspace metadata will be the observed projection of that record, not another authority. Maintaining authoritative Stream-to-workspace and workspace-to-Stream claims would create a multi-record transaction problem that these operations do not need. There will be no reverse claim index, SQLite store or generic enrollment claim protocol.

From a Herdr workspace, binding metadata in the same snapshot selects the exact Stream placement to read. Its catalog source, host, endpoint and workspace must agree with that record. The child already knows its Session; no caller-proof object is required. From an explicit Stream, read its placement directly and observe its named workspace. An unavailable endpoint is unknown, not positive absence; report it without creating a durable launch fence.

For an unpainted workspace, the already-collected current Session and Stream can name a concerned placement. If that placement names this workspace, missing metadata is a projection difference, not a reason to invent a new destination Stream. Missing binding fields may be planned as differences only when every present field agrees with the concerned record. An incomplete binding is not ready; a conflicting binding cannot be overwritten or adopted. Neither authorizes resource creation before a checked projection reply. Stream reopening follows the [active-Session workflow](record-local-operations.md#reopen-active-sessions), not saved layouts. Only the explicit user command discovers active Sessions; owner refresh never needs that set. Exact transcript locators live with host-local Session directory bindings, written through `DirectoryBindings` from the native driver's verified input. No saved arrangement or open-state history remains.

This removes workspace-keyed associations and enrollment claims. Their exclusions will not survive as hidden directory scans. The canonical placement and observed projection must agree before binding-dependent effects are admitted. A stale placement pointing at a workspace now bound to another Stream must not overwrite that association. Safe resource targeting follows the frontend identity contract, not a placement nonce.

## Native completion is best effort

Do not add native attempts or resource receipts to the placement record. Workspace, Pi, card and editor launches follow the [observe-and-act failure policy](native-observe-and-act.md#behaviour): reuse observed resources cheaply, report uncertain results and never replay automatically. A later explicit request observes again.

After accepted local enrollment and directory selection, the driver saves the desired placement and projects nonconflicting binding metadata. A failed projection retains the accepted enrollment and placement, warns and ends the pass. The writer returns the value it saved; checked replies may supply handles to later preplanned actions. Neither result fabricates a newer snapshot. The native executor remains the single writer of binding metadata; presentation helpers cannot publish a competing binding.

## Respect frontend capabilities

Herdr supplies observable binding metadata. CMUX currently provides no such binding metadata. An explicit Stream or current Session can still name its placement for the operations CMUX supports, using checked endpoint/workspace replies. A workspace-only current lookup without such a selector must refuse clearly. It must not infer ownership from a label, fabricate Herdr capabilities or fall back to a private reverse index.

This fresh record model follows the [manual catalog cutover](native-catalog-reset.md). It does not translate old associations or claims, read legacy layouts as placement, or add a migration/reset component. Old native evidence remains relevant only while the old runtime is active; the replacement does not read or migrate it.

## Keep accepted saves separate from completion

A directory or native completion failure after a local save will retain the accepted result with a warning. It will not imply rollback or retry the operation. The driver must release its native writer before publication or network sync. Publication has its own outcome and cannot revoke an accepted local save.
