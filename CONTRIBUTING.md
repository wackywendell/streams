# Contributing

## Set up

Install [mise](https://mise.jdx.dev/), then:

```sh
mise install   # Node 26 and pinned Rust
npm ci
npm --prefix integrations/herdr-work-cards ci --ignore-scripts
mise run build
```

This branch has no Python implementation or Python test dependency. Rust owns the
CLI, catalog, native effects and terminal card. TypeScript owns the in-process Pi
boundary and its supporting configuration, wire parsing, naming and subprocess code.

Rust verification needs Z3 headers/libraries, pkg-config and libclang:
`brew install z3 pkg-config` on macOS with Xcode command-line tools, or
`sudo apt-get install libz3-dev libclang-dev pkg-config` on Ubuntu. Production
builds do not link Z3. Temper is pinned in `Cargo.toml`; the compiler is pinned
in `rust-toolchain.toml` and `mise.toml`.

## Test

```sh
mise run test:fast         # Rust contracts and Pi boundary
mise run test              # full acceptance, including real Git and Temper
mise run test:rust         # all Rust targets, recovery, Temper L0–L3 and doctests
mise run test:rust:fast    # Rust library, binary and consolidated contracts
mise run test:rust:unit    # focused library/binary loop; not full acceptance
mise run test:pi           # Pi reporting, capture and helper-request tests
mise run test:herdr        # configuration, naming and subprocess boundaries
mise run test:replay       # production helper replay with real Rust preview
mise run test:native:rust  # Pi-to-Rust lifecycle with fake Herdr endpoints
```

Use `test:rust:unit` for focused library/binary iterations, then run the relevant
contracts before review. The [testing policy](docs/testing.md) records gate choices,
physical boundary coverage and measured costs.

Target one Rust area with:

```sh
mise exec -- sh -c 'RUSTC=$(rustup which rustc) cargo test --locked --test contracts runtime_state'
```

Contract files share one test binary. `git_catalog` and `verification` remain
separate targets for real Git recovery and the full Temper cascade. The fast gate
retains native-open model checks, but does not replace full acceptance before a
trial or release. Python differential suites and tests importing the retired
TypeScript coordinator are gone; frozen wire/rendering fixtures and Rust contract,
fault-injection and process tests remain. Test counts across the cutover are not
comparable measures of coverage.

Tasks isolate fixture repositories from personal Git configuration, hooks,
fsmonitor and signing. They do not change your Git configuration or production
signing; signing-failure tests still set their own fixture policy. CI runs the
Rust-only acceptance graph on Ubuntu and macOS.

Use mise for builds: it resolves the compiler before Cargo enters Temper's checkout,
whose toolchain file could otherwise switch a mise/rustup shim mid-build. Node tests
that invoke a real CLI use this checkout's requested Cargo target, not an installed
`streams`. The task graph builds that CLI before starting those tests.

## Try local changes without installing

Use `target/debug/streams`, or `$CARGO_TARGET_DIR/debug/streams` when configured.
The Pi package resolves the same debug target by default. An absolute
`STREAMS_BIN_PATH` overrides it; `STREAMS_RUST_BIN` is a compatibility alias, not an
independent native-backend selector. Conflicting selectors fail rather than mixing
builds. Packaged users must select their matching Rust executable explicitly.

Load only one checkout's Pi extension. Building or testing this branch does not
replace globally installed tools, reload Pi or activate native effects. Isolated
PTY and live-platform acceptance remain required before a trial.

The [catalog ADR](docs/durable-work-catalog.md) defines records, the
[Pi ADR](docs/pi-work-card.md) defines reporting, and the
[Rust runtime ADR](docs/rust-temper-architecture.md) defines the backend boundary.
The old scratch/checkpoint specifications and Python cutover plans are historical.
