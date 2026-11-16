# Guarded agent reporting

Use this protocol for generated progress or description changes outside Pi's built-in reporting helper. Do not generate new record text, then read a fresh base just to make it apply.

## Capture before inference

```sh
streams agent context --harness pi --session-id NATIVE_ID --json
```

Use `codex` or `claude` for those harnesses. Context returns persisted local `stream`, `session`, `title_defaults`, `host_id` and `catalog_source`. Its `sessions` list contains only the addressed Session, not a sibling roster. It does not fetch or attach. An optional `--stream-id` supplies an exact initial effort identity; it cannot switch an attached conversation.

Keep this capture immutable while generating one target's changes. A session report can update title, lifecycle status, recap, state, remaining and their matching `_short` forms. Recap records this conversation’s cumulative scope and achievements; state records what it is doing right now. Keep short forms to 1–2 words when possible, preferably under 20 characters and always under 25. A stream description report can update title, goal and goal_short, not stream status or archive state. Omit manually protected names/goals. Produce compact derivatives alongside their source, or let them clear when that source changes.

Before submitting, verify that the same native conversation and attachment are active and its captured evidence remains on the active branch. Appended turns may follow a valid captured prefix; a rewind or branch replacement invalidates it. If catalog selection changed, stop rather than applying to the replacement.

## Submit one target

Send a JSON object on stdin to `streams agent apply --json`, with these fields:

- `version`: `2`; old authority-bearing request formats are rejected.
- `operation_id`: a fresh stable ID for this exact intent; never reuse it with changed content.
- `catalog_source`: the original captured source value, not a newly read one.
- `kind`: `stream` or `session`.
- `actor`: `automatic` for generated reports; `user` only for expressly requested edits.
- `before`: the complete original captured target record, including its read-only identity fields.
- `changes`: only the allowed fields being changed, not a replacement record.
- `title_default`: the captured resolved `title_defaults.session` or `title_defaults.stream`, matching the target kind. Do not substitute the current setting after inference.
- `inputs`: `[]` for Session edits; for an automatic Stream edit, exactly `[{"kind":"session","record":CAPTURED_SESSION}]`. A direct user Stream edit may omit that dependency with `[]`. Sibling records and ownership tokens are not accepted.

Use separate captures and inferences for session progress and stream metadata. Capture the second target before its inference, after the first save; do not retrofit either base after inference. User-directed discussion synthesized by a helper still needs its captured inputs. Ordinary direct field edits can retain field-level reconciliation.

Inspect domain, local-save and sharing outcomes separately. `changes` or `no-change` with a successful local outcome is not proof of remote publication. `superseded` means discard automatic output and capture again before a new inference; never substitute a new base into the old request. Explicit conflicts need inspection rather than silent overwriting.

Exit code 2 can include accepted local records and receipts when publication or a later step failed. Preserve these outcomes. A null domain result or repair-required local result is not a successful edit. Do not blindly retry, recreate records or delete repair evidence.

## Reference proposals

A Session inference can produce progress and reference proposals from one capture. Save valid Session progress first. Construct the separate Stream reference request from the original captured Stream and Session after that save, including when progress has no changes. Validate proposals at this later stage so a malformed reference cannot erase accepted progress.

The reference request uses `kind: "stream"` and `changes: {reference_changes: [...]}`, with no ordinary field edits. Each set row has an exact `key`, complete `desired` value and `next_revision` equal to this operation's ID. Use a distinct ID for each effective operation. Automatic requests include the captured Session as their sole input; user reference edits have no Session input.

Automatic values contain name, purpose, role, state and target. Document targets carry kind, repository_url, path and optional nonempty ref. Runtime supplies provenance; automatic payloads forbid commit and source_session_id even when null. Existing recovery hints survive omitted automatic values. Explicit user values are complete, so copy recovery metadata before changing selected fields.

Captured existing rows compare their complete value and revision. Additions require the key to be absent at capture and apply. Automatic restoration of Dropped rows is refused per row; user restoration and deletion require explicit direction. User deletion uses desired=null and omits next_revision.

Inspect every `reference_results` row. A partial aggregate Changes receipt can preserve independent accepted references while refusing others. Retain the receipt and saved progress, surface unapplied rows and stop later inference without recapture or retry. New inference needs new evidence and capture. The [reference implementation contract](../../../docs/stream-reference-implementation.md) defines exact packet and result schemas.

## Initial enrollment

When policy permits enrollment, send a JSON object on stdin to `streams agent enroll --json` with `version: 2`, `operation_id`, captured `catalog_source`, `harness`, `harness_session_id`, `session: {title: ...}`, `actor` and an existing absolute `cwd`. Supply either an exact `stream_id` or initial `stream: {title: ..., goal: ...}`; omit goal if unknown. Existing caller-workspace associations take precedence over inventing another effort. A conflicting explicit creation fails rather than switching membership.

Enrollment saves records and remembers local directories before best-effort native marker projection, without opening, launching or focusing resources. It may save a Stream and Session separately. Keep both receipts on partial failure. Inspect actual context and retained receipts before deciding whether to resubmit the original request; do not blindly retry or turn partial enrollment into a new effort.
