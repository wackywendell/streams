**Overall Progress:** `95%`

# Selectable streams frontends

## Objective and scope

Provide a usable Herdr frontend alongside CMUX. Scratch notes remain the source of truth. Automation uses configured selection; frontend-dependent CLI calls select explicitly.

Work in `~/code/streams-herdr-frontend`, the isolated `herdr-frontend` jj workspace based on `c530c346`. Leave the main checkout's unrelated checkpoint changes untouched.

Opening a Herdr stream creates a workspace at its recorded `cwd`, resumes its latest supported recorded agent session when available, and adds the working-tab links pane. Automatic refresh updates an existing workspace; it never opens a workspace or launches an agent.

No checkout management, cross-frontend agent migration, workspace-wide docking, pane locking, bulk Herdr updates, or restart automation in this prototype. Reuse the macOS links example; additional platform support is not part of this slice.

Accepted design and rationale: [ADR.md](ADR.md). Implementation, matched-source activation and bounded shell-only live acceptance are complete. Existing interactive Pi sessions still need `/reload` before checkpoint use. This plan records tasks and verification, not the discussion history.

## Selection contract

One pure Python resolver owns selection policy. The CLI wrapper loads canonical configuration and snapshots caller context, then returns an immutable selection value through `streams frontend get --json`. Parse settings and resolver JSON at their boundaries; backend dispatch accepts a concrete frontend, never a nullable or unvalidated string. Pi consumes the result and calls the resolved explicit frontend command; TypeScript does not inspect terminal environment variables or select a frontend itself.

Configuration key: `frontend.mode`, with values `auto`, `cmux`, `herdr`, and `none`. Default: `auto`. `none` retains the existing ability to disable automatic projection. Explicit frontend CLI calls are not disabled by automation configuration.

Read legacy `cmux.enabled` only as migration input: an explicitly configured `true` maps to `cmux`, and `false` maps to `none`. Do not apply the old default when the setting is absent. Return canonical configuration rather than two independent selection controls. Setting `frontend.mode` removes the legacy key atomically; reject conflicting stored selectors rather than silently apply precedence.

Resolver JSON shape:

```json
{"configured":"auto","requested":null,"resolved":"herdr"}
```

`requested` records the supplied selector, or `null`. A missing request or `auto` follows configuration; a named request overrides it. For example, configured `cmux` with requested `auto` resolves to `cmux`, not context detection. `resolved` is `cmux`, `herdr`, or `null`. Resolution does not probe server health or choose a workspace.

When the effective policy is `auto`, use caller context, not executable availability or another client's focused UI. Recognise CMUX by a nonempty `CMUX_WORKSPACE_ID`; recognise Herdr by `HERDR_ENV=1` together with a nonempty `HERDR_PANE_ID`. A bare socket path or terminal-program name is not caller context. Reject incomplete Herdr context and report ambiguity when both frontends are recognised, rather than guessing precedence. Named selection does not depend on auto detection; its current-workspace operation still requires that frontend's caller context.

No recognised frontend resolves to `null`: Pi skips optional refresh, while explicit frontend-dependent operations fail before note mutation. In Pi, selection runs inside the post-save projection callback. Ambiguity or query failure becomes a refresh warning after a valid save, not a reason to block or repeat the note write.

## CLI contract

Retain explicit `streams cmux` operations. Add `streams herdr open` and `streams herdr update`, with current-workspace and explicit-workspace targeting. Add the links-view entrypoint needed by the bundled plugin.

`--current` is caller-only everywhere, including existing explicit CMUX commands and any omitted target that defaults to current. Remove CMUX's globally focused-workspace fallback. Without caller context, require an explicit `--workspace`; a configured frontend override never authorizes substituting another client's focus.

Generic start/rename/resume use one selector. `-c` / `--current` are aliases for `-f auto` / `--frontend auto`: they request the current-workspace action using configured policy. `-f cmux` and `-f herdr` request that action in the named frontend without another flag. Normalize all forms to one parsed selector value, not separate current/frontend fields.

