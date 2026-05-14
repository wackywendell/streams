# Workstream Cleanup and Retro

Do not add a `cleanup` workflow-state tag. Cleanup is an actionable phase, so use `+ready` with a concrete cleanup transition such as `Cleanup: archive scratch, extract learnings, prepare CMUX handoff, and mark done`.

Cleanup includes scratch retro work:

1. Update the scratch pad with final `## Test plan` evidence and any final `## Learnings`.
2. Add or update `**Completed:** <YYYY-MM-DD>` near the scratch pad header.
3. Extract reusable knowledge into the worknotes `reference/` tree.
4. Archive the pad to `~/worknotes/scratch/archive/YYYY-MM/<slug>.md`.
5. Update Taskwarrior `scratch:scratch/archive/YYYY-MM/<slug>.md` and annotate `Archived scratch: ~/worknotes/scratch/archive/YYYY-MM/<slug>.md`.
6. Clean local jj bookmark/workspace state as appropriate.
7. Prepare the CMUX workspace for closure, but do not close it automatically from the active AI session. Closing CMUX can end the conversation/session, so it is the final step and should be done by the user or only after explicit confirmation.
8. Mark the Taskwarrior task done only after cleanup/retro is complete and any required CMUX-closure handoff has been recorded.

Use Grove's reference taxonomy under `~/worknotes/reference/`:

```text
reference/
  architecture/   # how systems work: data flow, components, integrations
  runbooks/       # how to do things: commands, auth, deployment, operations
  patterns/       # reusable lessons: gotchas, debugging tricks, process insights
  tools/          # tool/system reference: CLIs, APIs, config, quirks
  ideas/          # proposals, research, future directions
  test-plans/     # verification steps and e2e/manual test plans
```

Retro extraction rules:

- Be comprehensive for `## Learnings`: the author already filtered these, so preserve every item somewhere under `reference/`.
- Be comprehensive for implementation changes: if the scratch pad describes a shipped command, flag, workflow, default, or behavior change, update or create the matching reference doc so it reflects current behavior.
- Be selective elsewhere: extract design/debugging/test-plan details only when they would save future unrelated work.
- Prefer updating an existing reference file over creating a duplicate. If none fits, create a lowercase-kebab-case file in the appropriate subdirectory.
- Skip one-off task status, PR numbers, internal variable names, and details derivable from current code/history/docs.
- After retro, note which reference files were created or updated in the scratch pad or Taskwarrior annotation.
