---
name: workstreams
description: Manage AI-assisted coding workstreams in Taskwarrior. Use when the user asks to capture, hand off, check, split, resume, list, validate, or update coding workstream state across repos, jj workspaces, jj bookmarks, PRs, scratch pads, CMUX workspaces, or AI sessions.
metadata:
  short-description: Manage Taskwarrior coding workstreams
---

# Workstreams

Use this skill to keep AI coding workstream state in Taskwarrior.

## Required Reading

Before changing tasks, read the shared policy:

- Skill-local link: [references/SPEC.md](references/SPEC.md)
- Live path: `~/.config/ai-workstreams/SPEC.md`
- Dotfiles source: `home/.config/ai-workstreams/SPEC.md`

`SPEC.md` is the source of truth for the model, invariants, state tags, scratch-pad protocol, cleanup/retro, CMUX display grammar, and workflows. Do not duplicate or override it here.

## Operational Rules

- Taskwarrior is canonical. Chat, CMUX, PRs, jj, and scratch pads are evidence/context.
- One Taskwarrior task is one workstream.
- Use Taskwarrior `uuid` for durable identity. Numeric ids are only for immediate commands after a fresh listing.
- `description` is the concrete next transition, not the whole goal. The full goal belongs in the scratch pad `**Goal:**` field and optionally a compact `Goal:` annotation.
- Maintain exactly one workflow-state tag: `doing`, `ready`, `review`, `blocked`, or `parked`.
- Active AI-assisted work should have `title`, `slug`, `scratch`, and `aisession` when known.
- If this dedicated AI session owns the task and `aisession` is empty, set it to the current prefixed session id. If another session is already recorded, alert the user instead of overwriting silently.
- Before replying after substantial work, check both:
  - scratch: did `**Goal:**`, `## Design`, `## Debugging`, `## Test plan`, or `## Learnings` need updates?
  - Taskwarrior: did next transition, state tag, wait time, PR/session/scratch metadata, or handoff evidence change?
- When a task has a linked CMUX workspace, update CMUX title/description/color/order alongside Taskwarrior state changes.

## Direct Taskwarrior Use

Use `task` directly. Use `rc.gc=off` for read-only inspection.

```bash
task rc.gc=off status:pending '( repo.not: or title.not: or slug.not: or scratch.not: or workspace.not: or bookmark.not: or pr.not: or aisession.not: or +doing or +ready or +review or +blocked or +parked )' columns:id,uuid,title,slug,scratch,project,repo,workspace,bookmark,pr,priority,urgency,due,wait,tags,description
task rc.gc=off +WAITING export
task rc.gc=off <id-or-uuid> information
task rc.gc=off <id-or-uuid> uuids
task <uuid> modify -doing -ready -review -blocked -parked +review wait:tomorrow "Check CI and merge if approved"
task <uuid> annotate "Refresh: PR is waiting on CI"
```

Taskwarrior reports can hide tasks with `wait:`; include `+WAITING export` for review/blocked/parked workstreams that may be waiting.

## Scratch Pad Protocol

Follow the full protocol in [references/SPEC.md](references/SPEC.md#scratch-pads). Short version:

- Scratch pads live under `~/worknotes`, usually `~/worknotes/scratch/<slug>.md`.
- Taskwarrior stores `scratch:scratch/<slug>.md` or an archive path.
- If an active AI task lacks a scratch pad, create it directly from the template in the spec, set the `scratch` UDA, and annotate `Scratch: ~/worknotes/scratch/<slug>.md`.
- Read the entire pad before implementation work.
- Update `**Goal:**`, `## Design`, `## Debugging`, `## Test plan`, and `## Learnings` as appropriate before responding.
- Do not put canonical task status or todo tracking in scratch; Taskwarrior owns that.

## Command Vocabulary

Use these terms consistently:

- `status`: summarize Taskwarrior state only. Include pending and `+WAITING`. Do not refresh external systems.
- `task-status`: summarize one task from Taskwarrior only unless asked to refresh.
- `refresh`: check external state for review/blocked/external-ready tasks and update Taskwarrior on clear evidence.
- `sync-cmux`: align CMUX from Taskwarrior only.
- `sync`: run refresh, then sync CMUX.
- `capture`: create/update one workstream from current context or supplied text/PR/issue.
- `task-open`: create/select CMUX workspace, ensure/read scratch for active AI work, and resume/start the AI session.
- `task-update`: update one task and its linked CMUX workspace.
- `task-split`: split one overloaded workstream into two tasks, with separate scratch/CMUX/session for the new stream when active.

## Workflow Checklist

When capturing/opening/updating/splitting:

1. Resolve the task by uuid, slug, title, PR, repo/workspace/bookmark, or distinctive words.
2. Gather relevant repo context only when needed: `pwd`, `jj workspace root`, `jj root`, `jj status`, `jj bookmark list --all`, PR URL.
3. Ensure active AI tasks have `title`, `slug`, `scratch`, and appropriate `workspace`/`bookmark`/`pr`/`aisession` metadata.
4. Apply the scratch protocol before implementation and before final replies.
5. Set `description` to the next concrete transition.
6. Set exactly one workflow-state tag.
7. Add compact evidence annotations using prefixes from the spec, especially `Current handoff:`, `Decision:`, `Validation:`, `Blocker:`, `Dependency:`, `Refresh:`, `CMUX:`, `Session:`, and `Error:`.
8. Re-list/inspect the task and run `workstream-slug doctor --cmux` when title/slug/CMUX mapping changed.
