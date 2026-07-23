---
name: streams
description: "Manage AI-assisted coding streams from scratch frontmatter. Use when the user asks for stream status, check/refresh, update/checkpoint/handoff, cleanup, split-off, open/switch/resume, CMUX alignment, or durable scratch state."
metadata:
  short-description: Manage scratch-first coding streams
---

# Streams

Use this skill to manage AI-assisted coding streams from scratch files under `~/worknotes/scratch`.

Scratch frontmatter is the source of truth for current state. The markdown body is the pickup document: goal, current status, important decisions, tradeoffs, deferred choices, and open questions. CMUX, status lists, and future tools are projections of that scratch state.

## CLI First

Use the `streams` CLI for mechanics instead of reimplementing parsing or matching in conversation:

| Need                                  | Command                                               |
| ------------------------------------- | ----------------------------------------------------- |
| List current streams                  | `streams list` or `streams list --json`               |
| Inspect one stream                    | `streams show <slug>` or `streams show <slug> --json` |
| Validate stream files                 | `streams check [slug]`                                |
| Create a stream                       | `streams start "Title"` or `streams s "Title"`        |
| Archive a done stream                 | `streams archive <slug>` or `streams a <slug>`        |
| Resume the latest recorded AI session | `streams resume [slug]`                               |
| Read the stream file format           | `streams format`                                      |
| Open/update CMUX projection           | `streams cmux ...` or `streams c ...`                 |

Use `streams format` as the schema and CLI contract source of truth. If this skill and `streams format` disagree, follow `streams format` and update this skill. Use `streams check [slug]` after editing scratch frontmatter.

## Stream State

One top-level markdown file under `~/worknotes/scratch` is one current stream. Archived streams live under `~/worknotes/scratch/archive`; `archived` is not a status.

Keep scratch frontmatter flat and tool-neutral. Do not store projection state such as CMUX metadata in scratch. Quote any string containing `#`, such as a PR or issue reference: in unquoted YAML, a whitespace-prefixed `#` starts a comment.

Use only the statuses defined by `streams format`. Keep `state` short enough for compact displays. Keep `sessions` ordered oldest to newest; `streams resume` uses the last entry.

Status describes actionability, not the kind or phase of work. Review streams are `active` while the user needs to review or re-review, and `waiting` while a change is with someone else. Put phases such as `PR in review` or `Changes requested; waiting for author updates` in `state`, while `description` remains the stable stream goal.

Set `cwd` to the directory where work should actually resume; `streams cmux open` and `streams resume` depend on it.

## AI Session

The `sessions` field should be a list of active AI sessions. Add yours to the end if not there. Sessions should be in the form `<agent>:<session_id>`, e.g.:

```yaml
sessions:
  - codex:1234abc-345-98712
  - pi:1234def-3afb-56789
  - claude:abc1234-567-8910
```

- In Pi, call `get_session_id` and record `pi:<session_id>`.
- In Codex, use `printenv CODEX_THREAD_ID` and record `codex:<id>`.
- In Claude, record the Claude resume/session id as `claude:<id-or-slug>`.

If a stream has multiple sessions, keep a short `## Sessions` section in the scratch body explaining what each session is for. If there is only one other session besides yours and you did not start the stream, the other session is likely the original/initiator/planner session; label it that way when the scratch context supports it.

## Scratch Body

The scratch body is a current-state pickup document. It records what has been learned from investigation and from the user so a new session or human can understand the goal, current status, important decisions, tradeoffs, and remaining work.

Edit it in place: remove or replace superseded status, outdated decisions, and stale assumptions. It should read like an ADR-style document organized by concept, not a chronological log. Keep `## Status` current; follow `streams format` for the canonical body shape.

Keep long stream-local drafts and outputs under `~/worknotes/scratch/artifacts/<slug>/` and link them from `## Artifacts`; follow `streams format` for retention and lifecycle rules.

## Reference Notes

If a stream produces durable knowledge beyond the immediate task, consider whether `~/worknotes/reference` should be updated. Reference notes are reusable knowledge, not current stream state and not chronological logs.

Before creating or updating reference notes during `check` / `refresh`, `checkpoint` / `handoff`, or `cleanup`, explain what reference changes you recommend and ask the human for their thoughts. Check existing notes first and prefer updating them with the new current understanding over creating duplicates.

Use existing reference categories when they fit:

- `architecture/` for system models, design contracts, and architecture investigations.
- `patterns/` for reusable implementation, review, release, or debugging patterns.
- `tools/` for tool and workflow behavior.
- `runbooks/` for repeatable operational procedures.
- `ideas/` for durable product, research, or strategy observations.

Link created or updated reference notes from the scratch body so future sessions can find them.

## Workflow Verbs

- `status`: read scratch state and recap what is recorded. Do not inspect external systems unless asked.
- `check` / `refresh`: inspect relevant local or external state, then update scratch and CMUX if recorded state is stale. Tell the user what has changed, then give them clear assessment
- `checkpoint` / `handoff` / stream-local `update`: update scratch and CMUX as below.
- `cleanup`: mark `status: done` or archive only with explicit evidence or a clear request. Use `streams archive <slug>` for done streams and `--force` only when intentionally archiving non-done work.
- `split off`: create a separate stream with `streams start "Title"` when related work has its own goal, repo/cwd, state, owner, or session context.
- `open` / `switch` / `resume`: change the active stream context; use `streams resume [slug]` for terminal resume and `streams cmux ...` for workspace projection.

## Update / Handoff Protocol

On stream start or resume, read the scratch file before implementation work.

Before replying after material work, ask:

- Did frontmatter state change?
- Are any frontmatter fields missing or out of date?
  - Is your own AI session in the `sessions` field?
- Does `## Status` still let a new human or AI session continue?
- Did an important decision, tradeoff, deferred choice, or open question change?
- Did the work produce durable reusable knowledge that may belong in `~/worknotes/reference`?
- Does the linked CMUX display need to be updated from scratch?

If no durable pickup context changed, leave scratch alone.

If context has changed:

- Update relevant frontmatter fields, including `updated`, `status`, `description`, and `state` when they changed.
- Check that `sessions`, `repo`, `cwd`, `prs`, `issues`, and other schema fields cover the current state.
- Edit the markdown body so `## Status` is coherent pickup context.
- If multiple sessions are recorded or their roles are ambiguous, add or refresh `## Sessions`.
- Run `streams check [slug]`.
- Update the linked CMUX projection if needed.

## CMUX

CMUX is only a projection of scratch state. Use `streams cmux open <slug>` and `streams cmux update ...` instead of hand-editing workspace title, description, color, or cwd. Use `streams start "Title" --current` or `streams s "Title" -c` to create a stream and immediately project it onto the current CMUX workspace.

Selecting a CMUX workspace is user-visible focus movement. Select a workspace only for explicit `open`, `switch`, `resume`, inspection, or when CMUX requires selection. For pure CMUX mechanics, use the sibling `cmux` skill.

## Recaps

For a single stream, recap briefly:

- `Overall`: current purpose and state.
- `Just completed`: what changed now.
- `Still to do`: concrete next work, blockers, or verification.

For portfolio status, group streams by `active`, `waiting`, `blocked`, `parked`, and `done`. Use `streams list --json` rather than scraping human output.
