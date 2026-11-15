# Modular Streams integrations and herdr-app support

Author: Wendell Smith

Date: 11 November 2026

Status: Draft

## Overview

Streams should expose a common integration contract for agent harnesses and workspace applications. Documented lifecycle hooks and interfaces define how an integration reports its context, requests work operations and reads saved work. Each integration implements the capabilities it supports and keeps its native resource types and commands within its adapter.

This design adds hc and native herdr-app views through that contract. It also brings the existing Herdr and cmux integrations behind the same boundary, preserving their supported behavior. Shared components own workflow decisions, catalog operations and card content.

## The problem

Streams lacks a complete integration contract independent of a particular frontend or agent harness. Its workspace interface covers creation, focus and naming. Reopening, conversation ownership and native cards still consume Herdr-specific observations. Another integration would need to adopt those assumptions or duplicate workflow decisions.

hc and herdr-app expose the limits of that boundary. hc runs persistent terminals; herdr-app arranges their presentation. Their separate lifetimes require the shared workflows to represent execution and presentation independently.

The catalog stores shared work in Streams and individual agent conversations in Sessions. Its existing card content provides the context for the proposed sidebar and attached companion. Integrations need access to that content and its navigation operations through the common contract.

## Goals

### Common integration boundary

The design has 4 integration goals.

- document the lifecycle hooks, requests, results, observations and saved display content available to any integration
- keep workflow decisions, catalog operations and card content in shared components that integrations can use independently
- let each integration declare its supported conversation and presentation capabilities, with native types and commands confined to its adapter
- verify shared behavior through common contract tests applied to the adapter implementations

### hc and herdr-app support

The proposed hc and herdr-app integration has these workflow goals.

- support Stream opening, exact saved-conversation resumption, space association, native context views, name synchronization and document navigation
- launch and reuse conversations through hc while herdr-app is closed, with execution and presentation reported independently
- preserve the existing Herdr workflow and cmux's current capabilities
- show Stream goals, documents, artifacts and links in a right-sidebar panel alongside Files and Changes
- show compact Session name, saved status and short current state in an attached pane strip, with full recap, current state and remaining work in popup detail
- open documents using a selected viewing Session, defaulting to the focused agent; preserve each artifact's producing Session identity
- retain a space's Stream association until the space is removed, including when its last terminal closes
- use existing platform APIs, catalog records and reporting behavior; keep native associations separate from the shared catalog model

The initial hc/herdr-app delivery targets Pi, including configured-remote owners with desktop-side native presentation. Same-owner space moves follow the existing verified move-versus-refresh membership policy.

## Non-goals

The following work is outside this draft.

- upstream hc or herdr-app API changes as an implementation dependency
- changes to Pi, Claude or Codex harness implementations or installed dependencies
- new Claude or Codex hc integration implementations in the initial delivery
- another writable work catalog or another inference pipeline for the native views
- independent writes to the app-owned layout document
- recreating the terminal card's pane arrangement in herdr-app
- restoring saved pane layouts or adding cross-host conversation execution

Native editing of existing card fields is outside this design. The native views show saved content and navigate its references. hc document navigation opens an ephemeral owner-side read-only less pager with a 4 MiB working-file cap and no truncation. Artifacts use the same secure pager with their existing limits. Herdr keeps its editor/reuse behavior. A configurable hc editor and sending file content over the mod RPC were rejected to keep this delivery's owner execution and desktop boundary small.

## Open questions and evidence limits

### Deferred harnesses

New Claude and Codex hc integrations are outside the initial delivery. Their saved-conversation and existing frontend support remain unchanged. Their identity investigations are retained for future work and do not gate the Pi release.

### Pi context changes remain an open question

The selected starting design keeps the current Session ID in process memory, checks the SDK context as needed and resets Session-specific state when the ID changes. Pi's ordinary `/new`, `/fork`, `/resume`, startup resume selection and reload commands remain enabled and unrestricted. Whether the relevant SDK callbacks provide current context and keep its native publication accurate through changes remains an open implementation question. Retained probes show why an old helper/context cannot prove current identity. The starting implementation will exercise these paths and report concrete problems with clear affected-operation errors. No Pi modification, transition guard or general recovery framework is selected.

### Configured-remote presentation

Remote native UI is included. Rust runs on the admitted work owner and performs catalog, process, terminal and association operations there. Each machine stores its own app layout document in its own core; remote spaces do not belong to the desktop core. The desktop mod executes only typed focus, naming and browser requests from fresh owner replies. It never opens an owner path locally or executes a remote-supplied command. The owner/desktop protocol below remains proposed and needs independent review and live acceptance.

### Native mod acceptance

The thin mod uses bounded Rust CLI calls, save notifications, hourly safety reads and manual Refresh as specified below. End-to-end local and remote delivery, producer coverage, shared task ownership and supported-host invocation limits need acceptance tests. Source inspection establishes API feasibility only.

### Native UI installation

The written installation plan targets existing app layout and mod APIs to preserve native pane controls plus Files and Changes; it remains unexecuted. Require `pane.lone = true`, retaining the single 30 px header even in unsplit tabs, as in the [inspected local main pane configuration](herdr-app-evidence/herdr-app-local-pane.json). Ordinary headers keep the native renderer and controls. Runtime Rust operations never rewrite the app-owned layout document.

## Technical plan

### Integration contract

The contract has 2 sides. Clients call Streams operations and send lifecycle notifications. Native adapters supply observations and execute prepared effects. The pseudocode below defines those logical calls. Native association storage and the mod's wire framing and process arrangement are specified below.

#### Shared identifiers and outcomes

A Streams application instance runs on one admitted catalog source and record-owner host. Record identifiers in its requests are relative to that scope, including when a desktop invokes it remotely. A native reference identifies a resource at a particular adapter endpoint. hc references contain the owner's permanent core identity and unqualified resource key. Viewer machine aliases route requests; they are not identities.

```text
// Illustrative pseudocode, not implementation code.

WorkScope = { catalog: CatalogSource, host: HostId }
ConversationId = { harness: HarnessId, native_id: String }
NativeRef = { adapter: AdapterId, endpoint: EndpointId, key: OpaqueKey }

Presence =
    Running { locations: NonEmptyList<NativeRef> }
  | Absent
  | Unknown { reason: String }

Access<T> =
    Available(T)
  | Unsupported
  | Unavailable { reason: String }

Effect<T> =
    Applied(T)
  | NotApplied { reason: String }
  | Uncertain { reason: String }
```

`Absent` means the adapter completed the required observation and did not find that conversation. `Unknown` means it could not establish presence. Reuse-capable work opening can schedule a replacement only for `Absent`. `Uncertain` means an effect may have happened despite a failed reply; it does not authorize replay.

#### Client-facing operations

These operations are shared application calls. The CLI, Pi extension and native views translate their inputs into these calls. Existing catalog mutation requests retain their current domain schemas and validation.

```text
OpenRequest = {
    stream: StreamId,
    sessions: AllEligible | Only(NonEmptyList<SessionId>),
    cwd: Optional<Path>
}

Reference =
    Document { stream: StreamId, document: DocumentKey,
               viewing_session: SessionId }
  | Artifact { key: ArtifactKey }
  | Link { stream: StreamId, link: LinkKey }

ActivationContext =
    Explicit
  | FollowFrame { terminal: NativeRef, expected_session: SessionId }

CompanionDisposition = Opened | Existing | Suppressed | Deferred
ProjectionResult = {
    companion: Optional<CompanionDisposition>,
    desired_pi_name: Optional<String>, warnings: List<String>
}
LifecycleReport = {
    native: Active | Inactive { reason: OutsideFrontend | NotEnrolled
                                     | Unavailable | Stopped },
    projection: ProjectionResult
}
ReadReply<T> = {
    view: T, viewer_actions: List<ViewerAction>, warnings: List<String>
}
DisplayReport = {
    preference: Effect<Unit>,
    presentation: Effect<ProjectionResult>
}

interface StreamsApi {
    scope: WorkScope

    read_stream(stream: StreamId) -> Result<ReadReply<StreamView>>
    read_session(session: SessionId) -> Result<ReadReply<SessionView>>
    read_frame(terminal: NativeRef) -> Result<ReadReply<FrameContext>>
    read_sidebar(space: NativeRef, viewing: ViewingChoice)
        -> Result<ReadReply<SidebarContext>>

    open_work(request: OpenRequest) -> Result<OpenReport>
    prepare_resume(session: SessionId, cwd: Optional<Path>)
        -> Result<VerifiedLaunch>

    show_companion(binding: ConversationBinding) -> Result<DisplayReport>
    hide_companion(binding: ConversationBinding) -> Result<DisplayReport>
    open_reference(reference: Reference, at: Optional<NativeRef>,
                   context: ActivationContext) -> Result<ReferenceOpenReport>

    lifecycle(binding: ConversationBinding, event: LifecycleEvent)
        -> Result<LifecycleReport>
}
```

`AllEligible` means the Stream's current same-host Sessions with status `active`. `Only` selects the named current members, including inactive Sessions, and changes no status. Harness support is checked separately, so deferred hc harnesses receive explicit skipped results. Archive controls ordinary Stream-list visibility; opening an addressed Stream does not unarchive it or close any runtime. The [existing active-Session policy](../docs/record-local-operations.md) remains authoritative.

`prepare_resume` verifies the saved conversation and checkout, then applies live-duplicate checks where the active adapter supports that harness. For Claude/Codex in hc shells, preserve existing verified transcript/directory checks and process replacement without claiming hc presence observation. A Session attached without a retained locator still fails explicitly; this does not add discovery or new Claude/Codex hc support. In-place prepare_resume inherits the caller's terminal IDs, TERM and credentials and adds STREAMS_STREAM_ID; it does not use the scrubbed environment for a newly created terminal. Pi uses the new hc checks. This compatibility path does not enable new hc `open_work` launches for deferred harnesses. Native views use `open_work` without replacing their own process.

DocumentKey identifies the existing repository/path pair. LinkKey identifies the saved issue-or-PR list and exact URI. `ArtifactKey` is the existing host, producing Stream, producer Session and name identity. The key's host must match `WorkScope.host`. Artifact references retain their producing Stream when the Session moves. These keys add no stored identifiers.

`open_reference` rereads the selected reference and validates its current scope. `FollowFrame` requires Rust to recheck that terminal's current enrolled Session against `expected_session`; a changed or unavailable frame fails without retargeting. Pinned/saved-key activation uses `Explicit`. It resolves a document against the captured viewing Session and an artifact against its producing Session. `at` identifies the actual calling/viewing native space; the adapter validates it freshly. hc document/artifact opening requires that published target, or a unique observed associated space when `at` is omitted. Missing or wrong-endpoint placement fails before terminal creation. Links need no space. Cached data supplies reference keys, never a trusted path or command.

A show or hide request saves the relevant card preference, then requests presentation for the bound conversation after a confirmed save. `DisplayReport` preserves both outcomes; an unconfirmed preference change prevents presentation. These requests trigger no catalog progress report or inference and change no Stream membership.

#### Saved view data

The view types contain existing catalog fields and reference identities. Their layout is independent of native resources.

```text
DocumentKey = { repository: String, path: String }
LinkKind = Issue | Pr
LinkKey = { kind: LinkKind, url: String }
SessionSummary = {
    id: SessionId, title: String, status: Status,
    harness: HarnessId, state_short: Optional<String>
}
DocumentReference = { key: DocumentKey, description: String }
LinkReference = { key: LinkKey, description: String }

StreamView = {
    id: StreamId, title: String,
    goal: Optional<String>, goal_short: Optional<String>,
    status: Status,
    sessions: List<SessionSummary>,
    documents: List<DocumentReference>,
    links: List<LinkReference>,
    artifacts: List<{ producer: SessionId,
                      entries: List<ArtifactSummary> }>
}

SessionView = {
    id: SessionId, stream: StreamId, title: String, status: Status,
    recap: String, recap_short: Optional<String>,
    state: String, state_short: Optional<String>,
    remaining: String, remaining_short: Optional<String>
}
```

These are derived reads of existing records. They add no catalog fields. SessionSummary projects the named Session fields; DocumentReference projects repository/path and description. Links map Stream.issues to Issue and Stream.prs to Pr, preserving each list's order with issues first. The kind is part of LinkKey, so a URI present in both lists remains two references. Documents retain saved order; Session and artifact ordering follows the delivery plan. Activation rereads the matching list and exact URI before validating a web URL. Neither read requires a pane, tab, terminal or workspace identifier. A Stream with no Sessions still has a valid `StreamView`.

The sidebar uses `StreamView`. Its viewing-Session selection is local UI state. The attached strip uses `SessionView.title`, `status` and `state_short`; popup detail uses the full progress fields. The existing terminal card shares addressed-record field mapping with these views and retains its owner-local DisplayContext for repositories and resolved checkout roots. It need not materialize StreamView's roster/artifact listings. Those owner paths are not native-view payloads.

#### Native context views

These reads use current native context to select saved views. They neither enroll nor move a Session. Space-only reads need no host process census. Sidebar Follow with a last-agent candidate additionally checks that one terminal's foreground context. Frame identity needs positive current-foreground evidence, without claiming conversation absence elsewhere.

```text
CompanionVisibility = Shown | Hidden

FrameContext =
    Enrolled { session: SessionView, companion: CompanionVisibility }
  | Unenrolled
  | NotApplicable
  | Unavailable { reason: String }

ViewingChoice =
    Follow { last_agent: Optional<NativeRef> }
  | Pinned { session: SessionId }

ViewingSession =
    Selected { session: SessionId }
  | None
  | Unavailable { reason: String }

SidebarContext =
    Associated { stream: StreamView, viewing_session: ViewingSession }
  | Unassociated
  | Unavailable { reason: String }
```

`NotApplicable` covers shells and positively identified non-interactive/deferred harnesses, preserving their native header. It says nothing about catalog enrollment. Failed identity observation is unavailable, never ordinary absence. `Unassociated` is a successfully observed empty sidebar context.

read_sidebar first resolves the published space association and loads its StreamView. With a valid association, viewing-context failure preserves Associated and changes only ViewingSession.

| Viewing input and fresh result | ViewingSession |
| --- | --- |
| Follow with no last_agent, or candidate outside this space | None |
| Follow with a foreground shell/deferred harness, unenrolled conversation, or Session in another Stream | None |
| Follow with verified current enrolled Session in this Stream | Selected with that Session ID |
| Follow with unknown/unavailable foreground evidence | Unavailable with reason |
| Pinned with an existing same-host current member of this Stream, including inactive members | Selected with that Session ID |
| Pinned with missing, foreign-host or non-member Session | Unavailable with reason |

The UI retains last_agent as a terminal candidate while the sidebar is focused. Rust resolves its current conversation. Follow uses point inspection with that terminal as the foreground candidate; Pinned needs no foreground check. Failed association/layout access returns SidebarContext.Unavailable; a verified space with no association returns Unassociated.

#### Lifecycle hooks

A conversation binding associates a live harness source with one exact conversation in the application's scope. The harness integration supplies that identity when the binding is established. Establishing a binding is separate from enrolling a Session.

Pi supplies its exact SDK session-file candidate, which may be absent before persistence or for --no-session. Rust verifies an existing candidate against that harness/conversation before retaining it in the existing saved-locator bindings. Do this on enrollment, on NewlyBound after external attachment, and when a later valid callback/request first supplies a persisted locator for an enrolled Session.

Missing-before-persistence leaves identity registration available. In-memory Sessions remain usable while Running; after exit, open_work skips a missing-locator Session explicitly and prepare_resume fails without discovery.

Enrollment saves a Session for this host/harness/native conversation and assigns it to one Stream, creating that Stream first if necessary. A meaningful Pi report can do this under automatic enrollment; manual enrollment uses explicit attachment. The catalog attachment operation records an existing conversation without launching it. Existing Pi reporting and inference policy remains unchanged. Identity registration alone writes no catalog records. For example, a new Pi first registers identity, then its first accepted report can enroll it and trigger `NewlyBound`. Restarting a helper for that already-enrolled Session emits `Reattached`, creates no enrollment and does not replay automatic opening. Preserve the [existing enrollment boundary](../docs/native-enrollment-boundary.md).

```text
ConversationBinding = {
    conversation: ConversationId,
    session_file: Optional<Path>,  // SDK-supplied candidate, not verified yet
    source: LiveHarnessSource
}

LifecycleEvent = Started | NewlyBound | Reattached | Stopping
               | NativeChanged { kind: NativeChangeKind }
NativeChangeKind = PlacementChanged | DisplayChanged

```

`LiveHarnessSource` denotes the existing SDK-bound channel/callback supplying identity. It supplies no saved caller proof. Pi retains its TypeScript current-context and branch checks before sending work over its private Rust pipe. Its process-scoped state and published hc context are specified below. New Codex and Claude hc sources are deferred. Each client retains its existing invalidation loop. The private child (streams serve) maps relevant core facts into lifecycle(binding, NativeChanged { kind }); the mod coalesces notices into saved reads and supplies no identity or move facts. PlacementChanged rereads placement and compares only with this binding's prior positive baseline; DisplayChanged rereads display inputs without membership mutation. Both request fresh reads; event payloads provide only invalidation. The existing invalidation path uses this call without adding watchers or public RPCs.

