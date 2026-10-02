# Pi stream checkpoint extension

Status: implemented against Pi 0.99. This document records the accepted behavior, including batched annotations and independent main-session input.

## Objective

Let the user update a stream through a side conversation without interrupting the main Pi conversation. The side agent helps establish the current state, decisions, reasons, and next action. It can ask questions, investigate, propose an update, or conclude that no update is needed.

The user sees the proposed scratch changes while chatting and can attach feedback to selected changes. Only explicit approval applies the reviewed draft.

Scratch remains the source of truth. The extension follows the streams skill and `streams format`; it does not introduce a second stream database.

## Scope and assumptions

Agreed behavior:

- manual trigger only: `/checkpoint [--stream <ref>] [instructions]`
- bare text is instructions; an explicit `--stream` reference can be a slug or quoted title
- a successful explicit selection changes the binding for the current Pi session
- an unbound session opens a chooser with recommendations, existing streams, and creation options
- all non-archived streams remain available, with 20 shown initially
- the LLM may recommend an existing stream or creating a named new stream
- confirmed creation runs `streams start` immediately and binds the returned stream, independently of checkpoint approval
- fresh drafting starts in the background; stream selection and saved unfinished work open their UI immediately
- a user-only transcript card records the invocation snapshot and updates when the draft or reply is ready
- completion opens the panel only if the main composer is empty, whether or not the main agent is still running
- the side agent can discuss and investigate, rather than following a fixed verification checklist
- Chat and Review are separate tabs; Tab switches between them without losing feedback or scroll position
- annotations remain local until sent as one batch, optionally with overall chat feedback
- drafting and user editing are mutually exclusive; reading and minimizing remain available during drafting
- Escape and Alt+P minimize without stopping the side agent; main-session submissions still run
- restart and reload preserve unfinished chat, draft, comments and input; confirmed Clear discards them without undoing stream creation or binding
- reopening resumes unfinished work with fresh main-session context and the current scratch note
- shared `cmux.enabled: true` runs `streams cmux update -c <slug>` after creation, including starter notes, and after validated scratch updates
- successful Apply validates the update, refreshes CMUX when enabled, posts a main-chat receipt, and closes the panel
- invoking `/checkpoint` after successful Apply starts a new checkpoint conversation

Current defaults:

- one checkpoint panel per Pi session; invoking the command without arguments focuses it
- without a model override, the side agent starts with the main session's current model and thinking level

The first version does not implement automatic compaction triggers, native concurrent tree branches, a general side-conversation manager, or automatic reference-note edits. It can coexist with `pi-smart-btw`; it does not depend on that extension's private state.

## Configuration

Shared settings belong to `streams`, not Pi or scratch frontmatter. Use the CLI rather than reading its config file from the extension:

```sh
streams config
streams config --json
streams config get cmux.enabled
streams config set cmux.enabled true
streams config set notes.root ~/notes
streams config set obsidian.vault notes
```

The CLI stores overrides in `$XDG_CONFIG_HOME/streams/config.json`, defaulting to `~/.config/streams/config.json`. `notes.root` defaults to `~/streams`; scratch, archive and artifact paths derive from it. `obsidian.vault` defaults to `streams`. `cmux.enabled` defaults to `false`. The human-readable listing shows the config path and whether each value is configured or a default. Unknown keys, invalid types and malformed files fail without overwriting the file.

The extension fetches effective settings with `streams config --json` when opening a checkpoint and again before Apply. A config failure blocks the operation rather than silently disabling CMUX. `streams update` itself does not automatically refresh CMUX; checkpoint clients explicitly perform that step when enabled.

Keep extension-only settings in `~/.pi/agent/checkpoint.json` (or the active Pi agent directory):

```json
{
  "focusShortcut": "alt+p",
  "selectorModel": { "provider": "<provider>", "id": "<model-id>" },
  "drafterModel": { "provider": "<provider>", "id": "<model-id>" }
}
```

All extension fields are optional. The old `cmux` field is rejected with migration instructions: set `cmux.enabled` through the CLI, then remove `cmux` from `checkpoint.json`. `focusShortcut` defaults to `alt+p`; change it in this file and run `/reload` to register the shortcut. It toggles between the open checkpoint and main editor, but does not open a closed checkpoint. Escape also minimizes. Neither action stops the side agent.

