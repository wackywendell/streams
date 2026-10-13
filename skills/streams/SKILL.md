---
name: streams
description: "Manage AI-assisted coding streams from scratch frontmatter. Use when the user asks for stream status, check/refresh, update/checkpoint/handoff, cleanup, split-off, open/switch/resume, CMUX or Herdr alignment, or durable scratch state."
metadata:
  short-description: Manage scratch-first coding streams
---

# Streams

Use this skill to manage AI-assisted coding streams from scratch files under `<notes.root>/scratch`. `notes.root` is the configured notes directory (`streams config get notes.root`, default `~/streams`).

Scratch frontmatter is the source of truth for current state. The markdown body is the pickup document: goal, current status, important decisions, tradeoffs, deferred choices, and open questions. CMUX, Herdr and status lists are projections of that scratch state.

## CLI First

Use the `streams` CLI for mechanics instead of reimplementing parsing or matching in conversation:

| Need                                  | Command                                               |
| ------------------------------------- | ----------------------------------------------------- |
| List current streams                  | `streams list` or `streams list --json`               |
| Inspect one stream                    | `streams show <slug>` or `streams show <slug> --json` |
| Read a note as stored, even invalid   | `streams read <slug>` or `streams read <slug> --json` |
| Validate stream files                 | `streams check [slug]`                                |
| Create a stream                       | `streams start "Title"` or `streams s "Title"`        |
| Rename a stream                       | `streams rename <stream-ref> "New Title"`              |
| Archive a done stream                 | `streams archive <slug>` or `streams a <slug>`        |
| Resume the latest recorded AI session | `streams resume <slug>` or `streams resume -c`       |
| Read the stream file format           | `streams format`                                      |
| Inspect frontend selection            | `streams frontend get --json`                        |
| Open/update a frontend projection     | `streams cmux ...` or `streams herdr ...`             |

Use `streams format` as the schema and CLI contract source of truth. If this skill and `streams format` disagree, follow `streams format` and update this skill. Use `streams check [slug]` after editing scratch frontmatter.

## Stream State

One top-level markdown file under `<notes.root>/scratch` is one current stream. Archived streams live under `<notes.root>/scratch/archive`; `archived` is not a status.

Keep scratch frontmatter tool-neutral. Do not store native frontend metadata or workspace associations in scratch. Quote any string containing `#`, such as a PR or issue reference: in unquoted YAML, a whitespace-prefixed `#` starts a comment. Record every issue and PR as a `url` plus a manually maintained 2-to-5-word `description`.

Use only the statuses defined by `streams format`. Keep `state` short enough for compact displays. Keep `sessions` ordered oldest to newest; `streams resume` uses the last entry.

Status describes actionability, not the kind or phase of work. Review streams are `active` while the user needs to review or re-review, and `waiting` while a change is with someone else. Put phases such as `PR in review` or `Changes requested; waiting for author updates` in `state`, while `description` remains the stable stream goal.

Set `repo` to the local root of the main/default checkout. Set `cwd` to the local root of the jj workspace or Git worktree where work should resume; use the same path as `repo` when working in the main checkout. Prefer a matching direct home-directory symlink such as `~/work`; otherwise write paths under the user's home with `~`, not the expanded home directory. Both frontend open commands and `streams resume` depend on `cwd`. Do not use repository identities, multi-repo descriptions, or arbitrary subdirectories in these fields. For work without a repository, set both `repo` and `vcs` to explicit `null` and keep the working directory in `cwd`. This is valid stream state; do not invent a repository or initialize version control just to create a stream.

## AI Session

The `sessions` field should be a list of active AI sessions. Add yours to the end if not there. Sessions should be in the form `<agent>:<session_id>`, e.g.:

```yaml
sessions:
  - codex:1234abc-345-98712
  - pi:1234def-3afb-56789
  - claude:abc1234-567-8910
```

- In Pi, call `get_session_id` and record `pi:<session_id>`.
- In Codex, use `printenv CODEX_THREAD_ID` and record `codex:<id>`.
- In Claude, record the Claude resume/session id as `claude:<id-or-slug>`.

If a stream has multiple sessions, keep a short `## Sessions` section in the scratch body explaining what each session is for. If there is only one other session besides yours and you did not start the stream, the other session is likely the original/initiator/planner session; label it that way when the scratch context supports it.