On a conversation switch, the harness integration stops the old binding and establishes a new one. Outstanding work associated with the old binding is cancelled or discarded under the existing admission rules. The same client process can continue.

A placement event requests a fresh comparison for the same verified conversation. Only an observed move to another space changes membership. It joins that space's associated same-host Stream, or creates/binds a Stream for an unmarked destination. Other conversations keep their existing membership. Preserve participation history and conversation checkout knowledge. Startup, reattachment, record changes and display-only events preserve saved membership. Cross-host/catalog destinations fail before writes. Herdr retains its verified pane-move event; hc compares positive placements across relevant core facts, clearing its in-memory baseline on observation loss. No stored move journal or inferred move across a gap is added. The [existing reconciliation contract](../docs/native-context-reconciliation.md) remains authoritative.

Started evaluates the effective card preference and opens a missing companion when automatic. `NewlyBound` is the observed transition of this verified conversation from no enrolled Session to an enrolled Session, including a confirmed first report or later reread after explicit attachment. It also evaluates the effective card preference and opens a missing companion when automatic, preserving the existing enrollment behavior. `Reattached` establishes the current binding baseline and restores observation without replaying enrollment or repeating startup opening. `Stopping` ends subscriptions and pending work. Connection readiness alone invokes none of these operations.

Pi retains conversation capture, model execution and branch admission. Native views only read saved data and submit explicit actions. A source-change notification selects current state to read; event payloads do not establish current ownership.

#### Native adapter interface

The adapter receives a prepared opening plan. Shared code has already selected Sessions, checked saved identity and resolved directories. The adapter performs native allocation and launch using those inputs. In Rust, native snapshot/action types are associated types of the adapter trait; shared workflows are generic over that trait. The existing built-in selection dispatches to concrete adapters. No dynamic registration framework is added.

A client-only integration can use `StreamsApi` without implementing a native adapter. Provider adapters declare unsupported opening explicitly and return `Unsupported` for projection or navigation they do not implement. The application maps these to explicit operation results.

Native APIs can combine space creation and conversation launch. [CMUX currently creates a workspace with its initial command in one call](../src/runtime/workspace/open.rs). A compulsory `create_space()` followed by `launch()` would change that path. The proposed interface preserves native call grouping and returns execution and presentation outcomes independently.

```text
OpeningSupport = {
    harnesses: NonEmptySet<HarnessId>,
    mode: ReuseObserved | CreateOnly,
    launch_limit: One | Many,
    placement: Independent | RequiresSpace
}

VerifiedLaunch = {
    session: SessionId, conversation: ConversationId,
    transcript: VerifiedPath, argv: List<String>, cwd: VerifiedDirectory,
    environment: ApprovedEnvironment
}

// Newly created terminals use scrubbed ApprovedEnvironment.
// In-place prepare_resume retains the caller environment as specified above.

LaunchItem = Conversation(VerifiedLaunch) | Shell { cwd: VerifiedDirectory }

SpaceDecision =
    None
  | Reuse { space: NativeRef }
  | Create { title: String, cwd: VerifiedDirectory }
  | Unavailable { reason: String }

WantedConversation = {
    conversation: ConversationId,
    locator: Optional<Path>  // already-known saved transcript locator
}

PreparedOpen = {
    stream: StreamId,
    space: SpaceDecision,
    launches: List<LaunchItem>,
    adopt: List<NativeRef>,  // selected already-running terminals to place
    focus: Bool
}

SpaceEvidence = None | Requested(NativeRef) | Known(NativeRef)

PresentationResult = {
    space: SpaceEvidence,
    state: Complete
         | Incomplete { reason: String }
         | Unavailable { reason: String }
         | Unsupported
         | Uncertain { reason: String }
}

OpenEffects = {
    launches: List<Effect<NativeRef>>,  // one per input, in input order
    adoptions: List<{ terminal: NativeRef, result: Effect<NativeRef> }>,
    presentation: PresentationResult,
    viewer_actions: List<ViewerAction>
}

ExecutionQuery =
    Work { stream: StreamId, wanted: List<WantedConversation> }
  | Terminals { terminals: List<NativeRef>, foreground: Optional<NativeRef> }

ForegroundConversation =
    Current(ConversationId) | NotApplicable | Unknown { reason: String }

ViewerAction =
    SelectSpace { space: NativeRef }
  | FocusTerminal { terminal: NativeRef }
  | RenameSpace { space: NativeRef, name: String }
  | RenameTab { tab: NativeRef, name: String }
  | OpenUrl { url: SafeHttpUrl }

ReferenceOpenReport = {
    known: List<NativeRef>,
    result: Applied | NeedsViewer(NonEmptyList<ViewerAction>)
          | NotApplied { reason: String } | Uncertain { reason: String }
}

ResolvedTarget =
    Document { stream: StreamId, document: DocumentKey,
               viewing_session: SessionId, path: VerifiedPath,
               checkout_id: String, checkout_root: VerifiedDirectory }
  | Artifact { key: ArtifactKey }
  | Link { url: SafeHttpUrl }

ProjectionPlan = {
    owner_actions: List<NativeAction>, viewer_actions: List<ViewerAction>,
    warnings: List<String>
}

interface NativeObservation {
    presentation() -> Access<PresentationSnapshot>  // pure lookup
}

interface NativeIntegration {
    opening_support: OpeningSupport | Unsupported

    observe() -> Result<NativeObservation>
    inspect_execution(observed: NativeObservation, query: ExecutionQuery)
        -> Access<ExecutionSnapshot>

    execute_open(plan: PreparedOpen) -> OpenEffects

    // Pure comparison using the retained observation.
    plan_projection(observed: PresentationSnapshot,
                    execution: Optional<ExecutionSnapshot>,
                    desired: DesiredProjection) -> Result<ProjectionPlan>
    apply_projection(action: NativeAction) -> Effect<ProjectionResult>

    open_target(target: ResolvedTarget, at: Optional<NativeRef>)
        -> ReferenceOpenReport
}

interface ExecutionSnapshot {
    presence(conversation: ConversationId) -> Presence  // pure lookup
    verified_terminals(conversation: ConversationId) -> List<NativeRef>
    stream_terminals(stream: StreamId) -> List<NativeRef>  // pure lookup
    foreground_conversation(terminal: NativeRef) -> ForegroundConversation
}

interface PresentationSnapshot {
    associated_spaces(stream: StreamId) -> List<NativeRef>  // retained order
    stream_for_space(space: NativeRef) -> Optional<StreamId>  // pure lookup
    caller_space() -> Optional<NativeRef>  // exact admitted caller, if any
    space_for_terminal(terminal: NativeRef) -> Optional<NativeRef>
}

NameIntent = LeaveUnchanged | Set(String)
CompanionIntent = Keep | Show | Hide
AssociationIntent = Keep | Bind { space: NativeRef }

DesiredProjection = {
    stream: StreamId,
    space: Optional<NativeRef>,  // exact observed projection target
    stream_display: { title: String, goal_short: Optional<String> },
    association: AssociationIntent,
    stream_name: NameIntent,
    sessions: List<{
        view: SessionView, conversation: ConversationId,
        name: NameIntent, companion: CompanionIntent
    }>
}
```

associated_spaces returns all matching published spaces in retained native order. Work opening reuses the first match. hc uses published space order and Herdr preserves its native workspace-list first-match behavior. Zero matches permit creation subject to the opening rules; unavailable observation is not zero. Reference navigation without at instead requires exactly one match and rejects both zero and multiple matches before spawning.

`observe` reads native context once without needing a Stream. Its retained presentation snapshot supplies reverse lookup and terminal placement. `inspect_execution` adds the requested owner-process evidence against that retained native inventory; it does not reread layout, associations or terminals. Space-only reads need no census. Terminals checks source/route process identities at exact PIDs in O(T), including background or suspended Pi; optional foreground must be one of those terminals and adds its group/stopped-state check. It never proves host-wide absence. verified_terminals returns only positively checked locations; projection cannot resolve a conversation from unverified labels. Foreground inspection cannot prove unrelated absence. Work inspection performs the bounded completeness observation needed for replacement. Snapshot accessors and planning remain pure. `--current` resolution and its subsequent opening share the same operation observation; clients cannot supply one.

`ViewerAction` contains only fixed desktop operations, with owner-core references and validated values. `SafeHttpUrl` means an absolute credential-free http/https URL with a host. Owner results and desktop outcomes remain separate; the client runs each reply's desktop requests at most once. NeedsViewer indicates pending desktop work; known owner effects remain visible after a desktop failure. No viewer action contains an executable or filesystem path.

`WantedConversation.locator` preserves Herdr's existing match against reported transcript paths without opening unrelated transcripts. It supplies only matching input. Resume still requires separate validation. PreparedOpen.adopt contains selected, positively verified Running terminals that an Available presentation snapshot shows are unplaced; a work-routing hint is not required. Unavailable presentation forces adopt to be empty. Already-placed conversations stay where they are. Placement never reselects Sessions or adopts every Stream terminal when the user requested only one.

`execute_open` is a native-effect boundary. It cannot read catalog records, choose Sessions, change a command or checkout, discover another destination or retry an uncertain effect. It can allocate native resources and combine calls. Herdr can create a workspace and use its root pane for the first command; CMUX can pass its one prepared command to workspace creation; hc can start terminals independently of app presentation.

`stream_terminals` reports observed terminals carrying this catalog, host and Stream association. It lets an empty Stream reuse an existing headless context. That association identifies work context; exact conversation presence still comes from `presence`.

Each input launch and adoption receives its own outcome, including unattempted items after a failure. Adoption reporting preserves the terminal reference and accepted or uncertain label effects. No failed or uncertain placement is replayed. A space-creation failure prevents launches for a `RequiresSpace` adapter. An Independent adapter can launch new terminals while presentation is unavailable, but cannot infer that existing terminals are unplaced or adopt them. Only new launches receive fresh placement labels on that path. The adapter retains its private native snapshot for the operation, and effect replies supply new references. After requesting new hc placement, the adapter reads its published layout once more before saving the association; already-observed spaces reuse the retained layout. hc requests placement through documented terminal labels, including unknown space IDs. `Requested` preserves that intended key without asserting space creation. Only positive native evidence yields `Known`. That extra read leaves Session selection and live conversation observation unchanged.

The hc opening capability admits only Pi initially. Selected Claude/Codex Sessions receive an explicit unsupported/skipped result, never fabricated absence or an hc launch. Existing adapters keep their supported harness sets. Saved views can still display every Session.

`ReuseObserved` requires current presence evidence before choosing replacement launches. `CreateOnly` describes CMUX's existing explicit single-workspace creation behavior, which has no Stream reuse observation. It preserves that limited contract without reporting unsupported observation as proof of absence. Herdr and hc must use `ReuseObserved` and cannot fall back to `CreateOnly` when presence observation is unavailable.

Shared code constructs `DesiredProjection` from records, naming preferences, card preferences and the triggering action. `Show` can create a missing companion. `Keep` updates or follows an existing companion without reopening a missing one. `Hide` removes the display. A refresh or child reattachment uses `Keep`; eligible startup, first enrollment or explicit opening can use `Show`.

After enrollment of the verified caller or a verified native move, an unassociated current published space receives Bind; a space associated with other work is rejected. hc records the binding through its association store specified below. Herdr preserves its existing workspace metadata projection. A failure remains a native-completion warning after the accepted catalog save. Ordinary reports and read-only refreshes use association `Keep`. Reverse lookup supplies `--current`, workspace enrollment context and sidebar selection from the retained snapshot; it performs no extra observation. A save concerning a different Session cannot borrow the caller's space.

Display inputs are independent of naming switches. Herdr retains `strm_goal_short`, Session status/short recap/state/remaining metadata and the ungated `Stream title: Session title` caption. Construct these per-Session display inputs from addressed records, without roster enumeration. hc supplies saved display content through reads; its terminal labels carry routing and invalidation hints only.

Shared projection preparation supplies the sidebar's requested space, the frame terminal's placement, or the live binding's caller placement as its exact observed space. Space naming requires that target's association to match the desired Stream; no target means no space rename. Bind must name the same validated target. Other same-Stream spaces are left for their own observations, preserving caller-targeted Herdr behavior.

For hc, workspace naming targets that associated app space through `space.rename`; tab naming targets the Session's actual first layout pane through `tab.rename`. Pi session naming stays in Pi; hc terminal slugs are untouched. Use the existing outgoing switches in hc contexts as well as Herdr. Disabled means no rename; displayed saved titles remain available. With no desktop consumer, GUI naming/focus remains pending. A later attached view derives current names afresh, without a publisher or ledger.

The pure planner maps intents onto private native resources and returns owner actions plus fixed desktop requests. Rust applies owner effects; the mod executes desktop requests outside rendering. Read-derived plans contain idempotent names only. Saved read_stream/read_session return empty viewer_actions and do no native inspection. read_frame requests its single terminal with foreground inspection and additionally reads its enrolled Session's Stream record for space naming. read_sidebar point-inspects its observed terminals, with the Follow foreground candidate when present, and derives names for verified Sessions only. Neither performs an absence census. Their ReadReply carries viewer_actions and native warnings; the CLI batches views and collects their actions/warnings once in the shared reply. Projection with no execution evidence can rename the associated space but leaves per-conversation names/companions unchanged with a warning. LifecycleReport preserves activity, companion outcome, desired_pi_name and native warnings. An hc show/hide label acceptance reports Deferred; no presentation attempt uses null. Existing Herdr opened/existing/suppressed outcomes remain intact. Native warnings stay separate from catalog publication warnings. Focus and browser requests require explicit actions. Association persistence and transport follow below.

#### One open request through the interfaces

The shared operation selects and verifies work before the adapter receives native effects.

```text
requested records -> supported/skipped selection -> native observation
                  -> presence classification -> verified launch inputs
                  -> pure opening plan -> native effects -> combined report
```

ReuseObserved admits replacement candidates only for Absent. Running work is reused; Unknown work is unavailable. CreateOnly retains cmux's explicit creation contract. Capability/launch-count and directory checks precede effects. The report preserves skipped/failed selections and each known or uncertain effect.

For hc, if no reusable space exists and both launches and adopt are empty, plan `SpaceDecision::None` and return space None with an explicit incomplete-presentation reason. Do not allocate a Requested key with no terminal to carry its labels. Already-running placed terminals remain where observed and retain their reported locations. A fresh Requested key requires at least one launch or adoption label attempt.

The original selected set controls initial-shell eligibility. Only zero selected Sessions with no reusable space/terminal context can request a shell. All-skipped or Unknown selections cannot become an empty-selection request.

Directory verification also resolves the requested Stream/selected-Session directory whenever the retained observation requires space creation, including when every selected conversation is already running or the Stream is empty. A missing directory fails before native effects; the adapter invents no fallback path.

Unavailable execution observation becomes `Unknown` for a reuse-capable adapter. It cannot become an empty running set. An unsupported multi-Session request is rejected before effects.

```text
SessionOpenResult =
    AlreadyRunning { locations: NonEmptyList<NativeRef> }
  | Launched { location: NativeRef }
  | Skipped { reason: String }
  | Uncertain { reason: String }

OpenReport = {
    stream: StreamId,
    sessions: Map<SessionId, SessionOpenResult>,
    adoptions: List<{ terminal: NativeRef, result: Effect<NativeRef> }>,
    initial_shell: Optional<Effect<NativeRef>>,
    presentation: PresentationResult,
    viewer_actions: List<ViewerAction>
}
```

For Herdr, shared open planning selects an observed workspace or sets `SpaceDecision::Create` in PreparedOpen. `execute_open` creates native tabs and starts the prepared commands, then focuses the workspace. Those resource operations stay inside the adapter.

For hc, `execute_open` starts prepared terminals independently and requests app placement with routing labels. The core accepts those labels even while the app is closed. The app adopts unknown placement IDs when connected. The reply distinguishes requested placement, positively observed space/binding and pending desktop actions from execution. No unobserved label intent becomes a saved space association. Desktop failure cannot cause another launch.

#### hc public CLI results

Herdr/cmux keep their existing OpenResult JSON. The new hc backend's HcRuntime value has shape `{"backend":"hc","report":<OpenReport>,"needs_attention":<List<String>>}`. streams open --frontend hc returns `{"stream_id":<StreamId>,"runtime":<HcRuntime>}`; Auto selecting hc uses the same shape. streams start preserves its existing catalog-save fields and places that same hc runtime value in runtime. A catalog-only creation has no runtime. No workspace_id is invented when space evidence is None; Requested and Known keep their tagged exact NativeRef in report.presentation. The report retains each adoption and the nullable initial_shell effect.

Build needs_attention in Session-ID order from Skipped/Uncertain Session results, then input order for non-Applied adoption/initial-shell effects, then any non-Complete presentation and pending viewer actions. AlreadyRunning, Launched, absent initial_shell and Complete with None are successful outcomes. Include the exact bind-space remedy for requested/unbound placement as specified above. The owner CLI has no desktop consumer, so emitted viewer actions remain pending. Normal text output uses the same ordered reasons and known references.

