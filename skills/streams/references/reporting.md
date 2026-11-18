# Guarded agent reporting

Use this protocol for generated progress or description changes outside Pi's built-in reporting helper. Do not generate new record text, then read a fresh base just to make it apply.

## Capture before inference

```sh
streams agent context --harness pi --session-id NATIVE_ID --json
```

Use `codex` or `claude` for those harnesses. Context returns persisted local `stream`, `session`, `title_defaults`, `host_id`, `catalog_source` and `patch_schemas`. For each returned record and actor (`automatic` or `user`), `patch_schemas` lists the fields that actor may change with the JSON schema of each value; the automatic lists already omit manually protected names and goals. Its `sessions` list contains only the addressed Session, not a sibling roster. It does not fetch or attach. An optional `--stream-id` supplies an exact initial effort identity; it cannot switch an attached conversation.

Keep this capture immutable while generating one target's changes. A session report can update title, lifecycle status, recap, state, remaining and their matching `_short` forms. Recap records this conversation’s cumulative scope and achievements; state records what it is doing right now. Keep short forms to 1–2 words when possible, preferably under 20 characters and always under 25. A stream description report can update title, goal and goal_short, not stream status or archive state. Omit manually protected names/goals. Produce compact derivatives alongside their source, or let them clear when that source changes.

Before submitting, verify that the same native conversation and attachment are active and its captured evidence remains on the active branch. Appended turns may follow a valid captured prefix; a rewind or branch replacement invalidates it. If catalog selection changed, stop rather than applying to the replacement.

## Submit one target

Send a JSON object on stdin to `streams agent apply --json`, with these fields:

- `version`: `2`; old authority-bearing request formats are rejected.
- `catalog_source`: the original captured source value, not a newly read one.
- `kind`: `stream` or `session`.
- `actor`: `automatic` for generated reports; `user` only for expressly requested edits.
- `before`: the complete original captured target record, including its read-only identity fields and `revision`.
- `changes`: only the allowed fields being changed, not a replacement record.
- `title_default`: the captured resolved `title_defaults.session` or `title_defaults.stream`, matching the target kind. Do not substitute the current setting after inference.
- `inputs`: `[]` for Session edits; for an automatic Stream edit, exactly `[{"kind":"session","record":CAPTURED_SESSION}]`. A direct user Stream edit may omit that dependency with `[]`. Sibling records and ownership tokens are not accepted.

Use separate captures and inferences for session progress and stream metadata. Capture the second target before its inference, after the first save; do not retrofit either base after inference. User-directed discussion synthesized by a helper still needs its captured inputs.

There is no operation ID. Resubmitting the same request is safe: one that already landed returns `no-change`, and one overtaken by a later edit returns `superseded` (automatic) or `conflict` (user). A user edit applies only to the captured `revision`.

Inspect domain, local-save and sharing outcomes separately. `changes` or `no-change` with a successful local outcome is not proof of remote publication. `superseded` means discard automatic output and capture again before a new inference; never substitute a new base into the old request. Explicit conflicts need inspection rather than silent overwriting.

Exit code 2 can include accepted local records when publication or a later step failed. Inspect the outcome first. A null domain result or repair-required local result is not a successful edit. Resubmitting the same captured request is safe; `superseded` or `conflict` means capture again. Do not recreate records or delete repair evidence.

## Reference proposals

A Session inference can produce progress and reference proposals from one capture. Save valid Session progress first. Construct the separate Stream reference request from the original captured Stream and Session after that save, including when progress has no changes. Validate proposals at this later stage so a malformed reference cannot erase accepted progress.

The reference request uses `kind: "stream"` and `changes: {reference_changes: [...]}`, with no ordinary field edits. Each set row has an exact `key` and complete `desired` value. Automatic requests include the captured Session as their sole input; user reference edits have no Session input.

Automatic values contain name, purpose, role, state and target. Document targets carry kind, repository_url, path and optional nonempty ref. Runtime supplies provenance; automatic payloads forbid commit and source_session_id even when null. Existing recovery hints survive omitted automatic values. Explicit user values are complete, so copy recovery metadata before changing selected fields.

Automatic rows compare the complete captured value; user reference edits apply only to the captured Stream `revision`. Additions require the key to be absent at capture and apply. Automatic restoration of Dropped rows is refused per row; user restoration and deletion require explicit direction. User deletion uses desired=null.

Inspect every `reference_results` row. Each row's `desired` is the value it saves, or would save if refused, with recovery hints filled in; it is null for a deletion. A partial Changes result can save independent references while refusing others. Surface the unapplied rows. Resubmitting the same captured request is safe but applies nothing new; a later attempt needs new evidence and a fresh capture. The [reference implementation contract](../../../docs/stream-reference-implementation.md) defines exact packet and result schemas.

## Initial enrollment

When policy permits enrollment, send a JSON object on stdin to `streams agent enroll --json` with `version: 2`, captured `catalog_source`, `harness`, `harness_session_id`, `session: {title: ...}`, `actor` and an existing absolute `cwd`. Supply either an exact `stream_id` or initial `stream: {title: ..., goal: ...}`; omit goal if unknown. Existing caller-workspace associations take precedence over inventing another effort. A conflicting explicit creation fails rather than switching membership.

Enrollment saves records and remembers local directories before best-effort native marker projection, without opening, launching or focusing resources. It may save a Stream and Session separately, and a partial failure reports the Stream it saved. Inspect the outcome and current context. Resubmitting the same request is safe and reuses what already landed; do not turn partial enrollment into a new effort.
