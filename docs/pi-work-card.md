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

Once attached, Pi does not offer a switch-to-another-stream action. There is no Session-move CLI or Pi switch action. Observed workspace moves follow [native reconciliation](native-context-reconciliation.md); a refresh preserves saved membership and warns on conflicting markers. Changing a title never changes membership.

Enrollment reuses an observed workspace association rather than guessing from names or cwd. The private Pi channel supplies conversation identity; native completion uses one current observation. Record saves and marker projection are separate, not a transaction or a claim of exactly-once concurrent enrollment. Native associations and absolute directories remain host-local. Enrollment does not open workspaces, launch agents or move focus.

## Viewing, editing and discussion

`/streams` and `/streams card` open the current work card even if no report or chat is running. It shows the stream's goal and references, the current Session's progress. Report activity and failures are visible without stealing focus from the main conversation.

Titles are short, recognizable sidebar and tab-bar handles, not descriptions: effort titles name the subject in 1–3 words (target 20 characters or fewer); session titles name the lane or role in 1–2 words, 3 at most (target 18 characters or fewer). Do not repeat effort-title words in the session title or add task verbs, generic filler or compound lists. Put descriptions in goal and state.

The panel separates “Whole effort” from “This session”. The editor uses labelled fields rather than raw JSON or YAML: text editors for titles and descriptions, choices for valid statuses, and explicit archive/restore actions. IDs, ownership and membership are read-only. Finishing a contribution never implies finishing the effort.

Changing a title or goal protects it as manual text. Show that protection beside the field, with an explicit way to return it to automatic management. Submit only dirty fields; opening or saving an unchanged form must not turn automatic text into manual text or erase compact derivatives. Background reports can refresh clean fields while dirty fields retain their captured base. Invalid or stale edits keep their input and show an actionable error.

The Stream tab's References manager includes every saved state. It edits names, purposes, roles, kinds, targets and document recovery fields. Documents use direct repository URLs and portable relative paths. Ordinary edits preserve Dropped state; explicit Restore chooses Active or Fulfilled, and Delete confirms that reporting may rediscover the resource.

Each exact selector retains its original capture and unsaved input. Reopening resumes it, including after source deletion. Retained-only rows remain independently inspectable and discardable; explicit reapply refreshes the capture and reconfirms deletion or restoration. A retained manual-add form remains separate from saved rows. Discussion reference batches require an empty retained-edit map, and partial results retain every unapplied row.

A direct edit is user intent. User-directed lifecycle edits are permitted, but background inference must never park, finish or archive an effort. Chat may discuss uncertainty without changing records. Separate a conversational reply, an automatic description refresh and an explicitly requested user edit; do not treat every chat turn as permission for management changes.

Closing the panel does not discard its unsent input or discussion. Reopening restores it for that native Pi conversation and catalog within the same process; extension reload does not. Menus refresh on action or reopen rather than pushing updates while a selection is open. Opening the panel alone neither starts a model request nor modifies the catalog. Only explicitly submitted edits or discussion run its agent.

An explicit report request saves progress without creating an approval queue. Group user-facing actions under `/streams` with space-separated subcommands, including `/streams card`, `/streams report`, `/streams reset`, `/streams settings` and `/streams attach`. `/streams reset` regenerates allowed automatic fields from conversation evidence without anchoring on prior generated text; manual titles and goals stay protected. Reset requires an attached work card and never enrolls an unbound conversation. Reset first discards whichever card records cannot be decoded, so it never depends on reading them. The card is the conversation's Session, found by its native identity, and the Stream that Session names. An unreadable Stream is replaced in place with the same ID, so every member keeps its membership, and the reset then infers its fields. An unreadable Session is removed, because the Stream it named is unknown, and the reset enrolls the conversation again. Readable and missing records are left alone, and each discarded record is named in a warning with its decode error. A native startup that failed is tried once more after a discard, because the discard changed what it read. `/streams reset --auto` also restores automatic title and goal modes. Enabled frontend naming continues to project saved titles; there is no separate native-name reclaim protocol. The work card panel offers both reset actions. Pi supports argument completion on a registered command, so this keeps related actions discoverable without separate colon-named registrations. Bare `/streams` opens the same panel as `/streams card`, without running a model or enrolling the conversation. `/streams report [instructions]` requests an immediate progress report; instructions belong after `report`, avoiding ambiguity with management actions. Alt+P is reserved for Herdr navigation. The model-callable `checkpoint` tool remains a separate interface. `/streams pane` explicitly opens the native Herdr card; `/streams pane close` closes only that display. They do not request a report or alter membership.

