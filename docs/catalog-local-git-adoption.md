# ADR: Adopt an existing catalog into local Git history

Status: design and implementation reviewed; live activation remains a separate coordinated step. This does not authorise a remote, repository creation on a hosting service, fetching or pushing.

## Context and decision

Add local version history to the existing Streams catalog without changing its identity or making remote publication a prerequisite. Keep the catalog directory, records, ownership periods and host-local runtime bindings. Ordinary saves should then succeed at a local Git commit, as specified by the [sync ADR](catalog-git-sync.md).

Local-only storage retains the current records, original stream-creation envelopes and retry fingerprints. It does not retain every operation that produced those records. Reconstructing a plausible edit history would invent evidence; issuing replacement creation operations would change ownership-period tokens. Putting the files in an arbitrary Git commit would also fail the existing history validator.

Use an explicit snapshot baseline: a new supported root commit certifies the existing state and carries its available provenance. Later commits remain ordinary domain operations. The baseline records where verifiable operation history begins, not a claim that unavailable history was recovered.

## A baseline is not a replayed operation

Keep the existing empty `Streams catalog v1` root valid. Add a distinct, versioned `Streams catalog baseline v1` root format for adoption. It must have no parents. Its tree contains exactly the existing YAML record files, with their bytes and canonical paths unchanged. The adapter validates the complete catalog and its relationships.

The root commit message contains a strictly parsed manifest with:

- the importing host identity
- the original stream-creation envelopes, indexed by stream ID
- the existing operation-ID-to-SHA256 retry fingerprints

The Git-history parser validates the manifest without reading installation state. It requires an exact match between creation-map keys and snapshot stream IDs. Each envelope must decode to a stream creation: `before` is null, the stream ID matches its key, and its owner matches both the manifest host and the current stream owner. Session and host records must also belong to that host. Mutable creation fields may differ from the current snapshot; the missing intermediate edits are not replayed.

The adopter additionally checks that the manifest host matches the existing local initialization marker and installation identity. Host identifiers retain the domain's validation; installation UUID checks remain in catalog configuration. Unsupported or incomplete provenance stops adoption rather than guessing authority. No ownership handover is introduced.

Preserve each original creation envelope, including origin attribution. Require canonical round-trip equality through `decode_operation` and `encode_operation(op, origin=origin)`. Its retry fingerprint is separately `SHA256(encode_operation(op))`, whose origin is empty, matching `State._digest(op)`. Do not hash the attributed envelope directly. Preserve omission rules for optional fields and captured inputs. Do not replace an old operation ID or payload with the latest record.

Parse the baseline into immutable records at the history boundary. Keep three meanings separate: the current catalog snapshot, creation-derived authorities, and retired operation fingerprints. `History.operations` continues to mean actual operation commits after the baseline. Add an owned immutable `History.fingerprints` map, seeded from baseline receipts and extended with each subsequent operation's digest. A creation envelope embedded as provenance is not presented as a replayed commit.

The fingerprint map is an imported claim from the validated local coordinator. It proves what retry identities that coordinator retained, not the contents, order or time of unavailable operations. It belongs in the baseline so retry protection does not depend solely on one host's private `.catalog-sync` directory. An isolated local clone must recover the same authorities and fingerprints without that directory.

After the root, retain linear history and the existing one-operation, one-record commit rule. History parsing rejects every commit whose operation ID already exists in `History.fingerprints`, even when its digest matches. Save handling is different: a retry with its original fingerprint is a no-op; changed intent under that ID is repair-required.

A shared digest-map boundary on `State` compares the complete history fingerprint map with private receipts before remembering missing entries. `GitCatalog._history()` uses that boundary before save-time deduplication. `read_history()` recovers records, authorities and fingerprints from Git objects alone. Opening a local clone still needs explicit host-bound coordinator setup; that setup does not fabricate private receipts, which are derived from validated history.

## Git without a remote

Use the existing Git coordinator with an explicit no-remote configuration, rather than adding a second commit implementation or a dummy remote. The setup interface is `streams init --sync git --adopt`, with branch `main` by default. `--adopt` requires `--sync git`, rejects `--remote`, and requires an existing initialized catalog and its matching installation identity. It must never create a replacement host identity. Fresh empty catalogs may select the same no-remote Git mode without `--adopt`; they keep the existing empty genesis. Ordinary initialization must not silently convert an existing catalog.

`GitConfig.remote` becomes `str | None`; the JSON key remains required, with null explicitly selecting no-remote mode. Do not configure `origin`. `_config()` fails closed if any Git remote was added externally. `_ready()` requires absent or default sync metadata and no publication marker; remote timestamps or confirmation state cannot exist in this mode.

Every save outcome reports `sharing="local-only"`, including no-change, conflict, superseded and repair-required outcomes. A committed save reports `local="committed"`. Non-repair outcomes carry the validated local tip. Acceptance remains a separate domain fact, so local-only sharing cannot disguise a refused write.

No-remote reads validate the saved snapshot and return it directly with freshness `local-only`, no confirmed snapshot and no remote revision. They do not call read scheduling, fetch or replay. In particular, baseline-backed operations cannot reconstruct the imported snapshot by replaying onto an empty catalog. Any replay or candidate construction involving a baseline requires both histories to share that baseline root; otherwise it fails closed. Existing empty-root reconciliation remains unchanged.