While minimized, the footer shows only checkpoint state and the restore shortcut, such as `Checkpoint · Draft 2 ready · Alt+P`. While expanded, the panel header shows its selected tab and activity; omit duplicate checkpoint footer status. Capitalize modifier names in visible shortcuts.

`selectorModel` controls recommendation generation; `drafterModel` controls the side conversation and draft. Omit either to use the current main-session model for that role. Resolve configured provider/ID pairs through Pi's model registry; report an unavailable configured model instead of silently falling back. Neither setting changes the main session's model.

When enabled, run `streams cmux update -c <slug>` using the actual canonical slug:

- after `streams start` succeeds and the stream is bound, even before a checkpoint draft exists
- after `streams update` validates and writes the approved scratch document

The current workspace is the intended target. Do not search for another workspace, open one, or change focus. Report command failures separately from scratch success. Never undo creation, binding, or a validated update because CMUX is unavailable. Retrying projection runs only the CMUX command.

## Command and stream selection

```text
/checkpoint
/checkpoint let's park this
/checkpoint --stream parser-ownership-cleanup
/checkpoint --stream "Parser ownership cleanup" let's park this
```

Bare text is a message to the checkpoint agent, never a stream lookup, even if it matches an existing title or slug. Only a leading `--stream <ref>` selects a stream. Quote a title containing spaces with single or double quotes; the remaining instructions retain their own quotes, punctuation and line breaks. A missing or malformed reference reports usage without opening or switching a checkpoint.

Send supplied instructions once, after stream selection and side-session initialization. On a fresh chat, they replace the automatic kickoff; on resume, they start a new turn alongside the saved discussion. Keep accepted instructions in pending input until the SDK accepts the message, then show them in Chat during generation. Initialization failure leaves that input available to retry.

If the panel is already open, a message without `--stream` focuses it and sends to the idle conversation. If it is busy, a chooser is open, or there is unsent chat or annotation feedback, report that the instructions were not sent. Do not silently queue a message, merge it with a feedback batch, or overwrite saved input. A bare `/checkpoint` only focuses an open panel.

Opening or switching a checkpoint obtains current stream metadata using `streams list --json`. Treat a missing executable, nonzero exit, invalid JSON, or malformed result as an error, not an empty list. Do not offer to create a supposedly missing stream based on a failed lookup.

Show a cancellable loader during configuration and stream lookup, and while generating recommendations. Esc immediately returns to the main composer and records cancellation. Abort the recommendation request; ignore late lookup results so they cannot open a chooser, change the binding or start drafting. A new `/checkpoint` invocation must not wait for the cancelled lookup to finish.

### Explicit reference

Resolve the supplied reference deterministically against the returned records, following the CLI's rules: normalized exact slug, normalized title, then unique substring match. Use the existing CLI resolver where practical rather than maintaining divergent matching rules.

- one match: select it and persist the binding; explicit selection needs no additional confirmation
- several matches: show the matches in the panel and ask the user to choose
- no match: keep the reference editable and offer “Write your own” to enter a new stream title
- invalid reference: explain the problem and allow correction; do not silently invent a replacement

An explicit reference can select a stream in any current status. If a panel is open, save and dismiss it before resolving the new reference. Confirm a change away from unfinished work; preserve that work under the old stream. Declining leaves the binding unchanged, and `/checkpoint` resumes it. Do not interrupt an in-progress Apply to switch streams.

### Bound session without a reference

Resolve the stored binding against the current list and reuse it without asking again. The selected stream remains available in the conversation context and session binding; the panel header stays `Streams Checkpoint`. Resume an unfinished checkpoint for that stream if one exists; otherwise start a new conversation.

If the stream was renamed, follow a verifiable CLI compatibility alias to its canonical current record. If it was removed or archived, explain that and return to the chooser. Do not recreate it automatically or fuzzy-match the binding to another stream.

### Unbound session without a reference

Show a loading indicator while gathering recommendations, then open a centered chooser. After selection, fresh drafting starts in the background; saved unfinished work opens the right-anchored conversation panel. Its existing-stream choices come from `streams list --json`, never from model-generated names.

The LLM can use the current Pi conversation and stream metadata to recommend:

- an existing stream, with a short reason
- a new stream, with a proposed title and a short reason why existing streams do not fit

