# Pi stream checkpoint extension

Status: draft updated with agreed selection, lifecycle, and context behavior. Technical design and implementation planning remain open.

## Objective

Let the user update a stream through a side conversation without interrupting the main Pi conversation. The side agent helps establish the current state, decisions, reasons, and next action. It can ask questions, investigate, propose an update, or conclude that no update is needed.

The user sees the proposed scratch changes while chatting and can attach feedback to selected changes. Only explicit approval applies the reviewed draft.

Scratch remains the source of truth. The extension follows the streams skill and `streams format`; it does not introduce a second stream database.

## Scope and assumptions

Agreed behavior:

- manual trigger only: `/checkpoint [stream-ref]`
- a stream reference can be a title or slug
- a successful explicit selection changes the binding for the current Pi session
- an unbound session opens a chooser with recommendations, existing streams, and creation options
- all non-archived streams remain available, with 20 shown initially
- the LLM may recommend an existing stream or creating a named new stream
- confirmed creation runs `streams start` immediately and binds the returned stream, independently of checkpoint approval
- the side agent can discuss and investigate, rather than following a fixed verification checklist
- chat fills the panel by default; a draft summary links to an expanded review view with the same message input
- annotations become chat feedback, not persistent inline review threads
- Close preserves unfinished chat and drafts across restarts; Close + clear discards them without undoing stream creation or binding
- reopening resumes unfinished work with fresh main-session context and the current scratch note
- `cmux: true` runs `streams cmux update -c <slug>` after creation, including starter notes, and after validated scratch updates
- successful Apply validates the update, refreshes CMUX when enabled, posts a main-chat receipt, and closes the panel
- invoking `/checkpoint` after successful Apply starts a new checkpoint conversation

Proposed defaults, not yet agreed:

- one checkpoint panel per Pi session; invoking the command again focuses it
- the side agent starts with the main session's current model and thinking level

The first version does not implement automatic compaction triggers, native concurrent tree branches, a general side-conversation manager, or automatic reference-note edits. It can coexist with `pi-smart-btw`; it does not depend on that extension's private state.

## Configuration

The extension configuration includes a boolean `cmux` setting:

```json
{
  "cmux": true
}
```

Only an explicit `true` enables automatic CMUX updates. With `false` or no setting, the extension makes no CMUX calls. Put this configuration in `~/.pi/agent/checkpoint.json` (or the active Pi agent directory); it is separate from scratch frontmatter.

When enabled, run `streams cmux update -c <slug>` using the actual canonical slug:

- after `streams start` succeeds and the stream is bound, even before a checkpoint draft exists
- after `streams update` validates and writes the approved scratch document

The current workspace is the intended target. Do not search for another workspace, open one, or change focus. Report command failures separately from scratch success. Never undo creation, binding, or a validated update because CMUX is unavailable. Retrying projection runs only the CMUX command.

## Command and stream selection

```text
/checkpoint
/checkpoint parser-ownership-cleanup
/checkpoint Parser ownership cleanup
```

The text after the command is one stream reference, including spaces in a title.

Each invocation obtains current stream metadata using `streams list --json`. Treat a missing executable, nonzero exit, invalid JSON, or malformed result as an error, not an empty list. Do not offer to create a supposedly missing stream based on a failed lookup.

### Explicit reference

Resolve the supplied reference deterministically against the returned records, following the CLI's rules: normalized exact slug, normalized title, then unique substring match. Use the existing CLI resolver where practical rather than maintaining divergent matching rules.

- one match: select it and persist the binding; explicit selection needs no additional confirmation
- several matches: show the matches in the panel and ask the user to choose
- no match: propose creation and show the title, resulting slug, and working directory for confirmation
- invalid reference: explain the problem and allow correction; do not silently invent a replacement

An explicit reference can select a stream in any current status. A failed or cancelled selection leaves the previous binding unchanged. If the open panel has pending work for another stream, resolve that work before switching its target.

### Bound session without a reference

Resolve the stored binding against the current list and reuse it without asking again. Show the selected stream in the panel header. Resume an unfinished checkpoint for that stream if one exists; otherwise start a new conversation.

If the stream was renamed, follow a verifiable CLI compatibility alias to its canonical current record. If it was removed or archived, explain that and return to the chooser. Do not recreate it automatically or fuzzy-match the binding to another stream.

