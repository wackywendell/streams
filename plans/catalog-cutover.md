# Durable catalog cutover

Temporary implementation plan. Remove after the CLI and integration cutover. The [catalog ADR](../docs/durable-work-catalog.md) remains the design authority; implementation contracts live beside the code in `src/_streams/catalog`.

## Current state

This checkout's CLI uses the durable catalog: explicit setup, discovery, field edits, lifecycle, session reports and native opening/resumption. Local-only and Git-backed coordinators handle saving, publication and read refresh. The CLI reports local-save and publication outcomes separately. Optional goals are represented as null.

`list` and `show` use read-only refreshed projections; mutations and effective authority use persisted local state. A record visible only in refreshed reads requires explicit `sync` before editing or opening. Reads never publish.

The Pi extension and bundled skill use catalog context, guarded reporting, guided field editing and initial enrollment. Both real Pi/CLI contract tests are restored with isolated catalogs.

On 18 October 2026, isolated Herdr/Pi acceptance passed with real Gemini Flash inference and local-only storage. It covered reporting controls, Alt+P edits and discussion, manual-field protection and workspace/session reuse. Session and effort completion remained independent; archive/restore preserved status. Live Git publication was not tested.

The matched CLI, extension and shared skill are now active from the canonical checkout on local `main`. The installed catalog is initialised at `~/.local/share/streams/catalog` with `sync: none`; old notes remain untouched. Fresh global Pi startup verified the durable commands and shared skill. Participating Pi conversations were reloaded for the initial catalog cutover; later integration changes require another reload. Published scratch-based packages remain unchanged and must not be mixed with this CLI.

The one-off migration on 18 October created or restored 20 catalog efforts, covering every active/waiting/blocked legacy stream and all eight open Herdr workspaces. It imported 28 verified conversations, including all 14 currently registered native Pi sessions, and preserved four existing helper-session records. Workspace bindings and `--current` lookups were verified without launching, focusing or renaming native resources. Effort statuses, old notes and saved Work Cards data were preserved. Historical progress is labelled as such; missing session files and ambiguous legacy memberships were not guessed. Backups, source evidence, receipts and the migration plan are host-local under `~/.local/state/streams/migrations/`.

Automatic Pi enrollment now checks the actual Herdr pane's native session identity before creating locks, claims or records. This prevents an in-process helper from silently claiming its parent's workspace. A live check accepted the registered conversation without new saves and rejected a real helper without catalog changes.

Work Cards' native side panes, naming and compact-description projections now read these records. Its old independent writer is disabled in the active Pi package configuration; its saved data is preserved. Catalog-backed Alt+P cards remain available alongside the native panes.

## Native UI continuation

The original Work Cards history through `e7d74ce2` is merged under `integrations/herdr-work-cards/` without squashing or rewriting the source commits. The import was verified byte-for-byte with executable modes, and its 864 tests and typecheck passed before adaptation. Its old Pi entrypoint and writer are not activated. Root `mise run test` now includes the imported regression suite.

The [native projection ADR](../docs/native-herdr-work-cards.md) preserves the per-session pane, renderer, navigation and naming policies while replacing catalog and membership authority. The Streams-backed projection, session-scoped checkout resolution, attested native effects and Pi lifecycle wiring are implemented and approved by Gemini Argon. The component suites pass 651 Python, 103 Pi and 1,049 native Herdr tests, with no skips. Document navigation uses the viewing Session's registered checkout and current working file; missing files remain errors without fallback or reference deletion.

Isolated native acceptance passed with exact Pi-session attestation, separate Alpha/Beta progress, document shortcuts opening each viewing checkout's current file, close/reopen suppression, and restoration of both unfinished conversations with a manual tab name preserved. The done Session and closed editor tabs were not restored. A fresh fixture with narratives longer than Herdr's metadata limit also passed card, document and restoration checks. The disposable workspaces were closed and their catalogs and evidence retained locally.

