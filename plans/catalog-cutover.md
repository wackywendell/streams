# Durable catalog cutover

Temporary implementation plan. Remove after the CLI and integration cutover. The [catalog ADR](../docs/durable-work-catalog.md) remains the design authority; implementation contracts live beside the code in `src/_streams/catalog`.

## Current state

This checkout's CLI uses the durable catalog: explicit setup, discovery, field edits, lifecycle, session reports and native opening/resumption. Local-only and Git-backed coordinators handle saving, publication and read refresh. The CLI reports local-save and publication outcomes separately. Optional goals are represented as null.

`list` and `show` use read-only refreshed projections; mutations and effective authority use persisted local state. A record visible only in refreshed reads requires explicit `sync` before editing or opening. Reads never publish.

The Pi extension and bundled skill use catalog context, guarded reporting, guided field editing and initial enrollment. Both real Pi/CLI contract tests are restored with isolated catalogs.

On 18 October 2026, isolated Herdr/Pi acceptance passed with real Gemini Flash inference and local-only storage. It covered reporting controls, Alt+P edits and discussion, manual-field protection and workspace/session reuse. Session and effort completion remained independent; archive/restore preserved status. Live Git publication was not tested.

The matched CLI, extension and shared skill are now active from the canonical checkout on local `main`. The installed catalog is initialised at `~/.local/share/streams/catalog` with `sync: none`; old notes remain untouched. Fresh global Pi startup and the full suite pass: 505 Python and 40 Node tests, no skips. All participating Pi conversations, including the migration conversation, have since reloaded the catalog-backed extension. Published scratch-based packages remain unchanged and must not be mixed with this CLI.

The one-off migration on 18 October created or restored 20 catalog efforts, covering every active/waiting/blocked legacy stream and all eight open Herdr workspaces. It imported 28 verified conversations, including all 14 currently registered native Pi sessions, and preserved four existing helper-session records. Workspace bindings and `--current` lookups were verified without launching, focusing or renaming native resources. Effort statuses, old notes and saved Work Cards data were preserved. Historical progress is labelled as such; missing session files and ambiguous legacy memberships were not guessed. Backups, source evidence, receipts and the migration plan are host-local under `~/.local/state/streams/migrations/`.

Automatic Pi enrollment now checks the actual Herdr pane's native session identity before creating locks, claims or records. This prevents an in-process helper from silently claiming its parent's workspace. A live check accepted the registered conversation without new saves and rejected a real helper without catalog changes.

Work Cards' native side panes, naming and short-description projections have not switched to these records. Its old independent writer is disabled in the active Pi package configuration; its saved data is preserved. Catalog-backed Alt+P cards are available through the migrated conversation associations, but the old side-pane renderers must not be treated as catalog views.

## Native UI continuation

The original Work Cards history through `e7d74ce2` is merged under `integrations/herdr-work-cards/` without squashing or rewriting the source commits. The import was verified byte-for-byte with executable modes, and its 864 tests and typecheck passed before adaptation. Its old Pi entrypoint and writer are not activated. Root `mise run test` now includes the imported regression suite.

The [native projection ADR](../docs/native-herdr-work-cards.md) preserves the per-session pane, renderer, navigation and naming policies while replacing catalog and membership authority. The Streams-backed projection, session-scoped checkout resolution, attested native effects and Pi lifecycle wiring are implemented and approved by Gemini Argon. The full checkout suite passes 651 Python, 103 Pi and 1,032 native Herdr tests, with no skips. Document navigation uses the viewing Session's registered checkout and current working file; missing files remain errors without fallback or reference deletion.

Isolated native acceptance on 18 October launched the exact saved Pi conversation in a disposable catalog and Herdr workspace, then restored the caller's focus. Card acceptance remains blocked: Herdr recognises the Pi process but exposes no `agent_session` identity, including after the managed reporter receives an acknowledgement. Streams rejects attestation before opening a card or projecting names. The disposable workspace was closed and its catalog, launch evidence and diagnostics retained locally. Per-session card rendering, native document navigation and multi-session restoration therefore remain unverified against the real frontend. No installed-runtime activation occurred.

A and B are two tabs/jj workspaces inside one Herdr workspace. Keep the runtime's one-native-workspace membership rule unchanged. Scope checkout bindings and editor reuse to the exact viewing session; a native-workspace-wide map would conflate its agents' checkouts.

The Pi slash-command family includes `/streams`, `/streams report [instructions]`, `/streams settings`, `/streams attach`, `/streams pane` and `/streams pane close`, with subcommand completion. The model-callable `checkpoint` tool is unchanged. The command rename is active in the canonical checkout; the native pane commands and adapter remain confined to this reviewed checkout pending acceptance and activation.

## Follow-on slices

- implement handover and reattachment coordination with confirmed authority separate from pending proposals
- resolve the native Herdr session-reporting blocker and complete isolated card, document and restoration acceptance before activation
- add captured document inputs before enabling inference that independently reads documents; the Pi helper currently uses conversation evidence and guarded catalog records
- consider advisor-guided restoration beyond the implemented last-recorded agent-tab arrangement; closed editor tabs and exact pane geometry are not reconstructed
- add dedicated conflict-resolution and interrupted-save repair commands; current code detects and preserves these conditions rather than repairing them automatically
- add an in-place local-only-to-Git transition preserving catalog records and host identity
- separately approve package publication and run live Git/multi-machine acceptance

Git backing and runtime publication integration are implemented in this checkout and covered by isolated tests. The installed catalog remains local-only; native activation and live Git/multi-machine acceptance are separate steps.

Start and attachment events check the shared M interval; explicit sync and shutdown bypass timing. Successful/no-change commands check once after all related saves. CLI commands are caller-driven; there is no background sync service.

No legacy-format compatibility or automatic migration is required. Existing streams can be migrated manually. Old scratch notes are reference-only, not a second writable catalog authority.
