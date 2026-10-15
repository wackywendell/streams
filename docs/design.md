# Work Cards architecture

## Goal and scope

Work Cards provide a high-level summary of a project, its current state, and relevant artifacts and links, to provide guidance for a user navigating multiple agents. It provides navigation to plans, issues and pull requests without requiring them to read agent logs or maintain parallel prose.

The card is durable orientation, not an activity feed, task runner or full project manager. Substantive plans and architecture decision records belong in the project checkout. The card points to them rather than becoming another plan store.

This document describes the implemented architecture and its reasons. [Agreed direction not yet implemented](#agreed-direction-not-yet-implemented) separates future constraints from current guarantees. The [README](../README.md) owns commands, configuration defaults and verification details.

## One durable record with independent consumers

A small ESM TypeScript package contains a shared validated data core, a Pi updater and an independent Herdr display. YAML is authoritative. The display reads it without Pi running and publishes workspace-summary metadata. Herdr tokens carry ownership and display information, not another durable copy of the card.

Cards live in `$XDG_STATE_HOME/herdr/cards/<slug>.yaml` and outlast sessions, panes and workspaces. There is no central registry, session catalogue or central plan store. Storage is per host, without automatic synchronisation or backup.

The seven-field schema keeps the record small:

| Fields | Meaning |
| --- | --- |
| `title` | Human-readable name |
| `status` | `active`, `waiting`, `blocked`, `parked` or `done` |
| `state` | Single-line Goal, Now and Remaining summary |
| `cwd` | Checkout boundary on the execution host |
| `documents` | Checkout-relative paths with descriptions |
| `issues`, `prs` | HTTP(S) links with descriptions |

All fields are required; arrays and state may be empty. Unknown fields, duplicate YAML keys and unsafe displayed text fail validation. The initial title determines a stable filename slug. Later title changes do not rename the card; creation collisions cannot overwrite it.

When opening a document, the client resolves `cwd` and the target to canonical filesystem paths. The target must be a regular file inside that checkout. Traversal and escaping symlinks are rejected. This makes the checkout an explicit boundary rather than relying on repository discovery. Resolution does not eliminate a malicious filesystem race during a later open. Exact validation and library contracts live in [the shared core](../src/card.ts).

## Limited model authority and guarded writes

The helper has no executable tools. It returns `no_change` or a patch limited to `status`, `state`, `documents`, `issues` and `prs`. It cannot rename or move cards, change the checkout or write repository documents. The prompt asks the helper to retain existing references unless removal has affirmative evidence. This depends on model compliance: structural validation permits a valid replacement array to remove references.

Trusted global configuration stays separate from repository content and model-controlled card data. There are no project overrides. The default `helpers/simple` alias uses an externally registered profile and inherits its thinking level unless overridden. Physical models are also configurable. Luna with low thinking was the replay profile, not a hard-coded product default. See [configuration](../README.md#configure-the-package) and [the summarizer](../src/pi/summarizer.ts).

The core validates every proposed patch. Raw-byte revisions detect intervening edits, including formatting changes. Cooperating writers use exclusive locks and atomic replacement. Stale revisions and held locks fail visibly rather than retrying, stealing locks or overwriting newer work. Creation also prevents overwrite.

`no_change`, including a semantically unchanged patch, preserves bytes and modification time. Cancellation is checked immediately before dispatching rename, without an intervening asynchronous step. Once rename is dispatched, cancellation cannot roll it back.

These guarantees prevent stale helper writes; they do not make YAML an editor transaction system. Arbitrary external editors ignore package locks and can race the locked read-to-replacement interval. Atomic visibility also does not guarantee survival of every filesystem or power-loss failure.

## Summaries from human-readable evidence

State remains a single-line string encoded as `Goal: ... | Now: ... | Remaining: ...`. Goal describes the project purpose; Now describes its broad usable capability or stage; Remaining describes major unfinished scope or blockers. The helper targets 20 to 30 words total, excluding labels. This is a soft prompt target, not a validator rule or summary-quality guarantee.

Legacy prose may be converted or condensed without new progress, preserving supported facts without inventing a goal, completion or plans. Once correctly formatted and useful, routine work, tests and commits should normally yield `no_change`. The schema and write guards are unchanged; no migration or document-reading tools are introduced.

The brief asks the helper to derive project maturity against its goal and major intended capabilities from dialogue and the compaction checkpoint first. It then compares that result with the previous card, an editable draft rather than a prose template or proof of progress. A sound project-level summary should stay stable through routine work, but change when affirmative evidence changes a major capability or remaining scope. Plans establish intent, not completion; any supplied goal or scope excerpts serve only to explain intended scope. The helper does not load documents.

The brief also reconciles older blockers with explicit successful outcomes and avoids promoting unrelated working-copy hygiene into required work without relevance or blocking evidence. These remain prompt instructions, not enforced summary-quality guarantees.

Each request starts with a stable system brief. One user JSON message follows, ordered `{compactionSummary, transcript, card}`, with the current card last. The input uses Pi's actual latest projected compaction summary and all retained nonblank user and assistant text chronologically. It respects projected redactions and never reconstructs raw precompaction history.

Thinking, tool calls, arguments, results, images, custom messages, system messages and branch summaries are excluded. The prompt labels conversation content as untrusted evidence and tells the model not to follow embedded instructions. Compliance is not enforced by the patch validator. This choice relies on assistant text to report outcomes; unreported tool outcomes are absent.

The evidence policy addresses a concrete failure: both bounded, tool-inclusive replay baselines contained only tool results in their evidence windows, 2 or 3 messages and no direct dialogue. Compaction-plus-dialogue inputs captured relevant remaining work with fewer input tokens. The comparison comprised 4 requests at 2 historical snapshots, both containing user wording guidance. It is neither a broad benchmark nor proof of unguided summary quality.

There is no application cap, clipping or fallback trimming. Provider context-limit failures surface visibly. `max_context_chars` remains accepted and validated but is ignored for compatibility. Putting the card last may permit stable-prefix cache reuse; measured cache-hit benefits have not been established. [Evidence normalisation](../src/pi/evidence.ts) defines the input boundary.

## Scheduling without starving updates

By default, Pi checks each distinct normalized summary-input change, including both user and assistant text. `check_every_n_responses` controls this interval; zero disables automatic checks but not explicit refresh. The former tool-cycle setting has been removed.

Pi observes the persisted SDK projection at request-time `context`, after user messages are delivered, and completed `turn_end`, after assistant text and its tool batch are persisted. `turn_start` precedes user persistence; `message_end` precedes persistence and later handlers can replace text. Work Cards does not append raw event text or transform the main request. Settlement remains a deduplicated fallback.

Revisions hash only normalized compaction summary and dialogue, not branch leaves. Repeated hooks with unchanged input do not advance the interval. Excluded tool activity cannot create new input; compaction and projected redaction changes can qualify. Empty initial input does not start automation.

Checks are single-flight per session. New eligible input replaces pending input, while continuing dialogue neither cancels the active check nor requires an idle session before application. Repeated busy refresh requests coalesce into at most one followup, retaining explicit retry intent with the latest eligible input. Generation cancellation clears queued work; card, binding and session checks reject stale results.

The Pi helper-request spinner is off by default. With `show_refresh_spinner: true`, only actual helper requests show an animated `<spinner> work-card` terminal status. Queued work and waits for retired checks do not. Completion, errors, timeout, invalidations and disabling the option clear the status and timer. Disabled, RPC and headless modes create no animation timer. Activity does not change YAML, Herdr IPC, tabs or Pi's main working indicator.

There are no automatic retries or cooldowns. A timeout stops local waiting and requests cancellation, but cannot guarantee the upstream request stops. Helper usage, including `no_change` calls, remains outside Pi's usage totals because Pi has no public API to add it. [The scheduler](../src/pi/scheduler.ts) and [Pi integration](../src/pi/index.ts) define these lifecycle rules.

## Explicit ownership instead of GUI focus

Herdr operations target the exact caller, socket and workspace identity, never whichever workspace has GUI focus. Workspace locks serialise cooperating ownership changes. Unrelated, surviving or restored live panes are never claimed as card owners.

Recovery happens only during explicit open or detach. It requires a complete matching departed-owner tuple: canonical card identity, nonce, pane and terminal. Both referenced pane and terminal identities must be absent, with no conflicting marked panes and an unambiguous live inventory. Ambiguity fails closed. Polling and passive session restore do not recover owners.

Pi keeps a session-specific target and update permission, not a cached card. Restore validates existing YAML but never reconstructs missing cards or opens panes. Forks require explicit open. Slugless open uses the same originating session's binding, not cwd search or automatic adoption.

Successful open saves its binding, then requests a background summary check without delaying attachment on the helper. Failed or stale opens do not request checks. New does not start an immediate check. `/work-card refresh` accepts no arguments and requires an enabled same-session binding. It works outside Herdr, bypasses automatic cadence and permits unchanged-input retries without opening a pane, rebinding or cancelling active work. A paused binding requires explicit open to resume.

Detach records paused update permission before fallible configuration loading or Herdr cleanup. Failed cleanup therefore leaves automation paused for recovery. Successful detach forgets the binding without deleting YAML. Custom session entries reach disk only after host user or assistant activity; `--no-session` does not persist them. The pause is recorded first, not guaranteed flushed immediately.

In contrast, `q` closes only the display. Pi updates continue until explicit detach. Closing a pane is not interpreted as withdrawing permission to maintain the durable record. [Ownership classification](../src/herdr/ownership.ts) and [the Herdr client](../src/herdr/client.ts) enforce these boundaries.

## Quiet display and editor-first navigation

The read-only card view separates semantic state from wrapping and layout. It shows title, status, state and relative document paths. A shared strict recognizer accepts only complete, ordered, nonempty Goal, Now and Remaining fields with exact labels and no surrounding whitespace. The exact ` | ` separator and repeated field headers are reserved; other literal pipes remain content. Legacy prose, malformed or ambiguous carriers, and extra fields fall back to the entire prose state, without partial extraction.

Recognized fields start on separate rows and wrap independently without carrier pipes. When the state budget permits, layout reserves a row for each later section so a long Goal cannot hide Remaining. Smaller budgets truncate in section order. Existing reference reservations, notice geometry and prose layout remain unchanged. The sidebar uses status plus Now for recognized fields, otherwise status plus the full state, retaining its 80-code-point cap.

Terminal-cell measurement and sanitisation keep untrusted text separate from trusted styling. Layout can truncate content without changing the stored summary.

Visible document letters open documents; `q` is reserved. Only shortcuts actually visible remain active. Issues and PRs have numbered OSC 8 links, handled by client Ctrl-click. Digits are inert because there is no verified remote-safe URL opener. The bottom-right close hint can cover trailing content rather than reserving another row.

Documents open in a full tab using the existing editor or pager on the execution host. Command precedence is trusted global `document_command`, then `EDITOR`, then `less`. The validated filename is a separate literal argument, never shell source. Card data cannot select an executable. External editors can write documents; the display and helper cannot.

The same canonical checkout and document reuse a verified existing tab through focus only. Reuse preserves editor state, unsaved work and renamed tab titles. Ambiguous identity records refuse; unmarked panes are not adopted or closed. Partial launches report known identities for inspection rather than risking an automatic close or duplicate launch. See [navigation behaviour](../README.md#run-on-the-server-host) and [the view](../src/herdr/view.ts).

## Packaging and verification boundary

Pi receives the complete TypeScript source graph for Jiti reload correctness. Compiled ESM helpers remained cached after Pi cleared its extension cache; source loading refreshes local helpers without global cache changes. Herdr and library consumers still receive compiled JavaScript.

The package requires Node.js 22 or later. It bundles neither host dependencies nor install hooks. Build and installation remain explicit. macOS has verified paths; Linux and SSH are unverified design targets, not demonstrated support. The [verification record](../README.md#verification-status) distinguishes unit, loader, packaging and isolated native checks from unverified live Pi lifecycle and summarisation.

## Agreed direction not yet implemented

New sessions should default to automatic startup, with manual mode configurable. Naming should coordinate with the existing naming extension after meaningful initial task context. Startup and naming are not implemented; their precise mechanism and adoption rules remain unsettled.

The requested multi-agent direction keeps independent per-agent status while sharing workspace plan documents, issues and PRs. The overall workspace goal must come only from plan documents. Shared context repeats in each agent card pane; there is no workspace-overview tab. Initial plan-document selection should be automatic, with manual intervention possibly later.

These are constraints, not a finished persistence design. Separate workspace and per-agent records, extraction rules and merge policies were proposals, not agreed schemas.

[Herdr Annotate](https://github.com/plannotator/herdr-annotate) and Plannotator remain deferred in favour of the existing editor. [Source research on feedback delivery](https://github.com/plannotator/plannotator-tui/blob/main/crates/plannotator-tui/src/delivery.rs) indicates terminal input targets a pane, not a session-pinned callback. A replacement agent could receive feedback. The integration has not been installed or runtime-tested.

## Appendix A Cost analysis

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
