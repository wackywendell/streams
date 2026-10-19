# Contributing

## Set up

Install [mise](https://mise.jdx.dev/), then:

```sh
mise install   # Node 26, uv and pinned Rust
uv sync
npm ci
npm --prefix integrations/herdr-work-cards ci --ignore-scripts
```

The Rust verification tests need Z3 headers/libraries, pkg-config and libclang:
`brew install z3 pkg-config` on macOS (with Xcode command-line tools), or
`sudo apt-get install libz3-dev libclang-dev pkg-config` on Ubuntu. Production
builds do not link Z3. Temper is pinned in `Cargo.toml`; the workspace compiler
is pinned in `rust-toolchain.toml` and `mise.toml`.

## Test

```sh
mise run test          # everything
mise run test:rust     # Rust catalog, fault sequences and Temper L0–L3
mise run test:differential # build adapter, compare durable Python/Rust traces
mise run test:python   # Python CLI reference tests
mise run test:pi       # Pi extension tests
mise run test:herdr    # imported native Work Cards regression tests
```

Set `UV_PYTHON` to test another Python version, for example `UV_PYTHON=3.11 mise run test:python`. CI runs `mise run test` on Ubuntu and macOS with Python 3.11 and 3.14.

Use the mise tasks for Rust builds in this environment: they resolve the
compiler to an absolute path before Cargo enters the Temper checkout, whose
own toolchain file would otherwise switch a mise/rustup shim mid-build.
The Rust slice is not yet the installed `streams` CLI; Python remains active.

## Try local changes

```sh
uv tool install -e .   # streams CLI from this checkout
pi install .           # Pi extension and skill from this checkout
```

The [catalog ADR](docs/durable-work-catalog.md) defines record meaning and the [Pi ADR](docs/pi-work-card.md) defines reporting and card interaction. The [native Herdr ADR](docs/native-herdr-work-cards.md) governs adaptation of the imported Work Cards UI. Keep usage in the README and implementation contracts beside code; the scratch-format and checkpoint specifications are historical.

Do not install the imported package's old Pi entrypoint. Its regression suite preserves the standalone implementation's coverage; the Streams-backed native adapter is not yet active.
