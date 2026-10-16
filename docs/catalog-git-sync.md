# ADR: Streams-managed storage and optional Git sync

Status: agreed design with an initial library implementation. The [sync implementation plan](../plans/catalog-sync.md) defines its testable contracts and scope; the [library usage guide](catalog-sync-usage.md) describes its API and manual inspection. CLI replacement, handover coordination and installed-runtime cutover remain follow-on work.

## Context and decision

Move catalog management from the Obsidian-managed notes workflow to the `streams` CLI. Keep the logical model, agent integrations, frontends, physical storage and synchronisation separate. Support local-only operation with sync set to `none`; choose `git` for sharing the catalog across hosts.

The Git strategy uses a dedicated private repository with one shared branch and a managed local copy on each host. Save changes as local commits and publish them according to their urgency. The user should not need to manage commits, pulls or pushes during ordinary work. These Git mechanics are not requirements of every storage backend.

The existing Streams format combines work metadata and a Markdown scratch body under a configurable notes root. It also uses local repository paths and Obsidian note links. Merely putting that directory in Git would preserve the wrong persistence model. The [durable work catalog ADR](durable-work-catalog.md) replaces scratch notes with structured records: the stream's goal and references, and each session's progress description. There is no second stored aggregate progress summary. Substantive plans and decisions belong in project repositories.

Git makes the catalog readable, inspectable and recoverable from published history. It is also the meeting point between hosts: one host commits and pushes, and another fetches when it next needs the catalog. The design does not require live cross-host communication, an always-running service, transcript synchronisation or project-checkout backup.

## Keep components independent

Modularity is a Streams design principle, not just a way to organise the Git implementation. Separate these responsibilities:

| Component | Responsibility |
| --- | --- |
| Logical data model | Shared record meaning, identities, validation, lifecycle permissions and operation reconciliation |
| Agent integration | Adapt a supported harness's sessions and evidence to the common operations; different harnesses can contribute together |
| Frontend | Project state and provide runtime actions through adapters such as CMUX and Herdr |
| Physical storage | Encode and persist logical records, initially as files, without making YAML, JSON or paths part of their meaning |
| Synchronisation | Coordinate sharing and freshness, using an explicit strategy such as `none` or `git` |

The durable work catalog ADR owns the logical and physical data-model design. This ADR owns synchronisation and the storage guarantees its Git strategy needs. The CLI composes these components. Agent integrations, frontends and the advisor use common management operations rather than writing files or running Git themselves. A session-start or shutdown trigger is an integration event, not a Pi-specific requirement in the sync engine.

Session records describe a contributor's assignment and last-known state. Sync preserves recorded observation times and edit provenance. Editing or publishing prose does not establish that its contents were freshly verified. Logical changes are classified as stream, session or host updates before they become files. Publication policy must not depend on YAML syntax or a frontend's representation.

The domain supplies a validated operation with a stable ID, exact targets and relevant preconditions. Applying it to current records returns changes, no change, a superseded automatic update, or a structured conflict. Persistence and sync expose version and outcome information without making a Git commit hash a domain identity. A Git text merge is not evidence that two lifecycle decisions are compatible.

These are separate responsibilities, not a promise that every storage and sync combination works. The initial Git strategy works with a file-backed store. A future PostgreSQL backend could use tables and transactions for both persistence and cross-host coordination. A ticket-system backend could use a central service. Neither should have to simulate YAML files or local Git commits. Such adapters must preserve the catalog's logical operations and personal lifecycle semantics rather than silently substitute a team's ticket workflow.

Define narrow contracts for the initial combinations, not a universal backend framework. Future database and ticket adapters are extension possibilities, not implementation scope here.

### Local-only operation

With sync set to `none`, the physical store provides local persistence without Git, a remote, credentials or network activity. Ordinary saves still validate and serialize concurrent local changes, but success does not require a Git commit. Results identify the catalog as local-only, not perpetually pending or failed publication. Sync triggers do no remote work; M and N do not apply.

Local-only mode provides no cross-host visibility or ownership transfer. A cross-host handover must not report success as a no-op. Moving to a shared backend requires explicit setup and migration; it is not a fallback that activates silently when a remote becomes available.

The remaining protocol sections describe the `git` strategy, not requirements of `none` or future centralised backends.

## Give Git one managed store per host

