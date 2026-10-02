# Changelog

## Unreleased

- `streams read` prints a note exactly as stored, even when it's invalid. `--hash` adds the SHA-256 for `update --if-match`, and `--json` gives parsed frontmatter, contents, hash and findings. `streams update --if-match` can now replace an invalid note
- **Breaking:** `streams list --json` returns `{"streams": [...], "findings": [...]}` instead of a bare array. An invalid note no longer breaks `list` and every lookup: it's skipped and reported, and `list` exits 1
- With `--json`, failing commands print `{"error": {"kind", "message", "candidates"}}`, so callers can tell a missing stream from an ambiguous ref or a broken note
- Malformed refs such as `streams show '!!!'` print a one-line error instead of a Python traceback
- Pi `stream_read` tool: the main agent can read the bound stream's note, another stream or the pending draft without a path-access prompt ([#3](https://github.com/wackywendell/streams/issues/3))
- Pi `checkpoint` tool: the main agent can start a checkpoint of the bound stream in the background, at natural points such as a significant decision. It never opens a chooser or takes focus, and only the user can apply. Set `agentTools: false` in `checkpoint.json` to turn it off ([#1](https://github.com/wackywendell/streams/issues/1))
- Pi `select_stream` tool: the main agent can recommend a stream, or a new stream title, and the user confirms the choice in the chooser. A receipt reports the choice or cancellation
- The stream chooser no longer blocks behind a loader: recommendations are found in the background, and the chooser opens when they're ready unless you're typing. Alt+P shows or hides it
- Checkpoint receipts return the side discussion to the main agent: user messages, checkpoint-agent replies and line comments, capped at 16,000 characters. Clear also reports the discussion if the user took part ([#2](https://github.com/wackywendell/streams/issues/2))

## 0.1.0 - 2026-10-01

First standalone release, extracted from a personal dotfiles repository with its development history.

- `streams` CLI, installable as `streams-cli`
- `streams` agent skill
- Pi `/checkpoint` extension and bundled skill, installable as `pi-streams`
- Notes default to `~/streams`; set `notes.root` to use another directory
