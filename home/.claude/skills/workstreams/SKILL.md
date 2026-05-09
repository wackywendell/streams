---
name: workstreams
description: Manage AI-assisted coding workstreams in Taskwarrior across repos, jj workspaces, bookmarks, PRs, scratch pads, CMUX workspaces, and AI sessions.
---

# Workstreams

Use this skill for Taskwarrior-backed coding workstreams.

## Required Reading

Before changing tasks, read the shared policy:

- Skill-local link: [references/SPEC.md](references/SPEC.md)
- Live path: `~/.config/ai-workstreams/SPEC.md`
- Dotfiles source: `home/.config/ai-workstreams/SPEC.md`

`SPEC.md` is the source of truth for the model, invariants, state tags, title/slug identity, scratch-pad protocol, cleanup/retro, CMUX display grammar, and workflows. Follow it rather than duplicating policy here.

## Claude-Specific Operational Notes

- Use Taskwarrior as the source of truth. Chat state is evidence only.
- Use Taskwarrior `uuid` for durable identity; numeric ids are only for immediate commands after a fresh listing.
- If this Claude session owns a workstream and `aisession` is empty, set `aisession:claude:<id-or-slug>` when known. If a different session is already recorded, alert the user instead of overwriting silently.
- For substantial AI-assisted work, read/create/update the scratch pad per `SPEC.md`. The scratch `**Goal:**` holds the overall task goal; Taskwarrior `description` remains the next transition.
- Before replying, update scratch if `**Goal:**`, design, debugging, test plan, or learnings changed; update Taskwarrior if next transition, state, wait, metadata, or handoff evidence changed.
- When a task has a linked CMUX workspace, update CMUX title/description/color/order alongside Taskwarrior state changes.
- Use direct `task` commands with `rc.gc=off` for read-only inspection. Include pending and `+WAITING` tasks when checking status.
