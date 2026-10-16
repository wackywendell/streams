# Work Cards

Work Cards shows shared workspace context beside each Pi agent's own assignment. The package provides a Pi extension and a read-only Herdr terminal pane. It does not launch agents or manage checkouts.

## Workspace and agent records

A workspace record holds its title, shared Goal, registered checkout roots, documents, issues and pull requests. Each Pi session has an agent record with its own title, status, Now and Remaining. Several agents can share a workspace without overwriting each other's progress.

Records live under `$XDG_STATE_HOME/herdr/cards`, defaulting to `~/.local/state/herdr/cards`:

- `workspaces/<workspace-id>.yaml` contains shared context
- `agents/<session-hash>.yaml` contains the session identity, membership and own assignment

These are strict, versioned records. Unknown fields, duplicate YAML keys, invalid membership and unsafe paths fail visibly. Writes use locks, revision checks and atomic publication. Cooperating writers cannot silently overwrite stale revisions. External editors do not honour these locks.

Goal, Now and Remaining are separate fields, not an encoded prose string. Selected registered Markdown documents take precedence for Goal: purpose-bearing designs, ADRs, relevant README files and plans. With no suitable selection, meaningful projected dialogue and compaction may establish a shared intended capability and automatic workspace name. Sparse conversation leaves Goal and short Goal empty, omits the Goal section and preserves the existing name; there is no placeholder or missing-document warning. A model judges whether a purpose is notable, without a word or message-count threshold. Neither documents nor conversation establish progress or completion. Own summaries use the agent's dialogue and compaction checkpoint. Shared reference proposals also receive inline file-operation paths.

Documents form a small current reading list, not a history of useful files. Helpers should prefer a few complementary documents explaining the purpose, governing current work or providing an active plan. Usually 3 to 5 is guidance, not a quota or hard cap. Reading or editing a file does not qualify it. Explicitly requested pins remain possible. Usable documents are never automatically pruned for relevance or count. During refresh, checked filesystem absence authorises removal of missing document references, not deletion of files.

Issues must be published issues or tickets, not CI runs, logs or informal problems. PRs must be actively worked on or explicitly tracked for the session's assignment, not merely mentioned. These are helper admission rules, not proof of remote artifact type or status. Existing issue and PR references require explicit cleanup.

Document identities combine a checkout key and relative path. Absolute document paths, traversal and symlinks escaping the registered checkout are rejected. Issue and PR URLs must use HTTP or HTTPS. Descriptions should describe the whole artifact, not its latest edit. Plans and ADRs belong in the project; records only reference them.

Records are local host state, with no automatic backup or synchronisation. Keep secrets out of their text and references. The previous single-card format, session bindings and public `./card` API are unsupported. There is no runtime importer or compatibility launcher. The library exports are `work-cards/records` and `work-cards/config`.

## Commands and lifecycle

| Command | Purpose |
| --- | --- |
| `/work-card new <title>` | Attach to workspace context and set this agent's assignment title without replacing shared context |
| `/work-card open [workspace-id]` | Open or join verified workspace context, optionally selecting an existing durable workspace |
| `/work-card refresh` | Reconcile with the prior card and force plan selection without reopening a closed display |
| `/work-card reset` | Infer own summaries and reference additions afresh, clean up confirmed-missing documents, then recompute Goal; retain attachment |
| `/work-card naming` | Apply current workspace and eligible tab titles, then resume automatic tracking; preserve Pi manual names |
| `/work-card show` | Show the current workspace, own agent, resolved references and latest refresh diagnostics |
| `/work-card detach` | Pause updates durably and close only the owned display; retain records |
| `/work-card remove <substring>` | Remove a uniquely matching document path or URL from shared references |

To let the summarizer name a new assignment, use `/work-card open` without a title. Bootstrap titles are first-message excerpts or `Untitled task`; automatic Pi and agent-tab naming waits for helper publication or successful explicit acceptance through `/work-card naming`. An automatic `Untitled task` remains ineligible, without blocking shared workspace naming. `/work-card new <title>` sets an explicit manual record title instead. Opening an existing assignment preserves its title mode. Native labels retain separate manual protection; unknown workspace labels require explicit `/work-card naming` to establish automatic tracking. See [ADR 3](docs/automatic-naming.md).

Reset omits the existing own summary and shared workspace title, Goal and references from the dialogue helpers' inputs. It requires a complete, validated own proposal, including empty fields when unknown; partial updates and `no_change` are rejected. It never clears records before successful inference. Stages still publish separately: a later failure does not roll back an earlier validated update.

Reset preserves session and workspace identities, membership, checkout registrations, manual titles and removal exclusions. References have no manual-versus-generated provenance. Both receive the same checked missing-document cleanup; reset otherwise preserves them. New additions use the normal guarded merge. Reset cannot relabel existing links it did not supply to the helper. Other agents' progress is untouched.

