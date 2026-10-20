from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))
import _streams.cli as cli_module
from _streams.catalog.sync import Result
import test_catalog_cli as fixtures


class GitCliTest(unittest.TestCase):
    setUp = fixtures.CatalogCliTest.setUp
    cli = fixtures.CatalogCliTest.cli
    create = fixtures.CatalogCliTest.create

    def git(self, path, *args):
        result = subprocess.run(["git", "-C", str(path), *args], text=True, capture_output=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def setup_git(self):
        config = self.root / "gitconfig"
        config.write_text("[user]\nname = Test\nemail = test@example.org\n[commit]\ngpgSign = false\n")
        os.environ.update(GIT_CONFIG_GLOBAL=str(config), GIT_CONFIG_NOSYSTEM="1")
        self.remote = self.root / "remote.git"
        self.git(self.root, "init", "--bare", str(self.remote))
        self.host("a")
        self.init_git()

    def host(self, name):
        os.environ.update(XDG_CONFIG_HOME=str(self.root / name / "config"), XDG_DATA_HOME=str(self.root / name / "data"), XDG_STATE_HOME=str(self.root / name / "state"))

    def init_git(self):
        code, out, err = self.cli("init", "--sync", "git", "--remote", str(self.remote), "--json")
        self.assertEqual((code, err), (0, ""), out + err)

    def catalog_path(self):
        return Path(os.environ["XDG_DATA_HOME"]) / "streams/catalog"

    def test_command_checks_sync_once_and_no_change_is_still_an_event(self):
        self.assertEqual(self.cli("init", "--sync", "none")[0], 0)
        with patch("_streams.cli.maybe_sync", wraps=cli_module.maybe_sync) as maybe:
            stream = self.create()
        self.assertEqual(maybe.call_count, 1)
        self.assertEqual(maybe.call_args.args[1], "stream")
        with patch("_streams.cli.maybe_sync", wraps=cli_module.maybe_sync) as maybe:
            self.assertEqual(self.cli("update", stream["id"], "--title", stream["title"])[0], 0)
        self.assertEqual(maybe.call_count, 1)
        with patch("_streams.cli.maybe_sync", wraps=cli_module.maybe_sync) as maybe:
            self.assertEqual(self.cli("update", stream["id"], "--goal", " ")[0], 2)
        maybe.assert_not_called()

    def test_lifecycle_sync_events_are_forwarded_and_local_only_never_uses_git(self):
        self.assertEqual(self.cli("init", "--sync", "none")[0], 0)
        with patch("subprocess.run", side_effect=AssertionError("local-only invoked Git")):
            for event in ("explicit", "start", "shutdown"):
                with self.subTest(event=event), patch("_streams.cli.maybe_sync", wraps=cli_module.maybe_sync) as maybe:
                    code, out, err = self.cli("sync", "--event", event, "--json")
                    self.assertEqual((code, err), (0, ""))
                    self.assertEqual(json.loads(out)["sharing"], "local-only")
                    self.assertEqual(maybe.call_args.args[1], event)
        self.assertEqual(self.cli("sync", "--event", "unknown")[0], 2)

    def test_explicit_git_init_create_and_force_sync_publish(self):
        self.setup_git()
        stream = self.create("Git catalog")
        self.assertEqual(self.cli("update", stream["id"], "--status", "parked")[0], 0)
        code, out, err = self.cli("sync", "--json")
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(json.loads(out)["sharing"], "confirmed")
        self.assertEqual(self.git(self.catalog_path(), "rev-parse", "HEAD"), self.git(self.remote, "rev-parse", "main"))

    def test_read_refresh_merges_view_without_pushing_or_using_it_as_write_base(self):
        self.setup_git()
        stream = self.create("Original")
        self.host("b")
        self.init_git()
        self.assertEqual(self.cli("update", stream["id"], "--title", "Remote title")[0], 0)
        self.assertEqual(self.cli("sync")[0], 0)
        remote_tip = self.git(self.remote, "rev-parse", "main")
        self.host("a")
        self.assertEqual(self.cli("update", stream["id"], "--status", "parked")[0], 0)
        local_tip = self.git(self.catalog_path(), "rev-parse", "HEAD")
        code, out, err = self.cli("show", stream["id"], "--refresh", "--json")
        self.assertEqual((code, err), (0, ""), out + err)
        result = json.loads(out)
        self.assertEqual((result["stream"]["title"], result["stream"]["status"]), ("Remote title", "parked"))
        self.assertEqual(result["sharing"]["view"], "projected")
        self.assertEqual(result["sharing"]["freshness"], "refreshed")
        self.assertEqual(self.git(self.catalog_path(), "rev-parse", "HEAD"), local_tip)
        self.assertEqual(self.git(self.remote, "rev-parse", "main"), remote_tip)
        # Mutation captures the actual saved base, not the projected read view.
        self.assertEqual(self.cli("update", stream["id"], "--goal", "Local goal")[0], 0)
        self.assertEqual(self.cli("sync")[0], 0)
        result = json.loads(self.cli("show", stream["id"], "--json")[1])
        self.assertEqual((result["stream"]["title"], result["stream"]["goal"]), ("Remote title", "Local goal"))

    def test_conflicted_read_exposes_confirmed_alternative_and_plain_list_warns(self):
        self.setup_git()
        stream = self.create()
        self.host("b")
        self.init_git()
        self.host("a")
        self.assertEqual(self.cli("update", stream["id"], "--status", "parked")[0], 0)
        self.host("b")
        self.assertEqual(self.cli("update", stream["id"], "--status", "done")[0], 0)
        self.assertEqual(self.cli("sync")[0], 0)
        self.host("a")
        code, out, err = self.cli("show", stream["id"], "--refresh", "--json")
        self.assertEqual((code, err), (0, ""))
        view = json.loads(out)
        self.assertEqual(view["stream"]["status"], "parked")
        self.assertTrue(view["sharing"]["conflicts"])
        self.assertEqual(view["sharing"]["confirmed"]["streams"][0]["status"], "done")
        code, _, err = self.cli("list")
        self.assertEqual(code, 0)
        self.assertIn("conflict", err)
        self.assertEqual(self.cli("sync")[0], 2)

    def test_remote_only_target_requires_explicit_sync_before_editing(self):
        self.setup_git()
        self.create("Initial")
        self.host("b")
        self.init_git()
        incoming = self.create("Remote-only")
        self.assertEqual(self.cli("sync")[0], 0)
        self.host("a")
        self.assertEqual(self.cli("show", incoming["id"], "--refresh")[0], 0)
        code, _, err = self.cli("update", incoming["id"], "--status", "parked")
        self.assertEqual(code, 2)
        self.assertIn("streams sync", err)
        self.assertEqual(self.cli("sync")[0], 0)
        self.assertEqual(self.cli("update", incoming["id"], "--status", "parked")[0], 0)

    def test_local_success_is_not_reported_as_failed_when_publication_fails(self):
        self.setup_git()
        backend = cli_module.config.open_catalog()
        with patch("_streams.cli.config.open_catalog", return_value=backend), patch.object(backend, "sync", return_value=Result("failed", detail="offline")):
            code, out, err = self.cli("create", "Offline edit", "--json")
        self.assertEqual((code, err), (0, ""))
        receipt = json.loads(out)
        self.assertEqual(receipt["save"]["local"], "committed")
        self.assertEqual(receipt["publication"]["sharing"], "failed")
        self.assertEqual(self.cli("show", receipt["stream"]["id"])[0], 0)
        self.assertEqual(self.cli("sync")[0], 0)


if __name__ == "__main__":
    unittest.main()
