---
name: workstreams
description: Manage AI-assisted coding workstreams in Taskwarrior. Use when the user asks for status, check/refresh, reconcile, update/checkpoint/handoff, cleanup, split-off, open/switch, or task state changes across repos, jj workspaces, jj bookmarks, PRs, scratch pads, CMUX workspaces, or AI sessions.
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

`SPEC.md` is the source of truth for the model, invariants, state tags, scratch-pad protocol, cleanup/retro, CMUX display grammar, scope vocabulary, and workflows. Follow it rather than duplicating policy here.

## Mechanics

Use `task` directly. Use `rc.gc=off` for read-only inspection, and use task UUIDs for durable mutations.

Useful commands:

```bash
task rc.gc=off <uuid> information
task rc.gc=off <uuid> export
task rc.gc=off +WAITING export
task rc.gc=off status:pending '( repo.not: or title.not: or slug.not: or scratch.not: or workspace.not: or bookmark.not: or pr.not: or aisession.not: or +doing or +ready or +review or +blocked or +parked )' columns:id,uuid,title,slug,scratch,project,repo,workspace,bookmark,pr,priority,urgency,due,wait,tags,description
task <uuid> modify <field-or-tag-changes> "<next transition>"
task <uuid> annotate "Current handoff: <compact resume summary>"
```

When the spec requires a current Pi session id, use the `get_session_id` tool and store the prefixed `pi:<uuid>` value.

Before replying after material work, consider the update/checkpoint behavior from `SPEC.md`: refresh durable scratch evidence, Taskwarrior state/metadata/handoff evidence, and linked CMUX display only when the work usefully changed them. Run `workstream-slug doctor --cmux` when title/slug/CMUX mapping changed.
