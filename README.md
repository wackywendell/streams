# streams

Streams keeps a personal catalog of ongoing work. Each stream has a stable identity, an optional goal, a work status and references to documents, PRs, issues and other useful material. Sessions describe their own scope and achievements, current status and remaining work. Plans and design decisions stay in project repositories, not in scratch notes.

This checkout includes the catalog CLI, Pi work-card extension and bundled agent skill. Published `v0.1.0` packages still use the scratch workflow; do not mix those packages with this CLI.

## Run from this checkout

This branch uses Rust for the CLI, catalog, native lifecycle and terminal card. Build it without replacing your installed CLI:

```sh
just build
./target/debug/streams --help
```

The examples below use `streams`; from this checkout, substitute `./target/debug/streams`. If `CARGO_TARGET_DIR` is set, use its `debug/streams` instead. Use isolated XDG directories when testing; commands otherwise use your normal configuration and catalog.

The Pi extension selects this checkout's debug Rust build by default. `STREAMS_BIN_PATH` can select another absolute Rust executable. There is no Python or `PATH` fallback. Loading this checkout does not update installed tools or reload running processes. The extension refuses a binary that was not built from this checkout's current Rust sources: every build embeds a digest of `Cargo.toml`, `Cargo.lock`, `build.rs` and `src/`, and the extension compares it with the same digest of its checkout when the binary starts. A stale or foreign binary stops Streams for that Pi session with one error naming the binary, the checkout and both digests. Run `just build` and reload Pi.

## Initialise a catalog

Both storage modes require explicit setup. Choose one:

```sh
streams init --sync none
streams init --sync git
streams init --sync git --remote git@github.com:you/work-catalog.git --branch main
```

Git without `--remote` records local commits without fetching or pushing. If you select a remote, it must already exist. Its branch must be empty or contain a supported Streams catalog, not arbitrary Git files. Setup records its strategy and a persistent local host identity. Ordinary commands never initialise a catalog or fall back to another storage mode.

Inspect or change the everyday settings:

```sh
streams config
streams config get catalog.root
streams config set catalog.root ~/work-catalog
streams config set frontend.mode auto
```

The supported core settings include `catalog.root`, `frontend.mode`, `naming.stream`, `naming.session`, and `lifecycle.decay_days` (default `14`, `0` disables), alongside `herdr.*` and `pi.*` namespaces. Defaults are `$XDG_DATA_HOME/streams/catalog`, `auto`, automatic title generation for both record types, and 14-day inactivity decay. Sync runs explicitly or at Pi startup/shutdown, not on a timer.

Configuration is host-local at `$XDG_CONFIG_HOME/streams/config.toml`; each CLI process reads it anew. Unset XDG variables use `~/.local/share` and `~/.config`. Sync strategy, remote and branch belong to catalog setup, not these settings. Changing `catalog.root` selects another catalog; initialise it before use.

Catalog records use host-partitioned addresses:
`hosts/<host>/streams/<id>.yaml` and `hosts/<host>/conversations/<id>.yaml`.
A Session owns its membership; moving it changes that Session alone. Checkout
URLs and host-local Session directory selections remain with the conversation.

