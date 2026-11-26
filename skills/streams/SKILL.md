---
name: streams
description: "Manage durable Streams work records and session progress. Use for work status, catalog discovery, initial attachment, reporting/checkpoint, field and reference edits, or resuming recorded work."
metadata:
  short-description: Manage durable work and contributions
---

# Streams

Streams records whole efforts separately from their sessions. Read the relevant records before working on an existing effort. Use the CLI or Pi integration, not direct catalog-file edits.

## Record semantics

- A Stream has a stable ID, a title (`{ text, manual }`), and an optional goal (`{ text, short, manual } | null`). Stream status is derived from its Sessions (`active > waiting > blocked > parked > done > closed`, or `active` when empty) and is not stored or edited directly.
- Each Session belongs to one Stream and host, keyed as `HOST/HARNESS/ID` where `ID` is the harness's real session ID. It owns its title (`{ text, manual }`), status, progress text pairs (`recap` and `next`, each `{ text, short } | null`), working `directory` (`{ path, repository } | null`), `references` (keyed by HTTP(S) URL or path relative to `directory.path`), and `artifacts` (keyed by filename).
- Session status is `active` (work underway), `waiting` (paused waiting for the user: decision, answer, review, approval, or manual step), `blocked` (stopped on an external event or dependency such as CI, another person's review or reply, another team, or an external service), `parked` (deliberately paused), `done` (contribution finished) or `closed`. Marking work `done` or `closed` does not close running agents or terminals.
- Automatic reports may update Session progress, Session references (without restoring `dropped` entries or rewriting manual names/purposes/roles), `directory.repository.branch`, and Session status (`active`, `waiting`, `blocked`, `parked` or `done`, never `closed`). Setting `closed` requires explicit user direction or `streams catalog tidy`.
- Manual marks on `title`, `goal`, and `references` protect user edits from automatic refinement.
- Plans, decisions and substantial drafts belong in project documents referenced from the Session, or in Session artifacts when they have no repository home.
- Record edit times are not verification times. Historical conversation and file contents do not establish that a branch, PR or document is still current.

## Entity CLI

Both local-only (`none`) and Git (`git`) catalogs require explicit `streams catalog init`. If setup is missing, report it; do not initialise or silently choose another catalog without permission.

The CLI does not read Pi env vars, `hc` labels, or frontend state to select records. Pass the selector explicitly: in Pi use `pi/$PI_SESSION_ID` (or `$PI_SESSION_ID` if unique); in other harnesses use `HARNESS/ID`; humans can use full keys, `HARNESS/ID`, or unique ID prefixes from `list`.

| Need | Command |
| --- | --- |
| List Streams (root alias) | `streams list [QUERY] [--all] [--json]` |
| List Streams with fields | `streams stream list [QUERY] [--fields ...] [--json]` |
| Show a Stream and its Sessions | `streams stream show STREAM [--json]` |
| Create a Stream | `streams stream create --title "Title" [--goal "Goal"] [--json]` |
| Edit a Stream | `streams stream edit STREAM --title "Title" [--clear-goal]` |
| Delete an empty Stream | `streams stream delete STREAM [--json]` |
| List Sessions | `streams session list [--stream STREAM] [--all] [--json]` |
| Show a Session | `streams session show SESSION [--json]` |
| Create (enroll) a Session | `streams session create HARNESS/ID --stream STREAM --title "Lane" [--directory PATH]` |
| Edit a Session | `streams session edit SESSION --status done [--recap "..."] [--clear-next]` |
| Move a Session to another Stream | `streams session edit SESSION --stream NEW_STREAM` |
| Add a Session reference | `streams session edit SESSION --add-reference URL_OR_PATH --ref-kind pr\|issue\|link\|document --ref-name NAME --ref-purpose PURPOSE --ref-role deliverable\|dependency\|brief\|background` |
| Drop / restore a reference | `streams session edit SESSION --drop-reference URL_OR_PATH` / `--restore-reference URL_OR_PATH` |
| Read a Session document | `streams session document read SESSION PATH` |
| Resume a Session in this terminal | `streams resume SESSION` (alias for `streams session resume SESSION`) |
| Write a Session artifact | `streams artifact write NAME --session HARNESS/ID --description "..." [--file PATH]` |
| List / read / delete artifacts | `streams artifact list --session SESSION \| --stream STREAM`, `streams artifact read KEY`, `streams artifact delete KEY --confirm` |
| Read or change config | `streams config get [KEY]`, `streams config set KEY VALUE`, `streams config unset KEY` |
| Validate, tidy, or sync catalog | `streams catalog check`, `streams catalog tidy`, `streams catalog sync --json` |

Abandoning a Stream means closing its Sessions (`streams session edit SESSION --status closed`, or `streams catalog tidy` for stale open Sessions). An empty Stream has derived status `active`.

Both `stream edit` and `session edit` also accept `--input -` with a JSON object `{"changes": {...}, "before": {...}?}`. When `before` is provided, each changed unit is checked against the current saved record inside the store's save callback; units whose saved value differs from `before` are left untouched and reported in `not_written` with exit code `3`.

## Pi work card

- `/streams` or `/streams card` opens the work card without starting inference. Stream and Session tabs are separate; `"References…"` lives in the Session tab.
- The extension reports automatically after completed responses by default using the public `streams` CLI. Do not duplicate those reports with shell writes.
- `/streams report [instructions]` requests a report now. `/streams reset [session|stream|all] [--auto]` regenerates automatic fields (with `--auto`, clearing manual marks first). The model-callable `checkpoint` tool requests an immediate report. Streams' tools (`checkpoint`, `stream_read`, `artifact`, `select_stream`, `create_stream`) are reached from codemode scripts, for example `await tools.checkpoint({})`.
- `/streams name [session|stream] <name>` saves a manual title. `/streams title-mode [session|stream] automatic|manual` switches the record's manual mark without changing its text.
- Pi `/name` imports a changed bound Session name as manual when `pi.naming.import_edits` is enabled.
- `/streams attach`, `select_stream`, and `create_stream` are for initial enrollment only. Use `streams session edit SESSION --stream NEW_STREAM` to move an already enrolled Session.
- `/streams settings` configures `pi.reporting.*`, `pi.naming.*`, and `naming.stream` / `naming.session`.
- `/streams sync` runs `streams catalog sync`. `/streams open` and `/streams close` open or close the classic Herdr card pane.

## Other harnesses and generated reports

Obtain the real native conversation ID: Pi's `get_session_id` / `$PI_SESSION_ID`, Codex's `CODEX_THREAD_ID`, or Claude's actual resume ID. Never invent an identity or copy another Session's ID.

Use the [guarded reporting protocol](references/reporting.md) for generated progress or description updates outside Pi's built-in helper. Read persisted records before inference, send `before` checks with `session edit --input -` and `stream edit --input -`, respect manual marks and dropped references, and handle exit code `3` (`not_written`) without blind retries.