Exit 0 means a returned report with empty needs_attention; exit 2 means a returned report needing attention. Existing preflight/transport error handling remains unchanged before a report or accepted save exists. A successful catalog save followed by failed opening keeps its receipt, completion_error and exit 2. No output conversion discards completed effects or resubmits an action. Stage 3 CLI tests freeze None, Requested and Known runtime shapes, adoption uncertainty, initial shell, saved-start/follow-on-failure and unchanged Herdr/cmux fixtures.

#### Navigation and refresh callbacks

A native view's callbacks make application calls. Rendering uses cached view data.

```text
on_refresh():
    coordinator_for(scope, endpoint).refresh(this_copy.view_selection)
    // Batched ViewSelection reads update this copy's cache asynchronously.
    // Frame and Sidebar use their context reads, not saved-ID shortcuts.

on_document_click(document_key):
    activation = capture_view_selection_from_cached_ui_state()
    streams.open_reference(Document {
        stream: selected_stream,
        document: document_key,
        viewing_session: activation.session
    }, at = activation.space, context = activation.context)

on_expand():
    show_popup(cached_session)

on_view_stream():
    select_sidebar_stream(cached_session.stream, cached_session.id)
```

The thin-mod transport specified below supplies refreshes. Expanding cached detail causes no process start or model request. The UI performs no checkout resolution, catalog mutation or native discovery while drawing.

#### Failure and cancellation contract

A client operation returns success, an accepted save with warnings, or an error. Successful responses can contain unavailable or uncertain native outcomes. An accepted catalog save remains accepted if later presentation fails.

Cancellation before an effect prevents it from starting. Running work checks cancellation at its next safe boundary. Completed saves and effects remain completed. Lost replies are reported as uncertain and are not automatically replayed.

## Alternatives and design limits

Short CLI calls reuse the existing Rust runtime on the admitted local or remote owner. A small desktop dispatcher performs fixed UI operations. A persistent UI service would add startup, authentication and reconnect ownership. Watched display files would need another publisher for agent-free Streams and remote delivery. Neither is part of this design.

The Streams-owned hc document keeps associations with the core's spaces. A host-local file would require another native mapping and independent lifetime handling. One retained-version conditional write is sufficient; concurrent edits fail explicitly. Layout history, merge loops and compensating native actions are excluded.

Shared interfaces model existing Herdr/cmux behavior and proposed hc behavior. They add no plugin loader or speculative harness implementations. Distinct unknown presence and uncertain effects are required to prevent duplicate launch and replay. Caller-facing results report saved preferences, native availability and uncertain effects separately.

For Pi identity, the selected baseline is one in-memory current context checked at SDK callbacks/request boundaries, with a reset on ID change and rejection of obsolete asynchronous results. A live helper or an old context's cached ID cannot establish current identity. The earlier process-lifetime proof and transition guard are superseded. OSC titles/status and transcript searches also fail to establish that identity. New/fork/resume behavior and the accuracy of its published native context remain open implementation checks; their uncertainty does not authorize Pi changes or pre-emptive recovery machinery.

Saved sidebar reads decode O(N) Session records on the admitted host; replies above 4 MiB fail without pagination. Work presence observes O(P) owner-host process metadata, bounded to 65,536 entries and 4 MiB. One unresolved possible interactive Pi under the core can block otherwise-absent Pi replacements across Streams; positively verified Running results remain usable. Space-only reads avoid that census. Point inspection uses exact PIDs in O(T); Follow uses its single foreground candidate. With M live helpers, a relevant layout change can cause M owner layout-document reads, separate from mod refreshes. Measure that O(M) fanout and bytes during acceptance. All spaces on a core share the 1 MiB association document, so unrelated concurrent binds can conflict and fail without retry.

Terminal labels are untrusted hints, never command/path/identity authority. Catalog and endpoint admission, verified owner processes and fresh reference resolution remain separate checks. Web links allow credential-free http/https only. Logs allow bounded operation/scope/reference metadata and never contents, commands, environments, URLs, payloads or raw stderr. Machine-route replacement and native layout reset require stopped clients and the written maintenance procedure.

The first release favors explicit errors for unsupported or ambiguous conditions. It adds no automatic repair after partial native completion. An abandoned read may overlap a newly opened view's read; logging may duplicate or lose entries. Those outcomes require no recovery bookkeeping.

## Detailed Implementation Plan

### Owner execution and desktop presentation

The owner runs Rust against its admitted catalog, filesystem, processes and hc core. The desktop runs the native app and mod. A local installation uses the same boundary with no remote hop. Existing configured `exec(..., { on })` routes carry short owner calls. No reverse connection or persistent UI service is added.

