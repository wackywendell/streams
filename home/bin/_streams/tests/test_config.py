from __future__ import annotations

import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from _streams.cli import main
from _streams.cmux import obsidian_link_for_stream
from _streams.models import Stream
from _streams.location import StreamLocation
from _streams.store import check_paths, list_streams, show_stream
from _streams.resolve import resolve_check_path_ref, resolve_stream_ref
from _streams.frontmatter import render_frontmatter


class ConfigTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.env = patch.dict(os.environ, {"HOME": str(self.root), "XDG_CONFIG_HOME": str(self.root / "config")})
        self.env.start()
        self.addCleanup(self.env.stop)
        self.path = self.root / "config/streams/config.json"

    def cli(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with patch("sys.stdout", out), patch("sys.stderr", err):
            code = main(list(args))
        return code, out.getvalue(), err.getvalue()

    def test_defaults_and_origins_without_creating_config(self):
        code, out, err = self.cli("config", "--json")
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(json.loads(out), {"notes": {"root": str(self.root / "worknotes")}, "obsidian": {"vault": "worknotes"}, "cmux": {"enabled": False}})
        self.assertFalse(self.path.exists())
        code, out, _ = self.cli("config")
        self.assertEqual(code, 0)
        self.assertIn(str(self.path), out)
        self.assertIn("cmux.enabled: false (default)", out)

    def test_set_get_typed_values_and_preserve_other_keys(self):
        for key, value in [("cmux.enabled", "true"), ("notes.root", "~/Notes"), ("obsidian.vault", "My notes")]:
            self.assertEqual(self.cli("config", "set", key, value)[0], 0)
        self.assertEqual(self.cli("config", "get", "cmux.enabled"), (0, "true\n", ""))
        self.assertEqual(self.cli("config", "get", "obsidian.vault"), (0, "My notes\n", ""))
        config = json.loads(self.cli("config", "--json")[1])
        self.assertEqual(config["notes"]["root"], str(self.root / "Notes"))
        self.assertTrue(config["cmux"]["enabled"])
        self.assertIn("cmux.enabled: true (configured)", self.cli("config")[1])
        self.assertEqual(self.cli("config", "set", "cmux.enabled", "false")[0], 0)
        self.assertEqual(self.cli("config", "get", "cmux.enabled")[1], "false\n")

    def test_reject_invalid_values_and_extension_keys_without_writing(self):
        for key, value in [("cmux.enabled", "yes"), ("cmux.enabled", "1"), ("notes.root", "relative"), ("obsidian.vault", " "), ("focusShortcut", "alt+p"), ("selectorModel", "foo")]:
            with self.subTest(key=key, value=value):
                code, out, err = self.cli("config", "set", key, value)
                self.assertEqual(code, 2)
                self.assertEqual(out, "")
                self.assertTrue(err.startswith("streams:"))
                self.assertFalse(self.path.exists())
        self.assertEqual(self.cli("config", "get", "unknown")[0], 2)

    def test_malformed_config_fails_closed_and_is_not_overwritten(self):
        self.path.parent.mkdir(parents=True)
        for text in ['{', '[]', '{"cmux":{"enabled":"true"}}', '{"notes":null}', '{"hotkeys":{}}']:
            with self.subTest(text=text):
                self.path.write_text(text)
                self.assertEqual(self.cli("config", "--json")[0], 2)
                self.assertEqual(self.cli("config", "set", "cmux.enabled", "true")[0], 2)
                self.assertEqual(self.path.read_text(), text)

    def test_runtime_paths_and_obsidian_links_follow_config(self):
        notes = self.root / "Notes"
        scratch = notes / "scratch"
        scratch.mkdir(parents=True)
        path = scratch / "example.md"
        fields = {"title": "Example", "status": "active", "description": "Example task", "state": "Working", "repo": str(self.root), "cwd": str(self.root), "vcs": "jj", "issues": [], "prs": [], "sessions": []}
        path.write_text(render_frontmatter(fields, "# Example\n"))
        self.assertEqual(self.cli("config", "set", "notes.root", str(notes))[0], 0)
        self.assertEqual(self.cli("config", "set", "obsidian.vault", "My notes")[0], 0)
        self.assertEqual([stream.path for stream in list_streams()], [path])
        self.assertEqual(check_paths(), [path])
        self.assertEqual(show_stream("example").path, path)
        self.assertEqual(resolve_stream_ref("Example").path, path)
        self.assertEqual(resolve_check_path_ref("Example"), path)
        self.assertIn("vault=My+notes&file=scratch%2Fexample.md", obsidian_link_for_stream(Stream(path, fields, "")))
        # Changing configuration in the same process must not leave frozen defaults.
        self.assertEqual(self.cli("config", "set", "notes.root", str(self.root / "Other"))[0], 0)
        self.assertEqual(list_streams(), [])

    def test_set_preserves_stow_symlink_and_unset_xdg_uses_home(self):
        target = self.root / "dotfiles/config.json"
        target.parent.mkdir()
        target.write_text('{}\n')
        self.path.parent.mkdir(parents=True)
        self.path.symlink_to(target)
        self.assertEqual(self.cli("config", "set", "cmux.enabled", "true")[0], 0)
        self.assertTrue(self.path.is_symlink())
        self.assertTrue(json.loads(target.read_text())["cmux"]["enabled"])
        with patch.dict(os.environ, {"XDG_CONFIG_HOME": ""}):
            self.assertIn(str(self.root / ".config/streams/config.json"), self.cli("config")[1])
            self.assertEqual(self.cli("config", "get", "cmux.enabled")[1], "false\n")

    def test_start_update_and_archive_use_configured_root(self):
        notes = self.root / "Notes"
        self.assertEqual(self.cli("config", "set", "notes.root", str(notes))[0], 0)
        with patch("_streams.store.discover_stream_location", return_value=StreamLocation("jj", self.root, self.root)):
            code, out, err = self.cli("start", "Example", "--json")
        self.assertEqual((code, err), (0, ""))
        path = Path(json.loads(out)["path"])
        self.assertEqual(path, notes / "scratch/example.md")
        original = path.read_text()
        proposed = original.replace("status: active", "status: done")
        with patch("sys.stdin", io.StringIO(proposed)):
            self.assertEqual(self.cli("update", "example", "--json")[0], 0)
        self.assertEqual(path.read_text(), proposed)
        self.assertEqual(self.cli("archive", "example", "--month", "2026-09")[0], 0)
        self.assertEqual((notes / "scratch/archive/2026-09/example.md").read_text(), proposed)
        self.assertFalse(path.exists())

    def test_failed_atomic_write_preserves_existing_configuration(self):
        self.assertEqual(self.cli("config", "set", "cmux.enabled", "true")[0], 0)
        original = self.path.read_bytes()
        with patch("os.replace", side_effect=OSError("disk failure")):
            self.assertEqual(self.cli("config", "set", "cmux.enabled", "false")[0], 2)
        self.assertEqual(self.path.read_bytes(), original)
        self.assertEqual(list(self.path.parent.iterdir()), [self.path])
