# Workspace implementation plan

Status: technical contract reviewed and approved by Astra; staged implementation in progress. The user has authorized documentation, implementation with Sol and Astra, verification and local commits. [The specification](workspace-context.md) records the agreed product behavior. Do not push or migrate live user data during implementation.

## Architecture choices

Use the existing package, storage directory and host dependencies. Add workspace/agent records without treating the legacy seven-field card as the new shared model. Keep legacy data readable and provide an explicit import path; do not silently overwrite legacy YAML.

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

Runtime identity, membership and directories are not helper-editable. Automatic title changes respect manual mode. Keep the current status enum and text/control safety rules. The new structured records no longer need a pipe-delimited state carrier; retain that recognizer for legacy import/display only.

Checkout registration canonicalizes existing directories and reuses an existing entry for the same directory. Discover the nearest `.git` file/directory or `.jj` directory above the session cwd; outside a repository, use the cwd itself. Preserve the actual session cwd separately. Distinct worktrees remain distinct. Do not run global VCS configuration commands.

Checkout keys are stable once assigned. Generate fresh opaque keys rather than recycling removed keys. Do not silently relocate a registered checkout. Explicit reference edits can target another registered checkout. Refuse removal of a checkout used by visible references, removed-reference exclusions or agent records.

## Persistence operations

Retain no-follow regular-file reads, strict schemas, raw-byte revisions, exclusive cooperating-writer locks and atomic replacement. Reuse existing primitives where doing so does not weaken legacy guards. Do not build a generic database or transaction framework.

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

An explicit workspace open may select a durable record for that Herdr workspace. Automatic joining uses its verified binding. An unbound destination gets fresh shared context, not an implicit copy of the moving agent's previous workspace. Reopening old context remains explicit when no reliable native binding survives. Document this recovery boundary.

## Pi lifecycle and scheduling

Retain the existing single-flight scheduler, normalized projected evidence, uncapped agent dialogue input and cancellation guards. Add a serialized membership reconciler; do not make the scheduler a general lifecycle manager.

Automatic startup runs after meaningful task context and is idempotent. Preserve valid legacy bindings until explicit conversion. Persist a display-open attempt and nonce before launching. Reload may recover an exactly verified existing pair, but absence after an attempt never permits another automatic launch. Explicit open starts a new attempt. Move replacement requires a positively verified live old display. A closed display does not stop summaries. This does not require the standalone card to edit Pi session history.

A move changes membership, cancels old-generation work and clears pending shared contributions. The agent's own summary remains until new evidence changes it. A resumed UUID finds its own record; a new fork UUID cannot claim the parent's record.

Persist a constant-size boundary: `{compactionDigest, transcriptLength, prefixDigest}` plus an evidence-generation token. A shared contributor receives only the current normalized transcript suffix when the saved compaction and prefix digests match. Never pass the compaction summary itself as shared contribution evidence. On mismatch, reset the boundary to the current projection and contribute no historical text until new messages arrive. Own-agent summaries still receive the full allowed projection.

Reset the boundary unconditionally on tree navigation and on resume/switch when continuity is unproven, even if digests match. An old branch may share the exact prefix while its suffix predates membership. Every reset changes evidence generation and invalidates pending shared work. Commit move membership and boundary together. Initial automatic creation uses the session-start baseline so inherited fork history is excluded but the first fresh task is eligible. A move uses its observed baseline; conservative omission of pre-detection text is preferable to copying old context.

Default automatic versus manual startup/joining is one global configuration choice. Existing attached summaries continue in manual mode. Observing a move in manual mode suspends source-workspace effects; destination joining waits for explicit open/join. Explicit refresh remains available outside Herdr for an existing valid binding; inability to verify a new native group must not cause a guessed rebind.

## Summarization and plan sources

Keep models without executable tools. Produce separately validated own-agent updates and shared proposals, with narrower evidence for shared additions. Avoid shared-goal rewrites on ordinary agent activity.

Initial plan selection is automatic among the workspace's known document references; do not add a recursive repository crawler. Foreground agents can add a plan explicitly, and legacy import seeds existing references. No known plan means no invented workspace goal.

