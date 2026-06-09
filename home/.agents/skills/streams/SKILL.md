---
name: streams
description: "Manage AI-assisted coding streams from scratch frontmatter. Use when the user asks for stream status, check/refresh, update/checkpoint/handoff, cleanup, split-off, open/switch/resume, CMUX alignment, or durable scratch state."
metadata:
  short-description: Manage scratch-first coding streams
---

# Streams

Use this skill to keep AI coding stream state in scratch files under `~/worknotes/scratch`.

Scratch frontmatter is the local source of truth for stream state. The markdown body is the pickup document: current status, goals, decisions, tradeoffs, deferred choices, and open questions. CMUX, status lists, and future tools are projections of that scratch state.

Taskwarrior is not part of this workflow.

## Required Reading

Always read:

- [references/core.md](references/core.md) — stream model, statuses, invariants, and split-off signals.
- [references/scratch.md](references/scratch.md) — frontmatter schema and markdown body protocol.

Then read by scope:

- **Portfolio / conductor scope**: [references/conductor.md](references/conductor.md), plus [references/cmux-integration.md](references/cmux-integration.md) when inspecting or aligning CMUX.
- **Single-stream scope**: [references/task-agent.md](references/task-agent.md), plus [references/cmux-integration.md](references/cmux-integration.md) when a CMUX workspace is open or should be created/updated.
- **Cleanup / retro**: [references/cleanup.md](references/cleanup.md).

Use the sibling [cmux skill](../cmux/SKILL.md) for pure CMUX command mechanics. The streams references define how CMUX is projected from scratch state.

## Command Vocabulary

Commands are interpreted by scope:

- `status`: read-only recorded-state recap from scratch.
- `check` / `refresh`: inspect relevant local/external state, update scratch and CMUX when reality changed, and recap.
- `reconcile`: conductor-only audit across scratch files and CMUX.
- `checkpoint` / `handoff`: single-stream synonyms for making the current scratch file durable and resumable.
- `update`: for conductor, synonym for `check` / `refresh`; for a stream, synonym for `checkpoint` / `handoff`.
- `cleanup`: single-stream completion/archive/retro flow; not a status.
- `split off`: create a related separate scratch stream without switching the current session.
- `open` / `switch` / `resume`: explicitly change the current stream and create/select/resume its CMUX/AI session as needed.

## Mechanics

Use `streams` for recorded stream state:

```bash
streams list
streams list --json
streams show <slug>
streams show <slug> --json
streams check [slug]
streams start "Title"
```

`streams check [slug]` validates scratch frontmatter schema. This is distinct from the workflow command word `check` / `refresh`, which means inspect reality and update stream state.

Before replying after material work, consider whether the scratch frontmatter, `## Status`, decisions, open questions, or linked CMUX display changed usefully. Update them only when the work usefully changed durable state or pickup context.
