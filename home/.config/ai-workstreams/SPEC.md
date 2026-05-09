# AI Workstream Management Spec

Taskwarrior is the source of truth for AI-assisted coding workstreams. Chats, PRs, bookmarks, and notes are evidence attached to a workstream; they are not the canonical state.

## Model

One Taskwarrior task represents one real workstream.

Use the task fields this way:

- built-in `uuid`: the durable Taskwarrior task identity. Use this for annotations, exact automation, and handoffs.
- `title`: the stable human-readable workstream name, such as `Grove Workstreams`. Use this as the CMUX workspace title and status-display label.
- `slug`: the stable lowercase/kebab-case machine handle, derived from `title` by default, such as `grove-workstreams`. Use this for deterministic task lookup, AI session names, scratch pad filenames, and remote tmux/workspace handles.
- `scratch`: optional worknotes-relative scratch pad path, usually `scratch/<slug>.md` under `~/worknotes`. Scratch pads are engineering logs and reusable-memory sources, not task state.
- `description`: the next transition that would advance the workstream, written as a concrete imperative.
- `project`: the project or product area, such as `widget-lib`.
- `repo`: repository in `organization/name` form, such as `example-org/monorepo`.
- `workspace`: local jj workspace root path, when known, such as `/Users/<username>/go/src/github.com/example-org/monorepo/workspaces/widget-java`.
- `bookmark`: jj bookmark or branch name, when known.
- `pr`: GitHub PR number, when known.
- `aisession`: optional AI chat/session id in `codex:<id>`, `claude:<id-or-slug>`, or `pi:<id>` form. When a CMUX workspace is opened for active work, use this to resume the matching AI session.
- `wait`: when to re-check a non-actionable workstream, when useful.
- `priority`: Taskwarrior priority (`H`, `M`, or `L`) when the task's importance should feed urgency. Use `H` for reviews, follow-ups, and anything blocking others; `M` for the user's own work PRs; `L` for personal/tooling/backlog work unless explicitly promoted.
- `due`: a real deadline, not a reminder. Use `wait` for reminders or re-checks.
- computed `urgency`: Taskwarrior's attention-needed score. Do not set it directly; fix task fields when the score feels wrong.
- annotations: focused evidence for resume and state transitions, such as links, decisions, validation, blockers, dependencies, exact errors, session identity, CMUX links, and current handoff notes.
- tags: exactly one workflow-state tag, plus any non-state topic tags that help filtering.

Taskwarrior numeric ids are report-local conveniences and can change. Use numeric ids for immediate `task <id> ...` commands only after listing current tasks. Use the task `uuid`, and `pr:<number>` when present, for durable references across sessions and tools.

Each active workstream should have a stable `title` and `slug`. The title is the human display label. The slug is a readable machine handle that must resolve to at most one pending workstream in automation. Prefer explicit `title`/`slug` fields; when bootstrapping old tasks, derive a suggested title/slug from the jj bookmark basename or workspace directory basename, with owner prefixes such as `user/` removed. The task UUID remains the durable identity.

CMUX workspace titles and descriptions use a small display grammar:

```text
title       = <task title>
description = [detail "\n"] meta ["\n" pr-link]
detail      = "_" <short phrase that adds information beyond the title> "_"
meta        = "**" (<priority> ": " <urgency> | "urg: " <urgency>) "**" [ " due " <short-date> ] [ " → " <short-date/time> ]
pr-link     = "[" <repo-basename> "#" <pr-number> "](" <full-pr-url> ")"
```

Use Taskwarrior `title` for the CMUX title. Omit `detail` when the title is enough; when present, it should build on the title with the next useful detail. CMUX color carries actionability. Keep the full Taskwarrior uuid and CMUX workspace ref in Taskwarrior annotations. In the steady state, do not put `task:<uuid-prefix>` in visible CMUX descriptions; scripts resolve CMUX workspace title by slugifying it and requiring exactly one pending task with that `slug`. During migration, scripts may still accept a legacy visible `task:<uuid-prefix>` as the authoritative link. CMUX workspace refs such as `workspace:9` are live UI handles, not durable identity.

CMUX renders workspace descriptions with inline-only Markdown while preserving whitespace. Use inline formatting only: emphasis for `detail`, strong emphasis for the priority/urgency part of `meta`, and normal Markdown links for `pr-link`. Do not rely on block Markdown in the workspace description; if a richer block is needed, use CMUX sidebar metadata blocks instead.

When updating a task that has an associated CMUX workspace, update that workspace's description, color, and order at the same time as the Taskwarrior state.

