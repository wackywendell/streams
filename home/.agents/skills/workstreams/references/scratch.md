# Workstream Scratch Pads

Scratch pads live in the Obsidian worknotes vault and are durable engineering logs. Taskwarrior remains canonical for status and next transitions; scratch pads record the overall goal, reasoning, investigations, verification, and reusable learnings.

Use a local-only symlink `~/worknotes -> ~/Documents/obsidian/worknotes`. This link is not stowed or tracked by dotfiles; Obsidian owns the documents underneath it.

Use this deterministic convention:

```text
worknotes root = ~/worknotes
scratch UDA    = task.scratch if set, else scratch/<task.slug>.md
path           = <worknotes root>/<scratch UDA>
```

Agents should create scratch pads directly rather than calling a separate helper command. When creating one:

1. Require a task `uuid`, `title`, and `slug`.
2. Create `~/worknotes/scratch/` if needed.
3. Write `scratch/<slug>.md` if it does not already exist.
4. Set `task <uuid> modify scratch:scratch/<slug>.md`.
5. Add one annotation: `Scratch: ~/worknotes/scratch/<slug>.md`.
6. Read the entire scratch pad before doing substantial work.

Use this template:

```markdown
# <Task title>

**Taskwarrior:** `<uuid>`
**Slug:** `<slug>`
**Created:** <YYYY-MM-DD>
**Repo:** `<repo>`
**Workspace:** `<workspace>`
**Bookmark:** `<bookmark>`
**PR:** `<pr>`
**AI session:** `<aisession>`
**Current transition at creation:** <description>

## Goal

_full task goal / outcome being pursued_

## Design

_Engineering design, architecture decisions, tradeoffs considered._

## Debugging

_Investigation steps, failed hypotheses, what turned out to matter._

## Test plan

_How to verify: commands, scenarios, expected outcomes._

## Learnings

_Novel, reusable knowledge worth remembering. Only add entries that would save time on a future unrelated task._
```

The full task goal belongs in the scratch pad `## Goal` field. Taskwarrior `description` remains the next transition only. Add a compact `Goal:` annotation when the goal should be visible in Taskwarrior status without opening the scratch pad.

## Mandatory Scratch Protocol for Active AI Sessions

- On session start or resume, read the entire scratch pad before implementation work.
- If creating or resuming substantial work, add a dated `## Design` entry with the understood task and planned approach.
- While working, consider whether `## Goal`, `## Design`, `## Debugging`, `## Test plan`, or `## Learnings` would be materially more useful if updated. The goal can change when the user clarifies intent or implementation learning changes the real outcome being pursued.
- Before replying, ask both:
  - Scratch: did anything since the last response usefully belong in `## Goal`, `## Design`, `## Debugging`, `## Test plan`, or `## Learnings`?
  - Taskwarrior: did the next transition, state tag, wait time, PR/session/scratch metadata, or handoff evidence change?
- This is a consideration gate, not a requirement to edit the scratch pad on every turn. If there is no durable, reusable, or resume-relevant evidence to record, leave the scratch pad unchanged.
- If either answer is yes, update the scratch pad and/or Taskwarrior before responding.
- Do not duplicate Taskwarrior state, todo lists, generic diff summaries, or large code blocks in the scratch pad.

`## Learnings` entries must pass this filter: not derivable from code/history, not a one-off task detail, and likely to save time on a future unrelated task.

<!-- TODO(workstreams): Add a workstream-scratch command only if direct agent-managed scratch creation proves error-prone, humans want a manual open/ensure flow, or archive/retro/remote lifecycle needs dedicated tooling. -->
