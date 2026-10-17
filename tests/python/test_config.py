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

from _streams import catalog_config as config
from _streams.cli import main


class ConfigTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.enterContext(patch.dict(os.environ, {
            "HOME": str(self.root), "XDG_CONFIG_HOME": str(self.root / "config"),
            "XDG_DATA_HOME": str(self.root / "data"), "XDG_STATE_HOME": str(self.root / "state"),
        }, clear=True))
        self.path = self.root / "config/streams/config.json"

    def cli(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with patch("sys.stdout", out), patch("sys.stderr", err):
            try:
                code = main(list(args))
            except SystemExit as error:
                code = error.code
        return code, out.getvalue(), err.getvalue()

    def test_defaults_and_origins_without_creating_config(self):
        code, out, err = self.cli("config", "--json")
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(json.loads(out), {
            "catalog": {"root": str(self.root / "data/streams/catalog")},
            "frontend": {"mode": "auto"},
        })
        self.assertFalse(self.path.exists())
        self.assertFalse(config.catalog_root().exists())
        code, out, _ = self.cli("config")
        self.assertEqual(code, 0)
        self.assertIn(str(self.path), out)
        self.assertIn('frontend.mode: "auto" (default)', out)
        self.assertIn(f'catalog.root: "{config.catalog_root()}" (default)', out)

    def test_set_get_values_and_preserve_other_keys(self):
        for key, value in (("frontend.mode", "herdr"), ("catalog.root", "~/Catalog")):
            self.assertEqual(self.cli("config", "set", key, value)[0], 0)
        self.assertEqual(self.cli("config", "get", "frontend.mode"), (0, "herdr\n", ""))
        self.assertEqual(self.cli("config", "get", "catalog.root", "--json"),
                         (0, json.dumps(str(self.root / "Catalog")) + "\n", ""))
        self.assertEqual(json.loads(self.path.read_text()), {
            "catalog": {"root": str(self.root / "Catalog")}, "frontend": {"mode": "herdr"},
        })
        self.assertIn('frontend.mode: "herdr" (configured)', self.cli("config")[1])
        self.assertFalse((self.root / "Catalog").exists())

    def test_removed_keys_and_invalid_values_are_rejected_without_writing(self):
        cases = (("frontend.mode", "yes"), ("frontend.mode", "AUTO"), ("catalog.root", "relative"),
                 ("catalog.root", " "), ("catalog.root", "/bad\x00path"),
                 ("cmux.enabled", "true"), ("notes.root", "/notes"),
                 ("obsidian.vault", "Notes"), ("focusShortcut", "alt+p"), ("selectorModel", "foo"))
        for key, value in cases:
            with self.subTest(key=key, value=value):
                code, out, err = self.cli("config", "set", key, value)
                self.assertEqual((code, out), (2, ""))
                self.assertTrue(err.startswith("streams:"))
                self.assertFalse(self.path.exists())
        for key in ("unknown", "cmux.enabled", "notes.root", "obsidian.vault"):
            self.assertEqual(self.cli("config", "get", key)[0], 2)

    def test_malformed_and_legacy_config_fail_closed_without_migration(self):
        self.path.parent.mkdir(parents=True)
        cases = ['{', '[]', '{"catalog":null}', '{"catalog":{"root":"relative"}}',
                 '{"catalog":{"unknown":"value"}}', '{"hotkeys":{}}',
                 '{"notes":{"root":"/notes"}}', '{"obsidian":{"vault":"Notes"}}',
                 '{"cmux":{"enabled":true}}', '{"cmux":{"enabled":false}}',
                 '{"frontend":{"mode":"cmux"},"cmux":{"enabled":true}}']
        for text in cases:
            with self.subTest(text=text):
                self.path.write_text(text)
                self.assertEqual(self.cli("config", "--json")[0], 2)
                self.assertEqual(self.cli("config", "set", "frontend.mode", "herdr")[0], 2)
                self.assertEqual(self.cli("list", "--json")[0], 2)
                self.assertEqual(self.path.read_text(), text)

    def test_stored_modes_are_strict_and_setter_preserves_invalid_file(self):
        self.path.parent.mkdir(parents=True)
        for mode in (None, True, 1, [], "", "AUTO", "unknown"):
            with self.subTest(mode=mode):
                self.path.write_text(json.dumps({"frontend": {"mode": mode}}))
                original = self.path.read_bytes()
                self.assertEqual(self.cli("config", "--json")[0], 2)
                self.assertEqual(self.cli("config", "set", "frontend.mode", "herdr")[0], 2)
                self.assertEqual(self.path.read_bytes(), original)

    def test_canonical_mode_drives_fresh_resolver_query(self):
        for mode, expected in (("auto", "herdr"), ("cmux", "cmux"), ("herdr", "herdr"), ("none", None)):
            with self.subTest(mode=mode):
                self.assertEqual(self.cli("config", "set", "frontend.mode", mode)[0], 0)
                with patch.dict(os.environ, {"HERDR_ENV": "1", "HERDR_PANE_ID": "w1:p1"}):
                    code, out, err = self.cli("frontend", "get", "-f", "auto", "--json")
                self.assertEqual((code, err), (0, ""))
                self.assertEqual(json.loads(out), {"configured": mode, "requested": "auto", "resolved": expected})

    def test_set_preserves_stow_symlink_and_unset_xdg_uses_home(self):
        target = self.root / "dotfiles/config.json"
        target.parent.mkdir()
        target.write_text('{}\n')
        self.path.parent.mkdir(parents=True)
        self.path.symlink_to(target)
        self.assertEqual(self.cli("config", "set", "frontend.mode", "herdr")[0], 0)
        self.assertTrue(self.path.is_symlink())
        self.assertEqual(json.loads(target.read_text())["frontend"]["mode"], "herdr")
        with patch.dict(os.environ, {"XDG_CONFIG_HOME": "", "XDG_DATA_HOME": ""}):
            self.assertIn(str(self.root / ".config/streams/config.json"), self.cli("config")[1])
            self.assertEqual(config.catalog_root(), self.root / ".local/share/streams/catalog")
            self.assertEqual(self.cli("config", "get", "frontend.mode")[1], "auto\n")

    def test_failed_atomic_write_preserves_existing_configuration_and_symlink(self):
        target = self.root / "managed.json"
        target.write_text('{"frontend":{"mode":"herdr"}}\n')
        self.path.parent.mkdir(parents=True)
        self.path.symlink_to(target)
        original = target.read_bytes()
        with patch("_streams.catalog_config.os.replace", side_effect=OSError("disk failure")):
            self.assertEqual(self.cli("config", "set", "frontend.mode", "none")[0], 2)
        self.assertTrue(self.path.is_symlink())
        self.assertEqual(target.read_bytes(), original)
        self.assertEqual(sorted(path.name for path in self.path.parent.iterdir()),
                         [".catalog-config.lock", "config.json"])

    def test_configured_root_requires_explicit_init_and_changes_are_fresh(self):
        first = self.root / "first"
        self.assertEqual(self.cli("config", "set", "catalog.root", str(first))[0], 0)
        self.assertEqual(self.cli("create", "Example", "--json")[0], 2)
        self.assertFalse(first.exists())
        self.assertEqual(self.cli("init", "--sync", "none")[0], 0)
        code, out, err = self.cli("create", "Example", "--json")
        self.assertEqual((code, err), (0, ""), out)
        identity = json.loads(out)["stream"]["id"]
        state = config.runtime_state_dir()
        self.assertEqual(self.cli("config", "set", "catalog.root", str(self.root / "second"))[0], 0)
        self.assertNotEqual(config.runtime_state_dir(), state)
        self.assertEqual(self.cli("list", "--json")[0], 2)
        self.assertEqual(self.cli("init", "--sync", "none")[0], 0)
        self.assertEqual(json.loads(self.cli("list", "--json")[1])["streams"], [])
        self.assertEqual(self.cli("config", "set", "catalog.root", str(first))[0], 0)
        self.assertEqual(json.loads(self.cli("show", identity, "--json")[1])["stream"]["id"], identity)


if __name__ == "__main__":
    unittest.main()
