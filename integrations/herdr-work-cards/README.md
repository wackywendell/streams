# Streams work cards

Rust projects saved Streams records into Herdr. This package supplies its manifest, launch helpers and the TypeScript Pi boundary utilities. The root Pi extension owns reporting and enrollment; this package does not run model inference or write a separate work catalog. See the [Streams README](../../README.md) for installation, commands and reporting settings.

The source was imported with its original history through `e7d74ce22f95893fa8eed570ef2e6a14d61f74bd`. The [native projection decision](../../docs/native-herdr-work-cards.md) explains the adaptation. The former standalone runtime and its `/work-card` commands are not supported.

## Saved records and native ownership

The card shows the shared Stream goal, documents, issues and pull requests alongside the viewing Session's title, status, current state and remaining work. It reads these through the Streams CLI, not scratch notes or standalone Work Cards YAML files.

Each card captures an exact catalog, host, Stream, Session and native Pi session identity. Native effects also require the attested Herdr pane, tab and workspace. A changed or unavailable identity clears the view and shortcuts rather than displaying stale records or selecting another workspace by name.

The projection does not create Streams, attach conversations, launch agents or manage checkouts. It can create and close its owned card and document displays, update eligible native names and open registered references. Enabled naming surfaces mirror saved Streams titles; disable a surface's naming switch to manage its label independently.

## Commands and display lifecycle

Use the root Pi extension:

| Command | Purpose |
| --- | --- |
| `/streams` or `/streams card` | Open the in-Pi editor for saved Stream and Session fields |
| `/streams report [instructions]` | Request a durable progress report |
| `/streams settings` | Configure reporting and Herdr startup and naming |
| `/streams pane` | Explicitly open the native Herdr card |
| `/streams pane close` | Close only the owned display |

Opening or closing the pane does not request a report or change membership. Pressing `q` closes the display. Automatic opening runs when Pi starts or resumes and `workspace_startup` is `automatic`. Within that Pi lifetime, a closed card stays closed until `/streams pane` reopens it. An exact recorded launch can finish interrupted pairing under the workspace lock. A launch without verifiable native identity stays blocked rather than risking a duplicate pane.

The terminal pane subscribes to native and saved-record changes and reflows on resize. Its document shortcuts use the currently displayed target and revalidate it before opening. If the reference or checkout changes, redraw and press the shortcut again.

## Documents and checkouts

Streams owns repository identities, document references and host-local checkout registrations. Register checkouts through the CLI; the card does not discover roots from Git remotes or choose an arbitrary directory.

A document normally opens from the viewing Session's selected checkout, including local edits. The app supplies available locations and any alternate choices. A recorded commit is provenance, not a requirement that the working file match that revision. Revision choices contain explicit metadata, not synthetic checkout IDs. The CLI extracts their contents only when you open them, never while refreshing the card. The adapter checks the selected binding and canonical file before opening. Missing mappings, missing files and paths escaping the checkout fail visibly; they do not remove the shared reference.

`document_command` is a trusted command prefix. It falls back to `$EDITOR`, then `less`. Do not copy untrusted document, record or model text into executable configuration.

## Presentation configuration

Global configuration lives at `$PI_CODING_AGENT_DIR/work-cards.json`, defaulting to `~/.pi/agent/work-cards.json`. There are no project overrides. Missing configuration uses defaults; partial configuration overlays them. Invalid and unknown fields fail with the file path.

| Setting | Default | Purpose |
| --- | --- | --- |
| `workspace_startup` | `automatic` | Open the native card automatically or require an explicit pane command |
| `workspace_naming` | `true` | Track eligible Herdr workspace names |
| `tab_naming` | `true` | Track eligible Herdr tab names |
| `session_naming` | `true` | Track eligible Pi session names |
| `document_command` | `null` | Trusted command prefix; otherwise `$EDITOR`, then `less` |
| `herdr_timeout_ms` | `3000` | Native operation and document lock-wait timeout |
| `pane_height_fraction` | `0.2` | Requested share of a new pair's initial split, within native layout constraints |

Startup accepts `automatic` or `manual`. Each naming setting accepts only a boolean and defaults to `true`, independently of startup. `null` is invalid for naming. Disabling naming leaves that surface independently managed. Re-enabling it projects the saved Streams title, replacing a different frontend name.

`herdr_timeout_ms` must be an integer from 1 to 2,147,483,647. Pane height must be strictly between zero and one. `document_command` accepts `null` or a nonblank command without control characters; malformed nonblank `$EDITOR` values also fail visibly.

Inference settings belong to the root extension's `stream-reporting.json`. Removed fields such as `model`, `thinking_level`, `show_refresh_spinner`, `check_every_n_responses`, `max_output_tokens`, `request_timeout_ms` and `pane_poll_interval_ms` are rejected in `work-cards.json`.

## Rust lifecycle routing

Rust is the only native implementation on this branch. The Pi extension defaults to this checkout's `target/debug/streams`, respecting `CARGO_TARGET_DIR`. Build it with `mise run build`. Set `STREAMS_BIN_PATH` to an absolute matching executable when using a packaged extension or a different build. Missing or invalid executables fail; there is no Python, TypeScript-coordinator or `PATH` fallback. This source cutover does not activate or replace installed tools.

The Pi extension keeps commands, tools, event serialization and the Pi session-name setter. Rust handles native card open/close, metadata, Herdr names and arrangement capture. Every effect re-attests the captured catalog, host, conversation, endpoint, terminal and workspace binding. A changed scope permanently suspends the old adapter until reload. Cancellation drains the child process group; it does not clear uncertain launch evidence or close displays.

Reporting and native effects share one executable selector. `STREAMS_RUST_BIN` remains a compatibility alias; when both variables are set, their canonical paths must match. Load only one checkout's Streams extension.

The Rust card uses `streams-rust-herdr-plugin.toml` and `bin/open-rust-streams-pane.sh`. The launcher requires the protected launch envelope and executes that explicit Rust binary, not a CLI found on `PATH`.

Run `mise run test:native:rust` for isolated Pi/Rust CLI acceptance with fake Herdr endpoints. PTY and live-platform acceptance remain required before a trial.

## Development

Use Node.js 22 or later. From the repository root:

```sh
npm --prefix integrations/herdr-work-cards ci --ignore-scripts
npm --prefix integrations/herdr-work-cards run check
npm --prefix integrations/herdr-work-cards test
```

`npm test` builds the surviving TypeScript support code and runs `tests/*.test.mjs`. `mise run build` builds the Rust native runtime. Restart participating Pi sessions and reopen native pane processes after changing loaded code. Do not mix old and new launch-record protocols during an update. Use the root Streams extension, not a standalone Pi entrypoint from this package.

The `streams-rust-herdr-plugin.toml` manifest requires a compatible Herdr native plugin API. Explicit pane opening can register the exact missing plugin; it refuses conflicting or disabled registrations. It does not update or restart Herdr.

This package tests configuration, executable selection, Pi naming, subprocesses and the document launcher. Rust tests cover rendering, exact context, ownership, native lifecycle and guarded documents, retaining frozen renderer fixtures. Native effects use fake protocols. Run isolated native acceptance before activating changed launch code; automated tests do not establish live platform support.

## Historical design documents

The imported [orientation design](docs/design.md), [workspace context design](docs/workspace-context.md) and [automatic naming design](docs/automatic-naming.md) record the original product's rationale. Their standalone record, inference and command contracts are historical, not current runtime instructions. The [Streams native projection decision](../../docs/native-herdr-work-cards.md) takes precedence.
