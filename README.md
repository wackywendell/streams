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
streams start "Fix flaky login test"   # works inside or outside a repository
streams list
streams show fix-flaky-login-test
streams check
streams format                         # the scratch note format
```

Notes live in `~/streams/scratch` by default. To use another directory:

```sh
streams config set notes.root ~/notes
```

`streams resume <slug>` restarts the latest recorded Codex, Claude Code or Pi session for a stream. Use `streams resume -c` to resolve the caller's stream through the configured frontend.

### Workspace frontends

CMUX and Herdr are optional displays of scratch state. Automatic Pi refresh defaults to `auto`, which detects the calling terminal context:

```sh
streams config set frontend.mode auto   # auto, cmux, herdr or none
streams frontend get --json             # inspect selection without native calls
streams start "Fix flaky login test" -c # create and update the caller's workspace
streams rename old-slug "New title" -f herdr
streams cmux update fix-flaky-login-test -c
streams herdr update fix-flaky-login-test -c
streams herdr open fix-flaky-login-test
```

`-c`, `--current`, `-f auto` and `--frontend auto` follow configured policy. A named selector overrides it and needs no additional `-c`. Supply one selector; bare `-f` and combined selectors are errors. Start and rename without a selector remain note-only. Resume with an explicit slug rejects a selector.

Current targeting always means the caller's workspace, never another client's GUI focus. Outside a frontend, explicit update commands require `--workspace`. A stored legacy `cmux.enabled` setting remains migration input; setting `frontend.mode` removes it atomically.

After successful stream creation/binding or checkpoint Apply, Pi resolves selection once and refreshes the selected frontend. No detected frontend means skip. Selection or refresh failures leave the note and Pi binding intact. Choosing an existing stream in Pi changes its session binding but does not immediately project it into Herdr.

Herdr opening requires macOS and Herdr 0.9.3 or newer. It opens at recorded `cwd`, resumes the latest recorded supported session when present, and adds a small links pane beneath the working terminal. Otherwise it leaves a shell. Opening may register the missing bundled `streams.links` plugin; conflicting or disabled registrations require explicit intervention. Do not resume an active recorded session without approval.

The links pane reads the workspace's explicit stream association under `$XDG_STATE_HOME/streams/herdr/` (default `~/.local/state/streams/herdr/`). Updates publish that association before changing sidebar display. Current-stream lookup uses the same exact identity, including rename aliases; a broken association never falls back to another note. Display failures can leave the sidebar stale while the new stream's links remain usable. The `[n]` before the title opens the current note in Obsidian. Issue/PR links appear blue and underlined, one per row; `[1]`–`[9]` open up to nine visible HTTP(S) links, and all links support terminal link activation. Hidden links are counted and have no active shortcuts; enlarge the pane to reveal them. If a shortcut's target changes before the keypress is handled, the pane refreshes and asks you to press it again rather than opening a different link.

This Herdr prototype has no workspace-wide dock, checkout management, agent migration or restart automation. Refresh never opens panes or changes focus. Automated tests and a live shell-only Herdr 0.9.3 smoke test cover opening, refresh and links. Recorded-session resumption and browser/Obsidian activation still need separate manual acceptance. See `streams format` for the full contract.

In Pi, run `/checkpoint` to draft an update to the current stream. The agent can read stream notes with the `stream_read` tool, can ask you to choose the session's stream with the `select_stream` and `create_stream` tools, and can start a checkpoint for you to review with the `checkpoint` tool. By default, the agent opens stream selection or creation only when you ask, and initiates checkpoints at substantial task completion or a clear pause/handoff when pickup context changed. Run `/checkpoint:settings`, or press Alt+S in the chooser or draft panel, to change either policy. Saved prompting settings take effect on the next agent turn without `/reload`. See [the Pi checkpoint specification](docs/pi-stream-checkpoint-spec.md) for behaviour and configuration.

## Supported versions

The generic CLI and Pi extension are tested with Python 3.11 and 3.14, Node 26 and Pi 0.99, on macOS and Linux. Herdr opening and its links pane are a macOS-only prototype targeting Herdr 0.9.3 or newer.

## Develop

```sh
uv sync
npm ci
mise run test
```

## License

MIT
