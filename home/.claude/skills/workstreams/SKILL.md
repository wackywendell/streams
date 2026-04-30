---
name: workstreams
description: Manage AI-assisted coding workstreams in Taskwarrior across repos, bookmarks, PRs, and AI sessions.
---

# Workstreams

Use Taskwarrior as the source of truth for coding workstream state.

Read the shared policy first:

- Live path: `~/.config/ai-workstreams/SPEC.md`
- Dotfiles source path: `home/.config/ai-workstreams/SPEC.md`

Follow the same core workflow as Codex:

- one Taskwarrior task per real workstream;
- `description` is the current next action;
- annotations are evidence and history;
- tags include exactly one workflow-state tag: `doing`, `ready`, `review`, `blocked`, or `parked`;
- state changes should be evidence-based and conservative.

Call `task` directly. Use `rc.gc=off` for read-only inspection, and use raw commands from the shared spec for capture, handoff, check, and split.
