# streams

Streams keeps AI-assisted coding work in plain markdown scratch notes. Each note is one stream: a piece of work with its status, location, linked issues and PRs, recorded agent sessions and next steps. Agents and people read and update the same note, so work can stop and resume across sessions.

This repository contains:

- `streams`, a command-line tool for creating, checking, updating, resuming and archiving streams (Python package `streams-cli`)
- an agent skill that teaches coding agents the stream workflow (`skills/streams`)
- a Pi extension that adds `/checkpoint`, a side conversation that drafts a scratch-note update from the current session (Pi package `pi-streams`)

## Install

Install the CLI with [uv](https://docs.astral.sh/uv/):

```sh
uv tool install git+https://github.com/wackywendell/streams@v0.1.0
```

Install the Pi extension and skill:

```sh
pi install git:github.com/wackywendell/streams@v0.1.0
```

The Pi extension calls the `streams` CLI, so install both. To use the skill with another agent, copy or link `skills/streams` into that agent's skills directory.

## Use

```sh
streams start "Fix flaky login test"   # run inside a jj workspace or Git worktree
streams list
streams show fix-flaky-login-test
streams check
streams format                         # the scratch note format
```

Notes live in `~/streams/scratch` by default. To use another directory:

```sh
streams config set notes.root ~/notes
```

`streams resume` restarts the latest recorded Codex, Claude Code or Pi session for a stream. CMUX workspace integration is optional and off by default; see `streams format` for details.

In Pi, run `/checkpoint` to draft an update to the current stream. The agent can read stream notes with the `stream_read` tool, and can start a checkpoint for you to review with the `checkpoint` tool. See [docs/pi-stream-checkpoint-spec.md](docs/pi-stream-checkpoint-spec.md) for behaviour and configuration.

## Supported versions

Tested with Python 3.11 and 3.14, Node 26 and Pi 0.99, on macOS and Linux.

## Develop

```sh
uv sync
npm ci
mise run test
```

## License

MIT
