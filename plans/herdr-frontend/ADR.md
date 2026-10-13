# Selectable streams frontends

Status: accepted design; Phase 4 implementation approved.

The [implementation plan](PLAN.md) defines task order and verification. This ADR records the design and its reasons.

## Context

Streams stores current work in scratch notes. Its existing workspace projection and checkpoint refresh assume CMUX. Herdr offers another terminal frontend, but neither frontend should own stream state.

The Herdr proof of concept has a tested links pane and a configured sidebar row. Sidebar metadata is plain text, not clickable links, and is not restored after a server restart. The pane is movable and tab-local; Herdr has no fixed docked-pane setting. A usable integration needs workspace opening, current-workspace updates, and checkpoint refresh rather than another manual display example.

## Decision

### Keep scratch authoritative

CMUX and Herdr are optional projections of scratch state. Keep frontend bindings, native resource IDs, and display metadata outside note frontmatter. Preserve CMUX alongside Herdr; selecting a different frontend does not migrate notes or live agents.

Use a fixed two-frontend dispatch. Share only cwd validation, conservative stream inference, and note URI generation. Keep CMUX Markdown formatting and status colours in its adapter. A registry or abstract adapter hierarchy would add machinery without another supported frontend.

### Use one configured selection policy

Store `frontend.mode = auto|cmux|herdr|none`, defaulting to `auto`. A named mode selects that frontend. `none` disables optional refresh.

One pure Python resolver owns this policy. Its CLI boundary loads configuration and snapshots caller context. Pi uses `streams frontend get --json`, not its own environment detection. Parse inputs at their boundaries, keep selection values immutable, and dispatch only a concrete frontend.

A missing request or a request of `auto` follows configuration. Only when the effective mode is `auto` does the resolver detect caller context. A named request overrides configuration. For example:

```json
{ "configured": "cmux", "requested": "auto", "resolved": "cmux" }
```

Recognise CMUX from a nonempty `CMUX_WORKSPACE_ID`. Herdr requires `HERDR_ENV=1` and a nonempty `HERDR_PANE_ID`. Do not infer a caller from executable availability, a socket path, or terminal-program names. Reject incomplete Herdr context. When both frontends are recognised, require a named choice rather than guess precedence.

With no recognised context, the query succeeds with `resolved: null`. Optional refresh skips; an explicit action needing a frontend fails before note mutation. With configured `none`, requests following configuration likewise resolve to null. Explicit named requests and command groups remain available.

Read `cmux.enabled` only as migration input. An explicitly stored `true` maps to `cmux`, `false` to `none`, and absence uses the new default. Return one canonical setting. Setting `frontend.mode` removes the legacy key atomically; reject conflicting stored selectors instead of choosing precedence.

### Make generic CLI selectors aliases

For generic start, rename, and current-stream resume, selecting a frontend also requests the current-workspace action:

| Selector                         | Meaning                                     |
| -------------------------------- | ------------------------------------------- |
| `-c` or `--current`              | follow configured selection                 |
| `-f auto` or `--frontend auto`   | the same choice as `--current`              |
| `-f cmux` or `--frontend cmux`   | explicitly use the caller's CMUX workspace  |
| `-f herdr` or `--frontend herdr` | explicitly use the caller's Herdr workspace |

Normalize these forms to one selector value, not independent current/frontend fields. `-f` always requires a value. Supply one selector form; reject combinations rather than apply precedence. Use ordinary argparse parsing, without custom argument tokenisation.

```sh
streams start "Title" -c
streams start "Title" --frontend auto     # same action as -c
streams start "Title" -f herdr            # no additional -c
streams rename old-slug "New title" -f herdr
streams resume -c                        # infer the current stream
streams resume stream-slug               # terminal-only, unchanged
```

Start and rename without a selector remain note-only. Resume without a stream reference requires a selector for inference. Reject a selector combined with an explicit resume reference; it must not be silently ignored.

Retain `streams cmux` and add `streams herdr`. Their groups already select the frontend, so configuration cannot redirect them. Inside either group, `--current` addresses that frontend's caller workspace; `--workspace` addresses an explicit workspace. Read-only resolver queries may use `--frontend` to inspect a named request without requesting a workspace mutation.

### Never target another client's focus

Current-workspace operations require caller context everywhere, including existing explicit CMUX commands. Remove CMUX's global focused-workspace fallback. Match CMUX caller identity against native IDs/refs only, never titles. Explicit human workspace selectors retain ID/ref/title matching with ambiguity checks. Outside a frontend, use an explicit `--workspace` instead.

Resolve Herdr current targeting through `herdr pane current --current`. Its live response accounts for pane movement; inherited workspace IDs can be stale. Parse native IDs from responses rather than derive them from labels or environment strings.

Herdr current-stream lookup first uses the explicit workspace association described below. Only an absent association permits conservative inference from exact workspace labels or a unique dedicated cwd. Do not infer from a shared main checkout or a repo-less directory. A broken association fails instead of falling back to hints; an explicit stream update can repair it. CMUX retains conservative label/cwd inference.

### Separate mandatory actions from optional refresh

Mandatory CLI projection validates selection and the target before creating or renaming a note. A later native failure reports partial success and the written slug/path; it must not repeat the note write.

One operation in Pi's CLI-integration layer, `StreamCli.refresh`, owns optional refresh. It resolves once after successful creation/binding or inside the approved note's post-save projection callback. It returns a discriminated result: skipped (disabled/no-context), updated (concrete frontend), selection-failed (warning), or backend-failed (concrete frontend and warning). Both callers and receipts consume this result rather than reconstructing it from configuration and parallel flags. Choosing an existing stream changes the Pi binding but does not immediately project.

