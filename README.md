# Work Cards

Work Cards keeps a short, durable record of a piece of work beside your terminal. It is smaller than Streams: one package provides a Pi extension and a Herdr terminal plugin, without managing agents or checkouts.

The package provides explicit card attachment, background updates from Pi evidence, and a read-only Herdr pane. See [verification status](#verification-status) for tested runtimes and remaining checks.

## Keep the card small

A card records the current status, a short state summary, the checkout directory, and links to documents, issues and pull requests. State should help you return a month later. Use `Goal` for the project purpose, `Now` for its broad usable capability or stage, and `Remaining` for major unfinished scope or blockers. Aim for 20 to 30 words total, excluding labels. This is a soft prompt target, not an enforced length or quality guarantee. Keep relevant facts, not an activity log, commit IDs or test counts. Plans and architecture decision records belong in the actual project, such as `adrs/foo-bar.md`. The card links to them; it is not a scratch document or a central notes repository.

Cards live at `$XDG_STATE_HOME/herdr/cards/<slug>.yaml`, or `~/.local/state/herdr/cards/<slug>.yaml` by default. They survive workspace deletion. The initial title determines the stable filename slug. Later title edits do not rename it, and creating a colliding slug fails rather than overwriting a card.

For example:

```yaml
title: Improve queue recovery
status: active
state: "Goal: Make queue recovery reliable | Now: Recovery failure reproduced and retry options documented | Remaining: Choose an approach and implement the fix"
cwd: /home/developer/code/queue-service
documents:
  - path: adrs/queue-recovery.md
    description: Recovery options and decision
issues:
  - url: https://github.com/example/queue-service/issues/42
    description: Track recovery failures
prs: []
```

All 7 fields are required. Arrays may be empty. Status is `active`, `waiting`, `blocked`, `parked` or `done`. State remains a single-line string: `Goal: ... | Now: ... | Remaining: ...`. Quote it in YAML because the labels contain colons. State may be empty; displayed text and descriptions must be single-line.

Section recognition requires these exact labels in order, with nonempty content and no surrounding whitespace. The exact ` | ` separator and repeated field headers are reserved; other literal pipes remain content. Legacy prose, incomplete or ambiguous sections, and extra fields stay readable as prose. No card migration is required.

Document descriptions remain required in YAML. Keep them brief and describe the whole document's purpose, not its latest edit.

`cwd` is an absolute path, or a `~/...` path expanded on the execution host. Document paths are relative to that checkout. Absolute paths, traversal and symlinks escaping the checkout are rejected. Issue and PR URLs must use HTTP or HTTPS. Unknown fields and duplicate YAML keys fail validation.

Cards are per-host state. They are not automatically synced or backed up. Keep secrets out of cards and linked descriptions.

## Attach deliberately

Use these Pi commands:

| Command | Purpose |
| --- | --- |
| `/work-card new <title>` | Create a card for Pi's current directory and attach it in the caller's Herdr workspace |
| `/work-card open [slug]` | Reopen this session's bound card, or attach an existing card by slug, in the caller's Herdr workspace |
| `/work-card refresh` | Request a background summary check of this session's attached card without opening a pane |
| `/work-card show` | Show the attached card and its source path |
| `/work-card detach` | Stop updates, close the owned card pane and clear the binding without deleting the YAML |

`new` and `open` require a Herdr-managed pane. They target the caller's workspace, not the focused GUI workspace. Opening the same card reuses its verified pane. A different existing card blocks replacement; detach it before switching.

A failed `new` may leave a created YAML file even though attachment failed. Inspect Herdr, then recover with `/work-card open <slug>`.

Explicit attachment or detachment can clear a departed owner's workspace markers only after strict checks under the workspace lock. The complete card identity, nonce, pane and terminal tuple must match. Both the referenced pane and terminal must be absent, with no conflicting marked panes and an unambiguous live inventory. Cleanup clears only Work Cards' `wc_*` workspace tokens. Surviving or restored owners, mismatched cards and ambiguous markers are never reclaimed. There is no automatic recovery.

Pi records the binding in custom session entries. Those entries reach disk only after user or assistant activity; `--no-session` does not persist them. Resuming the same originating session restores a binding only if its YAML still exists and validates. Restore never reconstructs a missing card or opens a pane. Forks and cloned sessions require explicit `/work-card open <slug>`.

After closing the card pane or reloading Pi in the same session, run `/work-card open` to reopen its bound card. It reads fresh YAML from the saved slug and path, not Pi's current directory. An explicit slug reads from the configured card directory. Without a same-session binding, bare `open` asks you to use `/work-card open <slug>` or `/work-card new <title>`; it does not search or create cards. Successful `detach` forgets the binding, so reopening then requires a slug.

Successful `open` also requests a background summary check after saving the binding. Attachment does not wait for the helper. Failed or stale opens do not request checks. `new` does not start a check until textual evidence changes.

`refresh` accepts no slug or other arguments. It requires an enabled same-session binding and works outside Herdr because it only updates YAML. It bypasses automatic cadence and can retry unchanged input after `no_change` or failure, without cancelling a running check or changing the binding.

Detach records `updates: false` before loading configuration or cleaning up Herdr. Failed cleanup or invalid configuration retains this durable paused binding so you can inspect Herdr and retry. It stays paused after restore until explicit `open` succeeds, with or without a slug. A failed `open` preserves the binding and its update permission. Legacy entries without an updates flag enable updates; malformed flags leave them paused. `refresh` cannot resume a paused binding; use explicit `open`. The custom-entry disk-flush caveat above still applies.

The current binding can detach after its YAML is deleted, using the canonical existing parent directory and filename to prove identity. A missing parent or ambiguous identity still blocks cleanup.

The read-only `work_card` tool returns the attached card, source path and document references. It cannot create, select, update or detach cards. Work Cards never automatically selects or creates cards, launches agents, or creates and switches checkouts. The helper cannot edit plans or ADRs.

## Run on the server host

The Herdr server, repository, Pi and plugin run together: either locally on macOS or remotely on Linux. A Mac client can attach to the remote Herdr server. Paths and card storage refer to the server host, not the attaching Mac.

The pane reads YAML as the source of truth and can display a card without Pi running. Herdr tokens hold ownership markers and display metadata, not durable card data. The `$wc_summary` token uses the status and `Now` for recognized sections, or the full state for legacy or malformed text, capped at 80 code points. The integration does not edit Herdr's internal `session.json`.

The borderless pane uses a bold title and, when width permits, a right-aligned status symbol and word: cyan `● active`, yellow `○ waiting`, red `! blocked`, dim `○ parked`, or green `✓ done`. Status describes the work, not whether Pi is running. Recognized state shows `Goal`, `Now` and `Remaining` on separate rows without the carrier pipes. Each section wraps independently at word boundaries in normal contrast; long words break at grapheme boundaries. Height-limited sections end with `…`, reserving a row for each later section when the state budget allows all 3. Smaller budgets show sections in order. Legacy prose keeps its existing wrapping and truncation. Reference space is retained where possible. Hidden references retain a compact count. Empty state and empty reference groups are omitted. A dim `[q] close` hint sits at the bottom-right without reserving a row, covering trailing content if necessary. Widths from 3 to 8 cells show `[q]`; narrower panes omit the hint. `q` still exits at every width.

Document letters and HTTP labels use the terminal accent colour. Directory prefixes are dim and filenames bold. The pane measures display cells, not ANSI bytes. Display text converts lone UTF-16 surrogates to replacement characters before measuring, then removes untrusted controls before adding trusted styling and links. Saved card values and document targets are unchanged. It shows each document's repository-relative path, not its description: `[a] docs/design.md`. Document shortcuts use single letters from `a` to `z`, skipping `q`; later documents have no shortcut. Only fully visible shortcuts are active. Digits do not open documents.

Issues and PRs share consecutive numbered labels, with issues first: `[1] Issue: …`, `[2] PR: …`. Numbering is independent of document letters and continues beyond 9. These HTTP labels are Ctrl-click-only, using OSC 8 terminal links and Herdr's local-client handling. Number-key opening is deferred; digits do nothing.

Press a displayed document letter to focus its still-open Work Cards tab in the same workspace, or open a new focused tab named after the displayed path's filename. Reuse matches the canonical checkout and document, not tab titles. It survives closing and reopening the card pane, preserves renamed tabs and unsaved work, and ignores changed editor settings. Tabs created before identity markers were added are not adopted or closed. Incomplete, mismatched or duplicate matching records fail visibly instead of guessing. `j` and `k` are document letters, not scrolling controls. The card display and its updater stay alive. Documents must still be listed on the freshly read card and resolve to regular files inside its checkout. No document action runs macOS `open` or remote `xdg-open`.

The tab runs `document_command`, then `$EDITOR` if the setting is `null`, then `less` if `$EDITOR` is unset or blank. There is no `$VISUAL` preference. An editor can modify the document; the card display and helper do not. Command failures stay visible in the document terminal with a configuration hint and a press-Enter-to-close prompt. Partial tab-opening failures appear on the card with returned pane, terminal and tab IDs when known; inspect that tab before retrying. Work Cards never closes a possibly used document tab.

In the main card view, `q` exits the display; it does not detach the Pi session or stop its updates. Run `/work-card open` in the bound session to recreate a departed display, or explicit `detach` to clear its validated orphan markers. Neither action reclaims a surviving or restored owner.

The card pane does not require sidebar configuration. An optional `$wc_summary` sidebar row requires a separate Herdr configuration change.

Exiting Pi leaves the card pane running independently. Disconnecting the client retains server processes. A cold server restart does not automatically preserve arbitrary pane processes. Run `/work-card open` in the bound session, or `/work-card open <slug>` otherwise, to reopen the pane; resuming Pi alone never opens it.

## Update from Pi evidence

`check_every_n_responses` counts distinct changes to the normalized summary input, including both new user text and completed assistant text; default `1`. Setting it to `0` disables automatic checks, not explicit `refresh` or successful `open` checks. The former `check_every_n_cycles` setting has been removed and is rejected as an unknown configuration field.

Pi observes its persisted projection at `context`, before the main request, and at completed `turn_end`, after the assistant and its tool batch are persisted. `turn_start` is too early for user persistence; `message_end` precedes persistence and later handlers can replace text. Work Cards neither copies raw event text nor transforms the main request. Settlement is a deduplicated fallback for later projection changes.

Revisions hash only normalized compaction summary and dialogue. The same input at multiple hooks does not advance the interval or repeat a check. Tool-only, thinking-only and blank-text activity cannot create new input. Compaction or projected redaction changes can qualify; empty initial input does not trigger automation.

Only one check runs at a time per Pi session, not globally across intentionally separate bindings. While it runs, newer eligible input replaces the pending input. Repeated busy refresh requests coalesce into at most one followup, retaining explicit retry intent with the latest eligible input. Ordinary dialogue does not cancel the active check or require an idle session before applying an update. Invalidations cancel queued work and stale requests.

The helper-request spinner is off by default. Set `show_refresh_spinner` to `true` to show an animated `<spinner> work-card` status in Pi's terminal UI during actual helper requests. Queued requests and waits for retired checks show no activity. Completion, errors, timeout and invalidations clear it. Disabling the option clears any live spinner. Disabled, RPC and headless modes create no animation timer. This Pi-only status does not replace Pi's main working indicator, rename tabs, or add YAML activity fields.

Each check uses Pi's current session projection: the latest compaction summary (or `null`) and all retained nonblank user and assistant text, in chronological order. It respects projected redactions and never reads raw precompaction history. Thinking, tool calls, arguments, results, images, custom/system messages and branch summaries are excluded. Summaries therefore rely on assistant text for outcomes, not tool output.

The helper receives one JSON message ordered `{compactionSummary, transcript, card}`, with the full current card last. There is no application context cap, clipping or fallback summary. Provider context-window errors appear through existing warnings, without silent trimming or automatic retry. Card-last ordering may allow prefix reuse when only the card changes; it does not guarantee provider caching.

The helper may condense verbose or activity-log-like state, or convert legacy prose to the short section format, without new progress. It must preserve supported facts, not invent a goal or completion when unknown. Once state is correctly formatted and useful, ordinary tool activity, passing tests and commits should return `no_change` unless the overall state changes. The brief asks for project-level orientation across agents, reconciles older blockers with explicit successful outcomes, and avoids treating unrelated working-copy hygiene as required work without relevance or blocking evidence. These are prompt instructions, not enforced summary-quality guarantees. Additional guidance cannot prevent legacy-state repair or override truthfulness and security restrictions.

The helper returns either `no_change` or a validated patch to `status`, `state`, `documents`, `issues` and `prs`. It cannot change `title`, `cwd`, or repository document contents. Checks reject stale results after card edits, binding changes or session changes.

`no_change` writes nothing, including no modification-time change. Updates use revision checks and atomic replacement. Cancellation is checked synchronously at the commit boundary, immediately before rename. An aborted prepared update removes its temporary file and releases its lock without replacing the card. Cancellation after rename is dispatched cannot roll back the write. External editors do not honour package locks, so arbitrary concurrent edits cannot be guaranteed safe. There are no automatic retries or cooldowns.

Helper calls cost tokens even when they return `no_change`. These standalone calls are not included in Pi's usage totals: Pi exposes no public API for adding that usage. The local timeout bounds how long Work Cards waits and requests provider cancellation on a best-effort basis. It cannot guarantee that a broken upstream request stops.

## Configure the package

Configuration lives at `<PI_CODING_AGENT_DIR>/work-cards.json`, defaulting to `~/.pi/agent/work-cards.json`. This is separate from Pi's main `settings.json`. There are no project overrides. A missing file uses defaults; a partial file overrides only the fields supplied. Invalid settings fail with the configuration path.

| Setting | Default | Purpose |
| --- | --- | --- |
| `model` | `helpers/simple` | Helper provider and model |
| `thinking_level` | `null` | Inherit the helper profile; otherwise disable thinking for a physical model |
| `show_refresh_spinner` | `false` | Opt into the Pi terminal helper-request spinner; accepts only `true` or `false` |
| `check_every_n_responses` | `1` | Distinct normalized-input change interval, including user and assistant text; `0` disables automatic checks |
| `max_context_chars` | `24000` | Deprecated; accepted and validated as a positive integer, but ignored |
| `max_document_bytes` | `262144` | Legacy preview setting; accepted but has no effect on external tools |
| `document_command` | `null` | Global command prefix; otherwise use `$EDITOR`, then `less` |
| `max_output_tokens` | `2048` | Helper output limit |
| `request_timeout_ms` | `30000` | Helper request timeout |
| `pane_poll_interval_ms` | `1000` | Card pane refresh interval |
| `herdr_timeout_ms` | `3000` | Herdr operation timeout |
| `pane_height_fraction` | `0.2` | Pane fraction, strictly between 0 and 1 |
| `update_guidance` | See default text below | Preserve stable facts and avoid speculative progress |

The default `update_guidance` is:

> Preserve stable facts and summarize durable milestones and important unfinished work. Condense verbose or activity-log-like state even without new progress. Otherwise update only for significant, evidenced changes; return no_change for routine activity. Do not infer progress or invent plans, links or documents.

`document_command` accepts `null` or a nonblank string without control characters. Examples are `vim`, `nano`, `less -R` or a quoted executable path with arguments. Malformed nonblank `$EDITOR` values fail visibly instead of falling back.

This command prefix is trusted global user configuration, executed by a POSIX shell. Do not copy untrusted card or model text into it. Card YAML and helper output cannot select an executable. The document's absolute filename is passed separately as one quoted positional argument, never inserted into shell source. The command runs in the canonical checkout on the server host. Pi's `$EDITOR` is forwarded explicitly to the card pane, including an empty value when unset, rather than inheriting an unexpected server value. A defined caller `$PATH` is forwarded to the card and document panes so editor lookup uses the same search path. No other caller environment or Herdr identity is forwarded. `max_document_bytes` does not cap files opened by the editor or pager.

Positive limits must remain positive; check intervals allow `0`. Override `thinking_level` with `off`, `minimal`, `low`, `medium`, `high`, `xhigh` or `max`.

The default model relies on an external helper-models extension and its `helper-models.json` configuration. Work Cards does not register `helpers/simple` or install that dependency. You can instead set `model` to a physical `provider/model` available to Pi.

There is no periodic PR network polling, Obsidian integration, note registry or central session catalogue.

## Develop and install locally

Use Node.js 22 or later. Pi loads `src/pi/index.ts` through its TypeScript extension loader. The package also includes compiled JavaScript for the standalone Herdr pane and library exports, and uses `yaml` and `string-width` at runtime. Pi and AI host packages are peer dependencies, not bundled copies.

From the package directory:

```sh
npm install
npm run build
npm test
```

`npm test` builds the package, then runs `tests/*.test.mjs` with Node's test runner. `npm run check` checks TypeScript without emitting files.

After changing Work Cards' Pi source, use `/reload` to refresh the extension and its local TypeScript helpers. Pi 1.2.0's unbundled Node loader retains native ESM imports of a compiled `.js` extension, even after clearing its extension cache. Loading the source graph avoids that stale-helper path. A Pi process that already loaded the former compiled entrypoint needs `/reload` after updating this package's manifest. Herdr still runs `dist/herdr/pane.js`; build after changing its source. This packaging change does not require relinking a correct, enabled Herdr registration.

After `npm install` and `npm run build`, register the package locally on the server host:

```sh
pi install /absolute/path/to/work-cards
herdr plugin link /absolute/path/to/work-cards/herdr-plugin.toml
```

Explicit attachment also registers the plugin if its exact registration is missing. It requires both the `card` and `document` entrypoints and refuses conflicting or disabled registrations rather than replacing them. Existing users may need to run the explicit `herdr plugin link` command again: linked manifests are read at server startup, and hot reload is not assumed. Work Cards does not approve hooks. Herdr must have a running, compatible server version 0.9.3 or later; Work Cards does not restart or update it.

These are installation instructions, not a record of changes to normal user settings.

## Verification status

For short-section rendering, `npm run check && npm test` passed on macOS with Node.js 26.11.1 and 22.23.3: 270 tests on each, no failures or skips. Added coverage includes strict section recognition, prose fallback, independent wrapping, sidebar selection and the packaged module. Offline renders cover 45 and 80 columns at 8 and 12 rows; these are not live UI or model-quality checks.

For the refresh and dialogue-cadence change, `npm run check && npm test` passed on macOS with Node.js 26.11.1 and 22.23.3: 258 tests on each, no failures or skips. Regressions cover manual retry at automatic interval zero, coalescing, nonblocking open, paused bindings, projected-input deduplication, request-only status and cleanup. An in-memory Pi 1.2.0 SDK test uses real message persistence, later `message_end` replacement, request-time context dispatch and projection, without provider calls. This is not a live Pi/Herdr or visual spinner check.

Before this change, macOS verification passed `npm run check && npm test` on Node.js 26.11.1 and 22.23.3: 245 tests on each, with no failures or skips.

Current quiet-layout checks cover wide and narrow text snapshots, all status styles, wrapping, grapheme/cell widths, control sanitization, closed SGR/OSC 8 sequences, tiny panes, notice-row reservation and fresh document-key validation. Footer regressions cover last-row header, state, document, HTTP link, overflow and notice content, including unavailable cards and covered shortcuts. Client regressions cover verified metadata, focus-only reuse across card nonces, canonical aliases, distinct identities, unmarked tabs, record conflicts and partial failures. Escaped YAML surrogate regressions check title, state, path and notice widths after UTF-8 encoding, while retaining original document targets and paired emoji. Prompt tests verify the summary contract and legacy-repair instructions, not live-model quality. The full native Herdr suite was not rerun for these followups. The user confirmed wrapping, the bottom-right close hint and manual refresh; the short-section layout still needs a live check.

A targeted isolated Herdr smoke passed 5 checks on both Node.js 26.11.1 and 22.23.3. It verified the visible footer, metadata round-trip, focus-only reuse preserving a renamed tab, reuse after card reopen, and fresh launch after document close. Evidence and terminal previews are under `/tmp/wc-followups-native-sVSPE7` (Node 26) and `/tmp/wc-followups-native-tLhbWy` (Node 22). Both workspaces closed and servers stopped; normal Herdr and user settings were untouched. These checks do not cover the full native suite or live Pi lifecycle.

During initial packaging verification, installed Pi 1.2.0 loaded the extracted npm tarball on both Node versions in isolated, sandboxed RPC sessions. Those RPC sessions were not rerun for the current source-entrypoint artifact. The `work-card` command and read-only `work_card` tool registered. Both runs exited successfully with empty stderr and no messages, tokens or model calls. That 28-file tarball included compiled entrypoints, manifest and docs, without `src`, tests or `node_modules`. The runs used supplied read-only host dependencies; they were not clean dependency-install tests.

The historical isolated native suite passed all 21 checks on macOS arm64, Node.js 26.11.1 and Herdr 0.9.4 (protocol 22). It verified registration, card startup, OSC 8 output, exact same-workspace basename/title and focus, input coalescing, command precedence, literal filenames and canonical checkout cwd. Card polling continued; caller/card identities, processes and geometry were preserved.

The suite exercised `less` fallback, missing-command failure with Enter-to-close, and real Vim with Ctrl-C and `:q!`. All 5 document tabs closed normally, leaving the caller and card panes intact. Card-view `q`, detach/reopen and deleted-YAML cleanup safety also passed. The caller was a shell, not a live Pi process; this does not verify real Pi lifecycle behavior.

All 5 isolated workspaces were closed. The disposable server stopped and its PID was absent. On this Herdr 0.9.4 run, the linked document entrypoint was visible before explicit relink and remained valid afterward. Relink explicitly if registration is stale; this observation is not a universal hot-reload guarantee.

Current loader regressions use Pi 1.2.0's actual loader, not a mock. On both Node versions, compiled ESM retained helper `v1` after same-process reload; a cold process saw `v2`. TypeScript with `.js` import specifiers refreshed to `v2`. A copied package's declared entrypoint also refreshed its private Herdr helper, with source and compiled clients resolving the same plugin root.

Actual `npm pack` and extraction passed on both Node versions: 45 files include all 10 TypeScript sources, compiled Herdr runtime, library exports and `bin/open-document.sh`. Pi's real loader registered `work-card` and `work_card` from the extracted `src/pi/index.ts`. These checks use supplied read-only host dependencies, not a clean dependency install, and do not start Herdr or make model calls.

Before the source-entrypoint change, `npm pack --dry-run` verified 35 files with all 7 then-required entries. That package check and the historical native results are recorded under `/tmp/wc-herdr-editor-fixed1-final-QRS9lz`. The native suite was not rerun for the source-entrypoint or quiet-layout changes.

An isolated comparison used 4 real replay calls at 2 historical snapshots. The compaction-plus-dialogue candidate produced useful outputs; this is not a broad benchmark or live Pi lifecycle verification. Current input regressions cover uncapped text, excluded tool data, card-last JSON, summary-only revisions and an actual SDK historical projection with compaction and redactions.

The full native suite has not run on Node.js 22. Linux, SSH, remote hosts, browser-click handling, real Pi lifecycle, live Pi summarization and visual sidebar rendering remain untested. Sidebar metadata tokens were verified, not the visual sidebar. Linux remains an untested design target. Verification did not activate Work Cards in normal Pi or Herdr settings or edit global settings.

See [the design document](docs/design.md) for the architecture decisions, principles and cost analysis.
