---
name: streams
description: "Manage durable Streams work records and session progress. Use for work status, catalog discovery, initial attachment, reporting/checkpoint, field and reference edits, archive/restore, or opening and resuming recorded work."
metadata:
  short-description: Manage durable work and contributions
---

# Streams

Streams records whole efforts separately from their sessions. Read the relevant records before working on an existing effort. Use the CLI or Pi integration, not direct catalog-file edits.

## Record semantics

- A stream has a stable ID, title, optional goal, status, repositories and references. A missing goal is null, not placeholder text.
- Each session belongs to one stream and host. Its concise title names its contribution; `recap` describes its scope and achievements, `state` its current moment (displayed as Status), and `remaining` its unfinished work. Each has a matching `_short` form: prefer 1–2 words and fewer than 20 characters, never 25 or more. Recap changes only with the scope or main outcome; state changes on most reports.
- Status is `active`, `waiting`, `blocked`, `parked` or `done`. Archive controls visibility independently. Neither closes running agents or workspaces.
- Agents may report their own progress and session status. Parking, finishing or archiving the whole effort requires user direction. An explicit request is approval; do not ask for a redundant confirmation.
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
| Attach an existing conversation | `streams session attach STREAM_ID --harness pi --session-id NATIVE_ID --title "Contribution"` |
| Inspect a contribution | `streams session show SESSION_RECORD_ID --json` |
| Make an explicit session edit | `streams session update SESSION_RECORD_ID --status done` |
| Hide or restore work | `streams archive STREAM_ID` / `streams restore STREAM_ID` |
| Open or reuse a workspace | `streams open STREAM_ID` |
| Resume a recorded conversation | `streams resume STREAM_ID --session SESSION_RECORD_ID` |
| Publish pending local work | `streams sync --json` |

Use `--help` for each command's flags. `list` searches literal title/goal substrings, not a query language. References resolve exact IDs first, then exact titles or unique title substrings. Report ambiguity rather than choosing a candidate. Store and reuse exact IDs once resolved.

`show` defaults to YAML; `--json` exposes the same logical records. Refreshed reads may show remote-only records: run explicit `sync` for adoption before editing them. Do not use a display projection as a write base or execution authority.

## Pi work card

- `/streams` or `/streams card` inspects or edits the card without starting inference. Whole-effort and session actions are separate. IDs, ownership and membership are read-only.
- The extension reports automatically after completed responses by default. Settings control reporting, initial enrollment, response frequency, context, model and Herdr startup/naming. Do not duplicate those reports with shell writes.
- `/streams report [instructions]` requests a report, not an approval draft. `/streams reset [--auto]` regenerates automatic fields (and with `--auto`, restores automatic title/goal modes before inference). The model-callable `checkpoint` tool keeps its name and optional `instructions` argument. No Apply/Clear step remains.
- `/streams name [session|stream] <name>` saves a manual title, even when its text is unchanged. `/streams title-mode [session|stream] automatic|manual|inherit` changes only title policy. Both default to Session and require an existing target.
- Pi `/name` imports a different bound Session name as manual by default. Equal names do not pin; unbound chats stay native-only. Import and outgoing synchronization are independent.
- `/streams settings` edits reporting and Herdr settings. Its Naming submenu separates generation defaults, incoming Pi name edits and outgoing synchronization. Defaults affect inherited titles only. Tab completes `card`, `report`, `name`, `title-mode`, `reset`, `settings`, `attach` and `pane`; free-text instructions must follow `report`. Unknown subcommands or extra arguments show usage without reporting or saving.
- `/streams attach`, `select_stream` and `create_stream` are for initial attachment only. Once attached, do not try to switch the conversation to another effort; contribution moves/forks are not implemented.
- Card discussion can answer without editing, refresh descriptions automatically, or carry out explicitly requested field/lifecycle edits. Uncertainty or a suggestion alone is not management permission.
- Follow any session `stream_prompting_policy` governing agent-initiated creation, selection and checkpoints. Do not bypass manual policy with equivalent CLI writes or settings changes. User-invoked commands remain available.

## Other harnesses and generated reports

Obtain the real native conversation ID: Pi's `get_session_id`, Codex's `CODEX_THREAD_ID`, or Claude's actual resume ID. Never invent an identity or copy another session's ID. A session record ID is distinct from that native ID.

Use the [guarded reporting protocol](references/reporting.md) for generated descriptions and progress. Capture records before inference, preserve the captured bases and inputs, and omit protected fields. Ordinary CLI updates are for explicit edits, not a way around generated-report guards.

For `status`, describe recorded state without contacting external systems unless asked. For `check` or `refresh`, verify the requested sources, distinguish findings from cached records, and update only what that evidence supports.

## References and runtime

Keep issues and PRs as HTTP(S) URLs with concise descriptions. Documents name an existing repository alias, a portable branch/bookmark/ref and a repository-relative path. Absolute checkout directories and native workspace identities stay host-local. Repository identity edits use `streams update`, not the card's reference-row editor.

Herdr `open` focuses or restores the recorded agent-tab arrangement, excluding done/parked Sessions unless selected explicitly. It leaves existing tabs and agents untouched. Without an arrangement, multiple eligible Sessions require an explicit choice. `resume` replaces the invoking terminal with one verified saved conversation; it does not reconstruct a workspace. `--current` means the invoking session/workspace's explicit association, never global GUI focus or a guessed title/cwd match. Do not move focus for routine reporting.

Preserve partial-save receipts and uncertain native-attempt evidence. A local save may have succeeded even when publication or a later step failed. Inspect outcomes before retrying; do not delete repair markers, recreate the effort, or launch another workspace to hide the problem. Missing conversations or checkout mappings are visible errors, not permission to guess a file, start a fresh conversation or delete a document reference. Register checkout URL bindings and external-repository defaults explicitly through `streams checkout`; use `streams session directory` for a Session's exact checkout/cwd. Ownership handover, contribution moves and arbitrary shell/editor-tab reconstruction are not implemented. Package publication and live runtime activation require user approval.
