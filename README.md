# streams

Streams keeps a personal catalog of ongoing work. Each Stream has a stable identity, a title, and an optional goal. Sessions belong to a Stream and own their progress (`recap`, `state`, `remaining`), working directory, references to documents, PRs, issues and links, and named artifacts. A Stream's status is derived from its Sessions. Plans and design decisions stay in project repositories, not in scratch notes.

This checkout includes the Rust CLI (`streams`), the Pi work-card extension, the `herdr-app` mod, and the bundled agent skill.

## Run from this checkout

Build the Rust CLI without replacing your installed binary:

```sh
just build
./target/debug/streams --help
```

The examples below use `streams`; from this checkout, substitute `./target/debug/streams` (or `$CARGO_TARGET_DIR/debug/streams`). Use isolated XDG directories when testing; commands otherwise use your normal configuration and catalog.

The Pi extension runs an absolute `STREAMS_BIN_PATH` if set, then the first `streams` on `PATH` (installed by `just install-cli`), then this checkout's debug Rust build. Loaded from a checkout, it refuses a binary built from other sources, so reinstall after pulling.

## Initialise a catalog

Both storage modes require explicit setup:

```sh
streams catalog init --sync none
streams catalog init --sync git
streams catalog init --sync git --remote git@github.com:you/work-catalog.git --branch main --label laptop
```

