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
streams init --sync git --remote git@github.com:you/work-catalog.git --branch main
```

The Git remote must already exist. Its branch must be empty or contain a supported Streams catalog, not arbitrary Git files. Streams uses one shared branch. Setup records its sync strategy and a persistent local host identity. Ordinary commands never initialise a catalog or fall back to another storage mode.

Inspect or change the everyday settings:

```sh
streams config
streams config get catalog.root
streams config set catalog.root ~/work-catalog
streams config set frontend.mode auto
```

The supported settings are `catalog.root` and `frontend.mode`. The defaults use `$XDG_DATA_HOME/streams/catalog` and `auto`. Configuration lives at `$XDG_CONFIG_HOME/streams/config.json`; unset XDG variables use `~/.local/share` and `~/.config`. Sync strategy, remote and branch belong to catalog setup, not these settings. Changing `catalog.root` selects another catalog; initialise it before use.

There is no legacy-format compatibility or automatic migration. Start new streams or migrate old records manually. An initialised catalog cannot currently change its sync strategy in place. Adding Git backing while preserving local records and identities is follow-on work.

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

`show` prints the complete logical stream and its sessions as YAML. `--json` gives the same structure. Sharing diagnostics sit beside the saved records, not inside them. `check` validates saved records and relationships; it does not check whether remote branches, documents or native sessions still exist.

## Edit work

```sh
streams update STREAM_ID --title "Catalog discovery"
streams update STREAM_ID --goal-file ./goal.txt
streams update STREAM_ID --clear-goal
streams update STREAM_ID --status parked
streams update STREAM_ID --add-repository streams https://github.com/wackywendell/streams
streams update STREAM_ID --add-document streams lifecycle-design docs/design.md "Design decisions"
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

- Alt+P opens the card without running a model. Edit labelled fields, choose valid statuses and manage reference rows. Whole-effort actions are separate from session actions; IDs, ownership and membership are read-only.
- Editing a title or goal makes it manual. Change its mode back to automatic to allow refinement. Invalid or conflicting edits retain their input; unchanged fields are not submitted.
- Card discussion can answer without edits, refresh descriptions, or carry out an explicitly requested edit. User direction is approval; uncertain intent produces a question rather than a management action.
- `/checkpoint` requests a report now, not a draft awaiting Apply. Explicit reporting can enroll an unbound conversation. `/checkpoint:attach` offers initial selection or creation; attached conversations cannot switch efforts.
- `/checkpoint:settings` controls automatic/manual reporting, initial enrollment, completed responses between reports, context and model. Manual reporting disables background requests, not explicit commands.

Settings live in `$PI_CODING_AGENT_DIR/stream-reporting.json`, defaulting to `~/.pi/agent/stream-reporting.json`. The defaults are:

```json
{"reporting": "automatic", "enrollment": "automatic", "frequency": 1, "context": "trimmed"}
```

Add `"model": {"provider": "PROVIDER", "id": "MODEL_ID"}` to select a helper model; omit it to use the current Pi model. Unavailable models fail visibly without fallback. Trimmed context excludes tool calls/results and thinking; full context preserves the active projected conversation, excluding incomplete tool batches. Oversized input fails rather than silently dropping history.

Session progress and effort metadata use separate inferences, so updating both takes two helper requests. Reports preserve captured records, ownership and conversation-lineage guards. They may update session status, but cannot park, finish or archive the whole effort. Historical conversation is evidence, not fresh verification of files or remote services.

The menu refreshes on action or reopen, not while waiting for a selection. Discussion and unsent inputs survive closing and reopening the card in the same Pi process, not extension reload. Work Cards projections and native tab/title updates remain separate follow-on work.

When testing, keep the CLI and extension from the same checkout, use isolated XDG/agent directories and do not also load the published scratch-based extension. Restart running Pi sessions after updating the extension; existing processes retain their previously loaded code. The [bundled skill](skills/streams/SKILL.md) covers record semantics and guarded reporting from other harnesses.

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

`open` creates or reuses a native workspace through Herdr or CMUX. With no attached session, it leaves a shell. `resume` replaces the current terminal process with one recorded Pi, Codex or Claude conversation. More than one eligible session requires `--session`; Streams does not guess the latest one. Full multi-tab reconstruction is not implemented.

`--current` uses the invoking session or workspace's explicit association. It never infers a stream from a title, working directory or another client's GUI focus. `--frontend` chooses an adapter independently of the target. A stale native binding or partial open requires inspection; Streams does not automatically relaunch or roll back native resources. If a creation response is lost, a saved pending-attempt file blocks another launch. Keep that evidence until you have reconciled native state.

An explicit `--cwd` updates the selected session's local directory mapping, including on resume and workspace reuse. Reusing a workspace does not change an existing terminal's working directory.

Local directories and native bindings live under `$XDG_STATE_HOME/streams/catalogs/<catalog-path-hash>`, not in the shared catalog. The default state root is `~/.local/state`. Opening requires effective authority for this host. Ownership handover and cross-host session reattachment are not implemented.

## Understand saving and sharing

A local-only save succeeds when storage persists it. A Git save succeeds after a local commit. Results report local saving separately from publication; an offline push does not undo a successful edit. Unpublished commits remain for later sync:

```sh
streams sync --json
streams list --refresh
streams show STREAM_ID --refresh --json
```

The Git coordinator checks publication intervals after successful and no-change writes. Stream edits, starts and attachments use the 5-minute interval. Session reports use 60 minutes, or 5 minutes while stream edits are pending. Explicit `sync` bypasses those intervals. There is no background daemon in this CLI.

`list` and `show` refresh shared state when the read interval expires, or when you pass `--refresh`. Reads never push. A refreshed view can combine confirmed remote changes with pending local edits without rewriting local records. Writes and runtime authority still use saved local state. Run `streams sync` before editing or opening a record visible only in that refreshed view.

If reconciliation finds a conflict, reads retain the complete local view and expose the confirmed alternative in sharing diagnostics. The whole outgoing batch stays blocked. Failed refreshes report stale data; uncertain publication and interrupted local saves remain visible rather than being treated as safe retries.

## Develop

```sh
uv sync
npm ci
mise run test
```

Python tests cover the catalog, CLI, sync library, guarded agent API and runtime adapters. Node tests cover reporting, the guided card editor, discussion, settings and real checkout-CLI integration inside and outside a repository; neither contract case is skipped. Tests exercise Pi's local model-registry and context-projection APIs without credentialed inference. Native runtime effects remain mocked rather than opening real workspaces.

The Python package requires Python 3.11 or newer. See the [durable catalog decision](docs/durable-work-catalog.md), [sync decision](docs/catalog-git-sync.md) and temporary [cutover plan](plans/catalog-cutover.md) for scope and remaining integration work.

## License

MIT
