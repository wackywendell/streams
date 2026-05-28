# Single-Workstream Task Agent Policy

Use this reference for a task-specific AI session, a task CMUX workspace, or a command with an explicit task selector.

Read [core.md](core.md) first. Read [scratch.md](scratch.md) before substantial task work. Read [cmux-integration.md](cmux-integration.md) when a CMUX workspace is open or should be created/updated. Read [cleanup.md](cleanup.md) during cleanup/retro.

## Scope

Single-workstream scope means a task CMUX workspace, a task-specific AI session, or a command with an explicit task selector. The current task/session is responsible for keeping its own Taskwarrior record, scratch pad, CMUX display, and relevant external reality up to date.

The session and scope invariants in [core.md](core.md) apply.

## Workstream Shapes

Not every workstream is an implementation PR. Reviewing someone else's PR, investigating a failure, writing a design note, or doing cleanup can also be a task. Keep one task for the stream while it has one concrete next transition and one coherent resume context.

The task `description` should name the next transition, not the whole stage. Annotations preserve evidence needed to resume or explain state: links, decisions, review asks, validation results, blockers, dependencies, exact errors, and current handoff summaries.

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

For PR-stack lifecycle, state follows who can act next:

- Keep or move the task to `ready` for concrete local/user actions such as signing unsigned commits, fixing a failed pre-push check, pushing the bookmark, marking a draft ready, asking Slack for review, addressing review feedback, or merging when approved and green.
- Move the task to `review` for CI, merge queue, merge check, rerun, review, author-update, or Slack-response waits. Use `wait:` when a re-check time is useful.
- Do not treat a vague GitHub "blocked" label as `blocked` by itself. Read merge check or check-run output directly; if it says something actionable like "1 of 3 commits unsigned," the task is `ready` with a fix/sign/push transition.
- After the PR is merged, keep the task `ready` with a `Cleanup` status/CMUX visual state until scratch archive/retro, local bookmark, workspace, CMUX-closure handoff, and task cleanup have been verified.
- Mark the task done only after the PR is merged and scratch/local cleanup is complete.

## Single-Workstream Commands

`status` means “tell me the currently recorded state.” It is read-only. In single-workstream scope, recap the current/selected task from Taskwarrior, scratch, and already-known session context. Do not check external systems unless asked.

`check` and `refresh` are equal-status synonyms. In single-workstream scope, inspect external state relevant to the current/selected task, such as PR, CI, review, Slack, mergeability, or local jj state when relevant; update scratch, Taskwarrior, and CMUX; recap.

`update`, `checkpoint`, and `handoff` are synonyms for making the current workstream record durable and resumable. Agents should also consider the relevant parts automatically before replies, but only perform updates when material state or useful durable evidence changed.

A checkpoint/update/handoff should:

1. Update scratch `## Goal`, `## Design`, `## Debugging`, `## Test plan`, or `## Learnings` when material evidence or clarified intent changed.
2. Set Taskwarrior `description` to the next concrete transition.
3. Set exactly one workflow-state tag.
4. Set `wait:` when a timed re-check is useful.
5. Update task metadata such as `pr`, `workspace`, `bookmark`, `scratch`, or `aisession` when it changed.
6. Add or refresh a compact `Current handoff:` annotation when a future human/agent may need to resume without reading chat history.
7. Update linked CMUX title/description/color/order when open.
8. Recap current state and uncertainties.

`cleanup` is a ready-phase single-workstream command, not a state tag. It performs final scratch archive/retro, local jj/bookmark/workspace cleanup, prepares the CMUX closure handoff without closing the active conversation unless explicitly confirmed, and completes Taskwarrior when appropriate. See [cleanup.md](cleanup.md).

`split off` creates a separate related workstream from current context without switching the current session. The agent should best-effort decide what belongs in the new task and what remains in the current task; if the boundary is uncertain, make a reasonable split-off task, stay on the current task, and tell the user what is uncertain.

When splitting off:

1. Create a new Taskwarrior task with its own `uuid`, `title`, `slug`, `description`, state tag, and metadata.
2. Create/link its scratch pad when it will be AI-assisted.
3. Put the full goal and relevant context in the new scratch pad, and compact provenance in annotations.
4. Leave the new task's `aisession` blank; do not copy the current task's session id. If the user wants the new task opened immediately, start a fresh AI session for it rather than reusing the conductor/current-task session.
5. Annotate the current task with the split-off task title/uuid when useful.
6. Continue the current task unless the user explicitly says to open/switch/resume the split-off task.

`open`, `switch`, and `resume` explicitly change the current workstream. They create/select the CMUX workspace, ensure/read scratch for active AI work, and resume the recorded `aisession` when one exists. When opening a task with no `aisession`, start a fresh AI session with a task-bootstrap prompt rather than copying the conductor/current-task session id. A suitable prompt is:

```text
You are the dedicated AI session for workstream task <uuid>.

Use the workstreams skill. Treat Taskwarrior as the local source of truth.

Read the role-specific references from the workstreams skill: core.md, task-agent.md, scratch.md, and cmux-integration.md if CMUX is involved. Orient yourself by reading the Taskwarrior task and its scratch pad, creating/repairing the scratch link only if the policy requires it. If the task's aisession is blank, record this new session's prefixed id; if it already has a different aisession, alert the user before changing it.

Summarize the task's goal, current evidence, and next transition. Ask the user only for clarifications that block getting started. If nothing is blocking, state the next concrete action you plan to take and update Taskwarrior/scratch only where the policy says material state or evidence changed.
```

The newly started session should set the task's `aisession` to its own prefixed session id once known.

## Recap Style

Single-workstream status recaps should use three short sections:

1. `Overall`: broad overview of what has been done across the workstream.
2. `Just completed`: what was just completed, with a little more detail than the overview.
3. `Still to do`: remaining concrete work, blockers, or verification.

End with one or two sentences stating the current/next step. For non-status checkpoint/handoff replies, start with a short status label such as `Ready`, `Review`, `Blocked`, `Parked`, `Cleanup`, or `Checkpoint`, then summarize the goal, changed evidence, next transition, and uncertainties.

## Single-Workstream Work Loop

1. Read the Taskwarrior task and scratch pad.
2. Work the current transition.
3. Before replying, consider whether scratch gained useful durable evidence or Taskwarrior/CMUX changed state.
4. If changed, run the update/checkpoint/handoff behavior above; otherwise leave records untouched.
5. Recap current state and next transition.
