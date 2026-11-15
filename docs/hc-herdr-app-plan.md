# Remaining hc and herdr-app work

The [integration design](hc-herdr-app-design.md) defines the target behavior. Commit `17043fa` contains a tested implementation checkpoint. Full native support remains incomplete. This plan separates that checkpoint, the remaining delivery work and the evidence still needed for acceptance.

The author authorized staged code, documentation and tests. Installation, live catalog or layout changes, and live local or remote acceptance require separate authorization. New hc support targets Pi 1.5.2. Existing Herdr and cmux behavior and existing Claude and Codex saved-conversation behavior must remain supported.

## Registered Pi sources

When opening or resuming work, hc checks Pi sources registered with the Streams integration on the selected core. It reuses verified registered conversations. It does not discover unregistered Pi processes or inspect their arguments or environments. An unregistered Pi can be missed and a second process may open the same conversation. Simultaneous opens have no exclusivity guarantee.

A malformed or unverifiable registered source is reported as unavailable. It is never treated as absent. A source belonging to an exited helper can be retired without deleting a replacement source. The implementation must distinguish a positively exited helper from an observation failure.

This decision replaces the host-wide Pi census, launch-profile classification and global unknown-Pi blocking described in the original design. The checked-in design remains the verbatim baseline; this plan records the later decision.

## Implemented checkpoint

| Area | Implemented behavior | Remaining boundary |
| --- | --- | --- |
| Pi Session ownership | Process-scoped identity and generation checks, obsolete-result rejection, same-ID draft retention, helper close-before-replacement, hc source publication before enrollment and exact SDK locator verification | NewlyBound completion and claim accuracy through live Pi transitions |
| Shared Rust workflows | Scoped types, saved views, prepared launch/effect results, shared document and artifact validation, lifecycle/projection policy and Herdr/cmux cutovers | Complete facade-level hc opening, resumption and lifecycle orchestration |
| Owner CLI protocol | Strict version-1 requests and replies, 16 KiB requests, 4 MiB replies and saved-read batching | Native frame reads and launch/navigation effects |
| hc connection | Owner-local socket resolution against the captured operation directory, permanent-core verification, one retained operation connection, typed failure evidence and prepared single-terminal starts | Connect starts to work opening; complete subscriptions and general frontend activation |
| Pi source route | Owner point observation, strict claim-key and ancestry checks, positive helper-exit handling, one-write publication, bounded EOF retirement, registered-source presence reads and in-place Pi resume checks | Connect presence reads to work opening and verify live transitions |
| Space associations | Checked publication and association snapshots, manual `bind-space`, one retained-version conditional write and explicit uncertainty | Association completion as part of launch, adoption, enrollment and moves |
| Sidebar reads | Saved Stream data, current same-host pinned membership and point inspection of registered in-space Follow candidates | Live Pi registration and transition acceptance |
| Navigation | Secure owner `view-document` pager, shared artifact pager and fresh saved-link validation with a typed browser handoff | Native placement of pagers and desktop browser dispatch |

The [owner dispatcher](../src/cli/integration.rs), [shared facade](../src/application/integration.rs), [hc transport](../src/frontend/hc.rs) and [association adapter](../src/runtime/hc.rs) implement these boundaries. The [CLI contracts](../tests/integration_cli.rs) and [hc contracts](../tests/integration_hc.rs) distinguish unavailable behavior from empty context and partial completion.

The checkpoint passed the complete serial test task, including 195 library tests, 55 binary tests, 316 contracts and 5 verification tests. It also passed 406 Pi tests, 29 Herdr boundary tests, 3 replay tests and 19 native Rust tests. Separate fixtures passed 13 Pi lifecycle tests and 2 selected Pi 1.5.2 runtime tests. Clippy, formatting and document checks passed. These results establish the exercised fixture and regression behavior. Live publication across Pi transitions and native application behavior remain unaccepted.

## Finish the native adapter

This is the remainder of implementation stage 3. Its exact file boundaries and test groups remain in the design's [staged delivery plan](hc-herdr-app-design.md#staged-delivery-and-file-boundaries).

