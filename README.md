# streams

Streams keeps a personal catalog of ongoing work. Each stream has a stable identity, an optional goal, a work status and links to project documents. Sessions describe their own scope and achievements, current status and remaining work. Plans and design decisions stay in project repositories, not in scratch notes.

This checkout includes the catalog CLI, Pi work-card extension and bundled agent skill. Published `v0.1.0` packages still use the scratch workflow; do not mix those packages with this CLI.

## Run from this checkout

This branch uses Rust for the CLI, catalog, native lifecycle and terminal card. Build it without replacing your installed CLI:

```sh
mise run build
./target/debug/streams --help
```

The examples below use `streams`; from this checkout, substitute `./target/debug/streams`. If `CARGO_TARGET_DIR` is set, use its `debug/streams` instead. Use isolated XDG directories when testing; commands otherwise use your normal configuration and catalog.

The Pi extension selects this checkout's debug Rust build by default. `STREAMS_BIN_PATH` can select another absolute Rust executable. There is no Python or `PATH` fallback. Loading this checkout does not update installed tools or reload running processes.

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

The supported settings are `catalog.root`, `frontend.mode`, `naming.stream` and `naming.session`. Defaults are `$XDG_DATA_HOME/streams/catalog`, `auto` and automatic title generation for both record types. Sync runs explicitly or at Pi startup/shutdown, not on a timer.

Configuration is host-local at `$XDG_CONFIG_HOME/streams/config.json`; each CLI process reads it anew. Unset XDG variables use `~/.local/share` and `~/.config`. Sync strategy, remote and branch belong to catalog setup, not these settings. Changing `catalog.root` selects another catalog; initialise it before use.

Catalog records use host-partitioned addresses:
`hosts/<host>/streams/<id>.yaml` and `hosts/<host>/conversations/<id>.yaml`.
A Session owns its membership; moving it changes that Session alone. Checkout
URLs and host-local Session directory selections remain with the conversation.

Initialization requires an empty catalog. Select `none` or `git` transport when
creating it; initialization does not adopt an existing repository or convert old
records. The alpha switch uses the [manual shutdown and backup
checklist](docs/record-local-operations.md), followed by fresh initialization and
re-enrollment. There is no runtime legacy reader or built reset component.

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

Search is a case-insensitive literal substring of the title or goal. It has no operators or query language. Repeat `--status` to select several statuses. `list` excludes archived streams unless you use `--archived` or `--all`.

References accept an exact ID, an exact title or a unique title substring. IDs take precedence; UUID prefixes are not supported. Ambiguous references fail and report candidate IDs; they never select the first result.

`show` prints the complete logical stream and its sessions as YAML. `--json` gives the same structure. `check` validates saved records and relationships. It does not require saved conversations to be running or probe remote branches.

## Edit work

```sh
streams update STREAM_ID --title "Catalog discovery"
streams update STREAM_ID --goal-file ./goal.txt
streams update STREAM_ID --clear-goal
streams update STREAM_ID --status parked
streams update STREAM_ID --add-repository streams https://github.com/wackywendell/streams
streams update STREAM_ID --add-document streams lifecycle-design docs/design.md "Design decisions"
streams update STREAM_ID --document-commit streams docs/design.md COMMIT_ID
streams update STREAM_ID --document-session streams docs/design.md SESSION_ID
streams update STREAM_ID --remove-document streams docs/design.md
streams update STREAM_ID --add-issue https://example.org/issues/42 "Tracking issue"
streams update STREAM_ID --add-pr https://example.org/pulls/43 "Implementation"
streams archive STREAM_ID
streams restore STREAM_ID
```

Statuses are `active`, `waiting`, `blocked`, `parked` and `done`. Archive controls visibility independently of status. Neither archiving nor marking work done closes its runtime.

Setting a title or goal makes that field manual. Use `--title-mode automatic` or `--goal-mode automatic` to allow future automatic refinement. Compact summaries clear when their source text changes.

References use repository aliases, remote branch or bookmark names, and repository-relative paths. Absolute checkout paths stay local. Collection flags are repeatable. Use the corresponding `--remove-*` flag to remove an entry; replacing an existing entry requires removing and adding it explicitly. Run `streams update --help` for argument shapes.

