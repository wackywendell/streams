# Stream CMUX Integration

This reference defines how AI coding streams use CMUX. For pure CMUX command mechanics, use the sibling [cmux skill](../../cmux/SKILL.md).

CMUX is a projection of generic scratch state. It is not canonical state.

## Display Policy

Use scratch fields this way:

```text
title       = frontmatter title
description = italicized frontmatter state, then issue/PR links
cwd         = frontmatter cwd
color       = derived from frontmatter status
```

Do not add CMUX-specific fields to scratch frontmatter.

Description format:

```markdown
_Issue #124 literal conversion helpers_
[widget-lib#124](https://github.com/example-org/widget-lib/issues/124)
```

Include every `issues` and `prs` entry when present, one per line, with issues before PRs. GitHub issue and PR URLs render with repo-only labels such as `widget-lib#124`; non-GitHub or non-URL entries remain plain text.

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
2. Current or explicit CMUX workspace title as a weak stream-reference hint.
3. Unique match between CMUX `current_directory` and stream `cwd`.

User-facing stream references are resolved by normalizing to slug form, checking for an exact current stream slug, then accepting a unique substring match. If there is no match or more than one match, fail clearly and ask for a more explicit slug.

CMUX does not expose a reliable marker that distinguishes an explicit workspace title from a default command-derived title. Treat the title as a hint, not proof of identity. If no stream can be inferred from title or cwd, fail clearly and ask the user to pass a slug.

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
