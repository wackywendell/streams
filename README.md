# Work Cards

Work Cards shows shared workspace context beside each Pi agent's own assignment. The package provides a Pi extension and a read-only Herdr terminal pane. It does not launch agents or manage checkouts.

## Workspace and agent records

A workspace record holds its title, plan-derived Goal, registered checkout roots, documents, issues and pull requests. Each Pi session has an agent record with its own title, status, Now and Remaining. Several agents can share a workspace without overwriting each other's progress.

Records live under `$XDG_STATE_HOME/herdr/cards`, defaulting to `~/.local/state/herdr/cards`:

- `workspaces/<workspace-id>.yaml` contains shared context
- `agents/<session-hash>.yaml` contains the session identity, membership and own assignment

These are strict, versioned records. Unknown fields, duplicate YAML keys, invalid membership and unsafe paths fail visibly. Writes use locks, revision checks and atomic publication. Cooperating writers cannot silently overwrite stale revisions. External editors do not honour these locks.

Goal, Now and Remaining are separate fields, not an encoded prose string. Goal comes from selected known Markdown plan documents. Plans establish intent, never proof of completed work. With no suitable known plan, Goal remains unknown. Own summaries use the agent's dialogue and compaction checkpoint. Shared reference proposals use only fresh dialogue from the current membership.

Document identities combine a checkout key and relative path. Absolute document paths, traversal and symlinks escaping the registered checkout are rejected. Issue and PR URLs must use HTTP or HTTPS. Descriptions should describe the whole artifact, not its latest edit. Plans and ADRs belong in the project; records only reference them.

Records are local host state, with no automatic backup or synchronisation. Keep secrets out of their text and references. The previous single-card format, session bindings and public `./card` API are unsupported. There is no runtime importer or compatibility launcher. The library exports are `work-cards/records` and `work-cards/config`.

## Commands and lifecycle

| Command | Purpose |
| --- | --- |
| `/work-card new <title>` | Attach to workspace context and set this agent's assignment title without replacing shared context |
| `/work-card open [workspace-id]` | Open or join verified workspace context, optionally selecting an existing durable workspace |
| `/work-card refresh` | Request a summary check and force plan selection without reopening a closed display |
| `/work-card show` | Show the current workspace, own agent and resolved references |
| `/work-card detach` | Pause updates durably and close only the owned display; retain records |
| `/work-card remove <substring>` | Remove a uniquely matching document path or URL from shared references |

Removal matches literal path or URL text, not descriptions. Ambiguous matches fail rather than opening a picker. Removal changes metadata, not source files, and records an exclusion against automatic re-addition.

The default `workspace_startup: "automatic"` joins or creates context after meaningful task input. Repeated observations and reloads reuse verified membership. Use `"manual"` to require explicit joining. Existing attached summaries continue in manual mode. New/open require a Herdr-managed pane and target the caller, never GUI focus. Refresh can update an existing valid binding outside Herdr; it cannot guess a new native group.

A resumed session UUID finds its own record. A fork with a new UUID cannot claim its parent's assignment. Moves cancel old-generation requests and reset shared contribution boundaries. An unbound destination gets fresh shared context, not an implicit copy of the source. Selecting older durable context remains explicit when no reliable native binding survives.

Each display attempt and nonce is persisted before launch. Closing with `q` stops only that display, not summaries. Refresh, reload and automatic observation do not respawn a closed or uncertain display. Use explicit `open` to reopen it. A positively verified separated pair can be replaced after an agent-only move; an absent display is not permission to respawn.

Detach persists its paused state before configuration reads or native cleanup. If cleanup fails, inspect Herdr and retry detach. Use open to resume updates. Committed new-session and fork departures, or resume to a different session UUID, retire only the agent's owned display. Reload, quit and same-UUID resume retain it. Native ownership uses terminal identity, membership and nonces rather than labels or restored pane IDs.

The `work_card` tool is read-only. It returns the current workspace and own agent records, source paths and resolved references. It never creates, selects, joins or edits. Foreground management-tool CRUD and complete automatic workspace/tab/Pi naming remain future slices.

## Terminal display and documents

A new display opens below its owning agent without changing focus. Once ownership is verified, Work Cards requests `pane_height_fraction` of the initial split for the display. Sizing is best effort and subject to Herdr's layout and minimum-size constraints. A resize failure retains the paired display without relaunching it. Reusing a display leaves its size unchanged; `0.5` skips resizing.

