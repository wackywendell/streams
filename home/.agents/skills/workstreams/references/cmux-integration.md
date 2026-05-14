# Workstream CMUX Integration

This reference defines how AI workstreams use CMUX. For pure CMUX command mechanics, use the sibling [cmux skill](../../cmux/SKILL.md).

## Display Grammar

CMUX workspace titles and descriptions use a small display grammar:

```text
title       = <task title>
description = [detail "\n"] meta ["\n" pr-link]
detail      = "_" <short phrase that adds information beyond the title> "_"
meta        = "**" (<priority> ": " <urgency> | "urg: " <urgency>) "**" [ " due " <short-date> ] [ " → " <short-date/time> ]
pr-link     = "[" <repo-basename> "#" <pr-number> "](" <full-pr-url> ")"
```

Use Taskwarrior `title` for the CMUX title. Omit `detail` when the title is enough; when present, it should build on the title with the next useful detail. CMUX color carries actionability. Keep the full Taskwarrior uuid and CMUX workspace ref in Taskwarrior annotations. In the steady state, do not put `task:<uuid-prefix>` in visible CMUX descriptions; scripts resolve CMUX workspace title by slugifying it and requiring exactly one pending task with that `slug`. During migration, scripts may still accept a legacy visible `task:<uuid-prefix>` as the authoritative link. CMUX workspace refs such as `workspace:9` are live UI handles, not durable identity.

CMUX renders workspace descriptions with inline-only Markdown while preserving whitespace. Use inline formatting only: emphasis for `detail`, strong emphasis for the priority/urgency part of `meta`, and normal Markdown links for `pr-link`. Do not rely on block Markdown in the workspace description; if a richer block is needed, use CMUX sidebar metadata blocks instead.

## Workspace Updates

When updating a task that has an associated CMUX workspace, update that workspace's description, color, and order at the same time as the Taskwarrior state.

Selecting a CMUX workspace is user-visible focus movement: it changes what the user is looking at and pulls their attention. Use `cmux select-workspace` sparingly, if at all. Prefer commands that target an explicit `--workspace` ref for inspection, updates, ordering, and notifications. Select a workspace only for explicit `open`/`switch`/`resume`, when the user asks to look at it, or when a CMUX command truly requires selection.

CMUX colors encode actionability: `Green` for `doing`, `ready`, and `Cleanup`; `Amber` for `review` and timed waits; `Orange` for `blocked`; `Red` for a temporarily open `parked` workspace. Parked workstreams generally should not have an open CMUX workspace.

Order CMUX task workspaces by Taskwarrior urgency by default, after the pinned `tasks` conductor workspace. Keep the color based on actionability; urgency changes order and description metadata, not the state color.

For `meta`, use `<priority>: <urgency>` when priority is set, such as `H: 8.8`, `M: 6.7`, or `L: 4.8`. If priority is absent, use `urg: <score>`. Append `due <short-date>` only for real deadlines. Append `→ <short-date/time>` for `wait` re-checks.

For overdue tasks, keep the color based on actionability and add an overdue signal in metadata instead. Prefer `cmux set-status tw-overdue "overdue" --icon alert-triangle --color "#C0392B"` for an open overdue workspace, and clear it when the due date is cleared or no longer overdue. If a status pill is too noisy or unavailable, include `overdue <short-date>` in the description metadata.

When updating several CMUX properties or several workspaces, batch the CMUX actions into one readable shell command block to avoid slow AI/tool round trips. Prefer `&&` when later actions should stop after a failure; use `;` only when every action is safe to run independently.

## Conductor Workspace

Ensure the conductor exists, is named exactly `tasks`, is pinned, and is first:

```bash
cmux list-workspaces
cmux new-workspace --name tasks --description "Taskwarrior conductor" --cwd ~/.dotfiles
cmux workspace-action --workspace <tasks-workspace-ref> --action rename --title tasks
cmux workspace-action --workspace <tasks-workspace-ref> --action set-description --description "Taskwarrior conductor"
cmux workspace-action --workspace <tasks-workspace-ref> --action pin
cmux workspace-action --workspace <tasks-workspace-ref> --action move-top
```

## Matching a Task to a Workspace

Prefer this matching/resolution order:

1. Taskwarrior `CMUX:` annotations that name a live workspace ref.
2. Legacy visible `task:<uuid-prefix>` tokens in a workspace description.
3. Title-to-slug resolution: slugify the CMUX title and require exactly one pending task with that `slug`.
4. PR/repo/workspace/bookmark evidence when it is unique and unambiguous.
5. Current-workspace context only as a last resort.

Normalize matched workspaces with the display grammar:

```bash
cmux rename-workspace --workspace <workspace-ref> "<task-title>"
cmux workspace-action --workspace <workspace-ref> --action set-description --description $'<detail>\n**<priority>: <urgency>**\n[<repo>#<pr>](<url>)'
cmux workspace-action --workspace <workspace-ref> --action set-color --color <color-from-policy>
cmux reorder-workspace --workspace <workspace-ref> --after <other-workspace-ref>
task <uuid> annotate "CMUX: <workspace-ref> (<task-title>) task:<task-uuid>"
```

## Creating or Opening Workstream Workspaces

Create only after checking for an existing match. Use the local jj workspace root as `--cwd` when known. Ensure title/slug/scratch metadata before starting an active AI session. Do not copy a conductor/current-task `aisession` to a newly opened task.

```bash
cmux new-workspace --name "<task-title>" --description $'<detail>\n**<priority>: <urgency>**\n[<repo>#<pr>](<url>)' --cwd <repo-or-jj-workspace-root> --command "<agent-command>"
task <uuid> annotate "CMUX: <workspace-ref> (<task-title>) task:<task-uuid>"
```

Agent command examples:

```bash
codex resume <id>
claude --resume <id-or-slug>
pi --session <id>
```

If no exact session is known, start a fresh agent session with the task-bootstrap prompt from [task-agent.md](task-agent.md); do not guess a nearby session or stamp the opener's session id onto the task.

Use panes for different views of the same task, not different tasks. If a pane becomes a separate workstream, split the Taskwarrior task first through the workstreams workflow.
