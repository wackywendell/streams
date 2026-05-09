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
- annotations are focused evidence, not transcripts; prefer prefixes such as `Current handoff:`, `Decision:`, `Validation:`, `Blocker:`, `Dependency:`, `Refresh:`, `CMUX:`, `Session:`, and `Error:`;
- `workspace` is the local jj workspace root, while `bookmark` is the code line;
- active workstreams should have `title` and `slug`: `title` is the human display label for CMUX/status, while `slug` is the stable lowercase/kebab-case machine handle derived from title by default; Taskwarrior `uuid` remains durable identity;
- active AI-assisted workstreams should have `scratch`, a vault-relative path under `~/Documents/obsidian/worknotes`, usually `scratch/<slug>.md`; scratch pads are engineering logs, not task state;
- CMUX title should be the workstream `title`; visible `task:<uuid-prefix>` links are legacy fallback only because automation should resolve slugify(CMUX title) to exactly one pending task `slug`;
- `aisession` uses `codex:<id>`, `claude:<id-or-slug>`, or `pi:<id>` when known;
- tags include exactly one workflow-state tag: `doing`, `ready`, `review`, `blocked`, or `parked`;
- use Taskwarrior `priority` to feed urgency: `H` for reviews, follow-ups, and anything blocking others; `M` for the user's own work PRs; `L` for personal/tooling/backlog work unless explicitly promoted;
- use `due` only for real deadlines, and use `wait` for reminders, snoozes, CI/review re-checks, and blocked follow-ups; default normal review waits to the next business day unless the task evidence calls for a same-day re-check or longer snooze;
- treat Taskwarrior `urgency` as "needs attention"; state tags explain what kind of attention is needed;
- state changes should be evidence-based and conservative.

Reviewing someone else's PR, investigating a failure, and cleanup can also be workstreams. Use `ready` when the user has the next move, including reviewing a PR, asking Slack for review, fixing known review/CI feedback, or final cleanup. Use `review` when the stream is waiting on review, author updates, Slack response, CI, mergeability, or a timed check. Do not invent a `waiting` tag. Use `Cleanup` as a status-update label, not a workflow-state tag. Parked tasks do not need an open CMUX workspace.

Overall status updates should order workstreams by Taskwarrior urgency by default. Urgency means "needs attention," not always "start coding now"; the state label explains whether the response is implementation, review, waiting, unblocking, cleanup, or metadata correction. Use explicit `Active / Ready`, `Review / Waiting`, `Blocked`, and `Parked` sections only when they make the status easier to scan, and do not let grouping hide high-urgency tasks. For general status updates or "what should I work on" scans, rely on Taskwarrior state and annotations only. Do not check GitHub, PR checks, CI, local git/jj status, or code state unless the user asks for fresh verification, asks about one specific task, or the Taskwarrior record is too ambiguous to answer. Use `task export` data plus `+WAITING export` rather than raw annotation-heavy reports. Individual task updates should start with a bold status label such as `**Ready**`, `**Blocked**`, `**Review**`, `**Parked**`, or `**Cleanup**`, include urgency plus priority/due/wait when relevant, include the full PR URL when present, and summarize what the task is, what is done, what remains, what is waiting, and the next transition.

If a blocked or parked task is high urgency because of an overdue `due`, treat that as an attention signal: clear or correct the deadline, unpark it, update `wait`, or otherwise fix the task record. Do not invent a parallel ranking system when Taskwarrior fields can be corrected.

Distinguish self-review from external review: draft PR self-review is the user's move (`ready` / `**Self-review**`), while `review` / `**Review**` means waiting on another person, Slack response, CI, mergeability, or a timed check.

When doing substantial AI-assisted work, read the task scratch pad before implementation. If it is missing, create `~/Documents/obsidian/worknotes/scratch/<slug>.md` directly using the shared spec template, set `scratch:scratch/<slug>.md`, and annotate `Scratch: <absolute path>`. Before replying, update `## Design`, `## Debugging`, `## Test plan`, or `## Learnings` with material evidence; do not duplicate Taskwarrior status or todos there.

When pausing active work, add a compact `Current handoff:` annotation with current local/external state, validation or exact error status, important decisions/constraints, and the exact next transition.

When splitting off a new task, create a new Taskwarrior task, new CMUX workspace, and new AI session in the appropriate directory. Split when the current task no longer has one coherent next transition: multiple independent actions, different repos/workspaces/PRs/owners, implementation mixed with backlog/design, a fresh AI session needing different context, or history that cannot be summarized by one `Current handoff:`. Do not copy the old `aisession`; leave it unset until the new session records its own prefixed id. Put enough context in the new task for a fresh AI session to start from scratch.

When a task has an associated CMUX workspace, update the CMUX title, description, color, status pills, and order alongside Taskwarrior state. Use the shared CMUX display grammar: `title = <task title>` and `description = [detail] + meta + optional PR markdown link`, where `meta` looks like `H: 8.8` or `M: 3.7 → Wed 09:00`. Keep the full uuid in Taskwarrior annotations; visible `task:<8-char-uuid-prefix>` links are legacy fallback only.

Use CMUX colors to show actionability: `Green` for `doing`, `ready`, and `Cleanup`; `Amber` for `review` and timed waits; `Orange` for `blocked`; `Red` for a temporarily open `parked` workspace. Do not add separate overdue colors; add a `tw-overdue` status pill or `Overdue: <date>` description metadata instead. Order CMUX task workspaces by Taskwarrior urgency after the pinned `tasks` conductor workspace. Parked tasks generally should not have an open CMUX workspace. When multiple CMUX actions are needed, batch them into one readable shell command block, using `&&` when later updates should stop after an earlier failure.

Call `task` directly. Use `rc.gc=off` for read-only inspection, and use raw commands from the shared spec for capture, handoff, check, and split.
