# Remaining hc and herdr-app work

The [integration design](hc-herdr-app-design.md) defines the initial target behavior. Commit `17043fa` and subsequent commits through `414e0dbe` implement the core Rust contracts, Pi process fencing, hc transport, space associations, and work opening. This plan records the implemented state, the simplifications adopted to keep the system predictable and small, and the remaining work.

The author authorized staged code, documentation and tests. Installation, live catalog or layout changes, and live local or remote acceptance require separate authorization. New hc support targets Pi 1.5.2. Existing Herdr and cmux behavior and existing Claude and Codex saved-conversation behavior must remain supported.

## Complexity budget and operating context

- **Operating context**: Single-user developer workstation (`herdr-app` desktop UI, `hc` local multiplexer core, and `streams` CLI/Pi extension). Callers are interactive user actions, Pi turn checkpoints, and manual/notice-driven view refreshes (human-scale concurrency, tens of Streams/Sessions).
- **Trust & authority boundary**: Catalog YAML records on disk are the sole durable authority for Stream and Session state. `hc`'s `streams` document stores space-to-Stream associations; terminal labels (`streams.pi.*`, `streams.work`, `streams.saved.<sha256>`) are runtime routing/presence/invalidation hints written by the user's own `streams` processes. A verified `streams.pi.*` claim proves the registered helper/SDK/terminal route for the last-published Pi conversation; live transition accuracy across `/new`, `/fork`, and `/resume` remains an acceptance check.
- **Reliability vs. simplicity tradeoffs**:
  - Per-record catalog saves on disk are atomic, while git sync/import reports partial or repair-required outcomes independently; native `hc`/`herdr-app` presentation effects are best-effort follow-ons that never roll back or block accepted catalog saves.
  - Operations use one-shot observe → plan → act without retry loops, background reconciliation daemons, or multi-step compensation.
  - Ambiguous or unavailable native state fails fast and visibly in the command reply (`Unavailable` / `NotApplied` / `Uncertain`) rather than triggering recovery state machines.
- **Out-of-budget concerns**: Host-wide process-table censuses, hostile-tenant label spoofing defenses within the same user UID, automatic catalog membership mutation on UI window/tab drag, multi-version wire migrations, custom rotating diagnostic log subsystems, and high-QPS daemon caching.

## Simplifications from the baseline design

The checked-in [integration design](hc-herdr-app-design.md) remains byte-for-byte unchanged (`7af4638e11611d2793f558485b11decb5565b322b543250be6101e9fadff086`). The following decisions supersede over-designed machinery in that baseline:

1. **Registered Pi sources only (no host-wide census).**
   When opening or resuming work, hc checks Pi sources registered with the Streams integration on the selected core. It reuses verified registered conversations. It does not discover unregistered Pi processes or inspect their arguments or environments. An unregistered Pi can be missed and a second process may open the same conversation. Simultaneous opens have no exclusivity guarantee.

2. **No background hc event-stream watcher in `streams serve` and no automatic membership mutation on terminal moves.**
   Moving a terminal between spaces in `herdr-app` is a UI layout action, not a catalog mutation. The Pi helper does not subscribe to `hc` layout events, track sequence-gap movement baselines, or automatically rewrite Stream membership when a terminal moves. Catalog Stream membership has one writer: explicit user/agent catalog actions.

3. **No companion-visibility label state machine (`streams.companion.*`).**
   In `herdr-app`, the attached strip is a lightweight view in the 30 px pane header layout, not a spawned sidecar process requiring hashed `streams.companion.<sha256>` terminal labels or `OpenCard`/`CloseCard` RPC state machines. Frame reads report the terminal's enrolled Session view directly (`CompanionVisibility::Shown`), and `OpenCard`/`CloseCard` return an explicit unsupported error in `hc`.

4. **No read-driven space/tab rename choreography.**
   `read_views` is a pure read. It does not emit `ViewerAction::RenameSpace` or `RenameTab` queues on every refresh to overwrite user space or tab titles. The `herdr-app` views display Stream and Session titles directly in the sidebar and pane strip.

