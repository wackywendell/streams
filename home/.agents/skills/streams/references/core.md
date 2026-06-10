# Streams Core Policy

Scratch files under `~/worknotes/scratch` are the source of truth for AI-assisted coding streams.

One scratch file represents one real stream. The filename stem is the stream slug. The frontmatter is structured current state. The markdown body is a cohesive pickup document for a future human or AI session.

## Model

Required frontmatter:

```yaml
title: Streams Scratch State Workflow
status: active
created: 2026-06-08
updated: 2026-06-08
description: Overall task description
state: Current state of the task
repo: /path/or/repo-name
cwd: /path/to/workspace
vcs: jj
issues: []
prs: []
sessions:
  - codex:123-abc
```

Field meanings:

- `title`: stable human-readable stream name; use it as the CMUX workspace title.
- `status`: one of `active`, `waiting`, `blocked`, `parked`, `review`, or `done`.
- `description`: longer overall task description.
- `state`: short current display state, roughly 5-12 words; use it in lists and CMUX descriptions.
- `repo`: repo path or repo identity.
- `cwd`: local directory where work should resume.
- `vcs`: local VCS, usually `jj` or `git`.
- `issues`: GitHub issue URLs.
- `prs`: GitHub pull request URLs.
- `sessions`: AI session ids in `codex:<id>`, `claude:<id-or-slug>`, or `pi:<id>` form.

Do not put `slug` in frontmatter. The slug is the scratch filename stem, and CMUX title lookup can derive candidate slugs from titles.

## Statuses

Use exactly these statuses:

- `active`: actively in the current human/AI work loop.
- `waiting`: waiting for a timed check, response, CI, review, or other expected external movement.
- `blocked`: cannot progress without an external dependency or decision.
- `parked`: intentionally paused; not expected to move soon.
- `review`: in explicit review/approval/merge-readiness flow.
- `done`: complete but not necessarily archived.

`archived` is not a status. A stream is archived when the scratch file is moved under `~/worknotes/scratch/archive`.

## Invariants

- The markdown body starts with enough context for a new human or AI session to continue.
- `## Status` is current pickup context, not a log.
- Frontmatter should remain tool-neutral. CMUX and other tools interpret generic fields rather than adding tool-specific fields.
- Slugs are lowercase kebab-case filename stems and should normally equal `slugify(title)`.
- `state` stays short; detailed reasoning belongs in markdown.
- `sessions` belongs to AI sessions actually working the stream. Do not copy a conductor or parent stream session into a new stream.
- If a stream splits into two real streams, create a second scratch file rather than overloading one.
- Be conservative about automatic state changes. State transitions should follow explicit evidence or user intent.

## Split-Off Signals

Split off a separate stream when related work no longer fits cleanly in the current file. Common signals:

- A follow-up idea has its own goal, owner, repo/cwd, or current state.
- The current `state` would need multiple independent actions.
- Implementation work and separate design/backlog work are accumulating in the same stream.
- A new worker would need a fresh AI session with different context.
- The active history cannot be summarized cleanly in `## Status`.

When splitting off, create a new scratch file with `streams start "Title"`, fill in frontmatter and pickup context, and continue the current stream unless the user explicitly says to switch.
