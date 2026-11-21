#!/bin/sh
# Never fall back to PATH or an installed Python CLI.
set -eu
: "${STREAMS_BIN_PATH:?Missing explicit Rust binary}"
case "$STREAMS_BIN_PATH" in
  /*) ;;
  *) printf '%s\n' 'STREAMS_BIN_PATH must be absolute' >&2; exit 1 ;;
esac
: "${STREAMS_CARDS_CONTEXT:?Missing card launch inputs}"
exec "$STREAMS_BIN_PATH" card pane