For suggested new titles, fewer words are better: 2 to 4 words usually suffice. Use the shortest stable, human-readable title that distinguishes the work. Put fuller context in the reason rather than the title. This is guidance, not a limit: preserve longer suggestions in the selectable option and title editor. Manually entered titles are unaffected.

Recommendations appear first and are labelled as recommendations, with each reason directly beneath its option. Visually separate the new recommendation from existing streams; distinguish titles, statuses, and supporting state with theme colors and italics. They are followed by other `active` streams, then the remaining non-archived streams. Show title, slug, status, and short state without duplicate entries.

Show up to 20 existing streams initially. Keep the rest accessible through search or Show more. This limits the display, not the list used for matching or recommendations. Creation and manual-entry choices remain visible outside that limit.

Always offer these additional choices:

1. Enter a title or slug.
2. Create a new stream with an AI-suggested title.
3. “Write your own”, opening a blank new-stream title field.

Keep “Write your own” visible below the list, including when search finds no matches. Selecting a suggested new stream opens the same title editor, prefilled with the suggestion. The first Enter opens editing; a second Enter submits the title and creates the stream. There is no y/n prompt. Esc from editing returns to the choices without creating anything or losing the search text.

An LLM recommendation alone never binds or creates a stream. Existing choices remain usable if recommendation generation fails. Typing a reference in the search field still uses the deterministic resolution flow from `--stream`. Ctrl+N opens the new-stream title editor with the search text prefilled.

### Creation

For any creation path, let the user edit the title before submitting it with Enter. Reject an empty title. Do not silently expand or rename slug-only input.

On submission, validate the title and obtain its canonical slug through `streams start --dry-run`. Refresh the list to catch an existing match, then run `streams start "<title>" --json` in the intended checkout, without `--current`. Keep input locked during creation to prevent duplicate submissions. The CLI owns slug generation, location discovery, and creation. Persist the session binding to the successful command's actual returned record before beginning the checkpoint discussion. Never interpolate the title into a shell command string.

After successful creation and binding, run `streams cmux update -c <slug>` if shared `cmux.enabled` is `true`. Report a projection failure in the panel and continue the checkpoint discussion with the created stream.

If creation fails, keep the chooser open and the old binding intact. Outside a valid jj or Git checkout, explain the CLI failure and ask for a valid creation location instead of fabricating `repo` or `cwd`.

Submitting the edited title writes the CLI's starter note and binds the returned stream. It does not authorize the subsequent checkpoint draft. Minimizing, restarting and Clear all leave the new note and binding in place. An unfilled starter note correctly records what the user has approved; it is not abandoned work to delete.

## Session binding

The binding belongs to the main Pi session, not to a conversation branch or checkpoint agent.

Store extension metadata containing the owning Pi session ID, canonical stream slug, and scratch path. Restore the latest binding for that owner across the complete session entry list. This is deliberately session-wide: `/tree` navigation must not revert the binding to an older branch's selection.

- reopening the same Pi session restores its binding
- a successful explicit selection replaces it
- a new, forked, or cloned Pi session with a different ID starts unbound, even if it inherited binding entries
- session metadata stays outside model context and scratch frontmatter

When proposing a scratch update, record `pi:<main-session-id>` in `sessions` if it is missing, before showing the draft. Preserve the existing ordering. Do not add the temporary checkpoint agent's ID, which would make `streams resume` target the wrong conversation.

## Invocation marker and readiness

Capture the main-branch context and system prompt when opening or switching a checkpoint, before asynchronous setup or stream selection. Record a user-only card at that point in the transcript. Use that same frozen context for recommendations and the side session. Later main-session messages do not enter the snapshot.

Update the card in place as setup and drafting progress. Show `Draft N ready` after a completed response with a draft, or `Reply ready` without one. Show `Needs attention` for errors and `Stopped` for interrupted output. The footer repeats readiness while the panel is hidden. Alt+P can open the active panel during drafting or after completion.

Keep a fresh drafting panel hidden initially. Open stream selection and saved unfinished work without waiting for a new response. When a side response finishes successfully, check the main composer once. Open the panel if its text is exactly empty, including while the main agent is running. Otherwise, leave the composer and focus unchanged. Clearing the composer later does not trigger an opening. Stopping, failing or shutting down a response does not auto-open the panel.

