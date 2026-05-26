# Workstreams Core Policy

Taskwarrior is the source of truth for AI-assisted coding workstreams. One Taskwarrior task represents one real workstream.

Chats, PRs, bookmarks, jj workspaces, CMUX workspaces, and scratch pads are evidence attached to a workstream; they are not canonical state.

## Model

Use Taskwarrior fields this way:

- built-in `uuid`: the durable Taskwarrior task identity. Use this for annotations, exact automation, and handoffs.
- `title`: the stable human-readable workstream name, such as `Grove Workstreams`. Use this as the CMUX workspace title and status-display label.
- `slug`: the stable lowercase/kebab-case machine handle, derived from `title` by default, such as `grove-workstreams`. Use this for deterministic task lookup, AI session names, scratch pad filenames, and remote tmux/workspace handles.
- `scratch`: optional worknotes-relative scratch pad path, usually `scratch/<slug>.md` under `~/worknotes`. Scratch pads are engineering logs and reusable-memory sources, not task state.
- `description`: the next transition that would advance the workstream, written as a concrete imperative.
- `project`: the project or product area, such as `widget-lib`.
- `repo`: repository in `organization/name` form, such as `example-org/monorepo`.
- `workspace`: local jj workspace root path, when known, such as `~/go/src/github.com/example-org/monorepo/workspaces/widget-java`.
- `bookmark`: jj bookmark or branch name, when known.
- `pr`: GitHub PR number, when known.
- `aisession`: optional AI chat/session id in `codex:<id>`, `claude:<id-or-slug>`, or `pi:<id>` form. If present, use it to resume the matching AI session. If absent when opening active work, start a fresh AI session for the task and let that new session record itself.
- `wait`: when to re-check a non-actionable workstream, when useful.
- `priority`: Taskwarrior priority (`H`, `M`, or `L`) when the task's importance should feed urgency. Use `H` for reviews, follow-ups, and anything blocking others; `M` for the user's own work PRs; `L` for personal/tooling/backlog work unless explicitly promoted.
- `due`: a real deadline, not a reminder. Use `wait` for reminders or re-checks.
- computed `urgency`: Taskwarrior's attention-needed score. Do not set it directly; fix task fields when the score feels wrong.
- annotations: focused evidence for resume and state transitions, such as links, decisions, validation, blockers, dependencies, exact errors, session identity, and current handoff notes.
- tags: exactly one workflow-state tag, plus any non-state topic tags that help filtering.

Taskwarrior numeric ids are report-local conveniences and can change. Use numeric ids for immediate `task <id> ...` commands only after listing current tasks. Use the task `uuid`, and `pr:<number>` when present, for durable references across sessions and tools.

For portability, write paths under the user's home directory with `~` in Taskwarrior fields, annotations, scratch pads, CMUX descriptions, prompts, docs, and handoffs. Expand `~` only at command-execution boundaries when a tool requires an absolute path. Existing absolute home paths may remain until touched; when updating one, convert it to `~` form.

Each active workstream should have a stable `title` and `slug`. The title is the human display label. The slug is a readable machine handle that must resolve to at most one pending workstream in automation. Prefer explicit `title`/`slug` fields; when bootstrapping old tasks, derive a suggested title/slug from the jj bookmark basename or workspace directory basename, with owner prefixes such as `user/` removed. The task UUID remains the durable identity.

## Workflow-State Tags

Workflow-state tags are exactly:

- `doing`: actively in the current human/AI work loop.
- `ready`: there is a concrete next move for the user, including asking for review or fixing known CI/review feedback.
- `review`: waiting on external review, Slack response, CI, mergeability, or a near-term check-back. Usually pair with `wait:`.
- `blocked`: waiting on an external dependency, another PR, another person, or a decision outside this workstream.
- `parked`: intentionally paused; not expected to move soon.

`Cleanup` is a presentation/status-update label and CMUX visual state, not a workflow-state tag. Use the `ready` tag plus a concrete cleanup `description` when the work is functionally done but still needs merge, scratch archive/retro, branch/workspace cleanup, CMUX cleanup, task completion, or final handoff.

Use non-state tags for topics only; do not invent extra state tags like `waiting`, `todo`, `active`, or `paused`.

## Priority, Due Dates, and Urgency

Use Taskwarrior urgency as the default conductor ordering signal. Urgency means "needs attention," not always "start coding now." The workflow state tag explains what kind of attention is needed: implement, review, wait, unblock, clean up metadata, or park.

Use `priority` to feed importance into urgency:

- `priority:H`: reviews, author follow-ups, tasks blocking other people, and anything where delayed attention blocks external progress.
- `priority:M`: the user's own active work PRs and normal implementation streams.
- `priority:L`: personal projects, tooling improvements, experiments, and backlog work unless explicitly promoted.

Use `due` only for real deadlines. Do not use `due` for normal review/CI check-ins. Use `wait` for reminders, snoozes, CI/review re-checks, and blocked follow-ups; default normal review waits to the next business day unless the task evidence calls for a same-day re-check or a longer snooze. If a blocked or parked task has high urgency because of an overdue `due`, treat that as a useful attention signal: either clear or correct the deadline, unpark the task, update `wait`, or otherwise fix the task record.

If urgency ordering looks wrong, fix Taskwarrior fields (`priority`, `due`, `wait`, state tags) instead of inventing a parallel ranking system.

## Invariants

