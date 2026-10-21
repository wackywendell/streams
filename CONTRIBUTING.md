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
mise run test:fast     # development: Rust contracts and Pi extension tests
mise run test          # full acceptance, including Git and cross-language parity
mise run test:rust     # all Rust tests, fault sequences, Temper L0–L3 and doctests
mise run test:differential # build adapters, compare Python/Rust traces once
mise run test:python   # Python reference tests, excluding the differential gate
mise run test:pi       # Pi extension tests
mise run test:herdr    # imported native Work Cards regression tests
```

For a Rust-only edit, run `mise run test:rust:fast`. To target one area, use:

```sh
mise exec -- sh -c 'RUSTC=$(rustup which rustc) cargo test --locked --test contracts runtime_state'
```

Contract files share one integration-test binary, so a library edit no longer
links a binary for every small test file. The separate `git_catalog` and
`verification` targets retain all real Git recovery traces and the catalog
Temper cascade. `test:fast` does not replace full acceptance before a trial or
release. It keeps the complete contract suite, including native-open model checks,
without running Git publication traces, Python parity, or the legacy Herdr suite.

Test tasks suppress inherited global/system Git configuration only for their
subprocesses. Personal hooks, fsmonitor and signing settings must not run inside
fixture repositories. This changes neither your Git configuration nor production
signing. Signing-failure tests still set and verify their explicit fixture policy.

The Git parity observer batches exact blob bytes and caches them by immutable
object ID. It still compares every requested step, record and reachable history.
No recovery traces, property-test cases or parity checks were removed.

The full task graph builds Rust probes once. Python reference and differential
gates select disjoint modules; full Python discovery would run parity twice.
Direct `uv run python -m unittest discover tests/python` still runs both groups.

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
