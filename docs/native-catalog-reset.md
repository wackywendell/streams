# ADR: Native cards at the catalog cutover

Status: accepted design, not implemented. This extends the [explicit catalog cutover](record-local-operations.md#explicit-cutover). It does not change ordinary card reconciliation or authorize a worker to touch live state.

The catalog cutover changes `catalog_source` to the saved host UUID and canonical catalog path. Native evidence keys include that source. Existing viewers also carry it in their immutable launch context. Keeping the old evidence alone would not keep those viewers working. Rekeying would need identity inputs that the digest-only receipts do not all retain, particularly for editors.

We will make a clean break for completed card lifetimes instead of adding source aliases, viewer handoff or a rekeying mechanism. This fits the empty catalog and explicit re-enrollment, and keeps the uncertainty rule intact: an unresolved launch is not permission to start another one.

## One offline boundary

All old Streams children and card viewers must be stopped before the procedure starts. The caller explicitly selects the host-local native evidence root and Herdr endpoint. This resets card lifetimes in that root, not an inferred catalog subset: a digest-only record cannot establish which catalog to select. Do not discover more roots or silently broaden the endpoint scope.

The native component receives the caller's already-held native writer guard. It must not acquire another native lock, open a catalog or construct Source. It reads the strict version-2 evidence and takes one fresh native snapshot. It constructs and validates the whole cleanup plan before dispatching any effects or changing catalog/runtime state.

Any `Unlaunched` or `Reserved` card or editor attempt blocks the cutover before mutation. Corrupt, mixed-endpoint, ambiguous or otherwise unclassifiable evidence also blocks it. Preserve those records unchanged. Stopping a writer or failing to find a pane is not proof that an unacknowledged creation did nothing. Do not repurpose the [version-1 evidence migration](native-context-reconciliation.md#offline-reset) as a way around these checks.

## End only proved card lifetimes

For completed card attempts, the private receipt, purpose, endpoint, nonce and fresh native ownership facts must identify the exact resource. The receipt's terminal identifies a completed card; its original pane and tab are not current placement. A separated or moved card is still the same resource when those facts agree. Unclassified panes are not cards to close.

The plan closes only those positively identified card resources, checks each authoritative close acknowledgement and retires only the corresponding evidence. If a resource is already absent, the snapshot must positively establish that absence. Clear only the matching owned pair and Session-summary metadata. Archive the retired card evidence with the cutover artifacts; do not turn it into evidence for a new live card. Missing or nonce-mismatched evidence makes any late old viewer exit, without writing evidence.

The same offline writer retires the obsolete Streams-owned workspace binding and bounded goal metadata. Clearing runtime associations alone would leave the old native source hash in place and prevent re-enrollment. Do not rename unassociated workspaces or panes, clear another producer's metadata or erase metadata that protects a preserved workspace-open reservation. If cleanup cannot be separated from such a reservation, preflight refuses rather than weakening its fence.

## Preserve everything outside that boundary

Leave editor panes and completed editor evidence alone. Do not close, adopt or rekey them. As with the existing explicit evidence-reset policy, a later document open may create another editor; accepting that alpha cost is simpler than guessing its identity.

Preserve every workspace-open attempt and reservation fence, including `workspace-open.json`. Never wipe the native/runtime tree. The caller owns catalog and runtime retirement; the native component does not reinterpret these attempts as catalog data or translate an old authority into permission.

The catalog reset follows only after native preflight and required cleanup succeed. The first failed or uncertain effect ends the pass. Keep evidence and markers that were not safely retired, retain the archive and report acknowledged effects; do not retry, roll back or create replacements. The coordinator alone runs the combined live procedure after fixture verification.
