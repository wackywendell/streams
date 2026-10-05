# Changelog

## Unreleased

- `streams read` prints a note exactly as stored, even when it's invalid. `--hash` adds the SHA-256 for `update --if-match`, and `--json` gives parsed frontmatter, contents, hash and findings. `streams update --if-match` can now replace an invalid note
- **Breaking:** `streams list --json` returns `{"streams": [...], "findings": [...]}` instead of a bare array. An invalid note no longer breaks `list` and every lookup: it's skipped and reported, and `list` exits 1
- With `--json`, failing commands print `{"error": {"kind", "message", "candidates"}}`, so callers can tell a missing stream from an ambiguous ref or a broken note
- `streams update` sets `updated` to today when it changes a note, and `--json` reports the SHA-256 of the note as written
- Malformed refs such as `streams show '!!!'` print a one-line error instead of a Python traceback
- Pi `stream_read` tool: the main agent can read the bound stream's note, another stream or the pending draft without a path-access prompt ([#3](https://github.com/wackywendell/streams/issues/3))
- Pi reads notes only through `streams read`, and Apply checks the note against the hash the CLI reports. Receipts, line comments and `stream_read` no longer show note paths, and `stream_read` shows an invalid note with its problems
- Pi now shows the CLI's warnings from `streams update`, which it had been dropping
- Pi `checkpoint` tool: the main agent can start a checkpoint of the bound stream in the background, at natural points such as a significant decision. Its guidance says to usually omit instructions, since the checkpoint agent already sees the conversation. It never opens a chooser or takes focus, and only the user can apply. Set `agentTools: false` in `checkpoint.json` to turn it off ([#1](https://github.com/wackywendell/streams/issues/1))
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
