from __future__ import annotations

import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

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
        self.env = patch.dict(os.environ, {"HOME": str(self.root), "XDG_CONFIG_HOME": str(self.root / "config")}, clear=True)
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
        self.assertEqual(json.loads(out), {"notes": {"root": str(self.root / "streams")}, "obsidian": {"vault": "streams"}, "frontend": {"mode": "auto"}})
        self.assertFalse(self.path.exists())
        code, out, _ = self.cli("config")
        self.assertEqual(code, 0)
        self.assertIn(str(self.path), out)
        self.assertIn('frontend.mode: "auto" (default)', out)

    def test_set_get_typed_values_and_preserve_other_keys(self):
        for key, value in [("frontend.mode", "herdr"), ("notes.root", "~/Notes"), ("obsidian.vault", "My notes")]:
            self.assertEqual(self.cli("config", "set", key, value)[0], 0)
        self.assertEqual(self.cli("config", "get", "frontend.mode"), (0, "herdr\n", ""))
        self.assertEqual(self.cli("config", "get", "obsidian.vault"), (0, "My notes\n", ""))
        config = json.loads(self.cli("config", "--json")[1])
        self.assertEqual(config["notes"]["root"], str(self.root / "Notes"))
        self.assertEqual(config["frontend"]["mode"], "herdr")
        self.assertNotIn("cmux", config)
        self.assertIn('frontend.mode: "herdr" (configured)', self.cli("config")[1])
        self.assertEqual(self.cli("config", "set", "frontend.mode", "none")[0], 0)
        self.assertEqual(self.cli("config", "get", "frontend.mode")[1], "none\n")

    def test_reject_invalid_values_and_extension_keys_without_writing(self):
        for key, value in [("frontend.mode", "yes"), ("frontend.mode", "1"), ("cmux.enabled", "true"), ("notes.root", "relative"), ("obsidian.vault", " "), ("focusShortcut", "alt+p"), ("selectorModel", "foo")]:
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
                self.assertEqual(self.cli("config", "set", "frontend.mode", "herdr")[0], 2)
                self.assertEqual(self.path.read_text(), text)

    def test_legacy_selection_is_read_only_and_only_canonical_config_is_returned(self):
        self.path.parent.mkdir(parents=True)
        for enabled, expected in ((True, "cmux"), (False, "none")):
            with self.subTest(enabled=enabled):
                self.path.write_text(json.dumps({"cmux": {"enabled": enabled}}))
                original = self.path.read_bytes()
                code, out, err = self.cli("config", "--json")
                self.assertEqual((code, err), (0, ""))
                self.assertEqual(json.loads(out)["frontend"], {"mode": expected})
                self.assertNotIn("cmux", json.loads(out))
                self.assertIn("legacy cmux.enabled", self.cli("config")[1])
                self.assertEqual(self.path.read_bytes(), original)
                self.assertEqual(self.cli("config", "get", "cmux.enabled")[0], 2)

    def test_canonical_set_atomically_removes_legacy_and_preserves_other_overrides(self):
        self.path.parent.mkdir(parents=True)
        for enabled in (True, False):
            for mode in ("auto", "cmux", "herdr", "none"):
                with self.subTest(enabled=enabled, mode=mode):
                    self.path.write_text(json.dumps({"cmux": {"enabled": enabled}, "obsidian": {"vault": "Work notes"}}))
                    self.assertEqual(self.cli("config", "set", "frontend.mode", mode)[0], 0)
                    self.assertEqual(json.loads(self.path.read_text()), {"frontend": {"mode": mode}, "obsidian": {"vault": "Work notes"}})
                    self.assertEqual(list(self.path.parent.iterdir()), [self.path])

    def test_conflicting_stored_selectors_fail_closed_without_overwrite(self):
        self.path.parent.mkdir(parents=True)
        for enabled, matching in ((True, "cmux"), (False, "none")):
            for mode in ("auto", "cmux", "herdr", "none"):
                with self.subTest(enabled=enabled, mode=mode):
                    self.path.write_text(json.dumps({"frontend": {"mode": mode}, "cmux": {"enabled": enabled}}))
                    original = self.path.read_bytes()
                    code, out, err = self.cli("config", "--json")
                    if mode == matching:
                        self.assertEqual(code, 0)
                        self.assertNotIn("cmux", json.loads(out))
                        self.assertEqual(self.cli("config", "set", "frontend.mode", mode)[0], 0)
                        self.assertNotIn("cmux", json.loads(self.path.read_text()))
                    else:
                        self.assertEqual((code, err), (2, ""))
                        self.assertIn("conflict", json.loads(out)["error"]["message"])
                        self.assertEqual(self.cli("config", "set", "frontend.mode", "auto")[0], 2)
                        self.assertEqual(self.path.read_bytes(), original)

    def test_stored_modes_are_strict_and_setter_never_writes_invalid_values(self):
        self.path.parent.mkdir(parents=True)
        for mode in (None, True, 1, [], "", "AUTO", "unknown"):
            with self.subTest(mode=mode):
                self.path.write_text(json.dumps({"frontend": {"mode": mode}}))
                original = self.path.read_bytes()
                self.assertEqual(self.cli("config", "--json")[0], 2)
                self.assertEqual(self.cli("config", "set", "frontend.mode", "herdr")[0], 2)
                self.assertEqual(self.path.read_bytes(), original)

    def test_failed_legacy_migration_preserves_legacy_file_and_symlink(self):
        target = self.root / "managed.json"
        original = '{"cmux":{"enabled":true},"obsidian":{"vault":"Work notes"}}\n'
        target.write_text(original)
        self.path.parent.mkdir(parents=True)
        self.path.symlink_to(target)
        with patch("os.replace", side_effect=OSError("disk failure")):
            self.assertEqual(self.cli("config", "set", "frontend.mode", "auto")[0], 2)
        self.assertTrue(self.path.is_symlink())
        self.assertEqual(target.read_text(), original)
        self.assertEqual(list(self.path.parent.iterdir()), [self.path])

    def test_canonical_mode_drives_fresh_resolver_query(self):
        for mode, expected in (("auto", "herdr"), ("cmux", "cmux"), ("herdr", "herdr"), ("none", None)):
            with self.subTest(mode=mode):
                self.assertEqual(self.cli("config", "set", "frontend.mode", mode)[0], 0)
                with patch.dict(os.environ, {"HERDR_ENV": "1", "HERDR_PANE_ID": "w1:p1"}):
                    code, out, err = self.cli("frontend", "get", "-f", "auto", "--json")
                self.assertEqual((code, err), (0, ""))
                self.assertEqual(json.loads(out), {"configured": mode, "requested": "auto", "resolved": expected})

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
        self.assertIn("vault=My+notes&file=scratch%2Fexample.md", obsidian_link_for_stream(Stream.parse(path, fields, "")))
        # Changing configuration in the same process must not leave frozen defaults.
        self.assertEqual(self.cli("config", "set", "notes.root", str(self.root / "Other"))[0], 0)
        self.assertEqual(list_streams(), [])

    def test_set_preserves_stow_symlink_and_unset_xdg_uses_home(self):
        target = self.root / "dotfiles/config.json"
        target.parent.mkdir()
        target.write_text('{}\n')
        self.path.parent.mkdir(parents=True)
        self.path.symlink_to(target)
        self.assertEqual(self.cli("config", "set", "frontend.mode", "herdr")[0], 0)
        self.assertTrue(self.path.is_symlink())
        self.assertEqual(json.loads(target.read_text())["frontend"]["mode"], "herdr")
        with patch.dict(os.environ, {"XDG_CONFIG_HOME": ""}):
            self.assertIn(str(self.root / ".config/streams/config.json"), self.cli("config")[1])
            self.assertEqual(self.cli("config", "get", "frontend.mode")[1], "auto\n")

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

    def test_note_commands_do_not_depend_on_optional_frontend_policy(self):
        notes = self.root / "Notes"
        self.assertEqual(self.cli("config", "set", "notes.root", str(notes))[0], 0)
        with patch("_streams.store.discover_stream_location", return_value=StreamLocation("jj", self.root, self.root)):
            code, out, _ = self.cli("start", "Example", "--json")
        self.assertEqual(code, 0)
        path = Path(json.loads(out)["path"])
        for policy in ({"frontend": {"mode": "unknown"}}, {"frontend": None}, {"cmux": {"enabled": "true"}},
                       {"frontend": {"mode": "herdr"}, "cmux": {"enabled": True}}):
            with self.subTest(policy=policy):
                original = path.read_text()
                proposed = original.replace("state: New stream ready for scoping.", "state: Saved despite invalid policy.")
                self.path.write_text(json.dumps({"notes": {"root": str(notes)}, **policy}))
                with patch("sys.stdout", io.TextIOWrapper(io.BytesIO())):
                    self.assertEqual(main(["read", "example", "--hash"]), 0)
                with patch("sys.stdin", io.StringIO(proposed)):
                    self.assertEqual(self.cli("update", "example", "--json")[0], 0)
                self.assertEqual(path.read_text(), proposed)
                self.assertEqual(self.cli("frontend", "get", "--json")[0], 2)
                self.assertEqual(self.cli("config", "--json")[0], 2)
                path.write_text(original)

    def test_note_root_validation_still_fails_closed(self):
        self.path.parent.mkdir(parents=True)
        for config in ("{", "[]", '{"notes":null}', '{"notes":{"root":"relative"}}', '{"notes":{"unknown":"value"}}'):
            with self.subTest(config=config):
                self.path.write_text(config)
                self.assertEqual(self.cli("list", "--json")[0], 2)

    def test_failed_atomic_write_preserves_existing_configuration(self):
        self.assertEqual(self.cli("config", "set", "frontend.mode", "herdr")[0], 0)
        original = self.path.read_bytes()
        with patch("os.replace", side_effect=OSError("disk failure")):
            self.assertEqual(self.cli("config", "set", "frontend.mode", "none")[0], 2)
        self.assertEqual(self.path.read_bytes(), original)
        self.assertEqual(list(self.path.parent.iterdir()), [self.path])
