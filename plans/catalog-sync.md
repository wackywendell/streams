# Catalog sync implementation handoff

Design draft for the next implementation model. The [sync ADR](../docs/catalog-git-sync.md) owns the decisions and rationale. The [catalog ADR](../docs/durable-work-catalog.md) owns record meaning and lifecycle. This plan defines the first sync slice against `durable-catalog-foundation`; it does not authorise changing the installed runtime or creating a hosted repository.

## Scope and existing contracts

Implement local-only coordination and a file-backed Git strategy. Keep policy, Git subprocess handling and domain reconciliation separate. Do not add a general backend framework, a daemon, automatic migration, frontend changes or agent inference.

The foundation provides:

- `catalog.operations.Operation`: stable ID, original before/after records, actor and session ownership precondition
- `catalog.operations.reconcile`: pure changed/no-change/superseded/conflict results
- `catalog.storage.Store.transaction`: a serialized scope, not rollback or a multi-record transaction
- `catalog.storage.apply_operation`: reconciliation and single-record persistence within that scope
- `catalog.yaml_store.YamlStore`: revision-checked YAML persistence without a Git dependency

Use one logical record per operation and operation commit initially. Keep the original operation even when reconciliation produces a record containing additional independent changes. Do not rebuild intent from the resulting file.

The current operation guards cover target before/after state and session authority. Cross-record or document-based inference needs an additional captured-input contract from its domain/integration layer. Do not claim that a fresh read immediately before saving protects inputs captured earlier. This slice adds no inference path and must not invent source-guard fields in domain records.

Physical path encoding belongs to the YAML adapter. If Git needs a managed-file inventory or record-to-path mapping, expose a small file-adapter capability rather than duplicate its private encoding or add paths to logical records. Keep `.catalog.lock` untracked and preserve its inode. Exclude `.catalog-staging/` and transport bookkeeping from commits. Orphan adapter staging files are not unexplained catalog edits.

## Common outcomes

Separate three concerns in results rather than overloading a single success boolean:

| Concern | Examples |
| --- | --- |
| Domain result | changes, no-change, superseded, conflict |
| Local save | persisted, committed, failed, repair-required |
| Sharing | local-only, pending, confirmed, blocked, failed, uncertain |

These are semantic distinctions, not prescribed class names. Return the operation ID and relevant revision references. Preserve origin host/session attribution where available so a conflict can be routed to the relevant contributor. Attribution is not authentication or authority.

For `none`, apply the operation within the store transaction and report local-only persistence. The coordinator supplies a stable, locally persisted ownership-period token without requiring Git history. Do not call Git, require a remote, run timing gates or invent pending publication. Cross-host handover is unsupported, not a successful no-op. A local persistence error must not be presented as proof of rollback; report whether the resulting state could be verified.

For `git`, a successful effective save requires a verified local commit. Publication can remain pending or fail separately. A no-change result needs no commit.

## Minimal local save protection

Do not implement automatic interrupted-save recovery. Detect the condition, preserve evidence and stop further catalog writes and publication until it is repaired.

Backend-private state contains a durable save marker. It records a version, the original operation, origin when known and the starting local commit. This is an in-flight marker, not another queue of record copies. Local commits hold completed pending operations.

The save sequence is:

1. Acquire the store's existing transaction scope. Check for incomplete transport state before reading the working files as accepted catalog state.
2. Refuse writes if a save marker remains, the managed index or working tree is unexpectedly dirty, or unsupported untracked content could be included. Ignore only explicitly recognised local machinery.
3. Reconcile against the local snapshot using coordinator-supplied effective authority. For no-change, conflict or supersession, return without creating a save marker or commit.
4. Persist the marker before the first record write. Retain the original operation and starting commit, not just its requested final text.
5. Save the proposed record through the adapter, stage only the intended managed record change, and create the operation commit under the same lock.
6. Verify the resulting commit's parent, operation envelope and changed records. Clear the marker only after that confirmation, then return local-save success.

If a step fails before any persistent change, it can report a clean failure. If a write may have happened, preserve the marker, working files, index and commits and report repair-required. A crash after a successful commit but before marker cleanup may conservatively require inspection. This false positive is acceptable; silent overwrite is not.

