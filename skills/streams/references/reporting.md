# Guarded agent reporting

Use this protocol for generated progress or description changes outside Pi's built-in reporting helper. Do not generate new record text, then read a fresh base just to make it apply.

## Capture before inference

```sh
streams agent context --harness pi --session-id NATIVE_ID --json
```

Use `codex` or `claude` for those harnesses. Context returns persisted local `stream`, `session`, related `sessions`, effective `authority`, `host_id` and an opaque `catalog_source`. It does not fetch or attach. An optional `--stream-id` supplies an exact initial effort identity; it cannot switch an attached conversation.

Keep this capture immutable while generating one target's changes. A session report can update title, lifecycle status, recap, state, remaining and their matching `_short` forms. Recap records this conversation’s cumulative scope and achievements; state records what it is doing right now. Keep short forms to 1–2 words when possible, preferably under 20 characters and always under 25. A stream description report can update title, goal and goal_short, not stream status or archive state. Omit manually protected names/goals. Produce compact derivatives alongside their source, or let them clear when that source changes.

Before submitting, verify that the same native conversation and attachment are active and its captured evidence remains on the active branch. Appended turns may follow a valid captured prefix; a rewind or branch replacement invalidates it. If catalog selection changed, stop rather than applying to the replacement.

## Submit one target

Send a JSON object on stdin to `streams agent apply --json`, with these fields:

- `operation_id`: a fresh stable ID for this exact intent; never reuse it with changed content.
- `catalog_source`: the original captured source value, not a newly read one.
- `kind`: `stream` or `session`.
- `actor`: `automatic` for generated reports; `user` only for expressly requested edits.
- `before`: the complete original captured target record, including its read-only identity fields.
- `changes`: only the allowed fields being changed, not a replacement record.
- `ownership_token`: the captured `authority.token` for session writes and automatic stream writes. A direct user stream edit does not need it.
- `inputs`: objects with `kind` and `record` for every other captured record supplied to inference. Include sibling sessions if they were shown, deduplicate exact identities, and exclude the target already supplied as `before`.

Use separate captures and inferences for session progress and stream metadata. Capture the second target before its inference, after the first save; do not retrofit either base after inference. User-directed discussion synthesized by a helper still needs its captured inputs. Ordinary direct field edits can retain field-level reconciliation.

Inspect domain, local-save and sharing outcomes separately. `changes` or `no-change` with a successful local outcome is not proof of remote publication. `superseded` means discard automatic output and capture again before a new inference; never substitute a new base into the old request. Explicit conflicts need inspection rather than silent overwriting.

Exit code 2 can include accepted local records and receipts when publication or a later step failed. Preserve these outcomes. A null domain result or repair-required local result is not a successful edit. Do not blindly retry, recreate records or delete repair evidence.

## Initial enrollment

When policy permits enrollment, send a JSON object on stdin to `streams agent enroll --json` with `operation_id`, captured `catalog_source`, `harness`, `harness_session_id`, `session: {title: ...}`, `actor` and an existing absolute `cwd`. Supply either an exact `stream_id` or initial `stream: {title: ..., goal: ...}`; omit goal if unknown. Existing caller-workspace associations take precedence over inventing another effort. A conflicting explicit creation fails rather than switching membership.

Enrollment remembers local directories and binds the caller's workspace without opening, launching or focusing native resources. It may save a stream and a session separately. Keep both receipts, including on partial failure, and inspect context before retrying the exact request. Never turn an interrupted enrollment into a new effort.
