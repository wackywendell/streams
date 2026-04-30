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
- One task is one workstream.
- `description` is the current next action and must be concrete.
- Tags must include exactly one state tag: `doing`, `ready`, `review`, `blocked`, or `parked`.
- `workspace` is the local jj workspace root; `bookmark` is the code line. Keep both when known.
- Preserve useful history in annotations.
- Be conservative about state changes; explain the evidence when moving a task.

## Direct Taskwarrior Use

Call Taskwarrior directly. Use `rc.gc=off` for read-only inspection to avoid Taskwarrior maintenance writes during simple checks.

```bash
task rc.gc=off status:pending '( repo.not: or workspace.not: or bookmark.not: or pr.not: or aisession.not: or +doing or +ready or +review or +blocked or +parked )' columns:id,project,repo,workspace,bookmark,pr,tags,wait,description
task rc.gc=off +WAITING export
task <id> information
task <id> modify -doing -ready -review -blocked -parked +review wait:tomorrow "Check CI and merge if approved"
task <id> annotate "PR is waiting on CI"
```

Remember that normal reports can hide tasks with `wait:`; use `+WAITING export` when checking non-actionable workstreams.

## Capture

1. Infer context from the current repo:
   - `pwd`, `jj workspace root`, `jj root`, `jj status`, `jj bookmark list --all`, and PR URLs when useful.
   - repo in `organization/name` form.
   - workspace, bookmark, PR, and AI session when known.
2. Search existing tasks before creating:
   - by `repo`, `workspace`, `bookmark`, `pr`, and distinctive description words.
3. Create or update exactly one task for the stream.
4. Add annotations for evidence: branch, PR URL, validation, blocker, key decision, or source chat/session.

## Handoff

When stopping work:

1. Set `description` to the next action.
2. Verify `workspace` still points to the local jj workspace where the work should resume.
3. Choose exactly one state.
4. Use `wait:` for review, blocked, or parked streams when a reminder helps.
5. Annotate enough evidence to resume without rereading the full chat.
6. Re-list or inspect the updated task and verify the invariants manually.

## Check

When checking review/blocked/parked tasks:

1. Inspect the task and current external evidence, such as PR status or repo state.
2. If still waiting, keep the state and optionally update `wait:`.
3. If actionable, update the next action and move to `ready`.
4. Do not promote stale work to `ready` from memory alone.

## Split

When a task contains two real streams:

1. Name the independent streams.
2. Keep the original task for one stream and narrow its description.
3. Create a second task for the other stream.
4. Annotate both with the split reason when practical.
