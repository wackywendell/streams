# Workstream Taskwarrior Integration

This reference defines the Taskwarrior reports and mutations specific to AI workstreams. For generic Taskwarrior CLI mechanics such as UUID-safe inspection, annotations, waits, due dates, priority, and export parsing, use the sibling [taskwarrior skill](../../taskwarrior/SKILL.md).

Use `task` directly for workstream records. Use `rc.gc=off` for read-only inspection, and use task UUIDs for durable mutations.

## Workstream Inspection

Taskwarrior reports can hide tasks with `wait:`. Always include `+WAITING export` when checking review, blocked, or parked workstreams that may be waiting.

Inspect any workstream task with missing `project`, missing `repo`, missing active-workstream `title`/`slug`/`scratch`, or not exactly one of the five workflow-state tags.

```bash
task rc.gc=off +WAITING export
task rc.gc=off status:pending '( repo.not: or title.not: or slug.not: or scratch.not: or workspace.not: or bookmark.not: or pr.not: or aisession.not: or +doing or +ready or +review or +blocked or +parked )' \
  columns:id,uuid,title,slug,scratch,project,repo,workspace,bookmark,pr,priority,urgency,due,wait,tags,description
```

## Workstream Mutations

```bash
task <uuid> modify <field-or-tag-changes> "<next transition>"
task <uuid> annotate "Current handoff: <compact resume summary>"
```

Use `description` for the next concrete transition, remove all other workflow-state tags when setting a new one, and preserve useful evidence in annotations before or while changing state.

## Create Example

```bash
task add project:widget-lib title:"Widget Lib Bump" slug:widget-lib-bump scratch:scratch/widget-lib-bump.md repo:example-org/monorepo workspace:~/go/src/github.com/example-org/monorepo/workspaces/widget-java bookmark:<username>/bump-widget-lib-0.3.2 pr:12345 +review wait:tomorrow "Check CI on PR #12345 and merge if approved"
task <id> annotate "PR: https://github.com/example-org/monorepo/pull/12345"
task <id> annotate "Validation: make test passed"
task <id> annotate "Current handoff: PR #12345 is waiting on CI; validation passed locally; next check CI and merge if approved."
task rc.gc=off <id> uuids
```

## Checkpoint / Update / Handoff Example

```bash
task <id> modify -doing -ready -review -blocked -parked +blocked wait:2026-05-02 "Re-check upstream PR and unblock the release bump"
task <id> annotate "Blocker: upstream release PR has not landed; no local action until it lands"
```

## Validate Title/Slug and CMUX Mapping

```bash
workstream-slug doctor --cmux
```
