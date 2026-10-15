# Workspace implementation plan

Status: technical contract reviewed and approved by Astra; staged implementation in progress. The user has authorized documentation, implementation with Sol and Astra, verification and local commits. [The specification](workspace-context.md) records the agreed product behavior. Do not push or migrate live user data during implementation.

## Architecture choices

Use the existing package, storage directory and host dependencies. Replace the seven-field card runtime with workspace and agent records. The user approved a one-off local conversion with backups: do not retain a legacy launch mode, compatibility runtime or permanent importer.

Use polling to reconcile live Herdr identity, including while Pi is idle. Reuse `pane_poll_interval_ms` and serialize reconciliation so slow native calls cannot overlap. This avoids adding a raw-socket subscription client and changing the minimum Herdr version solely for events. Polling also handles reconnects through authoritative reads. Move detection is eventual, not instantaneous.

The existing `herdr api snapshot` surface provides workspace, tab, pane and layout inventories. Capture the caller's terminal identity from a verified initial pane. Subsequent reconciliation follows that terminal in the authoritative inventory, not a stale `HERDR_PANE_ID`. Cross-workspace moves can change public pane IDs while preserving terminal identity. Missing or ambiguous identity suspends effects; never fall back to GUI focus.

Pi provides session ID and optional session-file APIs, but no Herdr-movement event. An isolated Herdr 0.9.4 probe verified terminal tracking, metadata movement and layout order; reconnect and full Work Cards lifecycle behavior still require verification.

## Durable contracts

Proposed new files live under the existing card directory:

- `workspaces/<opaque-id>.yaml`
- `agents/<sha256-session-id>.yaml`

Use a hash for agent filenames because Pi permits custom session IDs. Keep the actual ID in the record. Display titles never determine these identities. Legacy `<slug>.yaml` files remain separate.

Workspace fields:

- schema version and opaque ID
- title and automatic/manual title mode
- goal, selected plan-source identities and source fingerprint
- checkout table: stable local keys with canonical directory paths
- document references: checkout key, relative path and description
- issue and PR references: URL and description
- removed-reference exclusions, keyed by document identity or URL
- a creation token used to distinguish recovery of a reserved allocation from an unrelated existing record

Agent fields:

- schema version and workspace ID
- Pi session ID and optional session-file path
- membership token, changed on each workspace transition
- reference evidence boundary: compaction digest, transcript length, prefix digest and evidence-generation token
- persisted display-open attempt and nonce, initially absent
- checkout key and actual working directory
- title and automatic/manual title mode
- status, Now and Remaining

Runtime identity, membership and directories are not helper-editable. Automatic title changes respect manual mode. Keep the current status enum and text/control safety rules. The new structured records do not use a pipe-delimited state carrier. Parse existing state only in the one-off conversion, not in the installed runtime.

Checkout registration canonicalizes existing directories and reuses an existing entry for the same directory. Discover the nearest `.git` file/directory or `.jj` directory above the session cwd; outside a repository, use the cwd itself. Preserve the actual session cwd separately. Distinct worktrees remain distinct. Do not run global VCS configuration commands.

Checkout keys are stable once assigned. Generate fresh opaque keys rather than recycling removed keys. Do not silently relocate a registered checkout. Explicit reference edits can target another registered checkout. Refuse removal of a checkout used by visible references, removed-reference exclusions or agent records.

## Persistence operations

Retain no-follow regular-file reads, strict schemas, raw-byte revisions, exclusive cooperating-writer locks and atomic replacement. Reuse existing primitives where doing so preserves these guards. Do not build a generic database or transaction framework.

Keep the current hybrid concurrency policy: model reads and inference run outside write locks; proposals name specific fields or reference additions. Reference additions merge against current data. Other edits retain whole-record revision checks. Do not add general field-by-field conflict merging without evidence that rejected independent edits are a problem. The extension serializes the final read, check and write; rewriting YAML internally does not grant a helper whole-record editing authority.