Reset forces source selection and Goal inference, using selected documents first and projected dialogue plus compaction only when the selection is empty. It omits prior generated purpose and names from conversation-purpose inference. An insufficient proposal does not erase an established shared Goal or name, even on reset. Dialogue never replaces document-backed purpose. No repository crawling, raw logs or lost precompaction context are used; file-operation paths serve reference collection only. It requires an attached, unpaused session and never opens a display or joins a workspace. Use open to attach or resume first.

Explicit removal matches literal path or URL text, not descriptions. Ambiguous matches fail rather than opening a picker. It changes metadata, not source files, and records an exclusion against automatic re-addition.

Automatic missing-document cleanup runs after own-summary inference, before reference collection and Goal refresh, even without reference evidence. It creates no exclusion: a missing file is not a user rejection. New automatic documents must be readable regular files within a registered checkout. Renamed or consolidated replacements come from ordinary reference inference using projected evidence, not filename similarity or a repository scan. Stale history cannot restore a path while it remains missing.

Missing checkouts, changed roots, permissions, symlink components, nonregular entries and other I/O errors do not authorise cleanup. These unresolved references stay registered, with reason/count diagnostics. Automatic checks conservatively reject even contained symlinks; explicit document opening keeps its existing checked symlink behavior. Losing any selected document Goal source invalidates the whole document-derived Goal and provenance, never an unrelated conversation-backed Goal. If a source disappears during inference, guarded cleanup invalidates it and that pass fails; a later check regenerates Goal from surviving sources.

The default `workspace_startup: "automatic"` joins or creates context after meaningful task input. Repeated observations and reloads reuse verified membership. Use `"manual"` to require explicit joining. Existing attached summaries continue in manual mode. New/open require a Herdr-managed pane and target the caller, never GUI focus. Refresh can update an existing valid binding outside Herdr; it cannot guess a new native group.

A resumed session UUID finds its own record. A fork with a new UUID cannot claim its parent's assignment. Moves cancel old-generation requests and reset shared contribution boundaries. An unbound destination gets fresh shared context, not an implicit copy of the source. Selecting older durable context remains explicit when no reliable native binding survives.

Each display attempt and nonce is persisted before launch. Closing with `q` stops only that display, not summaries. Refresh, reload and automatic observation do not respawn a closed or uncertain display. Use explicit `open` to reopen it. A positively verified separated pair can be replaced after an agent-only move; an absent display is not permission to respawn.

Detach persists its paused state before configuration reads or native cleanup. If cleanup fails, inspect Herdr and retry detach. Use open to resume updates. Committed new-session and fork departures, or resume to a different session UUID, retire only the agent's owned display. Reload, quit and same-UUID resume retain it. Native ownership uses terminal identity, membership and nonces rather than labels or restored pane IDs.

Workspace records use one in-memory Goal value: unevaluated, evaluated unknown, or known with text and an optional short. Evaluated states carry a `provenance` union: `documents` with nonempty `sources` and `fingerprint`, or `conversation` with `sessionId` and a SHA-256 `digest` of captured projected dialogue and compaction. `evaluatedGoal(text, sources, fingerprint, short?)` constructs document-backed values; `conversationGoal(text, { sessionId, digest }, short?)` constructs conversation-backed values. Both copy and freeze provenance.

Version-1 YAML and public tool/show records keep flat `goal`, `goalShort`, `planSources` and `sourceFingerprint`. Conversation-backed results add only `conversationProvenance: { sessionId, digest }`, require empty `planSources` and null `sourceFingerprint`, and never persist transcripts. Document records omit the new field and retain existing validation and serialization. Reads and semantic no-ops do not rewrite old records. Evaluated unknown results also retain provenance, so unchanged insufficient evidence can be cached. Conversation caches distinguish originating sessions; evidence growth triggers later inference, while explicit refresh/reset bypass caches.

The `work_card` tool is read-only. It returns the current workspace and own agent records, source paths, resolved references and latest in-memory refresh diagnostics. It never creates, selects, joins or edits. Foreground management-tool CRUD remains a future slice. [ADR 3](docs/automatic-naming.md) defines the implemented workspace, tab and Pi naming policy. Automated checks pass; live naming rollout remains unverified.

## Terminal display and documents

A new display opens below its owning agent without changing focus. Once ownership is verified, Work Cards requests `pane_height_fraction` of the initial split for the display. Sizing is best effort and subject to Herdr's layout and minimum-size constraints. A resize failure retains the paired display without relaunching it. Reusing a display leaves its size unchanged; `0.5` skips resizing.

