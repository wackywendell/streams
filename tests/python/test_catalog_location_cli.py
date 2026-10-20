from __future__ import annotations

from contextlib import chdir
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))
from _streams.cli import main, runtime
from _streams.catalog.operations import Authority
from _streams.errors import StreamError
from _streams.herdr import HerdrPane, HerdrWorkspace
from _streams.herdr import HerdrEndpoint


class CatalogLocationCliTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.enterContext(chdir(self.root))
        self.enterContext(patch.dict(os.environ, {
            "HOME": str(self.root), "PATH": os.environ.get("PATH", os.defpath),
            "XDG_CONFIG_HOME": str(self.root / "config"), "XDG_DATA_HOME": str(self.root / "data"),
            "XDG_STATE_HOME": str(self.root / "state"),
        }, clear=True))
        self.call("init", "--sync", "none")
        self.call("config", "set", "frontend.mode", "none")

    def call(self, *args, code=0):
        output, error = io.StringIO(), io.StringIO()
        with patch("sys.stdout", output), patch("sys.stderr", error):
            actual = main([*args, "--json"])
        self.assertEqual((actual, error.getvalue()), (code, ""), output.getvalue() + error.getvalue())
        return json.loads(output.getvalue())

    def repo(self, name):
        directory = self.root / name
        subprocess.run(["git", "init", "--quiet", str(directory)], check=True, capture_output=True, timeout=30)
        (directory / "doc.md").write_text(name)
        return directory

    def checkout(self, directory, url="repo"):
        return self.call("checkout", "register", str(directory), "--repository", url)["checkout"]

    def effort(self, directory, *, with_checkout=True):
        stream = self.call("create", "Documents", "--cwd", str(directory), "--add-repository", "code", "repo",
                           "--add-document", "code", "old-recovery-ref", "doc.md", "Design")["stream"]
        args = ["session", "attach", stream["id"], "--harness", "pi", "--session-id", "native", "--title", "Work", "--cwd", str(directory)]
        if with_checkout:
            args.extend(["--repository", "code", "--ref", "current-bookmark"])
        session = self.call(*args)["session"]
        return stream, session

    def files(self):
        return {str(path.relative_to(self.root)): path.read_bytes() for path in self.root.rglob("*") if path.is_file()}

    def test_listing_and_show_do_not_change_registry_or_catalog(self):
        before = self.files()
        self.assertEqual(self.call("checkout", "list"), {"checkouts": []})
        self.assertEqual(self.files(), before)
        checkout = self.checkout(self.repo("code"))
        before = self.files()
        self.assertEqual(self.call("checkout", "show", checkout["id"])["checkout"], checkout)
        self.assertEqual(self.call("checkout", "list")["checkouts"], [checkout])
        self.assertEqual(self.files(), before)

    def test_defaults_are_explicit_and_resolution_reports_ambiguity(self):
        first, second = self.checkout(self.repo("one")), self.checkout(self.repo("two"))
        result = self.call("checkout", "resolve", "repo", code=2)
        self.assertEqual(result["error"]["kind"], "ambiguous")
        self.assertEqual(set(result["error"]["candidates"]), {first["id"], second["id"]})
        self.call("checkout", "default", second["id"], "repo")
        self.assertEqual(self.call("checkout", "resolve", "repo")["checkout"], second)
        self.call("checkout", "default", first["id"], "other-repo", code=2)
        self.assertEqual(self.call("checkout", "resolve", "repo")["checkout"], second)

    def test_context_uses_own_session_checkout_not_default_or_recorded_ref(self):
        first, second = self.repo("one"), self.repo("two")
        stream, session = self.effort(first)
        alternate = self.checkout(second)
        self.call("checkout", "default", alternate["id"], "repo")
        directory = self.call("session", "directory", session["id"])["directory"]
        self.assertEqual(directory["cwd"], str(first))
        context = self.call("agent", "context", "--harness", "pi", "--session-id", "native")
        self.assertEqual(context["document_locations"][0]["file"], str(first / "doc.md"))
        selected = self.call("session", "directory", session["id"], "--checkout", alternate["id"])["directory"]
        self.assertEqual(selected["cwd"], str(second))
        context = self.call("agent", "context", "--harness", "pi", "--session-id", "native")
        self.assertEqual(context["document_locations"][0]["file"], str(second / "doc.md"))
        self.assertEqual(context["stream"]["documents"], stream["documents"])

    def test_move_keeps_checkout_id_and_session_relative_directory(self):
        directory = self.repo("code")
        (directory / "src").mkdir()
        _, session = self.effort(directory)
        selected = self.call("session", "directory", session["id"])["directory"]
        self.call("session", "directory", session["id"], "--checkout", selected["checkout_id"], "--within", "src")
        moved = self.root / "renamed"
        directory.rename(moved)
        self.call("checkout", "show", selected["checkout_id"], code=2)
        self.call("checkout", "move", selected["checkout_id"], str(moved))
        current = self.call("session", "directory", session["id"])["directory"]
        self.assertEqual(current["checkout_id"], selected["checkout_id"])
        self.assertEqual(current["cwd"], str(moved / "src"))

    def test_missing_document_is_visible_and_read_does_not_remove_or_write(self):
        directory = self.repo("code")
        stream, _ = self.effort(directory)
        (directory / "doc.md").unlink()
        before = self.files()
        context = self.call("agent", "context", "--harness", "pi", "--session-id", "native")
        self.assertIsNone(context["document_locations"][0]["file"])
        self.assertIn("Document not present in this checkout", context["document_locations"][0]["error"])
        self.assertEqual(context["stream"]["documents"], stream["documents"])
        self.assertEqual(self.files(), before)

    def test_invalid_session_selection_does_not_change_remembered_directory(self):
        directory = self.repo("code")
        _, session = self.effort(directory)
        before = self.files()
        self.call("session", "directory", session["id"], "--cwd", str(directory), "--repository", "unknown", code=2)
        self.call("session", "directory", session["id"], "--within", "src", code=2)
        self.call("session", "directory", session["id"], "--repository", "code", code=2)
        self.assertEqual(self.files(), before)

    def test_attestation_requires_ready_native_binding_and_is_read_only(self):
        directory = self.repo("code")
        stream, session = self.effort(directory)
        before = self.files()
        self.call("agent", "context", "--harness", "pi", "--session-id", "native", "--attest", code=2)
        self.assertEqual(self.files(), before)
        local = runtime()
        from _streams.catalog_config import open_catalog
        catalog = open_catalog()
        authority = catalog.authority(stream["id"])
        saved_stream = catalog.snapshot().catalog.stream(stream["id"])
        saved_session = next(item for item in catalog.snapshot().catalog.sessions if item.id == session["id"])
        with patch("_streams.catalog_runtime.HerdrClient") as transport, patch.dict(os.environ, {
            "HERDR_ENV": "1", "HERDR_PANE_ID": "w1:p1", "HERDR_TAB_ID": "w1:t1", "HERDR_WORKSPACE_ID": "w1",
        }):
            client = transport.return_value
            client.endpoint.return_value = HerdrEndpoint("/test.sock", None)
            client.current_pane.return_value = HerdrPane("w1:p1", "w1:t1", "w1", "")
            client.resolve_workspace.return_value = HerdrWorkspace("w1", "Work")
            client.command_result.return_value = {"agent": {"agent": "pi", "pane_id": "w1:p1", "workspace_id": "w1",
                "agent_session": {"agent": "pi", "kind": "id", "value": "native"}}}
            with local.enrollment(frontend="herdr") as enrollment:
                enrollment.bind(saved_stream, authority=authority)
            self.call("config", "set", "frontend.mode", "herdr")
            before = self.files()
            context = self.call("agent", "context", "--harness", "pi", "--session-id", "native", "--attest")
            self.assertEqual(context["native"], {"frontend": "herdr", "pane_id": "w1:p1", "tab_id": "w1:t1", "workspace_id": "w1"})
            self.assertEqual(self.files(), before)
            with self.assertRaisesRegex(StreamError, "authority"):
                local.attest_context(saved_stream, saved_session, authority=Authority("another-host", "other-token"))
            self.assertEqual(self.files(), before)


if __name__ == "__main__":
    unittest.main()
