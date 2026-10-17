# Durable catalog cutover

Temporary implementation plan. Remove after the CLI and integration cutover. The [catalog ADR](../docs/durable-work-catalog.md) remains the design authority; implementation contracts live beside the code in `src/_streams/catalog`.

## Current state

This checkout's CLI uses the durable catalog: explicit setup, discovery, field edits, lifecycle, session reports and native opening/resumption. Local-only and Git-backed coordinators handle saving, publication and read refresh. The CLI reports local-save and publication outcomes separately. Optional goals are represented as null.

`list` and `show` use read-only refreshed projections; mutations and effective authority use persisted local state. A record visible only in refreshed reads requires explicit `sync` before editing or opening. Reads never publish.

The installed runtime is unchanged. The Pi checkpoint extension, bundled skill and Work Cards have not switched to these commands. Do not install that extension alongside this checkout's CLI.

## Follow-on slices

- implement handover and reattachment coordination with confirmed authority separate from pending proposals
- connect agent reporting and Work Cards projections to the logical records
- guard generated updates against captured cross-record, document and ownership inputs before enabling those inference paths
- extend single-session opening/resumption to advisor-guided multi-session and tab reconstruction
- add dedicated conflict-resolution and interrupted-save repair commands; current code detects and preserves these conditions rather than repairing them automatically
- retire scratch-generating checkpoints when their replacement is ready, then replace and re-enable the 2 deferred real Pi/CLI contract tests

Start and attachment events check the shared M interval; explicit sync and shutdown bypass timing. Successful/no-change commands check once after all related saves. CLI commands are caller-driven; there is no background sync service.

No legacy-format compatibility or automatic migration is required. Existing streams can be migrated manually. Do not switch the installed runtime or treat both formats as live authorities during implementation.
