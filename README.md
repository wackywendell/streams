# Work Cards

Work Cards keeps a short, durable record of a piece of work beside your terminal. It is smaller than Streams: one package provides a Pi extension and a Herdr terminal plugin, without managing agents or checkouts.

The package provides explicit card attachment, background updates from Pi evidence, and a read-only Herdr pane. See [verification status](#verification-status) for tested runtimes and remaining checks.

## Keep the card small

A card records the current status, a short state summary, the checkout directory, and links to documents, issues and pull requests. Plans and architecture decision records belong in the actual project, such as `adrs/foo-bar.md`. The card links to them; it is not a scratch document or a central notes repository.

Cards live at `$XDG_STATE_HOME/herdr/cards/<slug>.yaml`, or `~/.local/state/herdr/cards/<slug>.yaml` by default. They survive workspace deletion. The initial title determines the stable filename slug. Later title edits do not rename it, and creating a colliding slug fails rather than overwriting a card.

For example:

```yaml
title: Improve queue recovery
status: active
state: Compare retry behaviour before choosing an approach
cwd: /home/developer/code/queue-service
documents:
  - path: adrs/queue-recovery.md
    description: Recovery options and decision
issues:
  - url: https://github.com/example/queue-service/issues/42
    description: Track recovery failures
prs: []
```

All 7 fields are required. Arrays may be empty. Status is `active`, `waiting`, `blocked`, `parked` or `done`. State may be empty; displayed text and descriptions must be single-line.

Document descriptions remain required in YAML. Keep them brief and describe the whole document's purpose, not its latest edit.

`cwd` is an absolute path, or a `~/...` path expanded on the execution host. Document paths are relative to that checkout. Absolute paths, traversal and symlinks escaping the checkout are rejected. Issue and PR URLs must use HTTP or HTTPS. Unknown fields and duplicate YAML keys fail validation.

Cards are per-host state. They are not automatically synced or backed up. Keep secrets out of cards and linked descriptions.

## Attach deliberately

Use these Pi commands:

| Command | Purpose |
| --- | --- |
| `/work-card new <title>` | Create a card for Pi's current directory and attach it in the caller's Herdr workspace |
| `/work-card open <slug>` | Attach an existing card in the caller's Herdr workspace |
| `/work-card show` | Show the attached card and its source path |
| `/work-card detach` | Stop updates, close the owned card pane and clear the binding without deleting the YAML |

`new` and `open` require a Herdr-managed pane. They target the caller's workspace, not the focused GUI workspace. Opening the same card reuses its verified pane. A different existing card blocks replacement; detach it before switching.

A failed `new` may leave a created YAML file even though attachment failed. Inspect Herdr, then recover with `/work-card open <slug>`.

Explicit attachment or detachment can clear a departed owner's workspace markers only after strict checks under the workspace lock. The complete card identity, nonce, pane and terminal tuple must match. Both the referenced pane and terminal must be absent, with no conflicting marked panes and an unambiguous live inventory. Cleanup clears only Work Cards' `wc_*` workspace tokens. Surviving or restored owners, mismatched cards and ambiguous markers are never reclaimed. There is no automatic recovery.

Pi records the binding in custom session entries. Those entries reach disk only after user or assistant activity; `--no-session` does not persist them. Resuming the same originating session restores a binding only if its YAML still exists and validates. Restore never reconstructs a missing card or opens a pane. Forks and cloned sessions require explicit `/work-card open <slug>`.

Detach records `updates: false` before loading configuration or cleaning up Herdr. Failed cleanup or invalid configuration retains this durable paused binding so you can inspect Herdr and retry. It stays paused after restore until explicit `open` succeeds. Legacy entries without an updates flag enable updates; malformed flags leave them paused. The custom-entry disk-flush caveat above still applies.

The current binding can detach after its YAML is deleted, using the canonical existing parent directory and filename to prove identity. A missing parent or ambiguous identity still blocks cleanup.

The read-only `work_card` tool returns the attached card, source path and document references. It cannot create, select, update or detach cards. Work Cards never automatically selects or creates cards, launches agents, or creates and switches checkouts. The helper cannot edit plans or ADRs.

## Run on the server host

The Herdr server, repository, Pi and plugin run together: either locally on macOS or remotely on Linux. A Mac client can attach to the remote Herdr server. Paths and card storage refer to the server host, not the attaching Mac.

The pane reads YAML as the source of truth and can display a card without Pi running. Herdr tokens hold ownership markers and display metadata, not durable card data. The `$wc_summary` token is limited to 80 characters. The integration does not edit Herdr's internal `session.json`.

The compact pane shows each document's repository-relative path, not its description: `[a] docs/design.md`. Document shortcuts use single letters from `a` to `z`, skipping `q`; later documents have no shortcut. Only fully visible shortcuts are active. Digits do not open documents.

Issues and PRs share consecutive numbered labels, with issues first: `[1] Issue: …`, `[2] PR: …`. Numbering is independent of document letters and continues beyond 9. These HTTP labels are Ctrl-click-only, using OSC 8 terminal links and Herdr's local-client handling. Number-key opening is deferred; digits do nothing.

Press a displayed document letter to open it in a new focused Herdr tab, named after the displayed path's filename. `j` and `k` are document letters, not scrolling controls. The card display and its updater stay alive. Documents must still be listed on the freshly read card and resolve to regular files inside its checkout. No document action runs macOS `open` or remote `xdg-open`.

The tab runs `document_command`, then `$EDITOR` if the setting is `null`, then `less` if `$EDITOR` is unset or blank. There is no `$VISUAL` preference. An editor can modify the document; the card display and helper do not. Command failures stay visible in the document terminal with a configuration hint and a press-Enter-to-close prompt. Partial tab-opening failures appear on the card with the returned tab ID when known; inspect that tab before retrying. Work Cards never closes a possibly used document tab.

In the main card view, `q` exits the display; it does not detach the Pi session or stop its updates. Explicit `open` recreates a departed display, or explicit `detach` clears its validated orphan markers. Neither action reclaims a surviving or restored owner.

The card pane does not require sidebar configuration. An optional `$wc_summary` sidebar row requires a separate Herdr configuration change.

Exiting Pi leaves the card pane running independently. Disconnecting the client retains server processes. A cold server restart does not automatically preserve arbitrary pane processes. Run `/work-card open <slug>` to reopen the pane; resuming Pi alone never opens it.

## Update from Pi evidence

Automatic checks run in the background at 2 boundaries:

- `check_every_n_responses`: every N overall responses, at `agent_settled`; default `1`
- `check_every_n_cycles`: every N completed model-response and tool-results cycles, at `turn_end`; default `10`

A cycle is not a streamed chunk or an individual parallel tool result. The cycle count resets for each overall response. Setting either interval to `0` independently disables that trigger. Set both to `0` to disable automatic checks.

Only one check runs at a time per Pi session, not globally across intentionally separate bindings. While it runs, newer eligible evidence replaces the pending evidence. The same evidence revision is not checked twice. Continuing cycles must not cancel an in-flight check or require an idle session before applying an update.

The helper returns either `no_change` or a validated patch to `status`, `state`, `documents`, `issues` and `prs`. It cannot change `title`, `cwd`, or repository document contents. Checks reject stale results after card edits, binding changes or session changes.

`no_change` writes nothing, including no modification-time change. Updates use revision checks and atomic replacement. Cancellation is checked synchronously at the commit boundary, immediately before rename. An aborted prepared update removes its temporary file and releases its lock without replacing the card. Cancellation after rename is dispatched cannot roll back the write. External editors do not honour package locks, so arbitrary concurrent edits cannot be guaranteed safe. There are no automatic retries or cooldowns.

Helper calls cost tokens even when they return `no_change`. These standalone calls are not included in Pi's usage totals: Pi exposes no public API for adding that usage. The local timeout bounds how long Work Cards waits and requests provider cancellation on a best-effort basis. It cannot guarantee that a broken upstream request stops.

## Configure the package

Configuration lives at `<PI_CODING_AGENT_DIR>/work-cards.json`, defaulting to `~/.pi/agent/work-cards.json`. This is separate from Pi's main `settings.json`. There are no project overrides. A missing file uses defaults; a partial file overrides only the fields supplied. Invalid settings fail with the configuration path.

| Setting | Default | Purpose |
| --- | --- | --- |
| `model` | `helpers/simple` | Helper provider and model |
| `thinking_level` | `null` | Inherit the helper profile; otherwise disable thinking for a physical model |
| `check_every_n_responses` | `1` | Overall-response check interval |
| `check_every_n_cycles` | `10` | Within-response cycle check interval |
| `max_context_chars` | `24000` | Full helper context limit in characters, not tokens |
| `max_document_bytes` | `262144` | Legacy preview setting; accepted but has no effect on external tools |
| `document_command` | `null` | Global command prefix; otherwise use `$EDITOR`, then `less` |
| `max_output_tokens` | `2048` | Helper output limit |
| `request_timeout_ms` | `30000` | Helper request timeout |
| `pane_poll_interval_ms` | `1000` | Card pane refresh interval |
| `herdr_timeout_ms` | `3000` | Herdr operation timeout |
| `pane_height_fraction` | `0.2` | Pane fraction, strictly between 0 and 1 |
| `update_guidance` | See default text below | Preserve stable facts and avoid speculative progress |

The default `update_guidance` is:

> Update only for significant, evidenced changes. Preserve stable facts. Do not infer progress, invent links or documents, or rewrite wording without a substantive change. Return no_change when nothing important changed.

`document_command` accepts `null` or a nonblank string without control characters. Examples are `vim`, `nano`, `less -R` or a quoted executable path with arguments. Malformed nonblank `$EDITOR` values fail visibly instead of falling back.

This command prefix is trusted global user configuration, executed by a POSIX shell. Do not copy untrusted card or model text into it. Card YAML and helper output cannot select an executable. The document's absolute filename is passed separately as one quoted positional argument, never inserted into shell source. The command runs in the canonical checkout on the server host. Pi's `$EDITOR` is forwarded explicitly to the card pane, including an empty value when unset, rather than inheriting an unexpected server value. A defined caller `$PATH` is forwarded to the card and document panes so editor lookup uses the same search path. No other caller environment or Herdr identity is forwarded. `max_document_bytes` does not cap files opened by the editor or pager.

Positive limits must remain positive; check intervals allow `0`. Override `thinking_level` with `off`, `minimal`, `low`, `medium`, `high`, `xhigh` or `max`.

The default model relies on an external helper-models extension and its `helper-models.json` configuration. Work Cards does not register `helpers/simple` or install that dependency. You can instead set `model` to a physical `provider/model` available to Pi.

There is no periodic PR network polling, Obsidian integration, note registry or central session catalogue.

## Develop and install locally

Use Node.js 22 or later. The package compiles TypeScript to JavaScript for the standalone pane and uses `yaml` at runtime. Pi and AI host packages are peer dependencies, not bundled copies.

From the package directory:

```sh
npm install
npm run build
npm test
```

`npm test` builds the package, then runs `tests/*.test.mjs` with Node's test runner. `npm run check` checks TypeScript without emitting files.

After `npm install` and `npm run build`, register the package locally on the server host:

```sh
pi install /absolute/path/to/work-cards
herdr plugin link /absolute/path/to/work-cards/herdr-plugin.toml
```

Explicit attachment also registers the plugin if its exact registration is missing. It requires both the `card` and `document` entrypoints and refuses conflicting or disabled registrations rather than replacing them. Existing users may need to run the explicit `herdr plugin link` command again: linked manifests are read at server startup, and hot reload is not assumed. Work Cards does not approve hooks. Herdr must have a running, compatible server version 0.9.3 or later; Work Cards does not restart or update it.

These are installation instructions, not a record of changes to normal user settings.

## Verification status

On macOS, `npm run check && npm test` passed on Node.js 26.11.1 and 22.23.3: 203 tests on each, with no failures or skips.

During initial packaging verification, installed Pi 1.2.0 loaded the extracted npm tarball on both Node versions in isolated, sandboxed RPC sessions. This verification was not rerun for the current refactored artifact. The `work-card` command and read-only `work_card` tool registered. Both runs exited successfully with empty stderr and no messages, tokens or model calls. That 28-file tarball included compiled entrypoints, manifest and docs, without `src`, tests or `node_modules`. The runs used supplied read-only host dependencies; they were not clean dependency-install tests.

The current isolated native suite passed all 21 checks on macOS arm64, Node.js 26.11.1 and Herdr 0.9.4 (protocol 22). It verified registration, card startup, OSC 8 output, exact same-workspace basename/title and focus, input coalescing, command precedence, literal filenames and canonical checkout cwd. Card polling continued; caller/card identities, processes and geometry were preserved.

The suite exercised `less` fallback, missing-command failure with Enter-to-close, and real Vim with Ctrl-C and `:q!`. All 5 document tabs closed normally, leaving the caller and card panes intact. Card-view `q`, detach/reopen and deleted-YAML cleanup safety also passed. The caller was a shell, not a live Pi process; this does not verify real Pi lifecycle behavior.

All 5 isolated workspaces were closed. The disposable server stopped and its PID was absent. On this Herdr 0.9.4 run, the linked document entrypoint was visible before explicit relink and remained valid afterward. Relink explicitly if registration is stale; this observation is not a universal hot-reload guarantee.

Current `npm pack --dry-run` succeeded: 35 files included all 7 required entries, including `bin/open-document.sh`. This verifies package contents, not a clean dependency install or loading the current artifact in Pi. Native and package results are recorded under `/tmp/wc-herdr-editor-fixed1-final-QRS9lz`.

Native Node.js 22, Linux, SSH, remote hosts, browser-click handling, real Pi lifecycle, live-model requests and visual sidebar rendering remain untested. Sidebar metadata tokens were verified, not the visual sidebar. Linux remains an untested design target. Verification did not activate Work Cards in normal Pi or Herdr settings or edit global settings.

See [the design document](docs/design.md) for the schema, shared APIs and file-write guarantees.
