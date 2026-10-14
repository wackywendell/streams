# Streams Scratch Format

Scratch files live in `<notes.root>/scratch`, defaulting to `~/streams/scratch`. One top-level markdown file is one current stream. Archived streams live under `<notes.root>/scratch/archive`; `archived` is not a status. Paths below use the default notes root.

`streams format` is the source of truth for the stream schema and CLI behavior contract.

## Frontmatter

Use a top-level, tool-neutral YAML frontmatter mapping:

```yaml
title: Example Stream
status: active
created: 2026-06-08
updated: 2026-06-10
description: Overall task description
state: Current state in 5-12 words
repo: ~/work/widget-lib
cwd: ~/work/widget-lib/workspaces/example-stream
vcs: jj
issues:
  - url: https://github.com/example-org/widget-lib/issues/124
    description: Literal conversion helpers
prs:
  - url: https://github.com/example-org/widget-lib/pull/150
    description: Add literal helpers
sessions:
  - codex:123-abc
```

Quote any frontmatter string containing `#`, such as a PR or issue reference. In unquoted YAML, a whitespace-prefixed `#` starts a comment:

```yaml
state: "PR #186 design review complete; feedback ready."
```

Required fields:

- `title`: stable human-readable stream name. The filename stem is the slug; title should generally slugify to the filename.
- `status`: one of `active`, `waiting`, `blocked`, `parked`, or `done`. Status describes whether the stream is actionable, waiting, unable to proceed, intentionally inactive, or complete; it does not describe the type or phase of work.
- `created`: creation date, `YYYY-MM-DD`.
- `updated`: last meaningful stream-state update date, `YYYY-MM-DD`. `streams update` sets it to today whenever it changes the note. When editing the file directly, update it for handoffs, checkpoints, status changes, and durable pickup-context changes; do not churn it for typo-only edits.
- `description`: one-sentence overall task description. Longer than `state`, stable across handoffs, and focused on the stream goal.
- `state`: short current display state, roughly 5-12 words. This is used in compact displays, CMUX descriptions and Herdr metadata. Put transient workflow phases here, such as `PR in review; waiting for reviewer feedback.`
- `repo`: local root directory of the main/default checkout, or `null` when the stream has no repository. A path must not be an `owner/repo` identity or multi-repo description.
- `cwd`: directory where work should resume. Inside a repository, use the jj workspace or Git worktree root, not an arbitrary subdirectory. Without a repository, use the current working directory. AI sessions and frontend workspaces start here; this field cannot be null.

Inside a repository, `repo` and `cwd` have equal canonical paths for the main checkout and differ for a dedicated workspace or worktree. `streams start` discovers both roots. Outside jj or Git, it records `repo: null`, `vcs: null` and the current directory as `cwd`. Both null fields must be present; missing fields and inconsistent pairs are errors. `streams check` accepts intentional repository absence without a warning. Paths prefer a matching direct home-directory symlink such as `~/work`, then `~` for paths under home.
- `vcs`: `jj` or `git`, or `null` together with `repo: null`.
- `issues`: list of GitHub issue mappings. Each has a `url` and a manually maintained `description` of 2 to 5 words. Replace older URL-only entries with these mappings.
- `prs`: list of GitHub pull request mappings. Each has a `url` and a manually maintained `description` of 2 to 5 words. Replace older URL-only entries with these mappings.
- `sessions`: AI session ids in `codex:<id>`, `claude:<id-or-slug>`, or `pi:<id>` form, ordered oldest to newest. `streams resume` uses the last entry.

Status meanings:

- `active`: currently actionable implementation, investigation, or writing work.
- `waiting`: waiting on external feedback, time, or an event; no immediate local action is expected.
- `blocked`: cannot proceed until a dependency or blocker is resolved.
- `parked`: intentionally inactive; revisit only if the user or new evidence reactivates it.
- `done`: complete and ready to archive.

For review work, follow the next action rather than using a review-specific status:

- Use `active` while you need to review or re-review someone else's change.
- Use `waiting` while your own change is with reviewers, or after you have submitted feedback and are waiting for the author.
- Record the review phase in `state`; keep `description` focused on the stable stream goal.

Issue `url` values must look like `https://github.com/<owner>/<repo>/issues/<number>`. PR `url` values must look like `https://github.com/<owner>/<repo>/pull/<number>`. Do not use shorthand such as `repo#123` in frontmatter.