- Every workstream task has exactly one workflow-state tag.
- Active and ready workstreams should have `title` and `slug` set. Parked backlog work may omit them until it becomes active.
- Active AI-assisted workstreams should have `scratch` set, or agents should create and link the default `scratch/<slug>.md` before substantial work.
- `slug` is lowercase/kebab-case and should equal `slugify(title)` unless an intentional rename/preservation is annotated.
- `slug` resolves to at most one pending or waiting workstream; automation must fail on duplicate or ambiguous slug matches.
- `workstream-slug doctor --cmux` should report no errors before relying on CMUX title-to-slug resolution broadly; warnings are expected during migration for older tasks that still lack title/slug or have legacy visible `task:<uuid-prefix>` links.
- The next transition lives in `description`, not only in annotations.
- `description` is imperative and concrete: `Check CI on PR #12345 and merge if approved`, not `PR followup`.
- Non-actionable tasks use `review`, `blocked`, or `parked`; add `wait:` when a timed re-check would prevent drift.
- Useful evidence is preserved in annotations before or while changing the current state.
- `workspace` identifies where to resume locally; `bookmark` identifies the code line. Keep both when known, and prefer `~` form for paths under the user's home directory.
- If `aisession` is present, it identifies the preferred AI session to resume and uses `codex:`, `claude:`, or `pi:` prefix form. Preserve an existing different session id; alert the user and annotate the mismatch if it matters for handoff.
- `aisession` belongs to the AI session actually working that task. A conductor or parent-task session that creates a new task, opens a CMUX workspace for it, or starts a new agent command for it must not stamp its own `aisession` on the new task. Leave `aisession` blank so the newly started task agent can record its own session id after it begins.
- If a dedicated AI session updates the task it is actively working, it records its own prefixed session id in `aisession` when the field is empty. If the session id is unknown, add a `Session:` annotation with what is known.
- Parked tasks may keep `workspace`/`bookmark` fields as resume metadata, but they do not need an open CMUX workspace.
- If a workstream moves to a different jj workspace, update `workspace` and annotate the move.
- If one workstream splits into two real streams, create two tasks rather than overloading one task.
- Active and ready tasks with long histories have a current resume point: a `Current handoff:` annotation that summarizes the current state, important decision, validation, constraints, and exact next transition.
- Be conservative about automatic state changes. State transitions should follow explicit evidence or user intent.

## Session and Scope Invariants

Commands are interpreted by scope.

- An AI session working a task has a current workstream.
- Creating or splitting off another task does not switch the current session. Only an explicit `open`, `switch`, or `resume` request changes the current workstream.
- New, split-off, and forked tasks do not inherit the current `aisession`. Leave `aisession` blank until the new task's own AI session starts and records itself.
- Individual task agents are responsible for updating their own Taskwarrior state from external reality and progress. The conductor generally trusts those records.

## Resume Identity

Use four identities deliberately:

- Taskwarrior `uuid`: durable source-of-truth identity. Use it in task annotations and exact automation.
- `title`: human-readable display name. Use it as the CMUX workspace title.
- `slug`: stable machine handle, usually `slugify(title)`, for quick selection, deterministic lookup, AI session names, scratch pad filenames, and remote tmux/workspace names.
- `aisession`: preferred AI session to resume, in `codex:<id>`, `claude:<id-or-slug>`, or `pi:<id>` form.

A slug is convenient, not authoritative. It should resolve to at most one pending workstream before automation acts on it. A CMUX workspace resolves to a task by slugifying its title and finding exactly one pending task whose `slug` matches. During migration, a visible `task:<8-char-uuid-prefix>` link may remain as a fallback, but the target display model omits it from CMUX descriptions.

## Annotation Hygiene

Annotations are evidence for resuming work or justifying state transitions, not a transcript. Prefer compact annotations that preserve decisions, validation, blockers, dependencies, current errors, review submissions, session identity, and CMUX/task links.

Use lightweight prefixes when practical:

- `Current handoff:` for the latest resume summary.
- `Decision:` for user or technical choices that affect future work.
- `Validation:` for commands/results that establish confidence.
- `Blocker:` for the reason a task is blocked.
- `Dependency:` for PRs/issues/releases/work outside the task that affect it.
- `Refresh:` for external checks that changed state, wait time, head SHA, CI/review status, or blocker evidence.
- `CMUX:` for live workspace identity updates.
- `Session:` for AI session identity or uncertainty.
- `Error:` for exact command failures that the next worker may need.

When pausing active work, add or refresh one `Current handoff:` annotation. It should fit in one short paragraph and include the current local/external state, important decision, validation status, exact next transition, and any constraint such as “do not push until the user says to.”

For unchanged refreshes, report the check in command output and update `wait` when useful. Record a `Refresh:` annotation when the check changes task state, changes the next transition, changes a wait, observes a new head/status, or preserves ambiguity that affects future decisions.

If a task accumulates enough annotations that resuming requires archaeology, add a `Current handoff:` summary before adding more detail. If the summary cannot cover the active work cleanly, split the task.

## Split-Off Signals

Split off a separate workstream when related work no longer fits cleanly in the current task. Common signals:

- A follow-up idea has its own goal, owner, repo/workspace, or next transition.
- The current description would need multiple independent actions joined by “and,” “then,” or roadmap labels.
- Implementation work and separate design/backlog work are accumulating in the same task.
- A new worker would need a fresh AI session with different context.
- The active history needs more than a compact `Current handoff:` summary to be resumed safely.
- A parked idea becomes active and contains multiple possible first implementations.

When splitting off, best-effort decide what remains in the current task and what belongs in the new task. If the boundary is uncertain, create a reasonable split-off task, keep working the current task, and tell the user what is uncertain. Creating a split-off task does not switch the current session.
