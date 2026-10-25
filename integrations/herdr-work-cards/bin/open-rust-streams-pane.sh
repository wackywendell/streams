#!/bin/sh
# Never fall back to PATH or an installed Python CLI.
set -eu
: "${STREAMS_RUST_BIN:?Missing explicit Rust binary}"
case "$STREAMS_RUST_BIN" in
  /*) ;;
  *) printf '%s\n' 'STREAMS_RUST_BIN must be absolute' >&2; exit 1 ;;
esac
: "${STREAMS_CARDS_CONTEXT:?Missing card launch inputs}"
exec "$STREAMS_RUST_BIN" native pane