Selection, ambiguity, missing-target and backend failures do not prevent, undo or repeat a valid save. The CLI still owns note validation and optimistic concurrency before writing. Minimal refresh status is model-visible; warnings and raw command diagnostics stay in expanded receipt details.

Add the typed resolver query before migrating its callers. Switch canonical configuration output, projection, both callers, receipts, and removal of the old API in one verified step. Changing configuration output first would break the existing Pi decoder and real-CLI contract tests. Distinguish skipped refresh, successful update, selection failure, and backend failure. Preserve private diagnostics and readable legacy receipts.

### Open a complete Herdr working tab

`streams herdr open <stream>` creates a workspace at recorded `cwd`, resumes the latest supported recorded session when available, and adds a small links pane in its working tab. Without a recorded session, keep a shell workspace. Automatic refresh never opens a workspace, launches an agent, or moves focus.

Preflight cwd, session arguments, native compatibility, and plugin identity before creating layout. Register only a missing bundled plugin during explicit open. Reuse a matching enabled registration; report conflicting sources or a disabled plugin without silently replacing or enabling it. The installed CLI exposes registration identity through `herdr plugin list --plugin ID --json`.

Use returned workspace, root-pane, and tab IDs. Keep links creation from stealing pane focus. On partial startup failure, report created resources and diagnostics rather than blindly repeat workspace creation or agent launch. Opening is not live-agent migration.

### Associate a Herdr workspace without adding a stream database

Native metadata is transient display state, not a durable workspace-to-stream association. Live Herdr 0.9.3 can expose reported tokens; the explicit association remains the identity authority rather than relying on sidebar metadata.

Use a small atomically replaced JSON file outside notes, namespaced by the actual socket/session from `herdr status --json` and native workspace ID. It records the explicit workspace-to-stream association, not a second copy of note contents. Both current-stream lookup and the links pane use it; the pane receives its path. The Pi session binding remains independent.

Resolve stored identity through exact slugs and safe rename symlinks to canonical current notes in the same scratch directory. The CLI exposes `show --exact` for this boundary. Do not normalize a stored reference into a title, fuzzy-match it, or follow external, archived, broken or cyclic targets. Malformed, unreadable or dangling associations, and missing/invalid/unsafe notes, fail without label/cwd fallback. Only a genuinely absent association permits conservative hints. Explicit stream updates bypass a broken association so the user can repair it. Rename aliases avoid a separate alias database.

After validating the stream and native workspace, publish the association atomically before deriving the native label and metadata. If publication fails, do not attempt native display updates. If a later display update fails, keep the new association and report partial/stale display, without rollback or retry. Native display is derived, not the authority for which stream the workspace uses. This is not a cross-system transaction, and no failure retries a successful note write.

Port the dotfiles example's tested behavior into the Python package, leaving the example untouched. `n` opens the current note in Obsidian; Ctrl-click remains for safe HTTP(S) issue and PR links. Read current vault/root settings when opening a note. A failed binding or note refresh must not leave a stale note available to open.

### Use the opening distribution's runtime

Herdr manifests contain static argv arrays and do not expand environment variables. A tiny POSIX launcher executes the packaged view through the opening CLI's Python interpreter. Supply the loaded package's import root too, rather than trust the server's Python import environment. The view calls the CLI as a module through that runtime, not an arbitrary `streams` executable on PATH.

Run the view from its packaged plugin directory so the manifest's static relative launcher path resolves; the root shell still uses recorded stream cwd. Pass the opening HOME and configuration directory to the view as well, so the server's inherited environment cannot redirect note lookup.

Keep Unix-only view imports out of generic CLI startup. Package the renderer, launcher, and manifest in the wheel. Verify runtime provenance in an isolated installation with misleading global CLI and import-path settings. Prefer `HERDR_BIN_PATH` for native calls when provided.

## Consequences and limits

The single selector removes flag combinations and bare-`-f` argument-order traps. `-f auto` deliberately follows a configured named mode; it is not a way to bypass configuration and force detection.

Caller-only targeting changes CMUX scripts that previously relied on global focus. They must provide an explicit workspace outside CMUX. This compatibility cost prevents automation from modifying another client's workspace.

Task actionability and agent lifecycle remain separate. Sidebar metadata shows plain status/state, while links live in the terminal pane. The pane remains movable and tab-local.

This prototype excludes checkout management, cross-frontend live-agent migration, locked or workspace-wide panes, bulk Herdr updates, and restart automation. jj continues to manage jj workspaces.

## Verification and activation boundaries

Run the locked Pi baseline before changing behavior; the existing Python baseline passed 128 tests. Use red/green tests, temporary notes/configuration, fake native responses, and PTY interaction tests. Isolate caller environment and mock registration, agent launch, and OS openers so tests cannot operate the live app.

Verify wheel contents and installed behavior. Live acceptance starts with a disposable shell-only stream. Resume an inactive recorded session only with approval, never this live Pi session. Browser and Obsidian activation need a human check.

Implementation started in `~/code/streams-herdr-frontend`, based on `c530c346`. The user later approved reconciliation and activation at existing `~/code/streams` CLI/Pi/skill paths. The main checkout's checkpoint visibility/spinner work was preserved and independently checked by its owner. The bundled plugin is registered and local policy is `auto`; existing interactive Pi sessions still require `/reload` before checkpoint use.

Before activation, reconcile relevant main-checkout changes, present exact package/configuration paths, pause checkpoint activity, and coordinate active Pi reloads. Update the shipped skill, README, FORMAT, and Pi specification together. Do not discard unrelated work or commit without approval.
