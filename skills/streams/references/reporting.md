# Guarded agent reporting

Use this protocol for generated progress or description updates outside Pi's built-in reporting helper. Do not generate new text and then re-read a fresh record just to force an overwrite.

## 1. Read before inference

Read the current Session, its Stream, and host naming settings using the public CLI:

```sh
streams session show HARNESS/ID --json
streams stream show HOST/STREAM_ID --json
streams config get naming --json
```

Keep these snapshots immutable while generating updates:

- A **Session report** sees the conversation, the Session's own `references` (including `dropped` entries), and `session.directory`. It can propose updates to `title` (when `!session.title.manual` and `naming.session == "automatic"`), `status` (`active`, `waiting`, `blocked`, `parked`, `done` — never `closed`), `recap`, `next` (nullable; set to `null` when done with no genuine follow-up), `directory.repository.branch` (when `directory.repository` is set), and `references`.
  - Status meanings: `active` = work underway; `waiting` = paused waiting for the user (decision, answer, review, approval, or manual step — not merely because a turn ended during ongoing work); `blocked` = stopped on an external event or dependency (CI, another person's PR review or reply, another team or ticket, upstream fix, or external access/service issue); `parked` = explicitly paused; `done` = this contribution finished with no newer accepted task remaining.
- A **Stream report** updates only `title` (when `!stream.title.manual` and `naming.stream == "automatic"`) and `goal` (when `!stream.goal?.manual`). It never edits Stream status (which is derived from Sessions).
- For short forms (`short` inside `goal`, `recap`, `next`), keep under 25 characters (prefer 1–3 words). Never echo lifecycle status or repeat the Session title; use `null` when no short form adds new information.

## 2. Submit checked edits via `--input -`

Pipe a JSON object `{"changes": {...}, "before": {...}}` to `session edit` or `stream edit`:

```sh
streams session edit HOST/HARNESS/ID --input - --json <<'EOF'
{
  "changes": {
    "next": { "text": "Merge PR #43 once CI passes", "short": "Merge PR #43" },
    "references": {
      "https://github.com/o/r/pull/43": { "status": "fulfilled" },
      "docs/design.md": {
        "kind": "document",
        "name": "Design",
        "purpose": "Guides implementation",
        "role": "brief",
        "status": "active",
        "manual": false
      }
    }
  },
  "before": {
    "next": { "text": "Finish unit tests for entity CLI", "short": "Finish CLI tests" },
    "references": {
      "https://github.com/o/r/pull/43": {
        "kind": "pr",
        "name": "Add entity CLI",
        "purpose": "Implements redesign",
        "role": "deliverable",
        "status": "active",
        "manual": false
      },
      "docs/design.md": null
    }
  }
}
EOF
```

### Edit rules

- When `before` is supplied, its keys must match `changes` exactly at the top level and inside `references`.
- Each value in `changes` is merged onto its `before` value (modified RFC 7386: explicit `null` sets the member to `null`; absent members keep their `before` value). Use `null` in `before` for a new reference or an unset optional field.
- Reference rules for automatic reports:
  - Never restore or touch a `dropped` reference.
  - Never overwrite `name`, `purpose`, or `role` on a `manual: true` reference (only `status` may advance, keeping `manual: true`).
  - Document keys must be relative to `session.directory.path` (no `..` or paths outside the directory). Save external outputs with `streams artifact write` instead.
- Check the exit code and response JSON `{"key": "...", "not_written": {...}}`:
  - Exit `0`: every requested unit now matches the requested value.
  - Exit `3`: the command succeeded partially; units whose saved value differed from `before` were left untouched and appear in `not_written` with their current saved values. Log them and do not retry blindly — wait for the next turn and a fresh read.
  - Exit `1` or `2`: the request failed and wrote nothing.

## 3. Initial enrollment

When no Session record exists yet and policy permits enrollment:

1. Create a Stream if none is chosen:
   ```sh
   streams stream create --title "Effort" --title-mode automatic [--goal "..."] [--goal-mode automatic] --json
   ```
2. Create the Session with its exact `HARNESS/ID` and working directory:
   ```sh
   streams session create HARNESS/ID --stream HOST/STREAM_ID --title "Lane" --title-mode automatic --directory "$PWD" --json
   ```
3. Re-read the created records before running the first Session/Stream report.