A shared mutation must validate the calling agent's workspace and membership token as well as the workspace revision or merge preconditions. Serialize membership-topology changes, checkout removal and record deletion with one store-local topology lock. Acquire affected record locks in lexical path order beneath it, and hold protection through the agent commit. Shared mutations that check membership use the same topology guard. Never acquire a native lock from inside a store transaction. The runtime also checks live Herdr context before effects; filesystem locks do not freeze Herdr movement.

For joining an unbound destination:

1. Under the existing socket/workspace lock, re-read native identity and reserve one destination ID, store identity and creation token in verified metadata. Mark the reservation incomplete.
2. Under the store topology guard, create or recover the matching workspace record, register or reuse the checkout, and commit the agent's new workspace, checkout, membership token and evidence boundary together.
3. Revalidate native identity and publish a complete binding. Incomplete reservations are not ready workspaces and do not authorize publishers.

A retry recovers the ID from the reservation and verifies its creation token instead of allocating another record. Interrupted work may leave an unused record or checkout, not a dangling agent reference. If native reservation metadata is lost, use the documented explicit-recovery boundary rather than guessing. Test interruption after each native publication and file write.

Shared helper operations are additions, not replacement arrays. Merge against current data and enforce the removal exclusions. Foreground removal adds an exclusion in the same workspace write that removes the visible reference. An explicit foreground re-add clears the exclusion. Do not rely on a prompt to stop old conversation evidence from restoring a removed link.

Reference identity is checkout key plus relative path for documents and exact stored URL for links. Use canonical checkout/file identity for opening and editor reuse. Do not resolve an existing file merely to remove its reference; broken references must remain removable.

## Rendering and document navigation

Build a read-only view from workspace and agent snapshots. It contains the shared goal, the agent's own status/Now/Remaining, and shared references with explicit checkout targets. Never reconstruct document targets from display labels.

Compute shortest distinguishing checkout-directory suffixes from the workspace checkout table. With one checkout, retain short document labels. With several, prefix paths. Test collisions, directory-prefix edge cases, worktrees and aliases. Do not let truncation change key-to-target mappings.

Use the same qualification policy for automatic editor-tab names. Surface-scoped provenance contains verified surface identity, owner/nonce, automatic/manual mode and the last successfully applied automatic name. Rename only when the current name still equals that last-applied name and ownership remains valid; an external difference relinquishes automatic naming. Keep provenance with the native surface, not only the current first agent. Reuse preserves manual titles and editor state. Revalidate the document and caller before opening or focusing.

Keep document letters, inert link digits, HTTP terminal links, Unicode cell bounds, notice layout and `[q] close` behavior. Keep the optional refresh spinner off by default.

## Per-agent Herdr ownership

Workspace metadata identifies the shared workspace record, not a single card pane. Agent/card pairing uses exact terminal identities, agent-record identity and a nonce. Keep marker values within Herdr token limits. Card identity must include its state-directory location, not just a session hash that could collide across separate stores.

Create card splits relative to the verified Pi pane. Several agent/card pairs may coexist in separate tabs of one workspace. No agent may adopt or close another agent's card.

Following a verified move may refresh public pane/tab IDs for the same owned terminals and nonce. It must not recover an unrelated or restored process through matching labels. Whole-tab moves should preserve the pair. Define and test the case where only the Pi pane moves: a verified owned old display may be replaced, but an absent display must not be treated as permission to respawn one closed with `q`.

Publish per-agent summaries on the corresponding agent surface; do not let agents overwrite one workspace summary with their individual Now values. The workspace projection represents shared context. Keep lifecycle state separate from display metadata and do not take over Herdr's Pi agent-state integration.

Verify document-navigation authority independently of metadata publishing. A publishing failure must not discard a document key after fresh display verification. Skip unchanged projections before acquiring a write lock; revalidate under the lock before any changed publication. Treat absent native text tokens as empty text when comparing projections. Document opens may wait up to `herdr_timeout_ms` to acquire the native lock. Retry only lock acquisition, never an operation that has entered its callback or may have opened an editor. Cancellation stops the wait; stale locks are never stolen.