Ctrl+O expands the card to show the snapshot timestamp, main-session ID, source tree entry, supplied instructions and omitted incomplete-message count. Store lifecycle changes as append-only custom entries and render their latest state at the original marker. These entries stay outside model context; the separate Apply receipt still shares its minimal summary at the time of the write. Reload reconstructs saved status without starting work; interrupted runs must not remain labelled as drafting.

Refresh captures a new snapshot and adds a new source marker. Simply focusing an already open checkpoint neither refreshes context nor adds a marker.

Use state changes, not an animated transcript spinner. Pi's regular terminal mode redraws the transcript when an updated card is above the visible viewport; fullscreen mode updates its managed transcript.

## Side conversation

For a new checkpoint conversation, load:

- the full current main-branch context, including compaction summaries and respecting context edits
- the streams skill and current `streams format` contract
- the actual current scratch markdown, including its frontmatter, if a note exists
- relevant project instructions
- the selected stream, main session ID, source tree position, and working directory

Full current context means what the main agent currently sees, not every historical branch or previously discarded context. Use Pi's model-message conversion to preserve branch and compaction summaries and respect excluded shell output. If the main agent is streaming, seed from a completed, provider-valid boundary, not a partial response or unmatched tool call. Make omissions explicit rather than presenting a partial snapshot as complete.

The side agent runs independently. The main agent can keep working, and panel prompts do not steer or interrupt it. On a fresh checkpoint chat without supplied instructions, automatically ask the side agent to use the included streams skill and current scratch to draft a handoff for review; this opens a conversation, not permission to write scratch. Do not send another opening request when resuming saved chat or an unsent input. The side agent decides what to investigate and what to ask; the extension does not require a PR lookup, CI refresh, test run, or fixed interview sequence.

The side agent may propose a draft proactively or in response to the user. A draft is optional. After staging, its chat reply briefly summarizes substantive changes since the previous scratch note, not the mechanics of staging, routine verification, or the fact that scratch was not changed; mention material failures or blockers. Chat feedback can change the proposed state or reveal that no scratch change is needed.

### Resuming unfinished work

Reopening loads the same fresh inputs as a new conversation, alongside the saved checkpoint discussion and pending draft. Label that discussion as earlier work. Its assumptions must not override newer main-session evidence or the current scratch file.

Preserve the saved chat, including annotations and their quoted selections. Assemble model input from the latest main-context snapshot and saved side discussion; do not repeatedly append entire main-context snapshots to the transcript.

If scratch changed since the draft was generated, preserve the discussion but regenerate the diff against the current file before allowing Apply. Require approval of that revised draft.

While the panel stays open, its snapshot does not silently track new main messages. Record the source position and provide an explicit refresh action using the same context-assembly rules. Refresh preserves the existing draft, saved comments and unfinished annotation. Report success as information, not an attention-needed state. A changed scratch file remains a warning and blocks stale Apply.

Research is read-only. The model can stage a proposal but cannot apply it through ordinary file tools, shell commands, or mutating MCP tools. The extension owns approved creation and apply actions. Tool availability and enforcement must support that boundary; a prompt instruction alone is insufficient.

## Panel and draft feedback

The panel uses a right-anchored Pi TUI overlay. This covers part of the main transcript; it is not a reflowed second Pi column. Only one region has keyboard focus, but both agents can run.

```text
┌────────────────────────────────────────────────────────────┐
│ Streams Checkpoint  [Chat]  Review · 2  ⠋ Drafting          │
├────────────────────────────────────────────────────────────┤
│ You                                                        │
│ [3 annotations · Draft 1]                                  │
│ Please address these comments.                              │
│                                                            │
│ Checkpoint                                                 │
│ Wrapped response…                                          │
│ ────────────────────────────────────────────────────────── │
│ Multiline composer                                         │
│ ────────────────────────────────────────────────────────── │
│ Tab · Ctrl+C Stop · Alt+P Hide                             │
└────────────────────────────────────────────────────────────┘
```

Use one heading-styled header with Chat and Review tabs, a draft revision only when one exists, and activity only while running. Keep a separator below this header visible in both tabs, including before the first message. When no content or notices separate the header from the composer, use the editor's existing top border instead of adding a second divider. Preserve its input-scroll indicator. Do not repeat the stream title, draft summary, focus labels or idle “Ready” text.

