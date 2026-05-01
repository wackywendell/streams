---
description: Capture, hand off, check, split, or validate a Taskwarrior coding workstream across repos and jj workspaces
argument-hint: <status|refresh|sync|capture|open|update|split|next action>
---

# Workstream

Task: $ARGUMENTS

Use the `workstreams` skill. Keep Taskwarrior as the source of truth and follow `~/.config/ai-workstreams/SPEC.md`.

This is the general entrypoint for Taskwarrior-backed coding workstreams. Route the request by intent:

- For broad status, use Taskwarrior only unless the user asks to refresh external state.
- For one-task status, summarize the Taskwarrior record and any stored PR/CMUX/AI-session metadata.
- For refresh, inspect only the external state needed to make the next Taskwarrior transition.
- For sync, refresh real-world state where appropriate, then align CMUX from Taskwarrior.
- For capture, open, update, and split requests, make the Taskwarrior and CMUX changes directly when the intent is clear.
