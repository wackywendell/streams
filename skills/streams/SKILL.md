---
name: streams
description: "Manage durable Streams work records and session progress. Use for work status, catalog discovery, initial attachment, reporting/checkpoint, field and reference edits, or opening and resuming recorded work."
metadata:
  short-description: Manage durable work and contributions
---

# Streams

Streams records whole efforts separately from their sessions. Read the relevant records before working on an existing effort. Use the CLI or Pi integration, not direct catalog-file edits.

## Record semantics

- A stream has a stable ID, title, optional goal, repositories and references. A missing goal is null, not placeholder text. Stream status is derived from its sessions (`active > blocked > waiting > parked > done > closed`, or `active` when empty) and is not stored or edited directly.
- Each session belongs to one stream and host. Its concise title names its contribution; `recap` describes its scope and achievements, `state` its current moment (displayed as Status), and `remaining` its unfinished work. Each has a matching `_short` form: prefer 1–2 words and fewer than 20 characters, never 25 or more. Recap changes only with the scope or main outcome; state changes on most reports.
- Session status is `active`, `blocked`, `waiting`, `parked`, `done` or `closed`. Marking work `done` does not close running agents or workspaces.
- Automatic reports may update session progress and session status (`active`, `blocked`, `waiting`, `parked` or `done`, never `closed`). Setting `closed` on a session requires explicit user direction or `streams tidy`. An explicit request is approval; do not ask for a redundant confirmation.
- Manual title/goal modes protect text from automatic refinement. Change a mode to automatic only when requested. Do not change scope while improving wording.
- Plans, decisions and substantial drafts belong in project documents linked from the stream. Keep progress concise rather than copying those documents into records.
- Record edit times are not verification times. Historical conversation and file contents do not establish that a branch, PR or document is still current.

## CLI first

Both local-only and Git catalogs require explicit `streams init`. If setup is missing, report it; do not initialise or silently choose another catalog without permission.

| Need | Command |
| --- | --- |
| Find work | `streams list [TEXT] --json` |
| Inspect a stream and its sessions | `streams show STREAM_ID --json` |
| Validate saved records | `streams check` |
| Create without opening anything | `streams create "Title" [--goal "Goal"]` |
| Create and open a workspace | `streams start "Title"` |
| Edit explicit fields | `streams update STREAM_ID --title "Title"` |
| Clear a goal | `streams update STREAM_ID --clear-goal` |
| Inspect every reference state | `streams reference list STREAM_ID --json` |
| Drop a reference | `streams reference drop STREAM_ID --url URL` |
| Restore a dropped reference | `streams reference restore STREAM_ID --url URL --state active` |
| Forget a reference | `streams reference delete STREAM_ID --url URL --confirm` |
| Attach an existing conversation | `streams session attach STREAM_ID --harness pi --session-id NATIVE_ID --title "Contribution"` |
| Inspect a contribution | `streams session show SESSION_RECORD_ID --json` |
| Make an explicit session edit | `streams session update SESSION_RECORD_ID --status done` |
| Abandon a stream | Set each session closed: `streams session update SESSION_RECORD_ID --status closed` (an empty stream reads `active` until `lifecycle.decay_days`) |
| Close stale open sessions | `streams tidy` |
| Open or reuse a workspace | `streams open STREAM_ID` |
| Resume a recorded conversation | `streams resume STREAM_ID --session SESSION_RECORD_ID` |
| Publish pending local work | `streams sync --json` |

Use `--help` for each command's flags. `list` searches literal title/goal substrings, not a query language. Stream selectors resolve exact IDs first, then exact titles or unique title substrings; UUID prefixes are not supported. Report ambiguity rather than choosing a candidate. Store and reuse exact IDs once resolved.

`show` defaults to YAML; `--json` exposes the same logical records. Ordinary reads are local. Explicit `--refresh` runs sync before reading and can publish local changes; it is not a remote-only display projection. Generated reports use captured persisted records.

## Pi work card