Use a dedicated catalog repository, separate from the Streams source repository, project checkouts and the Obsidian vault. The physical storage adapter serializes the domain-defined stream, session and host records into files. Absolute checkout paths, saved-session locators, native workspace bindings and transport bookkeeping remain in host-local state.

Partition session files by host and session identity; host check-ins are also producer-owned. Different producers normally write different files. This avoids most content conflicts, but simultaneous pushes can still race on the shared branch tip.

Configure the remote, branch, local catalog location and sync intervals per host. Use normal Git authentication; do not put credentials in records or repository URLs. Hosting and credential setup remain open.

Serialize local validation, record changes and commits under the file store's existing lock. Concurrent agents must not race file replacement against a commit or share an uncoordinated Git index. The lock and the adapter's staging files are local machinery, never catalog content to publish.

The first implementation detects interrupted saves rather than repairing them automatically. Before changing a file, durably record a small save marker containing the original operation and starting commit. Clear it only after verifying the resulting commit. A leftover marker or unexplained index or working-tree change produces a repair-required result. Preserve the files and commits, allow diagnostic inspection, and block new catalog writes and pushes. Do not finish, roll back or reset an interrupted save automatically, even when a likely repair seems apparent.

A failed attempt known to have made no persistent change can report failure without leaving a repair condition. Once a write may have taken effect, do not present it as a clean failure or a successful save without confirmation. Manual repair is acceptable initially; a recovery wizard is not required.

The managed store is not a general-purpose user checkout. Files remain readable, but ordinary editing goes through Streams so it can preserve intent and validate changes. Invalid records and unsupported manual changes require visible repair; sync must not delete or automatically publish them.

Obsidian may remain useful for unrelated notes and inspecting retained material. It is no longer the catalog's editor, synchroniser or authority. Do not place the managed store under a second folder-sync mechanism or install a second automatic Git writer for it.

## Commit on changes and publish on triggers

With the Git strategy, a successful ordinary save means that Streams validated the update and committed it locally. Publication is a separate result. Unpublished commits survive restarts and network failures for later sync. A network failure does not turn a committed local save into a failed save.

Commit each effective stream or session update. The first implementation saves one logical record per operation and commit, matching the foundation's single-record persistence contract. Unchanged content produces no commit. Occasional host check-ins follow the session publication policy. Delay network publication, not the local record of a successful save.

Use two configurable intervals:

| Interval | Purpose                                                                                                   | Initial default  |
| -------- | --------------------------------------------------------------------------------------------------------- | ---------------- |
| N        | Session-only publication eligibility, measured from the last successful push                              | 60 minutes       |
| M        | Stream-change publication and attachment refresh eligibility, measured from the last successful full sync | 5 minutes        |

The trigger rules are:

| Trigger                                              | Action                                                                           |
| ---------------------------------------------------- | -------------------------------------------------------------------------------- |
| Session file or host check-in changes                | Commit; sync when N has elapsed, or when unpublished stream changes make M apply |
| Stream file changes                                  | Commit; sync when M has elapsed                                                  |
| Later session update with unpublished stream changes | Use M rather than waiting for N                                                  |
| Session starts or attaches                           | Sync when M has elapsed                                                          |
| Session shuts down                                   | Attempt sync regardless of the intervals                                         |
| Explicit sync request                                | Attempt sync regardless of the intervals                                         |
| Ownership handover                                   | Save and attempt immediate sync; activation has the separate rule below          |

A full sync fetches, reconciles and publishes pending changes together, not just the triggering file. Attachment, shutdown, stream changes and explicit requests can therefore publish session commits before N elapses.

Track the successful-sync times and unpublished stream changes once per host, not per session. The pending-stream condition persists through subsequent session commits. Do not restart the M interval on every edit or forget a pending stream change because the latest commit touched only a session file. With no successful baseline, a trigger is eligible immediately.

Complete related local saves before checking an event's publication gate. In particular, a running session normally creates its stream and session association together, then checks the normal timing rules. Each record is committed separately; this does not promise a multi-record rollback transaction. A failed later save reports the earlier successful saves rather than hiding or undoing them.

The intervals are eligibility checks at events, not daemon-enforced deadlines. Running integrations can update observations periodically and request the shared check. An unused host does nothing until its next use. Shutdown publication is best effort; abrupt termination may leave changes unpublished.

