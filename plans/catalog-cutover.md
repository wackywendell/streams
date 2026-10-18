# Durable catalog cutover

Temporary implementation plan. Remove after the CLI and integration cutover. The [catalog ADR](../docs/durable-work-catalog.md) remains the design authority; implementation contracts live beside the code in `src/_streams/catalog`.

## Current state

This checkout's CLI uses the durable catalog: explicit setup, discovery, field edits, lifecycle, session reports and native opening/resumption. Local-only and Git-backed coordinators handle saving, publication and read refresh. The CLI reports local-save and publication outcomes separately. Optional goals are represented as null.

`list` and `show` use read-only refreshed projections; mutations and effective authority use persisted local state. A record visible only in refreshed reads requires explicit `sync` before editing or opening. Reads never publish.

The checkout's Pi extension and bundled skill now use catalog context, guarded reporting, guided field editing and initial enrollment. Both real Pi/CLI contract tests are restored with isolated catalogs. The installed runtime remains unchanged; published scratch-based packages must not be mixed with this CLI. Work Cards has not switched to these records.

## Follow-on slices

- implement handover and reattachment coordination with confirmed authority separate from pending proposals
- connect Work Cards projections to the logical records
- add captured document inputs before enabling inference that independently reads documents; the Pi helper currently uses conversation evidence and guarded catalog records
- extend single-session opening/resumption to advisor-guided multi-session and tab reconstruction
- add dedicated conflict-resolution and interrupted-save repair commands; current code detects and preserves these conditions rather than repairing them automatically
- run native-host and credentialed-model acceptance tests, then separately approve package release and installed-runtime activation

Start and attachment events check the shared M interval; explicit sync and shutdown bypass timing. Successful/no-change commands check once after all related saves. CLI commands are caller-driven; there is no background sync service.

No legacy-format compatibility or automatic migration is required. Existing streams can be migrated manually. Do not switch the installed runtime or treat both formats as live authorities during implementation.
