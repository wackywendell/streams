"""Real CLI handoffs share physical catalog/runtime state, not mocked backends."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from uuid import NAMESPACE_URL, uuid5

ROOT = Path(__file__).resolve().parents[2]
RUST = ROOT / "target" / "debug" / "streams"


@unittest.skipUnless(RUST.is_file(), "build the Rust streams binary first")
class RustCliDifferentialTests(unittest.TestCase):
    def setUp(self):
        self.maxDiff = None
        self.temporary = tempfile.TemporaryDirectory(prefix="streams-cli-handoff-")
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name).resolve()
        self.cwd = self.base / "working"
        self.cwd.mkdir()
        home = self.base / "home"
        home.mkdir()
        gitconfig = home / "gitconfig"
        gitconfig.write_text("[user]\n name = Streams Test\n email = streams@example.test\n[commit]\n gpgSign = false\n", encoding="utf-8")
        self.env = {
            "HOME": str(home),
            "XDG_CONFIG_HOME": str(home / "config"),
            "XDG_DATA_HOME": str(home / "data"),
            "XDG_STATE_HOME": str(home / "state"),
            "PATH": os.environ.get("PATH", ""),
            "PYTHONPATH": str(ROOT / "src"),
            "PYTHONUTF8": "1",
            "PYTHONDONTWRITEBYTECODE": "1",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": str(gitconfig),
        }
        self.call("rust", "init", "--sync", "none")
        self.call("rust", "config", "set", "frontend.mode", "none")

    def invoke(self, implementation, *arguments, request=None):
        executable = [str(RUST)] if implementation == "rust" else [sys.executable, "-m", "_streams.cli"]
        result = subprocess.run(
            executable + list(arguments) + ["--json"],
            input=json.dumps(request, ensure_ascii=False) if request is not None else None,
            text=True, capture_output=True, cwd=self.cwd, env=self.env, timeout=30,
        )
        try:
            data = json.loads(result.stdout)
        except json.JSONDecodeError as error:
            self.fail(f"{implementation} {arguments}: {error}; stdout={result.stdout!r}; stderr={result.stderr!r}")
        return result.returncode, data

    def call(self, implementation, *arguments, request=None):
        code, data = self.invoke(implementation, *arguments, request=request)
        self.assertEqual(code, 0, (implementation, arguments, data))
        return data

    def context(self, implementation, native_id, stream_id=None):
        arguments = ["agent", "context", "--harness", "pi", "--session-id", native_id]
        if stream_id is not None:
            arguments += ["--stream-id", stream_id]
        return self.call(implementation, *arguments)

    def test_commands_read_each_others_records_and_preserve_visibility(self):
        stream = self.call("python", "create", "Straße", "--goal", "Literal [plan]", "--status", "waiting")["stream"]
        identity = stream["id"]
        for arguments in [("show", "STRASSE"), ("list", "literal ["), ("check", identity), ("sync",)]:
            self.assertEqual(self.call("rust", *arguments), self.call("python", *arguments))
        archived = self.call("rust", "archive", identity)["stream"]
        self.assertEqual(archived["status"], "waiting")
        for arguments in [("list",), ("list", "--all"), ("list", "--archived")]:
            self.assertEqual(self.call("rust", *arguments), self.call("python", *arguments))
        self.call("python", "restore", identity)
        self.call("rust", "update", identity, "--add-repository", "repo", "https://example.test/repo",
                  "--add-document", "repo", "main", "docs/design.md", "Design",
                  "--document-added-at", "repo", "docs/design.md", "abc123",
                  "--document-commit", "repo", "docs/design.md", "def456",
                  "--document-session", "repo", "docs/design.md", "source-session")
        self.assertEqual(self.call("rust", "show", identity), self.call("python", "show", identity))

    def test_rust_attached_sessions_are_deterministic_and_visible_to_python_context(self):
        stream = self.call("rust", "create", "Partitioned")["stream"]
        native = "opaque/路径 😀"
        session = self.call("rust", "session", "attach", stream["id"], "--harness", "pi",
                            "--session-id", native, "--title", "Contribution")["session"]
        expected = str(uuid5(NAMESPACE_URL, "streams:session:" + json.dumps([session["host_id"], "pi", native])))
        self.assertEqual(session["id"], expected)
        root = Path(self.call("rust", "config", "get", "catalog.root"))
        self.assertTrue((root / "sessions" / stream["id"] / session["host_id"] / f"{expected}.yaml").is_file())
        self.assertEqual(self.context("rust", native), self.context("python", native))
        self.assertEqual(self.call("python", "session", "attach", stream["id"], "--harness", "pi",
                                  "--session-id", native, "--title", "Contribution")["session"], session)

    def test_generated_stream_intent_survives_unrelated_concurrent_changes(self):
        stream = self.call("rust", "create", "Work")["stream"]
        captured = self.context("rust", "unknown", stream["id"])
        request = {"operation_id": "generated-stream-goal", "catalog_source": captured["catalog_source"],
                   "kind": "stream", "actor": "automatic", "before": stream,
                   "ownership_token": captured["authority"]["token"],
                   "changes": {"goal": "Generated"}, "inputs": []}
        self.call("python", "update", stream["id"], "--status", "waiting", "--add-issue",
                  "https://example.test/issue", "Concurrent")
        result = self.call("rust", "agent", "apply", request=request)
        self.assertEqual(result["save"]["domain"]["outcome"], "changes")
        self.assertEqual(self.call("rust", "agent", "apply", request=request),
                         self.call("python", "agent", "apply", request=request))
        current = self.call("rust", "show", stream["id"])["stream"]
        self.assertEqual(current["goal"], "Generated")
        self.assertEqual(current["status"], "waiting")
        self.assertEqual(len(current["issues"]), 1)

    def test_conflicted_reads_preserve_the_confirmed_cli_schema(self):
        remote = self.base / "remote.git"

        def git(*arguments):
            subprocess.run(["git", *map(str, arguments)], env=self.env, cwd=self.cwd,
                           check=True, capture_output=True, timeout=30)

        git("-c", "init.templateDir=", "init", "--bare", "--initial-branch=main", remote)
        self.call("rust", "config", "set", "catalog.root", str(self.base / "git-a"))
        self.call("rust", "config", "set", "sync.stream_seconds", "0")
        self.call("rust", "init", "--sync", "git", "--remote", str(remote))
        stream = self.call("rust", "create", "Shared")['stream']
        identity = stream['id']
        self.call("rust", "sync")
        original = self.env
        peer_home = self.base / "peer-home"
        peer_home.mkdir()
        peer_env = {**original, "HOME": str(peer_home),
                    "XDG_CONFIG_HOME": str(peer_home / "config"),
                    "XDG_DATA_HOME": str(peer_home / "data"),
                    "XDG_STATE_HOME": str(peer_home / "state")}
        try:
            self.env = peer_env
            self.call("rust", "init", "--sync", "git", "--remote", str(remote))
            self.call("rust", "config", "set", "sync.stream_seconds", "0")
        finally:
            self.env = original
        offline = self.base / "offline.git"
        remote.rename(offline)
        try:
            self.call("rust", "update", identity, "--goal", "Local intent")
        finally:
            offline.rename(remote)
        try:
            self.env = peer_env
            self.call("rust", "update", identity, "--goal", "Remote intent")
            self.call("rust", "sync")
        finally:
            self.env = original
        rust = self.call("rust", "list", "--refresh")
        python = self.call("python", "list", "--refresh")
        self.assertTrue(rust['sharing']['conflicts'])
        self.assertEqual(rust['streams'], python['streams'])
        self.assertEqual(rust['sharing']['confirmed'], python['sharing']['confirmed'])
        self.assertEqual(set(rust['sharing']['confirmed']), {'streams', 'sessions'})
        self.assertEqual(rust['sharing']['confirmed']['streams'][0]['goal'], 'Remote intent')

    def test_captured_requests_retry_across_languages_and_keep_stale_supporting_inputs(self):
        stream = self.call("rust", "create", "Work")["stream"]
        identity = stream["id"]
        captured = self.context("python", "unknown", identity)
        self.assertEqual(captured, self.context("rust", "unknown", identity))
        request = {"operation_id": "shared-intent", "catalog_source": captured["catalog_source"],
                   "kind": "stream", "actor": "user", "before": stream,
                   "changes": {"goal": "Captured"}, "inputs": []}
        self.call("rust", "agent", "apply", request=request)
        self.assertEqual(self.call("python", "agent", "apply", request=request),
                         self.call("rust", "agent", "apply", request=request))
        session = self.call("python", "session", "attach", identity, "--harness", "pi",
                            "--session-id", "opaque-native", "--title", "Contribution")["session"]
        captured = self.context("python", "opaque-native")
        self.assertEqual(captured, self.context("rust", "opaque-native"))
        self.call("rust", "update", identity, "--goal", "New manual intent")
        stale = {"operation_id": "stale-generated-output", "catalog_source": captured["catalog_source"],
                 "kind": "session", "actor": "automatic", "ownership_token": captured["authority"]["token"],
                 "before": session, "changes": {"state": "Generated against old goal"},
                 "inputs": [{"kind": "stream", "record": captured["stream"]}]}
        rust_code, rust = self.invoke("rust", "agent", "apply", request=stale)
        python_code, python = self.invoke("python", "agent", "apply", request=stale)
        self.assertEqual((rust_code, rust), (python_code, python))
        self.assertEqual(rust_code, 2)
        self.assertEqual(rust["save"]["domain"]["outcome"], "superseded")

    def test_current_document_locations_share_checkout_and_directory_identity(self):
        checkout = self.base / "checkout"
        checkout.mkdir()
        subprocess.run(["git", "-c", "init.templateDir=", "init", "--quiet", str(checkout)],
                       env=self.env, cwd=self.cwd, check=True, capture_output=True, timeout=30)
        document = checkout / "docs" / "design.md"
        document.parent.mkdir()
        document.write_text("Current worktree contents\n", encoding="utf-8")
        repository = "https://example.test/repo"
        registered = self.call("rust", "checkout", "register", str(checkout), "--repository", repository)["checkout"]
        self.assertEqual(self.call("python", "checkout", "show", registered["id"])["checkout"], registered)
        stream = self.call("rust", "create", "Documents", "--add-repository", "repo", repository,
                           "--add-document", "repo", "not-a-real-revision", "docs/design.md", "Design")["stream"]
        session = self.call("python", "session", "attach", stream["id"], "--harness", "pi",
                            "--session-id", "document-native", "--title", "Writing", "--cwd", str(checkout),
                            "--repository", "repo", "--ref", "also-not-a-real-revision")["session"]
        rust = self.context("rust", "document-native")
        self.assertEqual(rust, self.context("python", "document-native"))
        self.assertEqual(rust["document_locations"][0]["file"], str(document))
        self.assertEqual(rust["document_locations"][0]["checkout_id"], registered["id"])
        self.assertEqual(self.call("rust", "session", "directory", session["id"]),
                         self.call("python", "session", "directory", session["id"]))
        document.unlink()
        for implementation in ("rust", "python"):
            location = self.context(implementation, "document-native")["document_locations"][0]
            self.assertIsNone(location["file"])
            self.assertTrue(location["error"])
            self.assertEqual(location["checkout_id"], registered["id"])


if __name__ == "__main__":
    unittest.main()
