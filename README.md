# streams

Streams keeps a personal catalog of ongoing work. Each stream has a stable identity, an optional goal, a work status and links to project documents. Sessions describe their own current state and remaining work. Plans and design decisions stay in project repositories, not in scratch notes.

This checkout includes the catalog CLI, Pi work-card extension and bundled agent skill. Published `v0.1.0` packages still use the scratch workflow; do not mix those packages with this CLI.

## Run from this checkout

Use [uv](https://docs.astral.sh/uv/) to run the CLI without replacing your installed version:

```sh
uv sync
uv run streams --help
```

The examples below use `streams`. During development, run them as `uv run streams ...` from this checkout. Use isolated XDG directories when testing; commands otherwise use your normal configuration and catalog.

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
streams config set sync.stream_seconds 300
streams config set sync.session_seconds 3600
```

The supported settings are `catalog.root`, `frontend.mode`, `sync.stream_seconds` and `sync.session_seconds`. Defaults are `$XDG_DATA_HOME/streams/catalog`, `auto`, 300 seconds (M) and 3,600 seconds (N). Intervals accept finite nonnegative numbers; zero makes every eligible event due. N also controls automatic read refresh.

Configuration is host-local at `$XDG_CONFIG_HOME/streams/config.json`; each CLI process reads it anew. Unset XDG variables use `~/.local/share` and `~/.config`. Sync strategy, remote and branch belong to catalog setup, not these settings. Changing `catalog.root` selects another catalog; initialise it before use.

There is no legacy-format compatibility or automatic migration. Start new streams or migrate old scratch material manually. An existing `none` catalog can explicitly adopt local Git backing in place:

```sh
streams init --sync git --adopt --json
```

This preserves record bytes, identities, ownership evidence, retry protection and host-local bindings. It creates a snapshot baseline, not invented historical operations. It does not configure a remote. Ordinary initialization still refuses to convert an existing catalog. Coordinate a live adoption window first; see the [adoption contract](docs/catalog-local-git-adoption.md) and [acceptance guidance](docs/catalog-sync-usage.md#acceptance-before-activation).

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

References accept an exact ID, an exact title or a unique title substring. IDs take precedence. Ambiguous references fail and report candidate IDs; they never select the first result.

`show` prints the complete logical stream and its sessions as YAML. `--json` gives the same structure. Sharing diagnostics sit beside the saved records, not inside them. `check` validates saved records, relationships and host-local runtime state. With Herdr active, it also checks live workspace and Work Card pairings. It does not probe remote branches or require saved conversations to be running.

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
streams session update SESSION_RECORD_ID --state "Search tests pass" --remaining "Check refresh conflicts"
streams session update SESSION_RECORD_ID --state-file ./state.txt --remaining-file ./remaining.txt
streams session update SESSION_RECORD_ID --status done
```

Attaching records an existing conversation; it does not start it. A session has one concise title, a status, current state and remaining work. Its native identity belongs to one host. The owning host must authorise attachments and session updates. Finishing a session does not finish the stream.

For a portable checkout reference, add `--repository ALIAS --ref BRANCH` when attaching. `--cwd` records the local directory. Session references also accept `HOST_ID/SESSION_RECORD_ID` when needed to disambiguate IDs.

## Pi work cards

The checkout's Pi extension maintains catalog records instead of generating scratch drafts. By default it reports after each completed response, using the current Pi model and trimmed conversation context. Meaningful work can enroll an effort and its first session automatically; greetings alone do not. Existing caller-workspace associations are reused without launching agents or moving focus.

- `/streams` or `/streams card` opens the card without running a model. Edit labelled fields, choose valid statuses and manage reference rows. Whole-effort actions are separate from session actions; IDs, ownership and membership are read-only.
- Editing a title or goal makes it manual. Change its mode back to automatic to allow refinement. Invalid or conflicting edits retain their input; unchanged fields are not submitted.
- Card discussion can answer without edits, refresh descriptions, or carry out an explicitly requested edit. User direction is approval; uncertain intent produces a question rather than a management action.
- `/streams report` requests a report now, not a draft awaiting Apply. Use `/streams report [instructions]` to give reporting direction. Explicit reporting can enroll an unbound conversation. `/streams reset [--auto]` regenerates automatic fields (and with `--auto`, restores automatic title/goal modes and reclaims Pi/Herdr names). The model-callable `checkpoint` tool remains unchanged.
- `/streams attach` offers initial selection or creation; attached conversations cannot switch efforts.
- `/streams settings` controls automatic/manual reporting, initial enrollment, completed responses between reports, context, model and Herdr startup/naming. Its Debug section contains independent “Debug commands” and “Helper session logging” switches. Manual reporting disables background requests, not explicit commands.
- `/streams pane` opens the native Herdr card; `/streams pane close` closes only its display. Neither requests a report or changes membership.
- Tab completes `card`, `report`, `reset`, `settings`, `attach` and `pane`. Unknown subcommands or invalid management arguments show usage without reporting or saving. Free-text instructions must follow `report`.

Settings live in `$PI_CODING_AGENT_DIR/stream-reporting.json`, defaulting to `~/.pi/agent/stream-reporting.json`. The defaults are:

```json
{"reporting": "automatic", "enrollment": "automatic", "frequency": 1, "context": "trimmed", "session_logging": false, "debug_commands": false}
```

Add `"model": {"provider": "PROVIDER", "id": "MODEL_ID"}` to select a helper model; omit it to use the current Pi model. Unavailable models fail visibly without fallback. Trimmed context excludes tool calls/results and thinking; full context preserves the active projected conversation, excluding incomplete tool batches. Oversized input fails rather than silently dropping history.

Enable “Helper session logging” in `/streams settings` → Debug, or set `"session_logging": true`, to record each helper inference as a Pi JSONL session. Logs live in `$PI_CODING_AGENT_DIR/stream-reporting-sessions/`, defaulting to `~/.pi/agent/stream-reporting-sessions/`. Each log captures the exact prompt, tool schema and supplied context before inference, raw model responses before normalization, retries, errors and the parsed result. Reports, resets, enrollment and card discussion all use this setting. The originating Pi session records a `stream-helper-session` entry linking to the log; catalog-save receipts remain in that originating session.

Logging is disabled by default. Logs copy potentially sensitive conversation content and can be large; they have private file permissions and no automatic pruning. Disabling logging stops new captures without deleting existing logs. A logging failure stops that helper update and appears as a work-card error. Logging uses Pi's session storage only: it does not launch another agent or run capture extensions such as Trajectory.

Session progress and effort metadata use separate inferences, so updating both takes two helper requests. Reports preserve captured records, ownership and conversation-lineage guards. They may update session status, but cannot park, finish or archive the whole effort. Historical conversation is evidence, not fresh verification of files or remote services.

The menu refreshes on action or reopen, not while waiting for a selection. Discussion and unsent inputs survive closing and reopening the card in the same Pi process, not extension reload. The native Herdr card reads the same saved records; it does not run another summarizer.

When testing, keep the CLI and extension from the same checkout, use isolated XDG/agent directories and do not also load the published scratch-based extension. Restart running Pi sessions after updating the extension; existing processes retain their previously loaded code. The [bundled skill](skills/streams/SKILL.md) covers record semantics and guarded reporting from other harnesses.

### Debug prompt capture and replay

Enable “Debug commands” under `/streams settings` → Debug. This does not enable live helper logging or begin recording. Run `/reload` after enabling it to expose the agent tools.

```text
/streams debug capture
/streams debug capture stream STREAM_ID
/streams debug capture workspace HERDR_WORKSPACE_ID
/streams debug replay
/streams debug replay CAPTURE_ID --model PROVIDER/MODEL_ID
/streams debug replay CAPTURE_ID --reset session
/streams debug replay CAPTURE_ID --reset stream --auto
/streams debug replay CAPTURE_ID --reset --auto
```

Capture includes associated stream history and open Pi conversations in the selected workspace or the stream's bound workspaces, including conversations without cards. The snapshot always contains available cards, full conversation evidence and raw transcripts. Trimming happens during preparation, not capture. It records open state and observed locations separately from card status. Missing or foreign-host transcripts and unsupported harnesses remain listed. The current session uses its in-memory leaf; other sessions use their persisted leaf, which cannot reveal another process's unsaved branch selection.

Replay uses current shared prompts and strict production validation with the chosen Pi-registry model. Incremental updates include captured cards by default. `--reset` regenerates the selected automatic text; `--auto` also restores its automatic naming modes in the working copy. Neither option changes the capture. Each conversation starts independently from its frozen cards, while later helpers for that conversation see its earlier simulated updates. Replay never saves real cards, attaches conversations, changes names or alters layout.

#### Compare prompts without changing production

The non-interactive CLI separates capture, preparation and inference. Capture once, prepare different prompts, then run the saved requests:

```sh
streams debug capture stream STREAM_ID --json
streams debug prepare CAPTURE_ID --sample 0 --task session --json
streams debug prepare --request REQUEST_ID --prompt ./candidate.txt --json
streams debug inspect request CANDIDATE_REQUEST_ID --section input --json
streams debug run CANDIDATE_REQUEST_ID --model PROVIDER/MODEL_ID --json
streams debug inspect run RUN_ID --section output --json
```

Each command returns artifact IDs and paths. `list capture`, `list request` and `list run` discover saved artifacts. Sample indexes start at 0; capture summaries list them. `--task` selects `enroll`, `session` or `stream`. Enrollment requires an unbound conversation. A prompt file contains plain text that replaces the whole system prompt for that request. The production tool schema and strict response validation remain unchanged.

Preparation defaults to `trim=true`, using production trimming. Add `--full` to prepare full context from the same capture. Both modes remove incomplete tool-call batches and perform normal model-input cleanup. Preparation reads only frozen evidence, never the original transcript. Older trimmed captures need recapturing before full-context preparation. To change trimming on a saved request, prepare from its capture again.

`prepare` saves the actual model messages, prompt and schema without calling a model. `run` submits that saved input without rerendering it and records the raw response plus the validated value or error. It does not apply the value to cards. A rerun creates another result; changing a prompt creates another request. Inspection defaults to summaries; explicit input, output and error views support `--offset` and `--limit` character paging.

Full-workflow replay uses the same preparation and inference:

```sh
streams debug replay CAPTURE_ID --model PROVIDER/MODEL_ID --json
streams debug replay CAPTURE_ID --model PROVIDER/MODEL_ID --prompt ./candidate.txt --full --json
streams debug replay CAPTURE_ID --model PROVIDER/MODEL_ID --reset stream --auto --json
```

Replay's `--prompt` replaces the system prompt for every helper in that workflow. Use a single prepared task when testing a task-specific prompt. Omitting an override uses the shared production prompts. Interactive replay asks for confirmation; explicit CLI and tool calls authorise provider use and normal inference costs. Failed or cancelled runs and partial replay failures return a nonzero CLI status while retaining their artifacts.

The agent tools are `stream_debug_capture`, `stream_debug_prepare`, `stream_debug_run`, `stream_debug_replay`, `stream_debug_inspect` and `stream_debug_list`. They use the same operations and require Debug commands to remain enabled. Tools take prompt text directly; CLI `--prompt` reads a file. Disabling the setting blocks registered tools immediately; enabling it requires `/reload` to discover them.

The CLI requires Node and an installed Pi SDK with `createAgentSessionServices` (Pi 1.3 or later). It uses that installation's providers and authentication, not the repository's test SDK. It loads configured extension factories to register providers, but creates no Pi session and dispatches no session-start events. Source checkouts find the paired runner automatically. For a Python wheel install, set `STREAMS_PROMPT_LAB_RUNNER` to `extensions/stream-checkpoint/lab-cli.mjs` in the matching installed Pi Streams package.

Captures live under `$PI_CODING_AGENT_DIR/stream-debug/captures/`; requests under `stream-debug/requests/`; runs under `stream-debug/runs/`. Storage has private permissions and is rejected inside version control, even in ignored directories. Each replay writes `comparison.md`, `run.json`, prepared-request IDs, reconstructed inputs and private helper JSONL containing rendered prompts, schemas, raw responses and per-task failures. A successful session task does not hide a failed effort task. Skipped actions and refused simulated saves are distinct from successful empty model deltas. Saved files have no automatic pruning; disabling debug retains them. See the [capture and replay ADR](docs/pi-prompt-replay.md) for boundaries and rationale.

## Native Herdr work cards

The original Work Cards source, tests and design documents are imported under [`integrations/herdr-work-cards`](integrations/herdr-work-cards), with its original commit history intact through `e7d74ce22f95893fa8eed570ef2e6a14d61f74bd`. The [native projection decision](docs/native-herdr-work-cards.md) identifies which behavior carries forward and which legacy authority is superseded.

The read-only Streams adapter and Pi lifecycle wiring passed isolated native acceptance and are active in the local runtime. The [cutover plan](plans/catalog-cutover.md) distinguishes that activated base from follow-up changes awaiting their own verification and activation. The standalone Work Cards runtime is retired. The root Streams extension loads the projection and naming modules after exact Herdr/Pi attestation; they do not run a separate summarizer or write another catalog.

Native presentation settings remain in `$PI_CODING_AGENT_DIR/work-cards.json`, separate from reporting settings. `workspace_startup` selects automatic or manual pane startup. `workspace_naming`, `tab_naming` and `session_naming` are independent booleans, each defaulting to `true` regardless of startup; `null` is invalid. Manual native names remain protected. `document_command` selects a trusted editor command, otherwise `$EDITOR` or `less` is used. Removed inference settings are rejected; see the [presentation configuration](integrations/herdr-work-cards/README.md#presentation-configuration).

Closing with `q` or `/streams pane close` is display-only and suppresses automatic reopening, including after reload. `/streams pane` explicitly reopens it. An uncertain launch remains blocked for inspection, even after an explicit request.

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

`--repository` on `checkout register` is an exact URL; on `session directory` it is the Stream's repository alias. A Session's selected checkout wins for its repository. Set a default explicitly for documents in other repositories. Reads never infer bindings from Git remotes, create checkouts or change defaults. A moved checkout keeps its ID; `session directory --cwd PATH` can replace an obsolete directory selection.

Documents prefer the viewing Session's current working file, including local edits. Optional `commit` and `source_session_id` fields record provenance; they do not pin that working file. When it is unavailable, the card can use a known source checkout or recorded revision. Use Shift+letter to choose among available sources. The CLI extracts revision contents only when opened. Missing files or mappings remain visible errors and never remove the shared reference. Host-local checkout registrations live under `$XDG_STATE_HOME/streams/checkouts`.

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

Herdr `open` focuses an existing verified workspace, or restores the last recorded agent-tab arrangement after it closes. It preserves grouping, order and manual labels, excluding done and parked Sessions by default. `--session` explicitly selects a conversation, including done or parked work. Existing tabs and agents are never closed automatically. Without a saved arrangement, a sole unfinished local Session is restored; several candidates produce a shell and a request to choose.

`resume` replaces the current terminal process with one recorded Pi, Codex or Claude conversation. Multiple eligible Sessions require `--session`; it does not reconstruct or focus a workspace. Both paths refuse a duplicate live conversation and validate saved files. Missing or ambiguous files are reported, not replaced with fresh conversations. CMUX supports single-session opening, not arrangement reconstruction.

Capture the current verified Herdr arrangement with `streams workspace capture STREAM_ID`. Pi also captures after an attested native refresh. Capture does not enroll conversations or save progress. Restoration does not recreate unrelated shell/editor tabs or exact pane geometry.

`--current` uses the invoking session or workspace's explicit association. It never infers a stream from a title, working directory or another client's GUI focus. `--frontend` chooses an adapter independently of the target. An association on another frontend endpoint or a partial open requires inspection; Streams does not automatically relaunch or roll back native resources. If a creation response is lost, a saved pending-attempt file blocks another launch. Keep that evidence until you have reconciled native state.

An explicit `--cwd` updates the selected session's local directory mapping, including on resume and workspace reuse. Reusing a workspace does not change an existing terminal's working directory.

Local directories and native bindings live under `$XDG_STATE_HOME/streams/catalogs/<catalog-path-hash>`, not in the shared catalog. The default state root is `~/.local/state`. Opening requires effective authority for this host. Ownership handover and cross-host session reattachment are not implemented.

## Understand saving and sharing

A local-only save succeeds when storage persists it. A Git save succeeds after a local commit. Results report local saving separately from publication; an offline push does not undo a successful edit. Unpublished commits remain for later sync:

```sh
streams sync --json
streams list --refresh
streams show STREAM_ID --refresh --json
```

Without a remote, Git saves report `local="committed"` and `sharing="local-only"`. Reads and lifecycle syncs stay local, report freshness `local-only` and create no publication clocks. Adding a Git remote outside Streams fails closed; it does not enable publication.

With a configured remote, the Git coordinator checks publication intervals after successful and no-change writes. Stream edits, starts and attachments use M, initially 5 minutes. Session reports use N, initially 60 minutes, or M while stream edits are pending. Explicit `sync` bypasses those intervals and the failed-attempt cooldown, not safety checks. There is no background daemon.

Pi session startup runs `streams sync --event start --json`, respecting M and the cooldown. Shutdown cancels inference and awaits `streams sync --event shutdown --json`, bypassing timing without generating a summary. Both lifecycle calls have a 10-second total deadline covering the CLI and its child process group. Ordinary CLI commands retain their library timeouts. Failed or uncertain lifecycle syncs produce a warning and a durable `stream-catalog-sync` Pi entry; they do not undo local saves. A timeout can leave uncertain publication or interrupted adoption: run `streams sync` to inspect the outcome, and follow the [manual repair guidance](docs/catalog-sync-usage.md#inspecting-interrupted-saves-and-manual-changes) if required.

`list` and `show` refresh shared state when the read interval expires, or when you pass `--refresh`. Reads never push. A refreshed view can combine confirmed remote changes with pending local edits without rewriting local records. Writes and runtime authority still use saved local state. Run `streams sync` before editing or opening a record visible only in that refreshed view.

If reconciliation finds a conflict, reads retain the complete local view and expose the confirmed alternative in sharing diagnostics. The whole outgoing batch stays blocked. Failed refreshes report stale data; uncertain publication and interrupted local saves remain visible rather than being treated as safe retries.

## Develop

```sh
uv sync
npm ci
npm --prefix integrations/herdr-work-cards ci --ignore-scripts
mise run test
```

Python tests cover the catalog, CLI, sync library, guarded agent API and runtime adapters. Node tests cover reporting, the card editor, discussion, settings and real checkout-CLI integration inside and outside a repository. An isolated two-host Git test covers Pi lifecycle hooks, guarded reports, second-host refresh, offline saves and fresh-process retry. The native deadline test checks that the CLI and a child process stop. Tests use no live credentials or model inference. Native workspace effects remain mocked.

`mise run test:herdr` builds and runs the native work-card regression suite; it also runs as part of `mise run test`. It covers the Streams adapter, renderer, naming and document opening. Native effects use mocked protocols; run isolated native acceptance before activating changed launch code.

The Python package requires Python 3.11 or newer. See the [durable catalog decision](docs/durable-work-catalog.md), [sync decision](docs/catalog-git-sync.md) and temporary [cutover plan](plans/catalog-cutover.md) for scope and remaining integration work.

## License

MIT