CMUX colors encode actionability: `Green` for `doing`, `ready`, and `Cleanup`; `Amber` for `review` and timed waits; `Orange` for `blocked`; `Red` for a temporarily open `parked` workspace. Parked workstreams generally should not have an open CMUX workspace.

Order CMUX task workspaces by Taskwarrior urgency by default, after the pinned `tasks` conductor workspace. Keep the color based on actionability; urgency changes order and description metadata, not the state color.

For `meta`, use `<priority>: <urgency>` when priority is set, such as `H: 8.8`, `M: 6.7`, or `L: 4.8`. If priority is absent, use `urg: <score>`. Append `due <short-date>` only for real deadlines. Append `→ <short-date/time>` for `wait` re-checks.

For overdue tasks, keep the color based on actionability and add an overdue signal in metadata instead. Prefer `cmux set-status tw-overdue "overdue" --icon alert-triangle --color "#C0392B"` for an open overdue workspace, and clear it when the due date is cleared or no longer overdue. If a status pill is too noisy or unavailable, include `overdue <short-date>` in the description metadata.

When updating several CMUX properties or several workspaces, batch the CMUX actions into one readable shell command block to avoid slow AI/tool round trips. Prefer `&&` when later actions should stop after a failure; use `;` only when every action is safe to run independently.

Workflow-state tags are exactly:

- `doing`: actively in the current human/AI work loop.
- `ready`: there is a concrete next move for the user, including asking for review or fixing known CI/review feedback.
- `review`: waiting on external review, Slack response, CI, mergeability, or a near-term check-back. Usually pair with `wait:`.
- `blocked`: waiting on an external dependency, another PR, another person, or a decision outside this workstream.
- `parked`: intentionally paused; not expected to move soon.

`Cleanup` is a presentation/status-update label and CMUX visual state, not a workflow-state tag. Use the `ready` tag plus a concrete cleanup `description` when the work is functionally done but still needs merge, scratch archive/retro, branch/workspace cleanup, CMUX cleanup, task completion, or final handoff.

## Priority, Due Dates, and Urgency

Use Taskwarrior urgency as the default conductor ordering signal. Urgency means "needs attention," not always "start coding now." The workflow state tag explains what kind of attention is needed: implement, review, wait, unblock, clean up metadata, or park.

Use `priority` to feed importance into urgency:

- `priority:H`: reviews, author follow-ups, tasks blocking other people, and anything where delayed attention blocks external progress.
- `priority:M`: the user's own active work PRs and normal implementation streams.
- `priority:L`: personal projects, tooling improvements, experiments, and backlog work unless explicitly promoted.

Use `due` only for real deadlines. Do not use `due` for normal review/CI check-ins. Use `wait` for reminders, snoozes, CI/review re-checks, and blocked follow-ups; default normal review waits to the next business day unless the task evidence calls for a same-day re-check or a longer snooze. If a blocked or parked task has high urgency because of an overdue `due`, treat that as a useful attention signal: either clear or correct the deadline, unpark the task, update `wait`, or otherwise fix the task record.

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
- `workspace` identifies where to resume locally; `bookmark` identifies the code line. Keep both when known.
- If `aisession` is present, it identifies the preferred AI session to resume in CMUX and uses `codex:`, `claude:`, or `pi:` prefix form. Preserve an existing different session id; alert the user and annotate the mismatch if it matters for handoff.
- If a dedicated AI session updates or creates a task, it records its own prefixed session id in `aisession` when the field is empty. If the session id is unknown, add a `Session:` annotation with what is known.
- Parked tasks may keep `workspace`/`bookmark` fields as resume metadata, but they do not need an open CMUX workspace.
- If a workstream moves to a different jj workspace, update `workspace` and annotate the move.
- If one workstream splits into two real streams, create two tasks rather than overloading one task.
- Active and ready tasks with long histories have a current resume point: a `Current handoff:` annotation that summarizes the current state, important decision, validation, constraints, and exact next transition.
- Be conservative about automatic state changes. State transitions should follow explicit evidence or user intent.
- Use non-state tags for topics only; do not invent extra state tags like `waiting`, `todo`, `active`, or `paused`.

## Resume Identity

Use four identities deliberately:

- Taskwarrior `uuid`: durable source-of-truth identity. Use it in task annotations and exact automation.
- `title`: human-readable display name. Use it as the CMUX workspace title.
- `slug`: stable machine handle, usually `slugify(title)`, for quick selection, deterministic lookup, AI session names, scratch pad filenames, and remote tmux/workspace names.
- `aisession`: preferred AI session to resume, in `codex:<id>`, `claude:<id-or-slug>`, or `pi:<id>` form.