Settings use an in-place list for reporting mode, enrollment, frequency and context, with a searchable picker of available reporting models. The same dialog exposes Herdr startup and naming settings in `work-cards.json`, preserving other keys. Native presentation retains its independent document-command settings; they do not configure another summarizer. Native completion uses the returned current records and matching dependencies, not unvalidated model output or historical receipt destinations. The extension and bundled skill describe the same record semantics; they do not retain scratch-format aliases or instructions.

## Persistence boundary

Rust owns record validation, immutable host checks and persistence. TypeScript
exchanges versioned requests and receipts; it never writes catalog files. Report
bases are addressed persisted records, not a remote display projection.

Context returns `catalog_source`, resolved title defaults, the selected Session
and its Stream, and their patch schemas. For each record and actor, the patch
schemas list the fields that actor may change, in display order, with the JSON
schema of each value; the automatic lists already omit protected text. Pi builds
its field menus and the reporting tool schema from them. It reads model output
through the same schemas: empty text becomes null where a field clears and no
change where a field needs text, and a reset clears each short form the model
left out, keeping omitted long fields such as the goal. Rust validates the result, cuts short forms to their
`maxLength`, and Pi shows any refusal Rust returns. Source identity is the
configured host and canonical catalog path, not an inode or
directory-replacement generation. An offline reset stops writers; ordinary
requests do not detect replacement with copied IDs at the same path.

A version-2 apply carries the original target, including its record revision,
allowlisted changes and captured title default. It has no operation ID: a retry
that already landed is recognised from the record. Session edits have no extra captured inputs;
automatic Stream edits name their single captured Session. Pure apply separately
checks conversation-local fields and Stream membership epoch. Pi owns live
branch admission. Omit protected fields before constructing an automatic edit so
a manual title does not discard otherwise valid progress.

A Session inference also proposes references from retained conversation evidence. Save valid progress first, then validate and apply reference intent using the same original capture. Clear additions, updates and drops are automatic; uncertainty is omitted, and restoration and deletion require user direction. A later reference refusal preserves saved progress and stops without retrying. Independent reference changes may save alongside refused rows; complete receipts retain those outcomes.

Generate existing-session progress and stream metadata as separate target-scoped requests, capturing each request's inputs before its inference. This costs an additional model request when both need updating, but avoids making one write invalidate another operation from the same inference. Do not remove real input dependencies or substitute post-inference snapshots to make a stale operation apply. Initial enrollment may save a newly inferred stream and its session together as ordered, separately receipted saves.

Keep record saves separate rather than promising a multi-record rollback transaction. If one save succeeds and a later step fails, return the accepted records and receipts. Publication failure must not turn an accepted local report into an apparent unsaved operation. Session progress never rewrites the stream merely to refresh its timestamp.

## Consequences and boundaries

Automatic reporting removes the routine approval step, but makes captured inputs, manual protection and visible failures essential. A report is a last-known account, not independent verification of every statement. Keeping the card available for guided edits and discussion preserves user control without reinstating a second draft document.

This decision covers Pi integration and the bundled skill. Cross-host handover and saved-arrangement restoration are unsupported. Native movement and card lifetime follow their own application contracts; a conversation change does not invent those operations. The [README](../README.md) owns usage and configuration, and the [testing policy](testing.md) explains acceptance evidence.
