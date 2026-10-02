# Changelog

## Unreleased

- Pi `stream_read` tool: the main agent can read the bound stream's note, another stream or the pending draft without a path-access prompt ([#3](https://github.com/wackywendell/streams/issues/3))
- Pi `checkpoint` tool: the main agent can start a checkpoint in the background, at natural points such as a significant decision. It never opens a chooser or takes focus, and only the user can apply. Set `agentTools: false` in `checkpoint.json` to turn it off ([#1](https://github.com/wackywendell/streams/issues/1))
- Checkpoint receipts return the side discussion to the main agent: user messages, checkpoint-agent replies and line comments, capped at 16,000 characters. Clear also reports the discussion if the user took part ([#2](https://github.com/wackywendell/streams/issues/2))

## 0.1.0 - 2026-10-01

First standalone release, extracted from a personal dotfiles repository with its development history.

- `streams` CLI, installable as `streams-cli`
- `streams` agent skill
- Pi `/checkpoint` extension and bundled skill, installable as `pi-streams`
- Notes default to `~/streams`; set `notes.root` to use another directory