A slug is convenient, not authoritative. It should resolve to at most one pending workstream before automation acts on it. A CMUX workspace resolves to a task by slugifying its title and finding exactly one pending task whose `slug` matches. During migration, a visible `task:<8-char-uuid-prefix>` link may remain as a fallback, but the target display model omits it from CMUX descriptions.

## Scratch Pads

Scratch pads live in the Obsidian worknotes vault and are durable engineering logs. Taskwarrior remains canonical for status and next transitions; scratch pads record the overall goal, reasoning, investigations, verification, and reusable learnings.

Use a local-only symlink `~/worknotes -> ~/Documents/obsidian/worknotes`. This link is not stowed or tracked by dotfiles; Obsidian owns the documents underneath it.

Use this deterministic convention:

```text
worknotes root = ~/worknotes
scratch UDA    = task.scratch if set, else scratch/<task.slug>.md
path           = <worknotes root>/<scratch UDA>
```

Agents should create scratch pads directly rather than calling a separate helper command. When creating one:

1. Require a task `uuid`, `title`, and `slug`.
2. Create `~/worknotes/scratch/` if needed.
3. Write `scratch/<slug>.md` if it does not already exist.
4. Set `task <uuid> modify scratch:scratch/<slug>.md`.
5. Add one annotation: `Scratch: ~/worknotes/scratch/<slug>.md`.
6. Read the entire scratch pad before doing substantial work.

Use this template:

```markdown
# <Task title>

**Taskwarrior:** `<uuid>`
**Slug:** `<slug>`
**Created:** <YYYY-MM-DD>
**Repo:** `<repo>`
**Workspace:** `<workspace>`
**Bookmark:** `<bookmark>`
**PR:** `<pr>`
**AI session:** `<aisession>`
**Current transition at creation:** <description>

## Goal

_full task goal / outcome being pursued_

## Design

_Engineering design, architecture decisions, tradeoffs considered._

## Debugging

_Investigation steps, failed hypotheses, what turned out to matter._

## Test plan

_How to verify: commands, scenarios, expected outcomes._

## Learnings

_Novel, reusable knowledge worth remembering. Only add entries that would save time on a future unrelated task._
```

The full task goal belongs in the scratch pad `## Goal` field. Taskwarrior `description` remains the next transition only. Add a compact `Goal:` annotation when the goal should be visible in Taskwarrior status without opening the scratch pad.

Mandatory scratch protocol for active AI sessions:

- On session start or resume, read the entire scratch pad before implementation work.
- If creating or resuming substantial work, add a dated `## Design` entry with the understood task and planned approach.
- While working, update `## Goal`, `## Design`, `## Debugging`, `## Test plan`, and `## Learnings` as material evidence appears. The goal can change when the user clarifies intent or implementation learning changes the real outcome being pursued.
- Before replying, ask both:
  - Scratch: did anything since the last response belong in `## Goal`, `## Design`, `## Debugging`, `## Test plan`, or `## Learnings`?
  - Taskwarrior: did the next transition, state tag, wait time, PR/session/scratch metadata, or handoff evidence change?
- If either answer is yes, update the scratch pad and/or Taskwarrior before responding.
- Do not duplicate Taskwarrior state, todo lists, generic diff summaries, or large code blocks in the scratch pad.

`## Learnings` entries must pass this filter: not derivable from code/history, not a one-off task detail, and likely to save time on a future unrelated task.

## Cleanup and Retro

Do not add a `cleanup` workflow-state tag. Cleanup is an actionable phase, so use `+ready` with a concrete cleanup transition such as `Cleanup: archive scratch, extract learnings, close CMUX, and mark done`.

Cleanup includes scratch retro work:

1. Update the scratch pad with final `## Test plan` evidence and any final `## Learnings`.
2. Add or update `**Completed:** <YYYY-MM-DD>` near the scratch pad header.
3. Extract reusable knowledge from `## Learnings` into `~/worknotes/reference/` when it would help future unrelated work.
4. Archive the pad to `~/worknotes/scratch/archive/YYYY-MM/<slug>.md`.
5. Update Taskwarrior `scratch:scratch/archive/YYYY-MM/<slug>.md` and annotate `Archived scratch: ~/worknotes/scratch/archive/YYYY-MM/<slug>.md`.
6. Close/clean the CMUX workspace and local jj bookmark/workspace as appropriate.
7. Mark the Taskwarrior task done only after cleanup/retro is complete.

<!-- TODO(workstreams): Add a workstream-scratch command only if direct agent-managed scratch creation proves error-prone, humans want a manual open/ensure flow, or archive/retro/remote lifecycle needs dedicated tooling. -->

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
- After the PR is merged, keep the task `ready` with a `Cleanup` status/CMUX visual state until scratch archive/retro, local bookmark, workspace, CMUX, and task cleanup have been verified.
- Mark the task done only after the PR is merged and scratch/local cleanup is complete.

