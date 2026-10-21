# ADR: Replay frozen conversations through the reporting workflow

Status: accepted design; implemented and locally verified in the separate prompt-lab workspace, not activated or published. This decision does not authorise activation, publication or changes to production prompts. The batch-isolation policy below remains an implementation assumption.

## Context and decision

Streams needs a way to improve model-generated cards without repeating live work or risking the saved catalog. The user was dissatisfied with some summaries and wanted to capture Pi sessions, with or without cards, then rerun the helpers with a chosen model. The aim is prompt iteration, not another reporting system.

Capture frozen conversation samples and replay the normal reporting workflow against an isolated copy of their cards. Use the same evidence projection, prompt construction, model registry, normalization, strict response validation and catalog-domain rules as production. The user explicitly preferred using this work to make internal interfaces cleaner and more loosely coupled, rather than building a parallel implementation that might drift.

The [Pi work-card ADR](pi-work-card.md) continues to define reporting and manual-field protections. Replay changes where inputs come from and where proposals go; it does not define another meaning for a Stream or Session.

## Freeze the sample rather than require a historical helper call

Select a stream or an observed Herdr workspace. Capture the selected stream's associated session history together with open Pi conversations in the selected workspace or the stream's bound workspaces. Include open conversations without cards. Preserve another stream's actual membership when its conversation appears in that workspace; selection is not an attachment operation.

Record whether each conversation was open separately from its card status. A completed contribution can still be open, and an active card can describe a closed conversation. Preserve observed locations and explicitly list missing transcripts, foreign-host conversations and unsupported harnesses. If topology is unavailable or identities cannot be verified, report uncertainty rather than marking those conversations closed.

Freeze the projected evidence and catalog context, retaining private transcript snapshots for inspection. The current Pi session can supply its exact in-memory projection and leaf. Other sessions supply their persisted projection; an unsaved branch selection in another process is not recoverable from JSONL alone. Record that distinction rather than calling every snapshot the exact context of a historical request. Copy linked helper logs when available, but do not require logging to have been enabled.

Recording only actual helper calls was considered. It would reproduce a particular bad output well, but would omit conversations where no helper ran. Frozen samples were chosen first because they can exercise those conversations and remain useful while prompts change. A reconstructed request is not a historically sent request. Resending a recorded request unchanged is a separate possible mode, not the meaning of this replay command.

## Separate the workflow from its environment

The shared reporting workflow owns the ordering of enrollment, session reporting and effort reporting. It also owns incremental versus reset behavior and which automatic fields may change. Evidence preparation produces immutable conversation and catalog inputs. Model inference builds the request, runs the selected model and parses its response without needing the whole Pi extension context.

The live adapter supplies current attachment checks, cancellation, guarded catalog writes, receipt handling and native updates. The replay adapter supplies frozen evidence and a private working copy. A later helper in the same conversation sees earlier simulated card changes, just as the effort helper normally sees an accepted session report.

Simulated card operations use the real Python enrollment constructors, normalization and reconciliation over supplied snapshots. They do not open a backend, acquire live ownership, infer checkout bindings or publish. Simulated receipts are labelled as such. Replay tests card proposals and their ordering, not the live guarantees around concurrent edits, workspace claims, publication or native effects.

Capture also requires an explicitly read-only catalog inspection boundary. An ordinary catalog read may establish missing retry evidence, and a Git status check may refresh the index. Neither is acceptable during debug capture. Inspection must validate the catalog and effective authority without repairing bookkeeping, creating missing state or refreshing the index. Captured authority records preserve permission presence but redact live tokens.

## Keep incremental behavior as the default

The user chose incremental updates against captured cards, not fresh regeneration by default. Existing wording is part of the input because this should answer how the current prompts would update that state in normal use.

Reset is an explicit alternative, scoped to the Session, Stream or both. It regenerates permitted automatic text from frozen evidence while preserving manual text. With `--auto`, it also restores the selected automatic title and goal modes in the private working copy. It never reclaims real Pi or Herdr names. The original capture stays unchanged across all runs.

Each failed helper must remain visible independently. An accepted session proposal must not conceal a failed effort proposal. Report skipped targets separately from model or save failures; an accepted empty delta is a successful inference. Retain the target, actual model, rendered prompt and schema, raw response before normalization, parsed result or validation error, and any preceding simulated changes. Strict validation remains shared with production; replay must not silently discard unknown fields to make a run pass.

For the first version, change the actual shared prompts and reload the extension before replaying. Record source-file hashes and rendered requests so each result can be attributed to what ran. A separate template or override system would create another prompt path before there is evidence it is needed. Interactive card discussion is outside the initial replay scope.

## Make debug activity explicit and private

Use Pi commands initially. Pi already has the model registry, credentials and custom providers needed for normal inference. A shell runner could later make model sweeps easier, but would need to initialise that runtime without introducing another inference stack.

Expose capture and replay only when the default-disabled “Debug commands” setting is enabled. Group that setting and the separately opt-in “Helper session logging” setting under Debug in `/streams settings`. The wording is deliberately “Debug commands”, not “Debug features”: the gate controls these commands, not every possible future diagnostic feature. Enabling the gate does not start recording or enable live helper logging.

Capture files and replay results contain potentially sensitive conversation content. Keep them local, with private permissions, outside version control. Capture does not send the samples anywhere. Explicit replay does send the selected frozen evidence to the chosen model provider, with normal authentication and request costs. Confirm that action before inference. Disabling either setting stops its new activity but retains saved files.

Readable before-and-after cards and per-task artifacts support manual comparison. Automated scoring, model sweeps and publishing a preferred result back to real cards are not part of this decision.

## Initial batch policy

A capture contains both conversation evidence and available cards. Whether a run includes prior automatic card text is a run-level choice, using the same incremental and reset distinction as normal reporting. Reset does not mean making another capture without its cards.

The initial implementation treats each conversation as an independent trial starting from that snapshot. Its session and effort helpers share a working copy, but another sample's generated text does not affect it. This makes prompt comparisons independent of sample order.

Sample isolation is an implementation policy, not a separately accepted user decision. A shared, evolving stream copy would instead model a sequence of live reports and require an explicit replay order. That alternative is not exposed by the initial interface.