1. Connect registered-source presence reads to work opening. In-place Pi resume now refuses a verified running source or unavailable evidence and proceeds only on registered absence. It preserves exact verified resume inputs and inherited terminal environment; Claude and Codex retain their existing verified resume behavior. The retained source snapshot feeds the shared opening classifier and distinguishes registered absence from unavailable evidence. A scope-bound view separately reads `streams.work` routing hints; malformed hints make routing unavailable without changing conversation presence. It selects adoption candidates only from requested verified conversations that have neither a published placement nor existing placement labels. Existing labels may represent a pending request, so they are left alone. A terminal also stays in place when it contains a verified conversation outside the selected set. Unscoped routing is unavailable, and unregistered processes are not inspected. Sidebar Follow inspects only its supplied in-space terminal. Space-only reads inspect no processes. hc's list operation also requests configured remote machines, but the owner considers only addresses on its selected core.
2. Complete claim uniqueness and lifecycle publication around helper-owned source registration and retirement. The integration's publication supplies the conversation identity; process observations verify its route and liveness. Report malformed, competing or unverifiable registered sources explicitly. Unregistered Pi processes do not block replacement launches.
3. Complete shared `open_work` and `prepare_resume` orchestration with the hc execution snapshot. The facade selects exact requested current Sessions or active local members before native effects. The hc workspace planner classifies that selection, verifies exact saved locators and directories and selects only verified unplaced terminals; unavailable evidence cannot create an empty-shell fallback. The owner effect boundary applies the prepared plan once and retains each launch, adoption and placement outcome. This path is now connected to the admitted facade, `resolve_frontend` Auto/explicit selection, `streams open`, `streams start` and `--current` reverse lookup.
4. Connect prepared terminal creation and selected unplaced-terminal adoption to work opening through the existing placement labels. A prepared start retains exact owner-resolved argv, cwd and approved environment and requests a fresh tab and independent terminal lifetime. Confirmation requires the returned argv and cwd to match and the parent to be absent. A prepared adoption writes only its two placement labels to a selected, verified unplaced terminal. Both effects report unconfirmed results as uncertain without replay. Requested placement can be finished by one fresh published-layout read followed by at most one association write using the retained version; an unpublished space remains Requested, and only positive layout evidence makes it Known. Preserve Requested and Known space evidence independently from execution. After placement, perform the designed additional publication read and at most one association write. Report accepted terminals even when publication, binding or desktop presentation fails.
5. Complete frame identity, in-space Follow, naming, companion display and owner-side document/artifact terminal placement. Add full native request dispatch through the facade. Ordinary refresh and reattachment keep dismissed companions closed.
6. Add the separate plain event subscription, retained sequence handling and invalidation-only dispatch. Missed or disconnected observation clears the movement baseline. A fresh snapshot may establish a new baseline. It supplies no evidence of a move across the gap.

Verify exact fake-socket requests, complete and incomplete point observations, PID reuse, competing and stale claims, foreground shells with background Pi, stopped SDKs, lost replies and partial launch outcomes. Verify publication-before-binding, no mutation replay and unchanged Herdr/cmux behavior. New processes and tests must use the existing locked dependencies.

## Connect Pi registration and save notices

