# Streams Scratch Format

Scratch files live in `~/worknotes/scratch`. One top-level markdown file is one current stream. Archived streams live under `~/worknotes/scratch/archive`; `archived` is not a status.

`streams format` is the source of truth for the stream schema and CLI behavior contract.

## Frontmatter

Use flat YAML frontmatter:

```yaml
title: Example Stream
status: active
created: 2026-06-08
updated: 2026-06-10
description: Overall task description
state: Current state in 5-12 words
repo: ~/work/widget-lib
cwd: ~/work/widget-lib/workspaces/example-stream
vcs: jj
issues:
  - https://github.com/example-org/widget-lib/issues/124
prs:
  - https://github.com/example-org/widget-lib/pull/150
sessions:
  - codex:123-abc
```

Required fields:

- `title`: stable human-readable stream name. The filename stem is the slug; title should generally slugify to the filename.
- `status`: one of `active`, `waiting`, `blocked`, `parked`, `review`, or `done`.
- `created`: creation date, `YYYY-MM-DD`.
- `updated`: last meaningful stream-state update date, `YYYY-MM-DD`. Update this for handoffs, checkpoints, status changes, and durable pickup-context changes; do not churn it for typo-only edits.
- `description`: one-sentence overall task description. Longer than `state`, stable across handoffs, and focused on the stream goal.
- `state`: short current display state, roughly 5-12 words. This is used in compact displays and CMUX descriptions.
- `repo`: repo path or repo identity. Prefer a stable human path, using `~` for home and avoiding symlink expansion when the user normally uses the symlink path.
- `cwd`: local directory where work should resume. Use the workspace/worktree path if one exists; otherwise use the repo path. Prefer the same path style as `repo`.
- `vcs`: local VCS, usually `jj` or `git`.
- `issues`: list of GitHub issue URLs.
- `prs`: list of GitHub pull request URLs.
- `sessions`: AI session ids in `codex:<id>`, `claude:<id-or-slug>`, or `pi:<id>` form, ordered oldest to newest. `streams resume` uses the last entry.

Status meanings:

- `active`: currently actionable implementation, investigation, or writing work.
- `waiting`: waiting on external feedback, time, or an event; no immediate local action is expected.
- `blocked`: cannot proceed until a dependency or blocker is resolved.
- `parked`: intentionally inactive; revisit only if the user or new evidence reactivates it.
- `review`: in PR/comment/review loop state.
- `done`: complete and ready to archive.

Issue URLs must look like `https://github.com/<owner>/<repo>/issues/<number>`. PR URLs must look like `https://github.com/<owner>/<repo>/pull/<number>`. Do not use shorthand such as `repo#123` in frontmatter.

Do not include derived or projection-specific fields in frontmatter:

- `slug`: derived from the filename stem.
- `cmux`: CMUX state is a projection and can be stale or workspace-local.
- `current`: currentness is a view/query, not stream state.
- `taskwarrior`: legacy task database state is not part of scratch-first streams.

## Markdown Body

Use this body shape:

```markdown
# <Title>

## Goal

_The outcome this stream is pursuing._

## Status

_Current pickup context: where the task is, what work is in progress, and what needs doing next._

## Decisions

_Important decisions, reasoning, tradeoffs, and deferred choices._

## Sessions

_Optional: one line per recorded AI session when multiple sessions need disambiguation._

## Open Questions

_Unresolved questions that affect how to continue._
```

The body is a current pickup document, not an append-only log. Edit it in place so a new human or AI session can resume from the frontmatter and `## Status`.

Reference notes under `~/worknotes/reference` are optional durable outputs for reusable knowledge that outlives one stream. They are not stream state and should stay up to date rather than chronological. During check/refresh, handoff/checkpoint, and cleanup, consider whether new durable knowledge should update an existing reference note or justify a new one. Ask the human before creating or updating reference notes, and link any resulting notes from the scratch body.

The `sessions` frontmatter field is the ordered machine-readable resume list. Use optional `## Sessions` body notes for human-readable roles or labels when multiple sessions need disambiguation. Keep this concise, usually one line per session:

```markdown
## Sessions

- `codex:123-abc`: original planner/implementation session.
- `pi:456-def`: conductor handoff and cleanup session.
```

If there is only one other session besides the current one, and the current session did not start the stream, the other session is likely the original/initiator/planner session. Label it that way when the scratch context supports it.

## CLI Notes

`streams start "Title"` creates a scratch file only. `streams start "Title" --current` or `streams s "Title" -c` creates the scratch file and projects it onto the current CMUX workspace.

`streams resume [slug]` starts the last session listed in `sessions`. Without a slug, it infers the stream from the current CMUX workspace title or a unique workspace `cwd` match. If `sessions` is empty, start a new AI session intentionally and record it in scratch.

`cwd` must exist and be a directory for `streams resume [slug]` and `streams cmux open <slug>`. `streams resume [slug]` changes into `cwd` before executing the latest recorded AI session. If the agent's saved session directory differs from scratch `cwd`, choose the scratch/current directory unless you intentionally want the old session directory.

`streams cmux` can be abbreviated as `streams c`. `open` and `update` can be abbreviated as `o` and `u`. Use `-c` for `--current` and `-w` for `--workspace`.

`streams archive <slug>` moves a `done` stream to `~/worknotes/scratch/archive/YYYY-MM/`. Use `streams a <slug>` for the short form, `--dry-run` to preview, `--month YYYY-MM` to choose a month, and `--force` only when intentionally archiving a non-`done` stream.

## CMUX Projection

CMUX is a projection of generic stream state:

- title: `title`
- cwd: `cwd`
- color: derived from `status`
- description: italicized `state`, then one issue or PR link per line

`streams cmux open <slug>` creates a new CMUX workspace using the stream `cwd` and the latest recorded session command when present.

`streams cmux update ...` updates title, description, and color from scratch. It does not reliably repair an already-open terminal's working directory. If a workspace was opened with the wrong cwd, close and reopen it after fixing scratch `cwd`.

Example description:

```markdown
_Issue #124 literal conversion helpers_
[widget-lib#124](https://github.com/example-org/widget-lib/issues/124)
[widget-lib#150](https://github.com/example-org/widget-lib/pull/150)
```
