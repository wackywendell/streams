# ADR 1 Durable, evidence-grounded orientation

Status: accepted and implemented. [ADR 2](workspace-context.md) records the shared workspace and per-session model, including its unfinished naming and management work. The [README](../README.md) owns commands, configuration and verification instructions.

## Context and problem

People navigating several agents need to know the purpose of the work, its current state and where to find its plans and artifacts. Reading agent logs is too expensive. Maintaining a second account of the plan creates another source of truth.

An agent's activity is not necessarily project progress. Tests, commits and routine edits can leave the usable capability unchanged. A short summary must distinguish intended scope from demonstrated outcomes and leave uncertainty visible.

## Decision

### Keep orientation separate from substantive documents

Work Cards stores small, durable summaries and references. It is not an activity log, task runner, task manager or parallel plan store. Plans and architecture decision records belong in the project checkout. Work Cards points to them.

Summaries describe intended scope, evidenced progress and remaining work, not a list of recent commands. Routine activity alone should not change them. Existing summaries are editable drafts, not evidence of progress.

### Make local records authoritative

Validated local records outlast Pi sessions, panes and native workspaces. Pi updates them; an independent Herdr display reads them without requiring Pi to run. Herdr metadata carries transient bindings and projections, not another authoritative copy.

Storage is per host. There is no global repository registry, cloud synchronisation or automatic backup. Registered checkout directories define document boundaries. Opening a document requires a canonical regular file inside its registered checkout; traversal and escaping symlinks are rejected.

Writes use strict validation, revision checks, cooperating-writer locks and atomic replacement. Stale edits fail; additive reference proposals merge without dropping concurrent additions. Unchanged updates preserve bytes and modification time. Atomic visibility is not crash durability. External editors do not obey package locks, and path validation cannot eliminate a later malicious filesystem race.

The original single-card record combined project purpose, agent progress and references in one place. Its pipe-delimited summary was a presentation carrier, not a lasting domain model. [ADR 2](workspace-context.md) replaces both with structured workspace and agent records; neither is a current compatibility interface.

### Give helpers evidence, not executable authority

Helpers have no executable tools. Trusted runtime code chooses their inputs, validates their proposals and applies only permitted changes. Helpers cannot write project documents, choose executables, invent runtime identities or alter membership and directories.

Trusted global configuration selects the helper route and executable used for document opening. Repository text and record content cannot override it. Separate request contracts restrict own-agent updates, shared additions, plan selection and Goal inference. Structural restrictions enforce authority, not the truth or quality of model interpretation.

Human-readable evidence remains untrusted content. Prompts tell helpers not to follow embedded instructions, but validation cannot guarantee compliance with that instruction. Unsupported facts should remain unknown rather than become invented goals, completion claims or blockers.

### Use projected dialogue for own-agent summaries

Own-agent evidence contains Pi's latest projected compaction summary and all retained nonblank user and assistant text, in chronological order. It respects projected redactions and does not reconstruct raw precompaction history.

Thinking, tools and their arguments or results, images, custom messages, system messages and branch summaries are excluded. This relies on assistant text to report outcomes: an unreported tool outcome is absent. Plans establish intent, not completion.

The application does not clip this dialogue or silently trim it to fit. Provider context-limit failures remain visible. There is no guarantee that every session will fit or that a model will produce a useful summary.

This choice followed a narrow replay comparison. Both bounded, tool-inclusive baseline windows held only tool results: 2 or 3 messages, with no direct dialogue. Compaction-plus-dialogue inputs captured relevant remaining work with fewer input tokens. The comparison covered 4 requests at 2 historical snapshots, both containing user wording guidance. It is not a broad benchmark or proof of unguided summary quality.

