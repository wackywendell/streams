---
description: Capture, hand off, check, split, or validate a Taskwarrior coding workstream across repos and jj workspaces
argument-hint: <status|task-status|refresh|sync|sync-cmux|capture|task-open|task-update|task-split|next action>
---

# Workstream

Task: $ARGUMENTS

Use the `workstreams` skill. Keep Taskwarrior as the source of truth and follow `~/.config/ai-workstreams/SPEC.md`.

Prefer the specific thin prompts when possible:

- `status` for all-workstream Taskwarrior-only status;
- `task-status` for one-task Taskwarrior-only status;
- `refresh` for external reassessment into Taskwarrior;
- `sync-cmux` for CMUX alignment from Taskwarrior;
- `sync` for refresh then CMUX alignment;
- `capture` for new or existing workstream capture;
- `task-open`, `task-update`, and `task-split` for single-task workflows.