## Scratch Body

The scratch body is a current-state pickup document. It records what has been learned from investigation and from the user so a new session or human can understand the goal, current status, important decisions, tradeoffs, and remaining work. It is meant to be a decision record of the important, non-obvious decisions made, not a log of every change.

Edit it in place: remove or replace superseded status, outdated decisions, and stale assumptions. It should read like an ADR-style document organized by concept, not a chronological log. Keep `## Status` current; follow `streams format` for the canonical body shape.

Keep long stream-local drafts and outputs under `<notes.root>/scratch/artifacts/<slug>/` and link them from `## Artifacts`; follow `streams format` for retention and lifecycle rules.

## Reference Notes

If a stream produces durable knowledge beyond the immediate task, consider whether `<notes.root>/reference` should be updated. Reference notes are reusable knowledge, not current stream state and not chronological logs.

Before creating or updating reference notes during `check` / `refresh`, `checkpoint` / `handoff`, or `cleanup`, explain what reference changes you recommend and ask the human for their thoughts. Check existing notes first and prefer updating them with the new current understanding over creating duplicates.

Use existing reference categories when they fit:

- `architecture/` for system models, design contracts, and architecture investigations.
- `tools/` for tool and workflow behavior.
- `runbooks/` for repeatable operational procedures.
- `ideas/` for durable product, research, or strategy observations.

Link created or updated reference notes from the scratch body so future sessions can find them.

## Workflow Verbs

- `status`: read scratch state and recap what is recorded. Do not inspect external systems unless asked.
- `check` / `refresh`: inspect relevant local or external state, then update scratch and the selected frontend if recorded state is stale. Tell the user what has changed, then give them clear assessment
- `checkpoint` / `handoff` / stream-local `update`: update scratch and the selected frontend as below.
- `cleanup`: mark `status: done` or archive only with explicit evidence or a clear request. Use `streams archive <slug>` for done streams and `--force` only when intentionally archiving non-done work.
- `split off`: create a separate stream with `streams start "Title"` when related work has its own goal, repo/cwd, state, owner, or session context.
- `open` / `switch` / `resume`: change the active stream context; use `streams resume <slug>` for terminal resume and explicit frontend groups for workspace projection. Slug-free resume requires a current-workspace selector.

## Update / Handoff Protocol

In a main Pi session, follow the `stream_prompting_policy` system-prompt section for initiating stream selection, creation and checkpoints. It takes precedence over the proactive update triggers below. Keep the tools available, but do not bypass this policy by editing scratch directly or using equivalent CLI actions. Use `checkpoint` for a scratch update the user can review; only the user applies it. Do not select or create a stream merely to enable automatic checkpointing. User-invoked `/checkpoint` and the separate checkpoint conversation are not restricted by these initiation settings. `/checkpoint:settings` edits the policy; Alt+S opens the same view from the chooser or draft panel.

For other agents, or when that Pi policy is absent, use the update triggers below.

On stream start or resume, read the scratch file before implementation work.

Before replying after material work, ask:

- Did frontmatter state change since the last stream update?
- Are any frontmatter fields missing or out of date?
  - Is your own AI session in the `sessions` field?
- Does `## Status` still let a new human or AI session continue?
- Did an important decision, tradeoff, deferred choice, or open question change?
- Did the work produce durable reusable knowledge that may belong in `<notes.root>/reference`?
- Does the selected frontend display need to be updated from scratch?

If no durable pickup context significantly changed, leave scratch alone.

If context has changed:

- Update relevant frontmatter fields, including `updated` (`streams update` sets it for you), `status`, `description`, and `state` when they changed.
- Check that `sessions`, `repo`, `cwd`, `prs`, `issues`, and other schema fields cover the current state. Add or update the 2-to-5-word description for every tracked issue and PR.
- Edit the markdown body so `## Status` is coherent pickup context.
- If multiple sessions are recorded or their roles are ambiguous, add or refresh `## Sessions`.
- Run `streams check [slug]`.
- Refresh the selected frontend if needed, preserving a successful scratch write when selection or native refresh fails.

## Frontend selection