Git without `--remote` records local commits without fetching or pushing. If you select a remote, it must already exist and its branch must be empty or contain a supported format-5 Streams catalog. Setup records the sync strategy, a persistent local host identity, and `hosts/<host>/host.json` (`{ id, label }`, defaulting `label` to the machine's hostname). Ordinary commands never initialise a catalog or fall back to another storage mode.

To enable Git sync on an existing `none` catalog (preserving existing local records), update the configured remote on a `git` catalog, or show/update this host's label:

```sh
streams catalog configure --sync git --remote git@github.com:you/work-catalog.git --branch main
streams catalog configure --sync git --clear-remote
streams catalog label
streams catalog label work-laptop
streams catalog sync --timeout 15
```

Inspect or change host-local settings:

```sh
streams config get
streams config get catalog.root
streams config set catalog.root ~/work-catalog
streams config set frontend.mode auto
streams config set sync.auto_interval_minutes 10
streams config path
```

Core settings live in `$XDG_CONFIG_HOME/streams/config.toml` and include `catalog.root`, `frontend.mode`, `naming.stream`, `naming.session`, `lifecycle.decay_days` (default `14`, `0` disables), and `sync.auto_interval_minutes` (default `10`, `0` disables background sync after writes), alongside `herdr.*` and `pi.*` namespaces. `naming.stream` and `naming.session` are two-value settings (`automatic` or `manual`) controlling whether automatic reports may propose titles for Streams and Sessions. Defaults are `$XDG_DATA_HOME/streams/catalog`, `auto`, `automatic` for both naming settings, 14-day inactivity decay, and 10-minute automatic write-triggered sync.

Format-5 catalog records use host-partitioned JSON files:

- `hosts/<host>/host.json` (`{ "id", "label" }`)
- `hosts/<host>/streams/<id>.json`
- `hosts/<host>/sessions/<harness>/<id>/session.json`
- `hosts/<host>/sessions/<harness>/<id>/artifacts/<name>`

Each record carries its own `revision` and `updated_at_ms`, which change only when a save changes the record. A Session owns its `stream_id`, `directory`, `references`, and `artifacts`.

### Resetting the catalog

Streams reads only catalog format 5 and never migrates old formats at runtime. Commands against an older catalog fail with an error pointing to these steps. To reset:

1. Stop Pi sessions running the Streams extension, classic Herdr card viewers, and the `herdr-app` mod.
2. Move the catalog directory aside:

   ```sh
   root="$(streams config get catalog.root)"
   mv "$root" "$root.old"
   ```

3. Remove obsolete format-4 host state once all catalogs on this host are converted or reset:

   ```sh
   rm -rf "${XDG_STATE_HOME:-$HOME/.local/state}/streams/catalogs" \
          "${XDG_STATE_HOME:-$HOME/.local/state}/streams/checkouts"
   ```

4. Initialise an empty catalog with `streams catalog init --sync none` or `streams catalog init --sync git` (using a fresh remote branch when sharing).
5. Restart Pi and re-enroll work with `/streams attach` or `streams stream create` and `streams session create`.

Saving a record is local. Publication (`streams catalog sync`) is a separate transport operation, so a network failure cannot turn an accepted local save into a failed save.

## Entity CLI

Commands are organised by entity (`stream`, `session`, `artifact`, `config`, `catalog`), with root aliases `list` (for `stream list`) and `resume` (for `session resume`). The CLI never reads Pi environment variables, `hc` labels, or frontend state to select records — pass a full key, `HARNESS/ID`, ID, or unique ID prefix explicitly.

### Streams

```sh
streams stream create --title "Catalog CLI" --goal "Organise commands by entity"
streams list
streams stream list "catalog" --status active --local
streams stream list --fields key,title,status,goal --json
streams stream show STREAM
streams stream edit STREAM --title "Entity CLI"
streams stream edit STREAM --clear-goal
streams stream delete STREAM
```

`stream list` searches case-insensitive literal substrings of the title or goal. Stream status is derived from its Sessions (`active > waiting > blocked > parked > done > closed`, or `active` when empty) and is not stored or edited directly. `list` excludes `closed` Streams unless `--closed` or `--all` is passed.

Unreadable record files are reported on stderr (or under `unreadable` with `--json`) without failing the listing.

To abandon a Stream, close its Sessions (`streams session edit SESSION --status closed`, or run `streams catalog tidy [--days N]` to close inactive open Sessions past `lifecycle.decay_days`). An empty Stream derives status `active`.

Setting a Stream's title or goal via CLI flags marks that field `manual: true`. Use `--title-mode automatic` or `--goal-mode automatic` to allow future automatic reports to refine it.

### Sessions and references

```sh
streams session create pi/$PI_SESSION_ID --stream STREAM --title "CLI tests" --directory "$PWD"
streams session list --stream STREAM
streams session show pi/$PI_SESSION_ID --json
streams session edit pi/$PI_SESSION_ID --recap "Added entity CLI" --state "Running tests" --remaining "Update docs"
streams session edit pi/$PI_SESSION_ID --status done
streams session edit pi/$PI_SESSION_ID --stream OTHER_STREAM
```

A Session's ID is its harness session ID (`HOST/HARNESS/ID`). `session create` requires exact `HARNESS/ID`, records the conversation's working directory (inspecting Git/jj metadata once on disk when present), and never launches a harness. Moving a Session to another Stream on the same host is an ordinary `session edit SESSION --stream OTHER_STREAM`.

Each Session owns its `references` map, keyed by HTTP(S) URL (`pr`, `issue`, `link`) or path relative to `session.directory.path` (`document`). Roles are `deliverable`, `dependency`, `brief`, and `background`; statuses are `active`, `fulfilled`, and `dropped`.

```sh
streams session edit SESSION --add-document docs/design.md --name Design --purpose "Guide implementation" --role brief
streams session edit SESSION --add-pr https://example.org/org/repo/pull/43 --name "PR #43" --purpose "Deliver change" --role deliverable
streams session edit SESSION --add-issue https://example.org/org/repo/issues/12 --name "Issue #12" --purpose "Track bug" --role background
streams session edit SESSION --add-link https://example.org/docs --name Docs --purpose "Reference" --role background
streams session edit SESSION --drop-reference https://example.org/org/repo/pull/43
streams session document read SESSION docs/design.md
streams session document view SESSION docs/design.md
```

Dropped entries stay in the Session record so automatic reports do not re-add them. People can restore them explicitly by re-adding with `--add-document` / `--add-pr` / `--add-issue` / `--add-link` (or via Pi's Session panel).

Both `stream edit` and `session edit` also accept `--input -` with `{"changes": {...}, "before": {...}?}` for compare-and-save edits (see `skills/streams/references/reporting.md`). Exit codes: `0` done, `1` failed, `2` usage error, `3` printed usual result with partial failure (such as units listed in `not_written` or a sync sharing warning).

### Resume

```sh
streams resume SESSION
streams session resume SESSION
```

`resume` runs the Session's harness (`pi`, `codex`, or `claude`) in its recorded `directory.path` inside the current terminal.

### Artifacts

Artifacts store named UTF-8 outputs (up to 1 MiB) attached to a Session when they have no repository home. Names use 1–64 lowercase ASCII letters, digits, `.`, `_`, or `-`, starting with a letter or digit.

```sh
streams artifact write assessment.md --session pi/$PI_SESSION_ID --description "Investigation findings" --file ./assessment.md
printf 'Delegate findings' | streams artifact write findings.md --session pi/$PI_SESSION_ID --description "Delegate findings"
streams artifact list --session pi/$PI_SESSION_ID --json
streams artifact list --stream STREAM --json
streams artifact read HOST/pi/SESSION_ID/assessment.md
streams artifact view HOST/pi/SESSION_ID/assessment.md
streams artifact write assessment.md --session pi/$PI_SESSION_ID --description "Updated description" --description-only
streams artifact delete HOST/pi/SESSION_ID/assessment.md --confirm
```

Artifact keys are `HOST/HARNESS/ID/NAME` and follow the Session if it moves between Streams. `artifact read` writes raw bytes to stdout; `artifact view` opens a read-only `less` pager on a terminal.

## Pi work cards

The checkout's Pi extension talks directly to the public `streams` CLI and `hc`. By default it reports after each completed response using the current Pi model and trimmed conversation context.

- `/streams` or `/streams card` opens the work card without running a model. Stream and Session tabs are separate; `"References…"` lives in the Session tab.
- Editing a title or goal marks it `manual: true`. `/streams name [session|stream] <name>` saves a manual title; `/streams title-mode [session|stream] automatic|manual` switches only the manual mark.
- `/streams report [instructions]` requests a report immediately. `/streams reset [session|stream|all] [--auto]` regenerates automatic fields (with `--auto`, clearing manual marks first). The model-callable `checkpoint` tool requests an immediate report.
- `/streams attach`, `select_stream`, and `create_stream` handle initial enrollment only.
- `/streams settings` configures `pi.reporting.*`, `pi.naming.*`, and `naming.stream` / `naming.session`. `naming.stream` and `naming.session` directly control whether automatic reports may propose titles for Streams and Sessions (`automatic` or `manual`).
- `/streams sync` runs `streams catalog sync`.
- `/streams open` and `/streams close` open or close the classic Herdr card pane.
- Pi's built-in `/name` imports a changed bound Session name as manual when `pi.naming.import_edits` is enabled. Incoming edits and outgoing Pi name mirroring (`pi.naming.session`) are independent booleans.
- Under `hc` (`HC_TERMINAL_ID` set), the extension writes `streams.session=<KEY>` via `hc label` so `herdr-app` can identify the focused Session.

### Helper diagnostics and frozen prompt experiments

Enable `pi.reporting.session_logging` under Debug in `/streams settings` to save private JSONL helper logs under `$PI_CODING_AGENT_DIR/stream-helpers`.

Enable `pi.reporting.debug_commands` (`streams config set pi.reporting.debug_commands true`) to capture samples and replay helper prompts offline without modifying live catalog records:

```sh
streams debug prompts --help
streams debug prompts capture stream STREAM --json
streams debug prompts prepare CAPTURE_ID --sample 0 --task session --json
streams debug prompts run REQUEST_ID --model PROVIDER/MODEL --json
streams debug prompts replay CAPTURE_ID --model PROVIDER/MODEL --json
```

## Integrations

### `herdr-app` mod

The `herdr-app` mod lives under [`integrations/herdr-app`](integrations/herdr-app) and renders the Streams sidebar panel, pane header strip for enrolled Pi terminals, and Session detail popup. It calls the public `streams` CLI and `hc` directly (no daemon or custom RPC protocol):

- On focus changes or `streams.saved` label changes, it reads the focused Session (`session show`) and its Stream (`stream show`), reconciling `streams.stream` labels via `hc label`.
- Opening a Stream reuses running Pi Sessions or spawns `hc run ... streams session resume KEY` for stopped ones.
- Saved documents and artifacts open via `session document read|view` and `artifact read|view`.

Install the release `streams` CLI into `~/.local/bin/streams` and the `herdr-app` mod into `${HERDR_APP_HOME:-~/.config/herdr-app}/mods/streams/`:

```sh
just install
```

Or install only the mod (`just install-herdr-app-mod`), only the CLI (`just install-cli`), or both the mod and the reference layout (`just install-herdr-app`).

### Classic Herdr work cards

Classic Herdr support lives under [`integrations/herdr-work-cards`](integrations/herdr-work-cards) and is driven by hidden `streams card` subcommands (`card watch`, `card pane`, `card open`, `card close`, `card mode`). Card mode is `automatic` or `manual`.

## Saving and sharing

```sh
streams catalog check
streams catalog tidy
streams catalog sync --json
streams catalog configure --sync git --remote git@github.com:you/work-catalog.git
```

A save succeeds when its target record is persisted locally. With `none`, sync validates records without Git or network access. With `git`, sync commits local changes and, when a remote is configured, combines disjoint host-partitioned files and publishes. Ordinary reads are always local.

When a non-`none` sync backend is configured, writes trigger a detached background sync if at least `sync.auto_interval_minutes` (default `10`; set `0` to disable) have elapsed since the previous sync attempt. Sync outcomes are recorded in `.catalog-local/last-sync.json`, and failures are surfaced on subsequent writes and by `streams catalog check`. Pi also runs sync asynchronously on session start and with a bounded 10-second timeout after parking on session shutdown.

## Develop

```sh
npm ci
just test-fast
just test
just check-clippy
just check-fmt
```

See [Contributing](CONTRIBUTING.md) for developer setup and focused test recipes.

## License

MIT
