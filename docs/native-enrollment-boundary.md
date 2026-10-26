# Enrollment and native placement from one observation

The native driver will enroll a conversation from the same caller proof and relationship facts it collected for that trigger. The current CLI enrollment path connects another frontend, resolves the caller and verifies Pi twice. It also binds the native workspace itself. Keeping that helper unchanged would defeat the single-observation boundary and leave the driver with stale relationship facts after enrollment. The replacement is not yet active.

## Separate catalog work from native binding

The driver will own native admission, the Stream's placement record and its workspace binding projection. It will pass its admitted `OpenedRecords`, parsed `EnrollmentRequest` and already-collected `RecordContext` to a record-only App helper. App will save the Stream and Session and select their host-local directories. It will not resolve a native caller, collect another context, reopen the catalog or reload defaults. No native callback or display DTO crosses this boundary.

The local result must include its save receipts, the actual current Session, any matching Stream dependency already available, and whether directory selection completed. These are inputs to native completion, not a claim that native binding succeeded. A partial result may contain an accepted Stream without a Session; that does not permit a ready binding.

An explicit Stream target or new-effort proposal keeps its captured target. A different native association is a reason to refuse, not to retarget that request. Native creation of a destination effort after an owner move is a separate operation, not permission to reinterpret an earlier model proposal.

## Reuse current records, not historical receipts

A fresh Session save will return the Stream dependency it already read, in memory. It will not serialize a second copy into the receipt or read it again to construct a response. A retained retry must preserve its target-only read budget. Its receipt describes the original result; `save.current` describes the Session now.

The driver may reuse its earlier Stream only when that record matches the returned Session's actual current membership and immutable host. If no matching dependency is available, the result must say so and native completion must stop. It must not combine the current Session with the historical receipt destination or perform a hidden context refresh.

## Keep one placement authority

We will keep one host-local placement record per Stream, carrying its endpoint and workspace binding. Herdr bindings include a nonce for the placement lifetime, not a new nonce for each projection. Workspace metadata will be the observed projection of that record, not another authority. Maintaining authoritative Stream-to-workspace and workspace-to-Stream claims would create a multi-record transaction problem that these operations do not need. There will be no reverse claim index, SQLite store or generic enrollment claim protocol.

From a Herdr workspace, binding metadata in the same snapshot selects the exact Stream placement to read. Its catalog source, host, endpoint, workspace and nonce must agree with that record. Matching metadata is not native origin proof; the same opaque Pi caller proof remains required. From an explicit Stream, read its placement directly and observe its named workspace. A record or pending attempt at another endpoint is a refusal, not absence, so changing endpoint cannot bypass its fence.

For an unpainted workspace, the already-collected current Session and Stream can name a concerned placement. If that placement names this workspace, missing metadata is a projection difference, not a reason to invent a new destination Stream. Missing binding fields may be planned as differences only when every present field agrees with the concerned record. An incomplete binding is not ready; a conflicting binding cannot be overwritten or adopted. Neither authorizes resource creation before a checked projection reply. Stream reopening follows the [active-Session workflow](record-local-operations.md#reopen-active-sessions), not saved layouts. Only the explicit user command discovers active Sessions; owner refresh never needs that set. Exact transcript locators live with host-local Session directory bindings, written through `DirectoryBindings` from the native driver's verified input. No saved arrangement or open-state history remains.

This removes workspace-keyed associations and enrollment claims. Their exclusions will not survive as hidden directory scans. The canonical placement and observed projection must agree before binding-dependent effects are admitted. A stale placement pointing at a workspace now bound to another Stream cannot validate against that other binding's nonce.

## Preserve actual unsafe-call fences

Legacy enrollment claims record Stream and workspace identity around local catalog and directory writes. They do not journal native calls. Actual workspace, tab and agent launches use pending attempts that persist intent before dispatch and checked outcomes afterward. Card and editor launches have their own resource receipts. Those fences remain: no movement, enrollment or metadata difference may clear an uncertain launch, resume a loaded attempt or authorize a replacement resource.

The placement record will retain its Stream's active unsafe workspace attempt, including an unknown workspace result. This makes cross-endpoint exclusion a named Stream read rather than a search across every scope. It does not combine card/editor lifetime evidence with placement or treat missing metadata as proof that a resource is absent.

After accepted local enrollment and directory selection, the driver will save the desired placement and project nonconflicting binding metadata under the native writer. A projection failure retains the accepted enrollment and saved placement but cannot authorize a launch. The writer will return the record value whose save completed; checked metadata replies can bind later preplanned actions. Neither result fabricates a newer snapshot or reconstructs the Pi proof. The native executor remains the single writer of binding metadata; presentation helpers cannot publish an independent binding under another source tag.

## Respect frontend capabilities

Herdr supplies observable binding metadata. CMUX currently provides neither that metadata nor Pi attestation. An explicit Stream or current Session can still name its placement for the operations CMUX supports, using checked endpoint/workspace replies. A workspace-only current lookup without such a selector must refuse clearly. It must not infer ownership from a label, fabricate Herdr capabilities or fall back to a private reverse index.

This fresh record model follows the [manual catalog cutover](native-catalog-reset.md). It does not translate old associations or claims, read legacy layouts as placement, or add a migration/reset component. Card and editor evidence retain their existing strict lifetime format and safety rules.

## Keep accepted saves separate from completion

A directory or native completion failure after a local save will retain the accepted result with a warning. It will not imply rollback or retry the operation. The driver must release its native writer before publication or network sync. Publication has its own outcome and cannot revoke an accepted local save.
