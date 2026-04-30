# AI Workstream Management Spec

Taskwarrior is the source of truth for AI-assisted coding workstreams. Chats, PRs, bookmarks, and notes are evidence attached to a workstream; they are not the canonical state.

## Model

One Taskwarrior task represents one real workstream.

Use the task fields this way:

- `description`: the current next action, written as a concrete imperative.
- `project`: the project or product area, such as `widget-lib`.
- `repo`: repository in `organization/name` form, such as `example-org/monorepo`.
- `bookmark`: jj bookmark or branch name, when known.
- `pr`: GitHub PR number, when known.
- `aisession`: optional AI chat/session id, such as `codex:<id>` or `claude:<slug>`.
- `wait`: when to re-check a non-actionable workstream, when useful.
- annotations: evidence, history, links, prior decisions, validation, and handoff notes.
- tags: exactly one workflow-state tag, plus any non-state topic tags that help filtering.

Workflow-state tags are exactly:

- `doing`: currently active or in progress.
- `ready`: there is a concrete action for the user now.
- `review`: waiting on review, CI, mergeability, or a near-term check-back. Usually pair with `wait:`.
- `blocked`: waiting on an external dependency, another PR, another person, or a decision outside this workstream.
- `parked`: intentionally paused; not expected to move soon.

## Invariants

- Every workstream task has exactly one workflow-state tag.
- The next action lives in `description`, not only in annotations.
- `description` is imperative and concrete: `Check CI on PR #12345 and merge if approved`, not `PR followup`.
- Non-actionable tasks use `review`, `blocked`, or `parked`; add `wait:` when a timed re-check would prevent drift.
- Useful history is preserved in annotations before or while changing the current state.
- If one workstream splits into two real streams, create two tasks rather than overloading one task.
- Be conservative about automatic state changes. State transitions should follow explicit evidence or user intent.
- Use non-state tags for topics only; do not invent extra state tags like `waiting`, `todo`, `active`, or `paused`.

## Workflows

### Capture

Goal: create or update the workstream task for the current repo/chat/PR.

1. Gather context from the repo and chat:
   - repo name, preferably `organization/name`;
   - current jj bookmark, when available;
   - PR number or URL, when available;
   - AI session id or short slug, when useful;
   - current next action;
   - whether the action is actionable now.
2. Search for an existing task by `repo`, `bookmark`, `pr`, or distinctive description text.
3. If an existing task represents the same real stream, update it.
4. If no task exists, create one with required fields and exactly one state tag.
5. Add annotations for evidence: branch, PR URL, validation results, blocker, decisions, or chat/session context.

### Handoff

Goal: make the task enough to resume without reading the whole chat.

1. Replace `description` with the next concrete action.
2. Set exactly one state tag:
   - `ready` if the user can act now.
   - `doing` only for a stream actively being worked right now.
   - `review` for CI/review/near-term check-back.
   - `blocked` for external dependencies.
   - `parked` for intentional pause.
3. Set `wait:` for review, blocked, or parked tasks when a reminder is useful.
4. Annotate the evidence and why the state changed.

### Check

Goal: decide whether a non-active workstream needs attention.

1. Inspect the task, annotations, `wait`, PR, and current repo state.
2. If still waiting, preserve the state and refresh the annotation only when new evidence matters.
3. If actionable now, change the `description` to the next action and move to `ready`.
4. Avoid changing state based only on stale chat memory.

### Split

Goal: turn one overloaded task into two real workstreams.

1. Identify the independent next actions and ownership boundaries.
2. Keep the original task for one stream and narrow its `description`.
3. Create a new task for the other stream with its own state, repo/bookmark/pr fields, and next action.
4. Annotate both tasks with the split reason and cross-reference when practical.

## Raw Taskwarrior Examples

Create:

```bash
task add project:widget-lib repo:example-org/monorepo bookmark:<username>/bump-widget-lib-0.3.2 pr:12345 +review wait:tomorrow "Check CI on PR #12345 and merge if approved"
task <id> annotate "PR: https://github.com/example-org/monorepo/pull/12345"
task <id> annotate "Validation passed: make test"
```

Handoff:

```bash
task <id> modify -doing -ready -review -blocked -parked +blocked wait:2026-05-02 "Re-check upstream PR and unblock the release bump"
task <id> annotate "Blocked on upstream release PR; no local action until it lands"
```

Find likely workstreams:

```bash
task rc.gc=off status:pending '( repo.not: or +doing or +ready or +review or +blocked or +parked )' \
  columns:id,project,repo,bookmark,pr,tags,wait,description
task rc.gc=off +WAITING export
```

Validate state tags manually:

```bash
task rc.gc=off status:pending '( repo.not: or bookmark.not: or pr.not: or aisession.not: or +doing or +ready or +review or +blocked or +parked )' \
  columns:id,project,repo,bookmark,pr,tags,wait,description
task rc.gc=off +WAITING export
```

Taskwarrior reports can hide tasks with `wait:`. Always include `+WAITING export` when checking review, blocked, or parked workstreams that may be waiting.

Inspect any workstream task with missing `project`, missing `repo`, or not exactly one of the five workflow-state tags.
