# Contributing

## Set up

Install [mise](https://mise.jdx.dev/), then:

```sh
mise install   # Node 26 and uv
uv sync
npm ci
```

## Test

```sh
mise run test          # everything
mise run test:python   # CLI tests
mise run test:pi       # Pi extension tests
```

Set `UV_PYTHON` to test another Python version, for example `UV_PYTHON=3.11 mise run test:python`. CI runs `mise run test` on Ubuntu and macOS with Python 3.11 and 3.14.

## Try local changes

```sh
uv tool install -e .   # streams CLI from this checkout
pi install .           # Pi extension and skill from this checkout
```

The CLI's scratch format lives in `src/_streams/FORMAT.md` and is printed by `streams format`. The Pi extension's behaviour is described in `docs/pi-stream-checkpoint-spec.md`. Update both when behaviour changes.
