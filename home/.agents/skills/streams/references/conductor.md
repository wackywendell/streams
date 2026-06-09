# Streams Conductor Policy

Conductor scope means a broad status or reconciliation pass across multiple streams.

Use scratch files as canonical state. Do not inspect or mutate Taskwarrior.

## Commands

Use `streams list` for a readable status view:

```bash
streams list
```

Use JSON when another tool or script should consume state:

```bash
streams list --json
streams show <slug> --json
```

## Refresh / Reconcile

A conductor refresh should:

1. Read current scratch state with `streams list` or `streams list --json`.
2. Inspect CMUX only as a projection of scratch state when CMUX is relevant.
3. Update scratch files only when recorded state or pickup context is stale.
4. Update CMUX titles/descriptions/colors from generic stream fields when possible.
5. Recap active, waiting/review, blocked, parked, and done streams.

Do not drift into PR/CI/Slack/local-repo audits unless the user explicitly asks or a scratch file's current state makes that check the requested work.

## Recap Style

Keep conductor recaps concise:

- `Active`: streams currently moving.
- `Waiting / Review`: streams waiting on checks, reviews, responses, or merge readiness.
- `Blocked`: streams needing an external decision/dependency.
- `Parked`: intentionally paused.
- `Done`: completed streams that may still need manual archive.

Mention stale or missing frontmatter when it affects the status view.
