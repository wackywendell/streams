---
name: taskwarrior
description: "Use for Taskwarrior CLI/task database operations: inspecting tasks, UUID-safe mutations, reports, filters, annotations, wait/due/priority/urgency behavior, UDAs, exports, and Taskwarrior command mechanics."
metadata:
  short-description: Taskwarrior CLI mechanics
---

# Taskwarrior

Use this skill for pure Taskwarrior mechanics. AI-assisted coding stream state is tracked separately in scratch files through the sibling [streams skill](../streams/SKILL.md).

## Identity and Safety

- Taskwarrior numeric ids are report-local conveniences and can change.
- Use numeric ids only for immediate commands after listing current tasks.
- Use task UUIDs for durable references, scripts, annotations, and exact mutations.
- Use `rc.gc=off` for read-only inspection so garbage collection does not mutate state during status checks.
- Prefer `export` for machine-readable inspection.

Useful inspection commands:

```bash
task rc.gc=off <uuid> information
task rc.gc=off <uuid> export
task rc.gc=off status:pending export
task rc.gc=off +WAITING export
```

Taskwarrior reports can hide tasks with `wait:`. Include `+WAITING` when waiting tasks matter.

## Mutations

Use UUIDs for durable mutations:

```bash
task <uuid> modify <field:value> <tag-changes> "<description>"
task <uuid> annotate "<evidence>"
task <uuid> done
```

For tag changes, remove mutually exclusive tags explicitly when needed:

```bash
task <uuid> modify -oldtag +newtag "<new description>"
```

Annotations are append-only evidence. Keep them compact, durable, and useful for future inspection.

## Fields, Wait, Due, Priority, and Urgency

- `description`: the task's human-readable description. Domain-specific workflows may impose stricter meanings.
- `project`: hierarchy-like grouping.
- `priority`: Taskwarrior priority (`H`, `M`, `L`) when configured.
- `due`: a real deadline. Due dates affect urgency strongly.
- `wait`: hides the task from ordinary pending reports until the wait date/time; use `+WAITING` to inspect waiting tasks.
- `urgency`: computed by Taskwarrior. Do not set it directly; adjust fields/tags/dates that feed it.
- UDAs: user-defined attributes. Inspect config and task exports to understand available UDAs before writing them.

## Reports and Filters

Examples:

```bash
task rc.gc=off status:pending columns:id,uuid,project,priority,urgency,due,wait,tags,description
task rc.gc=off +WAITING columns:id,uuid,project,priority,urgency,due,wait,tags,description
task rc.gc=off <filter> export
```

Quote complex filters so the shell does not reinterpret them:

```bash
task rc.gc=off status:pending '( +tag or project:foo )' columns:id,uuid,tags,description
```

Use `task help <command>` or `task <command> rc.verbose=nothing` as needed for exact syntax in unfamiliar Taskwarrior versions.

## JSON Export Notes

`task ... export` emits JSON suitable for scripts. Prefer export when parsing task state instead of scraping column reports. Fields absent from a task may be missing or null depending on Taskwarrior version and UDA configuration.

## Relationship to Streams

Taskwarrior is not the source of truth for AI coding stream lifecycle state. Use this skill only for ordinary Taskwarrior mechanics, and load the streams skill when operating on scratch-first coding streams.