Do not include derived or projection-specific fields in frontmatter:

- `slug`: derived from the filename stem.
- `cmux` or `herdr`: native metadata and workspace associations stay outside notes.
- `current`: currentness is a view/query, not stream state.
- `taskwarrior`: legacy task database state is not part of scratch-first streams.

## Markdown Body

The main body is a concise resume brief and decision record, not a history of
the stream. Keep only:

- the current state and next action
- blockers or dependencies that still apply
- decisions that still govern the work done or to be done, with their reasons
- unresolved questions that affect how work continues

Most importantly: describe WHY decisions were made; the stream is intended to
capture the INTENT not necessarily evident in the product (code, PRs, artifacts,
etc.)

Remove external state that is no longer needed to resume - anything that can be
read from version control or the code itself should be removed.

When state changes, replace or remove stale context rather than adding a new
status entry. Generated artifacts such as drafts, proposals, and PR descriptions
can be placed under `~/streams/scratch/artifacts/<slug>/` and linked from an
optional `## Artifacts` section in the stream file.

Use this retention test: would removing this information prevent someone from
resuming the work or understanding why the current design was chosen? If not,
remove it.

Use this body shape:

```markdown
# <Title>

## Goal

_The outcome this stream is pursuing._

## Status

_Current local state, active blockers or dependencies, and the next concrete action. Do not include a chronology of how the stream reached this state._

## Decisions

_Choices that constrain the work, why they were chosen, and any important consequence. Remove superseded alternatives unless they explain why the chosen approach matters._

## Sessions

_Optional: one line per recorded AI session when multiple sessions need disambiguation._

## Open Questions

_Unresolved choices that affect the next work. Do not use this section as a general backlog or list of speculative ideas._

## Artifacts

_Optional links to retained stream-local outputs. Omit this section when there are none._

- Reply to issue explaining semantics: [[scratch/artifacts/example-stream/issue-124-reply.md]]
```

The body is a current pickup document, not an append-only log. Edit it in place so a new human or AI session can resume from the frontmatter and `## Status`.

Reference notes under `~/streams/reference` are optional durable outputs for reusable knowledge that outlives one stream. They are not stream state and should stay up to date rather than chronological. During check/refresh, handoff/checkpoint, and cleanup, consider whether new durable knowledge should update an existing reference note or justify a new one. Ask the human before creating or updating reference notes, and link any resulting notes from the scratch body.

The `sessions` frontmatter field is the ordered machine-readable resume list. Use optional `## Sessions` body notes for human-readable roles or labels when multiple sessions need disambiguation. Keep this concise, usually one line per session:

```markdown
## Sessions

- `codex:123-abc`: original planner/implementation session.
- `pi:456-def`: conductor handoff and cleanup session.
```

If there is only one other session besides the current one, and the current session did not start the stream, the other session is likely the original/initiator/planner session. Label it that way when the scratch context supports it.

## Configuration

Use `streams config` to see effective settings, the config path and which values come from defaults. Use `streams config --json` for machine-readable settings. Clients should fetch settings through the CLI, not read the file themselves.

```sh
streams config get notes.root
streams config set notes.root ~/notes
streams config set obsidian.vault notes
streams config set frontend.mode auto
```

| Key | Default | Purpose |
| --- | --- | --- |
| `notes.root` | `~/streams` | Absolute notes path; scratch, archive and artifact paths derive from it |
| `obsidian.vault` | `streams` | Vault name used for scratch-note links |
| `frontend.mode` | `auto` | Optional projection policy: `auto`, `cmux`, `herdr` or `none` |

Overrides live in `$XDG_CONFIG_HOME/streams/config.json`, or `~/.config/streams/config.json` when the environment variable is unset. `set` validates values and atomically replaces the file, preserving a managed symlink. Unknown keys, invalid values and malformed files are errors, not defaults.

Read legacy `cmux.enabled` only as migration input: stored `true` maps to `cmux`, `false` to `none`, and absence to `auto`. Canonical output contains only `frontend.mode`. Setting that key atomically removes the legacy section. Conflicting stored selectors fail; matching redundant selectors may be canonicalized by setting `frontend.mode`.

```json
{"notes":{"root":"/home/user/streams"},"obsidian":{"vault":"streams"},"frontend":{"mode":"auto"}}
```

Note operations validate their required notes-root settings independently of optional frontend policy. Invalid policy can therefore warn after a valid save. Malformed/unreadable JSON or invalid note-root settings still prevent locating notes safely.

