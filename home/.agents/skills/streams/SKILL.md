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

Use `streams format` as the schema source of truth. Use `streams check [slug]` for frontmatter validation.

## Stream State

One top-level markdown file under `~/worknotes/scratch` is one current stream. Archived streams live under `~/worknotes/scratch/archive`; `archived` is not a status.

Keep scratch frontmatter flat and tool-neutral. Do not add `cmux`, `current`, or `slug` fields. The slug is the filename stem, and is a normalized version of the title.

Use exactly these statuses: `active`, `waiting`, `blocked`, `parked`, `review`, and `done`. Keep `state` short enough for compact displays. Keep `sessions` ordered oldest to newest; `streams resume` uses the last entry.

The markdown body should be edited in place so it remains coherent. `## Status` is current pickup context.

## Scratch Body

The scratch body is a current-state pickup document. It records what has been learned from investigation and from the user so a new session or human can understand the goal, current status, important decisions, tradeoffs, and remaining work. Edit it in place: remove or replace superseded status, outdated decisions, and stale assumptions. It should read like an ADR-style document organized by concept, not a chronological log.

### Goal

Describe the task's overall goal: why we want this and what outcome we are trying to achieve. Include concrete requirements and softer expectations such as PR flow, compatibility, review posture, or migration constraints when they have been made clear by the user or in discussion with the user.

### Status

Describe the current state of the work: what goal has been achieved, what is in progress, what remains, and what is blocked or waiting. Keep this as pickup context, not a dated activity log.

### Design Decisions

Document important design and workflow decisions, including reasoning and tradeoffs. Use concept-specific subsections when useful, such as `#### Syntax` for a DSL, `#### API Design` for public interfaces, `#### Migration` for rollout choices, or `#### Testing` for verification strategy. Include deferred decisions when they affect how to continue.

### Open Questions

List unresolved questions that affect implementation, review, cleanup, or handoff.

## Workflow Verbs

- `status`: read scratch state and recap what is recorded. Do not inspect external systems unless asked.
- `check` / `refresh`: inspect relevant local or external state, then update scratch and CMUX if recorded state is stale.
- `checkpoint` / `handoff` / stream-local `update`: make the current stream durable and resumable.
- `cleanup`: mark `status: done` or archive only with explicit evidence or a clear request. Use `streams archive <slug>` for done streams and `--force` only when intentionally archiving non-done work.
- `split off`: create a separate stream with `streams start "Title"` when related work has its own goal, repo/cwd, state, owner, or session context.
- `open` / `switch` / `resume`: change the active stream context; use `streams resume [slug]` for terminal resume and `streams cmux ...` for workspace projection.

## Update Protocol

On stream start or resume, read the scratch file before implementation work.

Before replying after material work, ask:

- Did frontmatter state change?
- Are any frontmatter fields missing or out of date?
  - Is your own AI session in the `sessions` field?
- Does `## Status` still let a new human or AI session continue?
- Did an important decision, tradeoff, deferred choice, or open question change?
- Does the linked CMUX display need to be updated from scratch?

If yes, update scratch and CMUX before responding. If no durable pickup context changed, leave scratch alone.

## CMUX

CMUX is only a projection of scratch state. Use `streams cmux open <slug>` and `streams cmux update ...` instead of hand-editing workspace title, description, color, or cwd. Use `streams start "Title" --current` or `streams s "Title" -c` to create a stream and immediately project it onto the current CMUX workspace.

Selecting a CMUX workspace is user-visible focus movement. Select a workspace only for explicit `open`, `switch`, `resume`, inspection, or when CMUX requires selection. For pure CMUX mechanics, use the sibling `cmux` skill.

## Recaps

For a single stream, recap briefly:

- `Overall`: current purpose and state.
- `Just completed`: what changed now.
- `Still to do`: concrete next work, blockers, or verification.

For portfolio status, group streams by `active`, `waiting` / `review`, `blocked`, `parked`, and `done`. Use `streams list --json` rather than scraping human output.
