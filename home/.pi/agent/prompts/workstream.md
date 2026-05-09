---
description: Route Taskwarrior coding workstream requests across current-task and conductor scopes
argument-hint: <status|check|refresh|reconcile|update|checkpoint|handoff|cleanup|split off|open|switch|next action>
---

# Workstream

Task: $ARGUMENTS

Use the `workstreams` skill. Keep Taskwarrior as the source of truth and follow `~/.config/ai-workstreams/SPEC.md`.

For active AI-assisted work, follow the shared scratch pad protocol: resolve or create the task scratch pad under `~/worknotes/scratch/<slug>.md`, set the Taskwarrior `scratch` UDA if missing, read the pad before implementation, and before replying update both scratch Goal/Design/Debugging/Test plan/Learnings and Taskwarrior next transition/state/wait/metadata when material evidence or clarified intent changed.

This is the general entrypoint for Taskwarrior-backed coding workstreams. Route by scope and intent:

- `status`: recorded state only; conductor means all relevant tasks, single-workstream means current/selected task.
- `check` / `refresh`: synonyms. In conductor scope, inspect Taskwarrior+CMUX and align CMUX. In single-workstream scope, inspect relevant external reality and update scratch/Taskwarrior/CMUX.
- `reconcile`: rare conductor override; broadly audit external reality across tasks and align records/UI.
- `update` / `checkpoint` / `handoff`: synonyms for making the current workstream durable and resumable.
- `cleanup`: final ready-phase cleanup, scratch archive/retro, CMUX/local cleanup, and task completion.
- `split off`: create a separate related task without switching current session.
- `open` / `switch` / `resume`: explicitly change the current workstream.