Diagnostic reads may show the last clean committed snapshot and describe the incomplete working state separately. Do not silently present an uncommitted file as a successful saved update. Existing work in project checkouts need not stop, but automatic effects must not derive authority from uncertain local owner edits.

Manual repair is sufficient for this slice. No recovery wizard, automatic reset, rollback or roll-forward is required. Clearing a marker is not proof of repair: before resuming writes, verify a clean managed checkout, valid record state and supported operation history. Document how an operator can inspect the starting commit, original operation, current files and any resulting commit without discarding them.

## Operation history

Use a versioned JSON envelope in each operation commit message, with a readable subject. The envelope carries the original domain operation and optional origin attribution. The domain record codec supplies record encoding; sync must round-trip every operation input, including its ownership precondition.

Git commit hashes may change when unpublished operations are replayed. Operation IDs and original payloads do not. Durable local receipt fingerprints preserve retry protection after operations are satisfied or superseded and leave active history; local-only saves need that protection too. These are accepted-intent receipts, not another queue of record copies. The same ID with a different operation payload is corruption or unsupported manual modification, not a retry. A simple catalog-initialisation commit is permitted separately from operation commits; it must not invent prior domain actions or authority transfers.

Validate the configured history and catalog before publishing. Operation envelopes must explain their commits' materialized changes when reconciled against their parents; a valid-looking record alone is insufficient. Unsupported envelopes, invalid records, unexpected tracked transport files and unexpected replacement of confirmed remote history stop sync. A normal user-editable Git repository is not the intended input. Manual changes need explicit repair or adoption through validated operations, not an automatic `git add --all`.

## Event-driven timing

Initial defaults are M = 5 minutes and N = 60 minutes, configurable. Persist gates once per host/catalog instance so several sessions and process restarts share them.

- a session or host-record change commits locally and checks N since the last successful push
- if unpublished stream changes exist, the same trigger checks M since the last successful full sync instead
- a stream change commits and checks M
- start or attach checks M
- shutdown, explicit sync and a validated handover request attempt sync regardless of M/N
- a missing successful baseline is immediately eligible

A stream-pending flag must describe the whole active unpublished history, not just the latest commit. Later session commits do not clear it or restart the clock. Full sync publishes pending changes together, so a stream change can bring session commits forward before N.

Complete an event's related local saves before checking its gate. A running session normally creates a new stream and its first session association in that order, then triggers the timing check once. Both records are available locally immediately, without a special first-publication requirement. Each save remains a separate commit: if the second fails, report the successfully created stream and the failed association rather than pretending the pair rolled back.

A shared application coordinator accepts publication events after related saves, including no-change updates. CLI handlers and integrations do not keep separate timers.

List/show reads use N since the last successful fetch, with an explicit blocking refresh override. A successful write-sync fetch updates that freshness clock even when its later push fails. Reads do not push, adopt live records or reset publication timing. They replay pending intent onto confirmed state for display; a conflict keeps the complete local view and exposes the confirmed alternative and diagnostics. Fetch failures return cached state and diagnostic detail under a separate shared cooldown. Invalid history and repair-required state still fail closed.

A fetch-only inspection records freshness separately and does not reset the publication gate. A failed attempt does not advance either successful timestamp. A successful full sync with no push advances only the full-sync time. When a lost push response is later resolved, record its confirmation once as the scheduling baseline. Rechecking that same confirmed attempt while later operations conflict must not repeatedly advance it. Do not invent a remote acceptance time or update progress-observation times.

Keep retries small and bounded. Use timed Git calls, at most three competing-push attempts in one pass, and a shared short cooldown after failed automatic attempts. Exact timeout and cooldown defaults may be chosen during implementation and documented; they must not create an unbounded loop or one retry storm per session. Explicit requests may bypass the cooldown, but not repair-required or unresolved in-flight-publication checks.

No timer service runs when Streams has no callers. Session integrations provide events; the sync engine does not import Pi or a frontend. M/N are eligibility thresholds, not maximum delivery latency.

## Publication pass

Hold local coordination throughout the first implementation's bounded pass. Do not run model inference under this lock. Build and validate candidates outside the live catalog working tree, while keeping the original local commits reachable for recovery.

