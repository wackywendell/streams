---
name: workstreams
description: Manage AI-assisted coding workstreams in Taskwarrior across repos, jj workspaces, bookmarks, PRs, and AI sessions.
---

# Workstreams

Use Taskwarrior as the source of truth for coding workstream state.

Read the shared policy first:

- Live path: `~/.config/ai-workstreams/SPEC.md`
- Dotfiles source path: `home/.config/ai-workstreams/SPEC.md`

Follow the same core workflow as Codex:

- do not use Beads or `bd` for workstream state; older instructions that mention them are stale;
- one Taskwarrior task per real workstream;
- Taskwarrior numeric ids can change; use `uuid` for durable task identity;
- `description` is the next transition that would advance the stream;
- annotations are evidence and history;
- `workspace` is the local jj workspace root, while `bookmark` is the code line;
- tags include exactly one workflow-state tag: `doing`, `ready`, `review`, `blocked`, or `parked`;
- state changes should be evidence-based and conservative.

Reviewing someone else's PR, investigating a failure, and cleanup can also be workstreams. Use `ready` when the user has the next move, including reviewing a PR, asking Slack for review, fixing known review/CI feedback, or final cleanup. Use `review` when the stream is waiting on review, author updates, Slack response, CI, mergeability, or a timed check. Do not invent a `waiting` tag. Use `Cleanup` as a status-update label, not a workflow-state tag. Parked tasks do not need an open CMUX workspace.

Overall status updates should have explicit `Active / Ready`, `Review / Waiting`, `Blocked`, and `Parked` sections. For general status updates or "what should I work on" scans, rely on Taskwarrior state and annotations only. Do not check GitHub, PR checks, CI, local git/jj status, or code state unless the user asks for fresh verification, asks about one specific task, or the Taskwarrior record is too ambiguous to answer. Individual task updates should start with a bold status label such as `**Ready**`, `**Blocked**`, `**Review**`, `**Parked**`, or `**Cleanup**`, include the full PR URL when present, and summarize what the task is, what is done, what remains, what is waiting, and the next transition.

Distinguish self-review from external review: draft PR self-review is the user's move (`ready` / `**Self-review**`), while `review` / `**Review**` means waiting on another person, Slack response, CI, mergeability, or a timed check.

When splitting off a new task, create a new Taskwarrior task, new CMUX workspace, and new AI session in the appropriate directory. Do not copy the old `aisession`; leave it unset until the new session records its own id. Put enough context in the new task for a fresh AI session to start from scratch.

When a task has an associated CMUX workspace, update the CMUX title, description, color, and order alongside Taskwarrior state. Keep the title as the stable short slug. Format the CMUX description as `<Status>: <under-8-word state> <full-pr-url-if-present> task:<uuid>`.

Use CMUX colors to show actionability: `Green` for `doing`, `ready`, and `Cleanup`; `Amber` for `review` and timed waits; `Orange` for `blocked`; `Red` for a temporarily open `parked` workspace. Parked tasks generally should not have an open CMUX workspace. When multiple CMUX actions are needed, batch them into one readable shell command block, using `&&` when later updates should stop after an earlier failure.

Call `task` directly. Use `rc.gc=off` for read-only inspection, and use raw commands from the shared spec for capture, handoff, check, and split.