```sh
streams frontend get --json             # configured selection, resolved for this caller
streams frontend get -f auto --json     # the same policy, with requested=auto
streams start "Title" -c                # configured selection and current projection
streams start "Title" --frontend auto   # equivalent to -c
streams start "Title" -f herdr           # named current projection, no additional -c
streams rename old-slug "New title" -f herdr
streams resume -c                       # infer the stream from the configured frontend
streams resume stream-slug              # frontend-independent, unchanged
streams cmux update stream-slug -c
streams herdr update stream-slug -c
streams herdr update stream-slug -w workspace-id
streams herdr open stream-slug
```

Use ordinary argparse parsing; `-f` always requires a value. Supply one selector form and reject combinations instead of choosing precedence. Start/rename without a selector remain note-only. Resume without a stream reference requires a selector; an explicit reference rejects a selector. Inside named frontend groups, `--current` only targets that frontend and configuration cannot redirect it.

## Structure and style

Python implementation lives in `src/_streams/`; tests use `unittest` in `tests/python/`. Pi integration stays in `extensions/stream-checkpoint/`, with `node:test` coverage in `tests/stream_checkpoint_test.mjs`. Documentation lives in `README.md`, `src/_streams/FORMAT.md`, and `docs/pi-stream-checkpoint-spec.md`.

Use `frontend.py` for selection, `herdr.py` for the native CLI adapter, and a small neutral projection helper module only for functionality both frontends actually share. Keep the links renderer and plugin manifest in the Python package. Use a fixed two-frontend dispatch, not a registry, abstract class hierarchy, or new dependency.

Keep an explicit Herdr workspace-to-stream association in a small atomically replaced JSON file outside notes. Namespace it by the actual socket/session from `herdr status --json` and the returned workspace ID. Pass its path to the links pane; both the pane and current lookup use its exact stream identity, including safe same-scratch rename aliases. It does not duplicate note contents, and the Pi session binding is independent. Only absence permits conservative label/cwd inference; malformed, unreadable, dangling or otherwise broken associations fail without fallback. Explicit stream updates can repair them.

Publish the association before native label/metadata changes. Publication failure makes no native display changes. Later display failure keeps the new association and reports partial/stale display, without rollback or retry. Native display is derived from the association and authoritative scratch note.

Herdr manifest commands are static argv arrays, without environment expansion. Use a tiny POSIX launcher to execute `_streams.herdr_links` with the exact Python interpreter supplied by the opening CLI. Supply the opening distribution's import root so the server's Python environment cannot select a different `_streams`. The view invokes `_streams.cli` through that interpreter and import root, using a normal module entrypoint rather than an arbitrary `streams` executable on the server's PATH. Follow `HERDR_BIN_PATH` for native calls when provided. Verify runtime provenance from an isolated wheel environment.

Follow existing typed Python and TypeScript style. Existing session parsing provides the model:

```python
def latest_session_command(stream: Stream) -> AgentSessionCommand | None:
    if not stream.sessions:
        return None
    return command_from_session(stream.sessions[-1])
```

## Verification and boundaries

Configured verification commands:

```sh
uv run python -m unittest discover tests/python
npm ci
node --test tests/stream_checkpoint_test.mjs
uv build --wheel
```

The 128 baseline Python tests passed. The locked Pi baseline passed all 114 tests after `npm ci`. npm reported one high-severity finding in the unchanged dependency lock; no dependency upgrades or audit fixes were applied. The wheel build and archive checks passed; its links view also passed PTY/runtime-provenance checks from a disposable installed environment. No separate compiler or lint task is configured.

The resolver/query landed before the consumer migration. Task 8 switched canonical configuration output/setter and the Pi consumer in one verified cutover, without transitional output fields or compatibility wrappers.

Use pure selection tests, CLI tests with temporary notes/configuration, fake native command responses, and PTY tests for the links view. Clear or explicitly supply frontend context in tests and mock native execution, registration, agent launch, and OS openers. Inherited live Herdr variables must not turn tests into control operations. Finish with a bounded macOS Herdr smoke test.