### Unbound session without a reference

Show a loading indicator while gathering recommendations, then open a centered chooser before the right-anchored conversation panel. Its existing-stream choices come from `streams list --json`, never from model-generated names.

The LLM can use the current Pi conversation and stream metadata to recommend:

- an existing stream, with a short reason
- a new stream, with a proposed title and a short reason why existing streams do not fit

For suggested new titles, follow `streams format`'s stable human-readable naming guidance: aim for about six words and 40 characters, putting longer context in the reason rather than the title. This is guidance, not a limit: preserve the model's full title in the selectable option and creation preview even if it runs longer. Manually entered stream titles are unaffected.

Recommendations appear first and are labelled as recommendations, with each reason directly beneath its option. They are followed by other `active` streams, then the remaining non-archived streams. Show title, slug, status, and short state without duplicate entries.

Show up to 20 existing streams initially. Keep the rest accessible through search or Show more. This limits the display, not the list used for matching or recommendations. Creation and manual-entry choices remain visible outside that limit.

Always offer these additional choices:

1. Enter a title or slug.
2. Create a new stream with an AI-suggested title.
3. Create a new stream with a user-specified title.

The suggested new stream is a selectable row; selecting it opens the same creation preview and confirmation as a manually entered title. The user must confirm before creation; an LLM recommendation alone never binds or creates a stream. Existing choices remain usable if recommendation generation fails. Entering a reference uses the same deterministic resolution flow as the command argument.

### Creation

For any creation path, show the final proposed title and slug before asking for confirmation. The user can edit the title. For slug-only input, do not silently expand or rename it; any suggested human-readable title must be visible in the confirmation.

Refresh the list before creation to catch an existing match. On confirmation, immediately run `streams start "<title>" --json` in the intended checkout, without `--current`. The CLI owns slug generation, location discovery, and creation. Persist the session binding to the successful command's actual returned record before beginning the checkpoint discussion. Never interpolate the title into a shell command string.

After successful creation and binding, run `streams cmux update -c <slug>` if `cmux` is `true`. Report a projection failure in the panel and continue the checkpoint discussion with the created stream.

If creation fails, keep the chooser open and the old binding intact. Outside a valid jj or Git checkout, explain the CLI failure and ask for a valid creation location instead of fabricating `repo` or `cwd`.

Creation confirmation commits the choice of title and slug by writing the CLI's starter note and binding the stream. It does not authorize the subsequent checkpoint draft. Close and Close + clear both leave the new note and binding in place. An unfilled starter note correctly records what the user has approved; it is not abandoned work to delete.

## Session binding

The binding belongs to the main Pi session, not to a conversation branch or checkpoint agent.

Store extension metadata containing the owning Pi session ID, canonical stream slug, and scratch path. Restore the latest binding for that owner across the complete session entry list. This is deliberately session-wide: `/tree` navigation must not revert the binding to an older branch's selection.

- reopening the same Pi session restores its binding
- a successful explicit selection replaces it
- a new, forked, or cloned Pi session with a different ID starts unbound, even if it inherited binding entries
- session metadata stays outside model context and scratch frontmatter

When proposing a scratch update, record `pi:<main-session-id>` in `sessions` if it is missing, before showing the draft. Preserve the existing ordering. Do not add the temporary checkpoint agent's ID, which would make `streams resume` target the wrong conversation.

## Side conversation

For a new checkpoint conversation, load:

- the full current main-branch context, including compaction summaries and respecting context edits
- the streams skill and current `streams format` contract
- the actual current scratch markdown, including its frontmatter, if a note exists
- relevant project instructions
- the selected stream, main session ID, source tree position, and working directory

Full current context means what the main agent currently sees, not every historical branch or previously discarded context. If the main agent is streaming, seed from a completed, provider-valid boundary, not a partial response or unmatched tool call. Make omissions explicit rather than presenting a partial snapshot as complete.

The side agent runs independently. The main agent can keep working, and panel prompts do not steer or interrupt it. On a fresh checkpoint chat, automatically ask the side agent to use the included streams skill and current scratch to draft a handoff for review; this opens a conversation, not permission to write scratch. Do not send another opening request when resuming saved chat or an unsent input. The side agent decides what to investigate and what to ask; the extension does not require a PR lookup, CI refresh, test run, or fixed interview sequence.

