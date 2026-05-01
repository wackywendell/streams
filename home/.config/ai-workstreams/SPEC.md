# AI Workstream Management Spec

Taskwarrior is the source of truth for AI-assisted coding workstreams. Chats, PRs, bookmarks, and notes are evidence attached to a workstream; they are not the canonical state.

Do not use Beads or `bd` for workstream state. If older repo instructions mention Beads, treat them as stale and use Taskwarrior instead.

## Model

One Taskwarrior task represents one real workstream.

Use the task fields this way:

- built-in `uuid`: the durable Taskwarrior task identity. Use this for annotations, CMUX descriptions, and handoffs.
- `description`: the next transition that would advance the workstream, written as a concrete imperative.
- `project`: the project or product area, such as `widget-lib`.
- `repo`: repository in `organization/name` form, such as `example-org/monorepo`.
- `workspace`: local jj workspace root path, when known, such as `/Users/<username>/go/src/github.com/example-org/monorepo/workspaces/widget-java`.
- `bookmark`: jj bookmark or branch name, when known.
- `pr`: GitHub PR number, when known.
- `aisession`: optional AI chat/session id, such as `codex:<id>` or `claude:<slug>`. When a CMUX workspace is opened for active work, use this to resume the matching AI session.
- `wait`: when to re-check a non-actionable workstream, when useful.
- annotations: evidence, history, links, prior decisions, validation, and handoff notes.
- tags: exactly one workflow-state tag, plus any non-state topic tags that help filtering.

Taskwarrior numeric ids are report-local conveniences and can change. Use numeric ids for immediate `task <id> ...` commands only after listing current tasks. Use the task `uuid`, and `pr:<number>` when present, for durable references across sessions and tools.

CMUX workspace titles should stay short and stable, usually branch/bookmark slug or workspace basename. CMUX workspace descriptions should start with a status label, then a very short state phrase, then the full PR URL when present, with `task:<uuid>` after the human-facing part.

When updating a task that has an associated CMUX workspace, update that workspace's description, color, and order at the same time as the Taskwarrior state.

CMUX colors encode actionability: `Green` for `doing`, `ready`, and `Cleanup`; `Amber` for `review` and timed waits; `Orange` for `blocked`; `Red` for a temporarily open `parked` workspace. Parked workstreams generally should not have an open CMUX workspace.

When updating several CMUX properties or several workspaces, batch the CMUX actions into one readable shell command block to avoid slow AI/tool round trips. Prefer `&&` when later actions should stop after a failure; use `;` only when every action is safe to run independently.

Workflow-state tags are exactly:

- `doing`: actively in the current human/AI work loop.
- `ready`: there is a concrete next move for the user, including asking for review or fixing known CI/review feedback.
- `review`: waiting on external review, Slack response, CI, mergeability, or a near-term check-back. Usually pair with `wait:`.
- `blocked`: waiting on an external dependency, another PR, another person, or a decision outside this workstream.
- `parked`: intentionally paused; not expected to move soon.

`Cleanup` is a presentation/status-update label and CMUX visual state, not a workflow-state tag. Use the `ready` tag plus a concrete cleanup `description` when the work is functionally done but still needs merge, branch/workspace cleanup, CMUX cleanup, task completion, or final handoff.

## Invariants

- Every workstream task has exactly one workflow-state tag.
- The next transition lives in `description`, not only in annotations.
- `description` is imperative and concrete: `Check CI on PR #12345 and merge if approved`, not `PR followup`.
- Non-actionable tasks use `review`, `blocked`, or `parked`; add `wait:` when a timed re-check would prevent drift.
- Useful history is preserved in annotations before or while changing the current state.
- `workspace` identifies where to resume locally; `bookmark` identifies the code line. Keep both when known.
- If `aisession` is present, it identifies the preferred AI session to resume in CMUX. Do not overwrite it with a nearby or guessed session; annotate uncertainty instead.
- If a dedicated AI session updates or captures a task, it should record its own id in `aisession` when the field is empty. If `aisession` is already set to a different session, do not overwrite it silently; alert the user and annotate the mismatch if it matters for handoff.
- Parked tasks may keep `workspace`/`bookmark` fields as resume metadata, but they do not need an open CMUX workspace.
- If a workstream moves to a different jj workspace, update `workspace` and annotate the move.
- If one workstream splits into two real streams, create two tasks rather than overloading one task.
- Be conservative about automatic state changes. State transitions should follow explicit evidence or user intent.
- Use non-state tags for topics only; do not invent extra state tags like `waiting`, `todo`, `active`, or `paused`.

## Workstream Shapes

Not every workstream is an implementation PR. Reviewing someone else's PR, investigating a failure, writing a design note, or doing cleanup can also be a task. Keep one task for the stream unless it splits into genuinely independent work.

The task `description` should name the next transition, not the whole stage. Annotations preserve the evidence, links, decisions, review asks, validation results, and prior transitions.

Common implementation transitions:

