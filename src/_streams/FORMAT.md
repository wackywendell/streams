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
- `state`: short current display state, roughly 5-12 words. This is used in compact displays and CMUX descriptions. Put transient workflow phases here, such as `PR in review; waiting for reviewer feedback.`
- `repo`: local root directory of the main/default checkout, or `null` when the stream has no repository. A path must not be an `owner/repo` identity or multi-repo description.
- `cwd`: directory where work should resume. Inside a repository, use the jj workspace or Git worktree root, not an arbitrary subdirectory. Without a repository, use the current working directory. AI sessions and CMUX workspaces start here; this field cannot be null.

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
- `cmux`: CMUX state is a projection and can be stale or workspace-local.
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
streams config set cmux.enabled true
```

| Key | Default | Purpose |
| --- | --- | --- |
| `notes.root` | `~/streams` | Absolute notes path; scratch, archive and artifact paths derive from it |
| `obsidian.vault` | `streams` | Vault name used in CMUX note links |
| `cmux.enabled` | `false` | Enables explicit CMUX refresh by checkpoint clients |

Overrides live in `$XDG_CONFIG_HOME/streams/config.json`, or `~/.config/streams/config.json` when the environment variable is unset. `set` validates values and atomically replaces the file. Boolean values must be `true` or `false`. Unknown keys and malformed files are errors, not defaults.

These settings are shared workflow settings only. Pi models and hotkeys stay in the extension's separate `checkpoint.json`. `cmux.enabled` does not make every `streams update` or `start` call run CMUX; a checkpoint client follows a successful write with `streams cmux update -c <slug>` when enabled. Explicit CMUX commands and `--current` remain available regardless of this preference.

## CLI Notes

`streams list` shows the valid current streams. A note that fails validation is left out and reported on stderr instead of hiding every other stream, and the command then exits 1. `streams list --json` prints `{"streams": [...], "findings": [...]}`, where `findings` covers only the notes that were left out.

`streams read <stream-ref>` prints a note exactly as stored, even when it's invalid, so an agent can see and fix it. Only the exact slug reaches an invalid note; titles and partial matches cover valid streams only. Findings go to stderr, and the command exits 1 when the note has errors. `--hash` first prints the SHA-256 of the file's bytes on its own line: the value `update --if-match` checks. `--json` prints `{"slug", "frontmatter", "contents", "sha256", "findings"}` instead. `frontmatter` is the parsed YAML, or `null` when it doesn't parse, in which case `contents` holds the whole file.

Other commands skip invalid notes the same way. Naming one by its exact slug fails with an `invalid-stream` error that points to `streams read`.

With `--json`, a failing command prints `{"error": {"kind", "message", "candidates"}}` on stdout and exits 2. `kind` is one of `not-found`, `ambiguous` (with the matching slugs in `candidates`), `invalid-ref`, `invalid-stream` or `error`. Without `--json`, the message goes to stderr.

`streams start "Title"` creates a scratch file only. `streams start "Title" --current` or `streams s "Title" -c` creates the scratch file and projects it onto the current CMUX workspace.

`streams update [-n|--dry-run] <stream-ref>` reads a complete Markdown document from stdin and validates it against the existing stream's filename and frontmatter schema. Dry-run leaves scratch unchanged; otherwise it replaces the existing file with the input, setting `updated` to today and adding it if missing. Only that line changes, and input identical to the file writes nothing. For a reviewed draft, use `--if-match <sha256>` with the SHA-256 of the original file's UTF-8 bytes to reject stale updates. An invalid note can be replaced, with a valid document, only when `--if-match` is given, so the caller has read what it's overwriting. It does not create a stream; use `streams start` for that. `--json` reports the target, whether a write was requested, the SHA-256 of the note as written (or as a dry-run would write it), and schema warnings.

`streams rename <stream-ref> "New Title"` renames the scratch file, title, leading heading, and artifact directory. It leaves relative symlinks at the old scratch and artifact paths for compatibility. Rename symlinks are not separate streams and are omitted from list and check operations. Use `--current` or `-c` to update the current CMUX workspace after renaming.

`streams resume [slug]` starts the last session listed in `sessions`. Without a slug, it infers the stream from an exact current CMUX workspace title or from a unique dedicated `cwd` match. It does not infer a stream from the main checkout or a repo-less directory because multiple streams can share it. If `sessions` is empty, start a new AI session intentionally and record it in scratch.

`cwd` must exist and be a directory for `streams resume [slug]` and `streams cmux open <slug>`. `streams resume [slug]` changes into `cwd` before executing the latest recorded AI session. If the agent's saved session directory differs from scratch `cwd`, choose the scratch/current directory unless you intentionally want the old session directory.

`streams cmux` can be abbreviated as `streams c`. `open` and `update` can be abbreviated as `o` and `u`. Use `-c` for `--current` and `-w` for `--workspace`.

`streams archive <slug>` moves a `done` stream to `~/streams/scratch/archive/YYYY-MM/`. Use `streams a <slug>` for the short form, `--dry-run` to preview, `--month YYYY-MM` to choose a month, and `--force` only when intentionally archiving a non-`done` stream.

## CMUX Projection

CMUX is a projection of generic stream state:

- title: `title`
- cwd: `cwd`
- color: derived from `status`
- description: an Obsidian link to the scratch note, italicized `state`, then one issue or PR link per line

`streams cmux open <slug>` creates a new CMUX workspace using the stream `cwd` and the latest recorded session command when present.

`streams cmux update -c <slug>` updates the current title, description, and color from scratch.