Start compact and grow with content to the terminal height limit. Chat is the default tab. Tab switches to Review, where arrows select logical diff lines and Alt+K edits a comment. Chat input and the annotation editor hold separate text. Switching tabs preserves both. Page Up/Down scrolls the selected tab; in Review it also moves the selected line so subsequent arrows stay on the visible page.

Responses wrap instead of truncating. Submitted feedback appears in chat as soon as the SDK accepts it, even before the first response token. Hide the automatic kickoff instruction from visible chat. Show one animated activity label for setup, drafting, tools, Apply or Refresh.

While an operation runs, disable chat edits, annotation edits and submissions. Keep tab switching, reading and minimizing available. Ctrl+C stops drafting; it does not cancel Apply or Refresh. Escape and Alt+P always minimize rather than stopping the agent. On small terminals, keep input visible within the overlay height limit. Show only relevant shortcuts, without repeating Enter/Shift+Enter hints.

### Annotations

While idle, select a logical diff line in Review and press Alt+K. Enter saves the comment locally; Ctrl+C cancels its unfinished edit. Editing an existing comment to empty removes it. Show saved comments beneath their selected lines. Saving a comment never starts a model turn.

Each annotation contains the scratch path, immutable draft revision, original or proposed document side, exact line number, nearest heading and a numbered surrounding excerpt. Deleted lines refer to the original document. Wrapped and repeated lines retain their distinct document anchors.

Switch to Chat to add optional overall feedback and send the batch once. Chat shows a collapsed count, such as `[3 annotations · Draft 1]`; Alt+A expands the evidence. The model receives every comment and anchor as ordinary message text. Preserve feedback until the SDK accepts it, then clear only the accepted composer text and annotation batch.

The AI may replace the draft after receiving feedback. Keep sent annotations with their original chat turn, never attached to a later revision. Refuse Apply while chat feedback or an unfinished annotation remains unsent. There are no resolution states or partial-hunk approvals.

## Proposal and apply contract

A proposal targets one selected scratch file. Store its base content or digest, full proposed content, and a revision ID. Render the diff deterministically from those contents; do not ask the LLM to produce the displayed diff separately.

The proposal follows the streams retention rule: preserve what a new human or agent needs to resume or understand intent. Replace stale state instead of appending a chronology. Include frontmatter changes, session registration, and date changes in the visible diff.

Do not edit reference notes or stream-local artifacts as a hidden part of a checkpoint. The AI can discuss a recommendation for those separately.

### Apply

1. The user selects Apply for the displayed draft revision. Freeze that revision and disable duplicate application.
2. Reread the target scratch file and compare it with the draft's base. If it changed, do not overwrite it. Show the conflict and let the side agent propose a new draft against the latest content, requiring new approval.
3. Preview the exact candidate with `streams update --dry-run --if-match <base-sha256> <canonical-slug> --json`, sending the complete Markdown on stdin. This validates against the canonical schema without writing; a changed base rejects the preview.
4. Send the same approved bytes to `streams update --if-match <base-sha256> <canonical-slug> --json`. The CLI validates before replacing the file and rejects a changed base. Validation checks format, not the truth of the AI's claims.
5. If shared `cmux.enabled` is `true`, run `streams cmux update -c <canonical-slug>` to project the validated scratch onto the current workspace. Otherwise make no CMUX call.
6. Update the session binding if its canonical stream identity changed. Do not defer a new stream's initial binding until this step.
7. Add a main-chat receipt with the stream slug, scratch filename, a short summary of the applied changes and CMUX outcome (`updated`, `disabled` or `failed`). Keep raw diagnostic output outside model context. Do not trigger a main-agent response; if the main agent is streaming, Pi may defer receipt delivery until the turn ends.
8. Mark this checkpoint complete and close the panel. It is no longer resumable as unfinished work. The next `/checkpoint` starts a new side conversation using the updated scratch and current main context.

A new model response, stream switch, context refresh, or edited target must not cause Apply to write an unseen revision. Applying one snapshot does not claim to cover later main-session work.

The write path must not knowingly overwrite a concurrent edit. Cross-process changes between checking and replacing a file remain an implementation risk; do not claim an unconditional compare-and-swap guarantee from an ordinary atomic rename.

If the CLI fails before writing, keep the panel and draft available and show the error. If it writes successfully but confirmation fails, report the uncertain or partial result rather than retrying blindly. Offer a reviewed correction rather than a silent fix.

