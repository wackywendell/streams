# Contributing

## Set up

Install [mise](https://mise.jdx.dev/), then:

```sh
mise install   # Node 26, pinned Rust, just, and direnv from .tool-versions
npm ci
npm --prefix integrations/herdr-work-cards ci --ignore-scripts
just build
```

Rust owns the CLI, catalog, native effects and terminal card. TypeScript owns the
in-process Pi boundary and its supporting configuration, wire parsing and naming.
The compiler and tool versions are pinned in `rust-toolchain.toml` and `.tool-versions`, with tasks in `Justfile` and environment activation in `.envrc`. No Python, Z3
or libclang dependency is required.

## Test

```sh
just test-fast         # Rust contracts and Pi boundary
just test              # full acceptance, including real Git
just test-rust         # all Rust targets and doctests
just test-rust-fast    # Rust library, binary and consolidated contracts
just test-rust-unit    # focused library/binary loop; not full acceptance
just test-pi           # Pi reporting, capture and helper-request tests
just test-herdr        # configuration, naming and launcher boundaries
just test-herdr-app    # herdr-app mod state, batching and viewer dispatch tests
just test-replay       # production helper replay with real Rust preview
just test-native-rust  # Pi-to-Rust lifecycle with fake Herdr endpoints
```

Use the smallest relevant task while iterating, then run the complete landing
gates:

```sh
just test
just check-clippy
just check-fmt
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
`STREAMS_BIN_PATH` overrides it. Either way, the binary must be built from this
checkout's current Rust sources: `build.rs` embeds a source digest that the
binary reports in its ready frame, and the extension refuses a binary whose
digest differs from its checkout's, so edit, `just build`, then reload Pi.
Packaged users, whose extension has no sources to compare, must select a
matching Rust executable explicitly. There is no Python or `PATH` fallback.

Load only one checkout's Pi extension. Building and testing do not replace
installed tools or reload running processes. Use isolated homes and fake native
endpoints; installed-runtime changes and live-platform acceptance are separate.

The [catalog ADR](docs/durable-work-catalog.md) defines records,
[record-local operations](docs/record-local-operations.md) defines saving and
transport, the [Pi ADR](docs/pi-work-card.md) defines reporting, and the
[runtime ADR](docs/rust-temper-architecture.md) defines the Rust/Pi split.