Explicit refresh, startup, shutdown and `streams sync` remain network-free. Sync validates its event and local history, then returns `sharing="local-only"` with the local tip. Failures report local repair-required without publication failure bookkeeping. Neither reads nor sync write clocks, cooldowns or confirmation state. Low-level remote inspection and fetch reject no-remote configuration before a network command. Publication timing gates have nothing to schedule without a destination.

Remote publication requires separate explicit setup and baseline-publication design in future work. Existing configured-remote catalogs retain their current save, reconciliation and publication contracts; this proposal does not change them into local-only catalogs.

## Adoption changes machinery, not records

Hold the existing host configuration lock and catalog lock in their established order. Preserve the catalog directory and `.catalog.lock` inode. Do not swap the directory for a prepared clone: existing processes may already hold that lock, catalog-source fencing includes the directory identity, and runtime state is selected by the catalog's canonical path.

Validate the current local coordinator state and the complete record graph while holding the catalog lock. Refuse interrupted saves, existing Git machinery, unsupported files, symlinks or inconsistent provenance. Capture the record bytes and retained metadata from that locked state, not an earlier inventory.

Build and validate the Git objects and index in a temporary subdirectory of the catalog's `.catalog-staging/`, while holding the catalog lock. This preserves ancestor-scoped Git configuration rather than moving preparation to system temp. Use the host's normal Git author and signing configuration. Capture effective author/committer identity and signing policy, including format-specific signer settings. A preparation or signing failure cleans up only its own temporary candidate and leaves active local-only state unchanged; no uncertain adoption has begun.

Before installing prepared machinery, persist an adoption save marker with the intended baseline, original setup evidence and record fingerprints. Both local-only and Git readiness check this marker before directory or bookkeeping allowlists, so an interruption points to its evidence rather than an unexplained-directory error. Already-running older callers may still report their existing fail-closed directory error. Preserve original creation envelopes and retry-receipt bytes as evidence; the baseline carries the corresponding validated values.

Install only Git and coordinator machinery; do not rewrite record files or any host-local bindings. Verify the installed index against actual record files and require a clean managed worktree while the save marker still fences normal entry points. Resolve effective author/committer identity and signing policy at the final `.git` path and require equality with preparation. Exact-path `includeIf` rules can differ even when staging is inside the catalog. A mismatch stops before strategy switching and leaves the marker and evidence for manual repair; do not override configuration or silently roll back. This final-path check cannot be proved before Git machinery exists there.

Use internal validation rather than calling the normal readiness API through an active marker. Persist explicit no-remote setup, switch the CLI strategy marker after these checks, and clear the save marker last. Flush durable state using the existing atomic replacement and directory-sync boundaries.

Successful CLI output includes `baseline_revision` alongside catalog path, strategy and host ID, not acceptance of an invented domain operation. Repeating `--adopt` is idempotent only for matching host and branch, no-remote Git setup and a supported baseline root. It returns that root revision, not a later working tip. An ordinary empty-root Git catalog is not a completed adoption. Any interruption after the marker is preserved and reported as repair-required. Do not automatically roll back, resume an uncertain adoption, clear evidence or reinitialise it.

Existing processes are not a reason to weaken the save boundary. An old local-only coordinator waiting on the lock must reject the Git-backed directory after adoption. A later CLI process opens the configured Git coordinator. Because record bytes and logical identities do not change, captured record preconditions and verified native bindings do not need replacement IDs.

## Alternatives and accepted costs

Ordinary Git initialization with `add --all` was rejected: it could track private machinery and would not establish supported authority or retry history. Recreating every record through new operations would misrepresent historical intent and change creation tokens. Keeping a separate Git archive behind the local-only writer would retain two save boundaries and make interrupted saves harder to diagnose.

A root snapshot introduces a second supported genesis format and an explicit trust boundary. The parser must validate its graph and evidence, but cannot prove unavailable past operations. This is the cost of adopting state that was not originally event-logged. Current and subsequent history can be checked without pretending that the missing history exists.

The baseline makes retained provenance part of local Git history. Treat it as sensitive catalog data. Local commits protect against session loss, not machine loss; without a remote they provide no cross-host backup or publication guarantee.

## Acceptance and remaining gates

Before live activation, isolated adoption acceptance must demonstrate:

- identical record bytes, host and record IDs, creation envelopes, ownership tokens and retry digests before and after adoption
- unchanged catalog and lock identities, checkout IDs, directory bindings and native associations
- restoration of authorities and retry protection in a local clone with private bookkeeping absent
- safe same-ID retries and refusal of changed-intent reuse, including captured-input operations
- normal post-baseline commits passing the shared history validator
- zero fetch, push, remote probing or credential calls through every no-remote entry point
- path-scoped author/signing configuration honoured during preparation, with any final-path policy divergence refused before strategy switching
- failure injection around marker creation, Git installation, strategy switching and marker clearing, preserving evidence without rollback
- a waiting old writer refusing after adoption, and new CLI/Pi processes saving through Git without replacing membership

Live activation requires reviewed code, a tested native/runtime base, isolated acceptance results and a coordinated exclusive adoption window. Future remote activation must also define how published history retains fingerprints for post-baseline operations retired by replay, so fresh clones retain retry protection. Remote setup, publication and ownership handover remain outside this slice.