Omitted fields stay unchanged. `--clear-goal` removes a goal. Text-file flags read UTF-8; use `-` for stdin. Only one field can consume stdin per command.

## Record session progress

```sh
streams session attach STREAM_ID --harness pi --session-id NATIVE_ID --title "CLI tests"
streams session list --stream STREAM_ID
streams session show SESSION_RECORD_ID
streams session update SESSION_RECORD_ID --recap "Added search filters" --state "Checking refresh conflicts" --remaining "Review conflict handling"
streams session update SESSION_RECORD_ID --recap-file ./recap.txt --state-file ./state.txt --remaining-file ./remaining.txt
streams session update SESSION_RECORD_ID --recap-short "Search" --state-short "Review" --remaining-short "Land"
streams session update SESSION_RECORD_ID --status done
```

Attaching records an existing conversation; it does not start it. A session has one concise title, a lifecycle status, a cumulative `recap`, current `state` (labelled Status) and `remaining` work. Each text has a matching `_short` form. Reports prefer 1–2 words and fewer than 20 characters, with a 24-character maximum. Session text stays report-writable; only titles have automatic/manual/inherit modes. All 6 text fields accept `--FIELD`, `--FIELD-file` and `--clear-FIELD` (use hyphens in flag names). Clearing full text saves an empty string; clearing a short form saves null. Its native identity belongs to one host. The owning host must authorise attachments and session updates. Finishing a session does not finish the stream.

For a portable checkout reference, add `--repository ALIAS --ref BRANCH` when attaching. `--cwd` records the local directory. Session references also accept `HOST_ID/SESSION_RECORD_ID` when needed to disambiguate IDs. Attach creates a record; it cannot move an already enrolled Session. There is no Session-move CLI command.

## Pi work cards

The checkout's Pi extension maintains catalog records instead of generating scratch drafts. By default it reports after each completed response, using the current Pi model and trimmed conversation context. Meaningful work can enroll an effort and its first session automatically; greetings alone do not. Existing caller-workspace associations are reused without launching agents or moving focus.

- `/streams` or `/streams card` opens the card without running a model. Edit labelled fields, choose valid statuses and manage reference rows. Whole-effort actions are separate from session actions; IDs, ownership and membership are read-only.
- Editing a title or goal makes it manual. Titles can return to automatic or inherit the global default; goals use automatic or manual. Invalid or conflicting edits retain their input; unchanged fields are not submitted.
- `/streams name [session|stream] <name>` saves and protects a title, including unchanged text. The default target is Session. `/streams title-mode [session|stream] automatic|manual|inherit` changes policy without generating a new title.
- Card discussion can answer without edits, refresh descriptions, or carry out an explicitly requested edit. User direction is approval; uncertain intent produces a question rather than a management action.
- `/streams report` requests a report now, not a draft awaiting Apply. Use `/streams report [instructions]` to give reporting direction. Explicit reporting can enroll an unbound conversation. `/streams reset [--auto]` regenerates automatic fields (and with `--auto`, restores automatic title/goal modes before inference). The model-callable `checkpoint` tool remains unchanged.
- `/streams attach` offers initial selection or creation; attached conversations cannot switch efforts.
- `/streams settings` controls reporting, enrollment, response frequency, context, model and Herdr startup. Its Naming submenu separates title-generation defaults, incoming Pi name edits and outgoing frontend synchronization. Manual reporting disables background requests, not explicit commands.
- `/streams pane` opens the native Herdr card; `/streams pane close` closes only its display. Neither requests a report or changes membership.
- Tab completes `card`, `report`, `name`, `title-mode`, `reset`, `settings`, `attach` and `pane`. Unknown subcommands or invalid management arguments show usage without reporting or saving. Free-text instructions must follow `report`.

Settings live in `$PI_CODING_AGENT_DIR/stream-reporting.json`, defaulting to `~/.pi/agent/stream-reporting.json`. The defaults are:

```json
{"reporting": "automatic", "enrollment": "automatic", "frequency": 1, "context": "trimmed", "pi_name_edits": true}
```

