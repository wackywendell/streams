# Streams: guidance for agents

## Design goals

Every action should be simple by construction. An action reads the few records it concerns, applies a pure change, and writes them back, with no discovery, coordination, retries or recovery logic of its own. If an action needs those, the data model is wrong: change the model, not the action. In particular:

- Each piece of state has exactly one writer.
- Records hold everything their operations need.
- Events say what to re-read, never what to compute.

From that follow:

1. Keep the data model and interaction model well designed enough that an event-driven design just works, with very little computation or I/O for any operation. Reconciliation belongs in the apply step, not in extra reads.
2. Keep the system modular. Sync backends and frontends (Herdr, cmux and others) are interchangeable implementations behind interfaces.
3. This is alpha software. If the data model needs to change, change it. Migrate existing data if needed. Losing some data is acceptable, because it is all generated automatically.

### Examples

- Reading a Session by its key is the intended design. Searching the catalog or a transcript directory to find it is discovery: save the key or path in the record instead.
- One writer means one owner per field, not per host. Several Pi children and CLI processes share a host. A label that both the user and Streams can change has two writers.
- Publishing is separate from saving. Git sync never holds the local record lock while it waits on the network.
- Cost covers everything an operation does: file reads, directory listings, process starts, transcript reads, bytes kept in records, and how long locks are held. A single file that accumulates every earlier version is not a small input.
- Projections compare the desired value with the observed one, and act only on a difference. No history of past actions decides whether an action runs.
- External effects are the one exception. A frontend call that isn't idempotent, such as opening a pane, may need recorded intent and outcome. Name the unsafe call, and the frontend capability that would remove the need.
- Old formats are read only by explicit offline migrations, never by runtime code or caches.

## Local permissions

If `AGENTS.local.md` exists in the workspace root, read it too. It holds local permissions for committing, integration and publishing.
