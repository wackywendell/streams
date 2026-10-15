# Shared workspace context and agent cards

Status: the workspace runtime is active, the legacy runtime is removed and the local records have been merged with backups. Automatic naming and foreground management remain unfinished. [The implementation plan](workspace-implementation.md) records the reviewed contracts and verification evidence. [The architecture document](design.md) provides design rationale and earlier context.

## Objective

Show the shared purpose and references of a Herdr workspace alongside each Pi session's own assignment and progress. Support agents working in different repositories or worktrees without changing which file a shared reference opens.

A workspace can contain a library checkout and a client checkout. Each agent has its own status and summary. Both card panes show the same workspace goal and references. There is no separate workspace-overview tab.

## Agreed ownership

| Record or surface | Authority |
| --- | --- |
| Workspace record | Shared title, plan-derived goal, checkout table, documents, issues and PRs |
| Agent record | Its Pi session identity, workspace membership, checkout, working directory, title, status, Now and Remaining |
| Workspace name | Workspace title, independent of any agent's assignment |
| Pi session name | That agent's title |
| Herdr tab name | Agent title, but only the first pane in the current tab layout has naming rights |
| Document editor tab | Document identity, qualified by checkout and path when needed; preserve manual renames |

Work Cards will replace the separate `herdr-naming.ts` extension. Do not run competing automatic namers. Disable the old extension only when the replacement is ready.

The summarizer generates automatic titles. Explicit manual names remain protected. Titles and display labels are not record identities.

## Records and checkout identity

Store shared workspace data separately from agent data. An agent changing its status must not rewrite another agent's status or replace the shared reference collection.

Each workspace has a local checkout table. A checkout means a concrete directory on the execution host, not a Git remote or repository name. Two worktrees of one repository can hold different versions of a document and remain distinct checkouts. Do not introduce a global repository registry.

Documents reference a checkout key and a relative path. Agents identify their checkout and actual working directory, which may be a subdirectory. A document's location never depends on the viewing agent's working directory.

These examples show domain fields, not the serialized schema. The implementation plan specifies storage versioning, title provenance and summary-source metadata.

Workspace example:

```yaml
id: workspace-id
title: Library and client integration
goal: Introduce the new library API and update its client
checkouts:
  library:
    directory: /code/library
  client:
    directory: /code/client
documents:
  - checkout: library
    path: docs/design.md
    description: Library API change plan
  - checkout: client
    path: docs/integration.md
    description: Client integration plan
issues: []
prs: []
```

Agent example:

```yaml
workspace_id: workspace-id
session:
  id: pi-session-uuid
  file: /host/path/to/session.jsonl
checkout: client
cwd: /code/client/packages/sdk
title: Client integration
status: active
now: Client adapter implemented
remaining: Verify compatibility with the new library API
```

The Pi session UUID identifies the agent record. Resuming the same session retains that identity; a fork with a different UUID gets a new record. The session file is a locator, not an identity, and is null when unavailable. The runtime supplies these fields; the summarizer cannot invent or alter them.

Checkout keys stay stable when display labels change. Resolve directory aliases deliberately so one checkout is not registered twice under different spellings. Document access must still reject traversal, escaping symlinks and non-regular files. A missing document remains a removable reference; opening it reports the failure rather than guessing another checkout.

## Display and navigation

Each card combines the workspace Goal with the agent's Now and Remaining. The latter describe that agent's assignment, not the entire project's backlog.

Use the workspace title as the Goal label and the agent title as the Now label:

```text
Workspace title: shared goal
Agent title: current progress                    ● active
Remaining work
```

Italicize both titles and the remaining-work text. Keep Goal and Now text plain, without separate `Goal:` or `Now:` labels. Place the agent's status beside its progress when space permits. Omit empty remaining-work text; a title can stand alone when its corresponding text is empty. Wrap long text within the existing height and reference-space limits. Preserve reference shortcuts and the close hint.

For one checkout, retain the current short document path:

```text
[a] docs/design.md
```

For multiple checkouts, qualify document paths:

```text
[a] library/docs/design.md
[b] client/docs/integration.md
```

