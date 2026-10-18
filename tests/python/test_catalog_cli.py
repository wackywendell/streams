from __future__ import annotations

import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))
from _streams.cli import main
from _streams.errors import StreamError


class CatalogCliTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.env = self.enterContext(patch.dict(os.environ, {
            "HOME": str(self.root), "XDG_CONFIG_HOME": str(self.root / "config"),
            "XDG_DATA_HOME": str(self.root / "data"), "XDG_STATE_HOME": str(self.root / "state"),
        }, clear=True))

    def cli(self, *args, stdin=""):
        out, err = io.StringIO(), io.StringIO()
        with patch("sys.stdout", out), patch("sys.stderr", err), patch("sys.stdin", io.StringIO(stdin)):
            try:
                code = main(list(args))
            except SystemExit as error:
                code = error.code
        return code, out.getvalue(), err.getvalue()

    def initialize(self):
        code, out, err = self.cli("init", "--sync", "none", "--json")
        self.assertEqual((code, err), (0, ""), out + err)
        return json.loads(out)

    def create(self, title="Catalog CLI", *args):
        code, out, err = self.cli("create", title, *args, "--json")
        self.assertEqual((code, err), (0, ""), out + err)
        return json.loads(out)["stream"]

    def test_explicit_init_required_and_scratch_is_never_adopted(self):
        scratch = self.root / "streams/scratch"
        scratch.mkdir(parents=True)
        old = scratch / "legacy.md"
        old.write_text("not a catalog record")
        for args in (("list",), ("create", "New"), ("show", "legacy")):
            code, _, err = self.cli(*args)
            self.assertEqual(code, 2)
            self.assertIn("init", err)
        self.assertFalse((self.root / "data/streams/catalog").exists())
        self.initialize()
        self.assertEqual(json.loads(self.cli("list", "--json")[1])["streams"], [])
        self.assertEqual(old.read_text(), "not a catalog record")

    def test_create_unspecified_goal_and_show_complete_yaml(self):
        self.initialize()
        stream = self.create()
        self.assertIsNone(stream["goal"])
        self.assertIsNone(stream["goal_short"])
        self.assertEqual(stream["title_mode"], "manual")
        code, out, err = self.cli("show", stream["id"])
        self.assertEqual((code, err), (0, ""))
        view = yaml.safe_load(out)
        self.assertEqual(view["stream"], stream)
        self.assertEqual(view["sessions"], [])
        self.assertNotIn("revision", view["stream"])
        self.assertNotIn("path", view["stream"])

    def test_search_is_literal_case_insensitive_substring_with_filters(self):
        self.initialize()
        first = self.create("CLI", "--goal", "Implement Catalog Sync")
        second = self.create("Catalog:active", "--goal", "Other")
        self.cli("update", second["id"], "--status", "blocked")
        def ids(*args):
            code, out, err = self.cli("list", *args, "--json")
            self.assertEqual((code, err), (0, ""))
            return [item["id"] for item in json.loads(out)["streams"]]
        self.assertEqual(ids("CATALOG SYNC"), [first["id"]])
        self.assertEqual(ids("status:active"), [])
        self.assertEqual(ids("catalog", "--status", "blocked"), [second["id"]])
        self.assertEqual(ids("Catalog:active"), [second["id"]])

    def test_ref_precedence_rename_preserves_identity_and_ambiguous_ref_refuses(self):
        self.initialize()
        first = self.create("Catalog")
        second = self.create("Catalog runtime")
        self.assertEqual(json.loads(self.cli("show", "Catalog", "--json")[1])["stream"]["id"], first["id"])
        code, out, _ = self.cli("update", "CAT", "--status", "done", "--json")
        self.assertEqual(code, 2)
        self.assertEqual(json.loads(out)["error"]["kind"], "ambiguous")
        self.assertEqual(self.cli("update", first["id"], "--title", "Renamed")[0], 0)
        self.assertEqual(json.loads(self.cli("show", first["id"], "--json")[1])["stream"]["title"], "Renamed")
        self.assertEqual(json.loads(self.cli("show", "runtime", "--json")[1])["stream"]["id"], second["id"])

    def test_update_goal_files_clear_and_omission_are_distinct(self):
        self.initialize()
        stream = self.create("CLI", "--goal", "Original")
        self.assertEqual(self.cli("update", stream["id"], "--status", "waiting")[0], 0)
        self.assertEqual(json.loads(self.cli("show", stream["id"], "--json")[1])["stream"]["goal"], "Original")
        self.assertEqual(self.cli("update", stream["id"], "--goal-file", "-", stdin="New\nmultiline")[0], 0)
        changed = json.loads(self.cli("show", stream["id"], "--json")[1])["stream"]
        self.assertEqual(changed["goal"], "New\nmultiline")
        self.assertEqual(changed["goal_mode"], "manual")
        self.assertEqual(self.cli("update", stream["id"], "--clear-goal")[0], 0)
        self.assertIsNone(json.loads(self.cli("show", stream["id"], "--json")[1])["stream"]["goal"])
        self.assertNotEqual(self.cli("update", stream["id"], "--goal", " ")[0], 0)

    def test_archive_and_restore_do_not_change_status(self):
        self.initialize()
        stream = self.create()
        self.assertEqual(self.cli("archive", stream["id"])[0], 0)
        self.assertEqual(json.loads(self.cli("list", "--json")[1])["streams"], [])
        archived = json.loads(self.cli("list", "--archived", "--json")[1])["streams"]
        self.assertEqual([(s["id"], s["status"]) for s in archived], [(stream["id"], "active")])
        self.assertEqual(len(json.loads(self.cli("list", "--all", "--json")[1])["streams"]), 1)
        self.assertEqual(self.cli("restore", stream["id"])[0], 0)
        self.assertFalse(json.loads(self.cli("show", stream["id"], "--json")[1])["stream"]["archived"])

    def test_reference_collection_edits_and_validation_are_one_operation(self):
        self.initialize()
        stream = self.create()
        args = ("--add-repository", "main", "https://example.org/repo.git", "--add-document", "main", "topic", "docs/design.md", "Design", "--add-issue", "https://example.org/issue/1", "Issue")
        self.assertEqual(self.cli("update", stream["id"], *args)[0], 0)
        current = json.loads(self.cli("show", stream["id"], "--json")[1])["stream"]
        self.assertEqual(current["documents"][0]["path"], "docs/design.md")
        self.assertEqual(self.cli("update", stream["id"], "--remove-repository", "main")[0], 2)
        unchanged = json.loads(self.cli("show", stream["id"], "--json")[1])["stream"]
        self.assertEqual(unchanged, current)
        self.assertEqual(self.cli("update", stream["id"], "--remove-document", "main", "topic", "docs/design.md", "--remove-repository", "main")[0], 0)

    def test_attach_session_show_and_update_separate_progress(self):
        self.initialize()
        stream = self.create()
        code, out, err = self.cli("session", "attach", stream["id"], "--harness", "pi", "--session-id", "native", "--title", "CLI tests", "--json")
        self.assertEqual((code, err), (0, ""), out + err)
        session = json.loads(out)["session"]
        code, _, err = self.cli("session", "update", session["id"], "--state", "Tests pass", "--remaining-file", "-", stdin="Review\nPublish")
        self.assertEqual((code, err), (0, ""))
        view = json.loads(self.cli("show", stream["id"], "--json")[1])
        self.assertEqual(view["sessions"][0]["remaining"], "Review\nPublish")
        self.assertNotIn("state", view["stream"])
        self.assertEqual(len(json.loads(self.cli("session", "list", "--stream", stream["id"], "--json")[1])["sessions"]), 1)
        self.assertEqual(self.cli("session", "attach", stream["id"], "--harness", "pi", "--session-id", "native", "--title", "Duplicate")[0], 2)

    def test_removed_scratch_commands_and_protected_fields_are_not_exposed(self):
        self.initialize()
        stream = self.create()
        for args in (("read", stream["id"]), ("format",), ("rename", stream["id"], "New"), ("update", stream["id"], "--owner-host-id", "other"), ("archive", stream["id"], "--month", "2026-01")):
            self.assertNotEqual(self.cli(*args)[0], 0)
        self.assertEqual(self.cli("check")[0], 0)
        self.assertEqual(self.cli("sync")[0], 0)

    def test_start_retains_created_stream_when_open_fails(self):
        self.initialize()
        with patch("_streams.catalog_runtime.Runtime.open", side_effect=StreamError("native open failed")):
            code, out, _ = self.cli("start", "New", "--frontend", "herdr", "--json")
        self.assertEqual(code, 2)
        created = json.loads(out)
        self.assertEqual(created["stream"]["title"], "New")
        self.assertIn("native open failed", created["runtime"]["error"]["message"])
        self.assertEqual(self.cli("show", created["stream"]["id"])[0], 0)

    def test_current_and_frontend_are_independent_and_no_cwd_inference(self):
        self.initialize()
        stream = self.create()
        self.assertEqual(self.cli("show")[0], 2)
        self.assertEqual(self.cli("show", "--current")[0], 2)
        with patch.dict(os.environ, {"STREAMS_STREAM_ID": stream["id"]}):
            self.assertEqual(json.loads(self.cli("show", "--current", "--json")[1])["stream"]["id"], stream["id"])
        self.assertEqual(self.cli("show", stream["id"], "--current")[0], 2)

    def test_current_missing_identity_never_falls_back_to_title(self):
        self.initialize()
        self.create("missing-identity")
        with patch.dict(os.environ, {"STREAMS_STREAM_ID": "missing-identity"}):
            code, out, _ = self.cli("show", "--current", "--json")
        self.assertEqual(code, 2)
        self.assertEqual(json.loads(out)["error"]["kind"], "not-found")

    def test_multiple_stdin_fields_refuse_before_saving(self):
        self.initialize()
        stream = self.create()
        attached = json.loads(self.cli("session", "attach", stream["id"], "--harness", "pi", "--session-id", "native", "--title", "Tests", "--json")[1])["session"]
        code, _, _ = self.cli("session", "update", attached["id"], "--state-file", "-", "--remaining-file", "-", stdin="Ambiguous input")
        self.assertEqual(code, 2)
        current = json.loads(self.cli("session", "show", attached["id"], "--json")[1])["session"]
        self.assertEqual((current["state"], current["remaining"]), ("", ""))

    def test_post_save_failure_keeps_the_successful_save_receipt(self):
        self.initialize()
        import _streams.cli as module
        from _streams.catalog.storage import StorageError
        backend = module.config.open_catalog()
        with patch("_streams.cli.config.open_catalog", return_value=backend), patch.object(backend, "sync", return_value=module.Result("local-only")), patch.object(backend, "snapshot", side_effect=StorageError("readback failed")):
            code, out, _ = self.cli("create", "Retained", "--json")
        self.assertEqual(code, 2)
        receipt = json.loads(out)
        self.assertEqual(receipt["save"]["local"], "persisted")
        self.assertIn("readback failed", receipt["completion_error"]["message"])
        self.assertEqual(self.cli("show", receipt["stream"]["id"])[0], 0)

    def test_cli_module_entrypoint_uses_isolated_catalog(self):
        self.initialize()
        stream = self.create()
        env = dict(os.environ, PYTHONPATH=str(Path(__file__).resolve().parents[2] / "src"))
        result = subprocess.run([sys.executable, "-m", "_streams.cli", "show", stream["id"], "--json"], env=env, capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["stream"]["id"], stream["id"])


if __name__ == "__main__":
    unittest.main()