The matched runtime is active on local `main` at `da2eea43`. All 14 original Pi identities and eight workspaces were preserved, and each conversation has a verified Streams card with no unresolved document bindings. One idle Pi process was restarted with its exact saved conversation to replace cached compiled code; busy agents were not restarted. The verified legacy card was retired without deleting its saved records. Two old empty locks with no open holders were archived, not stolen. Live receipts and recovery evidence are under `~/.local/state/streams/cutovers/20261019T025531Z-native/`.

A and B are two tabs/jj workspaces inside one Herdr workspace. Keep the runtime's one-native-workspace membership rule unchanged. Scope checkout bindings and editor reuse to the exact viewing session; a native-workspace-wide map would conflate its agents' checkouts.

The Pi slash-command family includes `/streams`, `/streams report [instructions]`, `/streams settings`, `/streams attach`, `/streams pane` and `/streams pane close`, with subcommand completion. The model-callable `checkpoint` tool is unchanged. The command family and native adapter are active in the canonical checkout. Existing Pi lifetimes need `/reload` to pick up the commands. A lifetime that already imported an older compiled native adapter may retain it across reload; refresh only an idle process by resuming its exact saved conversation, preserving its UUID and history.

## Native recovery follow-up

The activated base above is unchanged. The recovery refactor through `efd43ee9` is implemented and Argon-approved. Combined `mise run test` passed 657 Python, 112 Pi and 1,085 native Herdr tests, with no skips. This verifies the checkout, not installation or fresh native acceptance.

The refactor scopes complete capture to the relevant workspace, repaints cached content immediately on resize, avoids effect locks for unchanged projections, isolates capture recovery, and cancels and drains native subprocess work. Workspace locks now retain a versioned inode and use kernel-released advisory ownership instead of unlink cleanup. The [native ADR](../docs/native-herdr-work-cards.md#separate-display-observation-and-effect-recovery) records the reasons and trade-offs.

Before activating this follow-up:

- verify the matched CLI, extension and compiled native package in a disposable workspace; check resize during a blocked refresh, incomplete capture preservation, and crash recovery
- coordinate the canonical-main and installation window through Work Organization Review; do not enable catalog synchronization or restart busy agents
- quiesce old native-effect clients before inspected legacy-lock recovery; never delete a new-format lock or infer ownership from file age
- preserve current catalog records, workspace bindings, native identities and parent conversation history

The reported in-place fork at `wH:p1` is a separate recovery-UX case. The new conversation `01a15481-ed9f-752e-afbb-cabd8e1e7dd7` must get its own contribution, not replace parent `01a13be3-6101-75b6-9a2e-6908f53c89bc`. Verify that the stale card cannot adopt the new owner. Useful failure messages and explicit display handover remain under discussion; do not count them as implemented or recommend attachment/reload as universal repair.

## Follow-on slices

- implement handover and reattachment coordination with confirmed authority separate from pending proposals
- add captured document inputs before enabling inference that independently reads documents; the Pi helper currently uses conversation evidence and guarded catalog records
- consider advisor-guided restoration beyond the implemented last-recorded agent-tab arrangement; closed editor tabs and exact pane geometry are not reconstructed
- add dedicated conflict-resolution and interrupted-save repair commands; current code detects and preserves these conditions rather than repairing them automatically
- complete the separately reviewed in-place local-Git adoption's signed rebase, combined verification and activation, preserving catalog records and host identity
- separately approve package publication and run live Git/multi-machine acceptance

Git backing and runtime publication integration are implemented in this checkout and covered by isolated tests. The installed catalog remains local-only. In-place adoption is implemented and reviewed in the separate sync checkout, but SSH signing blocks its rebase and activation. Native activation is complete; live Git/multi-machine acceptance remains separate. No remote or push is approved.

Start and attachment events check the shared M interval; explicit sync and shutdown bypass timing. Successful/no-change commands check once after all related saves. CLI commands are caller-driven; there is no background sync service.

No legacy-format compatibility or automatic migration is required. Existing streams can be migrated manually. Old scratch notes are reference-only, not a second writable catalog authority.
