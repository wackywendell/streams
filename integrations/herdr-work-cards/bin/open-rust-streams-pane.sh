#!/bin/sh
# Never fall back to PATH or an installed Python CLI.
set -eu
: "${STREAMS_RUST_BIN:?Missing explicit Rust binary}"
case "$STREAMS_RUST_BIN" in
  /*) ;;
  *) printf '%s\n' 'STREAMS_RUST_BIN must be absolute' >&2; exit 1 ;;
esac
: "${STREAMS_CARDS_SCOPE:?Missing captured scope}"
: "${STREAMS_CARDS_OWNER:?Missing captured owner}"
: "${STREAMS_CARDS_NONCE:?Missing launch nonce}"
: "${STREAMS_CARDS_OWNER_TERMINAL:?Missing owner terminal}"
: "${STREAMS_CARDS_READ_ENV:?Missing reader environment}"
: "${STREAMS_CARDS_READ_CWD:?Missing reader directory}"
: "${STREAMS_CARDS_STATE_DIR:?Missing evidence directory}"
: "${STREAMS_CARDS_PLUGIN_ROOT:?Missing plugin root}"
exec "$STREAMS_RUST_BIN" native pane
