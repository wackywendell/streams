"""The Python boundary forwards argv without parsing model or lab flags."""

import contextlib
import io
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

from _streams import cli, prompt_lab_cli


class PromptLabCliTests(unittest.TestCase):
    def test_exact_argv_environment_and_exit_code(self):
        with tempfile.TemporaryDirectory() as directory:
            runner = Path(directory).resolve() / "runner with spaces.mjs"
            runner.write_text("// fake")
            arguments = ["replay", "capture id", "--model", "p/m/variant", "--prompt", "a file", "--json"]
            original = {"STREAMS_PROMPT_LAB_RUNNER": str(runner), "STREAMS_BIN_PATH": "/wrong/streams",
                        "STREAMS_BIN": "/also/wrong", "PYTHONPATH": "/existing", "PI_CODING_AGENT_DIR": directory}
            seen = {}

            def run(argv, *, env):
                self.assertEqual(argv, ["node", str(runner), *arguments])
                self.assertEqual(env["STREAMS_BIN_PATH"], env["STREAMS_BIN"])
                backend = Path(env["STREAMS_BIN_PATH"])
                self.assertTrue(backend.is_file())
                self.assertEqual(backend.stat().st_mode & 0o777, 0o700)
                text = backend.read_text()
                self.assertIn(sys.executable, text)
                self.assertIn('-P -m _streams.cli "$@"', text)
                root, existing = env["PYTHONPATH"].split(os.pathsep)
                self.assertEqual(root, str(Path(prompt_lab_cli.__file__).resolve().parents[1]))
                self.assertEqual(existing, "/existing")
                self.assertEqual(env["PI_CODING_AGENT_DIR"], directory)
                seen["backend"] = backend
                return Mock(wait=Mock(return_value=7))

            with patch.dict(os.environ, original, clear=True), patch.object(prompt_lab_cli.subprocess, "Popen", side_effect=run):
                self.assertEqual(cli.main(["debug", *arguments]), 7)
                self.assertEqual(os.environ["STREAMS_BIN_PATH"], "/wrong/streams")
            self.assertFalse(seen["backend"].exists())

    def test_every_verb_and_help_forward_without_python_parser(self):
        for args in [["capture", "stream", "s"], ["prepare", "--request", "r"], ["run", "r", "--model", "p/m"],
                     ["replay", "c", "--reset"], ["inspect", "run", "r", "--offset", "0"], ["list", "run"],
                     ["--help"], ["run", "--help"], ["unknown", "--bad-flag"], []]:
            with self.subTest(args=args), patch.object(prompt_lab_cli, "forward", return_value=2) as forward:
                self.assertEqual(cli.main(["debug", *args]), 2)
                forward.assert_called_once_with(args)

    def test_module_and_entrypoint_argv(self):
        with patch.object(sys, "argv", ["streams", "debug", "list", "capture", "--json"]), patch.object(prompt_lab_cli, "forward", return_value=0) as forward:
            self.assertEqual(cli.main(), 0)
            forward.assert_called_once_with(["list", "capture", "--json"])

    def test_missing_explicit_runner_fails_clearly_without_fallback(self):
        with patch.dict(os.environ, {"STREAMS_PROMPT_LAB_RUNNER": "/not/installed/lab-cli.mjs"}, clear=True), patch.object(prompt_lab_cli.subprocess, "Popen") as run:
            output = io.StringIO()
            with contextlib.redirect_stderr(output):
                self.assertEqual(cli.main(["debug", "list", "capture"]), 2)
            self.assertIn("STREAMS_PROMPT_LAB_RUNNER", output.getvalue())
            run.assert_not_called()

    def test_source_checkout_runner_and_top_level_help(self):
        with patch.dict(os.environ, {}, clear=True):
            self.assertEqual(prompt_lab_cli.runner_path(), Path(__file__).resolve().parents[2] / "extensions/stream-checkpoint/lab-cli.mjs")
        self.assertIn("debug", cli.build_parser().format_help())

    def test_missing_node_is_a_clean_cli_failure(self):
        with patch.object(prompt_lab_cli.subprocess, "Popen", side_effect=FileNotFoundError("node is unavailable")):
            output = io.StringIO()
            with contextlib.redirect_stderr(output):
                self.assertEqual(cli.main(["debug", "--help"]), 2)
            self.assertIn("node is unavailable", output.getvalue())

    def test_signal_exit_code(self):
        with patch.object(prompt_lab_cli.subprocess, "Popen", return_value=Mock(wait=Mock(return_value=-15))):
            self.assertEqual(prompt_lab_cli.forward(["list", "run"]), 143)

    def test_signal_forwards_and_waits_before_cleaning_backend(self):
        child = Mock()
        original = {number: signal.getsignal(number) for number in (signal.SIGINT, signal.SIGTERM)}
        def wait():
            signal.raise_signal(signal.SIGTERM)
            child.send_signal.assert_called_once_with(signal.SIGTERM)
            return 130
        child.wait.side_effect = wait
        with patch.object(prompt_lab_cli.subprocess, "Popen", return_value=child):
            self.assertEqual(prompt_lab_cli.forward(["run", "r", "--model", "p/m"]), 130)
        for number, handler in original.items():
            self.assertEqual(signal.getsignal(number), handler)

    def test_pinned_backend_executes_this_module_without_catalog_writes(self):
        seen = []
        real_popen = subprocess.Popen
        def run(argv, *, env):
            # --help does not load a catalog, but proves executable, module and argv pinning.
            result = real_popen([env["STREAMS_BIN_PATH"], "--help"], env=env, cwd=competing, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            stdout, stderr = result.communicate(timeout=15)
            self.assertEqual(result.returncode, 0, stderr)
            self.assertIn("Manage durable work streams", stdout)
            seen.append(True)
            return Mock(wait=Mock(return_value=0))
        with tempfile.TemporaryDirectory() as directory, tempfile.TemporaryDirectory() as competing, patch.dict(os.environ, {"HOME": directory, "STREAMS_CATALOG_DIR": str(Path(directory) / "catalog")}), patch.object(prompt_lab_cli.subprocess, "Popen", side_effect=run):
            package = Path(competing) / "_streams"
            package.mkdir()
            (package / "__init__.py").write_text("")
            (package / "cli.py").write_text("raise RuntimeError('wrong checkout')")
            self.assertEqual(prompt_lab_cli.forward(["list", "capture"]), 0)
            self.assertEqual(list(Path(directory).iterdir()), [])
        self.assertEqual(seen, [True])


if __name__ == "__main__":
    unittest.main()
