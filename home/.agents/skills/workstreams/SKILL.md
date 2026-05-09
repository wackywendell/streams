---
name: workstreams
description: Manage AI-assisted coding workstreams in Taskwarrior. Use when the user asks to capture, hand off, check, split, resume, list, validate, or update coding workstream state across repos, jj workspaces, jj bookmarks, PRs, or AI sessions.
metadata:
  short-description: Manage Taskwarrior coding workstreams
---

# Workstreams

Use this skill to keep AI coding workstream state in Taskwarrior.

Before changing tasks, read the shared policy at `~/.config/ai-workstreams/SPEC.md` or the dotfiles source path `home/.config/ai-workstreams/SPEC.md`.

## Core Rules

- Taskwarrior is the source of truth. Chat state is evidence only.
- Do not use Beads or `bd` for workstream state; older instructions that mention them are stale.
- One task is one workstream.
- Taskwarrior numeric ids can change. Use `uuid` for durable task identity; use numeric ids only for immediate commands after listing current tasks.
- `description` is the next transition that would advance the stream and must be concrete.
- Tags must include exactly one state tag: `doing`, `ready`, `review`, `blocked`, or `parked`.
- `workspace` is the local jj workspace root; `bookmark` is the code line. Keep both when known.
- Active workstreams should have `title` and `slug`: `title` is the human display label for CMUX/status, while `slug` is the stable lowercase/kebab-case machine handle derived from title by default. Taskwarrior `uuid` remains durable identity.
- CMUX title should be the workstream `title`; in the steady state, visible CMUX descriptions do not need `task:<uuid-prefix>` because automation resolves slugify(CMUX title) to exactly one pending task `slug`. Accept legacy `task:<uuid-prefix>` while migrating.
- `aisession` uses `codex:<id>`, `claude:<id-or-slug>`, or `pi:<id>` when known.
- Use Taskwarrior `priority` to feed urgency: `H` for reviews, follow-ups, and anything blocking others; `M` for the user's own work PRs; `L` for personal/tooling/backlog work unless explicitly promoted.
- Use `due` only for real deadlines. Use `wait` for reminders, snoozes, CI/review re-checks, and blocked follow-ups; default normal review waits to the next business day unless the task evidence calls for a same-day re-check or longer snooze.
- Treat Taskwarrior `urgency` as "needs attention"; state tags explain what kind of attention is needed.
- Annotations are focused evidence, not transcripts. Prefer typed prefixes such as `Current handoff:`, `Decision:`, `Validation:`, `Blocker:`, `Dependency:`, `Refresh:`, `CMUX:`, `Session:`, and `Error:`.
- Be conservative about state changes; explain the evidence when moving a task.
- When a dedicated AI session updates or captures a task, fill `aisession` with the current prefixed AI session id if it is empty. If it is already set to a different session, alert the user instead of overwriting it silently.

## Direct Taskwarrior Use

Call Taskwarrior directly. Use `rc.gc=off` for read-only inspection to avoid Taskwarrior maintenance writes during simple checks.

```bash
task rc.gc=off status:pending '( repo.not: or workspace.not: or bookmark.not: or pr.not: or aisession.not: or +doing or +ready or +review or +blocked or +parked )' columns:id,uuid,project,repo,workspace,bookmark,pr,priority,urgency,due,wait,tags,description
task rc.gc=off +WAITING export
task <id> information
task rc.gc=off <id> uuids
task <id> modify -doing -ready -review -blocked -parked +review wait:tomorrow "Check CI and merge if approved"
task <id> annotate "PR is waiting on CI"
```

Remember that normal reports can hide tasks with `wait:`; use `+WAITING export` when checking non-actionable workstreams.

## Workstream Shape and State

Use the stream shape to choose precise descriptions. Implementation PRs are common, but reviewing someone else's PR, investigating a failure, writing a design note, or doing cleanup are also valid workstreams.

Implementation examples:

