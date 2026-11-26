# Session card fields: recap and next

Status: Implemented
Date: 2026-11-26

## Purpose

A Session card helps a returning person remember what a conversation is about and what it needs without reopening the transcript. Previously each Session stored three prose fields (`recap`, `state`, and `remaining`), and the Herdr right panel rendered `state` first and brightest. In practice, `state` and `remaining` overlapped on active work, `state` and `recap` duplicated each other on finished work, and `state` frequently relied on short-lived turn context such as commit hashes, jujutsu change IDs, internal terminal or dashboard IDs, and unlabeled choices like Option A or B.

This design collapses the three Session prose fields into two fields (`recap` and `next`), bumps the catalog record format from version 5 to version 6, orders every UI surface with `recap` first, and tightens the reporting prompt so every line stands on its own after a day away.

## Terms

- Session. A durable catalog record representing one agent conversation belonging to a Stream.
- TextPair. A `{ text, short }` object holding a full sentence (`text`) and an optional compact form under 25 characters (`short`) for narrow lists.
- Recap. The Session's `recap` TextPair, stating what the Session is working on and its main outcome so far in one self-contained sentence.
- Next. The Session's `next` TextPair, stating what is waiting, blocked, or needs to happen next in one self-contained sentence. It is `null` when finished with no genuine manual follow-up.
- Format 6. The catalog record schema version (`FORMAT_VERSION = 6` in `.catalog-local/records.json`) in which Session records carry `recap` and `next` without `state` or `remaining`.
- Cold reader. A person returning to a Session card hours or a day later who does not have the latest agent turn in memory.

## Assumptions

- Streams is alpha software. Runtime code reads only the current record format (format 6) and rejects older formats with a clear error.
- Existing catalogs on active machines are converted out of band by a one-off migration script or reset before running the format 6 binary.
- Automatic Session reports run through the configured Pi reporting model using trimmed conversation context and strict schema validation.

## Guarantees

1. One concept, one name across all surfaces. The catalog record, CLI flags, Pi panel, classic Herdr TUI card, and Herdr app right panel all name the two Session prose fields `recap` and `next`.
2. Orientation comes before action. Every Session view (`herdr-app` right panel, `herdr-app` popup, Pi panel, and TUI card) renders `Recap` ahead of `Next`, with `Recap` in primary text styling.
3. Stream list rows carry concrete signal. Each Session row in the `herdr-app` Stream view shows `next` short text when present and falls back to `recap` short text when `next` is `null`.
4. Strict format separation. Opening a format 5 catalog with a format 6 binary fails explicitly at store open before reading or mutating records.
5. Clearable `next`, durable `recap`. Automatic reports may clear `next` to `null` when work finishes, while `recap` cannot be cleared by an automatic report.

## Implementation

### Pieces

The change spans five layers in lockstep.

```text
  Pi reporting model (agent.ts prompts + 2-field wire schema)
          |
          v  record_card { changes: { title?, status?, recap?, next?, branch? } }
  Pi extension (core.ts, panel.ts)  --->  streams CLI (session create/edit/show/list)
                                                   |
                                                   v  format-6 JSON (recap, next)
                                           Rust model + store (FORMAT_VERSION = 6)
                                             /                     \
                                            v                       v
                              herdr-app mod (view_session,     Classic TUI & markers
                              view_stream, viewer, state)      (render.rs, projection.rs)
```

