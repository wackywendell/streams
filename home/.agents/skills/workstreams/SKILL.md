---
name: workstreams
description: "Manage AI-assisted coding workstreams in Taskwarrior. Use when the user asks for status, check/refresh, reconcile, update/checkpoint/handoff, cleanup, split-off, open/switch/resume, or task state changes across repos, jj workspaces, jj bookmarks, PRs, scratch pads, CMUX workspaces, or AI sessions."
metadata:
  short-description: Manage Taskwarrior coding workstreams
---

# Workstreams

Use this skill to keep AI coding workstream state in Taskwarrior.

Taskwarrior is the source of truth for workstreams. Chats, PRs, bookmarks, CMUX workspaces, and scratch pads are evidence attached to a workstream; they are not canonical state.

## Required Reading

Read the role-specific reference docs before changing workstream state.

Always read:

- [references/core.md](references/core.md) — model, identity, state tags, priority, invariants, annotations, and split-off signals.

Then read by scope:

- **Conductor / portfolio scope**: [references/conductor.md](references/conductor.md), plus [references/cmux-integration.md](references/cmux-integration.md) when inspecting or aligning CMUX.
- **Single-workstream scope**: [references/task-agent.md](references/task-agent.md), plus [references/scratch.md](references/scratch.md) before substantial task work.
- **Cleanup / retro**: [references/cleanup.md](references/cleanup.md).
- **Workstream-specific Taskwarrior fields and reports**: [references/taskwarrior-integration.md](references/taskwarrior-integration.md).
- **Workstream-specific CMUX display policy**: [references/cmux-integration.md](references/cmux-integration.md).

Use the sibling [taskwarrior skill](../taskwarrior/SKILL.md) for pure Taskwarrior CLI mechanics. Use the sibling [cmux skill](../cmux/SKILL.md) for pure CMUX command mechanics. The workstreams references define how those tools are used for AI coding workstreams.

If scope is unclear, read `core.md`, `conductor.md`, and `task-agent.md`, then infer conservatively or ask a clarifying question.

## Command Vocabulary

Commands are interpreted by scope:

- `status`: read-only recorded-state recap.
- `check` / `refresh`: synonyms for inspecting relevant state, updating records/UI when reality changed, and recapping.
- `reconcile`: conductor-only broad audit across Taskwarrior, CMUX, scratch when needed, local repo state when relevant, and external state.
- `update` / `checkpoint` / `handoff`: single-workstream synonyms for making the current task durable and resumable.
- `cleanup`: single-workstream ready-phase cleanup/retro/archive flow; not a workflow-state tag.
- `split off`: create a related separate workstream without switching the current session.
- `open` / `switch` / `resume`: explicitly change the current workstream and create/select/resume its CMUX/AI session as needed.

## Mechanics

Use `task` directly for workstream records, with `rc.gc=off` for read-only inspection and task UUIDs for durable mutations. See [references/taskwarrior-integration.md](references/taskwarrior-integration.md) for workstream reports and examples.

When the references require the current Pi session id, use the `get_session_id` tool and store the prefixed `pi:<uuid>` value.

Before replying after material work, consider the update/checkpoint behavior from `task-agent.md`: refresh durable scratch evidence, Taskwarrior state/metadata/handoff evidence, and linked CMUX display only when the work usefully changed them. Run `workstream-slug doctor --cmux` when title/slug/CMUX mapping changed.
