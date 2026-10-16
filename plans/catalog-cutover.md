# Durable catalog cutover

Temporary implementation plan. Remove after the CLI and integration cutover. The [catalog ADR](../docs/durable-work-catalog.md) remains the design authority; implementation contracts live beside the code in `src/_streams/catalog`.

## Current state

The logical records, domain reconciliation and local YAML adapter are implemented. The CLI, Pi extension and Work Cards have not switched to them and still use scratch notes.

## Follow-on slices

- connect catalog operations to CLI discovery, lifecycle management and structured output
- compose Git transport with serialized persistence and local commits, including operation journaling and crash recovery
- implement handover and reattachment coordination with confirmed authority separate from pending proposals
- connect agent reporting and Work Cards projections to the logical records
- guard generated updates against captured cross-record, document and ownership inputs before enabling those inference paths
- connect workspace and session reconstruction to the runtime adapters
- retire scratch-generating checkpoints when their replacement is ready

No legacy-format compatibility or automatic migration is required. Existing streams can be migrated manually. Do not switch the installed runtime or treat both formats as live authorities during implementation.