The pane border follows the workspace title. Inside, the shared Goal appears as muted, normal-weight text without a label. The agent title and `Remaining:` label are bold; their bodies use normal text. Blank lines separate these groups and the references only when spare rows permit, without displacing content or crowding the footer. The exact title `--clear` cannot be projected because Herdr reserves it as a flag; Work Cards reports the failure without clearing the caption or changing the record.

Status is active, waiting, blocked, parked or done. It describes the assignment, not whether Pi is running. Shared and own native metadata projections are separate; agents do not publish their own Now as the workspace summary. Projection currently runs in the display loop: closing the card stops native metadata updates, although Pi continues updating the durable records. Herdr sidebar configuration must explicitly select these metadata fields to display them.

Sections wrap at word boundaries, with long words split at grapheme boundaries. Clipped sections end with an ellipsis while retaining reference space where possible. The pane sanitises controls and measures Unicode display cells before adding trusted colours and OSC 8 links. A bottom-right `[q] close` hint overlays the last row; narrow panes show `[q]` or omit the hint.

Documents form a responsive grid, read left-to-right then top-to-bottom. Narrow panes use fewer columns. Filenames have normal weight, directories are dim and shortcut letters are cyan. Letters run from `a` to `z`, skipping `q`; only fully painted shortcuts are active. Additional documents have no key. Digits are inert. Issues and PRs stay on separate rows and share consecutive numbered HTTP link labels. Use your terminal's link gesture to open them. `j` and `k` are document letters, not scrolling controls.

A document key rechecks the displayed identity, membership, checkout and native owner before any effect. It opens an exact document in the same workspace or focuses its verified existing editor tab. Canonical checkout and document identities govern reuse, not titles. Duplicate relative paths across checkouts receive distinguishing labels. Reuse preserves manual names and editor state. Stale or ambiguous identities fail visibly; possibly used editor tabs are never closed automatically.

The document tab runs `document_command`, otherwise `$EDITOR`, otherwise `less`. There is no `$VISUAL` preference. Command prefixes are trusted global configuration executed by a POSIX shell. The filename is passed separately as one quoted positional argument, not inserted into shell source. Only the required launch identity, explicit editor setting and defined caller PATH are forwarded, not arbitrary caller environment.

Document failures remain visible. Partial native failures report known tab identities for inspection and never automatically retry a possibly completed editor launch. Projection-lock contention does not discard a document key after fresh ownership verification. Native locks are never stolen.

## Helper updates

`check_every_n_responses` counts distinct changes in normalised user and assistant text, including projected compaction changes. It does not count tool calls or raw event notifications. Zero disables automatic checks, not explicit refresh. Idle membership polling makes no helper calls.

Only one check runs at a time per session. New eligible input replaces pending input; busy refreshes coalesce. Invalidated membership, session changes and cancellation prevent stale updates. Own-agent input retains all allowed dialogue without an application text cap. Shared additions receive only the validated current-membership suffix, never inherited compaction history.

Helpers have no executable tools. Own updates, additive shared proposals, plan selection and goal inference have separate validated protocols. Helpers cannot change runtime identities, arbitrary checkout roots or source files. Selected Markdown plans are read through the checked checkout resolver, up to 256 KiB each. Oversized or unreadable selected sources fail visibly rather than becoming silently truncated plans. Sources are rechecked before publication.

Summary brevity, maturity assessment and plan interpretation remain model behaviour, not validator guarantees. No-change decisions leave record bytes and modification times untouched. Provider context errors are visible, without silent input trimming or automatic retries.

Calls cost tokens even when nothing changes. They are not included in Pi's usage totals. Timeout bounds local waiting and requests best-effort provider cancellation; it cannot guarantee an upstream request stops. An optional Pi-only request spinner is off by default.

## Configuration

Global configuration lives at `<PI_CODING_AGENT_DIR>/work-cards.json`, defaulting to `~/.pi/agent/work-cards.json`. There are no project overrides. Missing configuration uses defaults; partial configuration overlays them. Invalid or unknown fields fail with the file path.