Start checkout labels with the directory basename. If basenames collide, include more parent components until they distinguish the checkouts, such as `backend/common` and `frontend/common`. Compute labels consistently across the workspace, not separately for each agent. Labels may change as checkouts are added; stored identities must not.

Apply the same qualification to automatic editor-tab names when needed. Preserve manually renamed tabs and unsaved editors. Reuse uses verified canonical checkout and file identity, never the displayed label. Collision-driven title refresh requires surface ownership and a current name matching the last automatic name, as specified in the implementation plan.

Document letters and link numbers remain positional and may renumber. Removal does not use these shortcuts as identifiers. HTTP links keep their existing client-handled opening behavior.

## Startup and workspace membership

Startup and joining are automatic by default, with configurable manual operation. Manual operation waits for explicit open or join; it does not disable summaries after attachment. Start naming from meaningful task context rather than an empty session.

The live Herdr workspace defines the group. Do not infer membership merely because two sessions use the same checkout.

When an agent moves or resumes in another Herdr workspace:

1. Resolve the destination from verified live caller identity, not inherited environment IDs or GUI focus.
2. Join the destination's shared context and register or reuse the agent's checkout there.
3. Retain the agent's session identity, assignment and status.
4. Leave shared documents, issues and PRs in the source workspace. Do not copy them as part of the move.
5. Invalidate requests tied to the previous membership so their results cannot be applied after the transition is observed.

The destination may need a different local checkout key. That must not redirect a document or change the actual directory in which Pi runs.

Membership changes are not evidence to import references from old conversation history. The design must distinguish new-workspace contributions from inherited evidence, rather than allowing the first post-move summary to repopulate the source workspace's links.

Keep records after a pane or workspace closes. Closing a card with `q` remains a display action, not a detach or deletion. Automatic startup must be idempotent and must not keep reopening a deliberately closed card.

The exact behavior during unavailable Herdr identity, idle tab moves and restored sessions needs native API verification. Do not claim an event or instantaneous move detection without evidence.

## Summary inputs and write permissions

The workspace goal comes only from selected plan documents. Linked files are not automatically authoritative plans. Select plan documents automatically at first; do not invent a goal from agent dialogue when no suitable plan exists.

Agent summaries use session evidence and shared context, while retaining the current truthfulness and no-activity-log guidance. A plan proves intended scope, not completed work.

Background summaries may:

- update the calling agent's automatic title, status, Now and Remaining
- derive the workspace's automatic title and goal from their approved inputs
- propose additions to shared documents, issues and PRs

They may not:

- remove references or delete records
- modify another agent's fields
- change runtime identity, membership or working directories
- register arbitrary filesystem roots
- rename manually named surfaces or write project documents

The final request and patch contracts must enforce field ownership and additive reference updates. Prompt instructions alone are not a substitute for these restrictions. Source interpretation and summary quality still depend on the model.

Shared additions merge without dropping another agent's additions. Stale shared results cannot resurrect a removed reference. Explicit removal must also survive later summarization of old evidence. A workspace-level exclusion set preserves removals. Only an explicit foreground re-add clears an exclusion; excluded document identities also retain their checkout keys.

Plan loading must use known checkout boundaries and current selected references. Choose a bounded source-reading policy and a refresh trigger before implementation. Do not add an unrestricted model filesystem tool or silently truncate input while claiming a complete plan view.

## Foreground management and removal

Keep `work_card` read-only. Add scoped management actions for creating, reading, updating and deleting records and references. Start with the current workspace and calling agent, not a global manager for arbitrary agents' cards.

Foreground agent reference removals execute directly, without a confirmation dialog. The management API identifies a document by an exact checkout/path identity, or a link by its exact URL. It does not depend on display numbering.

The command is:

```text
/work-card remove <substring>
```

It performs a literal, case-sensitive substring match against resolved stored document paths and stored issue/PR URLs. Require a non-empty argument matching exactly one reference across all groups. A relative suffix still works because it occurs in the resolved path.

For no matches, report not found. For multiple matches, report qualified candidates and change nothing. Do not open a picker. Resolve against a checked snapshot and reject stale removal rather than silently selecting another target.