The pane border follows the workspace title. Inside, the shared Goal appears as muted, normal-weight text without a label. The agent title and `Remaining:` label are bold; their bodies use normal text. Blank lines separate these groups and the references only when spare rows permit, without displacing content or crowding the footer. The exact title `--clear` cannot be projected because Herdr reserves it as a flag; Work Cards reports the failure without clearing the caption or changing the record.

Status is active, waiting, blocked, parked or done. It describes the assignment, not whether Pi is running. Pi publishes shared purpose and own progress separately during membership polling and after successful refreshes. Closing the card with `q` does not stop these updates. Publishing requires verified agent ownership; an unmarked restored pane needs explicit open to establish it. The display process only projects its own card caption.

| Record field | Native token | Display-cell limit |
| --- | --- | --- |
| Workspace `goalShort` | `wcw_goal_short` | 25 |
| Agent `statusShort` | `wca_status_short` | 23, reserving 2 for a marker and space |

Helpers generate these fields alongside the full text, aiming for about 20 cells. They never derive them by truncation. Missing fields mean unknown. Full Goal, Now and Remaining stay in the card; existing `wcw_goal`, `wca_status` and `wca_now` tokens remain available. Agents do not publish their own progress as shared purpose. Herdr sidebar configuration must explicitly select the desired tokens.

Sections wrap at word boundaries, with long words split at grapheme boundaries. Clipped sections end with an ellipsis while retaining reference space where possible. The pane sanitises controls and measures Unicode display cells before adding trusted colours and OSC 8 links. A bottom-right `[q] close` hint overlays the last row; narrow panes show `[q]` or omit the hint.

Documents form a responsive grid, read left-to-right then top-to-bottom. Narrow panes use fewer columns. Filenames have normal weight, directories are dim and shortcut letters are cyan. Letters run from `a` to `z`, skipping `q`; only fully painted shortcuts are active. Additional documents have no key. Digits are inert. Issues and PRs stay on separate rows and share consecutive numbered HTTP link labels. Short artifact names preserve the full stored descriptions and exact URLs. Recognized repository URL paths supply the separate artifact number, such as `[1] PR #19: Go port`; other paths do not get invented numbers. Until a short name exists, the description remains visible. Use your terminal's link gesture to open them. `j` and `k` are document letters, not scrolling controls.

A document key rechecks the displayed identity, membership, checkout and native owner before any effect. It opens an exact document in the same workspace or focuses its verified existing editor tab. Canonical checkout and document identities govern reuse, not titles. Duplicate relative paths across checkouts receive distinguishing labels. Reuse preserves manual names and editor state. Stale or ambiguous identities fail visibly; possibly used editor tabs are never closed automatically.

The document tab runs `document_command`, otherwise `$EDITOR`, otherwise `less`. There is no `$VISUAL` preference. Command prefixes are trusted global configuration executed by a POSIX shell. The filename is passed separately as one quoted positional argument, not inserted into shell source. Only the required launch identity, explicit editor setting and defined caller PATH are forwarded, not arbitrary caller environment.

Document failures remain visible. Partial native failures report known tab identities for inspection and never automatically retry a possibly completed editor launch. Projection-lock contention does not discard a document key after fresh ownership verification. Native locks are never stolen.

## Helper updates

`check_every_n_responses` counts distinct changes in projected dialogue, compaction and allowed file-operation evidence. Other tool calls and raw event notifications do not count. Zero disables automatic checks, not explicit refresh or reset. Idle membership polling makes no helper calls.

Only one check runs at a time per session. New eligible input replaces pending input; busy requests coalesce, retaining the strongest pending intent: reset, then refresh, then automatic. Invalidated membership, session changes and cancellation prevent stale updates. Own-agent summaries and shared-reference collection receive the latest projected compaction summary and all available dialogue, without an application text cap. The reference collector judges relevance against the workspace's current title, Goal and known references. Attachment boundaries still guard continuity and fresh-input startup; they do not limit reference evidence. Explicit removal exclusions remain authoritative.

Reference evidence includes chronological `read`, `write` and `edit` operation names and paths, not tool bodies, outputs or arbitrary arguments. These are attempted operations, not proof of success or automatic reference admission. Supported codemode nested-call metadata contributes paths only when its parent call remains visible. Shell commands are not parsed. Own summaries remain dialogue-only. Conversation-purpose inference receives only projected dialogue and compaction, never file-operation paths, tool bodies/results or another agent's progress as purpose evidence. Editing or removing captured projected evidence invalidates an in-flight result; appending evidence does not.

