# Streams: guidance for agents

## Design goals

Streams should be simple, straightforward and predictable, both in how it's built and in how it behaves. Prefer a design a reader can follow at a glance, and behaviour a user can predict without reading the code. When a change makes either harder, stop and reconsider the design rather than adding to it.

- **Simple actions.** An action reads the few records it concerns, applies a pure change and writes them back. If it needs discovery, coordination, retries or recovery logic, the data model is wrong: change the model, not the action.
- **One writer per piece of state.** Records hold what their operations need. Events say what to re-read, not what to compute.
- **Observe, compare, act.** Projections compare the desired state with the observed one and act only on a difference. Expect outside changes, and prefer making a repeated action harmless to recording what was attempted.
- **Show correctness in tests.** Runtime guards are single, cheap checks. Tests with fakes that inject outside changes show that the program still behaves as expected.
- **Modular.** Sync backends and frontends (Herdr, cmux and others) are interchangeable implementations behind interfaces.
- **Build for the common path.** Build machinery only for what runs often or unattended. Rare events, or anything a person does while watching, get a written step, not code.
- **Alpha.** If the data model needs to change, change it, and reset rather than migrate. Losing some data is acceptable, because it is all generated automatically. Runtime code never reads old formats.

### Examples

- Reading a Session by its key is the intended design. Searching the catalog or a transcript directory to find it is discovery: save the key or path in the record instead.
- A label that both the user and Streams can change has two writers.
- Cost covers everything an operation does: file reads, directory listings, process starts, bytes kept in records, and how long locks are held.

## Local permissions

If `AGENTS.local.md` exists in the workspace root, read it too. It holds local permissions for committing, integration and publishing.
