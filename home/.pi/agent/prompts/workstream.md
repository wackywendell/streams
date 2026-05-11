---
description: Route Taskwarrior-backed coding workstream requests
argument-hint: <status|check|refresh|reconcile|update|checkpoint|handoff|cleanup|split off|open|switch|next action>
---

# Workstream

Task: $ARGUMENTS

Use the `workstreams` skill and follow `~/.config/ai-workstreams/SPEC.md` as the source of truth.

This single prompt handles conductor and single-workstream requests, including status, check/refresh, reconcile, update/checkpoint/handoff, cleanup, split-off, open/switch/resume, and ad hoc next actions. Route by the scope and vocabulary in the spec; do not duplicate or override that policy here. In particular, cleanup should prepare any CMUX-closure handoff but not close the active CMUX conversation unless the user explicitly confirms it.