Failed attempts do not advance successful-sync timestamps. A successful full sync with nothing to publish advances its sync time, but does not invent a successful push. Fetch-only inspection has its own freshness timestamp and must not postpone publication by resetting the M gate. Bounded retries and failure backoff must prevent several local sessions from repeatedly attempting the same failing sync.

## Reconcile operations rather than overwrite files

Use this publication protocol:

1. Acquire local store coordination. Refuse publication if an interrupted save or unexplained local change needs repair.
2. Fetch the configured branch, retaining its confirmed state separately from unpublished local edits.
3. Read the original pending operations from local commit metadata. Reconcile them against the fetched state through the domain layer, preserving their IDs and preconditions.
4. Build a candidate history separately from the live local checkout. On a domain conflict, preserve the local edits and publish nothing. Otherwise validate the whole candidate catalog.
5. Durably record the candidate, its remote base and the operation identities before attempting a normal push. On a branch-tip race, fetch and reconcile again within a bounded retry budget.
6. Confirm publication and adopt the resulting local history under the same coordination. Retain original local commits until adoption is complete; an interrupted adoption requires visible repair rather than automatic reset.

Store the original operation in a versioned JSON envelope in its commit message. Include its stable ID, exact intent and preconditions, plus producer attribution when available. One effective save creates one operation commit in the initial implementation. This preserves replay information without a second queue of record copies. Record serialization remains the physical adapter's responsibility; the operation codec must preserve all domain inputs without interpreting them differently.

A replayed commit retains the operation ID and original intent even if its hash or materialized record changes. Deduplicate by operation identity and payload, not current field equality. Reusing an ID with different intent is an error. A no-change save needs no commit; reconciliation that finds existing satisfaction must distinguish that result from proving a particular earlier push succeeded.

Independent reference additions and unrelated changes may combine when the domain permits them. Competing lifecycle decisions do not become last-writer-wins updates. Git ancestry orders committed session updates; replay preserves their logical order and stable operation identities. Captured-input and base-state guards prevent stale asynchronous results from overwriting newer updates. An increment-at-save counter would not provide those guards, and sync does not require one in the domain record.

Ownership-change guards remain a separate domain contract, without prescribing a particular field representation here. A fetched session file never becomes writable merely because it is locally available.

Retain the original local commits until reconciliation and local adoption are recoverable. Superseded old-owner updates leave the active retry queue but remain available for diagnosis. Conflicting user changes remain visible and recoverable, not silently discarded or reinterpreted. Generated descriptions must still satisfy their captured-input guards when published.

A genuine domain conflict blocks the entire outgoing push. Local saves and remote inspection can continue; unrelated updates and handovers may wait. This is a publication block, not a requirement to stop the agents. Ordinary branch-tip races and domain-approved independent changes still reconcile automatically.

Conflict diagnostics identify the affected stream or record, pending operation and its origin when known. The session that triggered sync is not automatically responsible for resolving someone else's decision. Resolution requires user direction, which may be given through the relevant session or another management interface. Keeping the remote decision or replacing the local intent must explicitly retire the conflicting pending operation, preserve its original history and revalidate dependent pending operations. Neither a newer timestamp nor repeatedly retrying supplies resolution.

This initial policy favours a clear all-or-nothing publication result over partial success. Publishing independent work while leaving conflicts unresolved is a possible later extension, not part of the first implementation.

Do not force-push or rewrite published catalog history. Local unpublished commits may need rebuilding after reconciliation, so operation identity must not depend on a commit hash remaining unchanged.

## Distinguish save, publication and authority

Results distinguish local save from publication: pending, confirmed, conflicting, failed or uncertain. Include operation identity, local revision and known remote revision so callers can reconcile rather than repeat an action blindly.

A lost push response is not proof that the remote rejected the update. Before retrying, fetch and check whether the recorded candidate is in the remote history or its operation identities have already been published. If the remote still has the recorded base and there are no later local saves, retrying the same candidate is safe. Otherwise reconcile the remaining operations, including later local saves, against the fetched base before making another candidate. An uncertain push must not cause those later saves to be lost during local adoption. Only normal, non-forced pushes are allowed; a competing late attempt cannot overwrite a different accepted history.

If confirmation is unavailable, report publication as uncertain and keep the pending-push record. Do not repeat the domain action with a new operation ID. Field equality is not proof of publication: the value could have changed and changed back. Unexpected remote history replacement requires repair rather than guessing which operations to replay.

