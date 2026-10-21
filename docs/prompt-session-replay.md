# ADR: Replay saved prompts without live effects

Status: implemented in the private Rust migration. Installation and live activation remain separate. Conversation isolation below is an implementation assumption, not a confirmed batch-replay requirement.

Streams needs to rerun session and effort reporting against saved setups. The goal is to compare prompts, models and reporting behaviour without changing the work being inspected. The reference in `streams-prompt-lab` reuses production reporting over frozen evidence; it does not recreate a working native workspace.

## Separate inspection from simulation

Capture uses a dedicated read-only catalog inspection. Ordinary catalog reads are unsuitable: Git reads can establish retry receipts, and status can refresh the index. Inspection instead returns durable local records and ownership evidence without repairing either. Capture redacts ownership tokens but preserves their presence and host. It does not resolve foreign-host transcripts on the local machine.

Native topology remains evidence, not authority. A failed or incomplete observation means the conversation's open state is unknown, not closed. Capture distinguishes the current process's selected in-memory branch from another process's persisted transcript. Missing files and unsupported harnesses remain visible. Raw snapshots and projected messages are frozen together, with hashes identifying the evidence and implementation used.

The Rust preview boundary takes only supplied records and intent. It shares enrollment parsing, operation construction and reconciliation with production. It has no backend, native transport or persistence capability. Its receipts explicitly say that nothing was saved or published. There is no simulated `SavePermit`: real saves still enter the existing Temper-enforced persistence protocol. Frozen evidence cannot resume a pending native attempt or mint a live effect permit.

## Reuse reporting rather than duplicate it

The Pi extension shares production prompt construction, inference parsing, manual-field protections and report ordering with replay. Replay substitutes frozen evidence for session reads and preview for catalog writes. Session reporting precedes effort reporting, so the latter sees the former's simulated result. Incremental reporting is the default; reset is explicit and preserves manual fields unless automatic naming is explicitly requested.

Each conversation trial starts from its own captured cards. Helpers within a trial share simulated changes; separate trials do not. This makes conversation comparisons independent of iteration order. A sequential shared-effort experiment would answer a different question and needs an explicit mode before it can replace this assumption.

## Keep sensitive artifacts and model calls explicit

Debug commands and helper logging default to disabled. Capture asks before copying private conversation evidence and sends nothing to a model. Replay requires a selected model and confirmation before sending evidence; there is no fallback model. Captures and runs use private directories and files outside version control. Storage rejects symlinks at or below each private root, but allows OS-managed aliases above it. Helper-log links can read only private regular files inside the reporting-log root. Cancellation stops later helpers while retaining diagnostics and any earlier simulated results.

This costs disk space and does not reproduce historical backend recovery, native layout restoration or another process's unsaved branch choice. It does preserve the inputs and reporting path needed to compare prompts without giving replay access to live effects.

## Running without activation

`STREAMS_BIN_PATH` can point one test process at the built Rust binary. This existing executable override does not replace the installed Python command. `mise run test:replay` exercises the production Pi helper workflow through the real Rust preview and capture commands, using deterministic model responses and an isolated home. No native workspace or model service is contacted.
