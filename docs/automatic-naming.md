# ADR 3 Automatic naming and manual overrides

Status: accepted. [ADR 2](workspace-context.md) defines records, membership and document identity. The [README verification status](../README.md#verification-status) distinguishes verified behavior from rollout requirements.

## Context and problem

Workspace, tab and Pi session names help people distinguish shared work from individual assignments. Independent naming prompts can disagree with the records or overwrite deliberate names. Pane moves and changes to the first pane also transfer naming authority without changing the named tab.

Existing nondefault names have uncertain origins. Text alone cannot distinguish an old automatic name from a manual choice. Matching a record title does not prove automatic ownership.

## Decision

### Project record titles without another model request

Naming is a deterministic projection of validated records and exact document identities:

| Surface | Name source | Authority |
| --- | --- | --- |
| Native workspace | Shared `WorkspaceRecord.title` | Verified workspace binding and current membership |
| Pi session | Own `AgentRecord.title` | Current Pi session UUID and own record |
| Agent tab | Own `AgentRecord.title` | Agent pane is the actual first layout pane |
| Editor tab | Exact document identity, qualified by checkout and path | Verified document owner is the actual first layout pane |

Only the actual first layout pane has tab naming rights. An ineligible first pane does not transfer those rights to a later agent or editor. A non-first agent can still name its own Pi session.

Record titles may be explicitly supplied or model-derived through the existing evidence contracts. An automatic agent title is a short assignment label within the workspace, not a repetition of the project name: “Naming and ADR” can distinguish an assignment inside “Work Cards”. Brevity is a soft target, not a truncation rule; retain words needed to distinguish the assignment. A redundant automatic title can be corrected without implying changed progress. Explicit manual titles remain protected.

Naming makes no model request and invents no purpose. Sparse purpose remains empty; neither fabricated Goal text nor compact summary fields supply a fallback label.

The card-pane caption remains a separate app-owned projection of the workspace title. Caption ownership does not authorise renaming other surfaces. Document identity governs editor reuse; its displayed name never becomes an identity or a document target.

### Require attribution for automatic agent names

Lifecycle bootstrap titles are raw first-user-message excerpts or `Untitled task`, not proof of a helper-selected name. The Pi host must not project them eagerly.

A successful own-helper response must contain an actual title proposal. After publication and revalidation, `WorkspaceRefreshInput.onOwnTitlePublished` conveys that verified title to the host. The host records the accepted source title in UUID-bound session-local naming state, separately from Pi applied-name provenance.

Successful explicit native naming can also establish acceptance of the current own source title, including a bootstrap title. Before recording acceptance, recheck host and scope authority and confirm the title has not changed during the operation.

Automatic Pi session and agent-tab naming require a matching accepted source title, or an explicit manual record title. Same-UUID reload can reuse the attribution. Automatic `Untitled task` remains ineligible, even after explicit acceptance. Shared workspace naming does not depend on own-title readiness; its initial claim and manual protection use separate provenance. This adds no `AgentRecord` schema field or title-length, status or plausibility heuristic.

Malformed session-local naming provenance visibly suspends Pi and automatic own-tab naming, not independently authorised workspace naming. Accepting a title must not silently replace malformed state. Optional naming failures do not invalidate a successfully published own summary or block subsequent shared-reference and Goal stages.

### Control each naming surface independently

`workspace_naming`, `tab_naming` and `session_naming` each accept `true`, `false` or `null`. Omitted or `null` settings follow `workspace_startup`: enabled for `automatic`, disabled for `manual`. Explicit booleans override that default independently. The tab setting covers agent and document-editor tabs. The session setting does not disable assignment-title attribution needed by tabs.

Disabling automatic naming leaves records and naming history intact. Re-enabling it does not reclaim a manually protected surface. `/work-card naming` is deliberate native reclaim, so it bypasses these automatic switches without changing Pi session names. Card-pane captions remain app-owned and are not controlled by these switches.

### Claim an initial workspace name once

Herdr 0.9.5 derives workspace labels from git and the working directory. Snapshot `WorkspaceInfo` exposes a label and number, but no automatic, manual, custom-name or default proof. First Goal creation cannot establish that the native label was never manually named. Automatic workspace naming instead grants one initial claim when a grounded purpose and name are accepted. This may replace a name chosen before Work Cards observed it; that is an accepted cost of automatic naming, not a native default-label heuristic.

Before inference, capture an unclaimed workspace's observed label in surface-scoped pending provenance. Pending state grants no automatic ownership. If the label changes before the claim, protect it as manual. A claim requires a known Goal and an accepted title proposal, or an explicitly authored workspace title. A known Goal with an unproven bootstrap title is not enough. Bind helper-title acceptance to the pending revision captured before that inference, so older source attribution cannot authorise a later baseline.

The host persists accepted workspace-title attribution in a UUID-bound Pi entry, separate from own-session naming. It records workspace identity, title and the pending revision, not conversation contents. Same-session reload retains this proof. Foreign sessions and changed workspace identities cannot reuse it. Reset, source loss and configuration toggles do not reset native claim history.

Existing manual pins remain protected, including pins written by the older conservative policy: their origin cannot be distinguished from a deliberate override. Use `/work-card naming` to reclaim those names. An existing bound workspace with genuinely missing native provenance can establish a pending baseline, but needs a fresh accepted name proposal; historical Goal text alone supplies no attribution.

### Preserve independent manual choices

Outside the initial workspace claim, preserve unproven nondefault names. Matching the desired title never proves automatic ownership. Names produced by an older namer receive no special authority.

A tab's default label is its one-based current position among that workspace's tabs in authoritative snapshot order. Its stable `tab.number` is not default-label proof after reordering. A verified default tab remains eligible for automatic naming.

A surface with valid automatic provenance can track its source while its observed label matches the last applied name. An external label change protects that surface as manual. Native observations cannot distinguish a same-value manual assignment without origin or event data. Pi can detect a same-value `/name` through a changed `session_info` entry ID.

A manual record title can be the desired projection, but does not authorise replacing a separately manual native label.

Native workspace, native tab and Pi session overrides are independent. A manual tab name must not become the Pi session name. Explicitly reclaiming native naming must not overwrite a separately named Pi session.

### Bind native provenance to the surface lifetime

`NameStore` holds native naming provenance. Its identity includes the canonical store, socket, native workspace, binding lifetime, surface kind and surface identity. The current owner is recorded, but is not part of that identity.

This scope prevents unrelated or recreated surfaces from inheriting naming rights. It also preserves history when first-pane ownership transfers between agents or between an agent and an editor. Transfer does not require the predecessor to remain live.

Provenance records a pending workspace baseline, the last applied automatic name, or a manual pin. It is a local cache, not project state or a global registry. Missing provenance does not prove automatic ownership. Malformed or unavailable provenance suspends naming rather than granting takeover rights.

Editor tabs share this native surface provenance. Separate agent and editor caches would lose manual protection when the first pane changes role.

Pi naming belongs to the Pi host. Its provenance is session-local and bound to the session UUID, not a native tab identity. Resume of the same UUID retains that scope; a new UUID cannot inherit another session's naming authority.

### Make native reclaim explicit

`/work-card naming` applies the current native workspace and eligible tab record titles, then resumes automatic tracking on those surfaces. It establishes automatic provenance even when the desired text already matches the observed label. A later manual rename protects the surface again.

The command requires attached, unpaused authority. It does not open a display, join a workspace, call a model, change record titles or call `setSessionName`. Successful explicit title acceptance permits later automatic naming of an otherwise unclaimed Pi session; independent Pi manual protection still applies.

Explicit intent permits native reclaim and bypasses the host's automatic-title readiness gate. It does not bypass membership, binding, cancellation, surface identity, provenance-validity checks or independent Pi manual protection. The actual-first-pane rule still applies. A non-first invocation reports that the tab was skipped instead of naming it from the wrong pane.

### Separate policy from native effects

`decideName` owns the pure decision over the desired title, observed label, provenance and intent. `NameStore` owns surface-scoped provenance. The native adapter owns verification, locks, native acknowledgements and effects; it has no Pi SDK dependency.

The adapter contract is:

```ts
WorkspaceClient.projectNames(
  store, context, paintedAgent, config, signal?, intent = 'automatic',
  acceptedOwnTitle = paintedAgent.record.title,
  acceptedWorkspaceTitle = null
)
```

The host passes the exact accepted own title, or `null` when it is unready. Automatic tab projection rechecks that title against the current record. `prepareWorkspaceNaming` captures the pending revision before inference. Initial workspace claims receive `{ title, pendingRevision }` and recheck both values against current records and provenance. Already claimed workspace names track independently of own-title readiness. Explicit intent bypasses source readiness, not native authority.

The adapter returns separate workspace and tab outcomes: `renamed`, `unchanged`, `manual`, `not-ready` or `disabled`. Tabs can also report `not-first`. Invalid authority, invalid provenance and native or persistence failures remain visible failures, not successful outcomes.

The Pi host owns session lifecycle and session-local naming. It calls native projection only through current attachment authority. Neither layer treats a successful record update as proof that a native label or Pi name changed.

Native rename effects reject names starting with `-` to avoid ambiguous Herdr option parsing. They do not invent replacement punctuation. Same-label reclaim needs no native rename and remains allowed.

Recheck current records, membership, binding, surface identity, layout and observed labels after asynchronous preparation and before effects. Cancellation or changed authority invalidates stale work. Do not hold record locks across native I/O. Native locks coordinate cooperating naming operations; they cannot freeze user actions.

A native rename requires a verified acknowledgement before recording success in provenance. Native success followed by provenance-write failure is partial progress: report it without claiming saved ownership. Retry must not adopt the resulting nondefault label as automatic merely because it matches. An existing valid history can still reveal the label change; missing history leaves it unproven.

Record publication, native operations and provenance persistence are separate effects. There is no distributed transaction or promise to roll back a successful rename after a later failure. Independent surface outcomes must remain inspectable.

## Alternatives and tradeoffs

| Alternative | Reason for this decision |
| --- | --- |
| Prompt separately for each visible name | Duplicates inference and can contradict authoritative titles |
| Take over old names that match records | Matching text does not establish consent or provenance |
| Key provenance by the current owner | First-pane transfer would discard manual protection and automatic history |
| Require the previous owner to remain live | A dead predecessor would prevent legitimate surface-authority transfer |
| Let a later eligible pane name the tab | Makes tab naming depend on skipping the actual first pane |
| Use native tab identity for Pi provenance | Couples independent session names to pane layout and movement |
| Make explicit reclaim bypass safety checks | User naming intent does not prove current native authority |
| Hold record locks across native operations | Blocks record writers without preventing external label changes |

These decisions leave some old generated names fixed until explicit reclaim. They add local provenance and conservative failure handling, but preserve manual choices across ownership changes.

## Rollout boundary

Only one automatic namer may control these surfaces. Remove the old `herdr-naming.ts` extension from discovery and fully reload participating Pi sessions before activating the replacement. Removing its installed link does not unload code already running in a session. Its retained source and tests do not constitute an active installation.

The old namer's shutdown aborts model work but does not await its in-flight native `syncQueue`. Discovery removal and `/reload` prevent future old work, but do not guarantee completion of native commands already dispatched. Activation must also ensure old native work has quiesced. Restarting the affected Pi process is the conservative option.

This repository does not uninstall extensions, change dotfiles, reload or restart other systems. Automated verification does not establish live activation, complete Pi lifecycle behavior or absence of an older in-memory namer. Native and Pi implementation verification and live rollout remain separate from acceptance of this design.