Workspace Goal and shared-reference additions have different evidence boundaries. [ADR 2](workspace-context.md#separate-evidence-by-purpose) defines them; full own-agent history is not permission to rewrite shared intent or repopulate links after a move.

### Bound execution without starving updates

Scheduling follows distinct normalised dialogue changes, not tool cycles. Checks are single-flight per session. New input replaces pending input without repeatedly cancelling an active check; busy refresh requests coalesce. Session and membership changes invalidate stale results.

There are no automatic helper retries. Timeout ends local waiting and requests cancellation, but cannot guarantee that an upstream request stops. Helper usage is outside Pi's usage totals because Pi has no public API to add it.

Own summaries, shared additions, plan selection and Goal inference use separate requests through the same configured model route. The current coordinator runs them sequentially, so Goal can arrive after the own-agent summary. These stages keep evidence and write permissions separate; they need not appear as one simultaneous update.

### Keep display and navigation outside helper authority

The display is read-only. Layout and truncation do not change stored meaning or document targets. Document opening uses a trusted editor or pager on the execution host, with the validated filename as a literal argument. That editor may write files; the display and helpers cannot.

Native effects require exact caller and ownership verification, never GUI focus. Ambiguity suspends effects rather than authorising adoption, cleanup or a guessed target. [ADR 2](workspace-context.md) defines grouping, lifecycle and navigation.

## Alternatives and tradeoffs

| Alternative | Reason for this decision |
| --- | --- |
| Read logs or show an activity feed | Activity does not establish capability, and logs make orientation costly |
| Store plans in cards | A second plan store competes with documents in the checkout |
| Treat Herdr metadata as durable state | Native surfaces can disappear; independent consumers need records that survive them |
| Include bounded tool history | The observed windows displaced direct dialogue; excluding tools instead loses unreported outcomes |
| Give helpers filesystem or native tools | Runtime-selected evidence and narrow proposals keep authority outside the model |
| Silently trim oversized evidence | A visible failure is preferable to presenting incomplete evidence as complete |
| Add a global registry or cloud service | The chosen boundary is local host state, accepting no automatic cross-host continuity |

These are design tradeoffs, not evidence that every alternative received a runtime evaluation.

Editor-first navigation also defers Herdr Annotate and Plannotator. [Herdr Annotate](https://github.com/plannotator/herdr-annotate) and [Plannotator feedback delivery](https://github.com/plannotator/plannotator-tui/blob/main/crates/plannotator-tui/src/delivery.rs) were considered as sources. The latter targets terminal input at a pane, not a session-pinned callback, so a replacement agent could receive feedback. Neither integration was installed or runtime-tested.

## Consequences and boundaries

The record can remain useful when the summarizer or display is unavailable. Users retain responsibility for substantive documents, backups and explicit management actions. Short summaries can be stale or wrong; validation is not a semantic quality guarantee.

The package keeps one shared data core with separate Pi and Herdr consumers. Reloading Pi must not leave stale helper code active; changing the display must not require Pi to own its process.

All components and paths refer to the execution host. macOS has verified paths; Linux and SSH remain unverified design targets. Isolated native checks and injected SDK tests do not establish every full Pi-session lifecycle. See the [verification status](../README.md#verification-status) and [ADR 2 status](workspace-context.md#status-and-boundaries).

## Historical evidence boundary

The appendix preserves the earlier replay and accounting data. It is supporting historical evidence, not an invoice, a current workload prediction, a warm-cache measurement or a quality benchmark. The later workspace runtime makes several separate helper requests, so the earlier single-card workload must not be presented as its usage estimate.

## Supporting evidence from historical cost analysis

### Prices and accounting

Prices were retrieved on 15 October 2026 from the published model pages for [Luna](https://developers.openai.com/api/docs/models/gpt-6-luna), [Sol](https://developers.openai.com/api/docs/models/gpt-6.1-sol) and [Astra](https://developers.openai.com/api/docs/models/gpt-6-astra). Rates are Standard USD per million tokens, not invoices or negotiated gateway prices.

| Model | Ordinary input | Cache reads | Cache writes | Output |
| --- | ---: | ---: | ---: | ---: |
| GPT-6 Luna | 0.10 | 0.01 | 0.125 | 0.50 |
| GPT-6.1 Sol | 2.00 | 0.10 | 2.50 | 10.00 |
| GPT-6 Astra | 10.00 | 1.00 | 12.50 | 50.00 |

Cost is `(ordinary input × input rate + cache reads × read rate + cache writes × write rate + output × output rate) / 1,000,000`. Reasoning is already included in output, not charged again. None of the recorded requests crossed the 272,000-input-token premium threshold.

### Measured replay usage

The bounded, tool-inclusive baseline is an older comparison arm, not current production input. All 4 Luna replay requests had 3 ordinary input tokens and zero cache reads. Total input equals ordinary input plus cache writes.

| Snapshot and arm | Total input | Cache writes | Output including reasoning | USD |
| --- | ---: | ---: | ---: | ---: |
| 1 baseline | 5,738 | 5,735 | 214 | 0.000824175 |
| 1 compaction and dialogue | 4,715 | 4,712 | 190 | 0.000684300 |
| 2 baseline | 6,488 | 6,485 | 326 | 0.000973925 |
| 2 compaction and dialogue | 4,608 | 4,605 | 267 | 0.000709425 |

The 2 candidate calls cost $0.001393725; all 4 cost $0.003191825. These are measured token footprints priced at published rates. They do not establish warm-cache performance.

### Historical estimated automatic workload

The accounting window runs from 19:28:39 UTC on 14 October 2026 to 00:09:56 UTC on 15 October. It excludes inherited earlier history and work after the cost question.

The previous response-ending and ten-cycle cadence suggests 43 opportunities: 39 response endings and 4 ten-cycle boundaries. This historical estimate does not describe the current dialogue-input cadence and is not an observed request count. Missing lifecycle records, aborts, reloads and pending-evidence coalescing can change it.

Historical Pi projections estimate 245,251 input tokens, averaging 5,704 and ranging from 4,207 to 7,753. Assumptions are an 886-character card allowance, 4.3608 serialized characters per token calibrated from 2 candidate replays, and 228.5 output tokens per request including reasoning.

With all input charged as new cache writes, the estimate is $0.03557. Assuming 90% cache reads lowers it to $0.01019; that is an unmeasured sensitivity scenario. Tokenisation, card size and future output length remain estimates. No further model requests were made for this calculation.

### Counterfactual main-agent prices

The window contains 195 recorded main-agent Astra calls: 585 ordinary input, 23,468,482 cache reads, 1,106,629 cache writes and 221,559 output tokens. Output includes 174,499 reasoning tokens.

The same 24 child agents made 529 calls. Their aggregate footprints allow the retained mixed-model child cost to be audited:

| Child model | Calls | Ordinary input | Cache reads | Cache writes | Output |
| --- | ---: | ---: | ---: | ---: | ---: |
| Luna | 132 | 4,980 | 8,267,097 | 779,472 | 78,931 |
| Sol | 319 | 954 | 18,881,396 | 1,065,706 | 138,243 |
| Astra | 78 | 234 | 3,745,201 | 419,171 | 22,381 |

Repricing only the main-agent footprint gives:

| Main-agent rate | Main conversation USD | Same children USD | Combined USD |
| --- | ---: | ---: | ---: |
| Sol | 7.33018 | 16.26304 | 23.59322 |
| Astra | 48.38514 | 16.26304 | 64.64818 |

This holds tokens, caching and delegation fixed; it does not predict identical behaviour from different models. Totals exclude unrecorded background helper or compaction usage and external service fees. Replay requests and the proposed watcher workload are separate from Pi's main usage ledger. Low watcher cost alone establishes neither summary quality nor a latency guarantee.