For mandatory CLI projection, validate selection and caller targeting before creating or renaming notes. For Pi's optional refresh, use the existing post-save projection callback; the CLI remains authoritative for note validation and optimistic concurrency before the write. Preserve written notes if selection or projection then fails; report partial success with the written slug/path and never retry the note write. Metadata and binding-file updates are not a cross-system transaction. Keep stream actionability separate from agent lifecycle state. Parse native IDs from responses, and preserve focus during automatic refresh and links-pane creation.

Ask before global package activation, replacing active Pi package paths, or reconciling unrelated work. Never discard main-checkout edits, resume this live Pi session for a smoke test, close someone else's panes, or stop the Herdr server. No commits without approval.

Before cutover, the installed CLI was editable from `~/code/streams/src`; Pi's package entry and Stowed skill used that checkout too. Approved integration preserved these paths and reconciled the six pending checkpoint files instead of retargeting packages independently. `~/.config/streams/config.json` remains Stowed from `.dotfiles/macos/.config/streams/config.json`; its legacy `cmux.enabled: true` was replaced with canonical `frontend.mode: auto`, preserving note-root/vault semantics and the symlink. The missing `streams.links` was explicitly registered from `~/code/streams/src/_streams/herdr_plugin`. Checkpoint use remains paused in loaded old runtimes until their interactive `/reload`.

## Implementation tasks

- [x] 🟩 **Task 0: Establish the pinned baseline**
  - [x] 🟩 Recorded the existing 128-test Python baseline and installed locked Node dependencies with `npm ci` before changing behavior.
  - [x] 🟩 Verified: `node --test tests/stream_checkpoint_test.mjs` — 114 passed, none skipped.
  - [x] 🟩 Updated PLAN.md.

- [x] 🟩 **Task 1: Shared resolver and read-only query**
  - [x] 🟩 Added 17 tests for default `auto`, missing/auto requests following configured policy, named overrides, disabled projection, legacy-derived policy, absent/partial/mixed caller context, immutable results, no native probing, and resolver JSON. Confirmed RED before implementation.
  - [x] 🟩 Implemented typed modes/frontend names, the pure resolver, the permanent configuration-mode accessor, `streams frontend get`, and `_streams.cli` module execution. The existing config wire schema remains intact until Task 8.
  - [x] 🟩 Verified focused frontend/config suites (17 and 8 tests), then the full Python suite (145 passed) and Pi suite (114 passed, including real-CLI contracts).
  - [x] 🟩 Reviewed selection boundaries, immutable results, legacy absence/false distinction, and preservation of current consumers. No native control, global activation, or live configuration changes.
  - [x] 🟩 Updated PLAN.md.

- [x] 🟩 **Task 2: Share only frontend-neutral projection helpers**
  - [x] 🟩 Added 10 tests before extraction for cwd validation, raw URI encoding/root boundaries, exact-title precedence, duplicate titles/cwds, and refusal to infer from fuzzy titles or shared/repo-less directories. Confirmed RED before implementation.
  - [x] 🟩 Extracted `projection.py`; CMUX retains Markdown wrapping, item formatting, and status colours. Resume imports neutral cwd validation directly.
  - [x] 🟩 Verified: `uv run python -m unittest discover tests/python` — 155 passed, including existing CMUX formatting and resume tests.
  - [x] 🟩 Updated PLAN.md.

