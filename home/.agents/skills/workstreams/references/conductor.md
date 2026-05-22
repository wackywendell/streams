# Workstreams Conductor Policy

Use this reference for the pinned `tasks` CMUX workspace or a request that clearly concerns the whole portfolio of workstreams.

Read [core.md](core.md) first. Read [cmux-integration.md](cmux-integration.md) when inspecting or aligning CMUX.

## Scope

Conductor scope means the pinned `tasks` CMUX workspace, or a request that clearly concerns the whole portfolio of workstreams. The conductor normally trusts individual task records and does not inspect external PR/CI/Slack/jj state for every task.

The session and scope invariants in [core.md](core.md) apply. The conductor may run `reconcile` as an explicit rare override when the user wants a broad audit of external reality across tasks.

## Commands

`status` means “tell me the currently recorded state.” It is read-only.

In conductor scope: recap all relevant workstreams from Taskwarrior, including pending and `+WAITING`, and inspect CMUX only for workspace presence/open-or-missing display. Do not open, close, or modify CMUX workspaces during `status`. Do not inspect GitHub, Slack, CI, local git/jj status, or code state.

Conductor status must include every `doing`, `ready`, and `blocked` workstream even when it is not open in CMUX; mark those entries as `not open in CMUX` so missing working rooms are visible.

Use these grouping labels when they help readability, ordered by Taskwarrior urgency within groups:

- `Active / Ready`: `doing` or `ready`, including `Cleanup` items.
- `Review / Waiting`: `review`, including review, Slack, CI, mergeability, or timed waits.
- `Blocked`: `blocked`.
- `Parked`: `parked`.

`check`, `refresh`, and `update` are equal-status synonyms.

In conductor scope: inspect Taskwarrior and CMUX, make CMUX match Taskwarrior by creating missing workspaces for `doing` and `ready` tasks, updating/reordering existing workspaces, closing parked workspaces as appropriate, and recapping. Do not auto-open `blocked`, `review`, or `parked` tasks unless the user asks or a workspace is already open and should be updated. Do not check external PR/CI/Slack/jj state by default.

`reconcile` is conductor-scope only unless the user explicitly scopes it to one task. It is the rare broad audit: inspect Taskwarrior, scratch pads when needed, CMUX, local repo state when relevant, and external state across tasks; update records/UI; recap what changed and what remains uncertain.

## Recap Style

Conductor recaps should name the workstream title, urgency, priority/due/wait when relevant, state, PR link when present, next transition, the latest compact handoff/blocker/dependency/refresh evidence, and CMUX presence when known. For `doing`, `ready`, and `blocked` entries that are not open in CMUX, explicitly say `not open in CMUX`.

## Conductor Check / Refresh Workflow

1. Read pending and `+WAITING` Taskwarrior tasks.
2. Inspect open CMUX workspaces.
3. Align CMUX from Taskwarrior: create missing workspaces for `doing` and `ready` tasks, update/close/reorder existing workspaces, set descriptions/colors, and avoid opening `review`, `blocked`, or `parked` workspaces unless the user explicitly asks.
4. Run `workstream-slug doctor --cmux` when title/slug/CMUX mapping may have changed.
5. Recap recorded state and any CMUX changes. Do not check external systems unless this is a reconcile.

## Conductor Reconcile Workflow

Use only when the user asks for a broad audit or says the records may be stale.

1. Start with conductor check/refresh.
2. For tasks whose records depend on external state, inspect the relevant external source.
3. Update Taskwarrior, scratch, and CMUX when reality differs from recorded state.
4. Record `Refresh:` or `Current handoff:` evidence where the update affects future decisions.
5. Recap changes, current portfolio state, and remaining uncertainties.
