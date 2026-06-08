# Legacy Taskwarrior Integration

Taskwarrior is no longer part of the active workstreams workflow.

Use this reference only when migrating or interpreting older workstream records. New workstream state belongs in scratch frontmatter under `~/worknotes/scratch`, and `streams` is the command surface for list/show/start behavior.

When migrating an old Taskwarrior workstream:

1. Create or locate the scratch file.
2. Convert the task title, description, repo/workspace, PR, and AI session fields into flat scratch frontmatter.
3. Convert the latest useful handoff annotation into `## Status`.
4. Preserve important decisions, tradeoffs, and open questions in the markdown body.
5. Do not keep Taskwarrior as a parallel source of truth.