1. Record model and store (`src/model.rs`, `src/file_store/records.rs`). `Session` replaces `state` and `remaining` with `next: Option<TextPair>`, keeping `#[serde(deny_unknown_fields)]`. `FORMAT_VERSION` advances from `5` to `6`.
2. Edits, field projections, and CLI (`src/operations/edits.rs`, `src/application/fields.rs`, `src/cli/args.rs`, `src/cli/sessions.rs`). `SESSION_EDITABLE_FIELDS` and `SESSION_FIELDS` expose `recap` and `next`. `StreamSessionSummary` (embedded in `stream show` under `sessions`) replaces `state` with `recap` and `next`. `SessionProgressArgs` replaces `--state*` and `--remaining*` with `--next`, `--next-file`, `--clear-next`, and `--next-short`.
3. Pi reporting and panel (`extensions/stream-checkpoint/`). `wire.ts`, `core.ts`, `agent.ts`, and `panel.ts` adopt `recap` and `next`. `buildSessionReportEdit` allows automatic reports to clear `next` to `null` while rejecting automatic clears of `recap`. The reporting prompt enforces the cold-reader standard: 10 to 15 word single-clause sentences, concrete decision choices, no commit hashes, change IDs, internal IDs, or test counts, and `short` forms that name the subject (`recap.short`) or concrete action (`next.short`) without echoing the lifecycle status or title.
4. Herdr app mod (`integrations/herdr-app/`). `state.js` normalizes `recap`, `recap_short`, `next`, and `next_short`, fetches off-tab summaries with `--fields key,stream_id,title,status,recap,next`, and exposes `sessionRowSubtitle(session)` returning `session.next_short || session.recap_short || ""`. `view_session.js` renders `Recap` first in primary text (12px foreground) and `Next` second (11.5px muted). `view_stream.js` renders `sessionRowSubtitle(sess)` on the second line of each Session row. `viewer.js` renders `Recap` and `Next` in the Session popup, and `frame.js` reads live Session state from `coordinator.sessions` when opening the popup.
5. Classic TUI and native markers (`src/tui/render.rs`, `src/runtime/native_cards/projection.rs`). The TUI card renders Stream goal, `Recap`, and `Next`. Native pane metadata markers publish `strm_recap_short` and `strm_next_short`.

### Walkthrough of one Session lifecycle

A developer investigates two schema bugs in a Pi session and pauses to decide between full-state reports and sparse deltas. The automatic report writes `status: "waiting"`, `recap: { text: "Diagnosed session report validation failures and silent field wipes, and proposed schema fixes.", short: "Null handling" }`, and `next: { text: "Choose the report fix: full-state session reports or sparse deltas, and how loud failures should be.", short: "Pick fix direction" }`.

In `herdr-app`, the Stream roster row shows `Report Schema [waiting]` with `Pick fix direction` underneath. Opening the Session view shows `Recap` first in foreground text so the returning developer remembers what the session found, followed by `Next` with the exact decision. After the developer picks an approach and pushes the fix, the next incremental report sends `{ status: "done", recap: { ... }, next: null }`. The `Next` block disappears from the Session view, and the Stream roster row automatically falls back to `recap_short`.

## Failures

- Unmigrated format 5 catalog (Guarantee 4). `FileRecordStore` reads `.catalog-local/records.json` before loading records, sees version `5 != 6`, and returns an error pointing to the reset steps in `README.md`. No partial reads or silent drops occur.
- Mixed-version sync over Git transport (Guarantee 4). Each host writes only its own partition under `hosts/<host_id>/`, and `Session` uses `deny_unknown_fields`. If one host still runs format 5 while another runs format 6, foreign-host format 5 Session records fail strict decoding and surface as explicit `unreadable` notices rather than corrupting local records. Upgrading both hosts and migrating the shared Git catalog together avoids unreadable foreign records.
- Model returns null, empty, or malformed `recap` (Guarantee 5). `parseInferenceResponse` strips `recap: null` and blank `recap.text` before validation (keeping the saved `recap` intact) and rejects non-object `recap` or invalid field types through TypeBox checks. `buildSessionReportEdit` refuses to clear `recap` on automatic reports independently.
- Model omits `short` on `recap` or `next` (Guarantee 3). `shortOf` in `integrations/herdr-app/state.js` falls back to `text`, and `view_stream.js` truncates the row line cleanly, so the Stream roster still shows readable text rather than an empty line.