The side agent may propose a draft proactively or in response to the user. A draft is optional. Chat feedback can change the proposed state or reveal that no scratch change is needed.

### Resuming unfinished work

Reopening loads the same fresh inputs as a new conversation, alongside the saved checkpoint discussion and pending draft. Label that discussion as earlier work. Its assumptions must not override newer main-session evidence or the current scratch file.

Preserve the saved chat, including annotations and their quoted selections. Assemble model input from the latest main-context snapshot and saved side discussion; do not repeatedly append entire main-context snapshots to the transcript.

If scratch changed since the draft was generated, preserve the discussion but regenerate the diff against the current file before allowing Apply. Require approval of that revised draft.

While the panel stays open, its snapshot does not silently track new main messages. Show the source position and provide an explicit refresh action using the same context-assembly rules. Refreshing marks an existing draft as needing review; it does not rewrite earlier chat as though the agent had the new context all along.

Research is read-only. The model can stage a proposal but cannot apply it through ordinary file tools, shell commands, or mutating MCP tools. The extension owns approved creation and apply actions. Tool availability and enforcement must support that boundary; a prompt instruction alone is insufficient.

## Panel and draft feedback

The panel uses a right-anchored Pi TUI overlay. This covers part of the main transcript; it is not a reflowed second Pi column. Only one region has keyboard focus, but both agents can run.

```text
┌─ Stream: Parser ownership cleanup ─────────┐
│ Draft 2: clarify current owner · Review   │
│                                           │
│ Wrapped, streaming side conversation      │
│                                           │
├───────────────────────────────────────────┤
│ Multiline message composer                │
│ Status · Apply · Refresh · Close           │
└───────────────────────────────────────────┘
```

Chat fills the panel by default. Review expands the diff in place, while retaining the same composer so the user can send feedback without leaving it. Before a draft exists, do not reserve space for a diff. Switching between chat and review does not discard either one's scroll position. Support keyboard focus movement between content, composer, and the main editor. Mouse support is optional where Pi supports it.

Stream responses wrap instead of truncating. Show initialization and tool activity promptly; keep the composer editable while the side agent responds, but do not automatically queue text typed during the response. Escape stops an active response before it closes the panel. On a small terminal, prioritize the composer and current content; do not hide the input behind overlay height clipping.

### Annotations

The user selects a changed line, range, or hunk and chooses Comment. The shared input receives an attached reference, not an automatically submitted message. The user types feedback and sends it normally.

An annotation records the draft revision, selected text, and its location. Preserve the quoted text in chat so the comment remains understandable after line numbers change. Comments on an older draft must not be silently attached to a newer one.

The AI responds in chat and can replace the proposed draft. There are no persistent inline threads, resolution states, or partial-hunk approvals in the first version.

## Proposal and apply contract

A proposal targets one selected scratch file. Store its base content or digest, full proposed content, and a revision ID. Render the diff deterministically from those contents; do not ask the LLM to produce the displayed diff separately.

The proposal follows the streams retention rule: preserve what a new human or agent needs to resume or understand intent. Replace stale state instead of appending a chronology. Include frontmatter changes, session registration, and date changes in the visible diff.

Do not edit reference notes or stream-local artifacts as a hidden part of a checkpoint. The AI can discuss a recommendation for those separately.

### Apply

1. The user selects Apply for the displayed draft revision. Freeze that revision and disable duplicate application.
2. Reread the target scratch file and compare it with the draft's base. If it changed, do not overwrite it. Show the conflict and let the side agent propose a new draft against the latest content, requiring new approval.
3. Preview the exact candidate with `streams update --dry-run --if-match <base-sha256> <canonical-slug> --json`, sending the complete Markdown on stdin. This validates against the canonical schema without writing; a changed base rejects the preview.
4. Send the same approved bytes to `streams update --if-match <base-sha256> <canonical-slug> --json`. The CLI validates before replacing the file and rejects a changed base. Validation checks format, not the truth of the AI's claims.
5. If `cmux` is `true`, run `streams cmux update -c <canonical-slug>` to project the validated scratch onto the current workspace. Otherwise make no CMUX call.
6. Update the session binding if its canonical stream identity changed. Do not defer a new stream's initial binding until this step.
7. Add a main-chat receipt with the stream slug, scratch filename, and a short summary of the applied changes. Include any projection warning. Do not trigger a main-agent response; if the main agent is streaming, Pi may defer receipt delivery until the turn ends.
8. Mark this checkpoint complete and close the panel. It is no longer resumable as unfinished work. The next `/checkpoint` starts a new side conversation using the updated scratch and current main context.