Removal unlinks metadata only. It does not delete a file or close an issue or PR. Whole-record deletion is a separate operation and must not silently cascade into other agents, project files or remote artifacts. Refuse workspace deletion while any agent record refers to it, including the caller. Explicit own-agent deletion may precede workspace deletion.

## Persistence and migration requirements

Retain strict validation, optimistic revision checks, exclusive cooperating-writer locks and atomic file replacement. Keep the existing distinction between atomic visibility and crash durability. Herdr metadata remains a transient binding or projection, not the durable source of truth.

Splitting records introduces multi-file transitions. Specify write ordering and recovery for partial creation, membership changes and migration; do not describe several independent renames as one atomic transaction.

Use only workspace and agent records after cutover. Do not retain the old launch environment, compatibility branches or a runtime importer. The user approved a one-off conversion of the small local inventory instead.

Back up the existing card files and native binding metadata before conversion. Preserve references, supported own-agent state and manual names. Leave session history untouched. Do not treat an old conversation-derived Goal as plan provenance. Verify the replacement before switching live bindings or disabling the separate naming extension.

## Implementation sequence for review

- [x] Specify and test the workspace, checkout, agent and reference contracts, including forbidden mutations.
- [x] Implement record persistence and guarded shared-reference operations, including removal exclusions and recovery from partial operations.
- [x] Adapt the combined card view and document opener to explicit checkout identities and qualified names.
- [x] Replace workspace-wide single-card ownership with verified per-agent pane ownership; test multiple tabs in one workspace.
- [x] Add session-backed startup, restore and membership transitions, including movement and `q` behavior.
- [x] Separate agent summarization from shared plan-derived updates, with provenance and stale-input checks.
- [ ] Add automatic naming and manual-name protection; prove first-pane-only tab naming.
- [x] Add command entrypoints, convert the local records once and remove the old runtime.
- [ ] Add scoped foreground management entrypoints.
- [ ] Verify the replacement before disabling the old naming extension and updating installed-session instructions.

This order may change after the native integration investigation. Each implementation slice needs failing tests first, relevant verification, review and a focused local commit. No pushes are authorized.

## Project conventions and verification

Use the existing ESM TypeScript package and dependencies. Runtime sources live in `src/`; Node test-runner tests live in `tests/`; design documents live in `docs/`. Pi loads the source TypeScript graph; Herdr loads compiled JavaScript. Do not introduce a second package or a central registry.

Match the current style: single quotes, 2-space indentation, small typed functions and explicit host-boundary dependencies. For example:

```ts
export interface CardDocument { path: string; description: string }
```

Use the commands defined in `package.json`:

```sh
npm run check
npm run build
npm test
```

`npm test` builds first and then runs `node --test tests/*.test.mjs`. Verify supported Node 22 and the current Node 26 environment. Use the real Pi loader/package tests, deterministic host doubles for lifecycle cases, and isolated native Herdr checks for movement and multi-tab ownership. Do not call test doubles live UI verification.

Required regression cases include:

- independent status updates from two agents sharing one workspace
- same relative document path in distinct checkouts and basename collisions
- canonical aliases, worktrees, missing files, escaping symlinks and Unicode display bounds
- shared additions racing with additions and explicit removals
- old evidence not undoing removal or copying references after a workspace move
- UUID-preserving resume, fresh-UUID forks and ephemeral sessions
- reload, tab moves, stale callbacks, partial creation and closed card panes
- first-pane naming rights and preserved manual session, workspace and editor names
- unique, missing and ambiguous removal matches across documents and URLs
- one-off local conversion, exact launcher-to-pane environment, package contents and read-only tool behavior

Always preserve user edits and unrelated `.pi/subagents.json`. Ask before adding dependencies, changing the approved ownership model or performing destructive live migration. Never overwrite another agent's state, guess ownership from GUI focus, run competing naming extensions, remove failing tests or bypass signing.

## Technical review and remaining verification

Astra approved the corrected contract and the implemented runtime slices. Native adapter checks cover movement, independent pairs, documents and sizing. Full Pi-session move/reconnect behavior and automatic naming still need runtime verification. Foreground management remains to be implemented. Keep persistence recovery, reference exclusions, evidence boundaries and source freshness in the review gates.

Product changes discovered during implementation return to the user rather than becoming undocumented defaults.