An explicit workspace open may select a durable record for that Herdr workspace. Automatic joining uses its verified binding. An unbound destination gets fresh shared context, not an implicit copy of the moving agent's previous workspace. Reopening old context remains explicit when no reliable native binding survives. Document this recovery boundary.

## Pi lifecycle and scheduling

Retain the existing single-flight scheduler, normalized projected evidence, uncapped agent dialogue input and cancellation guards. Add a serialized membership reconciler; do not make the scheduler a general lifecycle manager.

Pi's before-switch, before-fork and before-tree events are cancellable. They may invalidate requests, but must not retire a display before the transition succeeds. A successful replacement emits shutdown before disposing the old session; reload must retain the live pair. Stop idle timers on shutdown and bind subsequent work to fresh SDK context. Cancellation can leave the old session active without a completion event; recover only from a fresh confirmed context, never by reviving a disposed one.

Automatic startup runs after meaningful task context and is idempotent. After the one-off cutover, restore only workspace and agent records; ignore old session binding entries. Persist a display-open attempt and nonce before launching. Reload may recover an exactly verified existing pair, but absence after an attempt never permits another automatic launch. Explicit open starts a new attempt. Move replacement requires a positively verified live old display. Use a guarded separated-display replacement operation that holds the native lock through verification, cleanup and one replacement attempt. Ordinary automatic open remains suppressed by a persisted attempt, including after standalone cleanup. Cleanup or launch uncertainty never authorizes an automatic retry. A closed display does not stop summaries. This does not require the standalone card to edit Pi session history.

A move changes membership, cancels old-generation work and clears pending shared contributions. The agent's own summary remains until new evidence changes it. A resumed UUID finds its own record; a new fork UUID cannot claim the parent's record.

Persist a constant-size boundary: `{compactionDigest, transcriptLength, prefixDigest}` plus an evidence-generation token. A shared contributor receives only the current normalized transcript suffix when the saved compaction and prefix digests match. Never pass the compaction summary itself as shared contribution evidence. On mismatch, reset the boundary to the current projection and contribute no historical text until new messages arrive. Own-agent summaries still receive the full allowed projection.

Reset the boundary unconditionally on tree navigation and on resume/switch when continuity is unproven, even if digests match. An old branch may share the exact prefix while its suffix predates membership. Every reset changes evidence generation and invalidates pending shared work. Commit move membership and boundary together. Initial automatic creation uses the session-start baseline so inherited fork history is excluded but the first fresh task is eligible. A move uses its observed baseline; conservative omission of pre-detection text is preferable to copying old context.

Default automatic versus manual startup/joining is one global configuration choice. Existing attached summaries continue in manual mode. Observing a move in manual mode suspends source-workspace effects; destination joining waits for explicit open/join. Explicit refresh remains available outside Herdr for an existing valid binding; inability to verify a new native group must not cause a guessed rebind.

## Summarization and plan sources

Keep models without executable tools. Produce separately validated own-agent updates and shared proposals, with narrower evidence for shared additions. Avoid shared-goal rewrites on ordinary agent activity.

Initial plan selection is automatic among the workspace's known document references; do not add a recursive repository crawler. Foreground agents can add a plan explicitly, and the one-off conversion preserves existing references. No known plan means no invented workspace goal.

Read candidate Markdown plan text through the validated checkout resolver, at most 256 KiB per file. Fail visibly for oversized or unreadable selected sources rather than silently presenting a partial plan as complete. Reselect plans when candidate reference metadata changes. Scheduled checks reread selected sources and reuse unchanged goal results. Explicit refresh reruns selection and goal inference, allowing recovery from a poor inference without editing the sources. No new filesystem watcher is needed.

Source fingerprints include identities and bytes. Re-read the selected identities and bytes before committing a result. An unchanged previously captured fingerprint does not prove that files stayed unchanged during inference.

Workspace-goal inference must not use arbitrary agent dialogue as completion evidence. Keep selected sources and provenance with the result. A plan can supply intended scope, never proof of completed agent work. Summarizer field validation enforces authority; semantic source interpretation remains model behavior and must not be advertised as a validator guarantee.