## Split-Off Signals

Split off a separate workstream when related work no longer fits cleanly in the current task. Common signals:

- A follow-up idea has its own goal, owner, repo/workspace, or next transition.
- The current description would need multiple independent actions joined by “and,” “then,” or roadmap labels.
- Implementation work and separate design/backlog work are accumulating in the same task.
- A new worker would need a fresh AI session with different context.
- The active history needs more than a compact `Current handoff:` summary to be resumed safely.
- A parked idea becomes active and contains multiple possible first implementations.

When splitting off, best-effort decide what remains in the current task and what belongs in the new task. If the boundary is uncertain, create a reasonable split-off task, keep working the current task, and tell the user what is uncertain. Creating a split-off task does not switch the current session.

## Scope and Vocabulary

Commands are interpreted by scope.

- **Single-workstream scope**: a task CMUX workspace, a task-specific AI session, or a command with an explicit task selector. The current task/session is responsible for keeping its own Taskwarrior record, scratch pad, CMUX display, and relevant external reality up to date.
- **Conductor scope**: the pinned `tasks` CMUX workspace, or a request that clearly concerns the whole portfolio of workstreams. The conductor normally trusts individual task records and does not inspect external PR/CI/Slack/jj state for every task.

Hard invariants:

- An AI session working a task has a current workstream.
- Creating or splitting off another task does not switch the current session. Only an explicit `open`, `switch`, or `resume` request changes the current workstream.
- Split-off tasks do not inherit the current `aisession` unless a new dedicated session is actually started for them.
- Individual task agents are responsible for updating their own Taskwarrior state from external reality and progress. The conductor generally trusts those records.
- The conductor may run `reconcile` as an explicit rare override when the user wants a broad audit of external reality across tasks.

### Shared Commands

`status` means “tell me the currently recorded state.” It is read-only.

- In single-workstream scope: recap the current/selected task from Taskwarrior, scratch, and already-known session context. Do not check external systems unless asked.
- In conductor scope: recap all relevant workstreams from Taskwarrior only, including pending and `+WAITING`. Do not inspect CMUX or external systems.

Use these grouping labels when they help readability, ordered by Taskwarrior urgency within groups:

- `Active / Ready`: `doing` or `ready`, including `Cleanup` items.
- `Review / Waiting`: `review`, including review, Slack, CI, mergeability, or timed waits.
- `Blocked`: `blocked`.
- `Parked`: `parked`.

`check` and `refresh` are equal-status synonyms.

- In single-workstream scope: inspect external state relevant to the current/selected task, such as PR, CI, review, Slack, mergeability, or local jj state when relevant; update scratch, Taskwarrior, and CMUX; recap.
- In conductor scope: inspect Taskwarrior and CMUX, make CMUX match Taskwarrior by creating/updating/closing/reordering workspaces as appropriate, and recap. Do not check external PR/CI/Slack/jj state by default.

`reconcile` is conductor-scope only unless the user explicitly scopes it to one task. It is the rare broad audit: inspect Taskwarrior, scratch pads when needed, CMUX, local repo state when relevant, and external state across tasks; update records/UI; recap what changed and what remains uncertain.

### Single-Workstream Commands

`update`, `checkpoint`, and `handoff` are synonyms for making the current workstream record durable and resumable. Agents should also perform the relevant parts automatically before replies whenever material state changed.

A checkpoint/update/handoff should:

1. Update scratch `## Goal`, `## Design`, `## Debugging`, `## Test plan`, or `## Learnings` when material evidence or clarified intent changed.
2. Set Taskwarrior `description` to the next concrete transition.
3. Set exactly one workflow-state tag.
4. Set `wait:` when a timed re-check is useful.
5. Update task metadata such as `pr`, `workspace`, `bookmark`, `scratch`, or `aisession` when it changed.
6. Add or refresh a compact `Current handoff:` annotation when a future human/agent may need to resume without reading chat history.
7. Update linked CMUX title/description/color/order when open.
8. Recap current state and uncertainties.

