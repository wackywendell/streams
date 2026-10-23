# Streams: guidance for agents

## Design goals

1. Keep the data model and interaction model well designed enough that an event-driven design just works, with very little computation or I/O for any operation. An operation reads the records it concerns, applies the change as a pure function, and writes the records back. Reconciliation belongs in that apply step, not in extra lookups.
2. Keep the system modular. Sync backends and frontends (Herdr, cmux and others) are interchangeable implementations behind interfaces.
3. This is alpha software. If the data model needs to change, change it. Migrate existing data if needed. Losing some data is acceptable, because it is all generated automatically.
