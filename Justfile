export RUSTUP_TOOLCHAIN := "1.95.0"

default:
    @just --list

# Build this checkout's Rust CLI
build:
    RUSTC=$(rustup which rustc) RUSTDOC=$(rustup which rustdoc) cargo build --locked --bin streams

# Check every Rust target with warnings denied
check-clippy:
    RUSTC=$(rustup which rustc) RUSTDOC=$(rustup which rustdoc) cargo clippy --locked --all-targets -- -D warnings

# Check Rust formatting without rewriting files
check-fmt:
    cargo fmt --all -- --check

# Focused lib/bin loop; does not replace contracts or acceptance
test-rust-unit:
    GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1 RUSTC=$(rustup which rustc) RUSTDOC=$(rustup which rustdoc) cargo test --locked --lib --bins -- --test-threads=4

# Run Rust contracts in one harness; filter with cargo test --test contracts <module>
test-rust-fast:
    GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1 RUSTC=$(rustup which rustc) RUSTDOC=$(rustup which rustdoc) cargo test --locked --lib --bins --test contracts -- --test-threads=4

# Run bounded Temper formal verification and production conformance checks
test-verify:
    GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1 RUSTC=$(rustup which rustc) RUSTDOC=$(rustup which rustdoc) cargo test --locked --test verification -- --test-threads=4

# Run all Rust record, file transport, native, verification and documentation tests
test-rust:
    GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1 RUSTC=$(rustup which rustc) RUSTDOC=$(rustup which rustdoc) cargo test --locked --all-targets -- --test-threads=4
    GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1 RUSTC=$(rustup which rustc) RUSTDOC=$(rustup which rustdoc) cargo test --locked --doc

# Run Pi checkpoint and frozen helper-request tests against this Rust build
test-pi: build
    node --test tests/stream_child_test.mjs tests/stream_checkpoint_test.mjs tests/stream_sync_test.mjs tests/stream_outcomes_test.mjs tests/stream_native_test.mjs tests/stream_hc_lifecycle_test.mjs tests/stream_naming*_test.mjs tests/stream_session_naming_test.mjs tests/stream_wire_test.mjs tests/stream_bin_test.mjs tests/stream_prompt_*_test.mjs tests/stream_debug_settings_test.mjs tests/stream_capture_*_test.mjs tests/stream_lab_tools_test.mjs tests/stream_artifacts_test.mjs

# Exercise the Pi child protocol without starting Rust or Herdr
test-pi-child:
    node --test tests/stream_child_test.mjs

# Run Pi native lifecycle and child counting tests without Rust
test-pi-native:
    node --test tests/stream_native_test.mjs tests/stream_child_test.mjs

# Exercise Pi Session state and helper retirement without native I/O
test-pi-hc-context:
    node --test tests/stream_hc_lifecycle_test.mjs

# Opt-in selected Pi SDK runtime check; requires STREAMS_PI_SDK_ENTRY
test-pi-hc-runtime:
    node --test tests/stream_hc_sdk_runtime_test.mjs

# Run isolated Pi catalog sync acceptance
test-pi-sync: build
    node --test tests/stream_sync_test.mjs

# Replay production Pi helpers through the Rust CLI in isolated homes
test-replay: build
    node --test tests/stream_rust_replay_test.mjs

# Exercise Rust-only Pi native routing and launch assets through an isolated CLI
test-native-rust: build
    node --test tests/stream_rust_native_test.mjs

# Run Herdr document and artifact launcher tests
test-herdr:
    npm --prefix integrations/herdr-work-cards test

# Run herdr-app mod state, batching and viewer dispatch unit tests
test-herdr-app:
    node --test integrations/herdr-app/tests/state.test.mjs

# Development gate: Rust contracts and Pi tests, not full acceptance
test-fast: test-rust-fast test-pi

# Full Rust-only acceptance: contracts, file transport, Pi, Herdr boundaries and replay
test: test-rust test-pi test-herdr test-herdr-app test-replay test-native-rust

# Install the streams Rust CLI release binary into STREAMS_INSTALL_ROOT (default ~/.local)
install-cli:
    #!/usr/bin/env sh
    set -eu
    root="${STREAMS_INSTALL_ROOT:-$HOME/.local}"
    RUSTC=$(rustup which rustc) RUSTDOC=$(rustup which rustdoc) cargo install --locked --path . --root "$root" --bin streams --force

# Install the herdr-app Streams mod into HERDR_APP_HOME without modifying layouts
install-herdr-app-mod:
    #!/usr/bin/env sh
    set -eu
    home="${HERDR_APP_HOME:-$HOME/.config/herdr-app}"
    mkdir -p "$home/mods/streams/icons"
    cp integrations/herdr-app/mod.json integrations/herdr-app/main.js integrations/herdr-app/state.js "$home/mods/streams/"
    cp integrations/herdr-app/icons/*.svg "$home/mods/streams/icons/"
    if command -v herdr-app >/dev/null 2>&1; then
      herdr-app check "$home/mods/streams"
    fi

# Install the herdr-app Streams mod and standalone streams.json layout into HERDR_APP_HOME
install-herdr-app: install-herdr-app-mod
    #!/usr/bin/env sh
    set -eu
    home="${HERDR_APP_HOME:-$HOME/.config/herdr-app}"
    mkdir -p "$home/layouts"
    cp integrations/herdr-app/layouts/streams.json "$home/layouts/streams.json"

# Install the streams CLI binary and herdr-app mod
install: install-cli install-herdr-app-mod
