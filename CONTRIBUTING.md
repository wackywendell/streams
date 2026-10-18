# Contributing

## Set up

Install [mise](https://mise.jdx.dev/), then:

```sh
mise install   # Node 26 and uv
uv sync
npm ci
npm --prefix integrations/herdr-work-cards ci --ignore-scripts
```

## Test

```sh
mise run test          # everything
mise run test:python   # CLI tests
mise run test:pi       # Pi extension tests
mise run test:herdr    # imported native Work Cards regression tests
```

Set `UV_PYTHON` to test another Python version, for example `UV_PYTHON=3.11 mise run test:python`. CI runs `mise run test` on Ubuntu and macOS with Python 3.11 and 3.14.

## Try local changes

```sh
uv tool install -e .   # streams CLI from this checkout
pi install .           # Pi extension and skill from this checkout
```

The [catalog ADR](docs/durable-work-catalog.md) defines record meaning and the [Pi ADR](docs/pi-work-card.md) defines reporting and card interaction. The [native Herdr ADR](docs/native-herdr-work-cards.md) governs adaptation of the imported Work Cards UI. Keep usage in the README and implementation contracts beside code; the scratch-format and checkpoint specifications are historical.

Do not install the imported package's old Pi entrypoint. Its regression suite preserves the standalone implementation's coverage; the Streams-backed native adapter is not yet active.