- [x] 🟩 **Task 3: Native Herdr targeting and updates**
  - [x] 🟩 Added 31 adapter/binding tests, RED before implementation, for native JSON/errors, binary/runtime context, explicit targeting, moved/stale callers, conservative inference, focus-preserving updates, endpoint namespacing, rebinding, and partial failures.
  - [x] 🟩 Added the bounded Herdr adapter and `streams herdr update`. Current targeting uses live `pane current --current`; removed CMUX's global-focus fallback. No layout, agent, or focus commands are issued by refresh.
  - [x] 🟩 Current lookup honours the explicit workspace association. Only absence permits exact-label or unique dedicated-cwd inference; broken associations fail without fallback. Explicit stream updates can repair associations, and explicit workspace updates remain usable outside Herdr.
  - [x] 🟩 Projected label/status/state separately from agent lifecycle. Publish endpoint/workspace-scoped associations before native display changes; publication failure performs no native mutation, while later display failure retains the new association and reports partial refresh without retries.
  - [x] 🟩 Testing exposed that rename creates aliases but the CLI resolver ignored them. Added exact same-scratch alias resolution and 7 tests for repeated renames, canonical CLI reads/writes, invalid/broken/cyclic/external/archive targets, and no fuzzy alias matching. No alias database added.
  - [x] 🟩 Verified focused Herdr suite (31 passed), full Python suite (193 passed), and Pi suite (114 passed). Updated the CLI fixture's root seam after adding alias lookup. Native discovery used read-only help/status/current/schema commands; no live mutation or registration.
  - [x] 🟩 Updated PLAN.md.

- [x] 🟩 **Task 4: Package the tested links view**
  - [x] 🟩 Ported the example behavior with 19 tests, RED before implementation, covering rendering/control filtering, HTTP(S) OSC8 links, configuration/CLI decoding, refresh/rebinding/renames, fresh note actions, stale-opener refusal, opener failure, and PTY interaction/terminal restoration.
  - [x] 🟩 Packaged `_streams.herdr_links`, a static `streams.links` manifest, and an interpreter/import-root-pinned POSIX launcher. Module CLI queries bypass global `streams`, unsafe cwd imports, inherited PYTHONPATH, and PYTHONHOME. Unix terminal imports remain lazy; generic CLI startup is tested separately.
  - [x] 🟩 Verified wheel asset contents and installed PTY behavior in `/tmp/streams-herdr-wheel.ghqVzv/venv`, with a misleading global CLI, cwd/package import root, and Python home. Offline dependency installation found no usable cached PyYAML wheel; normal installation into this disposable environment succeeded. No global installation or plugin registration. Dotfiles example unchanged.
  - [x] 🟩 Verified focused links tests, full Python suite (211 passed), and `uv build --wheel`. Corrected two macOS PTY harness issues: EOF drain spinning and loss of unread final bytes when the last slave handle closes.
  - [x] 🟩 Updated PLAN.md.

- [x] 🟩 **Task 5: Open the complete Herdr working workspace**
  - [x] 🟩 Added 16 open tests, RED before implementation, for shell/session opening, native argument handling, plugin ownership/disabled state, malformed IDs, focus preservation, and partial startup failures.
  - [x] 🟩 Added `streams herdr open`: preflight cwd/session arguments, macOS support, compatible Herdr >=0.9.3, and plugin identity; create at recorded cwd, use returned workspace/tab/root IDs, project metadata/binding, add the bottom links pane without focus, size it near 20%, and resume the latest supported agent when recorded.
  - [x] 🟩 Register only a missing bundled plugin; matching enabled registrations are reused. Conflicting/disabled registrations fail without replacement/enabling. No actual global registration has run; activation still requires confirmation.
  - [x] 🟩 Partial failures report known resources and the attempted agent name, without closing resources, retrying, or relaunching. Native startup has a 20-second readiness bound inside the client's 30-second subprocess bound.
  - [x] 🟩 Read Herdr's pinned v0.9.3 native source to verify resize ratio/focus and plugin cwd behavior. The view runs from its packaged plugin directory so static `launch.sh` argv resolves, while the root shell uses recorded stream cwd. Pass opening HOME/config directory alongside interpreter/import root/binding so a server's stale environment does not redirect configuration. Added a manifest-argv/native-cwd execution test.
  - [x] 🟩 Verified the full Python suite (227 passed), Pi suite (114 passed), rebuilt wheel/archive contents, and five packaged-view tests against the latest wheel in the disposable environment. Native open/control/agent paths remain fake-tested; live acceptance is still pending.
  - [x] 🟩 Updated PLAN.md.

