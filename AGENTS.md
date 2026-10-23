# Streams: guidance for agents

## Design goals

Every action should be simple by construction. An action reads the few records it concerns, applies a pure change, and writes them back, with no lookups, coordination, retries or recovery logic of its own. If an action needs those, the data model is wrong: change the model, not the action. In particular:

- Each piece of state has exactly one writer.
- Records hold everything their operations need.
- Events say what to re-read, never what to compute.

From that follow:

1. Keep the data model and interaction model well designed enough that an event-driven design just works, with very little computation or I/O for any operation. Reconciliation belongs in the apply step, not in extra lookups.
2. Keep the system modular. Sync backends and frontends (Herdr, cmux and others) are interchangeable implementations behind interfaces.
3. This is alpha software. If the data model needs to change, change it. Migrate existing data if needed. Losing some data is acceptable, because it is all generated automatically.
