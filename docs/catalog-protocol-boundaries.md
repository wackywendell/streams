# ADR: Put catalog protocol invariants at typed boundaries

Status: implementation refinement of the [catalog sync ADR](catalog-git-sync.md). This records the structure chosen for its existing guarantees, not a new sync protocol or permission to change an installed catalog.

## Context and decision

Keep each protocol invariant with the boundary that can establish it. Parse durable transport state before using it, distinguish save acceptance from publication, and share one captured-input boundary across asynchronous agent work. Keep validated history immutable so reusing it cannot change an earlier validation result.

The domain already has immutable records and one reconciliation function. Several failures arose outside that core: a malformed timestamp became an ordinary retry failure, an incomplete publication marker failed deep inside sync, and enrollment mistook a returned repair condition for successful completion. Pi reporting and card discussion also checked different subsets of the same capture conditions. More checks in individual callers would leave these rules easy to omit again.

This refines implementation boundaries without adding a universal backend interface, multi-record transactions or a new effect-driven Git state machine.

## Parse durable protocol state before effects

The transport-state boundary owns decoding and validating configuration, timing metadata and phase-specific publication records. Callers receive immutable values rather than JSON dictionaries. An attempted publication has its candidate, local tip, remote base and operation identities. An adoption additionally requires its source and destination revisions; normal writes cannot proceed while it remains incomplete.

A supported JSON version alone does not establish valid state. Missing evidence, unknown phases, invalid timestamps or mismatched publication destinations require inspection before further effects. Preserve the original bytes rather than rewriting them as an ordinary network failure or cooldown. A missing timing file can establish a fresh baseline; a malformed existing file cannot.

Confirmation remains separate from remote acceptance time. Persist a recovered confirmation's scheduling baseline once and preserve it on later retries. Zero is a valid timestamp, not a missing-value sentinel.

## Keep acceptance and publication independent

A result carries separate domain, local-save and sharing facts. Shared predicates interpret those facts; command handlers and agent integrations do not reconstruct acceptance from arbitrary status strings. A save is accepted only when it has an accepted domain outcome and does not require repair. A publication result has no domain outcome and cannot, by itself, prove that an edit was saved.

Ordinary publication failure can warn while an accepted local save remains usable. Local repair, refused intent and incomplete command effects stop follow-on writes derived from that receipt. In particular, publication may be confirmed while adoption needs repair. Preserve both facts and any earlier successful save receipts; neither a nonzero command exit nor a warning means that those saves rolled back.

## Share captured-input validity across agent paths

Reporting and discussion use the same capture boundary. It retains the original catalog source, attachment, native conversation, working directory, environment selection and conversation prefix. Before applying generated intent, check that capture again and reread attachment identity. Keep the original logical record guards in the operation; do not replace them with a newer snapshot to make stale output applicable.

One session scope owns cancellation and validity. A tree change cancels inference and resets reporting cadence. It need not discard a valid transport diagnostic for the same live conversation. Replacing or shutting down the conversation also retires that diagnostic lifetime. Shutdown can then await its own bounded sync result without letting an older startup warning appear afterward.

Cancellation is not rollback. A write already submitted may finish; its durable receipt remains authoritative even when its completion must not update a retired conversation. Failed or cancelled discussion retains user input without starting replacement inference automatically. Runtime verification of who may enroll a native conversation is a separate check, not a property granted by a capture token.

## Reuse validated values without changing their meaning

History validation returns owned, immutable mappings, including operation attribution. Cache requested validated tips rather than each intermediate commit: retaining a confirmed tip and a later local tip should not evict one merely while traversing the other. The cache is an optimization, not permission to skip validation of an unknown descendant.

The file adapter decodes the one canonical record changed by a supported commit. The validator checks the allowed paths and compares the resulting logical catalog with domain reconciliation. This avoids repeatedly decoding the entire catalog under its lock without introducing a second record parser or a weaker history rule.

Edit preparation likewise belongs to the domain. User-supplied title or goal text selects manual mode unless explicitly overridden; changing source text clears its compact derivative unless replaced. Wire parsing normalizes before constructing a record, so clearing a goal cannot create an invalid intermediate short goal. A concurrent naming-mode change still fences captured text edits, even when normalization selected the same mode.

## Consequences and limits

Strict parsing accepts conservative repair conditions rather than guessing which evidence was intended. Immutable history snapshots retain maps in memory, so caching remains bounded. Sharing capture logic reduces duplicated policy but does not make subprocess cancellation or filesystem writes transactional.

The existing Git pass still holds the catalog lock through bounded publication. This refinement does not solve remote availability, provide automatic repair, or implement handover. Those remain separate decisions. Revisit the protocol shape if a concrete backend or multi-record operation needs guarantees that these boundaries cannot express; do not add that machinery merely to anticipate one.