A new model response, stream switch, context refresh, or edited target must not cause Apply to write an unseen revision. Applying one snapshot does not claim to cover later main-session work.

The write path must not knowingly overwrite a concurrent edit. Cross-process changes between checking and replacing a file remain an implementation risk; do not claim an unconditional compare-and-swap guarantee from an ordinary atomic rename.

If the CLI fails before writing, keep the panel and draft available and show the error. If it writes successfully but confirmation fails, report the uncertain or partial result rather than retrying blindly. Offer a reviewed correction rather than a silent fix.

If only CMUX refresh fails, record scratch success and the projection warning in the receipt, then close the completed checkpoint. A projection retry must not write scratch again. Failure handling must distinguish completed writes from pending work to prevent duplicate application.

## Panel lifecycle

| State | Behavior |
| --- | --- |
| Choosing | User can select, create, correct a reference, or cancel |
| Chatting | User and agent can discuss; no draft required |
| Draft ready | Chat and diff remain usable; user can annotate, revise, or apply |
| Applying | The approved revision is fixed; duplicate applies are disabled |
| Conflict or error | Preserve chat and proposal; show the failed step and recovery options |
| Applied | Update binding if needed, post the main-chat receipt, mark complete, and close; the next invocation starts fresh |
| Closed | Stop side work and save unfinished chat, draft, and unsent input; reopening resumes with fresh context |
| Cleared | After confirmation, discard checkpoint chat, draft, and unsent input; keep the stream and binding |

Close does not apply or discard a draft. Close + clear is the explicit discard action and asks for confirmation when there is pending work. Neither action undoes confirmed stream creation or successful scratch writes.

Persist unfinished checkpoint state across restarts, scoped to the owning main Pi session and selected stream. Save the side discussion, source-context identity, pending draft and its base, and unsent input. A different session ID must not inherit this pending work. Applied and cleared checkpoints must not reappear as unfinished conversations.

Reload or Pi session replacement preserves saved unfinished work but disposes the side agent and invalidates late results. It must not write into a different session's stream. Closing during generation must leave a resumable conversation, not an unmatched tool call treated as a completed exchange.

`/tree` does not change the session-wide binding. An open panel keeps its labelled source snapshot until an explicit refresh. Reopening refreshes from the then-current branch while preserving the earlier checkpoint discussion.

## Technical direction

Use the Pi SDK and TUI extension APIs, targeting the researched Pi 0.87.1 baseline. An embedded, separate `AgentSession` is the starting option, following the useful parts of `pi-btw`: independent execution, main-context seeding through `SessionManager`, and event-driven UI. Do not mutate `agent.state.messages` to seed history.

Only the main Pi extension runtime appends binding metadata to the main session. The side agent never writes the main session's JSONL file. Reuse TUI layout, scrolling, editor, and width-handling components; do not create a second terminal renderer.

Proposed repository locations:

```text
docs/pi-stream-checkpoint-spec.md                 this specification
home/.pi/agent/extensions/stream-checkpoint/       extension source, after approval
  index.ts                                       commands and lifecycle
  ...                                            selection, proposal, and panel code as needed
tests/stream_checkpoint_test.mjs                  proposed Node tests
home/bin/_streams/                               existing CLI and canonical schema
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
mise run streams:test
```

Proposed extension test command, once the test file exists:

```sh
node --test tests/stream_checkpoint_test.mjs
```

Pi loads TypeScript extensions directly, so this proposal does not add a build step. Confirm typechecking and lint commands in the implementation plan instead of claiming a nonexistent target. Manual TUI verification will use:

```sh
pi --extension ./home/.pi/agent/extensions/stream-checkpoint/index.ts
```

