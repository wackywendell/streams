# Durable catalog cutover

Temporary implementation plan. Remove after the CLI and integration cutover. The [catalog ADR](../docs/durable-work-catalog.md) remains the design authority; implementation contracts live beside the code in `src/_streams/catalog`.

## Current state

This checkout's CLI uses the durable catalog: explicit setup, discovery, field edits, lifecycle, session reports and native opening/resumption. Local-only and Git-backed coordinators handle saving, publication and read refresh. The CLI reports local-save and publication outcomes separately. Optional goals are represented as null.

`list` and `show` use read-only refreshed projections; mutations and effective authority use persisted local state. A record visible only in refreshed reads requires explicit `sync` before editing or opening. Reads never publish.

The Pi extension and bundled skill use catalog context, guarded reporting, guided field editing and initial enrollment. Both real Pi/CLI contract tests are restored with isolated catalogs.

On 18 October 2026, isolated Herdr/Pi acceptance passed with real Gemini Flash inference and local-only storage. It covered reporting controls, Alt+P edits and discussion, manual-field protection and workspace/session reuse. Session and effort completion remained independent; archive/restore preserved status. Live Git publication was not tested.

The matched CLI, extension and shared skill are now active from the canonical checkout on local `main`. The installed catalog is initialised at `~/.local/share/streams/catalog` with `sync: none`; old notes remain untouched. Fresh global Pi startup and the canonical checkout's full suite pass: 499 Python and 40 Node tests, no skips. Existing Pi processes need a restart. Published scratch-based packages remain unchanged and must not be mixed with this CLI.

Work Cards projections have not switched to these records. Its old independent writer is disabled in the active Pi package configuration; its saved data is preserved.

## Follow-on slices

- implement handover and reattachment coordination with confirmed authority separate from pending proposals
- connect Work Cards projections to the logical records
- add captured document inputs before enabling inference that independently reads documents; the Pi helper currently uses conversation evidence and guarded catalog records
- extend single-session opening/resumption to advisor-guided multi-session and tab reconstruction
- add dedicated conflict-resolution and interrupted-save repair commands; current code detects and preserves these conditions rather than repairing them automatically
- add an in-place local-only-to-Git transition preserving catalog records and host identity
- separately approve package publication and run live Git/multi-machine acceptance

Git backing and runtime publication integration are independent follow-on work; they do not block local use.

Start and attachment events check the shared M interval; explicit sync and shutdown bypass timing. Successful/no-change commands check once after all related saves. CLI commands are caller-driven; there is no background sync service.

No legacy-format compatibility or automatic migration is required. Existing streams can be migrated manually. Old scratch notes are reference-only, not a second writable catalog authority.