If only CMUX refresh fails, record scratch success and the projection warning in the receipt, then close the completed checkpoint. A projection retry must not write scratch again. Failure handling must distinguish completed writes from pending work to prevent duplicate application.

### Receipt and diagnostics

Render the receipt as a compact box in the main transcript. Pi's normal tool-expansion action (Ctrl+O by default) reveals command arguments, exit statuses, output and projection warnings. CMUX failure remains visible in the collapsed box alongside scratch success.

Use custom-message `content` for the minimal shared receipt and `details` for diagnostic data. Pi persists both, but sends only `content` to the model. Expanding the box changes presentation, not model context. Do not trigger a model turn to announce completion.

Retain the latest 16 CLI calls from the current panel lifetime, with stdout and stderr limited to 2,048 characters each plus a truncation marker. Never record the Markdown passed on stdin. Capture successful stderr and structured update warnings; show them through notifications and retain them in receipt details. Errors remain visible in the panel or notifications. These diagnostics are not a complete historical log across checkpoint restarts.

## Panel lifecycle

| State | Behavior |
| --- | --- |
| Choosing | User can select, create, correct a reference, or cancel |
| Editing | The agent is idle; the user can compose chat and batch annotations, with no draft required for chat |
| Drafting | Disable user edits and sends; keep reading, minimizing and Ctrl+C Stop available |
| Draft ready | The user can review, annotate, send feedback or apply, but not apply with unsent feedback |
| Applying | The approved revision is fixed; duplicate applies are disabled |
| Conflict or error | Preserve chat and proposal; show the failed step and recovery options |
| Applied | Update binding if needed, post the main-chat receipt, mark complete, and close; the next invocation starts fresh |
| Minimized | Hide the overlay and focus the main editor without stopping side work; submissions run in the main session |
| Closed by shutdown or reload | Stop side work and save unfinished state; reopening resumes with fresh context |
| Cleared | After confirmation, discard checkpoint chat, draft, annotations and input; keep the stream and binding |

Minimizing neither applies nor discards a draft. Alt+C Clear is the explicit discard action and requires confirmation. It closes the panel without undoing stream creation or successful scratch writes.

Persist unfinished checkpoint state across restarts, scoped to the owning main Pi session and selected stream. Save the side discussion, source-context identity, pending draft and its base, annotation batches, unfinished comment and chat input. A different session ID must not inherit this pending work. Applied and cleared checkpoints must not reappear as unfinished conversations.

Reload or Pi session replacement preserves saved unfinished work but disposes the side agent and invalidates late results. It must not write into a different session's stream. Closing during generation must leave a resumable conversation, not an unmatched tool call treated as a completed exchange.

`/tree` does not change the session-wide binding. An open panel keeps its labelled source snapshot until an explicit refresh. Reopening refreshes from the then-current branch while preserving the earlier checkpoint discussion.

## Technical direction

Use the Pi SDK and TUI extension APIs, targeting the researched Pi 0.87.1 baseline. An embedded, separate `AgentSession` is the starting option, following the useful parts of `pi-btw`: independent execution, main-context seeding through `SessionManager`, and event-driven UI. Do not mutate `agent.state.messages` to seed history.

The command starts the overlay task and returns immediately, releasing Pi's serialized prompt queue. Session shutdown cancels pending confirmations, dismisses the overlay and waits for controller cleanup. Do not await the overlay lifetime inside the command handler.

Only the main Pi extension runtime appends binding metadata to the main session. The side agent never writes the main session's JSONL file. Reuse TUI layout, scrolling, editor, and width-handling components; do not create a second terminal renderer.

Repository locations:

```text
docs/pi-stream-checkpoint-spec.md                 this specification
extensions/stream-checkpoint/                     extension source
  index.ts                                       command and overlay lifecycle
  conversation.ts                                model operations and durable feedback
  panel.ts                                       Chat/Review rendering and idle input
  review.ts                                      diffs, immutable anchors and prompt formatting
  view.ts                                        transcript projection and status labels
  chooser.ts                                     stream selection UI
  agent.ts                                       restricted SDK session and context
  core.ts                                        persistence, binding and guarded Apply
  streams.ts                                     argument-array CLI boundary and diagnostics
  receipt.ts                                     shared receipt and expandable user-only details
  journal.ts                                     source markers and append-only lifecycle status
tests/stream_checkpoint_test.mjs                  Node regression tests
src/_streams/                                     existing CLI and canonical schema
```