- `/streams` or `/streams card` inspects or edits the card without starting inference. Stream and session actions are separate. IDs, ownership and membership are read-only.
- The extension reports automatically after completed responses by default. Settings control reporting, initial enrollment, response frequency, context, model and Herdr startup/naming. Do not duplicate those reports with shell writes.
- `/streams report [instructions]` requests a report, not an approval draft. `/streams reset [--auto]` regenerates automatic fields (and with `--auto`, restores automatic title/goal modes before inference). The model-callable `checkpoint` tool keeps its name and optional `instructions` argument. No Apply/Clear step remains.
- `/streams name [session|stream] <name>` saves a manual title, even when its text is unchanged. `/streams title-mode [session|stream] automatic|manual|inherit` changes only title policy. Both default to Session and require an existing target.
- Pi `/name` imports a different bound Session name as manual by default. Equal names do not pin; unbound chats stay native-only. Import and outgoing synchronization are independent.
- `/streams settings` edits reporting and Herdr settings. Its Naming submenu separates generation defaults, incoming Pi name edits and outgoing synchronization. Defaults affect inherited titles only. Tab completes `card`, `report`, `name`, `title-mode`, `reset`, `settings`, `attach` and `pane`; free-text instructions must follow `report`. Unknown subcommands or extra arguments show usage without reporting or saving.
- `/streams attach`, `select_stream` and `create_stream` are for initial attachment only. Once attached, these tools cannot switch efforts. Native pane and workspace moves never change membership, and there is no Session-move CLI or Pi switch action yet.
- Card discussion can answer without editing, refresh descriptions automatically, or carry out explicitly requested field/lifecycle edits. Uncertainty or a suggestion alone is not management permission.
- Follow any session `stream_prompting_policy` governing agent-initiated creation, selection and checkpoints. Do not bypass manual policy with equivalent CLI writes or settings changes. User-invoked commands remain available.

## Other harnesses and generated reports

Obtain the real native conversation ID: Pi's `get_session_id`, Codex's `CODEX_THREAD_ID`, or Claude's actual resume ID. Never invent an identity or copy another session's ID. A session record ID is distinct from that native ID.

Use the [guarded reporting protocol](references/reporting.md) for generated descriptions and progress. Capture records before inference, preserve the captured bases and inputs, and omit protected fields. Ordinary CLI updates are for explicit edits, not a way around generated-report guards.

For `status`, describe recorded state without contacting external systems unless asked. For `check` or `refresh`, verify the requested sources, distinguish findings from cached records, and update only what that evidence supports.

## References and runtime

References carry a name, purpose, role (`deliverable`, `dependency`, `brief` or `background`) and state (`active`, `fulfilled` or `dropped`). A document uses an exact repository URL and portable relative path, with optional recovery hints. PR, Issue and Link share exact HTTP(S) URL identity. Preserve supplied spelling and reuse an established saved identity when the conversation recognises an equivalent resource.

Infer additions, updates and drops only from clear retained conversation evidence of identity and relevance to the whole effort. Omit uncertain relationships. Attempted file paths establish activity; they cannot establish file existence. Keep the configured trimmed/full evidence boundary and omit useful URLs available only in discarded tool output.

Drop retains identity and blocks automatic restoration. Delete forgets it and allows rediscovery, including through an in-flight report. User direction is required for deletion and restoration. Reference fulfillment or external merge state never finishes a Stream. Missing checkout mappings leave saved references intact.

Use `streams reference add/edit/drop/restore/delete` for explicit reference actions, with document selectors `--repository-url=URL --path=PATH` or web selectors `--url=URL`. Add requires kind, name, purpose and role. List all states through `streams reference list`. Preserve exact machine values and decode unsafe `selector-json` output before passing separate process arguments. Absolute checkout directories and native workspace identities stay host-local; repository configuration remains a separate `streams update` action.

Herdr `open` discovers active same-host Sessions and opens missing conversations in separate tabs. Already-observed running Sessions stay where they are. `--session` selects one conversation explicitly, including inactive work. There is no saved arrangement or layout restoration. `resume` replaces the invoking terminal with one verified saved conversation. `--current` means the invoking session/workspace's explicit association, never global GUI focus or a guessed title/cwd match. Do not move focus for routine reporting.

Read partial-save outcomes and diagnostics. A local save may have succeeded even when publication or a later step failed. Inspect outcomes first; resubmitting the same captured request is safe, and `superseded` or `conflict` means capture again; do not recreate the effort or launch another workspace to hide the problem. Native actions have no saved attempt to resume: a lost reply stops the pass, and a later explicit request observes again. Best-effort reuse can still leave duplicates. Missing conversations or checkout mappings are visible errors, not permission to guess a file, start a fresh conversation or delete a document reference. Register checkout URL bindings and external-repository defaults explicitly through `streams checkout`; use `streams session directory` for a Session's exact checkout/cwd. Ownership handover, cross-host Session moves and arbitrary shell/editor-tab reconstruction are unsupported. Directory selection is host-local and does not populate portable `session.checkout`; register a URL or pass an existing repository alias rather than infer one. Package publication and live runtime activation require user approval.