Use the same configured helper route. No billed model replays are authorized by this implementation plan; validate protocol and scheduling with doubles. Do not reuse or rerun the exhausted earlier comparison ledgers.

## Naming

Adapt the existing naming extension's session-info ownership technique instead of creating a second competing name generator. Pi reports supply session identity; model output supplies only proposed titles.

Only the current first layout pane may rename its tab. Recheck this at effect time. Session names remain per-agent, including for non-first panes. Workspace names come from the shared workspace title.

Preserve existing explicit names on adoption. Apply the surface-scoped provenance rule above to workspace, tab and editor names; automatic record title mode alone does not grant permission to overwrite a native label. Provenance stays with the surface when another pane becomes first. A manual tab name is a surface override; do not force it onto a separately manually named Pi session. Pi session-info entries identify Pi-name ownership. Preserve unproven existing nondefault names across reload and movement.

### Surface naming provenance

Herdr has pane and workspace metadata, but no tab metadata. Keep surface naming provenance in `.native-names/<hash>.json` under the store directory, separate from workspace and agent records. This is a local cache, not a registry or a source of project state.

Key each entry by canonical store identity, socket, native workspace ID, ready-binding lifetime nonce, surface kind and surface ID. Do not key it by the current first pane. Store the exact scope, last writer's owner/nonce, automatic/manual mode and last successfully applied name. Pi and editor namers share the same tab entry.

The actual first layout pane has tab rights only if it is an eligible Pi or editor pane. Do not skip an ineligible first pane. Ownership can transfer without the previous owner remaining alive, provided current authority is verified and the label still matches the last automatic name. Keep the binding nonce stable across reload and first-pane changes; change it for a new binding lifetime.

Hold the native workspace lock across the provenance check, rename and atomic cache write. Recheck context, binding, layout and current label before renaming. Use strict no-follow cache reads. Missing provenance never authorizes replacing a nondefault label; malformed provenance suspends naming. An observed label mismatch persists manual mode. If a rename succeeds but saving provenance fails, report partial progress and preserve the unproven label on retry. The lock coordinates cooperating writers, not concurrent native user actions.

Astra approved this storage approach; implementation still requires tests for owner disappearance, Pi/editor transfer, sticky manual mode and partial failures.

Disable the installed `herdr-naming.ts` only after the new path is verified. Use a reversible supported exclusion/disable mechanism; do not delete its source or alter unrelated user settings. Report when `/reload` is safe, after workers are finished.

## Commands and management

Retain new/open/show/refresh/detach behavior where compatible. Open selects workspace records, not old card slugs. New attaches the calling agent and names its assignment; it must not erase an existing shared group. Add `/work-card remove <substring>` with the specified unique path-or-URL match, no picker and conflict checking.

Keep `work_card` read-only and return both records plus resolved shared references. Add a narrowly scoped management tool for workspace creation/title updates, own-agent updates, checkout registration and shared-reference CRUD. Runtime identity fields and plan-derived goal are not arbitrary editable patch fields.

Foreground reference removal executes directly. Deletion is metadata-only. Refuse workspace deletion while any agent record references it, including the caller; explicit own-agent deletion can precede workspace deletion. Retain the selected workspace binding so that second action remains possible after deleting the own-agent record. Pause updates before fallible cleanup and preserve cleanup identity for retry. Never close the native Herdr workspace or source files as a side effect.

## One-off local cutover

A Herdr and Pi restart during implementation loaded the replacement entrypoint. It created a workspace record, an agent record and a verified native pair for the current session. Do not create another workspace or replace newer agent progress with the old saved state.

The original card still contains its title and 3 document references. A second, empty card points to a temporary test checkout; do not import it as project context. After review and verification, back up both old files, the current records and native metadata. Merge the original title and references into the existing workspace through checked core operations. Leave session history untouched and keep Goal unknown until selected plans establish it. Retain source backups for recovery; do not add a runtime importer.