`cleanup` is a ready-phase single-workstream command, not a state tag. It performs final scratch archive/retro, local jj/bookmark/workspace cleanup, CMUX cleanup, and Taskwarrior completion. See [Cleanup and Retro](#cleanup-and-retro).

`split off` creates a separate related workstream from current context without switching the current session. The agent should best-effort decide what belongs in the new task and what remains in the current task; if the boundary is uncertain, make a reasonable split-off task, stay on the current task, and tell the user what is uncertain.

When splitting off:

1. Create a new Taskwarrior task with its own `uuid`, `title`, `slug`, `description`, state tag, and metadata.
2. Create/link its scratch pad when it will be AI-assisted.
3. Put the full goal and relevant context in the new scratch pad, and compact provenance in annotations.
4. Do not copy the current task's `aisession`.
5. Annotate the current task with the split-off task title/uuid when useful.
6. Continue the current task unless the user explicitly says to open/switch/resume the split-off task.

`open`, `switch`, and `resume` explicitly change the current workstream. They create/select the CMUX workspace, ensure/read scratch for active AI work, and start/resume the appropriate AI session.

### Recap Style

Conductor recaps should name the workstream title, urgency, priority/due/wait when relevant, state, PR link when present, next transition, and the latest compact handoff/blocker/dependency/refresh evidence.

Single-workstream recaps should start with a short status label such as `Ready`, `Review`, `Blocked`, `Parked`, `Cleanup`, or `Checkpoint`, then summarize: goal, what changed, current evidence, next transition, and uncertainties.

If urgency ordering looks wrong, fix Taskwarrior fields (`priority`, `due`, `wait`, state tags) instead of inventing a parallel ranking system.

## Workflows

### Single-Workstream Work Loop

1. Read the Taskwarrior task and scratch pad.
2. Work the current transition.
3. Before replying, ask whether scratch or Taskwarrior/CMUX changed.
4. If changed, run the update/checkpoint/handoff behavior above.
5. Recap current state and next transition.

### Conductor Check / Refresh

1. Read pending and `+WAITING` Taskwarrior tasks.
2. Inspect open CMUX workspaces.
3. Align CMUX from Taskwarrior: create/update/close/reorder workspaces, set descriptions/colors, and avoid opening parked workspaces.
4. Run `workstream-slug doctor --cmux` when title/slug/CMUX mapping may have changed.
5. Recap recorded state and any CMUX changes. Do not check external systems unless this is a reconcile.

### Conductor Reconcile

Use only when the user asks for a broad audit or says the records may be stale.

1. Start with conductor check/refresh.
2. For tasks whose records depend on external state, inspect the relevant external source.
3. Update Taskwarrior, scratch, and CMUX when reality differs from recorded state.
4. Record `Refresh:` or `Current handoff:` evidence where the update affects future decisions.
5. Recap changes, current portfolio state, and remaining uncertainties.

## Raw Taskwarrior Examples

Create:

```bash
task add project:widget-lib title:"Widget Lib Bump" slug:widget-lib-bump scratch:scratch/widget-lib-bump.md repo:example-org/monorepo workspace:/Users/<username>/go/src/github.com/example-org/monorepo/workspaces/widget-java bookmark:<username>/bump-widget-lib-0.3.2 pr:12345 +review wait:tomorrow "Check CI on PR #12345 and merge if approved"
task <id> annotate "PR: https://github.com/example-org/monorepo/pull/12345"
task <id> annotate "Validation: make test passed"
task <id> annotate "Current handoff: PR #12345 is waiting on CI; validation passed locally; next check CI and merge if approved."
task rc.gc=off <id> uuids
```

Checkpoint / update / handoff:

```bash
task <id> modify -doing -ready -review -blocked -parked +blocked wait:2026-05-02 "Re-check upstream PR and unblock the release bump"
task <id> annotate "Blocker: upstream release PR has not landed; no local action until it lands"
```

Find likely workstreams:

```bash
task rc.gc=off status:pending '( repo.not: or title.not: or slug.not: or scratch.not: or +doing or +ready or +review or +blocked or +parked )' \
  columns:id,uuid,title,slug,scratch,project,repo,workspace,bookmark,pr,priority,urgency,due,wait,tags,description
task rc.gc=off +WAITING export
```

Validate state tags and title/slug mapping manually:

```bash
task rc.gc=off status:pending '( repo.not: or title.not: or slug.not: or scratch.not: or workspace.not: or bookmark.not: or pr.not: or aisession.not: or +doing or +ready or +review or +blocked or +parked )' \
  columns:id,uuid,title,slug,scratch,project,repo,workspace,bookmark,pr,priority,urgency,due,wait,tags,description
task rc.gc=off +WAITING export
workstream-slug doctor --cmux
```

Taskwarrior reports can hide tasks with `wait:`. Always include `+WAITING export` when checking review, blocked, or parked workstreams that may be waiting.

Inspect any workstream task with missing `project`, missing `repo`, missing active-workstream `title`/`slug`/`scratch`, or not exactly one of the five workflow-state tags.