Keep deterministic selection, proposal state, and diff generation separate from LLM calls and rendering. Follow existing TypeScript extension conventions. For example, proposal state should carry the version the user reviewed:

```typescript
interface CheckpointDraft {
    revision: number;
    targetPath: string;
    baseContent: string;
    proposedContent: string;
}
```

Module boundaries, dependencies, durable checkpoint storage, fresh-context assembly on resume, model resource loading, and validation integration belong in the implementation plan. Do not copy the full `pi-btw` implementation or add a general side-chat framework for this single workflow.

## Commands and verification

Existing runtime contracts:

```sh
streams list --json
streams show "Parser ownership cleanup" --json
streams format
streams start "Parser ownership cleanup" --json
streams update --dry-run --if-match <base-sha256> parser-ownership-cleanup --json < proposed.md
streams update --if-match <base-sha256> parser-ownership-cleanup --json < proposed.md
streams cmux update -c parser-ownership-cleanup
```

Execute CLI commands with separate arguments. Load the scratch body from the returned path: `streams show --json` returns metadata, not the markdown body.

Existing CLI test target:

```sh
mise run test:python
```

Extension test command:

```sh
node --test tests/stream_checkpoint_test.mjs
```

Pi loads TypeScript extensions directly, so this proposal does not add a build step. Confirm typechecking and lint commands in the implementation plan instead of claiming a nonexistent target. Manual TUI verification will use:

```sh
pi --extension ./extensions/stream-checkpoint/index.ts
```

Tests must use temporary scratch files and mocked CLI, model, and CMUX boundaries. They must not create, bind, update, or archive the user's real streams.

## Acceptance criteria

| Scenario | Required result |
| --- | --- |
| Unbound invocation | Show confirmed CLI candidates, advisory recommendations, and all 3 manual or creation choices |
| More than 20 streams | Show 20 initially; search or Show more reaches the full non-archived list, and manual or creation choices stay visible |
| LLM recommends a new stream | Show title and reason; Enter opens title editing and a second Enter creates |
| Write your own | Always show the option; open a blank title field and preserve the search when returning |
| Esc during lookup or recommendations | Return to the composer immediately; ignore late results and allow a new invocation |
| `--stream` title or slug supplied | Resolve deterministically; distinguish one match, ambiguity, invalid input, and absence |
| Bare instructions | Use the bound stream or retain the message through the chooser; send once instead of the fresh kickoff |
| Instructions on resume | Preserve saved history and send a new visible turn without another kickoff |
| Busy panel or unsent feedback | Report instructions as not sent; do not queue, overwrite or send an existing feedback batch |
| Invalid `--stream` syntax | Report usage without opening a panel or changing the binding |
| Listing fails | Explain failure; do not treat it as proof a stream is missing |
| Create is cancelled or fails | Preserve the old binding and do not continue as though creation succeeded |
| Create succeeds | Persist the binding to the actual returned stream immediately, before checkpoint discussion or Apply |
| Later no-argument invocation | Reuse the stored binding without asking again |
| Different explicit stream | Rebind only after successful resolution or creation and handle pending panel work |
| Restart or tree navigation | Restore the main session's latest binding; do not inherit another session ID's binding |
| Invocation snapshot | Record the source before setup; recommendations and drafting exclude later main-session messages |
| Fresh drafting | Keep the panel hidden; show a user-only transcript marker and footer status |
| Saved unfinished work | Open immediately, preserving draft, chat and unsent feedback |
| Completion with an empty composer | Open the panel even if the main agent is still running |
| Completion with composer text | Keep focus and text untouched; later clearing does not auto-open |
| Status replay | Update the original card, persist status outside model context and distinguish interrupted work after reload |
| Main agent is busy | Side discussion works without steering or aborting the main run |
| No draft needed | User can finish the discussion without a scratch write |
| Annotation on draft 1 followed by draft 2 | Chat retains draft 1's quoted selection without misattributing it to draft 2 |
| Batched feedback | Save several anchored comments without a model turn; send them with optional chat feedback as one turn |
| Drafting/editing exclusion | Drafting blocks chat and annotation edits; reading, minimizing and Stop remain available |
| Submitted feedback | Show the accepted user message during generation with a collapsed annotation count; the model receives full evidence |
| Chat and Review tabs | Switching tabs preserves chat input, unfinished comments and scroll position |
| Minimized panel | Escape and Alt+P do not stop the side agent; main-session submissions run normally |
| Refresh succeeds | Preserve feedback and draft; show informational status rather than “Needs attention” |
| Apply | Write exactly the approved revision, validate, update binding if needed, post slug/path/change-summary receipt without triggering the main agent, and close |
| Invocation after Apply | Start a fresh checkpoint chat with current main context and updated scratch |
| Apply fails | Keep the panel and draft available; distinguish rejected validation, a stale base, a failed write, and uncertain post-write confirmation |
| Concurrent scratch edit | Reject the stale proposal and require approval of a revised diff |
| Side agent attempts a write | Tool permissions prevent bypassing review through files, shell, or external services |
| Shutdown without Apply | Persist unfinished chat, draft, comments and input; leave scratch and binding unchanged, including a newly created starter note |
| Reopen after shutdown or reload | Resume saved discussion with full current main context, streams guidance and current scratch, without duplicating old context snapshots |
| Scratch changes while closed | Preserve chat but regenerate the diff against the current file and require new approval |
| Clear | Discard pending checkpoint work after confirmation; keep the stream and binding; next invocation starts fresh |
| Session replacement during side work | Preserve the old session's unfinished work; cancel or ignore late results without changing the new session or its files |
| `cmux.enabled: true` on creation | Run `streams cmux update -c <actual-slug>` after creation and binding, even if only the starter note exists |
| `cmux.enabled: true` on Apply | Run `streams cmux update -c <canonical-slug>` after the write passes validation |
| `cmux.enabled: false` or absent | Make no CMUX calls on creation or Apply |
| Shared configuration | Fetch through `streams config --json`; never read its file from Pi or store model/shortcut settings in streams config |
| Expand receipt | Show diagnostic details through Pi's tool-expansion control without adding them to model context |
| CMUX update fails | Preserve the scratch write and binding, report projection failure, and never target a fallback workspace |
| CMUX retry | Retry only projection, without recreating the stream or rewriting scratch |
| Compaction | Do not start a checkpoint automatically |