No native rebinding or stale-marker cleanup is currently needed. The display is now absent; do not reopen it during conversion. Reverify the owning session, workspace binding and pair markers before merging data. Refuse changed identity or ownership. Leave other panes, editor tabs and unrelated metadata alone. Use one launcher environment and one pane runtime. Test the exact environment produced by the launcher against the pane, rather than testing each side with separate fixtures.

## Implementation slices

1. Records and pure presentation: new workspace/agent storage, checkout identities, additive references, removal exclusions, labels and view composition. Tests prove authority and filesystem safety before wiring live behavior.
2. Herdr adapter and pane: per-agent ownership, resolved document targets, editor names, native reconciliation and independent sidebar projections. Remove the single-card launcher and pane branch at cutover.
3. Pi integration: session records, automatic/manual startup, moves, evidence boundaries, shared sources, title updates, commands and management tools.
4. Cutover and rollout: backed-up local conversion, real package loader tests, isolated native multi-agent/move checks and reversible naming-extension switchover.

Use Sol for implementation and Astra for substantive review. Inspect actual diffs, fix findings, rerun checks and make focused local commits. A review finding that changes product behavior returns to the user; implementation details can be corrected within this contract.

## Pi integration status

`src/pi/index.ts` now registers the workspace lifecycle, helper coordinator, commands and read-only tool. Startup captures the SDK projection; idle polling reconciles membership without scheduling model calls. Explicit refresh carries forced plan-selection intent through coalescing. Old session binding entries are not imported.

Astra approved the runtime slice, committed as `b2530f0d`, with 526 tests passing on Node 22 and Node 26, including the exact launcher environment consumed by the pane. Review corrections cover:

- competing native groups selecting one durable workspace
- detach persistence before fallible reads or cleanup
- cancelled providers that settle late, without overlapping requests
- same-UUID resume through a copied or symlinked session file
- detach superseding older queued open/new commands, including cleanup failure and disk-backed reload
- failed initial configuration, tested through awaited SDK startup and public command dispatch

If initial configuration prevents startup, fix the configuration and run `/reload` before retrying commands. Read-only access remains available, and detach still persists its pause immediately. Configuration changes after successful startup retain the existing recovery path.

The legacy runtime is removed: old card/state modules, native client/ownership, pane fallback, prose renderer, summary protocol and obsolete configuration fields. The shared lock and transport remain. `work-cards/records` replaces the old public card API. Astra approved the removal; 377 tests pass on Node 22.23.3 and 26.11.1. The lower count retires old-format and duplicate cases while retaining current safety coverage, including successful-looking editor metadata acknowledgements that do not persist. An actual tarball excludes the removed source and generated modules.

The workspace adapter now applies `pane_height_fraction` to newly created pairs after fresh authority checks. Below 0.5, it grows the agent downward; above 0.5, it grows the card upward. Always resizing the agent selected an unrelated outer separator in nested layouts. Astra caught that error; the corrected implementation passed review and all 406 tests on Node 22.23.3 and 26.11.1. Reuse preserves geometry, and sizing failures retain the pair and persisted attempt.

The restart activated the new source entrypoint before review finished. The active Pi instance still needs reloading after source edits finish. The isolated native check now passes; next, merge the saved references with backups.

Foreground management and native/session naming remain unfinished. The installed naming extension has not been disabled.

## Native integration evidence

An isolated Herdr 0.9.4 server passed 7 checks. Cross-workspace pane moves changed public pane IDs but preserved terminal IDs and pane metadata. Workspace metadata stayed in the source. Moving the agent pane alone left its old display identifiable. Moving both terminals into the destination restored a paired layout. Pane swaps changed the first layout pane, and inherited caller context resolved the moved terminal without GUI focus.

Artifacts: `/private/tmp/wc-move-contract.xqm49W/{probe.mjs,evidence.json,calls.json,result.json}`. The harness used private HOME, XDG directories, configuration and socket; it closed only its own workspaces and stopped its server cleanly. This proves native primitives, not automatic Work Cards reconciliation, naming preservation or restart recovery. The public schema exposes cross-workspace pane movement; its `tab.move` operation only reorders tabs, so no atomic cross-workspace whole-tab operation was tested.

