# Streams work cards

Rust projects saved Streams records into Herdr. This package supplies its manifest and launch helpers. The root Pi extension owns reporting and enrollment; this package does not run model inference or write a separate work catalog. See the [Streams README](../../README.md) for installation, commands and reporting settings.

The source was imported with its original history through `e7d74ce22f95893fa8eed570ef2e6a14d61f74bd`. The [native projection decision](../../docs/native-herdr-work-cards.md) explains the adaptation. The former standalone runtime and its `/work-card` commands are not supported.

## Saved records and native ownership

The card shows the shared Stream goal and non-Dropped Document, PR, Issue and Link references alongside the viewing Session's title, status, current state and remaining work. Reference headers show saved names, roles and states, followed by purpose text. It reads these through the Streams CLI, not scratch notes or standalone Work Cards YAML files.

Each card is scoped to a catalog, host, Session and native Pi conversation. The Session supplies current Stream membership. Native effects use current observed pane, tab and workspace identity, not stored attestation. Missing or contradictory identity stops affected actions rather than guessing from labels.

The projection does not create Streams, attach conversations, launch agents or manage checkouts. It can open cards and documents, close only its Session's card, update eligible native names and open registered references. Enabled naming surfaces mirror saved Streams titles. A disabled outgoing switch stops Streams from renaming that surface; incoming Pi name edits have their own setting.

## Commands and display lifecycle

Use the root Pi extension:

| Command | Purpose |
| --- | --- |
| `/streams` or `/streams card` | Open the in-Pi editor for saved Stream and Session fields |
| `/streams report [instructions]` | Request a durable progress report |
| `/streams name [session|stream] <name>` | Save a title and protect it from generation |
| `/streams title-mode [session|stream] automatic\|manual\|inherit` | Change title policy without inference |
| `/streams settings` | Configure reporting, startup and independent naming controls |
| `/streams pane` | Explicitly open the native Herdr card |
| `/streams pane close` | Close only the owned display |

Opening or closing the pane does not request a report or change membership. `/streams pane` saves automatic mode and opens the card. `/streams pane close` and `q` save manual mode. Other closes do not change preferences. If `q` cannot save, the card exits with a warning.

Automatic opening runs at Pi startup and when an outside attachment binds a running conversation to a Session. It uses the effective Streams preference. Refreshes and child restarts do not reopen a missing card.

A lost launch reply warns and ends the pass; later requests observe again. There is no pairing journal or exactly-once launch guarantee. Duplicate cards exit through an OS process lock.

The terminal pane subscribes to native and saved-record changes and reflows on resize. Its document shortcuts use the currently displayed target and revalidate it before opening. If the reference or checkout changes, redraw and press the shortcut again.

## Documents and checkouts

Streams owns repository identities, document references and host-local checkout registrations. Register checkouts through the CLI; the card does not discover roots from Git remotes or choose an arbitrary directory.

A document uses its saved exact repository URL and relative path to open the viewing Session's resolved working file, including local edits. Recovery hints remain available for explicit recovery. The adapter rechecks the complete saved Reference, selected binding and canonical file before opening. Missing mappings, missing files and paths escaping the checkout fail visibly; they leave the shared reference intact. URL activation also compares the full saved Reference before applying the shared HTTP(S) safety policy.

Documents open using `$EDITOR`, falling back to `less`. Do not copy untrusted document, record or model text into executable configuration.

## Presentation configuration

Presentation settings live in the unified Streams TOML configuration at `$XDG_CONFIG_HOME/streams/config.toml` (default `~/.config/streams/config.toml`), owned by the Rust CLI (`streams config`). Missing keys use defaults; unknown keys and invalid values fail with the file path.

| Setting | Default | Purpose |
| --- | --- | --- |
| `herdr.cards.auto_open` | `"automatic"` | Default card-opening mode (`"automatic"` or `"manual"`) |
| `herdr.naming.workspace` | `true` | Track eligible Herdr workspace names |
| `herdr.naming.tab` | `true` | Track eligible Herdr tab names |
| `herdr.timeout_ms` | `3000` | Native operation and document lock-wait timeout (1 to 2,147,483,647) |
| `herdr.pane_height_fraction` | `0.2` | Requested share of a new pair's initial split, strictly between 0 and 1 |
| `pi.naming.session` | `true` | Track eligible Pi session names |
| `pi.naming.import_edits` | `true` | Import Pi session name changes into Streams |

`/streams settings` edits the global `herdr.cards.auto_open` default, conversation overrides, and naming settings through the Rust child. Each naming setting accepts only a boolean. Disabling outgoing naming stops Streams from renaming that surface; re-enabling it projects the saved Streams title. Disable both `pi.naming.session` and `pi.naming.import_edits` to keep Pi's name independent. See the [configuration and reference viewer decision](../../docs/config-and-reference-viewer.md).

## Rust lifecycle routing

Rust is the only native implementation on this branch. The Pi extension defaults to this checkout's `target/debug/streams`, respecting `CARGO_TARGET_DIR`. Build it with `just build`. Set `STREAMS_BIN_PATH` to an absolute matching executable when using a packaged extension or a different build. Missing or invalid executables fail; there is no Python, TypeScript-coordinator or `PATH` fallback. This source cutover does not activate or replace installed tools.

The Pi extension keeps commands, tools, event serialization, branch admission and the Pi session-name setter. Rust handles card open/close, metadata and Herdr names. Each pass reads concerned records and observes at most one native snapshot; the executor checks subscription liveness before effects. There is no saved native launch evidence or arrangement capture. Reporting and native effects share `STREAMS_BIN_PATH`. Load only one checkout's extension.

The Rust card uses `streams-rust-herdr-plugin.toml` and `bin/open-rust-streams-pane.sh`. The launcher requires the protected launch envelope and executes that explicit Rust binary, not a CLI found on `PATH`.

Run `just test-native-rust` for isolated Pi/Rust CLI acceptance with fake Herdr endpoints. PTY and live-platform acceptance remain required before a trial.

## Development

Use Node.js 22 or later. From the repository root:

```sh
npm --prefix integrations/herdr-work-cards test
```

`npm test` runs `tests/*.test.mjs` for the POSIX document and artifact launchers. `just build` builds the Rust native runtime. Restart participating Pi sessions and reopen native pane processes after changing loaded code. Do not mix loaded CLI and extension revisions during an update. Use the root Streams extension, not a standalone Pi entrypoint from this package.

The `streams-rust-herdr-plugin.toml` manifest requires a compatible Herdr native plugin API. Explicit pane opening can register the exact missing plugin; it refuses conflicting or disabled registrations. It does not update or restart Herdr.

This package tests the document and artifact launchers. Rust tests cover rendering, exact context, ownership, native lifecycle, configuration and guarded documents, retaining frozen renderer fixtures. Native effects use fake protocols. Run isolated native acceptance before activating changed launch code; automated tests do not establish live platform support.

## Design contracts

The [native projection decision](../../docs/native-herdr-work-cards.md) preserves UI and document rationale. [Name ownership](../../docs/name-ownership.md) owns saved-title projection, and [observe and act](../../docs/native-observe-and-act.md) owns lifetime, failure and targeting policy. The imported standalone implementation remains in version history, not a second current runbook.