Read candidate Markdown plan text through the validated checkout resolver, at most 256 KiB per file. Fail visibly for oversized or unreadable selected sources rather than silently presenting a partial plan as complete. Refresh on explicit refresh, changed document references, and source checks during normal scheduled work; no new filesystem watcher is needed. Source fingerprints include identities and bytes. Re-read the selected identities and bytes before committing a result. An unchanged previously captured fingerprint does not prove that files stayed unchanged during inference.

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

Retain new/open/show/refresh/detach behavior where compatible, with workspace selection and legacy import made explicit. Add `/work-card remove <substring>` with the specified unique path-or-URL match, no picker and conflict checking.

Keep `work_card` read-only and return both records plus resolved shared references. Add a narrowly scoped management tool for workspace creation/title updates, own-agent updates, checkout registration and shared-reference CRUD. Runtime identity fields and plan-derived goal are not arbitrary editable patch fields.

Foreground reference removal executes directly. Deletion is metadata-only. Refuse workspace deletion while any agent record references it, including the caller; explicit own-agent deletion can precede workspace deletion. Retain the selected workspace binding so that second action remains possible after deleting the own-agent record. Pause updates before fallible cleanup and preserve cleanup identity for retry. Never close the native Herdr workspace or source files as a side effect.

## Legacy conversion

An explicit open of a legacy card imports it without overwriting the source. Preserve title, checkout and references. Keep the current session as the owning agent and copy supported agent state conservatively. Recompute the shared goal from plan sources instead of trusting old prose as plan provenance.

Record import source identity/revision and destination identity before fallible multi-record work so retries recover the same destination. Refuse conflicting imports rather than overwriting changed records. Keep a native workspace in legacy mode while its verified legacy owner remains active: no new shared publisher may compete with its workspace-summary writes. Validate imported records and complete verified legacy cleanup before switching bindings. A foreign live legacy owner requires explicit resolution, not automatic takeover. Keep new automatic naming inactive until the old-extension switchover.

## Implementation slices

1. Records and pure presentation: new workspace/agent storage, checkout identities, additive references, removal exclusions, labels and view composition. Tests prove authority and filesystem safety before wiring live behavior.
2. Herdr adapter and pane: per-agent ownership, resolved document targets, editor names, native reconciliation and independent sidebar projections. Retain verified legacy cleanup for import.
3. Pi integration: session records, automatic/manual startup, moves, evidence boundaries, shared sources, title updates, commands and management tools.
4. Migration and rollout: explicit import, partial-operation recovery, real package loader tests, isolated native multi-agent/move checks and reversible naming-extension switchover.

Use Sol for implementation and Astra for substantive review. Inspect actual diffs, fix findings, rerun checks and make focused local commits. A review finding that changes product behavior returns to the user; implementation details can be corrected within this contract.

## Native integration evidence

An isolated Herdr 0.9.4 server passed 7 checks. Cross-workspace pane moves changed public pane IDs but preserved terminal IDs and pane metadata. Workspace metadata stayed in the source. Moving the agent pane alone left its old display identifiable. Moving both terminals into the destination restored a paired layout. Pane swaps changed the first layout pane, and inherited caller context resolved the moved terminal without GUI focus.

Artifacts: `/private/tmp/wc-move-contract.xqm49W/{probe.mjs,evidence.json,calls.json,result.json}`. The harness used private HOME, XDG directories, configuration and socket; it closed only its own workspaces and stopped its server cleanly. This proves native primitives, not automatic Work Cards reconciliation, naming preservation or restart recovery. The public schema exposes cross-workspace pane movement; its `tab.move` operation only reorders tabs, so no atomic cross-workspace whole-tab operation was tested.

## Verification gates

Run `npm run check` and `npm test` after each atomic implementation slice. Keep legacy core safety coverage and update lifecycle tests for intentional contract changes rather than leaving a test-only legacy runtime. Final verification runs on Node 22 and Node 26.

Gate native rollout on isolated checks for two agents in one workspace, cross-workspace tab moves, first-pane naming, manual-name preservation, duplicate-basename documents, close-without-respawn, explicit removals and package entrypoints. Do not change normal Herdr resources to run these checks.

Verify the specified boundary representation and surface ownership provenance before wiring their runtime effects. If native behavior cannot support the documented grouping safely, stop with evidence rather than introduce GUI-focus fallbacks or silent copying.
