# Unified TOML Configuration and Reference Viewer

Streams keeps all user configuration in one TOML file at `$XDG_CONFIG_HOME/streams/config.toml` (`~/.config/streams/config.toml` by default), owned by the Rust CLI, and opens saved references in `herdr-app` with a configurable viewer and placement. Moving three JSON files into one TOML schema eliminates duplicate session-naming flags across `config.json`, `work-cards.json`, and `stream-reporting.json`, supports `jj config`-style table and leaf operations, and keeps Pi and `herdr-app` extensions free of direct config file access.

## Background and Terms

In Streams, a Stream is a durable unit of work tracked in the catalog, and a Session is an agent conversation enrolled in a Stream. A Reference is an item attached to a Stream in the sidebar's `References` list. A Reference is one of three kinds, namely a Document (a file path inside a registered repository checkout), an Artifact (a named text output stored inside the catalog), or a Link (a pull request, issue, or web URL).

When a person clicks a Document or Artifact in the `herdr-app` sidebar under default settings (`herdr.viewer.mode = "editor"`, `herdr.viewer.placement = "pane-right"`), `mods/streams` captures the active tab and pane, asks `streams integration` to validate the reference and resolve `$EDITOR`, and opens a right-hand split pane running `$EDITOR` on that file in the checkout directory. When `herdr.viewer.mode` is `"native"`, `streams integration` returns bounded text, and `mods/streams` renders Markdown or source text directly inside `herdr-app`.

## Problem

Streams currently splits settings across three JSON files in two directories.

1. `$XDG_CONFIG_HOME/streams/config.json` holds `catalog.root`, `frontend.mode`, `naming.stream`, `naming.session`, and `cards.auto_open`.
2. `$PI_CODING_AGENT_DIR/work-cards.json` (`~/.pi/agent/work-cards.json` by default) holds `workspace_naming`, `tab_naming`, `session_naming`, `document_command`, `herdr_timeout_ms`, and `pane_height_fraction`.
3. `$PI_CODING_AGENT_DIR/stream-reporting.json` (`~/.pi/agent/stream-reporting.json` by default) holds Pi reporting settings (`reporting`, `enrollment`, `frequency`, `context`, `model`, `session_logging`, `debug_commands`, and `pi_name_edits`).

This split creates three problems. First, session naming is duplicated and spread across all three files (`naming.session` for title generation, `session_naming` and `pi_name_edits` for Pi sync), while `herdr-app` reads `tab_naming` from a Pi agent file. Second, `streams config get` accepts only five hardcoded keys and rejects section lookups or edits for Pi and Herdr settings. Third, opening a document or artifact in `herdr-app` always launches `less` in a new unfocused tab, with no option for `$EDITOR`, `hunk`, or a native `herdr-app` view, and no option for side splits or floating windows.

## Goals

- Consolidate configuration into `$XDG_CONFIG_HOME/streams/config.toml` (default `~/.config/streams/config.toml`) and retire `config.json`, `work-cards.json`, and `stream-reporting.json`.
- Separate tool-neutral core settings (`catalog`, `frontend`, `naming`) from per-tool namespaces (`herdr`, `pi`) so future adapters fit beside them.
- Extend `streams config` with `list`, `get` (for both tables and leaf keys), `set`, `unset`, and `path`.
- Route all extension and mod config reads and writes through the `streams` binary (`streams serve` child requests and `streams integration` replies).
- Support `herdr.viewer.mode` (`editor`, `less`, `hunk`, `native`; default `editor`) and `herdr.viewer.placement` (`pane-right`, `pane-down`, `tab`, `floating`; default `pane-right`) for saved documents and artifacts.

## Operating Assumptions, Observable Guarantees, and Non-Goals

### Operating Assumptions

Callers run on a single-user workstation as interactive CLI commands, the local `herdr-app` desktop window, and Pi extension child processes under the same user account.

Under the repository's alpha policy, runtime code reads and writes only `$XDG_CONFIG_HOME/streams/config.toml` and performs no automatic JSON migration. Streams removes the old JSON readers and writers from the codebase; any existing JSON files left on disk are ignored and left for the user to delete manually, and prior JSON overrides do not carry over.

`herdr-app` (`mods/streams`) operates on local spaces only (`space.machine === null`), matching the local-owner scope in `docs/hc-herdr-app-plan.md`.

### Observable Guarantees