A later adapter and standalone-pane smoke test passed shared context, distinct agent projections, and checkout A document opening, reuse and manual-title preservation. It failed when checkout B's document shortcut was dropped during projection-lock contention. The pane discarded document keys when projection failed in that iteration, even after observing and verifying the pair. No wrong-file opening or ownership corruption was observed.

Artifacts: `/private/tmp/wc-workspace-native.b5jTSR/`, including `investigation.md` and `result.json`. All private workspaces were closed and the server exited with code 0. The run did not reach close/reopen or movement checks. After 3 harness correction retries, the user approved the targeted contention fix above and a fresh isolated verification run.

The corrected adapter and pane passed all 5 groups in `/private/tmp/wc-workspace-native.Q0pnzu/` in one run. Both checkouts opened their documents on the first keypress under the original 250 ms polling contention. Exact editor targets, reuse, manual labels, close-without-respawn, explicit reopening, agent-only move cleanup and paired recovery after 2 sequential pane moves all passed. The other agent pair and a separate sentinel pair stayed intact. The harness closed its 3 private workspaces; server PID 29938 exited with code 0. See `result.json`, `checks.jsonl` and `evidence-1.json` through `evidence-5.json`.

Astra approved the contention fix; all 439 tests passed on Node 22 and Node 26. That smoke exercised real card panes and dummy agent shells, not Pi lifecycle, automatic naming, local conversion or removal.

The recovered Pi host later passed 5 isolated native checks on Herdr 0.9.5 and Node 26.11.1: fresh-input attachment, exact document opening, close/refresh/reload suppression and explicit reopen, two independent agents, and own-only shutdown cleanup. Artifacts are in `/private/tmp/wc-runtime-cutover.kTWk8m/`. The harness used injected SDK callbacks and 5 local fake helper calls, not a complete Pi session. Its private workspace closed, server exited with code 0, socket disappeared and all captured fixture processes stopped.

This run preceded the review corrections for competing workspace selection, durable detach, late provider settlement and same-UUID resume through another file. It does not establish those edge cases or complete the naming and management rollout.

The current runtime passed 7 checks in `/private/tmp/wc-runtime-cutover.3m4nhl/` on Herdr 0.9.5 and Node 26.11.1. It repeated the 5 host/pane checks and added nested 20% and 75% card fractions. Both preserved the outer separator, unrelated pane geometry and focus. The harness used injected SDK callbacks, real card/editor processes and 6 local fake helper calls. All 22 source/compiled module hashes stayed unchanged during the run. Its workspace closed, server exited with code 0, socket disappeared and all 11 captured fixture processes stopped; see `cleanup-audit.json`.

The preceding attempt in `/private/tmp/wc-runtime-cutover.zAwyCi/` stopped at check 4 after a visible initial lock-acquisition refusal. The harness incorrectly waited for a helper that had not been scheduled. The reviewed correction retries only that refusal, with unchanged record revisions and no helper request. It also waits for an actual helper call after explicit open, not merely an inactive spinner. That failed run's 7 captured processes and private server were cleaned up; it did not reach the nested checks.

## Verification gates

Run `npm run check` and `npm test` after each atomic implementation slice. Port applicable filesystem, cancellation, rendering and lifecycle safety coverage to the sole workspace/agent runtime. Retire old-format-only expectations explicitly rather than leaving a test-only compatibility runtime. Final verification runs on Node 22 and Node 26.

Gate native rollout on isolated checks for two agents in one workspace, cross-workspace tab moves, first-pane naming, manual-name preservation, duplicate-basename documents, close-without-respawn, explicit removals and package entrypoints. Do not change normal Herdr resources to run these checks.

Verify the specified boundary representation and surface ownership provenance before wiring their runtime effects. If native behavior cannot support the documented grouping safely, stop with evidence rather than introduce GUI-focus fallbacks or silent copying.