Each machine's `herdr-app` document stores that machine's spaces, tabs and splits. The app forwards reads/writes to the selected machine. Owner Rust reads that local-core document and writes its separate `streams` document. Desktop focus and window state remain app-local. The [pinned layout synchronization implementation](https://github.com/herdrdev/herdr-app/blob/9950887361af3ba218b9ce7a347cb8ec7b66c71d/src/mirror_sync.rs) establishes this split.

A configured execution route pairs the admitted `WorkScope` and permanent owner core with an existing viewer machine alias, or no alias locally. Requests retain that route through completion. Owner Rust checks its actual hello endpoint against native references on each operation. The desktop qualifies an owner resource only with the captured alias prefix; decoding removes only that exact prefix. An alias supplied by a label, saved title or reference never selects a route. Changing/removing an execution route requires stopping in-flight clients first, as native layout reset does. A changed execution route invalidates pending display/projection results and never retargets submitted work.

#### Opening through native placement labels

The documented `herdr.space` and `herdr.tab` labels place terminals in existing resources, creating a new space/tab when the label names an ID absent from the layout when the app adopts them. hc opening uses this API. For a new space, Rust allocates a fresh opaque owner-local key and requests that placement at terminal creation. For every newly launched or selected unplaced terminal, set herdr.space to the chosen space key and herdr.tab to a fresh UUID tab key allocated for that terminal. The initial shell and owner pager terminals follow the same rule. This requests one new tab per terminal; no caller's existing tab is selected implicitly. Already-placed terminals stay put. An empty selection can request its one initial shell; adopting running work needs no bootstrap shell from `space.new`.

The app can adopt those labels while connected or after reopening. Before publication, the space result is `Requested`; no association is saved. Positive observed native evidence changes it to `Known`. One additional publication read and the binding rules below apply. App closure cannot undo terminal creation or placement labels. Source inspection supports this mechanism; local and remote closed-app adoption still need live acceptance.

#### Fixed desktop operations

A fresh owner reply can contain the canonical `ViewerAction` cases. The desktop checks reply scope, echoed requested endpoint, execution route and source generation before dispatch outside rendering. Native focus/rename/select also requires verified_endpoint matching the configured endpoint and every NativeRef. OpenUrl requires the admitted scope, matching requested endpoint and a fresh safe URL; it can work without a native connection. A saved-only reply can likewise have no verified endpoint. Any contradictory verified endpoint is rejected. Native focus/rename/select uses the mod SDK's in-process herdr.run with fixed command names and structured objects, after checking live machine layout readiness and the qualified target's existence. Space and tab references are qualified for that viewer only. Each checked reply records accepted dispatch; it does not prove later document persistence or visible paint. A failed/lost reply is not resent.

Browser opening uses the declared `Context.open_url` on the desktop. Both ends reject non-http/https URLs, missing hosts and credentials. The URL comes from fresh owner-side saved-link validation. The desktop runs no owner-supplied executable, shell, environment or file path.

The coordinator executes a fresh reply's desktop batch once. Saved-read replies may supply only idempotent name projection; cached view data excludes those requests. Explicit action reports carry their own focus/browser requests. A read or reload never replays focus or browser requests. The owner result and each desktop outcome stay separate; known owner terminals survive a failed focus/rename/browser step. A closed initiating view cannot cause a launch to be repeated or a new window to be selected implicitly. With no available desktop consumer, GUI completion is reported pending/unavailable.

#### Documents and artifacts

hc document and artifact views use owner-host terminals placed in the freshly verified calling space. This applies locally and remotely. Artifacts use the existing exact-key secure pager. Add a small `integration view-document` CLI path that rereads the saved document identity, viewing Session and working-copy resolution before using the same secure pager. Its admitted catalog/scope expectation is passed explicitly. Extract only the required pager helper from artifact view; retain `LESSSECURE=1`, empty LESS and cleared LESSOPEN/LESSCLOSE. No content passes through the mod reply.

Reference viewers use prepared argv, `no_parent: true`, `rm: true` and the verified `herdr.space` at creation. A missing, unpublished or wrong-core destination fails before starting an unplaced viewer terminal. Herdr keeps its existing workspace-tab document editor. Web links open on the desktop with no owner pager. This introduces no remote file URL or editor configuration.

#### Names and observed moves

Rust computes name intents from current records/preferences and the retained layout, including actual first-pane eligibility. The mod applies fixed native rename requests after a fresh response, only when the native value differs. Disabled switches preserve native names. With the app closed, Pi naming and saved reports continue; GUI names are derived when a view next attaches. Saved field reads remain pure and rendering executes no native command.

The private Rust child observes relevant existing hc terminal/document facts. Each live source owns one subscription client. Its ordinary transport heartbeats establish no identity and trigger no record read. A relevant layout fact requests one bounded layout read; comparing the exact bound terminal's last positive space with its fresh space supplies hc move detection. Startup, reattachment and loss establish/clear that in-memory baseline without moving membership. A fresh verified move invokes the existing record-local membership operation, then updates routing and association once. Accepted membership changes survive later native failure. Event payloads alone cannot authorize a move.

A shared core-document change can require one layout read per live source. Filter unrelated fact kinds in memory and measure that fanout, document bytes and native calls during acceptance. No shared observer service or retry machinery is added to hide the cost.

#### hc event subscription wire

The source opens a separate pinned owner-local socket, performs hello with framed false, verifies the same permanent endpoint, then sends `{"jsonrpc":"2.0","id":2,"method":"events.subscribe","params":{}}`. Empty params are unfiltered; terminal, children_of and labels filters are omitted because they suppress document events. After the matching response this plain connection carries notifications only. Ordinary RPCs use the operation connection.

The [hc 0.1.21 subscription handler](herdr-app-evidence/hc-v0.1.21-api.rs), [notification encoder](herdr-app-evidence/hc-v0.1.21-events.rs) and [plain stream loop](herdr-app-evidence/hc-v0.1.21-server.rs) define these shapes. Angle-bracketed names denote typed JSON placeholders.

```text
{"jsonrpc":"2.0","id":2,"result":{"seq":<u64>,"terminals":<List<Terminal>>,"channel":null}}
{"jsonrpc":"2.0","method":"event","params":{"seq":<u64>,"type":"doc","data":{"name":<String>,"version":<u64>}}}
{"jsonrpc":"2.0","method":"event","params":{"seq":<u64>,"type":<TerminalFact>,"terminal":<String>,"data":<JSON>}}
{"jsonrpc":"2.0","method":"heartbeat","params":{"seq":<u64>}}
{"jsonrpc":"2.0","method":"missed","params":{"seq":<u64>,"terminals":<List<Terminal>>}}
```

TerminalFact covers created, lost, restarted, keep, finished, removed, status, turn, title, labels and size. Terminal data can contain changed fields or an added record; Streams treats it as opaque invalidation data. Only facts for the exact bound terminal and local doc changes named herdr-app or streams request relevant fresh observation. Forwarded doc events carry a machine string; ignore those and terminal addresses other than the exact owner-local target. Neither data nor the initial terminal snapshot supplies conversation or movement authority.

seq is a core-wide unsigned sequence, initially retained from the subscription result. Accept increasing event sequences; gaps alone prove no missing fact because other events can be filtered. Heartbeats can repeat the current seq and trigger no read or freshness claim; their seq does not replace the last-event baseline. missed replaces the terminal snapshot and clears the prior placement baseline before a fresh layout observation; that observation establishes a baseline without inferring a move. Disconnect, malformed relevant envelopes, regressing sequence or a framing-bound violation makes observation unavailable and clears the baseline. Unknown fact kinds/extra payload fields are ignored. Explicit lifecycle reattachment can establish a new subscription; this adds no automatic reconnect/replay loop.

### Native association storage

The Streams-owned hc document holds space-to-Stream association metadata that survives the last terminal. Rust is its sole reader and writer in this delivery; the app accesses it through the Streams interface. The catalog owns work records and the app owns layout. Each record has a separate lifetime.

The association identifies the catalog source, host, Stream and native space at an exact endpoint. For hc, that endpoint includes the core's permanent identity so recycled space identifiers on another core cannot match. Each space has one associated Stream. An adapter validates the current native space before using the association.

```text
SpaceAssociation = {
    catalog: CatalogSource,
    host: HostId,
    stream: StreamId,
    space: NativeRef
}
```

The association contains no conversation identity, launch receipt, card content or command authority. A space observed absent is immediately ineligible for reuse; explicit native opening cleans its stored row. Closing an agent, losing an app connection or failing an observation leaves the row intact.

hc uses the separate `streams` document through the [existing document API](herdr-app-evidence/hc-schema.json). Herdr retains workspace.report_metadata under source streams, with strm_catalog, strm_host and strm_stream tokens, behind the same observation interface. See [native marker handling](../src/runtime/native_markers.rs). Resetting app layout also requires the written association-reset step; resetting only one document can leave stale references.

```text
App -> Streams interface -> Rust -> hc doc.get / doc.set
                                     -> Streams association document
```

#### Verified document concurrency

The [hc 0.1.20 document handler](https://github.com/herdrdev/herdr-core/blob/v0.1.20/crates/herdr-core/src/api/doc.rs) holds its state mutex across the version check, update and persistence. A stale version returns conflict before changing the document. A pre-rename persistence failure restores the previous in-memory value and version. hc holds the JSON document in memory and persists it.

An association operation retains the version from its initial read and makes one conditional doc.set, or no write when values are equal. It uses no second association read, local coordination lock, merge loop or automatic retry. The detailed sequence below defines that commit. Concurrent edits to different rows can conflict because they share the document and its 1 MiB limit.

Space placement and association persistence are separate effects. A conflict confirms this association write changed nothing; a failed or lost reply can leave its effect uncertain. Either outcome preserves completed native effects and their exact references, reports partial completion and stops affected work. No automatic space recreation, compensation or mutation replay follows.

### Association update and removal

A new association is saved only after its space appears in the core-owned layout. Every saved association has positive publication evidence. Cleanup can remove rows for currently absent spaces using a single snapshot, including after a restart. It needs no prior layout history.

#### Published space evidence

The app can return a created space before publishing it in the core's layout document. Its layout writes run in a background task. The app's layout-ready flag describes availability and does not establish that publication finished.

A published layout is a successful read of the herdr-app document from the expected permanent core identity, decoded using the supported layout format. A successfully read null value represents an empty layout. A failed read, malformed value or different endpoint supplies no absence evidence. Document calls use the expected core connection. Connection failure ends the attempt without retargeting another core.

```text
DocVersion = u64

PublishedLayout = {
    endpoint: EndpointId,
    spaces: List<PublishedSpace>,
    space_keys: Set<String>  // derived from spaces
}

PublishedSpace = { key: String, name: String, tabs: List<PublishedTab> }
PublishedTab = {
    key: String, name: String,
    panes: List<PublishedPane>,
    floating_terminals: Set<String>
}
PublishedPane = Terminal { key: String, terminal: String }
              | Mod { key: String }

AssociationDocument = { version: 1, spaces: Map<String, SpaceAssociation> }

AssociationSnapshot = {
    endpoint: EndpointId,
    version: DocVersion,  // hc compare-and-set token
    rows: Map<String, SpaceAssociation>
}
```

Decode the app-owned subset shown by the [pinned mirror types](herdr-app-evidence/herdr-app-mirror.rs) and [layout node types](herdr-app-evidence/herdr-app-layout.rs).

- Match missing-field defaults. Missing spaces and tabs mean empty arrays; missing names and Node.id mean empty strings; a missing tab root is an empty row with id empty. Thus `{}` is an empty document and a named tab without root is empty.
- Read spaces[].id/name/tabs[] and each tab's id/name/root. Reject empty space/tab IDs or terminal strings, wrong field types, and missing required Node.type/children/content in a supplied node.
- Match Node.panes() depth-first order. Row, column and stack traverse children in array order, including hidden/inactive children; block content.terminal and content.mod identify panes. A leading mod pane prevents a later terminal from claiming first-pane eligibility.
- Walk every root descendant's Node.floating and collect floating[].content.terminal separately for placement, excluding floats from pane order. SharedTab.rest.floating is a distinct array of `{pane: Node, rect: [...]}` from tab UI state. The adapter ignores it, matching the pinned mirror's shared placement traversal.
- Context content with tab or its context alias, and Agent content with agent, are known non-pane shapes and contribute no pane. Ignore unrelated app-owned fields; do not apply deny_unknown_fields to this document.
- Malformed required structures, unsupported required variants or conflicting placements return unavailable. The normalized lists derive names, first-pane eligibility and terminal placement without further native reads.

The keys are unqualified hc space IDs. Each association's NativeRef must match its map key and endpoint. The Streams-owned document holds the version-1 envelope and these rows; the compare-and-set version belongs to hc. A null association document decodes as an empty map. Invalid data or an unsupported envelope version returns an error and remains unchanged.

#### Read, open and bind

The hc adapter reads the association document before reading the published layout in its opening observation. It retains both snapshots. Shared code selects the requested work using that observation and verifies its inputs as already specified.

The adapter performs the prepared native opening once, preserving native call grouping. hc requests placement through terminal labels; other adapters can combine space creation with the first launch. Successful execution effects and references are retained before association work. An uncertain affected native result stops that work without binding or cleanup.

For a reused space, the retained published layout supplies positive evidence. After requesting new hc placement, read publication once more. Missing publication returns `Requested` plus completed launches, with no association write. A positive publication yields `Known`, including when the later association write fails. No wait, repeated placement or launch follows.

After that check, the adapter prepares one map replacement. It removes rows absent from the retained published layout and includes the requested binding. A new space uses the later publication snapshot for that comparison. An occupied space belonging to another Stream or scope is rejected; binding does not reassign it. An identical binding is left unchanged.

```text
finish_associations(
    before: AssociationSnapshot,
    published: PublishedLayout,
    wanted: Optional<SpaceAssociation>
) -> Effect<Unit>:
    if before.endpoint != published.endpoint:
        return NotApplied("layout belongs to another endpoint")
    // Snapshot decoding has already checked row keys and endpoints.

    if wanted exists:
        if wanted.space.endpoint != published.endpoint:
            return NotApplied("space belongs to another endpoint")
        if wanted.space.key is absent from published.space_keys:
            return NotApplied("space is absent from the published layout")
        existing = before.rows.get(wanted.space.key)
        if existing exists and existing != wanted:
            return NotApplied("space is associated with other work")

    desired = { key: row for (key, row) in before.rows
                if key in published.space_keys }
    if wanted exists:
        desired[wanted.space.key] = wanted
    return commit_associations_once(before, desired)

commit_associations_once(before, desired) -> Effect<Unit>:
    if desired == before.rows:
        return Applied(Unit)  // requested state already held when read
    if cancelled:
        return NotApplied("cancelled before association write")

    match hc.doc_set(streams_document, before.version,
                     AssociationDocument { version: 1, spaces: desired }):
        success:             return Applied(Unit)
        conflict:            return NotApplied("association conflict")
        confirmed_rejection: return NotApplied("association write rejected")
        effect_unknown:      return Uncertain("association write unconfirmed")
```

The association version always comes from the retained read. A concurrent map change makes the replacement fail its precondition, including a change to an unrelated row. There is no re-read-and-merge loop. Binding and cleanup share this one write; an opening that leaves the map unchanged performs no write.

The endpoint and key checks preserve exact native addressing. A map row supplies work context only. Conversation identity and permission to affect a terminal continue to come from the verified harness source.

#### Explicit binding after partial presentation

When a requested or created space has not yet published, the partial result includes its exact intended native reference and a shell-quoted `streams integration bind-space --space-ref <json> --stream <id> --scope-json <WorkScope-json>` remedy. These are the exact required clap flags. Scope is an expected-identity assertion against the command's configured catalog and host; it does not select or discover another catalog. Existing open/start commands gain no catalog/host flags. After publication, this explicit command reads the association document and current published layout at that endpoint, validates the requested saved Stream, and runs `finish_associations` once with that binding. An occupied space is rejected; an identical binding is a no-op. An unavailable layout or conflict returns an error. The command creates no space, launches no conversation and schedules no retry. The user can then Refresh the view.

This is a new observation and binding action. It also works after an uncertain association reply because the current matching row supplies the no-op result. That exact reference is the input; no search reconstructs which unbound space belonged to an earlier request. Fresh publication is required even when the earlier result was only `Requested`. Once bound, closing the last terminal leaves the association intact.

#### Cleanup lifetime and cost

Explicit native work opening and explicit binding perform this cleanup using current published state. First enrollment and verified native moves can bind the caller's unassociated published space with the same operation; ordinary reports do not write hc associations. The first opening after a restart can remove rows for spaces deleted while Streams was stopped. A cleanup-only update uses the same retained snapshots and conditional write, with wanted absent.

A removed space is immediately ineligible for reuse when observed absent. Its stored row may remain until the next explicit opening. Catalog reporting, saved-view reads and rendering leave the association document unchanged. Cleanup adds no watcher, timer, retained layout history or separate service.

A headless opening can clean up using hc's published layout while the app is closed. It creates no space association for a space that does not exist. Its execution and presentation results remain independent, and terminal routing metadata continues to identify the Stream for later presentation.

Each opening reads the association document and published layout once. Requesting new placement adds one publication read. The association stage makes at most one conditional write, whose value remains subject to hc's 1 MiB limit. Malformed or unavailable layout data prevents cleanup. A failed cleanup leaves rows stored and reports its failure; it schedules no retry.

Native ID reuse during an observation gap cannot be reconstructed from current state. Native layout reset or restoration is an offline maintenance operation that clears the generated association document before clients resume. Runtime code does not infer old lifetimes or migrate old state.

#### Offline association reset or restoration

This is operator maintenance for a deliberate native layout reset/restore. It clears all Streams associations on the intended core, across scopes, while leaving catalog records unchanged. Runtime code performs no reset or recovery.

1. Stop the affected native views and Streams/Pi sources before changing layout. Use a plain maintenance terminal and keep clients stopped until both layout maintenance and this clear are confirmed.
2. Run hc connect --session <intended-session> on the owner. On that single connection send the hello request below and compare result.core.id with the intended permanent core ID. Stop on a mismatch; do not search for another core.
3. Read the streams document and copy its returned version. An already-null value needs no write. Otherwise replace N below with that exact integer and send the conditional null write once.
4. Confirm the write result and read the document again. A conflict requires inspecting current value/version before deciding on a new manual clear. After a lost reply, reconnect, verify the core and inspect the document. Do not blindly repeat the write.
5. Complete the separate native layout reset/restore and confirm both states before resuming clients. Reestablish required associations with explicit open/bind operations. Names or old source labels do not reconstruct them.

```text
{"jsonrpc":"2.0","id":1,"method":"hello","params":{}}
{"jsonrpc":"2.0","id":2,"method":"doc.get","params":{"name":"streams"}}
{"jsonrpc":"2.0","id":3,"method":"doc.set","params":{"name":"streams","version":N,"value":null}}
{"jsonrpc":"2.0","id":4,"method":"doc.get","params":{"name":"streams"}}
```

The maintenance instructions in docs/hc-herdr-app-integration.md must include these requests and the conflict/uncertainty rule. Verify them on an explicitly approved disposable core; the earlier storage probe supplies no current-version acceptance.

#### Partial presentation results

PresentationResult keeps space evidence independent from completion. `Requested` retains an intended key without asserting resource existence. `Known` retains a positively observed space even if binding fails. The primitive Effect type describes individual effects; per-launch results retain each conversation's outcome.

A lost association reply preserves the known space with an uncertain outcome. A lost native creation/placement reply can leave only requested or no space evidence. Existing launch results identify every confirmed terminal and any unattempted or uncertain launch. `viewer_actions` are separately pending desktop work; owner-side completion never establishes that they ran. Clients show desktop completion only after their separate results.

Creation can already have started conversations or shown the space. Failure returns the accumulated results and stops remaining affected presentation work. It never rolls back a catalog save, kills a successfully started conversation, repeats a native effect or deletes the created space as compensation.

Cancellation before an effect prevents that effect. Cancellation after creation retains its known references. A completed association save remains saved, and cancellation stops later work at the next safe boundary. The caller can show what completed and what needs inspection.

#### Required verification

Tests must inject outside changes through fake native and document APIs and assert the exact calls and returned results.

- Requested placement absent from publication leaves associations unchanged and returns `Requested` plus completed launches. A confirmed native creation can retain `Known` even before publication. Later publication followed by explicit binding makes no create/launch call; its association survives last-terminal closure.
- A newly published space is bound with one conditional write; an unchanged binding produces no write.
- A terminal closing leaves its association intact while the space remains published.
- Cold-start cleanup reads associations before layout and removes only rows absent from a valid layout at the same endpoint.
- A map update after the retained read causes a conflict and preserves the concurrent writer's value. No retry or native recreation follows.
- An unavailable, malformed or wrong-endpoint layout causes no deletion.
- An occupied space is left unchanged. Reuse requires the matching Stream and scope.
- A lost combined create/launch reply causes no fallback launch. A lost association reply retains known native references and reports uncertainty.
- Cancellation before writing makes no write. Cancellation after a confirmed save preserves that save.
- A headless opening preserves successful execution when presentation is unavailable; reads and rendering perform no cleanup writes.

The required verification covers the implemented integration. The current evidence establishes component behavior only. The linked hc 0.1.20 document-store checks cover ordinary restart and a pre-rename persistence failure. Re-verify them on the installed target version during acceptance. Power-loss durability is unverified. The publication ordering comes from herdr-app 0.1.15 source inspection. Production contract tests and live integration lifecycle tests remain required.

### Pi conversation binding in hc

The Pi extension keeps one current integration state in memory for the Pi process. Relevant SDK callbacks and requests read the Session ID from their SDK context. An unchanged ID reuses a still-live current state. A changed ID retires the previous Session's state and initializes the new Session. Normal `/new`, `/fork`, `/resume` and startup `pi --resume` remain supported. Pi and its installed dependencies stay unchanged.

This is the selected starting implementation, with the open lifecycle question below. It adds no Pi transition lock, input restriction, recovery coordinator or new identity daemon. The implementation will exercise ordinary transitions and report concrete trouble before adding mechanisms.

#### In-memory Session state

Keep the current state at process scope so an obsolete extension instance cannot maintain an independent authoritative cache. A local generation distinguishes an old request from later work for the same Session after switching away and back.

```text
PiContextState = {
    conversation: ConversationId,
    generation: u64,
    session_file: Optional<Path>,
    helper: Optional<StreamsChild>
}
```

Apply these state rules at admitted boundaries.

- Read the SDK Session ID before using cached integration state. On a change, install a new state/generation, mark the old state retired, stop its helper and clear Session-specific native context.
- Shutdown retires only its captured state when it still owns the current slot. An old shutdown callback cannot clear newer state or promote its old context. The next start initializes a new generation even when reload preserves the Session ID.
- Capture ctx.sessionManager.getSessionFile() as session_file. A changed path with the same ID updates the candidate without a new generation. Injected contexts or services may omit the getter and supply null; production uses the SDK getter.
- Initialize the helper for the observed ID only while that state is current. The first start callback, or first request if no start callback has run, may initialize from its current SDK context.
- Native readiness requires the observed ID to own the current state, ready helper transport and a completed hc registration attempt through session_started or restart attach. Register source context before checking enrollment. A failed registration leaves native access unavailable and catalog requests available.
- Concurrent native requests during initialization receive unavailable. Catalog requests keep their existing lazy helper behavior.
- Production registration uses the process-owned state holder. Registrations with independent service/cli dependencies own independent holders. Lifecycle tests explicitly share a holder when simulating old/new extension instances, keeping repeated registerWorkCard command fixtures isolated without a test-mode flag or synthetic session_start.

Capture the state/generation with asynchronous work. Before accepting its result, compare it with the current state. An obsolete result cannot update the new Session's cache, apply a report to another Session, publish current context or dispose the newer helper. Existing captured-target and branch-admission checks remain in force. Accepted saves and native effects retain their original outcomes.

Retain the bounded restart policy and correct the helper cleanup. Change child.ts and index.ts to perform retirement cleanup in try/finally and await actual close. The current child clears its process handle and restarts immediately after kill; that is not the required behavior. Retain a dying ChildProcess and its close promise until close fires. Neither a replacement start nor stop() resolution may overtake that close. Keep the existing one-restart budget; do not replay requests. A helper-only restart retains the bound conversation and sends attach (mapping to `LifecycleEvent::Reattached`) without reopening a dismissed companion. Pending requests are not replayed. These are extension resource rules; they make no guarantee that Pi serializes its own transitions.

#### Native source and process checks

An interactive empty or in-memory Pi can register native context before enrollment. Keep the existing Herdr transcript/hasConversation gate unchanged. hc registration requests no model report and creates no catalog record. Automatic-opening preference remains separate from identity.

The extension publishes its selected context through its existing private helper. Standalone hc readers consume that native snapshot; they cannot query the private stdio pipe or directly read SDK memory. Record exact process keys for route/liveness validation and PID reuse detection.

```text
PiChildStart = {
    conversation: ConversationId,
    sdk_pid: u32,
    session_file: Optional<Path>
}
PiRequestContext = {
    conversation: ConversationId,
    session_file: Optional<Path>
}

ProcessKey = { boot: BootId, pid: u32, started: KernelStartValue }

PiSourceClaim = {
    version: 1, endpoint: EndpointId,
    conversation: ConversationId,
    terminal_root: ProcessKey,
    sdk: ProcessKey,
    helper: ProcessKey
}

source_key(helper) = "streams.pi." + digest(canonical_encode(helper))
```

ProcessKey uses exactly the fields boot, pid and started. pid is a positive JSON u32. boot and started are strings. On Linux, boot is the lowercase kernel boot_id UUID and started is `linux:<starttime-ticks>` from /proc/<pid>/stat. On macOS, boot is the lowercase kern.bootsessionuuid and started is `macos:<pbi_start_tvsec>:<pbi_start_tvusec>` from native process metadata. Decimal components have no leading zeros except zero. Failure to read these facts is unavailable. All process-key collection happens on the owner.

Start the private child with serve --session-id <id> --sdk-pid <u32> --plugin-root <path>. Add sdk_pid as an optional clap field for existing non-hc serve compatibility; hc registration requires it and validates the SDK/helper relationship. Rust verifies boot/start facts. Herdr response shapes remain unchanged.

For an hc-selected lifecycle request, session_started and attach return `{"backend":"hc","report":<LifecycleReport>}`; open_card and close_card return `{"backend":"hc","report":<DisplayReport>}`. Angle-bracketed names denote typed JSON placeholders. Protocol-2 attach maps to `LifecycleEvent::Reattached`. LifecycleReport.native uses `{"kind":"active"}` or `{"kind":"inactive","reason":<Reason>}`, where Reason is one of "outside_frontend", "not_enrolled", "unavailable" or "stopped". The report retains nullable companion/desired_pi_name and warnings.

A registered but unenrolled source returns "not_enrolled" without a display attempt. Successful hc show/hide label acceptance reports "deferred". Disabled automatic opening stays suppressed and Keep does not become Show. Only legacy callers outside both frontends receive the unwrapped `{"native":"inactive","reason":"outside_herdr"}` acknowledgement; an hc-selected request retains its hc envelope even when native completion is inactive. native-rust.ts normalizes hc reports for existing native callbacks; child.ts validates them without altering Herdr branches.

After validating its admitted SDK/terminal route, serve writes one JSON-string claim at source_key(helper) on that terminal. The conversation comes from the extension's current state at startup; Rust supplies/verifies process keys. Ordinary request frames do not repeat process keys. Add optional `pi_context: PiRequestContext` to protocol-2 request envelopes, absent for existing non-hc callers. hc requests admitted from the SDK carry the captured conversation and nullable session_file, including session_started/attach, report/save requests and the final pre-EOF sync. The helper rejects a context whose conversation differs from its bound startup ID; it never retargets a save. This metadata remains outside EnrollmentRequest and DomainObject. Before retaining a supplied file, call the public `conversations::conversation_file(harness, native_id, Some(path), &ConversationRoots::default())` wrapper, which verifies that exact locator without discovery. An absent candidate skips this call; never pass None to that resolver. ENOENT before first persistence leaves no locator and does not fail registration; other validation failures are native-completion warnings, preserving accepted catalog saves. A subsequent admitted request verifies its newly supplied exact candidate; equality is a no-op. No failed mutation is replayed. Metadata and paths are not published in labels or diagnostic logs. The claim is immutable for that helper. Retirement removes only its own key and stops/reaps that helper; a replacement gets its own key. A dead helper's residual label is inactive. Source-key/value/endpoint disagreement or malformed candidate data is unavailable, never silently ignored as absence.

The child retires its hc source explicitly on normal stdin EOF. Add an internal `NativeWork::Retire` message with an acknowledgement in src/cli/serve.rs. Stop admission and cancel outstanding request work, then queue retirement on the native worker before cancelling its shared stopping token. Retirement uses a fresh cancellation/deadline, bounded by the existing five-second SHUTDOWN_GRACE, to remove only source_key(helper) once and stop its subscription. Wait for its acknowledgement or deadline, then cancel the worker token and complete the bounded join. Drop performs no native I/O. A timeout or failed deletion reports unavailable/uncertain cleanup and still exits; it never retries or deletes another source. Forced process death can leave an inactive residual label. Test cleanup ordering with an already-cancelled request token, retained worker token, deadline expiry and replacement-source preservation. No public shutdown request variant is added. The final pre-EOF sync request carries captured pi_context; EOF retirement uses the helper's bound source key.

Rust checks the expected permanent core, terminal root and SDK/helper parent relationships on the owner host. HC_TERMINAL_ID remains a lookup hint. Process observations validate the route and liveness; they cannot establish the current conversation by themselves. Source cleanup affects only its own native key and never erases a replacement source.

For the starting standalone baseline, an interactive Pi with one route- and live-ProcessKey-verified claim supplies Running evidence for its published conversation, including while backgrounded or suspended. A live interactive Pi with zero or competing claims is Unknown. Foreground Current is a separate check. Compare the live SDK process's OS process-group ID with the retained core terminal.foreground.pid, which identifies the foreground group leader, and require a non-stopped SDK. A positively observed foreground shell remains NotApplicable even when a background Pi is Running. Missing/changing group facts or competing foreground SDKs make the frame unavailable. Check group membership on the owner host with the process keys; never infer foreground ownership from ancestry or claim liveness alone. Process existence alone supplies no identity. The SDK-selected publication supplies the conversation; process checks validate its route and continued liveness.

This baseline depends on the extension retiring/updating its publication when context changes. It is not an independent proof that a surviving helper still describes Pi's active runtime. Even one stale orphan can be wrong; retain that case in the implementation checks below. No fresh SDK IPC service or transition guard is added. Native views retain last saved data with stale/unavailable status when observation fails; identity-dependent effects then fail explicitly.

For work-presence completeness, collect one bounded owner-host process inventory against the operation's terminal inventory. Align installed launcher verification with lab-cli.mjs's exported resolveSdkEntry. Recognize the static aube-bin-shim v2 target without evaluating its shell and verify the package name and dist/bundle/cli.js or dist/cli.js entry. Direct Node profiles accept that verified script, with an optional `--` separator and the leading Node flags --no-warnings and --enable-source-maps; other loader/Node options are Unknown. The initial native target is Pi 1.5.2; an unverified package/version is Unknown.

Parse CLI option positions and value arities using the retained [Pi 1.5.2 help](herdr-app-evidence/pi-1.5.2-cli-help.txt), stopping at `--` so prompt text cannot become a mode flag. Exclude positively parsed --print/-p, --mode rpc, --help/-h, --version/-v, --export and --list-models. Exclude the initial utility commands install, remove, uninstall, update, list, config, auth and mcp. An unprinted json mode or an unknown extension flag is Unknown until classified; do not guess that it is absent. Ordinary text mode, startup resume/fork and --no-session remain interactive-capable. Custom embedded SDK launchers are unsupported. No transcript search, environment reads or full-argv logging is added.

On macOS use native process/start/argument metadata; on Linux use the direct numeric /proc partition. The inventory is bounded to 65,536 entries and 4 MiB normalized metadata. Permission, namespace, changing/unclassifiable profile and size gaps return Unknown. This O(P) cost is measured for work opening; placement-only context reads perform no census.

A missing claim after failed registration is not absence. An unresolved interactive Pi under the core blocks otherwise-absent Pi replacement candidates, while positively identified Running candidates remain usable. The error identifies the unresolved terminal. This design adds no ignore-unknown launch option or automatic repair.

#### Open question for new, fork and resume

Does reading the SDK ID at the relevant callbacks and requests reliably identify the active conversation across `/new`, `/fork`, `/resume` and reload, and keep the published claim accurate? A retained old context can still return an old ID. Verify that the process-scoped state retires old sources, including an orphan left when the new helper fails. Uniqueness/liveness alone cannot resolve this case. The publication contract above is buildable but remains unverified through transitions.

The retained probes demonstrate failures of the earlier helper-lifetime proof and transition guard. They do not verify the selected in-memory reset approach. Ordinary transitions must be exercised during implementation, including switching away and back and startup resume selection. Also check delayed old responses, cancelled changes and an unusual overlapping transition. This is an explicit implementation question; it does not pause the remaining design work or authorize Pi changes.

If a supported path exposes a concrete problem, stop its affected integration operation with a clear visible error and reassess that path. Do not restrict ordinary conversation changes or introduce a general lifecycle/recovery framework pre-emptively.

#### Verification and cost

Test the extension's actual state reset and helper path alongside Rust admission. Keep the tests focused on observable behavior.

- An unchanged ID reuses state; a changed ID resets it once at the next relevant boundary.
- Ordinary new/fork/resume, startup resume selection and reload retain normal Pi behavior.
- A switch away and back rejects earlier-generation results even when the conversation ID matches again.
- Old asynchronous startup/cleanup cannot publish current context or stop the newer helper.
- Initialization/SDK lookup failure gives an unavailable result, never a guessed Session.
- Cancelled changes leave the actual current Session usable when it is next observed.
- Accepted catalog saves remain accepted after reset or helper failure; requests are not replayed.
- Stale native source metadata, reused PIDs and unavailable process observations do not establish current identity or false absence.
- A helper restart does not replay enrollment or reopen a hidden companion.
- Existing Herdr reporting, branch admission, native lifecycle and preference tests preserve their behavior.

ID comparison is in memory at existing callbacks/request boundaries. Context changes use existing helper startup/cleanup and native label operations. No periodic SDK identity check, additional identity socket, watcher or persistent UI service is added. Measure publication and owner-host observation separately. The claim's transition accuracy remains unverified.

### Thin JavaScript mod

The native mod caches saved views and uses short Rust CLI calls for refreshes and explicit actions. One installed Streams mod supplies strip, sidebar and popup roles so its copies share one module cache. Rendering and cached popup expansion start no process. Rust owns catalog admission, live-context validation, reference resolution and owner effects. The mod executes the fixed desktop requests defined above.

The mod refreshes after relevant native changes and save notifications. An hourly safety read covers changes that supply no notification, including commands outside hc, independent sync and an unreported helper exit. Each view also offers Refresh. The hourly cadence includes retained hidden copies and permits stale data between successful reads. Every reference activation still obtains fresh Rust validation.

#### Request framing and scope

Each owner invocation passes one JSON request argument through the declared `exec` API. The request is bounded to 16 KiB and the JSON reply to 4 MiB, below the API's 8 MiB output ceiling. The entry point is `streams integration request --request-json <json>`. Its argument vector contains no mod-supplied shell command. Standard output contains one JSON reply; diagnostics use standard error. The invocation timeout is 30,000 milliseconds.

```text
// Version-1 data contracts. The wire spelling is fixed below.

ViewSelection =
    SavedStream { stream: StreamId }
  | SavedSession { session: SessionId }
  | Frame { terminal: NativeRef }
  | Sidebar { space: NativeRef, viewing: ViewingChoice }

ModAction =
    OpenReference { reference: Reference, context: ActivationContext,
                    at: Optional<NativeRef> }
  | OpenWork { request: OpenRequest }

ModRequest = {
    version: 1,
    scope: WorkScope,
    endpoint: EndpointId,
    body: Read { selections: NonEmptyList<ViewSelection> }
        | Act { action: ModAction }
}

ModReadValue =
    Stream(StreamView)
  | Session(SessionView)
  | Frame { context: FrameContext }
  | Sidebar(SidebarContext)

ModResult =
    Read { values: List<Result<ModReadValue>>, notification_key: String }
  | ReferenceOpened(ReferenceOpenReport)
  | WorkOpened(OpenReport)

ModReply = {
    version: 1, scope: WorkScope,
    endpoint: EndpointId,  // echoes the requested endpoint
    verified_endpoint: Optional<EndpointId>,
    result: Result<ModResult>,
    viewer_projection: List<ViewerAction>,  // read-derived names only
    warnings: List<String>
}
```

Version-1 mod RPC objects use the exact snake_case field names above, including all Optional fields as explicit JSON null when empty. Keep the v1 DTO codecs in src/integration.rs and use its crate's existing required_nullable helper; ordinary serde Option defaults would also accept omission. The CLI consumes these exported DTOs without exposing codec internals. IDs, paths and SafeHttpUrl serialize as strings; scalar enums use lower_snake_case strings. Payload unions use an internal kind tag, lower_snake_case variant names, named fields inline, and value for a single unnamed payload. Unit variants of a payload union contain only kind. Angle-bracketed names in wire shapes denote typed JSON placeholders. A Frame read value has shape `{"kind":"frame","context":<FrameContext>}`, a saved Stream value `{"kind":"stream","value":<StreamView>}`, and ReferenceOpened `{"kind":"reference_opened","value":<ReferenceOpenReport>}`. Effect.Applied has `"value":null` for Unit. Result<T> is exactly `{"ok":<T>}` or `{"error":{"kind":<String>,"message":<String>}}`. Reject unknown wrapper fields/tags and require exactly one result branch. Saved record fields retain their existing Rust Serialize representation. Stage 2 freezes the shared JSON fixtures in tests/fixtures/integration_v1.json and checks them in tests/integration_cli.rs. Stage 5's JavaScript protocol tests consume that same fixture file.

A read body has shape `{"kind":"read","selections":<List<ModRead>>}`; an action body has shape `{"kind":"act","action":<ModAction>}`. OpenReference has `"kind":"open_reference"` and reference, context and at fields; OpenWork has `"kind":"open_work"` and request. Named fields in the canonical types use the same spelling on the wire. Replies echo the requested scope and endpoint; success requires actual configured-scope admission. An echo alone establishes no admission. A valid admitted request gets one reply even when its operation fails. Invalid JSON/envelopes fail nonzero with stderr and no fabricated reply. request and describe own JSON stdout; a global --json adds no second encoding.

integration describe --frontend hc returns exactly `{"version":1,"scope":<WorkScope>,"endpoint":<EndpointId>,"notification_key":<String>}` after scope admission and hello verification. Failure is nonzero with stderr. integration view-document requires --scope-json <WorkScope-json> --reference-json <Document-reference-json>; it accepts only the Document Reference variant, never a client path, and rejects global --json like artifact view. Resolve the saved key and viewing checkout afresh and record the verified file's device/inode during resolution. Then open with O_NOFOLLOW | O_NONBLOCK, require a regular file and compare fstat with the retained device/inode before reading. An intervening replacement fails this open. Reject a size over 4 MiB and perform a bounded 4 MiB-plus-one read to detect growth. Only after successful validation pipe the bytes to the secure less `--` helper; do not pass an unchecked file path to less. Existing artifact limits remain unchanged.

Rust opens the configured source on the requested execution route and compares its admitted catalog and host with `WorkScope`. A mismatch returns unavailable. The explicit mod boundary selects hc using `endpoint` even without caller-terminal environment variables. Resolve the configured local hc socket, then compare hello.core.id with that endpoint before native reads/effects, including OpenWork and document/artifact references without at. The ID supplies only the expected identity. No catalog/core search or fallback occurs. Pure saved reads can succeed independently when native access is unavailable; each native reference must match the configured endpoint. Set verified_endpoint only after hello matches the requested endpoint. A failed native connection leaves it empty while independent saved reads or saved-link validation can succeed. The reply's read-derived `viewer_projection` contains rename cases only and requires that verified endpoint. Explicit `ReferenceOpenReport` and `OpenReport` carry their separate desktop requests.

Frame and sidebar selections resolve native context before calling the logical saved-read operations. The CLI unwraps each ReadReply.view into its ordered ModReadValue, concatenates its checked rename actions into viewer_projection and native warnings into warnings. Saved-only selections contribute no native actions. No nested duplicate action list is serialized. An identified supported Pi frame without an enrolled Session returns `Unenrolled`; observing it does not enroll it. Shells and positively identified non-interactive/deferred-harness frames return `NotApplicable` and retain native appearance; this says nothing about those harnesses' catalog enrollment. An incomplete live-identity observation returns `Unavailable`. An enrolled frame returns its saved Session and separate companion visibility. Hidden companions retain ordinary native header content. A Stream with no Sessions still has a valid saved view. A successfully observed unassociated space returns `Sidebar(Unassociated)`, an ordinary empty state.

The sidebar's Follow mode uses the focused enrolled agent in that Stream and retains its last agent when focus enters the sidebar. An agent from another Stream cannot select a viewing Session. Choosing a named Session pins it until Follow is selected again. Missing viewing context leaves document references visible with opening unavailable. Artifact activation always uses its complete producing key.

Unknown versions or fields, malformed replies and scope mismatches fail at the boundary. Read results preserve input order. An oversized request is rejected before dispatch; an oversized read returns an explicit error without truncating entries. This baseline adds no automatic pagination or resubmission. Supported-host tests must establish argument encoding and size handling before enabling the transport.

#### Save notifications

After confirming a catalog save and releasing its storage locks, Rust attempts one terminal-label update through its known hc route. The key is streams.saved. followed by the scope digest defined below; the value is a fresh UUID string with no ordering meaning. The read reply supplies that scope's key to the mod. Each update changes only this key and preserves all other labels, including immutable Pi source claims.

A registered serve producer checks its captured core identity and terminal. A one-shot CLI mutation in an ordinary hc shell has no pre-pinned core ID. It uses HC_TERMINAL_ID on its resolved local hc socket and can obtain hello on that connection. It does not require a prior identity registration. An inherited terminal identifier is a destination hint for a reread notification and confers no catalog or conversation authority. A command without that hint sends no notification. No producer searches for a carrier, creates a terminal, or falls back to another socket after failure.

The hook covers accepted checkpoint updates, enrollment and membership changes, Stream and Session edits, archive/restore and artifact mutations in both child and ordinary CLI dispatch. Explicit sync that imports records also invalidates the scope after releasing its locks. A confirmed save followed by a directory or presentation failure still sends its notice. Read-only operations write no token.

Concurrent writers may overwrite a notification token. The mod compares token changes and rereads current state; it assigns no version or ordering meaning to the value. Separate scope keys preserve invalidation when one terminal writes to multiple scopes. A failed or lost notification reply preserves the catalog outcome and is not retried. A failed known delivery writes a CLI stderr warning, including for --json commands without changing their stdout. Private-child callers emit the existing protocol-2 warning frame, separate from the response outcome. In particular, a notification failure never adds PublicationResult.warnings or turns a successful sync into SavedWithWarnings; resultNeedsAttention continues to describe catalog publication. Background native moves use that same nonfatal warning channel. The message says that views may need Refresh. A standalone command without native context retains its ordinary save result.

The app's existing subscription includes local and configured-remote terminal events. Its terminal-label events update the model exposed by `terminals()`. Each Streams copy calls that function so the app tracks its dependency. It compares only the relevant notification and context projection; unrelated title, status or size changes cause no Rust read.

#### Shared cache and task lifetime

A coordinator is keyed by `(WorkScope, EndpointId)` and retains its configured execution route, subscription set, selection generations, dirty flag and one in-flight read slot. A route change invalidates old display results before another request. An in-flight action retains its original destination.

```text
save notice, changed context, hourly tick or Refresh
                         |
                         v
                shared scope coordinator
                         |
                         | one bounded Rust call
                         v
               scope and context admission
                         |
                         v
                  logical saved reads
                         |
                         v
                 shared in-memory cache
                         |
                         v
                 subscribed native views
```

Each copy establishes its live input dependencies and samples the relevant projection before its initial read. A changed projection or local selection marks the coordinator dirty and resolves an in-memory wake promise. Promise continuations run after rendering, so the waiting task starts Rust in a task context. Rendering performs no catalog I/O or native mutation.

The coordinator deduplicates identical selections. A new change during a read queues one follow-up read of the latest selection set. Each copy accepts a result only while its scope and selection generation still match. Read results wake copy-owned cache-change tasks, whose async contexts redraw their own views. Ordinary call-scoped contexts are never retained.

The first subscription starts the waiting coordinator task and hourly timer from a supported callback with `{ owner: null }`. That declared ownership lets them outlive the initiating copy. Each copy's cache-change waiter remains copy-owned. Closing one copy synchronously removes its subscription. The last subscriber cancels the timer and prevents future scheduled reads.

When the last subscriber closes, remove the coordinator from the scope map and discard its later read results and unapplied name projection. A new subscriber creates a fresh coordinator and may start a new read while the abandoned bounded read finishes. This occasional overlap is allowed for read-only requests. No retiring-entry registry or cross-lifetime busy slot is kept. JavaScript cancellation does not prove that `exec` stopped. Submitted actions remain single-submit and are never repeated by resubscription.

The app scheduler retires tasks with their evaluated script generation. Reload creates a fresh coordinator. A bounded old process may still complete externally; its display result cannot enter the new generation. Acceptance tests must exercise multiple copies, hidden copies, closure and reload against the actual mod runtime.

The hourly timer reads all subscribed selections. Event-triggered reads do not postpone its cadence. A tick during a read coalesces with the next current-state read. Suspension or a blocked event loop can delay a tick; resumption requests current state without replaying missed hours. Use Refresh when immediate state is needed after sleep or failed reads.

#### Activation and failure behavior

A reference click captures its complete reference, viewing Session and actual calling space. Rust rereads the saved reference, validates current membership and resolves its target on the owning host. The mod supplies no trusted filesystem path or prepared launch command.

Follow activation supplies the expected frame and Session. Rust checks that frame's current exact conversation and saved Session before opening the document. A mismatch rejects the activation without substituting another Session. Pinned activation preserves the explicit saved Session and checks its current membership and checkout. The UI checks its local selection before submission; subsequent focus changes cannot retarget the submitted operation.

An action is submitted once. Duplicate submission stays disabled through owner and desktop completion. A structured reply retains every confirmed and uncertain outcome. Desktop requests execute once in reply order within a 30-second total desktop budget; no new step starts after expiry or lost generation. A failed required step stops affected later steps and preserves owner effects. Optional name failure does not prevent an independent explicit focus; outcomes remain separate. A timeout, malformed reply or lost reply can follow completed native effects, so the UI reports an uncertain action result and offers inspection and Refresh. It never automatically resends the action.

Closing a view stops future submissions and display updates. Accepted effects retain their results even when the reply path disappears. Merely closing a popup, hiding a sidebar or unloading a mod does not change companion preferences or request a report. Those changes remain explicit harness-bound operations.

A failed first read shows unavailable. A later failure retains the last successful data with a stale indication and its successful-read time. Only a successful read can establish an empty state. A failed read waits for a new event, safety tick or explicit Refresh.

#### Failure diagnostics

Failures leave retained, bounded diagnostics as well as their existing UI or CLI result. Coverage includes failed refreshes, rejected or malformed transport replies, scope/endpoint mismatches, failed identity registration or validation, failed save notifications, partial native completion and uncertain actions. Unenrolled or not-applicable frames, an unassociated sidebar and discarded results from an old selection are expected states and produce no error entry.

The mod owns `appHome()/streams-errors.json`. Its shared coordinator retains the newest 100 entries within a 256 KiB file and serializes persistence through the existing `readTextFile` and conditional `writeTextFile` APIs. The operation failure handler records a bounded entry and persists asynchronously. UI results do not wait for that persistence. Duplicate entries and occasional entry loss are acceptable; there is no cross-copy deduplication registry or logging receipt. One conditional write is attempted for a snapshot; a conflict or write failure is reported without an automatic retry. This destination remains usable when the Rust executable or remote host is unavailable.

Rust CLI and private-child failures are also recorded on the executing host at `RuntimeState::root()`/integration-errors.jsonl, using the admitted catalog runtime base and host. Rotate at 10 MiB to integration-errors.previous.jsonl and use integration-errors.lock for the append/rotation lock. Retain only that previous file. Concurrent processes use one nonblocking append/rotation lock. A busy lock or failed write falls back to standard error without a lock wait or automatic retry. Logging occurs after catalog locks are released. Failure before the runtime directory can be resolved uses standard error. Because child.ts drains and discards stderr, the child's Rust failure boundary writes the host log directly.

Both logs use this entry schema. summary comes from a fixed allowlist for error_code; raw exception messages never enter it. Unknown local digest metadata remains null. The app file is `{version: 1, entries: [DiagnosticEntry]}` newest first; the owner file has one entry per JSONL line.

```text
DiagnosticEntry = {
    version: 1,
    time_ms: u64,  // unsigned epoch milliseconds
    component: mod | owner,
    operation: read | open_work | open_reference | projection
             | notification | source | transport | diagnostics,
    error_code: unavailable | invalid_request | scope_mismatch | endpoint_mismatch
              | conflict | uncertain | timeout | too_large | io | internal,
    summary: String,
    scope_key: Optional<String>,   // scope digest
    native_key: Optional<String>, // canonical NativeRef digest defined below
    save: accepted | not_saved | unknown,
    effects: none | known | uncertain
}
```

Each encoded entry is capped at 2 KiB. Logging uses allowlisted fields and excludes report text, document/artifact contents, credentials, environment values, complete commands and raw request/reply or stderr buffers. The caller retains the complete typed operation result independently.

Logging is best effort. A full disk, permissions failure or crash can lose diagnostics. The mod also reports failures through the app console; if its retained log cannot be written, it shows a diagnostics-unavailable indication alongside the original failure. The logging failure does not log recursively, replay an action or change an accepted save, partial result, uncertain outcome or StreamsChild.stop(). Diagnostics supply no identity or command authority and are never read to recover effects.

#### Cost and required verification

For `B` subscribed scope/endpoint coordinators, the safety cadence requests approximately `B` Rust invocations an hour while the app runs and reads complete. Initial loading, context changes, actual notifications and explicit actions add requests. Retained hidden copies count as subscribers. Remote execution can add an hc wrapper and an owner-host Rust process.

Catalog I/O needs separate measurement. A Session view uses addressed records. A Stream sidebar needs saved membership inspection and artifact-producer listings. `RecordInventory::inventory` enumerates the whole catalog; Stage 2 adds a host-scoped Session inspection method. The storage query and its directory/record cost must be explicit; this design adds no persisted membership index implicitly.

Each covered save adds at most one notification-label update per command completion with a usable route, plus required connection/handshake work. A read writes no notification. Diagnostic storage adds bounded I/O on failures and no periodic work or extra Rust invocation. Desktop native dispatch makes at most one in-process herdr.run call per changed name, selection or focus; browser dispatch uses Context.open_url. Count these separately from owner calls. The mod adds no folder watcher, derived display file, socket, persistent Rust UI service or independent identity timer. The app's folder API retains a worker with a 150 millisecond idle loop after its last waiter disappears and supplies no remote subscription.

Verification must cover strict framing and limits, wrong scopes and endpoints, every save producer, local and remote delivery, initial-read and in-flight-save ordering, multiple scopes sharing a terminal, unrelated native events, initiating-copy closure, last-subscriber cleanup, immediate reopening with abandoned read results discarded, hidden copies, suspension, source reload, failed reads and uncertain actions without replay. Reference tests include a producer with same-named artifacts in two Streams and Follow/pinned document activation after conversation or membership changes. Process starts, native RPCs, directory enumeration and record bytes are measured separately.

Diagnostic tests cover retention across restart, entry/file bounds, rotation and concurrent Rust writers, tolerated duplicate entries, secret/content exclusion, full disk and permissions failures, conditional-write conflict, and no recursive logging or changed operation outcome when logging fails. A client-side failure must remain recordable while Rust is unavailable. These tests also verify that an accepted save with a failed notification is logged as accepted with a warning.

### Native UI installation and visibility

Ship the UI as JavaScript assets under `integrations/herdr-app/`. `main.js` exports one mod with frame, sidebar and popup roles. `cache.js` owns scope subscriptions and read-only caching. `frame.js` adapts the shipped 0.1.15 pane renderer and keeps its native program/status indicators and explicit-pane focus, split, zoom and close commands. It changes the title/context cells only for an enrolled, shown companion. An unavailable or hidden companion retains ordinary native presentation; an error indicator explains unavailable context.

Use the existing 30 px header. The enrolled header shows Session title, saved status, short state and Expand; folder detail moves to hover text. Expand uses the captured cached Session and starts no process. The sidebar sits beside Files and Changes and supports Follow and pinned Session selection. The popup shows full saved progress and reference actions. All reference actions go back through Rust validation.

Current companion visibility is distinct from automatic-opening preference. Use the terminal-label key and JSON marker specified in the label contract below; it is separate from identity and save-notification keys and contains no saved report text. Show writes that marker once, Hide removes it once, and Keep leaves it unchanged. Missing means Hidden; malformed or unavailable data means unavailable. Only a separately verified current conversation can use the marker. No marker supplies identity or command authority.

A marker written while the app is closed preserves the requested display choice. It cannot confirm rendering. A refresh, popup close, sidebar hide or helper reattachment never writes a marker or changes preference. Multiple conversations keep separate keys so switching cannot transfer one conversation's dismissal to another.

Installation follows this written procedure after implementation approval.

1. Run `streams integration describe --frontend hc` through the selected owner's exact Streams executable and execution route. This new read-only command returns version 1, the admitted `WorkScope` and the permanent hc endpoint. It resolves no other catalog and writes no native state.
2. Copy the packaged mod files to `appHome()/mods/streams/`. Configure the ModProps bindings above with scope, expected endpoint, exact Streams executable and declared local/remote exec route. These values come from the describe result and the user's existing configured app route; labels and display names do not select a catalog.
3. Clone `main` with `layout.clone` into a personal Streams layout and clone `tabs` with `mod.clone` into `streams-tabs`. A destination collision stops setup; it never overwrites an existing personal clone.
4. In those personal copies, add Streams to the right-hand panel stack and to the tabs mod's panel button and enabled-panel list. Keep Files, Changes, panel sizing, tabs/window controls and drag behavior. Keep pane.lone true and set the pane header content to the Streams frame role at 30 px; do not add another row.
5. Run `herdr-app check`, inspect `status` and an actual rendered screenshot. Verify every native pane control and all three panel choices before selecting the personal layout for routine use.

App upgrades need an explicit compatibility check of the pinned pane/tabs adaptations. There is no automatic source merge or installer repair loop. Runtime Rust never edits the layout. Removing the integration means selecting the original layout and removing only the explicitly installed personal copies.

### Native platform contract

Socket selection follows the owner-local hc 0.1.21 [path rules](herdr-app-evidence/hc-v0.1.21-paths.rs) and [Unix defaults](herdr-app-evidence/hc-v0.1.21-unix.rs). Trim HC_SESSION, default empty/missing to default, and reject dot, dot-dot, control characters and the path-forbidden characters in the pinned session function. Prefer nonempty HC_SOCKET, then the config.socket string from XDG_CONFIG_HOME/herdr-core/config.toml or HOME/.config/herdr-core/config.toml. An unreadable/malformed existing config fails explicitly; absent config has no override. Decode only the socket field and ignore other settings. Promote the already locked toml 0.8.23 crate to the direct dependency toml = "=0.8.23"; do not add a parser or change Pi dependencies.

Without an override, state is (XDG_STATE_HOME or HOME/.local/state)/herdr-core/<session>. The runtime directory is nonempty XDG_RUNTIME_DIR, otherwise /run/user/<getuid()> when that directory exists, otherwise state. A runtime directory gets the herdr-core suffix; state already has it. The socket is that directory/<session>.sock on both macOS and Linux. Resolve relative paths against the operation's captured cwd. Match the pinned socket/holder length checks using the platform sockaddr_un limit. Read no other sessions and never start a core. Capture HOME, XDG_STATE_HOME, XDG_CONFIG_HOME, XDG_RUNTIME_DIR, HC_SOCKET, HC_SESSION and HC_TERMINAL_ID in the CLI caller environment without activating hc from socket variables. Launch environment forwarding adds the socket/session/runtime selectors only. Never forward caller HC_TERMINAL_ID or parent identity into terminal.run.env; hc supplies each new terminal's identity. The hc adapter forwards its resolved absolute socket path so a different child cwd cannot reinterpret a relative selector.

#### Label bytes

All Streams label JSON is encoded in Rust in src/runtime/hc/labels.rs. JavaScript consumes returned keys and treats labels only as invalidation hints. Use the existing SHA-256 helper and lowercase hexadecimal output. Canonical digest bytes are UTF-8 arrays without whitespace; string elements use `codec::ascii_json`, and pid uses ordinary unsigned decimal. Scope preimage is [catalog,host], process-key preimage is [boot,pid,started], companion preimage is [catalog,host,harness,native_id], and native_key preimage is [adapter,endpoint,key]. All four use the same canonical encoding and SHA-256 lowercase hexadecimal output. Thus canonical_encode(helper) in source_key means that exact process-key array. JSON values below use exact field names and the wire conventions above.

| Purpose | Key | Value |
| --- | --- | --- |
| Routing hint | streams.work | `{version: 1, scope: WorkScope, stream: StreamId}` |
| Source claim | streams.pi.<process-key digest> | PiSourceClaim as defined above |
| Save notice | streams.saved.<scope digest> | Fresh UUID string |
| Companion visibility | streams.companion.<companion digest> | `{version: 1, scope: WorkScope, conversation: ConversationId, session: SessionId}` |
| Requested app placement | herdr.space and herdr.tab | The unqualified space and per-terminal fresh tab keys |

Terminal-label values are strings. JSON objects are JSON-stringified inside the label value. Removal is the existing terminal.label operation with that exact key set to the empty string; null is invalid. Each operation touches only its own keys. Source retirement removes its helper key; Hide removes the companion key. Neither deletes other scopes' or helpers' labels. Add golden fixtures for Unicode, field ordering, numeric encoding, own-key deletion and the separate placement keys.

#### Mod configuration and desktop calls

The manifest is exactly `{"place":"none","title":"Streams"}`. main.js is the default View export and imports the other ES modules. Configure these props; do not infer a scope from a terminal label.

```text
ModProps = {
    role: frame | sidebar | popup,
    bindings: NonEmptyList<{
        scope: WorkScope,
        endpoint: EndpointId,
        on: Optional<String>,
        streams_bin: String  // verified absolute executable on that owner
    }>,
    session: Optional<SessionId>,
    space: Optional<NativeRef>,
    context: Optional<ActivationContext>
}
```

These are installation/overlay props, separate from the strict RPC envelope. Omitted optional configuration fields mean null. The app additionally supplies pane and terminal for a header and overlay for a popup; retain those host-injected keys for native controls and closure. Popup bindings is the single captured binding, with its captured Session, space and context.

Each installation binding names one configured owner/catalog and a unique app machine alias, with null for local. During installation verify that this app machine route and the owner Rust socket configuration select the same core returned by describe. A mismatch stops setup; do not search or fall back. Changing either route requires stopped clients and new binding admission. Require unique (scope, endpoint) pairs as well, so one coordinator never acquires competing execution routes. The same bindings are supplied to frame and sidebar copies. Find a frame's PaneInfo using props().pane, then its parent SpaceInfo through PaneInfo.tab; that space's machine selects the exact binding. Sidebar first uses its captured explicit space, then focused()?.space. If both are absent, use the sole active SpaceInfo from spaces(), including when its tabs array is empty; that space's machine selects its binding. Zero active spaces means no current space; multiple active candidates are unavailable. Never reuse a space from another binding. Popup receives the captured binding, Session and activation context from its opening click. Missing/duplicate bindings or unavailable machine layout are explicit unavailable states. Native IDs are qualified only with that binding's captured exact alias prefix; never split arbitrary opaque keys or use a focused-resource default.

The owner call is exec(binding.streams_bin, ["integration","request","--request-json",JSON.stringify(request)], `{on: binding.on, timeout: 30000}`). The fixed desktop dispatcher uses in-process herdr.run and a live AsyncContext for browser opening. It starts no desktop subprocess and needs no app executable path. Names are explicit structured values, including leading hyphens, so no rename dialog or flag parsing is involved.

| Viewer action | In-process dispatch |
| --- | --- |
| SelectSpace | `herdr.run("space.select", {id: qualified(space)})` |
| FocusTerminal | `herdr.run("pane.focus", {terminal: qualified(terminal)})` |
| RenameSpace | `herdr.run("space.rename", {id: qualified(space), name})` |
| RenameTab | `herdr.run("tab.rename", {id: qualified(tab), name})` |
| OpenUrl | `liveAsyncContext.open_url(validated_url)` |

Before dispatch, machines() must show the captured alias ready. Find exact qualified space/tab IDs in spaces(), and a terminal's model pane through panes() and its tab's owning space. Require that space's machine to match the captured alias. An unpublished/missing target is pending/not-dispatched; never use a focused-resource default. Skip a rename only when its live named flag is true and name already matches. A successful run call records Dispatched only; exceptions after call entry are uncertain, with no retry or claim of persistence/paint. The existing separate 30-second desktop budget is checked between these synchronous calls, as are source generation and initiating-view liveness. No per-command subprocess timeout remains.

Use the pinned [herdr module signatures](herdr-app-evidence/herdr-app-0.1.15-herdr.d.ts) and [GPUI context/task signatures](herdr-app-evidence/herdr-app-0.1.15-context-api.md). exec takes command, string args and `{cwd?,on?,timeout?}`; readTextFile(path,`{on?}`) and writeTextFile(path,text,expected) retain their declared bounds and conditional-write semantics. Start owner-independent work with cx.spawn(body,`{owner:null}`) and the hourly callback with cx.timer.every(3600000,handler,`{owner:null}`); retain their Task handles for cancel(). Each subscriber waiter remains copy-owned. Only the AsyncContext passed to a live task may cross await; no call-scoped Context is retained.

frame.js adapts the pinned [pane implementation](herdr-app-evidence/herdr-app-0.1.15-pane.js) and keeps its ordinary icon/name/folder/status behavior. Preserve pane.focus {pane}, split-right/down with direction and pane, pane.zoom {pane}, pane.close {pane}, stop_propagation, native SVG asset paths, hover behavior and bottom-header border. Only enrolled companion presentation substitutes the specified header content; the layout still supplies its single 30 px row. No native control changes target or disappears.

### Staged delivery and file boundaries

The author authorized the full staged implementation. Installation and live local/remote acceptance still require separate authorization. Each stage leaves existing commands buildable and tested. The Create/Modify lists below are phase write allowlists. They grant no permission to refactor adjacent code. Delete no files. Initial support matches the current Unix Rust runtime on macOS and Linux. No Windows implementation or Pi SDK/dependency change is introduced. The sole Rust dependency adjustment promotes the already locked toml 0.8.23 package for hc socket configuration.

#### Stage 0. Exercise the simple Pi context state

Create `tests/stream_hc_lifecycle_test.mjs` and `tests/stream_hc_sdk_runtime_test.mjs`. Modify `extensions/stream-checkpoint/index.ts`, `extensions/stream-checkpoint/child.ts`, `tests/stream_child_test.mjs`, `tests/stream_sync_test.mjs` and `mise.toml`. Update existing child-test assertions that assumed replacement before close to assert the required close barrier; preserve all request/no-replay assertions. Existing sync interleaving fixtures must await entry into their held sync callback before replacing/stopping its context, rather than assuming synchronous startup admission. Preserve the existing same-conversation draft-retention assertions.

During implementation, start with the selected in-memory ID comparison/reset in `extensions/stream-checkpoint/index.ts` and `child.ts`. Add `tests/stream_hc_lifecycle_test.mjs` and register `test:pi:hc-context` as `node --test tests/stream_hc_lifecycle_test.mjs` in `mise.toml`; Stage 4 extends the same file. Use the existing tests/stream_child_test.mjs harness and fake SDK callbacks. The default test imports no Pi 1.3+/1.5.2 runtime API and stays hermetic with CI's pinned 0.99.2 dependency. Register the separate, non-aggregate `test:pi:hc-runtime` task as node --test tests/stream_hc_sdk_runtime_test.mjs. That optional test requires an explicit absolute STREAMS_PI_SDK_ENTRY, uses lab-cli.mjs's exported resolveSdkEntry, validates the selected SDK version, imports that SDK and checks its runtime exports before constructing fixtures. No SDK upgrade, npm lockfile edit, network/model call or global installation belongs to these tests. CI's default lifecycle test does not establish installed Pi 1.5.2 transition accuracy. If a real installed-SDK scratch check is needed, keep it in `.tmp/herdr-app-pi-state-probe.test.mjs` and record its version/results in `.tmp/herdr-app-pi-lifecycle-probe.md`. Stage 0 implements the state/retirement rules against fake helpers without importing later Rust modules or publishing native labels. Stage 4 connects actual hc registration. Cover ordinary new/fork/resume, startup resume selection, reload, switching away/back and delayed old responses. Retain the old counterexamples as evidence for the superseded lifetime/guard schemes; they are not passing tests of this approach.

Whether the SDK-selected state and its published hc claim remain accurate through these transitions is the explicit open question. Try the small implementation and inspect concrete failures. A lookup or ambiguous transition can make the affected native operation unavailable, without blocking Pi's normal commands or undoing accepted catalog saves. No Pi patch, transition lock, automatic replay or recovery coordinator is part of this stage. Production implementation has been authorized; installation and live acceptance remain excluded.

#### Stage 1. Introduce the shared contract without changing behavior

Create `src/integration.rs`, `src/application/integration.rs`, `src/application/documents.rs`, `src/web_url.rs` and `tests/integration_contract.rs`. Stage 1 also modifies `src/tui/render.rs` to extract its existing web-link parser unchanged; SafeHttpUrl adds credential rejection for the shared desktop contract without changing existing TUI link behavior. Modify `src/lib.rs`, `src/application.rs`, `src/frontend/mod.rs`, `src/frontend/herdr.rs`, `src/frontend/cmux.rs`, `src/runtime/native.rs`, `src/runtime/workspace.rs`, `src/runtime/workspace/open.rs`, `src/runtime/workspace/resume.rs`, `src/runtime/native_cards.rs`, `src/runtime/native_cards/owner.rs`, `src/runtime/native_cards/control.rs`, `src/runtime/native_cards/projection.rs`, `src/runtime/native_cards/actions.rs`, `src/runtime/native_cards/documents.rs`, `src/runtime/native_cards/artifacts.rs`, `src/runtime/card_settings.rs`, `src/cli/mod.rs`, `src/cli/settings.rs`, `src/cli/documents.rs`, `src/cli/cards.rs`, `src/cli/cards/source.rs`, `src/cli/cards/host.rs`, `src/cli/serve.rs`, `src/cli/serve/native.rs`, `src/cli/serve/control.rs`, `src/tui/snapshot.rs`, `src/cli/workspaces.rs`, `tests/serve/subscriptions.rs` and `tests/contracts.rs`.

Create `src/integration.rs` for the canonical scope, references, direct result types, adapter trait and pure selection/projection orchestration. Export it from `src/lib.rs`. Keep record-local save rules in their existing operation/application modules. Introduce `src/application/integration.rs` and export it from `src/application.rs` as the host-admitted facade for saved reads and work actions.

Extract the existing Session-directory, checkout-registry and working-document resolution from src/cli/documents.rs and src/cli/cards/source.rs into src/application/documents.rs, registered in src/application.rs. All existing agent-context/card callers and new reference/view-document callers delegate to that one resolver, including repository URL selection, registered checkout and unregistered VCS-root checks. Keep its tests inline and add parity cases in integration_contract; do not copy the policy into the facade or CLI.

Adapt `src/frontend/mod.rs`, `src/frontend/herdr.rs`, `src/frontend/cmux.rs`, `src/runtime/native.rs`, `src/runtime/workspace/open.rs`, `src/runtime/workspace/resume.rs`, `src/runtime/native_cards.rs` and `src/cli/workspaces.rs` to call that shared boundary. Keep existing Herdr topology inside its adapter and preserve cmux's combined one-workspace/initial-command call. Existing Herdr/cmux public CLI outputs remain stable. The new hc backend uses the output mapping below.

Route existing Herdr card/reference/preference entry points through the shared facade as part of this stage. Adapt src/cli/cards/source.rs and src/tui/snapshot.rs to shared addressed-record field mapping plus existing owner-local DisplayContext, without a roster read per card. src/cli/cards.rs and native_cards/actions.rs dispatch reference activation through shared resolution; split the existing documents.rs/artifacts.rs native-effect helpers to accept verified targets. `NativeIntegration::open_target` calls those effect-only helpers, never the higher-level facade again. Keep editor reuse, checkout identity/CWD and artifact keys unchanged. Parameterize CardSettings in src/runtime/card_settings.rs with the supplied ConversationId harness; existing Pi callers pass Pi and retain their preferences. cards/host.rs and serve/control/native wiring make only the required constructor/delegation adaptations and preserve old response shapes. Keep topology and terminal-card rendering inside Herdr; share policy and data mapping.

Create `tests/integration_contract.rs` and register it in `tests/contracts.rs`. Fake adapters assert selection before effects, unknown blocking launch, prepared argv/directory preservation, exact endpoint targeting, combined native calls, partial completion and no replay. Retain and run existing frontend, workspace, native-card and saved-conversation tests.

#### Stage 2. Add saved views and the bounded CLI boundary

Create `src/cli/integration.rs`, `tests/integration_cli.rs` and `tests/fixtures/integration_v1.json`. Modify `src/cli/args.rs`, `src/cli/mod.rs`, `src/integration.rs`, `src/application/integration.rs`, `src/application/records.rs`, `src/storage/records.rs`, `src/yaml_store/records/inventory.rs`, `tests/integration_contract.rs` and `tests/contracts.rs`.

Add `src/cli/integration.rs` and register `integration request --request-json` in `src/cli/args.rs` and `src/cli/mod.rs`. At this checkpoint implement SavedStream/SavedSession reads and the complete strict wire schema. Native selections and actions return explicit unavailable results before effects until Stage 3 adds hc. Saved-only replies echo the requested endpoint with verified_endpoint empty. Emit one stdout JSON reply, diagnostics on stderr and the stated bounds. Never interpret a mod-supplied shell line. Defer integration describe, native action dispatch and endpoint-handshake tests to Stage 3.

Add a host-scoped Session inspection method to `RecordInventory` in `src/storage/records.rs`, implemented by `src/yaml_store/records/inventory.rs`. This is a display/inspection capability and is not added to RecordReader or record mutations. Add the narrow delegation method to OpenedRecords in src/application/records.rs so the facade can use the inventory without exposing its private YamlRecordStore. Keep open_records_observed available for the I/O counter tests. List only `hosts/<encoded-host>/conversations`, decode each direct regular record once using existing strict readers, then filter current membership. Do not recurse through artifacts, other hosts or repository directories. Corrupt/unreadable data within that inspected partition fails the query explicitly.

`src/application/integration.rs` constructs saved Stream/Session views using addressed records and existing artifact-summary logic.

- A Session-only read has no directory inventory. A sidebar batch enumerates the requested host once and shares the decoded Session records across requested Streams.
- Through OpenedRecords.artifacts().with_artifacts, acquire one artifact reader for the batch and call stream_producers once per distinct requested Stream. Reuse its existing directory validation and missing-directory behavior. Each affected Stream read fails explicitly on malformed producer entries or other I/O errors.
- Derive `artifacts::summaries` only for returned producer IDs present in the decoded Session map, before current-membership filtering. A missing producer directory contributes no artifacts; a moved Session with a retained producer directory still contributes its saved artifacts. Ignore leftover directories without Session records, preserving standalone semantics.
- Read only the requested Stream records. Sort member summaries by (CaseMapper.fold(title), Session ID), using existing catalog case-folding. Sort artifact producer groups by complete producer key and summaries by ArtifactKey.
- Keep standalone artifact APIs unchanged. Reuse decoded records without calling standalone listing, relisting producers or reopening content files. Use complete producing keys. Parity tests cover moved producers, missing directories, malformed/non-directory producer entries and unreadable directories.

For N saved Sessions on the requested host and S distinct requested Streams, the batch decodes N Session records once, reads the requested addressed records and performs S artifact-producer directory listings. The additional metadata cost is O(P) for P total entries in those listings; artifact contents are never opened. No membership index or constant-cost Stream read is introduced. A reply above the bound fails clearly without pagination.

Extend `tests/integration_contract.rs` and create `tests/integration_cli.rs`, registered in `tests/contracts.rs`. Assert strict framing, wrong scope/host, empty versus unavailable, explicit unavailability of not-yet-enabled native requests, saved-only endpoint metadata, stored fields without model calls, moved producers with same-named artifacts, bounded partitions and I/O counters. Define no new catalog fields.

#### Stage 3. Implement the hc native adapter

Create `src/frontend/hc.rs`, `src/frontend/hc/processes.rs`, `src/frontend/socket_io.rs`, `src/runtime/hc.rs`, `src/runtime/hc/labels.rs`, `src/cli/pager.rs` and `tests/integration_hc.rs`. Modify `Cargo.toml`, `Cargo.lock`, `src/lib.rs`, `src/frontend/mod.rs`, `src/frontend/herdr/socket.rs`, `src/runtime/mod.rs`, `src/runtime/native.rs`, `src/runtime/launch.rs`, `src/runtime/workspace.rs`, `src/runtime/workspace/open.rs`, `src/runtime/workspace/resume.rs`, `src/runtime/native_cards/config.rs`, `src/application/integration.rs`, `src/cli/integration.rs`, `src/cli/args.rs`, `src/cli/workspaces.rs`, `src/cli/mod.rs`, `src/cli/artifacts.rs`, `src/cli/cards.rs`, `src/tui/render.rs`, `src/tui/snapshot.rs`, `tests/integration_cli.rs` and `tests/contracts.rs`.

Implement hc transport/selection first within this stage, then extend src/cli/integration.rs, src/cli/args.rs and src/cli/mod.rs with integration describe --frontend hc and full native request dispatch. Describe uses the existing explicit frontend-selection type and reports the admitted scope/verified core ID without caller-terminal environment or writes. The installer captures that output and its separate execution route. Extend `tests/integration_cli.rs` with describe, actions and wrong-endpoint tests after the transport/hello path below exists.

Create src/frontend/hc.rs for typed JSON-RPC 2.0 calls over the owner-local hc Unix socket. Add toml = "=0.8.23" to Cargo.toml and update only the root dependency edge in Cargo.lock, retaining the existing locked package. Register src/runtime/hc/labels.rs from src/runtime/hc.rs; its inline tests own the label golden vectors. Extract/reuse only the cancellable nonblocking connect, poll and bounded JSON-line framing from `src/frontend/herdr/socket.rs` into `src/frontend/socket_io.rs`, registered in `src/frontend/mod.rs`. Keep its unit tests inline. Keep Herdr's protocol-22 envelopes and behavior unchanged; hc owns its RPC IDs, results/errors and subscriptions. Retain one connection per operation from hello through its reads/effects, without reconnection/retargeting. The existing live source owns a separate events.subscribe connection; heartbeats are not state changes. Missed/disconnected events clear the in-memory placement baseline; missed supplies a fresh snapshot that can establish a new baseline without inferring a move. Record hc RPC evidence as `NativeRequest::Socket`. Create `src/frontend/hc/processes.rs` for injected, bounded owner-host process inventory, exact PID/start/boot/parent observations and fresh process-group/stopped-state facts. Create `src/runtime/hc.rs` for association snapshots, pure map changes, source-claim validation and prepared hc effects. Register the adapter in `src/frontend/mod.rs`, `src/runtime/mod.rs` and `src/runtime/native.rs`. Preserve wanted transcript locators, selected adoption references, context-first observation, pure terminal placement/foreground lookup and first-enrollment/move association projection. Execution inspection adds owner-process evidence against the retained core inventory; space-only reads perform no census.

Add `hc` to explicit frontend selection. Auto selects hc only from admitted HC_TERMINAL_ID caller presence, matching the existing Herdr/cmux distinction between caller and endpoint hints. HC_SOCKET and HC_SESSION select the hc socket when hc is chosen; they do not activate Auto. Conflicting caller contexts fail and request an explicit frontend. Extend the existing environment allowlists in src/cli/mod.rs and src/runtime/workspace.rs. Replace `App::current_stream_id`'s hardcoded Herdr lookup with the shared selected frontend's retained reverse lookup. Route `App::metadata_workspace_target` in src/cli/workspaces.rs through the same retained observation for open/resume --current; it is a separate existing path. `App::resume_session` selects the admitted backend using the shared caller/endpoint policy. Update exhaustive BuiltinFrontend dispatch in src/runtime/workspace/open.rs and keep prepare_resume's deferred-harness/inherited-environment rules in resume.rs. Preserve Herdr/cmux callers, errors and output shapes. The hc output mapping below applies to explicit and Auto selection. An explicit mod request supplies its expected endpoint independently of Auto. The first successful hello pins the operation connection; a mismatch/disconnection stops it.

Use document `streams` for the version-1 association map and the app-owned `herdr-app` document only as a read. A missing Streams document is empty; an unsupported version is an error. Preserve the retained read order, publication check and at-most-one conditional write specified above. No additional lock, discovery pass or automatic retry is added.

Prepared launching uses hc terminal creation independently from app presentation. During hc preparation, resolve the executable on the owner into a verified absolute path that cannot be interpreted as a template name. Apply this to Pi and document/artifact viewer argv; extend `src/runtime/launch.rs` for this resolution without changing other adapters' command construction. A missing or non-template-safe executable fails before effects. Preserve the remaining verified argv/cwd. Test a template named pi and exact direct argv, and verify this interpretation on the supported disposable core. Set `no_parent: true` explicitly for Streams-created work terminals so ending the invoking agent/terminal cannot end the opened work. Delegated reviewer/worker children retain their separate parent policy. Routing labels contain only the admitted catalog, host and Stream plus a version; they do not supply launch commands or conversation identity. App placement uses documented labels with existing or fresh space keys. Retain `Requested` versus `Known` space evidence, and derive fixed desktop requests without running a remote GUI command. A later explicit open can place still-running selected terminals without relaunching them.

Implement `open_target` for local and remote owners with the owner pager and desktop browser split above. Add `integration view-document` in `src/cli/integration.rs` and extract the secure pager helper from `src/cli/artifacts.rs` into `src/cli/pager.rs`, registered in `src/cli/mod.rs`, with inline unit tests. Document view rereads the complete document/viewing-Session key and expected scope. Artifact view keeps `STREAMS_ARTIFACT_CATALOG_SOURCE` and full producing keys. Use verified placement labels, prepared argv, `no_parent: true` and `rm: true`. Links return validated desktop OpenUrl requests; owner Rust starts no browser. Unknown acceptance is Uncertain and never resubmitted.

Extract the existing `web_url` parser from `src/tui/render.rs` to `src/web_url.rs`, export it from `src/lib.rs`, and preserve its whitespace/control/escape/scheme/host checks. Build `SafeHttpUrl` there with the existing card credential restriction. Update `src/tui/render.rs` and `src/cli/cards.rs` to call the shared helper. TUI parsing retains its current policy; native cards and integration use the credential-free type. Keep helper unit tests inline. Preserve `CardRecords::contains_link` and `CardSnapshot::contains_link` accepting `&url::Url`, along with TUI `Target::Link` and existing callers/tests. Integration callers pass SafeHttpUrl's borrowed as_url() value to that saved-reference check. Expose immutable naming switches from src/runtime/native_cards/config.rs through a common parser that does not require hc callers to supply a Herdr plugin_root. Keep existing Herdr loading through that same parser. Add shared regression tests for malformed escapes, encoded controls, missing host and credentials.

Add the explicit `integration bind-space` command to `src/cli/integration.rs`, reusing the same one-write association operation. Its required flags are --space-ref <NativeRef-json> --stream <id> --scope-json <WorkScope-json>, with expected-scope admission as specified above. It performs no native creation or launch. Partial presentation diagnostics include those known inputs and the manual command.

Create `tests/integration_hc.rs`, registered in `tests/contracts.rs`. Assert exact calls and preserve every known terminal/space reference in these groups.

- Transport and associations. Fake Unix sockets cover framing, hello pinning, cancellation, the exact unfiltered plain subscription/notification shapes above, local versus forwarded document facts, heartbeats/missed sequences and structured conflicts, retaining Herdr transport tests. Inject map conflicts, outside removal, occupied spaces, unpublished creation followed by publication/manual binding/last-terminal closure, endpoint replacement and lost replies.
- Execution and admission. Cover accepted terminal start with failed placement, failed registration without a residual label, incomplete inventory, PID reuse, foreground shell with suspended Pi, return-to-foreground and missing/changing group facts. Include template collision, endpoint-verified link/native replies and saved-only replies without native verification. Terminals checks exact PIDs without invoking inventory.
- Placement. Two spaces bound to one Stream retain first-match work reuse; references without at fail ambiguity and explicit at validates its target. Already-placed work produces no phantom Requested key. Headless Pi without streams.work is adoptable only with verified Running and available unplaced evidence. Unavailable layout produces no adoption labels. Assert each adoption's applied/not-applied/uncertain result.
- Context and display. Test an associated active space with zero tabs/terminals through sidebar fallback. Test every Follow/Pinned row, Associated data preserved on viewing failure, Frame-derived space naming without a sidebar and warning/action aggregation. Cover pre-message registration, newly enrolled automatic opening, unassociated sidebar, reverse context, selected-only adoption and saved path-reported Herdr presence without extra transcript opens.
- References and lifetime. Cover local/remote owner pager argv and exact scope, desktop-only links, deferred desktop results and no unlabeled viewer launch. Closing the invoking terminal or app leaves independently opened work running.

#### Stage 4. Bind Pi and add complete save notifications

Create `src/cli/notify.rs`. Modify `extensions/stream-checkpoint/index.ts`, `extensions/stream-checkpoint/child.ts`, `extensions/stream-checkpoint/child-protocol.ts`, `extensions/stream-checkpoint/native.ts`, `extensions/stream-checkpoint/native-rust.ts`, `extensions/stream-checkpoint/settings.ts`, `extensions/stream-checkpoint/streams.ts`, `src/child_protocol.rs`, `src/child_protocol/tests.rs`, `tests/fixtures/pi-child-v2.json`, `src/cli/args.rs`, `src/cli/mod.rs`, `src/cli/serve.rs`, `src/cli/serve/native.rs`, `src/cli/serve/dispatch.rs`, `src/cli/serve/control.rs`, `src/cli/catalog.rs`, `src/cli/sessions.rs`, `src/cli/enrollment.rs`, `src/cli/artifacts.rs`, `src/cli/workspaces.rs`, `src/sync/records.rs`, `tests/stream_hc_lifecycle_test.mjs`, `tests/stream_child_test.mjs`, `tests/stream_native_test.mjs`, `tests/stream_naming_settings_test.mjs`, `tests/integration_cli.rs` and `mise.toml`.

Update `extensions/stream-checkpoint/index.ts`, `child.ts`, `native.ts` and `native-rust.ts` for the process-scoped current Session state, SDK ID checks, state/generation reset and old-result rejection. Implement the awaited-close retirement/restart behavior above in child.ts and index.ts, retaining the existing one-restart budget. Keep reporting/model capture and branch admission unchanged. Register interactive hc context before persistence/enrollment, preserving the existing Herdr transcript gate. Exercise the Stage 0 cases as this path is built. Do not add the superseded hc-lifecycle transition guard or restrict new/fork/resume. Define only the required startup/context handoff fields; there is no identity heartbeat. Keep Herdr's throwing native.start() behavior in cli(ctx) and Reporter.prepare. For hc only, normalize a completed registration failure to native-unavailable state for that generation and allow catalog context/enroll/reporting; pane commands still fail explicitly. Helper transport, protocol and scope failures retain their ordinary errors. Test both branches, including the existing Herdr failure test.

Update `src/cli/serve.rs`, `src/cli/serve/native.rs` and `src/cli/serve/dispatch.rs` in these groups.

- Lifecycle. Add hc registration, fact subscription and lifecycle/move dispatch through the common boundary, with explicit EOF retirement before worker cancellation. Keep a positive placement baseline only in the live binding; clear it on loss and preserve membership on startup/reattachment or display-only changes. Accepted membership survives native follow-on errors.
- Caller context. Generalize App's remembered native workspace_stream beyond self.herdr, including remember_app and agent_observation. A verified hc association supplies workspace_stream_id; failed registration supplies None and `Controller::enroll` uses record-only fallback. Expose existing naming switches in hc contexts; keep Herdr compact metadata/caption ungated.
- Protocol. Add serve's --sdk-pid clap field in src/cli/args.rs. Keep protocol-2 response envelopes and DomainObject unchanged. Pass optional request-envelope pi_context from streams.ts/native-rust.ts through child.ts; define its type/transport option in child-protocol.ts and validate it in Rust dispatch. Retain verified locators through DirectoryBindings on enrollment/NewlyBound or later persistence.
- Compatibility. Extend src/child_protocol/tests.rs and tests/fixtures/pi-child-v2.json for legacy omission, valid hc context, mismatch and malformed envelopes. Validate hc lifecycle reports in TypeScript. Rename settings.ts display labels to Native workspace naming and Native tab naming, update exact assertions in tests/stream_naming_settings_test.mjs and preserve persisted keys, Herdr dispatch and existing test meaning. Add no identity heartbeat.

Create `src/cli/notify.rs` as a completion-boundary helper. It receives the admitted scope, resolved hc socket and known terminal, with the registered expected core ID when one exists. Registered serve calls verify that ID; ordinary one-shot CLI callers need no prior registration/core ID. Attempt one scoped label mutation after saves/locks complete and return a warning without changing the save. Wire it once per completed command in src/cli/mod.rs and child dispatch, and after saved background membership completion in src/cli/serve/native.rs. Mark accepted mutations in src/cli/catalog.rs, sessions.rs, enrollment.rs, artifacts.rs and workspaces.rs create_stream/start. `App::refresh_records` must propagate confirmed imported saves for list/show --refresh, independently of Output.accepted_save. Expose that information from src/sync/records.rs, including a partial import followed by failure, and aggregate one notice at completion. Use the separate stderr/protocol-warning channel above; never attach a notice warning to a sync publication result. Keep all native notification I/O outside storage/domain operations.

Extend Stage 0's `tests/stream_hc_lifecycle_test.mjs`, plus `tests/stream_child_test.mjs` and `tests/stream_native_test.mjs`. Extend `tests/integration_cli.rs` for every save producer, accepted-save/follow-on-error, shared-terminal multi-scope isolation, missing native context and failed notice. Reads send no notice. Existing reporting, artifact, naming and capture tests remain unchanged in meaning. Add locator cases for pre-persistence null/ENOENT, later persistence, ordinary enrollment, external attach followed by NewlyBound, mismatched file/session rejection, no-session after exit and a retained Claude/Codex locator under in-place inherited environment. Verify optional legacy pi_context omission and captured hc context on requests; no file metadata appears in labels. Exercise NativeChanged placement/display through lifecycle, preserving membership on display, startup and observation gaps, and preserve native status, desired_pi_name and non-publication warnings.

#### Stage 5. Ship the thin native views and diagnostics

Create `integrations/herdr-app/mod.json`, `integrations/herdr-app/main.js`, `integrations/herdr-app/frame.js`, `integrations/herdr-app/cache.js`, `integrations/herdr-app/diagnostics.js`, `integrations/herdr-app/viewer.js`, `integrations/herdr-app/package.json`, `integrations/herdr-app/tests/cache.test.mjs`, `integrations/herdr-app/tests/diagnostics.test.mjs`, `integrations/herdr-app/tests/protocol.test.mjs`, `integrations/herdr-app/tests/viewer.test.mjs` and `src/runtime/integration_errors.rs`. Modify `src/runtime/mod.rs`, `src/cli/mod.rs`, `src/cli/integration.rs`, `src/cli/notify.rs`, `src/cli/serve.rs`, `src/cli/serve/native.rs` and `mise.toml`.

Create `integrations/herdr-app/mod.json`, `main.js`, `frame.js`, `cache.js`, `diagnostics.js`, `package.json`, `tests/cache.test.mjs`, `tests/diagnostics.test.mjs` and `tests/protocol.test.mjs`. Use ES modules with Node's test runner and injected fake platform calls; add no npm dependency. The package test command is `node --test tests/*.test.mjs`. Only main.js and frame.js may statically import host modules herdr, gpui-kit or gpui-base. cache.js, diagnostics.js and viewer.js import only portable modules and receive all platform effects through injected callbacks. Export encodeRequest and decodeReply for the strict v1 codec from cache.js; tests/protocol.test.mjs imports those exports and the shared JSON fixtures. Node tests never import main.js/frame.js or their host-module graph. No module loader shim or additional protocol file is needed.

Add `viewer.js` and `tests/viewer.test.mjs` for fixed desktop dispatch. Test exact alias qualification, endpoint/scope/generation rejection, rename-only read projection, in-process command/object dispatch including names/keys beginning with hyphens, live readiness/target checks and named-state comparison, desktop Context.open_url, one-submit budgets, owner success with desktop failure and no cached action replay. Use no supplied command/path executor.

`main.js` renders the sidebar/popup and dispatches the frame role. `frame.js` retains the shipped pane controls. `cache.js` implements one active coordinator per subscribed scope/endpoint pair, generation checks, one dirty follow-up and one ordinary hourly timer. It releases entries on last close, accepts harmless read overlap after reopen, and has no retry queue. `diagnostics.js` writes bounded best-effort snapshots with no deduplication registry. Test failure while Rust is unavailable, redaction, size limits, stale results, hidden subscriptions, route changes, closure/reopen and reload.

Add `src/runtime/integration_errors.rs`, exported from `src/runtime/mod.rs`, for sanitized best-effort 10 MiB rotating Rust diagnostics. Reuse `file_io::try_lock`; a busy lock falls back to stderr. Call it from the CLI integration/notification and private-child error boundaries after catalog locks release. No existing operation outcome is changed to make logging succeed.

Add `test:herdr-app` to `mise.toml` using `npm --prefix integrations/herdr-app test` and include it in the full `test` task's depends list. Append `tests/stream_hc_lifecycle_test.mjs` to the existing node --test run strings of `test:pi:native` and `test:pi`; preserve their existing dependencies. Keep `test:pi:hc-context` as its separately runnable focused task. Keep ordinary tests hermetic; native app checks run only in the explicitly approved live acceptance environment.

#### Stage 6. Review, document and accept the working integration

Create `integrations/herdr-app/README.md` and `docs/hc-herdr-app-integration.md`. Modify `README.md`, `CONTRIBUTING.md`, `docs/pi-child-protocol.md`, `docs/card-preferences-and-control.md` and `docs/testing.md`.

Create `integrations/herdr-app/README.md` with the exact manual clone/configuration/check procedure, original-layout rollback, supported versions, configured remote prerequisites and restart instructions for a rejected/unavailable lifecycle. Add `docs/hc-herdr-app-integration.md` for the shared contract, native ownership rules and the offline association-reset procedure above. Include the same maintenance link in the integration README. Update CONTRIBUTING.md's test-task list with the focused hc lifecycle task, optional installed-SDK task and hermetic mod tests. Verify its conditional clear/conflict/uncertain-reply steps on a disposable core during authorized acceptance. Update `README.md`, `docs/pi-child-protocol.md`, `docs/card-preferences-and-control.md` and `docs/testing.md` for the new backend and commands. Preserve existing Herdr/cmux setup instructions and the explicit Claude/Codex deferral.

Run the configured gates `mise run check:fmt`, `mise run check:clippy`, `mise run test:fast` and `mise run test`. During each implementation stage, first run its focused contract or Node tests, then the broader gate before considering the stage complete. The current unit/scheduling probes are evidence for their own boundaries and replace none of these gates.

Perform an independent code review after implementation and fix blocking findings. Live acceptance then uses disposable catalogs and explicitly approved native setup. Verify local and configured-remote ownership; app-closed launch/reuse followed by app adoption; normal startup/new/resume/fork/reload; rejected overlaps; crash/helper failure; terminal/space moves and closure; empty Streams; all native controls; Follow/pinned navigation; hide/show/preference behavior; notices, hourly/manual refresh; wrong catalog and uncertain results. No catalog progress report or inference is generated by viewing or navigation.

Record observed process starts, native RPCs, enumerated directories and record bytes for representative batches. Add no watcher, service or index to improve a measurement without a separate design justification. If a configured remote host or native behavior cannot be exercised, state the missing acceptance explicitly and keep that support unaccepted.

Delivery is complete only when the supported behavior works live, regression gates and code review pass, and the docs match the tested setup. Publish/install or merge only when separately authorized.

## Design sources

The attached platform API basis is hc 0.1.20 and herdr-app 0.1.15. Later investigation observed installed hc 0.1.21 and Codex 0.169.0. The [retained document-store probe](herdr-app-document-probe.json) and [concurrency analysis](herdr-app-document-concurrency.md) apply to hc 0.1.20. Re-verify API and persistence behavior on the installed target release during authorized disposable-core acceptance. These results establish no power-loss guarantee.

- [Pi lifecycle probe](herdr-app-pi-lifecycle-probe.md) records the failed overlap assumption and its executable counterexamples.
- [Codex identity audit](herdr-app-codex-identity-audit.md) records the current-display interface gap and matching source.
- [Claude identity audit](herdr-app-claude-identity-audit.md) records the direct-interactive query candidate, partial probe and remaining boundaries.

- [Native observation and effects](../docs/native-observe-and-act.md) defines the existing one-observation execution model.
- [Native context reconciliation](../docs/native-context-reconciliation.md) defines verified movement, refresh and cross-host refusal.
- [Owner/desktop investigation](herdr-app-remote-owner-viewer.md) records the revised remote direction and evidence boundaries; canonical contracts are defined in this draft.
- [Name ownership](../docs/name-ownership.md) defines naming preferences and reconciliation.
- [Pi child protocol](../docs/pi-child-protocol.md) describes the current private Rust child and harness source.
- [Installed hc schema](herdr-app-evidence/hc-schema.json) documents doc.get, doc.set and their version preconditions. The retained [hc 0.1.21 event encoder](herdr-app-evidence/hc-v0.1.21-events.rs), [subscription handler](herdr-app-evidence/hc-v0.1.21-api.rs) and [plain stream loop](herdr-app-evidence/hc-v0.1.21-server.rs) define notification framing beyond that schema.
- [herdr-app command and publication implementation](https://github.com/herdrdev/herdr-app/blob/9950887361af3ba218b9ce7a347cb8ec7b66c71d/src/window.rs) establishes that space creation can return before layout publication.
- [herdr-app layout synchronization](https://github.com/herdrdev/herdr-app/blob/9950887361af3ba218b9ce7a347cb8ec7b66c71d/src/mirror_sync.rs) describes background document writes and version conflicts.
- [herdr-app terminal-event application](https://github.com/herdrdev/herdr-app/blob/9950887361af3ba218b9ce7a347cb8ec7b66c71d/src/hc.rs) and [mod input dependencies](https://github.com/herdrdev/herdr-app/blob/9950887361af3ba218b9ce7a347cb8ec7b66c71d/src/mods.rs) establish the label-to-mod notification path.
- [herdr-app folder subscriptions](https://github.com/herdrdev/herdr-app/blob/9950887361af3ba218b9ce7a347cb8ec7b66c71d/src/files.rs) establish recursive coverage, retained workers and the 150 millisecond idle loop.
- [herdr-app task scheduling](https://github.com/herdrdev/herdr-app/blob/9950887361af3ba218b9ce7a347cb8ec7b66c71d/vendor/gpui-kit/crates/shell/src/engine/quickjs/scheduler.rs) defines owner-null tasks, deferred work after rendering and script-generation retirement.
- [Existing artifact keys](../src/artifacts.rs) and [artifact reads](../src/application/artifacts.rs) preserve producing-Stream identity across Session moves.
- [herdr-app conditional file writes](https://github.com/herdrdev/herdr-app/blob/9950887361af3ba218b9ce7a347cb8ec7b66c71d/src/mod_files.rs) provide bounded local diagnostic snapshots. [Streams runtime paths](../src/config.rs) locate owner-host diagnostics; the [current child stderr handling](../extensions/stream-checkpoint/child.ts) drains that stream without retaining it.
- [Codex hook lifecycle](https://learn.chatgpt.com/docs/hooks#sessionend) and [Claude hook lifecycle](https://code.claude.com/docs/en/hooks) describe identity inputs whose live-terminal binding remains unresolved.

Drafted with EGM.
