# ADR: Catalog-backed Pi work cards

Status: accepted; the catalog-backed Pi integration implements this decision. Native Herdr presentation is a separate [projection decision](native-herdr-work-cards.md), not another catalog.

## Decision

Replace the scratch-generating checkpoint workflow with automatic updates to durable Streams records. Use `/streams` or `/streams card` to inspect, edit and discuss the work card. The card is a view of the catalog, not a separate document or another source of truth.

The user chose automatic updates by default, with configurable automatic/manual mode, frequency, model and full versus trimmed context. Review remains available without making every progress report wait for approval. Plans and rationale stay in linked project documents rather than growing inside a checkpoint body.

## Reporting and context

The defaults are automatic reporting after each completed response, the current Pi model and trimmed context. Frequency is a positive number of completed responses. Manual mode disables background reporting and enrollment; explicit requests still work. A configured model that cannot run fails visibly rather than falling back to a different model.

A report captures Pi's active projected conversation, respecting compaction and context edits. Full mode preserves that projection, excluding incomplete tool batches; trimmed mode retains conversational content without tool calls, tool results or thinking. Do not silently truncate oversized inputs. Report failures leave recorded progress intact and do not interrupt the main agent.

The helper receives captured catalog records and conversation evidence. It has no project-writing or shell tools and does not independently reread documents. Historical file contents in the conversation remain historical evidence, not a fresh verification claim. Include current names, goals and progress in its input so later reports can respect edits made through the card.

Use a single background operation per Pi session. Coalesce newer evidence rather than queueing a report for every event. Before applying, check session identity, attachment and that the captured conversation remains on the active branch. Appending work does not invalidate an older captured prefix: save that report and schedule a follow-up. A rewind, branch switch or session replacement invalidates the result. Automatic updates transition `session.status` (`active`, `waiting`, `blocked`, `parked`, `done`) from conversation evidence—including when the user indicates the session is done, paused, waiting or blocked—while whole-effort `stream.status`, archive state and protected naming modes remain manual-only.

## Identity and initial enrollment

A catalog session identifies one native Pi conversation on one host and belongs to one stream. Reopening that conversation reuses its record. A fork with a new native ID gets another session record.

Resolve an existing session association before enrolling anything. A caller workspace's explicit stream association can supply the initial effort. With meaningful work but no association, automatic mode may enroll an effort and its first session. Avoid enrollment for greetings or other context that does not establish an effort. Explicit initial attachment is available for an unbound conversation.

Once attached, Pi does not offer a switch-to-another-stream action. Moving or forking contributions needs a future explicit lifecycle command; changing a title must never silently change membership. Conflicting session and caller-workspace associations fail visibly rather than choosing one by guesswork.

Shared workspace enrollment must converge on one stream identity even when several Pi conversations begin together. Automatic Herdr enrollment must also verify that the requested conversation is the exact native caller: background helpers can inherit a parent's pane environment without being that conversation. Inherited environment alone must not let a helper claim the workspace under its narrower assignment. Native associations and absolute directories remain host-local. Enrollment does not open workspaces, launch agents or move focus.

## Viewing, editing and discussion

`/streams` and `/streams card` open the current work card even if no report or chat is running. It shows the stream's goal and references, the current session's progress and other recorded contributions. Report activity and failures are visible without stealing focus from the main conversation.

Titles are short, recognizable sidebar and tab-bar handles, not descriptions: effort titles name the subject in 1–3 words (target 20 characters or fewer); session titles name the lane or role in 1–2 words, 3 at most (target 18 characters or fewer). Do not repeat effort-title words in the session title or add task verbs, generic filler or compound lists. Put descriptions in goal and state.

The panel separates “Whole effort” from “This session”. The editor uses labelled fields rather than raw JSON or YAML: text editors for titles and descriptions, choices for valid statuses, and explicit archive/restore actions. IDs, ownership and membership are read-only. Finishing a contribution never implies finishing the effort.

Changing a title or goal protects it as manual text. Show that protection beside the field, with an explicit way to return it to automatic management. Submit only dirty fields; opening or saving an unchanged form must not turn automatic text into manual text or erase compact derivatives. Background reports can refresh clean fields while dirty fields retain their captured base. Invalid or stale edits keep their input and show an actionable error.

