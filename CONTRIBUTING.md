# Contributing

## Set up

Install [mise](https://mise.jdx.dev/), then:

```sh
mise install   # Node 26, pinned Rust, just, and direnv from .tool-versions
npm ci
just build
```

Rust owns the CLI, format-6 catalog storage, edit engine, VCS inspection, and classic Herdr card projection. TypeScript owns the in-process Pi extension (`extensions/stream-checkpoint/`) and its reporting, naming, and prompt-lab helpers. JavaScript under `integrations/herdr-app/` owns the `herdr-app` mod talking directly to the public `streams` CLI and `hc`.

## Test

```sh
just test-fast         # Rust contracts and Pi boundary
just test              # full acceptance, including real Git
just test-rust         # all Rust targets and doctests
just test-rust-fast    # Rust library, binary and consolidated contracts
just test-rust-unit    # focused library/binary loop; not full acceptance
just test-pi           # Pi reporting, capture and helper-request tests
just test-herdr        # Herdr document and artifact launcher tests
just test-herdr-app    # herdr-app mod state, focus refresh and viewer tests
just test-replay       # production helper capture and in-memory replay tests
just test-native-rust  # Pi-to-Rust card mode and hc label integration tests
```

Use the smallest relevant task while iterating, then run the complete landing gates:

```sh
just test
just check-clippy
just check-fmt
```

Target one Rust area with:

```sh
mise exec -- sh -c 'GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1 RUSTC=$(rustup which rustc) RUSTDOC=$(rustup which rustdoc) cargo test --locked --test contracts record_updates'
```

Tasks isolate fixture repositories from personal Git configuration, hooks, fsmonitor and signing. Node tests that invoke a real CLI use this checkout's Cargo target (`target/debug/streams`).

## Try local changes without installing

Use `target/debug/streams`, or `$CARGO_TARGET_DIR/debug/streams` when configured. The Pi extension prefers an installed `streams` on `PATH` over the debug target, so set `STREAMS_BIN_PATH` to the absolute debug path when `streams` is installed. An absolute `STREAMS_BIN_PATH` overrides both.

Load only one checkout's Pi extension. Building and testing do not replace installed tools or reload running processes. Use isolated homes and fake native endpoints when testing.

Design records live in `docs/`, including [`docs/cli-by-entity.md`](docs/cli-by-entity.md), [`docs/session-references.md`](docs/session-references.md), [`docs/record-local-operations.md`](docs/record-local-operations.md), and [`docs/testing.md`](docs/testing.md).
