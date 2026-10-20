from __future__ import annotations

import io
import json
from contextlib import chdir
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
from _streams.catalog.sync import Result
from _streams.errors import StreamError


class CatalogCliTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.enterContext(chdir(self.root))
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

    def test_checkout_document_materializes_only_an_explicit_safe_revision_request(self):
        self.initialize()
        root = self.root / "checkout"
        subprocess.run(["git", "init", "--quiet", str(root)], check=True, capture_output=True)
        (root / "docs").mkdir()
        (root / "docs/design.md").write_text("recorded revision\n")
        subprocess.run(["git", "add", "docs/design.md"], cwd=root, check=True, capture_output=True)
        subprocess.run(["git", "-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false",
                        "commit", "--quiet", "-m", "document"], cwd=root, check=True, capture_output=True)
        revision = subprocess.run(["git", "rev-parse", "HEAD"], cwd=root, check=True, capture_output=True, text=True).stdout.strip()
        code, out, err = self.cli("checkout", "register", str(root), "--repository", "repo", "--json")
        self.assertEqual((code, err), (0, ""), out)
        checkout_id = json.loads(out)["checkout"]["id"]
        (root / "docs/design.md").write_text("current worktree edits\n")
        code, out, err = self.cli("checkout", "document", checkout_id, "docs/design.md", "--revision", revision, "--json")
        self.assertEqual((code, err), (0, ""), out)
        result = json.loads(out)
        self.assertEqual(result["checkout_id"], checkout_id)
        self.assertEqual(result["revision"], revision)
        self.assertEqual(Path(result["file"]).read_text(), "recorded revision\n")
        self.assertEqual(Path(result["file"]), Path(result["root"]) / "docs/design.md")
        self.assertEqual((root / "docs/design.md").read_text(), "current worktree edits\n")
        outside = self.root / "outside.md"
        outside.write_text("must not overwrite")
        Path(result["file"]).unlink()
        Path(result["file"]).symlink_to(outside)
        code, out, _ = self.cli("checkout", "document", checkout_id, "docs/design.md", "--revision", revision, "--json")
        self.assertEqual(code, 2)
        self.assertIn("unavailable", json.loads(out)["error"]["message"])
        self.assertEqual(outside.read_text(), "must not overwrite")
        for identity, path, rev in (("unknown", "docs/design.md", revision), (checkout_id, "../escape", revision),
                                    (checkout_id, "docs/missing.md", revision), (checkout_id, "docs/design.md", "missing-revision")):
            code, out, err = self.cli("checkout", "document", identity, path, "--revision", rev, "--json")
            self.assertEqual(code, 2)
            self.assertTrue(json.loads(out)["error"])

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
        self.assertEqual(self.cli("update", stream["id"], "--remove-document", "main", "docs/design.md", "--remove-repository", "main")[0], 0)

    def test_document_provenance_and_identity_are_independent_of_recovery_ref(self):
        self.initialize()
        stream = self.create("Documents", "--add-repository", "code", "https://example.org/code",
                             "--add-document", "code", "topic", "docs/design.md", "Design",
                             "--document-commit", "code", "docs/design.md", "commit:known")
        document = stream["documents"][0]
        self.assertEqual(document["commit"], "commit:known")
        self.assertEqual(self.cli("update", stream["id"], "--add-document", "code", "other",
                                 "docs/design.md", "Design")[0], 2)
        self.assertEqual(self.cli("update", stream["id"], "--document-commit", "code",
                                 "missing.md", "commit:known")[0], 2)
        self.assertEqual(self.cli("update", stream["id"], "--remove-document", "code", "docs/design.md",
                                 "--add-document", "code", "other", "docs/design.md", "Updated")[0], 0)
        current = json.loads(self.cli("show", stream["id"], "--json")[1])["stream"]
        self.assertEqual(current["documents"], [{"repository": "code", "ref": "other", "path": "docs/design.md",
                                                "description": "Updated"}])
        self.assertEqual(self.cli("update", stream["id"], "--remove-document", "code", "docs/design.md")[0], 0)
        self.assertEqual(json.loads(self.cli("show", stream["id"], "--json")[1])["stream"]["documents"], [])

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
        with patch("_streams.cli.config.open_catalog", return_value=backend), patch.object(backend, "sync", return_value=Result("local-only")), patch.object(backend, "snapshot", side_effect=StorageError("readback failed")):
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

    def test_check_detects_and_fixes_stale_agent_owner_markers(self):
        self.initialize()
        stream = self.create()
        session = json.loads(self.cli("session", "attach", stream["id"], "--harness", "pi", "--session-id", "01a154ad-a5fc-7180-a631-bd34e8c4c093", "--title", "Core", "--json")[1])["session"]
        transcript = self.root / "session.jsonl"
        transcript.write_text(json.dumps({"type": "session", "version": 3, "id": "01a154ad-a5fc-7180-a631-bd34e8c4c093"}) + "\n")
        from _streams import catalog_config as config
        from _streams.catalog_runtime import Runtime, _key
        from _streams.herdr import HerdrEndpoint
        rt = Runtime(config.runtime_state_dir(), config.host_id(), config.get_frontend_mode(), checkout_dir=config.checkout_state_dir())
        socket = str(self.root / "config/herdr/herdr.sock")
        scope = _key("herdr", socket, "default")
        rt._write(rt._association_path(scope, "wH"), {"stream_id": stream["id"], "workspace_id": "wH", "session_id": None, "command": None, "ready": True})
        pane_tokens = {
            "strm_agent": "stale-digest",
            "strm_card": "term_closed_card",
            "strm_native": "01a13be3-6101-75b6-9a2e-6908f53c89bc",
            "strm_nonce": "nonce-1",
            "strm_pi": "term_1",
            "strm_role": "agent",
        }
        snapshot = {
            "workspaces": [{"workspace_id": "wH", "tokens": {}}],
            "panes": [{
                "pane_id": "wH:p1",
                "terminal_id": "term_1",
                "tab_id": "wH:t1",
                "workspace_id": "wH",
                "agent": "pi",
                "agent_session": {"agent": "pi", "kind": "path", "value": str(transcript)},
                "tokens": pane_tokens,
            }],
        }
        runs = []
        with patch("_streams.catalog_check.HerdrClient.endpoint", return_value=HerdrEndpoint(socket=socket, session="default")), \
             patch("_streams.catalog_check.HerdrClient.command_result", return_value={"type": "session_snapshot", "snapshot": snapshot}), \
             patch("_streams.catalog_check.HerdrClient.run", side_effect=lambda args: runs.append(args)):
            code, out, _ = self.cli("check", "--pane", "wH:p1", "--json")
            self.assertEqual(code, 2)
            report = json.loads(out)
            self.assertFalse(report["valid"])
            self.assertEqual(report["issues"][0]["kind"], "stale-agent-owner-markers")
            self.assertEqual(report["issues"][0]["pane_id"], "wH:p1")
            self.assertEqual(runs, [])

            code, out, _ = self.cli("check", "--pane", "wH:p1", "--fix", "--json")
            self.assertEqual(code, 0)
            fixed = json.loads(out)
            self.assertTrue(fixed["valid"])
            self.assertEqual(fixed["fixed"][0]["kind"], "stale-agent-owner-markers-cleared")
            self.assertEqual(len(runs), 1)
            self.assertEqual(runs[0][:5], ["pane", "report-metadata", "wH:p1", "--source", "streams-work-cards"])

    def test_check_fix_reports_unavailable_unbound_checkout_without_aborting(self):
        self.initialize()
        stream = self.create()
        checkout = self.root / "checkout"
        subprocess.run(["git", "init", "--quiet", str(checkout)], check=True, capture_output=True)
        from _streams.cli import runtime
        local = runtime()
        registered = local.checkouts.register(checkout)
        checkout.rename(self.root / "moved-checkout")
        before = local.checkouts.path.read_bytes()
        code, out, err = self.cli("check", "--fix", "--json")
        self.assertEqual((code, err), (2, ""), out)
        report = json.loads(out)
        self.assertFalse(report["valid"])
        issue = next(issue for issue in report["issues"] if issue["kind"] == "invalid-checkout")
        self.assertEqual(issue["checkout_id"], registered.id)
        self.assertIn("unavailable", issue["message"])
        self.assertEqual(report["streams"], 1)
        self.assertEqual(self.cli("show", stream["id"])[0], 0)
        self.assertEqual(local.checkouts.path.read_bytes(), before)

    def test_scoped_check_reports_pending_and_unstamped_card_without_mutation(self):
        self.initialize()
        stream = self.create()
        native_id = "native"
        session = json.loads(self.cli("session", "attach", stream["id"], "--harness", "pi",
                                     "--session-id", native_id, "--title", "Core", "--json")[1])["session"]
        transcript = self.root / "session.jsonl"
        transcript.write_text(json.dumps({"type": "session", "id": native_id}) + "\n")
        from _streams import catalog_config as config
        from _streams.catalog_agent import source_id
        from _streams.catalog_check import _js_digest
        from _streams.cli import runtime
        from _streams.catalog_runtime import _key
        from _streams.herdr import HerdrEndpoint
        local = runtime()
        socket = str(self.root / "herdr/herdr.sock")
        scope = _key("herdr", socket, "default")
        local._write(local._association_path(scope, "wH"),
                     {"stream_id": stream["id"], "workspace_id": "wH", "session_id": None,
                      "command": None, "ready": True})
        owner = {"pane_id": "wH:p1", "terminal_id": "owner", "tab_id": "wH:t1",
                 "workspace_id": "wH", "agent": "pi", "tokens": {},
                 "agent_session": {"agent": "pi", "kind": "path", "value": str(transcript)}}
        card = {"pane_id": "wH:p2", "terminal_id": "card", "tab_id": "wH:t1",
                "workspace_id": "wH", "tokens": {}}
        scope_obj = {"catalogSource": source_id(config.open_catalog()), "hostId": config.host_id(),
                     "streamId": stream["id"], "sessionId": session["id"], "nativeSessionId": native_id}
        key = _js_digest([socket, scope_obj, ["wH:p1", "owner", "wH", "wH:t1"]])
        state_path = Path(socket).parent / ".streams-work-cards" / (key + ".json")
        state_path.parent.mkdir(parents=True)
        state_path.write_text(json.dumps({"phase": "pending", "key": key, "paneId": "wH:p2",
                                         "terminalId": "card", "tabId": "wH:t1", "nonce": "nonce"}))
        before = {path: path.read_bytes() for path in self.root.rglob("*") if path.is_file()}
        snapshot = {"workspaces": [{"workspace_id": "wH", "tokens": {}}], "panes": [owner, card]}
        with patch("_streams.catalog_check.HerdrClient.endpoint", return_value=HerdrEndpoint(socket, "default")), \
             patch("_streams.catalog_check.HerdrClient.command_result", return_value={"snapshot": snapshot}), \
             patch("_streams.catalog_check.HerdrClient.run", side_effect=AssertionError("native mutation")):
            for args in (("--pane", "wH:p1"), (stream["id"],)):
                for fix in ((), ("--fix",)):
                    with self.subTest(args=args, fix=fix):
                        code, out, err = self.cli("check", *args, *fix, "--json")
                        self.assertEqual((code, err), (2, ""), out)
                        report = json.loads(out)
                        self.assertEqual({issue["kind"] for issue in report["issues"]},
                                         {"pending-work-card-launch", "unstamped-work-card-pair"})
                        self.assertNotIn("fixed", report)
        self.assertEqual({path: path.read_bytes() for path in self.root.rglob("*") if path.is_file()}, before)

    def test_check_fix_binds_unbound_session_checkout_from_origin_remote(self):
        self.initialize()
        repo = self.root / "worktree"
        subprocess.run(["git", "init", "--quiet", str(repo)], check=True, capture_output=True, timeout=30)
        subprocess.run(["git", "-C", str(repo), "remote", "add", "origin", "git@github.com:example/streams.git"], check=True, capture_output=True, timeout=30)
        (repo / "doc.md").write_text("hello\n")
        stream = json.loads(self.cli(
            "create", "RepoTest", "--cwd", str(repo),
            "--add-repository", "streams", "git@github.com:example/streams.git",
            "--add-document", "streams", "main", "doc.md", "Doc", "--json",
        )[1])["stream"]
        session = json.loads(self.cli(
            "session", "attach", stream["id"], "--harness", "pi",
            "--session-id", "01a1556b-8410-75b9-851d-778193659ea0", "--title", "Temper", "--cwd", str(repo), "--json",
        )[1])["session"]
        transcript = self.root / "temper.jsonl"
        transcript.write_text(json.dumps({"type": "session", "version": 3, "id": "01a1556b-8410-75b9-851d-778193659ea0"}) + "\n")
        from _streams import catalog_config as config
        from _streams.catalog_runtime import Runtime, _key
        from _streams.herdr import HerdrEndpoint
        rt = Runtime(config.runtime_state_dir(), config.host_id(), config.get_frontend_mode(), checkout_dir=config.checkout_state_dir())
        socket = str(self.root / "config/herdr/herdr.sock")
        scope = _key("herdr", socket, "default")
        rt._write(rt._association_path(scope, "wH"), {"stream_id": stream["id"], "workspace_id": "wH", "session_id": None, "command": None, "ready": True})
        snapshot = {
            "workspaces": [{"workspace_id": "wH", "tokens": {}}],
            "panes": [{
                "pane_id": "wH:p2E",
                "terminal_id": "term_2",
                "tab_id": "wH:tS",
                "workspace_id": "wH",
                "agent": "pi",
                "agent_session": {"agent": "pi", "kind": "path", "value": str(transcript)},
                "tokens": {},
            }],
        }
        with patch("_streams.catalog_check.HerdrClient.endpoint", return_value=HerdrEndpoint(socket=socket, session="default")), \
             patch("_streams.catalog_check.HerdrClient.command_result", return_value={"type": "session_snapshot", "snapshot": snapshot}):
            code, out, _ = self.cli("check", "--pane", "wH:p2E", "--json")
            self.assertEqual(code, 2)
            self.assertIn("unresolved-session-document", {issue["kind"] for issue in json.loads(out)["issues"]})

            code, out, _ = self.cli("check", "--pane", "wH:p2E", "--fix", "--json")
            self.assertEqual(code, 0)
            fixed = json.loads(out)
            self.assertTrue(fixed["valid"])
            self.assertEqual(fixed["fixed"][0]["kind"], "checkout-repository-bound")
            self.assertEqual(fixed["fixed"][0]["repository_urls"], ["git@github.com:example/streams.git"])


if __name__ == "__main__":
    unittest.main()