Initialization requires an empty catalog. Select `none` or `git` transport when
creating it; initialization does not adopt an existing repository or convert old
records. The alpha switch uses the [manual shutdown and backup
checklist](docs/record-local-operations.md), followed by fresh initialization and
re-enrollment. The shared reference format requires a fresh catalog and, when sharing, a fresh compatible destination. Follow the [offline activation steps](docs/stream-reference-implementation.md#offline-activation) before using the new binary with live data.

Saving a record is local. Publication is a separate transport operation, so a
network failure cannot turn an accepted local save into a failed save.

## Create and find work

```sh
streams create "Catalog CLI"
streams create "Login reliability" --goal "Make login tests reliable"
streams list
streams list "login" --status active
streams show "Login reliability"
streams show "Login reliability" --json
streams check
```

`create` saves a stream without launching a workspace. A missing goal is `null`, not placeholder text. Creation reports the stable ID.

Search is a case-insensitive literal substring of the title or goal. It has no operators or query language. Repeat `--status` to select several statuses. `list` projects live Session activity for `active`/`parked` Streams and excludes `closed` streams unless you use `--closed` or `--all`. Run `streams tidy [--days N]` to persist `closed` on inactive Sessions and Streams untouched past `lifecycle.decay_days`.

`list` shows every Stream it can read. It names each record file it cannot decode, with its kind, host, catalog path, cause and repair, on stderr, or under `unreadable` with `--json`, and still exits 0, because the listing itself succeeded. `list` fails only when the catalog itself cannot be read. The repair depends on the kind of record:

- Stream: run `/streams reset` in a conversation on that Stream, or delete or fix the file.
- Session: run `/streams reset` in that conversation, or delete or fix the file.
- Host: delete or fix the file.

`/streams reset` repairs only this host's records; repair another host's records on that host. An unreadable Session's Stream is unknown, so it counts toward no Stream: a row's counts, status and recency reflect its readable Sessions only.

Stream selectors accept an exact ID, an exact title or a unique title substring. IDs take precedence; UUID prefixes are not supported. Ambiguous references fail and report candidate IDs; they never select the first result.

`show` prints the complete logical stream and its sessions as YAML. `--json` gives the same structure. `check` validates saved records and relationships. It does not require saved conversations to be running or probe remote branches.

## Edit work

```sh
streams update STREAM_ID --title "Catalog discovery"
streams update STREAM_ID --goal-file ./goal.txt
streams update STREAM_ID --clear-goal
streams update STREAM_ID --status parked
streams update STREAM_ID --add-repository streams https://github.com/wackywendell/streams
streams close STREAM_ID
streams restore STREAM_ID
streams tidy
```

Statuses are `active`, `waiting`, `blocked`, `parked`, `done`, and `closed`. Closing a Stream closes its non-done child Sessions; restoring reopens the Stream to `active`. Marking work `done` does not close its runtime.

Setting a title or goal makes that field manual. Use `--title-mode automatic` or `--goal-mode automatic` to allow future automatic refinement. Compact summaries clear when their source text changes.

Repository aliases configure the effort's repositories and checkout commands. Reference management uses the separate command family below.

Omitted fields stay unchanged. `--clear-goal` removes a goal. Text-file flags read UTF-8; use `-` for stdin. Only one field can consume stdin per command.

## Manage references

Each reference has a name, purpose, role and state. Roles are `deliverable`, `dependency`, `brief` and `background`; states are `active`, `fulfilled` and `dropped`. Targets are documents, PRs, issues or links.

```sh
streams reference add STREAM_ID --kind document --repository-url https://example.org/tools --path docs/design.md --name Design --purpose 'Guide implementation' --role brief --ref lifecycle-design
streams reference add STREAM_ID --kind pr --url https://example.org/tools/pull/43 --name Implementation --purpose 'Deliver the change' --role deliverable
streams reference list STREAM_ID
streams reference list STREAM_ID --json
streams reference edit STREAM_ID --url https://example.org/tools/pull/43 --purpose 'Merge and deploy the change'
streams reference drop STREAM_ID --url https://example.org/tools/pull/43
streams reference restore STREAM_ID --url https://example.org/tools/pull/43 --state active
streams reference delete STREAM_ID --url https://example.org/tools/pull/43 --confirm
```

Document identity is the exact repository URL and portable relative path. PR, Issue and Link share exact URL identity. Recovery ref, commit and source Session hints remain optional. References stay saved when local files or checkout mappings are unavailable.

Drop keeps the identity and prevents automatic restoration. Delete forgets it and allows reporting to rediscover it, including through a report already underway. Fulfilled references remain visible; Dropped entries are available in the manager and CLI list. Explicit restoration chooses Active or Fulfilled.

Plain list output supplies exact selector flags. Unsafe document selectors appear as `selector-json` data requiring JSON decoding; machine output preserves every value. See `streams reference --help` and the [implementation contract](docs/stream-reference-implementation.md#cli-entry-points) for correction flags and recovery-field clearing.

## Record session progress

```sh
streams session attach STREAM_ID --harness pi --session-id NATIVE_ID --title "CLI tests"
streams session list --stream STREAM_ID
streams session show SESSION_RECORD_ID
streams session update SESSION_RECORD_ID --recap "Added search filters" --state "Checking refresh conflicts" --remaining "Review conflict handling"
streams session update SESSION_RECORD_ID --recap-file ./recap.txt --state-file ./state.txt --remaining-file ./remaining.txt
streams session update SESSION_RECORD_ID --recap-short "Search" --state-short "Review" --remaining-short "Land"
streams session update SESSION_RECORD_ID --status done
streams session lifecycle park --harness pi --session-id NATIVE_ID
```

Interactive Pi TUI sessions park active Sessions on shutdown and reactivate `parked` or `closed` Sessions on resume (`session_started`); non-interactive (`print`, `json`) and ephemeral (`--no-session`) Pi runs skip Streams startup and reporting.

Attaching records an existing conversation; it does not start it. A session has one concise title, a lifecycle status, a cumulative `recap`, current `state` (labelled Status) and `remaining` work. Each text has a matching `_short` form. Reports prefer 1–2 words and fewer than 20 characters, with a 24-character maximum. Session text stays report-writable; only titles have automatic/manual/inherit modes. All 6 text fields accept `--FIELD`, `--FIELD-file` and `--clear-FIELD` (use hyphens in flag names). Clearing full text saves an empty string; clearing a short form saves null. Its native identity belongs to one host. The owning host must authorise attachments and session updates. Finishing a session does not finish the stream.

For a portable checkout reference, add `--repository ALIAS --ref BRANCH` when attaching. `--cwd` records the local directory. Session references also accept `HOST_ID/SESSION_RECORD_ID` when needed to disambiguate IDs. Attach creates a record; it cannot move an already enrolled Session. There is no Session-move CLI command.

## Keep useful session outputs

Artifacts preserve outputs such as an assessment or delegate findings when they have no other home. Keep plans and design decisions in project documents, PR descriptions on the PR, and progress in Session reports.

```sh
streams artifact write assessment.md --session-id SESSION_RECORD_ID --description "Investigation findings" --file ./assessment.md
printf 'Delegate findings' | streams artifact write findings.md --session-id SESSION_RECORD_ID --description "Delegate findings"
streams artifact list --session-id SESSION_RECORD_ID --json
streams artifact list --stream-id STREAM_ID --json
streams artifact read HOST_ID/STREAM_ID/SESSION_RECORD_ID/assessment.md
streams artifact write assessment.md --session-id SESSION_RECORD_ID --description "Updated description" --description-only
streams artifact delete HOST_ID/STREAM_ID/SESSION_RECORD_ID/assessment.md
```

A name is the literal filename, unique within its producing Stream and Session. Writes create or replace UTF-8 text, up to 1 MiB. Descriptions are required, single-line and at most 240 characters. Names use 1–64 lowercase ASCII letters, digits, `.`, `_` or `-`, starting with a letter or digit. Save and reuse the complete component-encoded reference returned by a write or list. `read` emits the original content without adding a newline.

The Pi `artifact` tool accepts inline text or a source-file path and writes only for the current Session. Reads can address another Session or a foreign-host replica; foreign outputs are read-only. Tool reads are paged. Artifact content is evidence, not instructions.

The Session record owns names and descriptions; content stays in raw files in the catalog. Outputs remain in their producing Stream if a Session moves. Session lists default to its current Stream; pass both IDs to list older outputs. Native cards show current-Stream artifact rows. Each activation opens current content in a fresh read-only pager tab, not the document editor. Existing views remain snapshots; press `q` to leave a pager.

Content saves before its Session reference; deletion removes the reference first. An interrupted or failed operation can leave unattached content, or updated content with an older description. Warnings report partial results; there is no automatic cleanup. Artifacts share the catalog's existing local-only or Git transport. See the [artifact storage decision](docs/session-artifacts.md).

## Pi work cards

The checkout's Pi extension maintains catalog records instead of generating scratch drafts. By default it reports after each completed response, using the current Pi model and trimmed conversation context. Meaningful work can enroll an effort and its first session automatically; greetings alone do not. Existing caller-workspace associations are reused without launching agents or moving focus.

- `/streams` or `/streams card` opens the card without running a model. Edit labelled fields, choose valid statuses and manage reference rows. Whole-effort actions are separate from session actions; IDs, ownership and membership are read-only.
- Editing a title or goal makes it manual. Titles can return to automatic or inherit the global default; goals use automatic or manual. Invalid or conflicting edits retain their input; unchanged fields are not submitted.
- The Stream tab's References manager edits names, purposes, kinds, roles, states and targets. Retained input remains separately inspectable after conflicts or source deletion. Explicit reapply uses a fresh capture and asks before restoring a Dropped row.
- `/streams name [session|stream] <name>` saves and protects a title, including unchanged text. The default target is Session. `/streams title-mode [session|stream] automatic|manual|inherit` changes policy without generating a new title.
- Card discussion can answer without edits, refresh descriptions, or carry out an explicitly requested edit. User direction is approval; uncertain intent produces a question rather than a management action.
- `/streams report` requests a report now, not a draft awaiting Apply. Use `/streams report [instructions]` to give reporting direction. Explicit reporting can enroll an unbound conversation. `/streams reset [--auto]` regenerates automatic fields (and with `--auto`, restores automatic title/goal modes before inference). Reset first discards any card record that cannot be decoded and names it in a warning, so it can always rebuild the card. The model-callable `checkpoint` tool remains unchanged.
- `/streams attach` offers initial selection or creation; attached conversations cannot switch efforts.
- `/streams settings` controls reporting, enrollment, response frequency, context, model and Herdr startup. Its Naming submenu separates title-generation defaults, incoming Pi name edits and outgoing frontend synchronization. Manual reporting disables background requests, not explicit commands.
- `/streams pane` opens the native Herdr card; `/streams pane close` closes only its display. An enrolled conversation opens without reporting. An unenrolled conversation with automatic enrollment reports once before opening; manual enrollment requires `/streams attach`.
- Tab completes `card`, `report`, `name`, `title-mode`, `reset`, `settings`, `attach` and `pane`. Unknown subcommands or invalid management arguments show usage without reporting or saving. Free-text instructions must follow `report`.

Settings live in `$XDG_CONFIG_HOME/streams/config.toml` under `pi.*` and `naming.*` (managed through `streams config` or `/streams settings`). Legacy JSON config files are ignored. The defaults for Pi reporting are `pi.reporting.mode = "automatic"`, `pi.reporting.enrollment = "automatic"`, `pi.reporting.frequency = 1`, `pi.reporting.context = "trimmed"`, and `pi.naming.import_edits = true`.

Set `pi.reporting.model` (`provider/id`) to select a helper model; unset it to use the current Pi model. Unavailable models fail visibly without fallback. Trimmed context excludes tool calls/results and thinking; full context preserves the active projected conversation, excluding incomplete tool batches. Oversized input fails rather than silently dropping history.

The Naming submenu saves Stream and Session title-generation defaults through the Rust configuration API (`naming.stream` and `naming.session`). Defaults affect records in `inherit` mode, not explicit automatic or manual modes. Enrollment may still create an initial title under a manual default. Changing a default does not run inference or rename a frontend.

Pi's built-in `/name` imports a different name as a manual Session title when Pi name edits are enabled. Matching names are ignored; use `/streams name session <name>` to pin one. Unbound conversations stay native-only. Incoming edits and outgoing Pi synchronization are independent: disable both to keep the names separate. A rare late projection echo can be imported as manual; see the [name-ownership decision](docs/name-ownership.md#native-pi-name).

Session inference can propose reference additions, updates and drops alongside progress. Progress saves first, then references use the original capture; a reference refusal preserves saved progress and stops later inference without retrying. Reference state describes this effort's relationship to the resource and does not change external tickets or Stream lifecycle.

Session progress and effort metadata use separate inferences, so updating both takes two helper requests. Reports preserve captured records, ownership and conversation-lineage guards. They may update session status, but cannot park, finish or archive the whole effort. Historical conversation is evidence, not fresh verification of files or remote services.

The menu refreshes on action or reopen, not while waiting for a selection. Discussion and unsent inputs survive closing and reopening the card in the same Pi process, not extension reload. The native Herdr card reads the same saved records; it does not run another summarizer.

When testing, keep the CLI and extension from the same checkout, use isolated XDG/agent directories and do not also load the published scratch-based extension. Restart running Pi sessions after updating the extension; existing processes retain their previously loaded code. The [bundled skill](skills/streams/SKILL.md) covers record semantics and guarded reporting from other harnesses.

### Helper transport diagnostics

Enable “Helper session logging” under Debug in `/streams settings` to save private JSONL logs in `$PI_CODING_AGENT_DIR/stream-reporting-sessions`. It defaults to disabled and is independent of “Debug commands”. Logs retain the application input, normalized SDK response, parsed result or error, and application retry attempts.

When the provider exposes SDK hooks, `stream-helper-provider-request` records its converted request body and model identity; `stream-helper-provider-response` records HTTP status and allowlisted request-ID, trace, retry and content-type headers. `stream-helper-provider-event` records Anthropic Messages and OpenAI Responses tool arguments and completion boundaries before SDK normalization. Compare these entries with the normalized response to distinguish an empty report from a missing tool-block close or an interrupted stream. Unsupported adapters may omit hooks; absent entries do not prove that no request or event occurred. These are parsed provider events, not a raw HTTP capture or evidence of what happened inside a gateway.

Transport entries omit credential-shaped fields, opaque reasoning and streamed text/thinking content. The existing application inputs and normalized responses remain unchanged. Logs still contain private conversation and generated card text: inspect them before sharing. Logging does not change prompts, model selection, retries, empty-delta validity or save rules. Disabling it stops new logs without deleting existing files.

### Frozen report experiments

Enable “Debug commands” in `/streams settings` to capture full conversation samples, prepare a helper request and rerun its saved input without changing live cards:

```sh
streams debug --help
streams debug capture stream STREAM_ID --json
streams debug prepare CAPTURE_ID --sample 0 --task session --json
streams debug run REQUEST_ID --model PROVIDER/MODEL --json
```

Capture, preparation and inspection stay local. Run and replay send evidence to the selected provider using normal credentials and request costs. They require Pi 1.3 or newer; `STREAMS_PI_SDK_ENTRY` can select an absolute matching SDK entry. Prepared requests preserve the rendered input rather than rebuilding it at execution. Private files live under `$PI_CODING_AGENT_DIR/stream-debug`; the [frozen-request ADR](docs/pi-prompt-replay.md) defines the boundaries.

## Native Herdr work cards

Work Cards history is retained through `e7d74ce22f95893fa8eed570ef2e6a14d61f74bd`. Its surviving support code and package documentation live under [`integrations/herdr-work-cards`](integrations/herdr-work-cards); the duplicate native implementation is retired. The [native projection decision](docs/native-herdr-work-cards.md) identifies which behavior carries forward and which legacy authority is superseded.

The root Pi extension delegates native effects and the terminal card to Rust. It observes current Herdr/Pi identity and plans effects from that pass; there is no saved caller proof or native launch journal. TypeScript retains Pi events, branch admission, inline UI and the Pi session-name setter. The native card runs no separate summarizer and writes no second catalog.

Streams owns card-opening preferences. `/streams settings` edits the global `cards.auto_open` default (`automatic` or `manual`) and this conversation's override (`inherit`, `automatic` or `manual`) through the Rust child. Automatic cards open at Pi startup or when an outside attachment binds a running conversation to a Session. Ordinary refreshes and child restarts do not reopen missing cards.

Native presentation settings live in the Streams TOML config with the rest of the configuration. `herdr.naming.workspace`, `herdr.naming.tab` and `pi.naming.session` are independent booleans, each defaulting to `true`. Enabled surfaces mirror saved Streams titles. Disabling outgoing synchronization stops Streams from renaming that surface; Pi name edits are controlled separately by `pi.naming.import_edits`. Saved references open with `herdr.viewer.mode`, which defaults to `$EDITOR` or `less`. See the [configuration and reference viewer decision](docs/config-and-reference-viewer.md).

Closing with `q` or `/streams pane close` saves a manual conversation preference without changing membership or requesting a report. `/streams pane` saves automatic mode and opens the card. Other native closes do not change preferences. If `q` cannot save, the card still exits and warns. See the [card preference decision](docs/card-preferences-and-control.md).

An uncertain launch warns and ends the pass. A later explicit request observes again, with best-effort reuse rather than an exactly-once guarantee.

### Select local checkouts

Register exact Git or jj roots and their repository URLs explicitly:

```sh
streams checkout register ~/code/project --repository https://github.com/you/project.git
streams checkout list
streams session directory SESSION_RECORD_ID --checkout CHECKOUT_ID --repository code
streams session directory SESSION_RECORD_ID --checkout CHECKOUT_ID --within src --repository code
streams checkout default CHECKOUT_ID https://github.com/you/project.git
streams checkout resolve https://github.com/you/project.git
streams checkout move CHECKOUT_ID ~/code/project-moved
```

`--repository` on `checkout register` is an exact URL; on `session directory` it is the Stream's repository alias. A Session's selected checkout wins for its repository. Set a default explicitly for documents in other repositories. Reads never infer bindings from Git remotes, create checkouts or change defaults. A moved checkout keeps its ID; `session directory --cwd PATH` can replace an obsolete directory selection. Register its URL first, or pass `--repository ALIAS` using an existing Stream repository. Without either, a discovered root has no inferred URL binding.

Directory selection is host-local and does not populate the portable `session.checkout`. Automatic enrollment can therefore have a null `session.checkout` while `checkouts[]` contains valid local mappings. Explicit attach with `--repository` and `--ref` can populate portable metadata; directory selection cannot invent a ref.

The current native card opens the viewing Session's resolved working file, including local edits. Optional `commit` and `source_session_id` record provenance, not a navigation pin. The card does not currently offer a source/revision chooser. Missing files or mappings remain visible errors and never remove the shared reference. Host-local checkout registrations live under `$XDG_STATE_HOME/streams/checkouts`.

## Open or resume work

```sh
streams config set frontend.mode herdr  # auto, hc, herdr, cmux or none
streams frontend get --json
streams start "New effort" --frontend hc
streams open STREAM_ID --frontend hc
streams open STREAM_ID --session SESSION_RECORD_ID --cwd ~/code/project
streams resume STREAM_ID --session SESSION_RECORD_ID
streams open --current
streams resume --current
```

`start` creates a stream and opens it. If opening fails, the stream remains saved and the result reports its ID. Retry `open` with that ID instead of creating another stream.

In `hc` and `herdr-app`, `open` inspects registered Pi sources (`streams.pi.*`) on the selected core, adopts unplaced running terminals into the Stream's space, starts missing Pi 1.5.2 conversations (or a work shell when a Stream has no Sessions yet), and associates the space in the `streams` document when published. In Herdr, `open` discovers active same-host Sessions and opens missing conversations in separate tabs in an observed or new Stream workspace. Already-observed running Sessions stay where they are and their locations are reported. `--session` explicitly selects one conversation, including inactive work. No tab order, pane geometry or saved arrangement is restored. Existing tabs and agents are never closed automatically.

`resume` replaces the current terminal process with one recorded Pi, Codex or Claude conversation. Multiple eligible Sessions require `--session`; it does not reconstruct or focus a workspace. Both paths refuse a duplicate live conversation and validate saved files. Missing or ambiguous files are reported, not replaced with fresh conversations. CMUX supports single-session opening, not multi-Session reopening.

`--current` uses the invoking Session or workspace's explicit association (`HC_TERMINAL_ID` in `hc`, or Herdr workspace metadata). It never infers a Stream from a title, working directory or another client's GUI focus. `--frontend` chooses an adapter independently of the target. Native reuse uses one current observation. A failed or lost reply warns and stops; no saved attempt is replayed. Rare duplicate workspaces, editors or conversations can require manual cleanup.

An explicit `--cwd` updates the selected session's local directory mapping, including on resume and workspace reuse. Reusing a workspace does not change an existing terminal's working directory.

### Native `herdr-app` mod

The `herdr-app` integration lives under [`integrations/herdr-app`](integrations/herdr-app) and renders the Streams sidebar panel, the single 30 px pane header strip for enrolled Pi terminals, and the Session detail popup. It calls the owner CLI (`streams integration describe --frontend hc` and `streams integration request --request-json <json>`), batches active sidebar and visible frame selections into one request per context or save-notice change, and opens saved Document and Artifact references in ephemeral `hc` viewer terminals (`no_parent: true`, `rm: true`). See the [hc and herdr-app implementation plan](docs/hc-herdr-app-plan.md) and [baseline design](docs/hc-herdr-app-design.md) for setup and rollback steps.

Install the release `streams` CLI into `~/.local/bin/streams` and the `herdr-app` mod into `${HERDR_APP_HOME:-~/.config/herdr-app}/mods/streams/` without touching your existing layouts:

```sh
just install
```

Or install only the mod (`just install-herdr-app-mod`), only the CLI (`just install-cli`), or both the mod and the standalone reference layout ([`integrations/herdr-app/layouts/streams.json`](integrations/herdr-app/layouts/streams.json)):

```sh
just install-herdr-app
```

To compose `mods/streams` into your own `herdr-app` layout JSON:

- Set `pane.header.content` to `{ "mod": "streams" }`. Role `"frame"` is inferred from the `pane` / `terminal` props injected by `herdr-app`, keeping `pane` header parity while showing a clickable `≋ <Stream> · <Session>` pill on enrolled terminals.
- Prepend `{ "id": "streams", "type": "block", "content": { "mod": "streams", "props": { "label": "Streams", "icon": "≋" } } }` to `panel-body.children`. Role `"sidebar"` is inferred when `pane` / `terminal` are absent, and `label` / `icon` let layout-driven tab bars render the toggle button without custom tab-bar forks.

Local directories and native bindings live under `$XDG_STATE_HOME/streams/catalogs/<catalog-path-hash>`, not in the shared catalog. The default state root is `~/.local/state`. Opening requires the Stream's immutable host to match this host. Ownership handover and cross-host Session reattachment are unsupported.

## Understand saving and sharing

A save succeeds when its target record and embedded receipt are persisted locally, for either storage mode. Git commits and publication happen separately:

```sh
streams sync --json
streams list --refresh
streams show STREAM_ID --refresh --json
```

With `none`, sync validates records without Git or network access. Git without a remote checkpoints local producer changes and reports local-only sharing. With a remote, sync combines disjoint host-partitioned files and publishes. Concurrent changes to the same file stop sync; it never merges record fields or replays domain operations.

Ordinary reads are local. Explicit `--refresh` runs sync before reading, including publication of pending local files; it is not a remote-only display projection. There are no timed refreshes, publication intervals, cooldowns or background daemon.

Pi startup and shutdown request the same sync operation through the Rust child. Shutdown cancels inference first and does not generate a report. Lifecycle requests have a 10-second total deadline; ordinary Git subprocesses retain a 30-second deadline. Failures warn and produce a durable `stream-catalog-sync` Pi entry without undoing local saves.

A lost reply may leave an uncertain save or push. Preserve accepted receipts and inspect the actual state rather than blindly retrying. See [saving and sync](docs/catalog-sync-usage.md) for outcomes and manual inspection.

## Develop

```sh
npm ci
npm --prefix integrations/herdr-work-cards ci --ignore-scripts
just test-fast
just test
```

Rust tests cover record-local saves, captured reports, file transport, CLI, native effects and the terminal card. Frozen wire and renderer fixtures remain after retiring the Python and duplicate TypeScript implementations. Node tests cover the Pi boundary, frozen helper requests, session names, configuration and subprocess shutdown. Real Rust CLI acceptance exercises checkout integration, two-host Git publication, offline saves and fresh-process recovery.

`test:fast` is a development gate, not full acceptance; run `test` before integration, a trial or release. Tests use isolated homes, fake native endpoints and deterministic model responses, not live credentials or inference. See [Contributing](CONTRIBUTING.md) for requirements and focused commands, and the [Rust runtime decision](docs/rust-temper-architecture.md) for the cutover boundary.

## Documentation

- [Contributing](CONTRIBUTING.md): developer setup and landing checks
- [Catalog saving and sync](docs/catalog-sync-usage.md): local saves, sharing and troubleshooting
- [Testing policy](docs/testing.md): test selection, isolation and verification limits

Design decisions and architecture records live in [`docs/`](docs/), alongside usage guides.

## License

MIT
