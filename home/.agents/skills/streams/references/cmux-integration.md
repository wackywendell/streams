# Stream CMUX Integration

This reference defines how AI coding streams use CMUX. For pure CMUX command mechanics, use the sibling [cmux skill](../../cmux/SKILL.md).

CMUX is a projection of generic scratch state. It is not canonical state.

## Display Policy

Use scratch fields this way:

```text
title       = frontmatter title
description = frontmatter state
cwd         = frontmatter cwd
color       = derived from frontmatter status
```

Do not add CMUX-specific fields to scratch frontmatter.

CMUX colors:

- `active`: Green
- `waiting`: Amber
- `review`: Amber
- `blocked`: Orange
- `parked`: Red when temporarily open
- `done`: Green until manually closed/archived

Selecting a CMUX workspace is user-visible focus movement. Prefer commands that target an explicit workspace ref for updates. Select a workspace only for explicit `open`/`switch`/`resume`, when the user asks to look at it, or when CMUX requires selection.

## Matching A Workspace To A Stream

Prefer this resolution order:

1. Explicit slug from the user.
2. Current CMUX workspace title slugified to `~/worknotes/scratch/<slug>.md`.
3. Explicit workspace ref/title, then title slugified to a scratch filename.

Fail clearly when the scratch file does not exist or when the workspace title cannot produce a valid slug.

## Opening / Updating

The intended command surface is:

```bash
streams cmux open <slug>
streams cmux update <slug> --current
streams cmux update <slug> --workspace workspace:3
streams cmux update --current
streams cmux update --workspace workspace:3
```

Implement this under the `streams` executable, with CMUX behavior separated into an internal module. CMUX commands should consume the same parsed stream model used by `streams list` and `streams show`.

Agent command examples:

```bash
codex --no-sandbox resume <id>
claude --resume <id-or-slug>
pi --session <id>
```

If no exact session is known, start a fresh agent session for the stream. Do not copy a conductor/current-session id onto a newly opened stream.