The pass does the following:

1. Inspect incomplete save or publication state. A save requiring repair blocks the pass. An uncertain push must be reconciled before making a new attempt.
2. Fetch the configured remote branch and retain that confirmed snapshot separately from local pending edits.
3. Find the pending operations in local causal order. Remove already-published IDs from replay only after checking matching payloads in supported remote history. Current field equality is not proof that a specific operation was published.
4. Materialize a candidate from the fetched catalog. Reconcile pending operations in order, preserving the original inputs. Seed candidate authority from confirmed history, then account for accepted stream creations so their following session records can replay. When domain handover is supported, it likewise supplies prospective authority for validating the candidate, not for granting live authority before publication. Never use Git's text merge as the decision rule.
5. A changed result produces a candidate operation commit. A no-change result can be recorded as satisfied without another change commit; this is not confirmation that this operation ID was published. Superseded automatic operations leave the active queue, with their original commits retained for diagnosis.
6. A genuine conflict aborts the outgoing batch. Preserve the original local history and return affected records, operation IDs and available origin attribution. Do not publish a successful prefix or unrelated changes.
7. Validate the whole candidate. Before pushing, persist a publication marker containing the remote/branch identity, fetched base, candidate commit, original local tip and attempted operation IDs. Protect referenced local commits from loss during the attempt.
8. Push without force. A definite branch-tip race permits another fetch and reconciliation within the retry budget. Other failures return a clear failed or uncertain publication result.
9. Confirm publication, then adopt the resulting local history. Keep the publication marker through adoption. If adoption is interrupted, report repair-required and preserve both histories rather than reset automatically.

Remote inspection may still discover a changed owner when publication is blocked. Keep the confirmed snapshot available to the domain coordinator; a conflicting local stream file must not hide revocation of the former owner's authority.

On a pass where every operation is already satisfied or superseded, a push may be unnecessary. Adopting the confirmed state must still preserve the original local history for diagnosis and must not leave retired operations in an endless retry queue.

## Conflicts and manual resolution

The first version blocks the whole outgoing push. It does not block independent edits in project checkouts or require the session that triggered sync to resolve the conflict. Clean, committed local catalog edits may continue while publication is domain-blocked; a repair-required working tree is different and blocks those writes too.

Resolution is user-directed. A user can keep the remote decision, replace a pending operation with a new intent, or ask the relevant session to reconcile it. Retain the old local commits. Do not mutate the original operation's meaning under its old ID, force-push, or silently drop dependent edits.

An explicit repair can reconstruct the unpublished history from the fetched base with the user's retained or replacement operations. Reconcile remaining dependent operations again; they may surface another conflict. Resume publication only after the resulting pending history is clean, supported and domain-valid. A dedicated resolution UI or command is not required in this slice, but provide actionable diagnostic information for an operator or agent to perform the repair.

## Uncertain publication and ownership

The publication marker is written before invoking push, including when the process might die before receiving its result. The next sync first fetches and checks the recorded attempt:

- if the candidate is in remote ancestry, publication is confirmed
- if matching operation identities are already published, do not repeat their domain effects; use the newest confirmed state
- if the remote is still at the recorded base and no later local saves exist, retry the same candidate rather than create new operation identities
- if the remote advanced or local saves followed the attempt, reconcile remaining operations against the fetched base before making another candidate
- if the remote cannot be inspected, retain uncertainty and grant no handover authority; valid local creation authority remains usable

An uncertain push does not block further clean local commits. Keep the attempted tip separate from the current local tip. After checking the remote, preserve and replay any later local operations too; adopting the old candidate must never discard those saves. A new attempt covers the remaining batch and retains the original operation identities. An earlier attempt may already have published its batch, but that does not permit publishing a new conflicting batch partially.

Normal push coordination prevents competing late attempts from replacing an already accepted different history. Remote force-pushes or history replacement are outside the contract and require intervention.

A confirmed handover is not a distributed process lock. It neither stops nor proves the death of an unreachable old session. A later handover may also supersede a confirmed operation, so effective authority comes from the newest confirmed ownership state, not merely proof that an older transfer happened.