1. Failed config writes preserve existing settings. Unknown sections, unknown keys, type mismatches, incomplete `[pi.reporting.model]` tables, invalid enums, and broken config symlinks fail before locking or touching `config.toml`, leaving the previous file contents and defaults intact.
2. Extensions never read or write config files directly. TypeScript (`extensions/stream-checkpoint`) and QuickJS (`integrations/herdr-app`) obtain and mutate settings solely through `streams`. When `streams` is unavailable or returns an error, the caller reports that error and does not fall back to disk files.
3. Reference validation stays in Rust before any viewer opens. `streams` verifies Stream membership, checkout registration, `O_NOFOLLOW` regular-file status (`pager::observe_file`), the 4 MiB byte ceiling (`DOCUMENT_BYTES`), and `$EDITOR` or `hunk` availability before returning a launch command or bounded content. Failed validation leaves panes untouched and displays the error reason in the sidebar.
4. Native content never exceeds transport limits or exposes host paths. Both Markdown and plain text or artifacts in `native` mode are rejected if binary (NUL bytes or invalid UTF-8) and capped at 400 KiB on a UTF-8 boundary with an explicit truncation notice, keeping `ModReply` well below its 4 MiB wire ceiling and keeping filesystem paths out of `ModReply`.
5. Viewer fallbacks and placement errors stay explicit. When `herdr.viewer.mode` is `editor` and `$EDITOR` is unset or empty, Documents fall back to `less`. When `herdr.viewer.mode` is `editor` or `hunk` and the target is a catalog Artifact (which has no checkout path), Streams falls back to `streams artifact view` (`less`). When `$EDITOR` or `hunk` is explicitly configured or chosen but its binary cannot be resolved, or when `floating` placement is paired with a terminal mode (`editor`, `less`, or `hunk`), Streams returns `not_applied` with a clear reason and opens no pane.

### Non-Goals

