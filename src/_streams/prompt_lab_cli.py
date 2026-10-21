"""Forward debug commands to the shared Node frontend, never to a Python model client."""

from __future__ import annotations

import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys
import tempfile


def runner_path() -> Path:
    override = os.environ.get("STREAMS_PROMPT_LAB_RUNNER")
    if override:
        path = Path(override).expanduser().resolve()
    else:
        path = Path(__file__).resolve().parents[2] / "extensions" / "stream-checkpoint" / "lab-cli.mjs"
    if not path.is_file():
        raise OSError(
            f"Streams prompt lab frontend is not installed at {path}; "
            "install the matching pi-streams package and set STREAMS_PROMPT_LAB_RUNNER to its lab-cli.mjs"
        )
    return path


def forward(argv: list[str]) -> int:
    runner = runner_path()
    env = dict(os.environ)
    # The TS backend accepts an executable path, not an argv array. Give it a
    # private exec-only launcher pinned to this interpreter and module tree.
    module_root = str(Path(__file__).resolve().parents[1])
    env["PYTHONPATH"] = os.pathsep.join(filter(None, (module_root, env.get("PYTHONPATH"))))
    with tempfile.TemporaryDirectory(prefix="streams-debug-") as directory:
        backend = Path(directory) / "streams"
        backend.write_text(
            f"#!/bin/sh\nexec {shlex.quote(sys.executable)} -P -m _streams.cli \"$@\"\n",
            encoding="utf-8",
        )
        backend.chmod(0o700)
        env["STREAMS_BIN"] = env["STREAMS_BIN_PATH"] = str(backend)
        child = subprocess.Popen(["node", str(runner), *argv], env=env)
        # Keep the backend launcher alive until Node has saved cancellation
        # artifacts. Python's default SIGINT handler would interrupt wait().
        def cancel(signum, _frame):
            try:
                child.send_signal(signum)
            except ProcessLookupError:
                pass
        handlers = {signum: signal.signal(signum, cancel) for signum in (signal.SIGINT, signal.SIGTERM)}
        try:
            code = child.wait()
        finally:
            for signum, handler in handlers.items():
                signal.signal(signum, handler)
    return code if code >= 0 else 128 - code