- discovery/planning: `Write down acceptance criteria for <topic>`;
- AI implementation: `Implement <specific change> in <workspace>`;
- draft PR: `Open draft PR for <bookmark>`;
- self-review: `Self-review draft PR #<n> and address obvious issues` (the user's move);
- mark ready: `Mark PR #<n> ready for review by others`;
- ask for review: `Ask for Slack review on PR #<n>` (handoff to external review);
- waiting: `Check review response on PR #<n>` or `Check CI rerun on PR #<n>`;
- feedback/CI action: `Address <review/CI issue> on PR #<n>`;
- merge/cleanup: `Merge PR #<n> and clean up workspace/bookmark`.

Review examples:

- start review: `Inspect PR #<n> and linked context`;
- write findings: `Review PR #<n> locally and write findings`;
- post review: `Post review comments on PR #<n>`;
- wait for author: `Check author updates on PR #<n>`;
- re-review: `Re-review PR #<n> after updates`;
- approve: `Approve PR #<n> if concerns are resolved`.

Investigation examples:

- reproduce: `Reproduce <failure> and record evidence`;
- analyze: `Compare <behavior> with expected contract`;
- hand off: `Summarize findings and decide whether to fix, file, or park`.

State tags describe who can move the stream next:

- `doing`: actively in the current human/AI loop.
- `ready`: the user has the next move, including self-reviewing a draft PR, marking it ready for review by others, reviewing someone else's PR, asking Slack for review, signing/fixing/pushing a PR stack, merging, or fixing known feedback/CI.
- `review`: waiting on review, author updates, Slack response, CI, merge check, merge queue, rerun, or a timed check; use `wait:` when useful.
- `blocked`: waiting on a true external dependency outside the normal review/CI loop.
- `parked`: intentionally paused.

Do not invent a `waiting` tag; use `review` for normal review/CI waits. A vague GitHub "blocked" state is not enough for `blocked`: read merge check/check-run text directly when refreshing, and move to `ready` if the output names an actionable fix such as unsigned commits. Use `Cleanup` as a status-update label and CMUX visual state, not a Taskwarrior state tag; the task remains `ready` with a cleanup description until PR merge and local bookmark/workspace/CMUX cleanup are verified. Parked tasks do not need an open CMUX workspace.

## Status Updates

For overall status updates, especially in the `tasks` CMUX workspace, order workstreams by Taskwarrior urgency by default. When grouping makes the status easier to scan, use these sections:

- `Active / Ready`
- `Review / Waiting`
- `Blocked`
- `Parked`

Urgency means "needs attention," not always "start coding now"; the state label explains whether the response is implementation, review, waiting, unblocking, cleanup, or metadata correction. Include `Cleanup` items under `Active / Ready` unless the user asks for a separate cleanup section. Each item should name the task/topic, urgency, priority/due/wait when relevant, full PR URL when present, current state, and next transition.

If a blocked or parked task is high urgency because of an overdue `due`, treat that as an attention signal: clear or correct the deadline, unpark it, update `wait`, or otherwise fix the task record. Do not invent a parallel ranking system when Taskwarrior fields can be corrected.

For general status updates or "what should I work on" scans, rely on Taskwarrior state and annotations only. Do not check GitHub, PR checks, CI, local git/jj status, or code state unless the user asks for fresh verification, asks about one specific task, or the Taskwarrior record is too ambiguous to answer. Use `task export` data plus `+WAITING export` rather than raw annotation-heavy reports; describe external state as recorded task state instead of silently refreshing it.

For individual task updates, start with one bold status word or short phrase: `**Ready**`, `**Blocked**`, `**Review**`, `**Parked**`, `**Cleanup**`, or another precise label when those do not fit. Then give a concise summary of what the task is, why it exists, what has been achieved, what remains, what is being waited for, and the current next transition. If a PR is open, include the full PR URL.

Do not blur self-review and external review. A draft PR that the user needs to inspect should be `**Ready**` or `**Self-review**`. Use `**Review**` only when waiting on someone else, Slack response, CI, mergeability, or a timed check.

When a task has an associated CMUX workspace, update the CMUX title, description, color, status pills, and order alongside Taskwarrior state. Use the shared CMUX display grammar: `title = <task title>` and `description = [detail] + meta + optional PR markdown link`, where `meta` looks like `H: 8.8` or `M: 3.7 → Wed 09:00`. Keep the full Taskwarrior uuid in Taskwarrior annotations; visible `task:<8-char-uuid-prefix>` links are legacy fallback only.

Use CMUX colors to show actionability: `Green` for `doing`, `ready`, and `Cleanup`; `Amber` for `review` and timed waits; `Orange` for `blocked`; `Red` for a temporarily open `parked` workspace. Do not add separate overdue colors; add a `tw-overdue` status pill or `Overdue: <date>` description metadata instead. Order CMUX task workspaces by Taskwarrior urgency after the pinned `tasks` conductor workspace. Parked tasks generally should not have an open CMUX workspace. When multiple CMUX actions are needed, batch them into one readable shell command block, using `&&` when later updates should stop after an earlier failure.

## Command Vocabulary

Use these thin command names consistently:

- `status`: all-workstream status from Taskwarrior only; include pending and `+WAITING`; do not inspect CMUX or external systems.
- `task-status`: one-task status from Taskwarrior only.
- `refresh`: reassess external state for `review`, `blocked`, and externally-linked `ready` tasks; make obvious state changes and ask on ambiguous evidence.
- `sync-cmux`: align CMUX from Taskwarrior only.
- `sync`: run `refresh`, then `sync-cmux`.
- `capture`: create or update one workstream from current context or arguments such as free text, PR URL, GitHub issue, or Jira issue.
- `task-open`: create/select the CMUX workspace for one task and resume/start the appropriate AI session.
- `task-update`: update one task plus its linked CMUX workspace.
- `task-split`: split one workstream into two, with a fresh task/CMUX/AI session for the new work.

## Capture

1. Infer context from the current repo:
   - `pwd`, `jj workspace root`, `jj root`, `jj status`, `jj bookmark list --all`, and PR URLs when useful.
   - repo in `organization/name` form.
   - workspace, bookmark, PR, and AI session when known.
2. Search existing tasks before creating:
   - by `uuid`, `slug`, `title`, `pr`, `repo`, `workspace`, `bookmark`, and distinctive description words.
3. Create or update exactly one task for the stream.
4. If this dedicated AI session owns the work and `aisession` is empty, set it to the current prefixed AI session id when known. If `aisession` is already another session, alert the user and annotate only if useful.
5. Add focused evidence annotations with prefixes such as `Session:`, `PR:`, `Decision:`, `Validation:`, `Blocker:`, `Dependency:`, `Error:`, or `Current handoff:`.

## Handoff

When stopping work:

1. Set `description` to the next transition.
2. Verify `workspace` still points to the local jj workspace where the work should resume.
3. Choose exactly one state.
4. Use `wait:` for review, blocked, or parked streams when a reminder helps.
5. Add a compact `Current handoff:` annotation with current local/external state, validation or exact error status, important decisions/constraints, and the exact next transition.
6. For PR work, record the next transition according to who can act next: fix/sign/push is `ready`, CI/merge check/rerun is `review`, and merged-but-not-cleaned-up is `ready` with `Cleanup`.
7. Re-list or inspect the updated task and verify the invariants manually.

## Check

When checking review/blocked/parked tasks:

1. Inspect the task and current external evidence, such as PR status or repo state.
2. If still waiting, keep the state and record new evidence when it changes the wait, blocker, head/status, or next decision.
3. If actionable, update the next transition and move to `ready`.
4. Do not promote stale work to `ready` from memory alone.

## Refresh

When reassessing real-world state:

1. Inspect pending workstreams, including `+WAITING`.
2. Reassess `review` and `blocked` tasks, plus `ready` tasks with external hooks such as PRs, CI, Slack review asks, author-update waits, or mergeability.
3. Use current external evidence only for this command family: GitHub PR/review/check state, Slack replies, CI/merge check details, or local repo state if directly relevant.
4. If the transition is obvious, update Taskwarrior and the linked CMUX workspace.
5. Record a `Refresh:` annotation when the check changes task state, next transition, wait, head/status, or preserves ambiguity that affects future decisions.
6. If evidence is ambiguous, record the evidence and ask the user before changing state.

## Split

Split when the current task no longer has one coherent next transition: multiple independent actions, different repos/workspaces/PRs/owners, implementation mixed with backlog/design, a fresh AI session needing different context, or history that cannot be summarized by one `Current handoff:`.

When a task contains two real streams:

1. Name the independent streams.
2. Keep the original task for one stream and narrow its description.
3. Create a second task for the other stream with its own repo/workspace/bookmark/pr fields and next transition.
4. Include enough context for a fresh AI session: source task uuid, split reason, relevant files, PR/issue links, prior decisions, validation state, and desired next transition.
5. Do not copy the old `aisession`; leave it unset for the new AI session to record.
6. Create a new CMUX workspace in the appropriate directory and start a new AI session there.
7. Annotate both with the split reason when practical.