- Automatic migration from `config.json`, `work-cards.json`, or `stream-reporting.json`.
- Project-local `.streams.toml` overrides, per-layout `props` viewer overrides, or multi-file config merging.
- `pane-left` and `pane-up` split placements (unsupported natively by `herdr-app`'s `pane.split`).
- `floating` placement for terminal viewer modes (`editor`, `less`, `hunk`); `floating` is supported for `native` mode only.
- Inline commenting inside `hunk open` (Hunk limits commenting to `hunk diff` and `hunk patch` sessions).
- Adding `cmux` or `codex` viewer implementations in this change.

## Architecture and Design

### 1. Unified TOML Config Schema

`src/config.rs` owns `$XDG_CONFIG_HOME/streams/config.toml` (using the existing `toml = "=0.8.23"` crate), default resolution, and `.catalog-config.lock`. `WORK_CARDS_CONFIG_PATH` is retired. Every setting has one canonical dotted key.

| Key | Type / Values | Default | Role |
| :--- | :--- | :--- | :--- |
| `catalog.root` | absolute path or `~/` | `$XDG_DATA_HOME/streams/catalog` | Owner catalog directory |
| `frontend.mode` | `auto`, `none`, `herdr`, `hc`, `cmux`, or adapter ID | `auto` | Active workspace frontend policy |
| `naming.stream` | `automatic`, `manual` | `automatic` | Default Stream title generation policy |
| `naming.session` | `automatic`, `manual` | `automatic` | Default Session title generation policy |
| `lifecycle.decay_days` | non-negative integer | `14` | Days before inactive work projects and tidies to `closed` (`0` disables) |
| `herdr.viewer.mode` | `editor`, `less`, `hunk`, `native` | `editor` | Viewer for documents and artifacts |
| `herdr.viewer.placement` | `pane-right`, `pane-down`, `tab`, `floating` | `pane-right` | Where `herdr-app` places the opened viewer |
| `herdr.naming.tab` | boolean | `true` | Mirror enrolled Session title onto `herdr-app` tab |
| `herdr.naming.workspace` | boolean | `true` | Mirror Stream title onto legacy Herdr workspace |
| `herdr.cards.auto_open` | `automatic`, `manual` | `automatic` | Automatic opening for legacy Herdr cards |
| `herdr.timeout_ms` | positive integer (`1..=2147483647`) | `3000` | Legacy Herdr socket timeout |
| `herdr.pane_height_fraction` | float (`0.0 < x < 1.0`) | `0.2` | Legacy Herdr card strip height fraction |
| `pi.naming.session` | boolean | `true` | Project saved Session title into Pi |
| `pi.naming.import_edits` | boolean | `true` | Import Pi `/name` edits as manual Session titles |
| `pi.reporting.mode` | `automatic`, `manual` | `automatic` | Automatic progress checkpoints after turns |
| `pi.reporting.enrollment` | `automatic`, `manual` | `automatic` | Automatic enrollment into a Stream |
| `pi.reporting.frequency` | integer (`>= 1`) | `1` | Turns between automatic progress reports |
| `pi.reporting.context` | `trimmed`, `full` | `trimmed` | Conversation history passed to reporting model |
| `pi.reporting.model` | table `{ provider, id }` (optional) | unset (uses Pi model) | Reporting inference model override |
| `pi.reporting.session_logging` | boolean | `false` | Save helper inference transcripts locally |
| `pi.reporting.debug_commands` | boolean | `false` | Enable `/streams debug` commands |

`[pi.reporting.model]` requires both non-empty `provider` and non-empty `id` when present in `config.toml`; partial tables missing either field fail validation.

`src/runtime/native_cards/config.rs` (`NativeCardsConfig::load`) reads `herdr.*` and `pi.naming.session` from `streams::config::Config`, replacing `work-cards.json`. Its legacy Herdr call sites read `herdr.naming.workspace`, `herdr.naming.tab`, `herdr.cards.auto_open`, `herdr.timeout_ms`, and `herdr.pane_height_fraction`, while `src/cli/serve/hc.rs` reads `pi.naming.session` for Pi session title projection and `src/runtime/native_cards/documents.rs` resolves `$EDITOR` (falling back to `less`).

### 2. `streams config` CLI

`ConfigCommand` in `src/cli/args.rs` follows `jj config` over the effective tree (defaults merged with on-disk overrides in `config.toml`).

- `streams config [--overrides]` or `streams config list [--overrides]` prints the effective configuration (or only configured overrides) as TOML in plain mode, or as a nested JSON object under `--json`.
- `streams config get <key>` accepts any valid table or leaf path (`herdr`, `herdr.viewer`, `pi.reporting.model`, `herdr.viewer.mode`). Plain mode renders TOML for tables and raw text for scalars; `--json` renders the JSON value or subtree. Unknown keys or unset optional keys (`pi.reporting.model` when absent) exit nonzero with a clear error.
- `streams config set <key> <value>` parses `<value>` strictly for the target leaf key (`true`/`false` for booleans, parsed integers/floats for numeric keys, strings for enums/paths) and supports setting `pi.reporting.model <provider>/<id>` atomically (splitting on the first `/` with non-empty `provider` and `id`). Setting `.provider` or `.id` alone when `[pi.reporting.model]` is unset fails whole-config validation to prevent writing a half-configured table. Both `--json` and plain mode emit the updated value and best-effort call `notify_hc_save()` when an opened catalog and `HC_TERMINAL_ID` exist.
- `streams config unset <key>` removes a leaf key or the `[pi.reporting.model]` table (pruning empty parent tables) under `.catalog-config.lock` so the setting reverts to its default, emitting the unset confirmation (JSON scalar/null under `--json`) and invoking `notify_hc_save()`.
- `streams config path` prints the resolved `config.toml` path (JSON string under `--json`).

### 3. Single-Boundary Config Access for Extensions

In the Pi child protocol (`src/child_protocol.rs`, `src/cli/serve/dispatch.rs`), three typed requests replace direct JSON file access in `extensions/stream-checkpoint/settings.ts`.

- `Request::GetConfig {}` returns `{"config": <ConfigSnapshot>}` containing effective `naming`, `pi`, and `herdr` fields needed by Pi hooks and `/streams settings`.
- `Request::SetConfig { key: String, value: String }` validates and mutates `config.toml` via `src/config.rs`, invokes `notify_hc_save()`, re-emits `ServerFrame::View` when `pi.naming.session` changes, and returns `{"config": <ConfigSnapshot>}`.
- `Request::UnsetConfig { key: String }` unsets `key` under lock, invokes `notify_hc_save()`, re-emits `ServerFrame::View` if `pi.naming.session` changed, and returns `{"config": <ConfigSnapshot>}`.

Existing `Request::CardSettings`, `Request::SetCardMode`, and `Request::SetTitleDefault` remain on the child protocol for per-conversation card state (`$XDG_STATE_HOME`) and convenience updates, with `CardTarget::Global` backed by `herdr.cards.auto_open`.

In `extensions/stream-checkpoint/index.ts`, an in-memory `config` object starts with schema defaults so synchronous callers (`getArgumentCompletions` and `registerLabTools.enabled`) never block on I/O. It refreshes asynchronously via `GetConfig` on `useContext` and `session_start`, refreshes on `SetConfig` and `UnsetConfig` responses, and drives `/streams settings` (`settingsDialog`). The `/streams settings` menu keeps only `pi.*` (`pi.reporting.*`, `pi.naming.*`) and `naming.*` (`naming.stream`, `naming.session`) rows, matching the TOML keys and values directly without `herdr.*` rows. In `extensions/stream-checkpoint/lab-cli.mjs`, `StreamsChild` and `StreamCli` start before checking `(await client.getConfig()).pi.reporting.debug_commands`, and help messages refer to `streams config set pi.reporting.debug_commands true`.

In the `herdr-app` integration (`src/integration.rs`, `src/cli/integration.rs`), `ModResult::Read` replaces `tab_naming: bool` with `herdr_config: HerdrConfig` containing `{ tab_naming: bool, viewer_mode: ViewerMode, viewer_placement: ViewerPlacement }`. `mods/streams` uses `herdr_config.tab_naming` for live tab renaming while Rust remains the sole decision point for viewer resolution on each `OpenReference` action.

### 4. Configurable Reference Viewer and Placement

Opening a saved reference routes every validation and policy choice through Rust and executes placement synchronously in `herdr-app` with zero polling or background `terminal.keep` calls.

First, `integrations/herdr-app/main.js` captures the originating context synchronously inside the reference row `on_click` handler before calling `clickCx.spawn(async (asyncCx) => { ... })`. Specifically, it captures `spaceId` and `tabId` from the active tab plus `targetPaneId` (`focused()?.pane ?? panes().find(p => p.tab === tabId && p.focused)?.pane ?? panes().find(p => p.tab === tabId)?.pane`), keeping `AsyncContext` alive across the `await coordinator.openReference(...)` call.

Second, `integration_open_reference` in `src/cli/integration.rs` reads `herdr.viewer.mode` and `herdr.viewer.placement` from `Config` and resolves the reference by kind.

- Links (`pr`, `issue`, `link`) call `open_link` as before and return `ReferenceOpenResult::NeedsViewer { value: [ViewerAction::OpenUrl { url }] }`.
- Documents and Artifacts in `floating` placement with `editor`, `less`, or `hunk` mode return `ReferenceOpenResult::NotApplied { reason: "floating placement requires herdr.viewer.mode = native" }`.
- Documents and Artifacts in terminal modes (`editor`, `less`, `hunk`) with `tab`, `pane-right`, or `pane-down` validate Stream membership, checkout containment (for Documents), `pager::observe_file` (`O_NOFOLLOW` regular file) and the 4 MiB `DOCUMENT_BYTES` limit, plus executable resolution before returning `ReferenceOpenResult::LaunchCommand { title, args, placement }`. For Documents in `editor` mode, if `$EDITOR` is unset or empty, Streams selects `less`; otherwise it trims `$EDITOR`, splits on ASCII whitespace into `[program, ..flags]`, rejects control characters, and resolves `program` via `resolve_executable` (returning `NotApplied` if unavailable). In `hunk` mode, Documents verify `resolve_executable("hunk", ...)` (returning `NotApplied` if unavailable). Artifacts in `editor` or `hunk` mode fall back to `less`. The returned `args` array contains only safe `streams` subcommand arguments (`["integration", "view-document", "--mode", <mode>, "--scope-json", <json>, "--reference-json", <json>]` or `["artifact", "view", "<stream/name>"]`), exposing zero host paths or `$EDITOR` flags in `ModReply`. When `herdr-app` spawns the viewer terminal, `streams integration view-document` re-verifies the saved reference, changes working directory to `checkout_root`, and `exec`s the resolved program (`$EDITOR`, `hunk open`, or `less` with `LESSSECURE=1`).
- Documents and Artifacts in `native` mode (`tab`, `pane-right`, `pane-down`, or `floating`) read Documents through `pager::observe_file` + `pager::read_document` (4 MiB limit) and Artifacts through `application::artifacts::read_artifact`, rejecting NUL bytes or invalid UTF-8 with `ReferenceOpenResult::NotApplied`. Content is capped at the largest UTF-8 boundary within 400 KiB (`400_000` bytes) for both Markdown and plain text or artifacts, appending `"\n… (file truncated)"` when truncated, and returned via `ReferenceOpenResult::ShowContent { title, filename, content, truncated, placement }` (`filename` is `document.path` or `key.name`; `title` is `document.name` if non-empty else `document.path` or `key.name`). `ModAction::OpenReference` accepts optional `mode` and `placement` overrides so a truncated `native` view offers an `"Open in editor"` action that reopens the full file via `editor` mode.

Third, `mods/streams` executes the returned presentation synchronously in `integrations/herdr-app/main.js` without spawning `hc` terminals directly or polling `terminals()`.

- For `LaunchCommand`, `mods/streams` prepends its resolved `streams` executable path to `args` and runs `herdr.run("tab.new", { space: spaceId, ...(tabNaming ? { name: title } : {}), argv })` for `tab`, or `herdr.run("pane.split", { direction: "right" | "down", pane: targetPaneId, argv })` for `pane-right` and `pane-down`.
- For `ShowContent`, `mods/streams` records pending viewer metadata keyed by `spaceId`/`tabId` and passes `props = { kind: "reference-viewer", title, filename, content, truncated, reference, spaceId, tabId, targetPaneId }` to `herdr.run("tab.new", ...)` for `tab`, `herdr.run("pane.split", ...)` for `pane-right` and `pane-down`, or `herdr.openOverlay({ kind: "window", key: "streams-viewer", width: 0.65, height: 0.75, mod: "streams", props })` for `floating`.
- If `targetPaneId` is missing when splitting (`pane-right` or `pane-down`), or if `herdr.run` throws, `mods/streams` surfaces the error in the sidebar status banner without touching other panes.

Inside `integrations/herdr-app/state.js` and `main.js`, `resolveRole(rawProps)` returns `"viewer"` when `rawProps?.kind === "reference-viewer"` or `rawProps?.role === "viewer"`. `main.js` imports `TextView`, `Editor`, and `EditorState` from `"gpui-base"`, and `settings` from `"herdr"`. The viewer class initializes `this._editorState = EditorState.new(...)` outside `render(cx)` (in `init` and when props change) for non-Markdown references, releases it with `.release()` in `close()` and before replacement, and renders `TextView.markdown("streams-md-viewer", content).scrollable(true).selectable(true)` for `.md` filenames or readonly `Editor.new(this._editorState).flex_1().min_h(0)` for other text. For split and tab viewer panes (`p.mod === "streams"`), `renderFrame` displays the file icon, title, filename, `truncated` badge, and `"Open in editor"` button in the single outer pane header while `renderViewer` omits its inner header bar. In floating overlays, `renderViewer` draws the header bar with `"Open in editor"` (when truncated) and a Close button calling `closeOverlay(props().overlay)` (with `cx.stop_propagation()`).

With `tab.new` replacing `hc run` for reference opening, the `focusNewTerminal` polling workaround in `integrations/herdr-app/main.js` and `state.js` is removed.

## Alternatives Considered

Keeping three JSON config files avoids moving TypeScript settings validation into Rust, but leaves naming and viewer keys scattered across Pi and global config files and forces `herdr-app` to depend on Pi's directory. Letting extensions read `config.toml` directly avoids child protocol requests, but creates competing TOML parsers across Rust, Node, and QuickJS and bypasses `.catalog-config.lock`. Launching viewer terminals from Rust via `hc run` and placing them with `terminal.keep` avoids adding a `streams integration view-document --mode` flag, yet forces `mods/streams` to poll `terminals()` asynchronously and risks orphaned terminals if `terminal.keep` fails. Using `hunk diff --files /dev/null <file>` opens a Hunk session with inline comments on documents, yet renders entire documents as added diff lines; `hunk open <file>` gives a clean whole-file view first, and an inline-comment mode can be added separately later.

## Target Scope and File Boundary

All file modifications fall into four groups.

### Rust config and CLI

Modify `src/config.rs`, `src/config/tests.rs`, `src/cli/args.rs`, `src/cli/mod.rs`, `src/runtime/native_cards/config.rs`, `src/runtime/native_cards/actions.rs`, `src/runtime/native_cards/documents.rs`, `src/cli/cards.rs`, `src/cli/serve/hc.rs`, and `src/cli/serve/native.rs`.

### Child protocol and Pi extension

Modify `src/child_protocol.rs`, `src/child_protocol/tests.rs`, `src/cli/serve.rs`, `src/cli/serve/dispatch.rs`, `extensions/stream-checkpoint/settings.ts`, `extensions/stream-checkpoint/streams.ts`, `extensions/stream-checkpoint/child-protocol.ts`, `extensions/stream-checkpoint/index.ts`, `extensions/stream-checkpoint/lab-cli.mjs`, and `extensions/stream-checkpoint/native.ts`.

### `herdr-app` integration and viewer

Modify `src/integration.rs`, `src/application/integration.rs`, `src/cli/integration.rs`, `integrations/herdr-app/main.js`, and `integrations/herdr-app/state.js`.

### Tests and fixtures

Modify `tests/config.rs`, `tests/cli.rs`, `tests/cli_debug.rs`, `tests/serve.rs`, `tests/serve/cards.rs`, `tests/serve/subscriptions.rs`, `tests/integration_cli.rs`, `tests/integration_contract.rs`, `tests/fixtures/integration_v1.json`, `integrations/herdr-app/tests/state.test.mjs`, `tests/stream_naming_settings_test.mjs`, `tests/stream_debug_settings_test.mjs`, and `tests/stream_child_test.mjs`.

## Phasing and Checkpoints

1. Phase 1 implements unified TOML `Config`, `streams config` CLI, and `NativeCardsConfig` cutover (`src/config.rs`, `src/cli/`, and `src/runtime/native_cards/`), removing `work-cards.json` and `WORK_CARDS_CONFIG_PATH`. Verify with `just check-fmt && just check-clippy && just test-rust-fast`.
2. Phase 2 adds `Request::GetConfig`, `Request::SetConfig`, and `Request::UnsetConfig`, removes `stream-reporting.json` I/O from `extensions/stream-checkpoint/`, hydrates in-memory settings in `index.ts` and `lab-cli.mjs`, and trims `/streams settings` to `pi.*` and `naming.*`. Verify with `just test-rust-fast && just test-pi`.
3. Phase 3 adds `HerdrConfig` on `ModResult::Read`, `LaunchCommand` and `ShowContent` on `ReferenceOpenResult`, `--mode` on `view-document`, `$EDITOR`/Hunk/native content validation, and `mods/streams` viewer role plus synchronous `tab.new`/`pane.split`/`openOverlay` execution. Verify with `just test-rust-fast && just test-herdr-app && just test-herdr && just test-native-rust && just test-replay`.

## Test Plan

- Config schema and CLI tests (`src/config/tests.rs`, `tests/cli.rs`) verify round-trip TOML defaults and overrides, rejection of unknown keys, wrong types, broken symlinks, and partial `[pi.reporting.model]` tables, atomic `set pi.reporting.model <provider>/<id>`, `unset`, `get` on tables and scalars, `--overrides`, `path`, and `--json` envelopes.
- Child protocol and Pi settings tests (`tests/serve.rs`, `tests/stream_naming_settings_test.mjs`, `tests/stream_debug_settings_test.mjs`) verify `GetConfig`, `SetConfig`, and `UnsetConfig` validation errors, `notify_hc_save`, `UiFacts.desired_pi_name` updates on `pi.naming.session` edits, startup hydration for `getArgumentCompletions` and `lab-cli.mjs`, and `/streams settings` containing only `pi.*` and `naming.*` rows.
- `herdr-app` integration and viewer tests (`tests/integration_contract.rs`, `tests/integration_cli.rs`, `integrations/herdr-app/tests/state.test.mjs`) verify `HerdrConfig` wire round-trips, `$EDITOR` flag splitting, unset fallback to `less`, missing binary `NotApplied`, `floating` + terminal `NotApplied`, `pager::observe_file` + 4 MiB checks on `editor`/`hunk`, NUL rejection and 400 KiB truncation on both `.md` and text in `ShowContent`, `resolveRole("viewer")`, `tab.new`/`pane.split`/`openOverlay` dispatch, and `EditorState` cleanup on close.

## Risks and Mitigations

If `$EDITOR` names a GUI launcher such as `code` without `--wait`, the process exits after forwarding to the desktop window and `herdr-app` closes the viewer pane immediately. Using a terminal editor (`nvim`, `vim`, `hx`) or passing a wait flag (`code --wait`) keeps the pane open until editing finishes.

Because legacy JSON files are no longer read at runtime, existing customizations in `~/.config/streams/config.json`, `~/.pi/agent/work-cards.json`, and `~/.pi/agent/stream-reporting.json` do not apply after upgrade until re-entered with `streams config set` (and the old JSON files may be deleted manually).
