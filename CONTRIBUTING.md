# Contributing

## Set up

Install [mise](https://mise.jdx.dev/), then:

```sh
mise install   # Node 26 and pinned Rust
npm ci
npm --prefix integrations/herdr-work-cards ci --ignore-scripts
mise run build
```

Rust owns the CLI, catalog, native effects and terminal card. TypeScript owns the
in-process Pi boundary and its supporting configuration, wire parsing and naming.
The compiler is pinned in `rust-toolchain.toml` and `mise.toml`. No Python, Z3
or libclang dependency is required.

## Test

```sh
mise run test:fast         # Rust contracts and Pi boundary
mise run test              # full acceptance, including real Git
mise run test:rust         # all Rust targets and doctests
mise run test:rust:fast    # Rust library, binary and consolidated contracts
mise run test:rust:unit    # focused library/binary loop; not full acceptance
mise run test:pi           # Pi reporting, capture and helper-request tests
mise run test:herdr        # configuration, naming and launcher boundaries
mise run test:replay       # production helper replay with real Rust preview
mise run test:native:rust  # Pi-to-Rust lifecycle with fake Herdr endpoints
```

Use the smallest relevant task while iterating, then run the complete landing
gates:

```sh
MISE_AUTO_INSTALL=false mise run -j 1 test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Target one Rust area with:

```sh
mise exec -- sh -c 'RUSTC=$(rustup which rustc) RUSTDOC=$(rustup which rustdoc) cargo test --locked --test contracts record_updates'
```

Contract files share one test binary. Record-local and file-transport tests
exercise current production paths; there is no separate formal protocol model.
Frozen wire/rendering fixtures, fault injection and real process tests retain
boundary evidence. See the [testing policy](docs/testing.md) for gate choices,
isolation and what those tests do not establish.

Tasks isolate fixture repositories from personal Git configuration, hooks,
fsmonitor and signing. They do not change production signing. CI runs acceptance
on Ubuntu and macOS. Use mise for builds to select the pinned compiler. Node tests
that invoke a real CLI use this checkout's Cargo target; the task graph builds it
first.

## Try local changes without installing

Use `target/debug/streams`, or `$CARGO_TARGET_DIR/debug/streams` when configured.
The Pi package resolves the same debug target by default. An absolute
`STREAMS_BIN_PATH` overrides it. Packaged users must select a matching Rust
executable explicitly. There is no Python or `PATH` fallback.

Load only one checkout's Pi extension. Building and testing do not replace
installed tools or reload running processes. Use isolated homes and fake native
endpoints; installed-runtime changes and live-platform acceptance are separate.

The [catalog ADR](docs/durable-work-catalog.md) defines records,
[record-local operations](docs/record-local-operations.md) defines saving and
transport, the [Pi ADR](docs/pi-work-card.md) defines reporting, and the
[runtime ADR](docs/rust-temper-architecture.md) defines the Rust/Pi split.