Cover selection and binding with unit tests; cover draft and apply transitions with temporary-file integration tests. Check the panel manually in regular and fullscreen modes, narrow terminals, resize events, streaming, Unicode text, focus changes, and cancellation.

## Boundaries

Always:

- use CLI output to establish which streams exist
- label recommendations and proposals as such
- show all proposed scratch changes before applying
- keep the main session ID as the resume identity
- preserve user edits and report partial success accurately

Ask first:

- create a stream, apply a proposal, or discard pending panel work
- choose among ambiguous streams or creation locations
- expand scope to reference notes, external mutations, or automatic checkpointing

Never:

- infer permission to write from a recommendation or ordinary chat reply
- create or update a stream automatically on compaction
- use a failed lookup as evidence of absence
- write unseen draft changes or silently repair an applied note
- start a new Pi conversation when the user chooses a new stream

## Implementation boundaries

Pending checkpoint state is stored atomically under the active agent directory, keyed by main-session owner and stream. The isolated side session exposes local read, grep, find and ls tools plus the proposal tool; external-service research adapters are not included.

Context preflight uses Pi's message-token estimate and rejects input above 90% of the model window with an actionable error. It does not silently truncate context. Guarded Apply uses the streams CLI's schema and base-content hash; an atomic rename alone does not guarantee cross-process compare-and-swap.

## Evidence

- [Streams format](../src/_streams/FORMAT.md): schema, intent-focused body, session ordering, and CMUX projection
- [Streams reference resolution](../src/_streams/resolve.py): deterministic title and slug matching
- [Streams creation](../src/_streams/store.py): starter note, location discovery, and returned stream
- [Streams CLI](../src/_streams/cli.py): JSON flags, checks, and projection commands
- Local extension test pattern (`tests/wait_extension_test.mjs` in the original dotfiles repository): Node tests loading TypeScript through jiti
- `~/.agents/skills/streams/SKILL.md`: checkpoint and handoff policy
- Pi 0.87.1 `docs/extensions.md`, `docs/tui.md`, and `docs/sdk.md`: runtime, overlay, and context contracts inspected during research
- `pi-btw` 0.6.1 and `pi-smart-btw` 0.2.8: researched precedents for independent side sessions, not native concurrent main-tree branches