Add `"model": {"provider": "PROVIDER", "id": "MODEL_ID"}` to select a helper model; omit it to use the current Pi model. Unavailable models fail visibly without fallback. Trimmed context excludes tool calls/results and thinking; full context preserves the active projected conversation, excluding incomplete tool batches. Oversized input fails rather than silently dropping history.

The Naming submenu saves Stream and Session title-generation defaults through the Rust configuration API (`naming.stream` and `naming.session`). They live in `$XDG_CONFIG_HOME/streams/config.json`, normally `~/.config/streams/config.json`, not `stream-reporting.json`. Defaults affect records in `inherit` mode, not explicit automatic or manual modes. Enrollment may still create an initial title under a manual default. Changing a default does not run inference or rename a frontend.

Pi's built-in `/name` imports a different name as a manual Session title when Pi name edits are enabled. Matching names are ignored; use `/streams name session <name>` to pin one. Unbound conversations stay native-only. Incoming edits and outgoing Pi synchronization are independent: disable both to keep the names separate. A rare late projection echo can be imported as manual; see the [name-ownership decision](docs/name-ownership.md#native-pi-name).

Session progress and effort metadata use separate inferences, so updating both takes two helper requests. Reports preserve captured records, ownership and conversation-lineage guards. They may update session status, but cannot park, finish or archive the whole effort. Historical conversation is evidence, not fresh verification of files or remote services.

The menu refreshes on action or reopen, not while waiting for a selection. Discussion and unsent inputs survive closing and reopening the card in the same Pi process, not extension reload. The native Herdr card reads the same saved records; it does not run another summarizer.

When testing, keep the CLI and extension from the same checkout, use isolated XDG/agent directories and do not also load the published scratch-based extension. Restart running Pi sessions after updating the extension; existing processes retain their previously loaded code. The [bundled skill](skills/streams/SKILL.md) covers record semantics and guarded reporting from other harnesses.

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

Other native presentation settings remain in `$PI_CODING_AGENT_DIR/work-cards.json`, separate from reporting settings. `workspace_naming`, `tab_naming` and `session_naming` are independent booleans, each defaulting to `true`; `null` is invalid. Enabled surfaces mirror saved Streams titles. Disabling outgoing synchronization stops Streams from renaming that surface; Pi name edits remain independently controlled by `pi_name_edits`. `document_command` selects a trusted editor command, otherwise `$EDITOR` or `less` is used. Removed inference settings are rejected; see the [presentation configuration](integrations/herdr-work-cards/README.md#presentation-configuration).

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
streams config set frontend.mode herdr  # auto, herdr, cmux or none
streams frontend get --json
streams start "New effort" --frontend herdr
streams open STREAM_ID --frontend herdr
streams open STREAM_ID --session SESSION_RECORD_ID --cwd ~/code/project
streams resume STREAM_ID --session SESSION_RECORD_ID
streams open --current
streams resume --current
```

`start` creates a stream and opens it. If opening fails, the stream remains saved and the result reports its ID. Retry `open` with that ID instead of creating another stream.

Herdr `open` discovers active same-host Sessions and opens missing conversations in separate tabs in an observed or new Stream workspace. Already-observed running Sessions stay where they are and their locations are reported. `--session` explicitly selects one conversation, including inactive work. No tab order, pane geometry or saved arrangement is restored. Existing tabs and agents are never closed automatically.

`resume` replaces the current terminal process with one recorded Pi, Codex or Claude conversation. Multiple eligible Sessions require `--session`; it does not reconstruct or focus a workspace. Both paths refuse a duplicate live conversation and validate saved files. Missing or ambiguous files are reported, not replaced with fresh conversations. CMUX supports single-session opening, not multi-Session reopening.

`--current` uses the invoking Session or workspace's explicit association. It never infers a Stream from a title, working directory or another client's GUI focus. `--frontend` chooses an adapter independently of the target. Native reuse uses one current observation. A failed or lost reply warns and stops; no saved attempt is replayed. Rare duplicate workspaces, editors or conversations can require manual cleanup.

An explicit `--cwd` updates the selected session's local directory mapping, including on resume and workspace reuse. Reusing a workspace does not change an existing terminal's working directory.

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
mise run test:fast
mise run test
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
