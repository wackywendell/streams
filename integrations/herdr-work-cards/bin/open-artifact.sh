#!/bin/sh
# Rust resolves the reference; the viewer never receives a catalog file path.
(
  set -eu
  : "${STREAMS_BIN_PATH:?Missing explicit Rust binary}"
  case "$STREAMS_BIN_PATH" in
    /*) ;;
    *) printf '%s\n' 'STREAMS_BIN_PATH must be absolute' >&2; exit 1 ;;
  esac
  : "${STREAMS_ARTIFACT_REFERENCE:?Missing artifact reference}"
  : "${STREAMS_ARTIFACT_CATALOG_SOURCE:?Missing catalog identity}"
  : "${STREAMS_ARTIFACT_CWD:?Missing reader working directory}"
  cd "$STREAMS_ARTIFACT_CWD"
  exec "$STREAMS_BIN_PATH" artifact view "$STREAMS_ARTIFACT_REFERENCE"
)
status=$?
[ "$status" -eq 0 ] && exit 0
printf '\nStreams artifact viewer failed (%s). See the error above.\nPress Enter to close this tab.\n' "$status" >&2
IFS= read -r answer
exit "$status"