Creation and handover have different activation rules. Creating a genuinely new stream with a fresh identity establishes its creator's local ownership immediately. A running session can create that stream and attach to it before either record is pushed. This follows ordinary delayed publication; there is no publish-first gate. Derive that initial authority from the recorded creation operation, not merely a writable owner field. It is not a way to claim an existing stream or bypass a handover.

An ownership handover bypasses the timing gates and attempts publication immediately. Its local save can succeed while the handover remains pending. The destination gains authority only after publication is confirmed, and only while the confirmed ownership state still authorises it. A pending owner edit to an existing stream is not execution authority.

Retries preserve the ownership precondition captured for the request. An intervening handover is a conflict, not permission to seize ownership again, even if ownership later returns to the same host. A later successful sync may publish a still-valid pending handover, but saved intent and effective ownership remain distinct. Confirmed transfer does not prove that an unreachable old process stopped. The domain rejects updates made under superseded ownership and controls suspension of old-owner automatic effects.

Readers need both the latest fetched state and the existence of local unpublished changes. The coordinator tracks effective authority from the confirmed operation history, with local creation provenance for genuinely new streams not yet published. The initial creation operation ID, and subsequently each validated handover operation ID, can identify ownership periods without depending on rewritten commit hashes. Replaying the original creation preserves its ID; returning ownership to a previous host requires a new handover ID.

Keep pending transfers separate from that effective authority. Candidate reconciliation can evaluate prospective ownership changes without granting live execution rights. A remote identity collision or different ownership history blocks publication rather than silently claiming the existing effort. Cache freshness, local save time and publication time do not freshen session descriptions, concrete execution observations or user priority review.

## Cut over without maintaining two catalogs

No legacy-format compatibility or automatic migration is required. Existing efforts can be recreated manually through validated catalog operations. Preserve old scratch material for reference rather than importing the vault or recreating a scratch-body field. Useful rationale can be moved into project documents when reviewed.

Prepare the new store outside the Obsidian vault. For Git, configure and verify its remote publication before adopting it for cross-host work. For `none`, verify local persistence without claiming remote recovery. Do not create a repository or publish private notes merely because the software supports doing so.

Switch the CLI and integrations only when their catalog replacements are ready. Retire old catalog writers, including scratch-generating checkpoints, at that explicit cutover. Thereafter the old notes are retained material, not a second live account to keep in sync. Unrelated Obsidian notes and configuration are unaffected.

A failed setup leaves the existing notes intact. Implementation and tests use isolated stores; they do not switch the installed runtime. Returning to the old workflow after new catalog edits is an explicit recovery choice, never an automatic fallback.

## Alternatives and consequences

Object storage and a hosted service were considered, but Git was chosen for ordinary files and inspectable history. Keeping Obsidian or folder synchronisation as another writer would undermine the CLI's validation and conflict rules. Two branches could separate catalog history from periodic session updates, but the chosen single branch is simpler. Producer-partitioned files and delayed pushes address the expected contention without separate histories.

A scheduled host task was rejected in favour of session-driven sync. An unused machine need not maintain a current replica. This accepts last-known state and potentially unpublished changes until the next trigger; it is not a promise that updates appear remotely within M or N minutes.

Local commits protect against losing an agent session, not losing the machine before a push. The catalog and project repositories also have separate publication boundaries: syncing the catalog does not push source code or documents. Cross-host recovery reaches only published catalog and project state.

Session and host snapshots replace current files, but their earlier versions remain in Git history. Delayed publication reduces network operations, not the number of local commits already created. There is no automatic history truncation in this design. Report frequency and repository growth should be measured before introducing retention machinery.

## Implementation boundary

The [sync implementation plan](../plans/catalog-sync.md) records the concrete states, failure checks and acceptance scenarios for the next model. Use M = 5 minutes and N = 60 minutes as configurable initial defaults. Hosting, credential provisioning and installed-runtime cutover are deployment choices, not permission for implementation to change live services.

Automatic save repair, partial publication past a domain conflict, legacy-format compatibility and automatic migration are outside the first implementation. Record schemas, producer identities, lifecycle permissions and resume selection remain governed by the durable work catalog ADR. Handover requires that domain's validated coordination; the transport must not bypass its generic-operation restrictions.