Pi models and hotkeys stay in the extension's separate `checkpoint.json`. Plain `streams update` and selector-free start/rename do not project. Pi resolves optional projection after successful creation/binding or approved note save. Selection and native failures warn without undoing or repeating the write. Explicit named frontend commands ignore the automatic-projection preference.

### Frontend selection

`streams frontend get --json` loads configured policy and caller context without contacting native servers:

```json
{"configured":"auto","requested":null,"resolved":"herdr"}
```

A missing request or `-f auto` follows configuration. Only effective `auto` detects context: nonempty `CMUX_WORKSPACE_ID`, or `HERDR_ENV=1` plus nonempty `HERDR_PANE_ID`. A socket path alone is not context. Incomplete Herdr context or both recognized frontends is an error; select a named frontend to avoid ambiguity. Named requests override configuration. `none`, or `auto` with no context, resolves to `null`.

Optional refresh skips a null resolution. A mandatory current-workspace action fails before note changes. Named current actions still require that frontend's caller context; configuration never permits using another client's focus.

## CLI Notes

`streams list` shows the valid current streams. A note that fails validation is left out and reported on stderr instead of hiding every other stream, and the command then exits 1. `streams list --json` prints `{"streams": [...], "findings": [...]}`, where `findings` covers only the notes that were left out.

`streams read <stream-ref>` prints a note exactly as stored, even when it's invalid, so an agent can see and fix it. Only the exact slug reaches an invalid note; titles and partial matches cover valid streams only. Findings go to stderr, and the command exits 1 when the note has errors. `--hash` first prints the SHA-256 of the file's bytes on its own line: the value `update --if-match` checks. `--json` prints `{"slug", "frontmatter", "contents", "sha256", "findings"}` instead. `frontmatter` is the parsed YAML, or `null` when it doesn't parse, in which case `contents` holds the whole file.

Other commands skip invalid notes the same way. Naming one by its exact slug fails with an `invalid-stream` error that points to `streams read`.

`streams show <slug> --exact [--json]` accepts a canonical slug or an exact same-scratch rename alias. It does not normalize titles or fall back to substring matches. Broken, external, archived and cyclic alias targets fail. Use this contract for stored identities; ordinary `show` retains human-friendly reference matching.

With `--json`, a failing command prints `{"error": {"kind", "message", "candidates"}}` on stdout and exits 2. `kind` is one of `not-found`, `ambiguous` (with the matching slugs in `candidates`), `invalid-ref`, `invalid-stream` or `error`. Without `--json`, the message goes to stderr.

`streams start "Title"` creates a scratch file only. Generic start, rename and slug-free resume accept one selector:

| Selector | Current-workspace action |
| --- | --- |
| `-c`, `--current`, `-f auto`, `--frontend auto` | Follow configured frontend policy |
| `-f cmux` | Use the caller's CMUX workspace |
| `-f herdr` | Use the caller's Herdr workspace |

Named selectors need no additional `-c`. Bare `-f`, repeated or combined forms, and start dry-run with a selector are errors. Start/rename preflight selection and target before writes. A later projection failure retains the note and reports its actual slug/path and target, without retrying.

`streams update [-n|--dry-run] <stream-ref>` reads a complete Markdown document from stdin and validates it against the existing stream's filename and frontmatter schema. Dry-run leaves scratch unchanged; otherwise it replaces the existing file with the input, setting `updated` to today and adding it if missing. Only that line changes, and input identical to the file writes nothing. For a reviewed draft, use `--if-match <sha256>` with the SHA-256 of the original file's UTF-8 bytes to reject stale updates. An invalid note can be replaced, with a valid document, only when `--if-match` is given, so the caller has read what it's overwriting. It does not create a stream; use `streams start` for that. `--json` reports the target, whether a write was requested, the SHA-256 of the note as written (or as a dry-run would write it), and schema warnings.

`streams rename <stream-ref> "New Title"` renames the scratch file, title, leading heading, and artifact directory. It leaves relative symlinks at the old scratch and artifact paths for compatibility. Rename symlinks are not separate streams and are omitted from list and check operations. Use one current-workspace selector to update the selected frontend after renaming. Exact old-slug aliases resolve to the current canonical note.