Scope reference editing to issue/PR URL-and-description rows and documents whose repository is chosen from existing aliases. Document paths must be relative; URLs and required fields are validated before save. Repository identity changes remain CLI operations rather than a nested free-form table.

A direct edit is user intent. User-directed lifecycle edits are permitted, but background inference must never park, finish or archive an effort. Chat may discuss uncertainty without changing records. Separate a conversational reply, an automatic description refresh and an explicitly requested user edit; do not treat every chat turn as permission for management changes.

Closing the panel does not discard its unsent input or discussion. Reopening restores it for that native Pi conversation and catalog within the same process; extension reload does not. Menus refresh on action or reopen rather than pushing updates while a selection is open. Opening the panel alone neither starts a model request nor modifies the catalog. Only explicitly submitted edits or discussion run its agent.

An explicit report request saves progress without creating an approval queue. Group user-facing actions under `/streams` with space-separated subcommands, including `/streams card`, `/streams report`, `/streams reset`, `/streams settings` and `/streams attach`. `/streams reset` regenerates allowed automatic fields from conversation evidence without anchoring on prior generated text; manual titles and goals stay protected. Reset requires an attached work card and never enrolls an unbound conversation. `/streams reset --auto` also restores automatic title and goal modes and reclaims Pi session and Herdr workspace/tab names. The work card panel offers both reset actions. Pi supports argument completion on a registered command, so this keeps related actions discoverable without separate colon-named registrations. Bare `/streams` opens the same panel as `/streams card`, without running a model or enrolling the conversation. `/streams report [instructions]` requests an immediate progress report; instructions belong after `report`, avoiding ambiguity with management actions. Alt+P is reserved for Herdr navigation. The model-callable `checkpoint` tool remains a separate interface. `/streams pane` explicitly opens the native Herdr card; `/streams pane close` closes only that display. They do not request a report or alter membership.

Settings use an in-place list for reporting mode, enrollment, frequency and context, with a searchable picker of available reporting models. The same dialog exposes Herdr startup and naming settings in `work-cards.json`, preserving other keys. Native presentation retains its independent document-command settings; they do not configure another summarizer. Native refreshes reread persisted records after accepted operations rather than projecting model output or partial receipts. The extension and bundled skill describe the same record semantics; they do not retain scratch-format aliases or instructions.

## Persistence boundary

The Python CLI owns record validation, effective authority, persistence and publication scheduling. TypeScript exchanges structured requests and receipts with it; it never writes catalog YAML. Use persisted local records for report bases and execution authority, not refreshed display projections.

A local-context CLI endpoint returns persisted records and effective authority without a fetch. An opaque host-local source guard fences catalog selection or replacement, including replacements with copied record IDs; it is not a shared domain field. A structured apply endpoint accepts this guard, stable operation IDs, the original captured target, allowlisted changes and every other captured catalog record supplied to inference. It does not retake an edit's base after inference or force generated text into manual naming modes. Omit protected fields before constructing an automatic operation so a manual title does not discard an otherwise valid goal update.

Domain reconciliation rejects changed inputs and ownership, including during publication replay, before an already-satisfied/no-change shortcut. Ordinary user edits retain field-level reconciliation. Generated output is parsed into an allowlisted update shape before it reaches persistence.

Generate existing-session progress and stream metadata as separate target-scoped requests, capturing each request's inputs before its inference. This costs an additional model request when both need updating, but avoids making one write invalidate another operation from the same inference. Do not remove real input dependencies or substitute post-inference snapshots to make a stale operation apply. Initial enrollment may save a newly inferred stream and its session together as ordered, separately receipted saves.

Keep record saves separate rather than promising a multi-record rollback transaction. If one save succeeds and a later step fails, return the accepted records and receipts. Publication failure must not turn an accepted local report into an apparent unsaved operation. Session progress never rewrites the stream merely to refresh its timestamp.

## Consequences and boundaries

Automatic reporting removes the routine approval step, but makes captured inputs, manual protection and visible failures essential. A report is a last-known account, not independent verification of every statement. Keeping the card available for guided edits and discussion preserves user control without reinstating a second draft document.

This decision covers the Pi integration and bundled skill. Cross-host handover, contribution moves, arrangement restoration and native pane ownership have separate application contracts; a Pi conversation change must not silently invent those operations. The [README](../README.md) owns usage and configuration, while tests and the temporary cutover plan track implementation and acceptance evidence.
