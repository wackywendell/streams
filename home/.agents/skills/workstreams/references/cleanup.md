# Workstream Cleanup

Cleanup is the flow for completing or archiving a scratch-first stream.

## Done

Use `status: done` when the stream's work is complete. A done stream may remain in `~/worknotes/scratch` until it is explicitly archived.

Before marking done:

1. Verify the work outcome is complete.
2. Update `## Status` with final pickup/closure context.
3. Preserve reusable decisions or learnings in the markdown body.
4. Update `updated`.
5. Update or close CMUX projection as appropriate.

## Archive

`archived` is not a status. A stream is archived by moving its file under `~/worknotes/scratch/archive`.

Do not auto-archive a stream merely because it is `done`. Archive only when explicitly requested or during a cleanup flow that clearly includes archive.