| Setting | Default | Purpose |
| --- | --- | --- |
| `workspace_startup` | `automatic` | Automatic or manual joining |
| `model` | `helpers/simple` | Helper provider/model or registered alias |
| `thinking_level` | `null` | Inherit helper profile; physical models default to thinking off |
| `show_refresh_spinner` | `false` | Show activity only during helper requests |
| `check_every_n_responses` | `1` | Distinct projected-input interval; zero disables automatic checks |
| `document_command` | `null` | Trusted command prefix; otherwise EDITOR, then less |
| `max_output_tokens` | `2048` | Helper output limit |
| `request_timeout_ms` | `30000` | Local helper timeout |
| `pane_poll_interval_ms` | `1000` | Display refresh interval |
| `herdr_timeout_ms` | `3000` | Native operation and document lock-wait timeout |
| `pane_height_fraction` | `0.2` | Requested display share of a new pair's initial split; best effort within native layout constraints |

Thinking overrides are `off`, `minimal`, `low`, `medium`, `high`, `xhigh` or `max`. Numeric limits must be positive integers, except the check interval can be zero. Pane fraction must be strictly between zero and one. Removed legacy options are rejected rather than silently accepted.

`document_command` accepts null or a nonblank command without controls, such as `vim`, `nano -w` or `less -R`. Nonblank malformed EDITOR values fail visibly. Do not copy untrusted record or model text into executable configuration.

The default helper route requires a separately installed helper-models extension and its `helper-models.json`. Work Cards does not register that alias or install its dependency. A physical provider/model already available to Pi can be configured instead.

## Develop and install

Use Node.js 22 or later. Pi loads the source entrypoint through its TypeScript loader; Herdr runs compiled JavaScript.

```sh
npm install
npm run check
npm test
pi install /absolute/path/to/work-cards
herdr plugin link /absolute/path/to/work-cards/herdr-plugin.toml
```

`npm test` builds with TypeScript and runs `tests/*.test.mjs`. `npm run build` refreshes compiled Herdr code. Build after changing the pane. Once changes are complete, `/reload` refreshes Pi's local TypeScript graph. Compiled native ESM extensions can retain stale helpers across same-process reloads; the declared Pi entrypoint remains `src/pi/index.ts`.

The plugin requires a compatible Herdr server version 0.9.3 or later. Explicit open registers a missing exact plugin registration, but does not replace a conflicting or disabled registration. Relink explicitly if the linked manifest lacks the card or document entrypoint. Work Cards never restarts or updates Herdr or approves hooks.

Run Pi, the plugin, repository and Herdr server on the same host. macOS local and Linux remote hosts are intended configurations; paths always refer to that host. Disconnecting a client retains server processes, but cold server restart does not guarantee arbitrary pane-process restoration.

## Verification status

The current runtime passes `npm run check` and all 442 tests on macOS with Node.js 22.23.3 and 26.11.1, without skips. The suite retains filesystem, concurrency, cancellation, ownership, document, terminal and packaging checks, including nested split sizing. Old-format-only expectations are retired. Package tests use Pi 1.2.0's real loader, verify same-process source reload and source/compiled plugin roots, and extract an actual npm tarball. Supplied read-only host dependencies are not a clean dependency-install test.

The latest isolated runtime run passed 7 checks on Herdr 0.9.5: attachment, exact document opening, close/refresh/reload suppression, explicit reopen, independent pairs, own-only cleanup and nested sizing. Both 20% and 75% card fractions preserved the outer split, unrelated pane geometry and focus. It used real card/editor processes with injected SDK callbacks and local fake helpers, not a complete live Pi session. Earlier native checks also verified multiple checkouts, editor reuse, manual labels and movement.

A separate isolated Herdr 0.9.5 display check verified the grouped body, three-column document grid and automatic card-caption updates, including leading-dash Unicode titles. Other labels and focus were unchanged. It used the real pane with a synthetic shell owner, no Pi SDK or helper calls, and did not launch documents. Its private server and captured fixture processes were cleaned up.

These checks made no changes to normal Herdr resources and used no paid model calls. Full automatic naming and foreground management remain unfinished. Linux, SSH and remote operation, browser-click handling, visual sidebar rendering and complete live Pi summarisation remain unverified. The existing naming extension is not disabled by this package change.

See the approved [workspace contract](docs/workspace-context.md) and [implementation plan](docs/workspace-implementation.md) for scope and rollout boundaries.
