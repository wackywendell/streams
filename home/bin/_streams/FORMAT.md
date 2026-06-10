# Streams Scratch Format

Scratch files live in `~/worknotes/scratch`. One top-level markdown file is one current stream. Archived streams live under `~/worknotes/scratch/archive`.

## Frontmatter

Use flat YAML frontmatter:

```yaml
title: Example Stream
status: active
created: 2026-06-08
updated: 2026-06-10
description: Overall task description
state: Current state in 5-12 words
repo: /path/or/repo-name
cwd: /path/to/workspace
vcs: jj
issues:
  - https://github.com/example-org/widget-lib/issues/124
prs:
  - https://github.com/example-org/widget-lib/pull/150
sessions:
  - codex:123-abc
```

Required fields:

- `title`: stable human-readable stream name.
- `status`: one of `active`, `waiting`, `blocked`, `parked`, `review`, or `done`.
- `created`: creation date.
- `updated`: last meaningful stream-state update date.
- `description`: longer overall task description.
- `state`: short current display state, roughly 5-12 words.
- `repo`: repo path or repo identity.
- `cwd`: local directory where work should resume.
- `vcs`: local VCS, usually `jj` or `git`.
- `issues`: list of GitHub issue URLs.
- `prs`: list of GitHub pull request URLs.
- `sessions`: AI session ids in `codex:<id>`, `claude:<id-or-slug>`, or `pi:<id>` form.

Issue URLs must look like `https://github.com/<owner>/<repo>/issues/<number>`. PR URLs must look like `https://github.com/<owner>/<repo>/pull/<number>`.

Do not include `slug`, `cmux`, `taskwarrior`, or `current` in frontmatter. The slug is the filename stem.

`streams resume [slug]` starts the last session listed in `sessions`. Without a slug, it infers the stream from the current CMUX workspace title or a unique workspace `cwd` match. If `sessions` is empty, start a new AI session intentionally and record it in scratch.

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

## Open Questions

_Unresolved questions that affect how to continue._
```

The body is a current pickup document, not an append-only log. Edit it in place so a new human or AI session can resume from the frontmatter and `## Status`.

## CLI Notes

`streams start "Title"` creates a scratch file only. `streams start "Title" --current` or `streams s "Title" -c` creates the scratch file and projects it onto the current CMUX workspace.

`streams cmux` can be abbreviated as `streams c`. `open` and `update` can be abbreviated as `o` and `u`. Use `-c` for `--current` and `-w` for `--workspace`.

## CMUX Projection

CMUX is a projection of generic stream state:

- title: `title`
- cwd: `cwd`
- color: derived from `status`
- description: italicized `state`, then one issue or PR link per line

Example description:

```markdown
_Issue #124 literal conversion helpers_
[widget-lib#124](https://github.com/example-org/widget-lib/issues/124)
[widget-lib#150](https://github.com/example-org/widget-lib/pull/150)
```