`frontend.mode` defaults to `auto`; it may also be `cmux`, `herdr` or `none`. Inspect policy through `streams frontend get --json`, not independent environment detection. A missing request or `auto` follows configured policy. Named requests and explicit frontend groups override it. Legacy `cmux.enabled` is read only for migration; setting `frontend.mode` removes it atomically.

Generic start, rename and slug-free resume accept one selector. `-c`, `--current`, `-f auto` and `--frontend auto` follow configuration. `-f cmux` and `-f herdr` choose that frontend's current-workspace action without another `-c`. Bare `-f`, repeated/combined selectors, and an explicit resume reference plus selector are errors. Start/rename without a selector remain note-only.

Current always means the caller's workspace, never global GUI focus. Explicit update commands outside a frontend require `--workspace`. Auto skips optional refresh when no frontend context exists; mandatory current actions fail before note writes. Ambiguous or incomplete context must not be guessed around. Do not forge caller identity to control another workspace.

Pi resolves optional refresh after successful new-stream creation/binding and checkpoint Apply. Choosing an existing stream changes the Pi binding without immediately changing Herdr. Selection and backend failures warn but never undo or repeat a successful note write. The user must approve live package activation or user-global plugin registration when an agent performs setup.

## CMUX

CMUX is only a projection of scratch state. Its description starts with a linked text-style spiral notepad (`🗒︎`) for the scratch note, then italicized `state`, followed by linked text-style information (`ℹ︎#<number>`) and branch (`⑂︎#<number>`) labels with their scratch descriptions. Use `streams cmux open <slug>` and `streams cmux update ...` instead of hand-editing workspace title, description, color, or cwd. `streams cmux update --all` updates every matched workspace, warns for unmatched workspaces, and skips only index `0` when it is titled `Tasks`. Use `streams start "Title" -f cmux` to create a stream and project it onto the caller's CMUX workspace.

Use `streams rename <stream-ref> "New Title" -f cmux` to rename a stream and update the caller's CMUX projection together. Use `-c` instead to follow configured policy. Rename preserves compatibility symlinks at the old scratch and artifact paths.

Selecting a CMUX workspace is user-visible focus movement. Select a workspace only for explicit `open`, `switch`, `resume`, inspection, or when CMUX requires selection. For pure CMUX mechanics, use the sibling `cmux` skill.

## Herdr

Use `streams herdr update <slug> -c` for the caller's workspace, or `-w <workspace-id>` for an explicit target. Refresh changes only the workspace association and display metadata, not focus, layout or agents. Current targeting follows the live caller pane after movement.

A real file under `$XDG_STATE_HOME/streams/herdr/` (default `~/.local/state/streams/herdr/`) stores the explicit workspace-to-stream association. The links pane and current-stream lookup share its exact slug/rename-alias identity. Only an unassociated workspace permits conservative label or unique dedicated-cwd inference. Broken associations and missing/archived notes fail; never fuzzy-match a stored identity to another stream. `streams show <slug> --exact --json` exposes exact lookup. Repair a broken association with an explicit stream update.

Updates publish the association before native label/status/state display. If that publication fails, no native display update runs. A later display failure retains the association; report the stale sidebar rather than rolling back or repeating the note write. Pi session bindings are separate, and no projection changes the running Pi session or agent.

`streams herdr open <slug>` creates a working tab at recorded cwd, resumes a recorded supported session or leaves a shell, and adds a small bottom links pane. Opening requires macOS and compatible Herdr 0.9.3 or newer. It may link a missing bundled plugin globally; it does not silently replace a conflicting source or enable a disabled plugin. Ask before setup changes or resuming an active recorded session.

The pane is movable and tab-local. It shows HTTP(S) issue/PR links; `n` rereads the current note and vault for Obsidian. It refuses stale note opening after lookup failure. No checkout management, live-agent migration, workspace-wide dock or restart automation is included. Refresh never opens a workspace or pane.

## Recaps

For a single stream, recap briefly:

- `Overall`: current purpose and state.
- `Just completed`: what changed now.
- `Still to do`: concrete next work, blockers, or verification.

For portfolio status, group streams by `active`, `waiting`, `blocked`, `parked`, and `done`. Use `streams list --json` rather than scraping human output; it returns `{"streams": [...], "findings": [...]}`, and `findings` names notes left out because they're invalid.
