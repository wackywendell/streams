# Streams Scratch Format

Scratch files live in `~/worknotes/scratch`. One top-level markdown file is one current stream. Archived streams live under `~/worknotes/scratch/archive`; `archived` is not a status.

`streams format` is the source of truth for the stream schema and CLI behavior contract.

## Frontmatter

Use a top-level, tool-neutral YAML frontmatter mapping:

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
  - url: https://github.com/example-org/widget-lib/issues/124
    description: Literal conversion helpers
prs:
  - url: https://github.com/example-org/widget-lib/pull/150
    description: Add literal helpers
sessions:
  - codex:123-abc
```

Quote any frontmatter string containing `#`, such as a PR or issue reference. In unquoted YAML, a whitespace-prefixed `#` starts a comment:

```yaml
state: "PR #186 design review complete; feedback ready."
```

Required fields:

- `title`: stable human-readable stream name. The filename stem is the slug; title should generally slugify to the filename.
- `status`: one of `active`, `waiting`, `blocked`, `parked`, or `done`. Status describes whether the stream is actionable, waiting, unable to proceed, intentionally inactive, or complete; it does not describe the type or phase of work.
- `created`: creation date, `YYYY-MM-DD`.
- `updated`: last meaningful stream-state update date, `YYYY-MM-DD`. Update this for handoffs, checkpoints, status changes, and durable pickup-context changes; do not churn it for typo-only edits.
- `description`: one-sentence overall task description. Longer than `state`, stable across handoffs, and focused on the stream goal.
- `state`: short current display state, roughly 5-12 words. This is used in compact displays and CMUX descriptions. Put transient workflow phases here, such as `PR in review; waiting for reviewer feedback.`
- `repo`: repo path or repo identity. Prefer a stable human path, using `~` for home and avoiding symlink expansion when the user normally uses the symlink path.
- `cwd`: local directory where work should resume. Use the workspace/worktree path if one exists; otherwise use the repo path. Prefer the same path style as `repo`.
- `vcs`: local VCS, usually `jj` or `git`.
- `issues`: list of GitHub issue mappings. Each has a `url` and a manually maintained `description` of 2 to 5 words. Replace older URL-only entries with these mappings.
- `prs`: list of GitHub pull request mappings. Each has a `url` and a manually maintained `description` of 2 to 5 words. Replace older URL-only entries with these mappings.
- `sessions`: AI session ids in `codex:<id>`, `claude:<id-or-slug>`, or `pi:<id>` form, ordered oldest to newest. `streams resume` uses the last entry.

Status meanings:

- `active`: currently actionable implementation, investigation, or writing work.
- `waiting`: waiting on external feedback, time, or an event; no immediate local action is expected.
- `blocked`: cannot proceed until a dependency or blocker is resolved.
- `parked`: intentionally inactive; revisit only if the user or new evidence reactivates it.
- `done`: complete and ready to archive.

For review work, follow the next action rather than using a review-specific status:

- Use `active` while you need to review or re-review someone else's change.
- Use `waiting` while your own change is with reviewers, or after you have submitted feedback and are waiting for the author.
- Record the review phase in `state`; keep `description` focused on the stable stream goal.

Issue `url` values must look like `https://github.com/<owner>/<repo>/issues/<number>`. PR `url` values must look like `https://github.com/<owner>/<repo>/pull/<number>`. Do not use shorthand such as `repo#123` in frontmatter.

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
- description: an Obsidian link to the scratch note, italicized `state`, then one issue or PR link per line

`streams cmux open <slug>` creates a new CMUX workspace using the stream `cwd` and the latest recorded session command when present.

`streams cmux update ...` updates title, description, and color from scratch. `streams cmux update --all` updates every CMUX workspace with a matching stream and prints a `Warning:` for each unmatched workspace. It skips workspace index `0` only when its title is `Tasks`. Neither command reliably repairs an already-open terminal's working directory. If a workspace was opened with the wrong cwd, close and reopen it after fixing scratch `cwd`.

Example description:

```markdown
[🗒︎](obsidian://open?vault=worknotes&file=scratch%2Fexample-stream.md) _Issue #124 literal conversion helpers_
[ℹ︎#124](https://github.com/example-org/widget-lib/issues/124): Literal conversion helpers
[⑂︎#150](https://github.com/example-org/widget-lib/pull/150): Add literal helpers
```