Tests must use temporary scratch files and mocked CLI, model, and CMUX boundaries. They must not create, bind, update, or archive the user's real streams.

## Acceptance criteria

| Scenario | Required result |
| --- | --- |
| Unbound invocation | Show confirmed CLI candidates, advisory recommendations, and all 3 manual or creation choices |
| More than 20 streams | Show 20 initially; search or Show more reaches the full non-archived list, and manual or creation choices stay visible |
| LLM recommends a new stream | Show proposed title and reason; create only after confirmation |
| Title or slug supplied | Resolve deterministically; distinguish one match, ambiguity, invalid input, and absence |
| Listing fails | Explain failure; do not treat it as proof a stream is missing |
| Create is cancelled or fails | Preserve the old binding and do not continue as though creation succeeded |
| Create succeeds | Persist the binding to the actual returned stream immediately, before checkpoint discussion or Apply |
| Later no-argument invocation | Reuse the stored binding without asking again |
| Different explicit stream | Rebind only after successful resolution or creation and handle pending panel work |
| Restart or tree navigation | Restore the main session's latest binding; do not inherit another session ID's binding |
| Main agent is busy | Side discussion works without steering or aborting the main run |
| No draft needed | User can finish the discussion without a scratch write |
| Annotation on draft 1 followed by draft 2 | Chat retains draft 1's quoted selection without misattributing it to draft 2 |
| Chatting while reviewing | Review shows the complete draft with the composer available; switching back preserves chat and does not lose typed input |
| Apply | Write exactly the approved revision, validate, update binding if needed, post slug/path/change-summary receipt without triggering the main agent, and close |
| Invocation after Apply | Start a fresh checkpoint chat with current main context and updated scratch |
| Apply fails | Keep the panel and draft available; distinguish rejected validation, a stale base, a failed write, and uncertain post-write confirmation |
| Concurrent scratch edit | Reject the stale proposal and require approval of a revised diff |
| Side agent attempts a write | Tool permissions prevent bypassing review through files, shell, or external services |
| Close without Apply | Persist unfinished chat, draft, and input; leave scratch and binding unchanged, including a newly created starter note |
| Reopen after Close or restart | Resume saved discussion with full current main context, streams guidance, and current scratch, without duplicating old context snapshots |
| Scratch changes while closed | Preserve chat but regenerate the diff against the current file and require new approval |
| Close + clear | Discard pending checkpoint work after confirmation; keep the stream and binding; next invocation starts fresh |
| Session replacement during side work | Preserve the old session's unfinished work; cancel or ignore late results without changing the new session or its files |
| `cmux: true` on creation | Run `streams cmux update -c <actual-slug>` after creation and binding, even if only the starter note exists |
| `cmux: true` on Apply | Run `streams cmux update -c <canonical-slug>` after the write passes validation |
| `cmux: false` or absent | Make no CMUX calls on creation or Apply |
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

## Remaining technical decisions

The stream list, immediate creation and binding, persistent unfinished conversations, fresh context on resume, and close-after-Apply workflow are agreed.

The implementation plan must resolve configuration location and loading, durable checkpoint storage, and context assembly within the model's context budget. It must also verify candidate validation through the existing streams schema and enforcement of read-only research tools. These constraints do not permit silent context truncation, weaker approval, or overwriting concurrent edits.

## Evidence

- [Streams format](../home/bin/_streams/FORMAT.md): schema, intent-focused body, session ordering, and CMUX projection
- [Streams reference resolution](../home/bin/_streams/resolve.py): deterministic title and slug matching
- [Streams creation](../home/bin/_streams/store.py): starter note, location discovery, and returned stream
- [Streams CLI](../home/bin/_streams/cli.py): JSON flags, checks, and projection commands
- [Local extension test pattern](../tests/wait_extension_test.mjs): Node tests loading TypeScript through jiti
- `~/.agents/skills/streams/SKILL.md`: checkpoint and handoff policy
- Pi 0.87.1 `docs/extensions.md`, `docs/tui.md`, and `docs/sdk.md`: runtime, overlay, and context contracts inspected during research
- `pi-btw` 0.6.1 and `pi-smart-btw` 0.2.8: researched precedents for independent side sessions, not native concurrent main-tree branches