`streams resume <stream-ref>` starts the last session listed in `sessions`, independently of a frontend, and rejects a selector. Without a reference, resume requires a current-workspace selector. CMUX uses an exact workspace title or unique dedicated `cwd`. Herdr uses its explicit workspace association first; only an unassociated workspace uses those hints. Main checkouts, repo-less directories and ambiguous hints do not identify a stream. If `sessions` is empty, start a new AI session intentionally and record it in scratch.

`cwd` must exist and be a directory for resume and either frontend's open command. Terminal resume changes into `cwd` before executing the latest recorded AI session. If the agent's saved session directory differs from scratch `cwd`, choose the scratch/current directory unless you intentionally want the old session directory.

`streams cmux` can be abbreviated as `streams c`. `open` and `update` can be abbreviated as `o` and `u`. Use `-c` for `--current` and `-w` for `--workspace`.

`streams archive <slug>` moves a `done` stream to `~/streams/scratch/archive/YYYY-MM/`. Use `streams a <slug>` for the short form, `--dry-run` to preview, `--month YYYY-MM` to choose a month, and `--force` only when intentionally archiving a non-`done` stream.

## CMUX Projection

CMUX is a projection of generic stream state:

- title: `title`
- cwd: `cwd`
- color: derived from `status`
- description: an Obsidian link to the scratch note, italicized `state`, then one issue or PR link per line

`streams cmux open <slug>` creates a new CMUX workspace using the stream `cwd` and the latest recorded session command when present.

`streams cmux update -c <slug>` updates the caller's title, description and color from scratch. Current lookup matches native ID/ref only, never a title or global GUI focus. Outside CMUX, pass `--workspace`. Explicit workspace selectors may match ID, ref or title; ambiguous matches fail.

## Herdr projection

`streams herdr update [slug] [-c | -w <workspace-id>]` updates an existing workspace without changing focus, layout or agent state. Current targeting uses the calling pane's live workspace, even after pane movement. Outside Herdr, pass an explicit workspace ID. With no slug, use the existing workspace association; only an unassociated workspace permits conservative label/cwd inference.

The association is a real JSON file containing `{"stream":"canonical-slug"}`, under `$XDG_STATE_HOME/streams/herdr/` or `~/.local/state/streams/herdr/`. Actual server socket/session and native workspace ID determine its namespace. The links pane and current-stream lookup share this exact identity. Rename aliases resolve to canonical current notes. A malformed association, invalid slug, unavailable note or unsafe alias fails without label/cwd fallback. Repair it with an explicit stream update.

Publish the validated association atomically before updating the workspace label and `stream_slug`, `stream_status`, `stream_state`, `stream_summary` metadata. Herdr's display-only `stream_status` uses `🟩` for active, `◷` for waiting, `!` for blocked, `Ⅱ` for parked and `✓` for done; scratch notes and CLI output keep the canonical status words. `stream_summary` joins the symbol and state with one space. Use `["$stream_summary"]` as a sidebar row to avoid Herdr's separator between separate tokens; the individual status and state fields remain available. Herdr still truncates sidebar rows and limits each metadata value to 80 characters; this does not enable wrapping. If publication fails, do not attempt native display changes. If a later native command fails, retain the new association and report the possibly stale display. Neither failure repeats a scratch write. Pi's session binding is independent; projection does not rename or switch its running agent.

`streams herdr open <slug>` is a macOS-only prototype requiring compatible Herdr 0.9.3 or newer. It creates a workspace at recorded cwd, resumes the latest supported recorded session or leaves a shell, and places a small links pane beneath the root terminal in its working tab. The links pane does not steal root focus. It is movable and tab-local, not a locked workspace-wide dock.

Explicit opening links a missing bundled `streams.links` plugin, reuses a matching enabled registration, and refuses conflicting or disabled registrations. Its launcher pins the opening interpreter, loaded import root, home and configuration directory. Partial startup errors list created resources and attempted agent names; inspect native state rather than blindly repeat opening.

The links pane refreshes its exact note reference. Press `n` to reread the association/note/vault and open Obsidian. Issue/PR links are blue and underlined, one per row. Keys `1`–`9` open up to nine visible HTTP(S) links; all HTTP(S) links also support terminal link activation. Hidden links are counted and have no active shortcuts; enlarge the pane to reveal them. A shortcut whose target changes at keypress refreshes the view and requires another press, rather than opening a different link. A failed note lookup leaves nothing stale available to open. Refresh never creates panes/workspaces, starts agents or changes focus. Checkout management, live-agent migration and restart automation are outside this prototype.