Generic foundation operations deliberately cannot hand over ownership or reattach sessions. The lifecycle coordinator must validate those intents and expected previous authority. Sync supplies forced publication and confirmation, never a bypass through raw `save`. Until that domain path exists, report such requests as unsupported rather than pretend that an owner-field patch implements handover.

Use the initial stream-creation operation ID or latest validated handover operation ID as the opaque ownership-period token. Derive it from the recorded ownership chain, not owner-field equality or the locally newest operation. Each new ownership period has a distinct operation ID, including return to a previous host. Handover validation compares the full expected previous Authority: host and token. Reattachment under the new token remains explicit.

Initial creation is local-first. For a fresh stream identity created by this host, its recorded creation establishes local authority immediately and allows the creating session's association to be saved before publication. Track this creation provenance separately from a pending transfer of an existing stream. When syncing, the creation must reconcile successfully before its dependent session operations; a conflicting remote identity blocks the batch. Do not reinterpret a failed creation as permission to take over an existing stream.

For existing shared streams, pending owner edits cannot replace confirmed authority. For `none`, an equivalent locally persisted token suffices, and cross-host handover remains unsupported.

## Acceptance scenarios

Use temporary directories and local bare Git remotes. Do not depend on GitHub, real catalog records, a running agent or a terminal frontend.

1. Local-only saves work without Git installed or any configured remote and report local-only state.
2. Git saves create one operation commit under the store lock; no-change saves create none. Two local callers cannot interleave save/index/commit work.
3. Lock, staging and transport state never enter commits. Physical IDs and filenames use the adapter's canonical mapping.
4. A write or commit interruption leaves visible evidence and blocks subsequent writes/pushes. No automatic completion, reset or overwrite occurs, including after a commit succeeded but cleanup did not.
5. A session-only update obeys N. A later session update publishes an older pending stream edit under M. Gates survive restart and are shared by local sessions.
6. Start, shutdown, explicit sync and forced publication follow their rules. Failed attempts and fetch-only reads do not falsely reset publication timing.
7. Two clones independently update permitted fields or producer-owned files. A push race replays safely without losing either change or changing operation identity.
8. A genuine conflict blocks every outgoing change in that pass. Local original commits remain available, and origin diagnostics do not assign resolution to the triggering session.
9. A superseded automatic update does not retry forever. Previously published operations are not applied again after later user edits changed their fields back.
10. Simulate a successful remote update with a lost response, process restart and additional local saves. Confirm the recorded attempt without duplicate operations, lost later saves or invented publication timestamps.
11. A running session can create a new stream and attach locally before the first push. Timing is checked after those saves. The creation and association replay in order; a colliding remote identity blocks both rather than claiming the existing stream.
12. A remote race or ownership change invalidates old preconditions. Pending or uncertain handover never grants authority; unsupported domain handover cannot be smuggled through generic operations.
13. A failed local adoption after remote success is reported as published but locally repair-required, preserving both histories.
14. Invalid records, changed operation payloads under the same ID, tracked local lock files and unexpected remote history replacement fail visibly.
15. Reads fetch at N or explicit refresh, never push and preserve pending local intent. Successful write fetches count for read freshness, even before a failed push. Read refresh and its failure cooldown do not postpone publication. Concurrent due readers share one fetch; cached conflict and failure diagnostics remain visible.

## Implementation order and handoff constraints

Build the common outcomes, local-only coordinator and pure timing policy first. Next add operation encoding and protected local Git saves. Then add candidate reconciliation, publication and uncertain-result handling. Test each slice before adding CLI or session hooks.

The current test entry point is `mise run test:python` (`uv run python -m unittest discover tests/python`); read the current project configuration before running it. Tests for Git can use the executable against temporary repositories without making the Streams source checkout a Git test fixture.

Implementation may add narrow file-adapter capabilities or operation codecs needed for composition. It must not redefine lifecycle permission, introduce a session counter into domain YAML, infer authority from pending owner edits or collapse save/publication outcomes. Domain handover is a separate follow-on; do not implement it inside transport.

No legacy-format compatibility or automatic migration is required. Preserve old notes; only switch the installed CLI, Pi extension and Work Cards when their catalog replacements are ready and explicitly authorised. Future SQL/ticket backends, partial publication, automated repair and user-interface design are outside this slice.