5. **Direct terminal claim lookup (no foreground process-group tree walk).**
   A live terminal's verified `streams.pi.*` claim identifies its Pi conversation directly, comparing only `ProcessKey` and `parent_pid` along the helper/SDK/terminal-root route. Follow and Frame reads do not require `terminal.foreground.pid` to descend from the Pi SDK process or check process-group/stopped state, so running a tool, subshell, or pager inside Pi never makes the conversation disappear or fail validation.

6. **Direct reference activation (no `FollowFrame` click-time rejection).**
   Clicking a Document, Artifact, or Link validates the saved reference against the catalog (including that a document's `viewing_session` belongs to `stream`) and opens it directly with `context: { kind: "explicit" }` without re-checking terminal foreground PIDs at click time.

7. **No duplicate projection DSL for hc.**
   Legacy Herdr retains its existing projection module; `hc` lifecycle and save follow-ons in `streams serve` directly record the Session directory, bind an unassociated caller space (`HcOperation::bind_caller_terminal`), write the `streams.work` routing hint, and project `desired_pi_name` using `NativeCardsConfig::session_naming()`.

8. **No custom rotating JSONL diagnostic logger or in-memory ring-buffer subsystem.**
   Errors and warnings return through typed command replies and standard logging rather than a bespoke 10 MiB rotating JSONL writer and 256 KiB desktop ring buffer.

9. **Local owner and native `herdr-app` reactivity first.**
   The `herdr-app` mod uses `herdr-app`'s built-in reactive primitives (`spaces()`, `terminals()`, `focused()`, `panes()`, `exec()`, `run()`) against the local `streams` CLI and local `hc` core, treating remote spaces (`space.machine !== null`) as `NotApplicable` rather than building a custom `hc` socket subscriber or remote-owner RPC proxy in JS.

## Implemented checkpoint

| Area | Implemented behavior | Status |
| --- | --- | --- |
| Pi Session ownership | Process-scoped identity and generation checks, obsolete-result rejection, same-ID draft retention, helper close-before-replacement, hc source publication before enrollment, exact SDK locator verification, directory recording, and `NativeCardsConfig::session_naming()` projection | Implemented |
| Shared Rust workflows | Scoped types, saved views, prepared launch/effect results, shared document and artifact validation, Herdr/cmux cutovers, `streams open` / `streams start` / `--current` hc runtime support, Frame reads (preserving `Unavailable` on corrupt records), and document/artifact reference opening in hc | Implemented |
| Owner CLI protocol | Strict version-1 requests and replies, 16 KiB requests, 4 MiB replies, saved-read batching, `ViewSelection::Frame`, `ViewSelection::HostStreams`, `ModAction::OpenWork`, and `ModAction::OpenReference` (Link, Document, Artifact, with optional `mode`/`placement` overrides) and `resolve_executable` verification | Implemented |
| hc connection & save notices | Owner-local socket resolution against the captured operation directory, permanent-core verification, one retained operation connection, typed failure evidence, prepared work/viewer terminal starts and adoptions, `HcOperation::bind_caller_terminal`, and `HcOperation::notify_saved` (`streams.saved.<sha256>` on `HC_TERMINAL_ID`) | Implemented |
| Pi source route | Owner point observation, claim verification (`ProcessKey` + `parent_pid`), helper-exit handling, one-write publication, bounded EOF retirement, registered-source presence reads, direct terminal claim lookup, and in-place Pi resume checks | Implemented |
| Space associations | Checked publication and association snapshots, manual `bind-space`, one retained-version conditional write, explicit uncertainty, association completion during `open_work` and initial enrollment, and `workspace_stream_id` lookup in `agent_observation` | Implemented |
| Sidebar & Frame reads | Saved Stream data, host Stream listing (`HostStreams`), current same-host pinned membership, point inspection of registered in-space Follow candidates, and Frame enrolled Session resolution | Implemented |
| Navigation & `herdr-app` mod | Secure owner `view-document` pager, shared artifact pager, fresh saved-link validation with browser handoff, configurable reference viewer modes/placements (`editor`, `less`, `hunk`, `native`), and `integrations/herdr-app/{mod.json,main.js,state.js,icons/*.svg,layouts/streams.json,tests/state.test.mjs}` | Implemented (composable mod + `Justfile` install recipes) |

## Detailed Implementation Plan

### 1. Pi enrollment follow-on and save notices (`src/runtime/hc.rs`, `src/cli/mod.rs`, `src/cli/serve.rs`, `src/cli/serve/hc.rs`, `src/cli/serve/native.rs`, `src/cli/catalog.rs`, `src/application/lifecycle.rs`, `tests/serve/hc.rs`, `tests/integration_cli.rs`)
- In `src/runtime/hc.rs` (`HcOperation::caller_stream_id`):
  - Check the caller terminal's own space association first, then walk up to 8 terminals (starting at `caller_terminal`) checking `streams.work` labels and ancestor space associations at each hop in nearest-ancestor order so child terminals inherit their parent terminal's Stream.
- In `HcOperation::bind_caller_terminal` (`src/runtime/hc.rs`):
  - Inspect `presentation_for_caller(scope, Some(caller_terminal))`.
  - If the caller terminal sits in an unassociated published space, bind that space via `finish_associations`. If the space is already associated with this Stream, keep it without rewriting; if associated with another Stream or unpublished, leave space associations untouched.
  - Write `"streams.work"` (`labels::work_value(scope, stream)`) on `caller_terminal` via `terminal.label`.
- In `src/cli/serve/hc.rs`, `src/cli/serve/native.rs`, and `src/cli/serve.rs`:
  - Read `session_naming` via `NativeCardsConfig::load(&app.environment, &app.cwd, plugin_root)?.session_naming()`.
  - In `hc`, `SessionStarted` and `Attach` record missing Session cwd (`locations.remember`), emit `ServerFrame::View { view: UiFacts { desired_pi_name } }`, call `bind_caller_terminal` when enrolled, and return `{"backend":"hc","report":<LifecycleReport>}` (`native.kind = "active"` when enrolled, `native.kind = "inactive", reason = "not_enrolled"` when unenrolled).
  - On `Enroll` and `Apply` in `hc`, preserve their existing catalog response shapes, run `HcSource::after_save` to project `desired_pi_name`, bind/route the caller terminal, and emit the save notice (`notify_saved`), sending any native follow-on failure through `Server::warning` so accepted saves never turn into `SavedWithWarnings` or errors.
  - On `Request::Lifecycle { action: "resume" | "park" }` (`src/cli/serve.rs`, `src/cli/serve/dispatch.rs`), run `resume_session` or `park_session` (reopening a `closed` Stream on resume without writing no-op receipts when unchanged), preserve partial saves and `completion_error`, and emit `after_save(..., bind_space = false)` / `controller.saved` notices when records change.
  - In `extensions/stream-checkpoint/index.ts`, interactive TUI Sessions call `lifecycle("resume")` on `session_start` and `lifecycle("park")` on `session_shutdown` symmetrically across all frontends, while non-interactive (`print`, `json`) and ephemeral (`--no-session`) Pi runs skip Streams startup and reporting.
  - Read paths (`HostStreams`, `StreamView`, `streams list`) remain read-only observational and project live Session activity plus `lifecycle.decay_days` via `project_stream_status`; `streams tidy [--days N]` persists `closed` on disk with revision and membership guards (skipping Stream closure if any child Session was concurrently updated).
  - In `src/cli/mod.rs`, populate `workspace_stream_id` in `agent_observation` and `--current` via `HcOperation::caller_stream_id`, and emit `notify_hc_save` on one-shot CLI saves (including `streams sync`, `streams tidy`, and `streams session lifecycle`) when `HC_TERMINAL_ID` is set.

### 2. Thin `herdr-app` JavaScript mod (`integrations/herdr-app/mod.json`, `integrations/herdr-app/package.json`, `integrations/herdr-app/state.js`, `integrations/herdr-app/main.js`, `integrations/herdr-app/icons/*.svg`, `integrations/herdr-app/layouts/streams.json`, `integrations/herdr-app/tests/state.test.mjs`, `Justfile`)
- `integrations/herdr-app/state.js` implements `resolveRole` (honoring explicit `role`, then `kind === "reference-viewer"` → `"viewer"`, `kind === "session-popup"` or `"stream-menu"` → `"popup"`, `pane`/`terminal` → `"frame"`, otherwise `"sidebar"`) and the portable `StreamsCoordinator`:
  - Calls `exec(streamsBin, ["integration", "describe", "--frontend", "hc"])` once for local spaces (`space.machine === null`) and marks remote spaces (`space.machine !== null`) as `NotApplicable`.
  - Uses reactive `herdr-app` hooks and `streams.saved.<sha256>` terminal labels (plus a 5-minute `STALE_FALLBACK_MS` staleness check and manual Refresh) to batch `ViewSelection::Frame` for visible tab panes, `ViewSelection::SavedStream` when enrolled, and `ViewSelection::HostStreams` on unenrolled tabs into `streams integration request --request-json <json>`, discarding stale async read generations.
  - Dispatches `ViewerAction` items once on fresh `open_work` and `open_reference` replies (`SelectSpace` → `run("space.select", { id })`, `FocusTerminal` → `run("pane.focus", { terminal })`, `OpenUrl` → `asyncCx.open_url(url)`), plus `launch_command` / `show_content` reference presentations across `pane-right`, `pane-down`, `tab`, and `floating` (`native` only).
- `integrations/herdr-app/main.js` renders `"frame"`, `"sidebar"`, `"viewer"`, and `"popup"` roles from `props()`, keeping the 30 px `pane` header parity (`Split`, `Zoom`, `Close`, `cwd`, status indicators) and adding an inline `<status symbol> <session title>` label on enrolled terminals (plus viewer metadata and `"Open in editor"` on truncated `streams` native viewer panes). Clicking the session label selects `props()?.panel_id ?? "streams"` in `panel-body` and shows or hides `panel`, or opens `session-popup` when no panel exists. The tab-local `Streams` sidebar shows the active Session rail, Stream `References` with bundled `icons/*.svg`, collapsible sibling `"Other sessions"` on enrolled tabs, and clickable host Streams on unenrolled tabs. A failed host listing shows its error, which names the unreadable record, in place of the list; `hostStreamsError` is written only by the `HostStreams` read, so a later successful selection in the same batch cannot hide it.
- `integrations/herdr-app/layouts/streams.json` provides a standalone layout based on Herdr's stock `main.json` with `{ "mod": "streams" }` in `pane.header.content` (zero-boilerplate role inference) and `{ "id": "streams", "type": "block", "content": { "mod": "streams", "props": { "label": "Streams", "icon": "≋" } } }` prepended to `panel-body.children`.
- `Justfile` (with tool versions including `jj` in `.tool-versions`) provides `just install-cli` (installs the release `streams` binary into `~/.local/bin/streams`), `just install-herdr-app-mod` (installs `mod.json`, `main.js`, `state.js`, and `icons/*.svg` into `${HERDR_APP_HOME:-~/.config/herdr-app}/mods/streams/` and runs `herdr-app check` when available, without touching layouts), `just install` (runs `install-cli` and `install-herdr-app-mod`), and `just install-herdr-app` (also installs `integrations/herdr-app/layouts/streams.json` into `${HERDR_APP_HOME:-~/.config/herdr-app}/layouts/streams.json`).
- `integrations/herdr-app/tests/state.test.mjs` and `Justfile` (`test-herdr-app`) verify role resolution, batching, save-notice reactivity, stale-read rejection, viewer metadata binding, host Stream inspection, and one-shot `ViewerAction` dispatch.

### 3. User documentation and regression gates (`README.md`, `docs/hc-herdr-app-plan.md`)
- Document `hc` and `herdr-app` usage and setup boundaries in `README.md` and `docs/hc-herdr-app-plan.md`.
- Run `just check-fmt`, `just check-clippy`, `just test-pi-hc-context`, `just test-herdr-app`, and `just test`. Live installation and live local/remote acceptance remain separately gated.