- Clarify what the work is and write down the plan or acceptance criteria.
- Implement the change with AI in the local workspace.
- Open or update a draft PR.
- Self-review the draft PR and address obvious issues. This is still the user's move, not an external review wait.
- Mark the PR ready for review by others.
- Ask for review in Slack, either from a specific person or a channel. This is the handoff from self-review to external review.
- Wait for review, Slack response, CI, mergeability, or a rerun.
- Address review feedback or known CI failures.
- Merge the PR and clean up the local workspace, bookmark, and CMUX workspace.

Common PR-review transitions:

- Inspect PR #123 and linked context.
- Review PR #123 locally and write findings.
- Post review comments on PR #123.
- Ask author a clarifying question on PR #123 or Slack.
- Wait for author updates on PR #123.
- Re-review PR #123 after updates.
- Approve PR #123 if the remaining concerns are resolved.

Common investigation or support transitions:

- Reproduce the failure and record evidence.
- Compare current behavior with the expected contract.
- Summarize findings and decide whether to fix, file, or park.
- Hand off findings in Slack, a PR comment, or an issue.

State tag guidance:

- Use `doing` when the stream is actively in the current human/AI loop, whether that is discovery, implementation, reviewing someone else's PR, feedback handling, investigation, or cleanup.
- Use `ready` when the user has the next move but is not actively doing it now, such as `Self-review draft PR #123`, `Mark PR #123 ready for review`, `Ask for Slack review on PR #123`, `Review PR #456`, or `Fix failing lint job on PR #123`.
- Use `review` when the next step is an external or timed wait, such as `Check review response on PR #123`, `Check author updates on PR #456`, `Check CI rerun on PR #123`, or `Merge PR #123 if approved and green`.
- Use `blocked` only for real external blockers or decisions outside the normal review/CI loop.
- Use `parked` for intentional pauses that are not expected to move soon.

## Status Updates

Overall status updates, especially from the `tasks` CMUX workspace/conductor session, should group workstreams by actionability:

- `Active / Ready`: tasks in `doing` or `ready`, including `Cleanup` items where the next move is final cleanup.
- `Review / Waiting`: tasks in `review`, including review response, Slack response, CI, mergeability, or rerun waits.
- `Blocked`: tasks in `blocked`.
- `Parked`: tasks in `parked`.

For general status updates or "what should I work on" scans, rely on Taskwarrior state and annotations only. Do not check GitHub, PR checks, CI, local git/jj status, or code state unless the user asks for fresh verification, asks about one specific task, or the Taskwarrior record is too ambiguous to answer. The fast path is the pending workstream report plus `+WAITING export`; stale-but-recorded external state should be described as recorded task state, not silently refreshed.

Use these sections explicitly even if one is empty when the user asks for an overall status or asks what to work on. Keep each item concise: task/topic, PR link when present, next transition, and any important wait/blocker.

Individual task status updates should start with one bold status word or short phrase, such as `**Ready**`, `**Blocked**`, `**Review**`, `**Parked**`, or `**Cleanup**`. If a PR is open, include the full PR URL. Then summarize:

- what the task is and why it exists, assuming the user may have forgotten;
- what has been achieved;
- what remains to be achieved;
- what is being waited for, if anything;
- the next transition in `description`.

Distinguish self-review from external review clearly. A draft PR that the user still needs to inspect is `**Ready**` or `**Self-review**`, not `**Review**`. Use `**Review**` when the task is waiting on someone else, Slack response, CI, mergeability, or a timed check.

Finally, give a short 1-2 sentence description of what the user should work on next.

## Command Vocabulary

Use these names for the thin command/prompt layer. Keep the substantive behavior in this shared spec plus the `workstreams` and `cmux` skills.

- `status`: all-workstream status from Taskwarrior only. Include pending and `+WAITING` tasks. Do not inspect CMUX, GitHub, Slack, CI, local git/jj, or code state unless the Taskwarrior record is too ambiguous to answer.
- `task-status`: single-task status from Taskwarrior only. Use a task id, uuid, PR number, workspace slug, or distinctive words to resolve the task. Do not refresh external state unless explicitly asked.
- `refresh`: reassess real-world state and update Taskwarrior. Inspect external state for `review`, `blocked`, and `ready` tasks when they have external hooks such as PRs, CI, Slack review asks, or author-update waits. If the transition is obvious, update the task. If the evidence is ambiguous, annotate the evidence and ask the user before changing state. Print a status update.
- `sync-cmux`: align CMUX from Taskwarrior only. Create/select/update/close/reorder task workspaces, set descriptions and CMUX colors, and avoid opening parked workspaces. Do not refresh external state.
- `sync`: run `refresh`, then `sync-cmux`, so real world -> Taskwarrior -> CMUX are aligned.
- `capture`: create or update one Taskwarrior workstream from arguments or current context. Infer whether the source is free text, current repo, a PR URL, GitHub issue, Jira issue, or other linked work.
- `task-open`: for one task, create or select its CMUX workspace and resume/start the appropriate AI session.
- `task-update`: update one task's next transition, state, wait, annotations, and `aisession` if appropriate; update the linked CMUX workspace at the same time.
- `task-split`: split one overloaded task into two workstreams. The new task gets its own context, its own CMUX workspace when active, and a fresh AI session rather than copying the old `aisession`.

