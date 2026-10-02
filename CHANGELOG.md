# Changelog

## Unreleased

- Pi `stream_read` tool: the main agent can read the bound stream's note, another stream or the pending draft without a path-access prompt ([#3](https://github.com/wackywendell/streams/issues/3))

## 0.1.0 - 2026-10-01

First standalone release, extracted from a personal dotfiles repository with its development history.

- `streams` CLI, installable as `streams-cli`
- `streams` agent skill
- Pi `/checkpoint` extension and bundled skill, installable as `pi-streams`
- Notes default to `~/streams`; set `notes.root` to use another directory