- [x] 🟩 **Task 6: One-selector current-workspace shortcuts**
  - [x] 🟩 Added 18 tests for all four configured-policy aliases, named selectors without `-c`, missing/invalid `-f` values, conflicting/repeated forms, note-only defaults, dry runs, caller/target failures before writes, conservative resume inference, and explicit-reference resume rejecting selectors. Confirmed RED before implementation.
  - [x] 🟩 Generic start/rename/resume parse one optional typed selector. A fixed union pairs each preflighted workspace with its native client; selection and live caller targeting happen before note creation/rename. Note-only defaults and explicit-reference terminal resume stay frontend-independent. No global-focus fallback; Herdr stored identity resolves exactly or fails.
  - [x] 🟩 Projection failures retain created/renamed notes and rename aliases, report the actual slug/path and target, and do not retry. Dry-run plus projection is rejected without target lookup or writes. Named frontend groups retain independent targeting semantics.
  - [x] 🟩 Verified full Python suite (245 passed), including existing CMUX formatting/explicit-workspace coverage, plus start/resume help. Native execution was forbidden in the new suite; no live workspace/configuration changes.
  - [x] 🟩 Updated PLAN.md.

- [x] 🟩 **Task 7: Add the typed Pi resolver query**
  - [x] 🟩 Added three query tests, RED before implementation, for enum/null decoding, missing/invalid values, structured ambiguity, unavailable CLI, warnings, and bounded diagnostics. Added real-CLI query contracts inside/outside repositories; explicitly removed inherited CMUX/Herdr caller identity from those subprocesses ahead of default-auto activation.
  - [x] 🟩 Added `StreamCli.frontend()` and typed boundary parsing. Python still owns selection; Pi does not inspect terminal context or repeat policy logic. Existing configuration/project methods remain unchanged for Task 8's atomic consumer cutover; no compatibility shim added.
  - [x] 🟩 Updated the obsolete checkpoint-config migration hint to `streams config set frontend.mode`.
  - [x] 🟩 Verified full Pi suite (117 passed, none skipped), including real-CLI contracts. Read pinned Pi extension guidance. No activation, live configuration changes, or native control.
  - [x] 🟩 Updated PLAN.md.

- [x] 🟩 **Task 8: Switch canonical configuration and Pi atomically**
  - [x] 🟩 Confirmed RED for canonical `frontend.mode` default/migration tests, strict values, conflicting selectors, equal redundant inputs, atomic legacy removal, symlink preservation and failed-write rollback. Config suite: 16 passed.
  - [x] 🟩 Switched Python config output/setter and both Pi callers together. Removed the old Pi config-reading API/types; projection requires a concrete frontend. Real-CLI tests isolate inherited frontend context.
  - [x] 🟩 `StreamCli.refresh` owns query-once optional refresh after creation/binding or confirmed Apply save. Existing selection does not immediately project. Note validation, concurrency guards and uncertain-write confirmation remain separate; selection/native failure cannot roll back or repeat a successful save.
  - [x] 🟩 Both callers and receipts consume skipped/updated/selection-failed/backend-failed outcomes. Warnings and command diagnostics remain private in expanded receipts. Persisted legacy CMUX receipts remain readable without rewriting.
  - [x] 🟩 Structural-review follow-ups fixed CMUX caller ID/title collisions and exact stored links identity, with 14 regression tests. Added 13 association tests for authoritative lookup, absence-only inference, broken/unreadable/dangling files, rename aliases, publication-first ordering, native partial failure, current resume, and explicit repair.
  - [x] 🟩 Follow-up review reproduced frontend-validation coupling in note-root lookup. Separated JSON decoding from per-consumer validation: invalid optional policy no longer blocks note reads/saves; malformed JSON or invalid note roots still fail. Added RED/green Python and real-CLI Pi Apply regressions. Added a schema regression rejecting Herdr metadata in notes. Reviewer re-inspection confirmed the finding resolved; no remaining qualifying association/refresh findings.
  - [x] 🟩 Parent verified full Python suite (281 passed) and Pi suite (119 passed, none skipped), including real CLI integration. No activation, native control, commits or live configuration changes.
  - [x] 🟩 Updated PLAN.md.