Implementation stage 4 connects the tested Session state to the native adapter. The [Pi binding contract](hc-herdr-app-design.md#pi-conversation-binding-in-hc) defines startup fields, source claims, request context and shutdown ordering.

The helper registers interactive context before enrollment or transcript persistence and receives captured conversation context on admitted requests. Before publishing, it rejects a competing live claim for the same SDK, malformed claims and unavailable observations; positively exited claims are left in place. This snapshot check is not a concurrency guarantee. Registration failure leaves record-only catalog work available. Existing Herdr startup-failure behavior remains unchanged. The helper verifies an exact supplied locator against the bound Pi identity before retaining it for an enrolled Session; a missing file is deferred and invalid data leaves the existing locator intact. Catalog reads remain independent while locator work follows admission order on the native worker. NewlyBound directory completion remains to be connected.

EOF queues retirement of the helper's own source on the native worker before cancelling its worker token. The bounded shutdown deadline and helper-specific key preserve replacement sources; Drop performs no hc I/O. Fake-core tests cover EOF, cancellation, timeout and forced death. Test late results and the live transition path.

Connect started, newly-bound, reattached, display-change and placement-change handling through the shared lifecycle policy. Only positive observed same-owner movement may change membership. Startup, refresh and gaps in observation preserve membership. Accepted saves survive native follow-on failures.

Emit one best-effort scoped save notice after each completed save or confirmed import, outside catalog locks. Cover ordinary CLI producers, background membership completion and partial imports. Notice failure becomes a separate warning and cannot alter a save or publication result. Add no identity heartbeat.

Verify ordinary new, fork, resume, startup resume selection and reload through the supported Pi path. Retain cases for cancelled or overlapping transitions, switching away and back, stale orphan publications and delayed old responses. Fixtures using the selected SDK establish runtime ordering only. They do not prove that a live native claim follows the active conversation.

## Build desktop views and diagnostics

Implementation stage 5 adds the [thin JavaScript mod](hc-herdr-app-design.md#thin-javascript-mod). Owner Rust performs catalog, process, terminal and association work. The desktop renders saved fields and applies checked fixed focus, naming and browser actions.

Implement the sidebar, attached strip and progress popup while retaining native controls, Files and Changes, and the single 30 px header. Use one subscribed coordinator per scope and endpoint. Refresh on relevant notices, manual Refresh and one hourly safety timer. Include retained hidden views. Reject obsolete generations and routes, and replay no cached action.

Include configured-remote owners in the initial delivery. Retain the exact execution route and qualify resources with its configured alias. Validate scope, permanent core, generation, readiness and target existence before desktop effects. Owner paths, file contents, commands, environments and raw errors stay off desktop RPC.

Implement the specified bounded diagnostics. Owner JSONL keeps 10 MiB and one previous file. The app retains the newest 100 entries within 256 KiB. Logging is best effort and cannot change saved or native outcomes.

Keep host-only imports in the entry and frame modules. Test codecs, cache behavior, route changes, hidden subscriptions, viewer dispatch and diagnostic limits with injected platform calls and the shared version-1 fixtures. Add the hermetic native-mod test task to the aggregate gate.

## Document and accept the complete integration

Implementation stage 6 adds user-facing setup, maintenance and rollback instructions. Preserve existing frontend setup and the explicit deferral of new Claude/Codex hc implementations.

Run focused tests after each implementation sequence and the complete configured regression gates before accepting a checkpoint. Preserve the Git-sync ten-second deadline and publication assertions. Obtain an independent code review of the completed implementation.

Live acceptance requires separate authorization and disposable catalogs and native setup. Exercise local and configured-remote ownership, app-closed launch followed by adoption, Pi transitions, moves and observation loss, empty Streams, controls, pinned/Follow navigation, hide/show behavior, notices and hourly/manual refresh. Measure process starts, RPCs, directory inventories and record bytes. Record any unavailable environment as missing acceptance.

Full delivery requires passing regression gates, code review and supported live behavior, with setup documentation matching the tested installation. Commit and test evidence alone establishes no installation or remote acceptance.

## Open questions and potential issues

### Active conversation through Pi transitions

Does the published claim continue to identify the active conversation through ordinary new, fork, resume, reload and overlapping transitions? The selected SDK fixtures establish callback ordering and process-owned state handling. A surviving helper can still publish an obsolete conversation if the live transition path leaves it behind. Verify publication through those paths before allowing identity-dependent native effects. Preserve Pi's normal commands and keep uncertain identity unavailable.

### Registered source liveness

Can supported owner hosts reliably verify the registered helper, SDK and terminal root during process changes and permission failures? The aggregate observation keeps verified running locations on unaffected terminals while incomplete evidence makes unobserved conversations Unknown. A malformed or competing claim makes its terminal unavailable for reuse or adoption. macOS and Linux need different point observations. The point reader treats a different observed process key or a kernel ESRCH as exit evidence; other observation failures remain unavailable. Focused fixtures cover a replacement and a reaped child, but supported live transitions and permission failures still need acceptance. Verify that PID reuse cannot revive a stale source and that a malformed or competing registered source produces a clear result. An unregistered Pi remains outside this check.

### App closure and remote placement

Will terminals with the chosen placement labels appear in the intended space after the app reopens, both locally and through a configured remote route? Source inspection supports the mechanism; the complete interaction still needs disposable-environment testing. Verify late publication and association conflicts while preserving every acknowledged terminal. An adoption uses a retained observation followed by an unconditional label write; a concurrent placement change between them is not excluded. Desktop focus, naming or browser failure must leave the owner result intact.

### Event gaps and source retirement

Can EOF retirement finish before cancellation, remove only its own source and preserve a replacement during shutdown races? Test cancelled work, lost replies, helper death and the five-second grace limit. Also verify subscription framing and gaps on the target hc version. A missed event clears the movement baseline; a later snapshot cannot establish a move across that gap.

### UI lifecycle and host limits

Do the native views preserve controls, the single header and Files and Changes across empty spaces, hidden copies and reloads? Verify timer and subscriber ownership, stale route rejection, supported invocation sizes and diagnostic limits on local and remote hosts. Cached data must never replay focus, browser or launch actions.

### Design and implementation alignment

Stage boundaries need comparison with the current code as native dispatch, lifecycle responses and saved-start result mapping are connected. Keep unavailable behavior explicit until its owner operation exists. Preserve the existing Herdr/cmux output shapes throughout those changes.

### Evidence available to future readers

The checked-in design is a byte-for-byte copy of the original draft. Some relative evidence links refer to local untracked extracts and investigations, which are absent from a clean checkout. Preserve any evidence needed for future decisions in separate repository files after checking it for private configuration or paths. Keep the baseline text unchanged.

The baseline SHA-256 is `7af4638e11611d2793f558485b11decb5565b322b543f250be6101e9fadff086`. This companion plan holds implementation status and remaining questions separately from that baseline.
