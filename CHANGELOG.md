# Changelog

## Unreleased

- Moving a Pi pane into another Herdr workspace no longer moves its Session or creates an "Untitled effort" Stream. Layout never changes membership in any frontend, and no command moves a Session to another Stream yet. An hc space associated with another Stream now produces a warning, as a Herdr workspace does
- The Pi child uses the frontend selection to decide whether it runs Herdr cards, so a Herdr pane with `frontend.mode` set to `none` or `cmux` stays inactive
- The Pi child reports an error instead of following a symlink or reading a non-regular file at a watched record path, as Herdr cards already did
- `/checkpoint` keeps the chooser's open or minimized state when drafting begins. Explicitly minimizing either stage keeps it closed after recommendations or a draft finish
- Animated busy indicators now appear in the stream chooser and checkpoint card, including when the draft panel is minimized. Card frames do not add session entries; off-screen frames pause in regular terminal mode to avoid repeated scrollback redraws

- Pi agent prompting is configurable: stream selection/creation defaults to `manual`, while checkpoint prompting defaults to `low`, with `manual`, `medium` and `high` available. The tools remain available; these settings guide initiation rather than enforce permissions
- `/checkpoint:settings` and Alt+S in the chooser or draft panel open the same settings editor. Save preserves other config fields and updates the next agent turn without reload; Cancel leaves the file unchanged and returns to the same panel

- Streams can be created outside jj or Git, including through Pi's chooser. These notes record `repo: null`, `vcs: null` and the working directory as `cwd`; `streams check` accepts them

- `streams read` prints a note exactly as stored, even when it's invalid. `--hash` adds the SHA-256 for `update --if-match`, and `--json` gives parsed frontmatter, contents, hash and findings. `streams update --if-match` can now replace an invalid note
- **Breaking:** `streams list --json` returns `{"streams": [...], "findings": [...]}` instead of a bare array. An invalid note no longer breaks `list` and every lookup: it's skipped and reported, and `list` exits 1
- With `--json`, failing commands print `{"error": {"kind", "message", "candidates"}}`, so callers can tell a missing stream from an ambiguous ref or a broken note
- `streams update` sets `updated` to today when it changes a note, and `--json` reports the SHA-256 of the note as written
- Malformed refs such as `streams show '!!!'` print a one-line error instead of a Python traceback
- Pi `stream_read` tool: the main agent can read the bound stream's note, another stream or the pending draft without a path-access prompt ([#3](https://github.com/wackywendell/streams/issues/3))
- Pi reads notes only through `streams read`, and Apply checks the note against the hash the CLI reports. Receipts, line comments and `stream_read` no longer show note paths, and `stream_read` shows an invalid note with its problems
- Pi now shows the CLI's warnings from `streams update`, which it had been dropping
- Pi `checkpoint` tool: the main agent can start a checkpoint of the bound stream in the background, according to the configured prompting policy. Its guidance says to usually omit instructions, since the checkpoint agent already sees the conversation, including any text the main agent wrote just before calling it. It never opens a chooser or takes focus, and only the user can apply. Set `agentTools: false` in `checkpoint.json` to turn it off ([#1](https://github.com/wackywendell/streams/issues/1))
- Pi `select_stream` and `create_stream` tools: the main agent can recommend an existing stream by its exact slug, or propose a title for a new stream, and the user confirms the choice in the chooser. A receipt reports the choice or cancellation
- The stream chooser no longer blocks behind a loader: recommendations are found in the background, and the chooser opens when they're ready unless you're typing. Alt+P shows or hides it
- Checkpoint receipts return the side discussion to the main agent: user messages, checkpoint-agent replies and line comments, capped at 16,000 characters. Clear also reports the discussion if the user took part ([#2](https://github.com/wackywendell/streams/issues/2))
- Messages to the checkpoint agent start with a tag line, such as `[streams feedback 8f3c1a2b]`, which you can search for in session files. The chat recognizes messages by tag, so rewording the kickoff can no longer make it appear as yours
- The Apply receipt reads `Applied · CMUX …` rather than `Saved · CMUX …`, since "Saved for resume" means a checkpoint that wasn't applied

## 0.1.0 - 2026-10-01

First standalone release, extracted from a personal dotfiles repository with its development history.

- `streams` CLI, installable as `streams-cli`
- `streams` agent skill
- Pi `/checkpoint` extension and bundled skill, installable as `pi-streams`
- Notes default to `~/streams`; set `notes.root` to use another directory