- [ ] 🟥 **Task 9: Documentation, activation, and live acceptance**
  - [x] 🟩 Updated README, `streams format`, the shipped streams skill, Pi specification and ADR for canonical configuration, one-selector CLI examples, configured-policy aliases, caller-only targeting, exact identity, association ownership/publication ordering, shared optional refresh, private diagnostics and prototype limits.
  - [x] 🟩 Pre-activation verification passed full Python (281) and Pi (119) suites, rebuilt the latest wheel, checked bundled assets and shell invocation, and reinstalled only into the disposable `/tmp/streams-herdr-wheel.ghqVzv/venv`. Five installed links/provenance/PTY tests passed. Installed CLI checks verified canonical config/resolver JSON, exact rename aliases, fuzzy refusal, real association files and note reads with invalid policy; no native controls. CLI help and documentation contracts checked.
  - [x] 🟩 User approved activation. Froze/backed up the six main-checkout files, reconciled in isolation, preserved visibility/spinner changes and tests, and obtained their owner's preservation review. Integrated at existing `~/code/streams` editable CLI/Pi/skill paths without changing settings paths or creating intentional commits. Main passed 283 Python and 127 Pi tests.
  - [x] 🟩 Registered matching `streams.links` from `~/code/streams/src/_streams/herdr_plugin`; set canonical `frontend.mode: auto` through the Stow-preserving setter, retaining note-root/vault semantics. New Pi 1.2 RPC startup loaded both checkpoint commands without a model prompt or session persistence, with native caller identity removed.
  - [ ] 🟥 Existing interactive Pi sessions must run `/reload` before checkpoint use. Peers were notified and checkpoint activity paused; their UI reload remains a user action. Do not restart or resume recorded sessions.
  - [x] 🟩 Live shell-only smoke passed on Herdr server 0.9.3/client 0.9.4: recorded cwd, root-focus-preserving 80/20 links layout, actual `applyDraft` → refresh, sidebar status/state, visible issue/PR links, explicit rebind, rename-alias current lookup despite a misleading label, broken-association refusal and explicit repair. No recorded session or OS/browser opener ran.
  - [x] 🟩 Live opening exposed incorrect fake response assumptions. Added RED/green regressions and corrected rename to parse `workspace_info`/matching workspace ID, and metadata reporting to accept silent successful exit. The failed test workspace w5 was inspected and closed before a controlled retest; successful w6 was closed after acceptance. Original w1/t1/p1 focus/layout/session identity was restored and preserved.
  - [x] 🟩 Both working copies remain undescribed/uncommitted. Backup manifest, before-trees, settings copies and smoke evidence are under `/var/folders/sw/bffqcbxd0qqdl1ft5synkq3r0000gp/T/streams-herdr-cutover-cv3u7c8g`. Dotfiles changes are limited to the approved streams setting; its unrelated VS Code edit and the old Herdr example were preserved.
  - [x] 🟩 Final main verification passed 283 Python and 127 Pi tests (none skipped), rebuilt the wheel from main, reinstalled only in the disposable venv and passed all five installed package/provenance/PTY tests. Wheel bytes match all package sources/assets; 41 main/prototype implementation/planning files match. Final source hashes and smoke/reload status are appended to the backup manifest.
  - [x] 🟩 Updated PLAN.md with the verification results and pending activation boundaries.

## Completion criteria

Selection has one implementation shared by CLI and Pi. Generic CLI projection/inference requires one selector. `--current` and `--frontend auto` follow configured policy; named selectors act without a separate current flag. Current targeting never substitutes UI focus for caller context. Automation honors configuration, resolves fresh at each optional projection boundary, and skips when `auto` has no context. Valid notes remain saved when refresh selection or native updates fail. Both frontends retain explicit commands. Herdr opening provides the recorded cwd, supported session, and links view using the intended package runtime. Tests and package checks pass, and coordinated local activation does not discard unrelated checkpoint work.