## Workflows

### Capture

Goal: create or update the workstream task for the current repo/chat/PR.

1. Gather context from the repo and chat:
   - repo name, preferably `organization/name`;
   - jj workspace root path from `jj workspace root` or `jj root`, when available;
   - current jj bookmark, when available;
   - PR number or URL, when available;
   - AI session id or short slug, when useful;
   - current next transition;
   - whether the action is actionable now.
2. Search for an existing task by `uuid`, `pr`, `repo`, `workspace`, `bookmark`, or distinctive description text.
3. If an existing task represents the same real stream, update it.
4. If no task exists, create one with required fields and exactly one state tag.
5. If this is being done inside a dedicated AI session, set `aisession` to the current AI session id when it is known and the task has none. If another session is already recorded, alert the user instead of overwriting it silently.
6. Add annotations for evidence: branch, PR URL, validation results, blocker, decisions, or chat/session context.

### Handoff

Goal: make the task enough to resume without reading the whole chat.

1. Replace `description` with the next concrete transition.
2. Verify `workspace` still points to the local jj workspace where the work should resume.
3. Set exactly one state tag:
   - `ready` if the user can act now.
   - `doing` only for a stream actively being worked right now.
   - `review` for CI/review/near-term check-back.
   - `blocked` for external dependencies.
   - `parked` for intentional pause.
4. Set `wait:` for review, blocked, or parked tasks when a reminder is useful.
5. Annotate the evidence and why the state changed.

### Check

Goal: decide whether a non-active workstream needs attention.

1. Inspect the task, annotations, `wait`, PR, and current repo state.
2. If still waiting, preserve the state and refresh the annotation only when new evidence matters.
3. If actionable now, change the `description` to the next transition and move to `ready`.
4. Avoid changing state based only on stale chat memory.

### Refresh

Goal: pull real-world changes into Taskwarrior.

1. Inspect pending workstreams, including hidden `+WAITING` tasks.
2. Reassess tasks in `review` or `blocked`, and `ready` tasks that have external hooks such as PRs, CI, Slack review asks, author-update waits, or mergeability.
3. Use current external evidence where needed: GitHub PR/review/check state, Slack replies, CI/merge check details, or local repo state if the task specifically depends on it.
4. Make obvious transitions directly. For example: a task waiting for review got approval and green CI, so move it to `ready` with a merge/cleanup transition.
5. For ambiguous evidence, annotate what changed and ask the user before changing state. Examples: a vague review comment, unclear CI ownership, or a PR update that may invalidate prior assumptions.
6. After each task state change, update its linked CMUX workspace description, color, and order if one exists.

### Split

Goal: turn one overloaded task into two real workstreams.

1. Identify the independent next transitions and ownership boundaries.
2. Keep the original task for one stream and narrow its `description`.
3. Create a new task for the other stream with its own state, repo/workspace/bookmark/pr fields, and next transition.
4. Put enough context in the new task description and annotations for a fresh AI session to start from scratch: source task uuid, why it split, relevant files, repo/workspace, PR/issue links, decisions already made, validation state, and the desired next transition.
5. Do not copy the old task's `aisession` to the new task. Leave `aisession` unset until the new AI session starts and records its own id.
6. Create a new CMUX workspace for the new task in the appropriate repo or jj workspace directory, and start a new AI session there.
7. Annotate both tasks with the split reason and cross-reference when practical.

## Raw Taskwarrior Examples

Create:

```bash
task add project:widget-lib repo:example-org/monorepo workspace:/Users/<username>/go/src/github.com/example-org/monorepo/workspaces/widget-java bookmark:<username>/bump-widget-lib-0.3.2 pr:12345 +review wait:tomorrow "Check CI on PR #12345 and merge if approved"
task <id> annotate "PR: https://github.com/example-org/monorepo/pull/12345"
task <id> annotate "Validation passed: make test"
task rc.gc=off <id> uuids
```

Handoff:

```bash
task <id> modify -doing -ready -review -blocked -parked +blocked wait:2026-05-02 "Re-check upstream PR and unblock the release bump"
task <id> annotate "Blocked on upstream release PR; no local action until it lands"
```

Find likely workstreams:

```bash
task rc.gc=off status:pending '( repo.not: or +doing or +ready or +review or +blocked or +parked )' \
  columns:id,uuid,project,repo,workspace,bookmark,pr,tags,wait,description
task rc.gc=off +WAITING export
```

Validate state tags manually:

```bash
task rc.gc=off status:pending '( repo.not: or workspace.not: or bookmark.not: or pr.not: or aisession.not: or +doing or +ready or +review or +blocked or +parked )' \
  columns:id,uuid,project,repo,workspace,bookmark,pr,tags,wait,description
task rc.gc=off +WAITING export
```

Taskwarrior reports can hide tasks with `wait:`. Always include `+WAITING export` when checking review, blocked, or parked workstreams that may be waiting.

Inspect any workstream task with missing `project`, missing `repo`, or not exactly one of the five workflow-state tags.
