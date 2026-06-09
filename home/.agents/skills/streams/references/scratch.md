# Stream Scratch Files

Scratch files live in the Obsidian worknotes vault under `~/worknotes/scratch`. Use the local symlink `~/worknotes -> ~/Documents/obsidian/worknotes`; Obsidian owns the documents underneath it.

## Source Of Truth

Scratch frontmatter is canonical current state. The markdown body is a current pickup document, not an append-only log.

Use:

```bash
streams list
streams list --json
streams show <slug>
streams show <slug> --json
streams start "Title"
```

## Frontmatter

Use flat frontmatter so Obsidian can read it easily:

```yaml
title: Example Stream
status: active
created: 2026-06-08
updated: 2026-06-08
description: Overall task description
state: Current state of task
repo: /path/or/repo-name
cwd: /path/to/workspace
vcs: jj
issues: []
prs: []
sessions:
  - codex:123-abc
```

Do not add `cmux`, `taskwarrior`, or other tool-specific namespaces. Tooling consumes the generic fields.

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

The body should be edited in place so it remains coherent. Keep history only when it explains current choices or useful deferred options.

## Update Protocol

On session start or resume, read the entire scratch file before implementation work.

Before replying after material work, ask:

- Did frontmatter state change?
- Does `## Status` still let a new human or AI session pick up the work?
- Did a decision, tradeoff, deferred choice, or open question change?
- Does linked CMUX display need to be updated from the generic fields?

If yes, update the scratch file and CMUX projection before responding. If no durable state or pickup context changed, leave the file alone.
