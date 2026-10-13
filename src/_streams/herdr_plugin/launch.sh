#!/bin/sh
# Native plugin argv is static. The opening CLI supplies its own runtime.
set -eu
: "${STREAMS_PYTHON:?opening CLI must supply STREAMS_PYTHON}"
: "${STREAMS_IMPORT_ROOT:?opening CLI must supply STREAMS_IMPORT_ROOT}"
: "${STREAMS_BINDING:?opening CLI must supply STREAMS_BINDING}"
unset PYTHONHOME
export PYTHONPATH="$STREAMS_IMPORT_ROOT"
exec "$STREAMS_PYTHON" -P -m _streams.herdr_links --binding "$STREAMS_BINDING"