Helpers have no executable tools. Own updates, shared proposals, source selection and Goal inference have separate validated protocols. Shared proposals can add references and label exact existing issue/PR URLs; they cannot replace lists, URLs or descriptions. Label updates skip references removed or described differently since the request. Helpers cannot change runtime identities, arbitrary checkout roots or source files. Selected Markdown documents are read through the checked checkout resolver, up to 256 KiB each. Oversized or unreadable selected sources fail visibly rather than becoming silently truncated plans. Sources are rechecked before publication.

Refresh diagnostics distinguish own updates, shared proposals, source selection and Goal inference, including failures, skipped Goal inference and successful selection with no sources. Explicit refresh and reset report ordinary completion when no purpose is established; real provider, I/O and validation failures remain visible. Diagnostics contain no dialogue or document contents and are not persisted; inspection makes no helper requests. Sidebar publication failures are reported separately from successful record updates.

Short fields have validated display-cell limits. Full-summary brevity, maturity assessment, source interpretation and semantic reference admission remain model behaviour, not validator guarantees. No-change decisions leave record bytes and modification times untouched. Provider context errors are visible, without silent input trimming or automatic retries.

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

Keep commits logically atomic and fold minor corrections into their owning change. Use Astra review for significant implementation or design changes, not every small edit. Product or authority changes require user agreement.

`npm test` builds with TypeScript and runs `tests/*.test.mjs`. `npm run build` refreshes compiled Herdr code. Build after changing the pane. Once changes are complete, `/reload` refreshes Pi's local TypeScript graph. Reload all participating Pi clients and reopen old card processes before generating newly added record fields; old strict readers cannot read unknown fields. A read alone does not rewrite existing records. Compiled native ESM extensions can retain stale helpers across same-process reloads; the declared Pi entrypoint remains `src/pi/index.ts`.

The plugin requires a compatible Herdr server version 0.9.3 or later. Explicit open registers a missing exact plugin registration, but does not replace a conflicting or disabled registration. Relink explicitly if the linked manifest lacks the card or document entrypoint. Work Cards never restarts or updates Herdr or approves hooks.

Run Pi, the plugin, repository and Herdr server on the same host. macOS local and Linux remote hosts are intended configurations; paths always refer to that host. Disconnecting a client retains server processes, but cold server restart does not guarantee arbitrary pane-process restoration.

## Verification status

The current runtime passes `npm run check` and all 772 tests on macOS with Node.js 22.23.3 and 26.12.0, without skips. The suite retains filesystem, concurrency, cancellation, ownership, document, terminal and packaging checks, including nested split sizing. Old-format-only expectations are retired. Package tests use Pi 1.2.0's real loader, verify same-process source reload and source/compiled plugin roots, and extract an actual npm tarball. Supplied read-only host dependencies are not a clean dependency-install test.

The latest isolated runtime run passed 7 checks on Herdr 0.9.5: attachment, exact document opening, close/refresh/reload suppression, explicit reopen, independent pairs, own-only cleanup and nested sizing. Both 20% and 75% card fractions preserved the outer split, unrelated pane geometry and focus. It used real card/editor processes with injected SDK callbacks and local fake helpers, not a complete live Pi session. Earlier native checks also verified multiple checkouts, editor reuse, manual labels and movement.

A separate isolated Herdr 0.9.5 display check verified the grouped body, three-column document grid and automatic card-caption updates, including leading-dash Unicode titles. Other labels and focus were unchanged. It used the real pane with a synthetic shell owner, no Pi SDK or helper calls, and did not launch documents. Its private server and captured fixture processes were cleaned up.

These checks made no changes to normal Herdr resources and used no paid model calls. The compact fields, reference rules, stage diagnostics and Pi-owned sidebar publishing pass deterministic tests; they have not yet had a live model/sidebar rollout. Conversation-backed Goal and [automatic naming](docs/automatic-naming.md) pass automated checks, including real SDK session persistence, resume and fork tests. Their live helper/native rollout remains unverified. Foreground management remains unfinished. Linux, SSH and remote operation, browser-click handling, visual sidebar rendering and complete live Pi summarisation remain unverified. Naming activation requires removing the old namer from discovery, fully reloading participating Pi sessions and ensuring old native work has quiesced. The old shutdown does not await dispatched native work; restarting the affected Pi process is the conservative option. This package does not uninstall, reload or restart other systems.

## Architecture decisions

- [ADR 1 Durable, evidence-grounded orientation](docs/design.md) explains the product purpose, local authority, helper evidence and tradeoffs, with the historical cost analysis
- [ADR 2 Shared workspace context and per-session agent state](docs/workspace-context.md) explains ownership, checkout identity, membership and the combined interface
- [ADR 3 Automatic naming and manual overrides](docs/automatic-naming.md) defines name sources, surface provenance, explicit native reclaim and rollout boundaries

The former implementation plan is retired. These ADRs record decisions and boundaries; commands, configuration and verification status remain here.
