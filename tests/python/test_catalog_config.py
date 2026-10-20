from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import sys
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from uuid import UUID, uuid4

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams import catalog_config as config
from _streams.catalog.storage import StorageError
from _streams.catalog.sync import GitCatalog, LocalCatalog
from _streams.catalog.sync.policy import Policy
from _streams.errors import StreamError
from _streams.frontend import FrontendMode


class CatalogConfigTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.home = Path(temporary.name)
        environment = {"HOME": str(self.home), "XDG_CONFIG_HOME": str(self.home / "config"),
                       "XDG_DATA_HOME": str(self.home / "data"), "XDG_STATE_HOME": str(self.home / "state")}
        self.env = patch.dict(os.environ, environment)
        self.env.start()
        self.addCleanup(self.env.stop)

    def write_config(self, data):
        config.config_path().parent.mkdir(parents=True, exist_ok=True)
        config.config_path().write_text(json.dumps(data))

    def test_defaults_and_reads_create_nothing(self):
        self.assertEqual(config.CONFIG_KEYS, ("catalog.root", "frontend.mode", "sync.stream_seconds", "sync.session_seconds"))
        self.assertEqual(config.config_path(), self.home / "config/streams/config.json")
        self.assertEqual(config.read_overrides(), {})
        self.assertEqual(config.load_config(), {"catalog": {"root": str(self.home / "data/streams/catalog")},
                                               "frontend": {"mode": "auto"},
                                               "sync": {"stream_seconds": Policy().stream_seconds,
                                                        "session_seconds": Policy().session_seconds}})
        self.assertEqual(config.get_frontend_mode(), FrontendMode.AUTO)
        state = config.runtime_state_dir()
        self.assertTrue(state.is_relative_to(self.home / "state/streams/catalogs"))
        self.assertFalse(state.exists())
        with self.assertRaisesRegex(StreamError, "run streams init"):
            config.open_catalog()
        with self.assertRaisesRegex(StreamError, "run streams init"):
            config.host_id()
        self.assertEqual(list(self.home.iterdir()), [])

    def test_standard_home_fallbacks(self):
        with patch.dict(os.environ, {"XDG_CONFIG_HOME": "", "XDG_DATA_HOME": "", "XDG_STATE_HOME": ""}):
            self.assertEqual(config.config_path(), self.home / ".config/streams/config.json")
            self.assertEqual(config.catalog_root(), self.home / ".local/share/streams/catalog")
            self.assertTrue(config.runtime_state_dir().is_relative_to(self.home / ".local/state/streams/catalogs"))

    def test_only_new_keys_and_valid_values_are_accepted(self):
        for key, value in (("notes.root", str(self.home)), ("obsidian.vault", "old"),
                           ("cmux.enabled", "true"), ("catalog.root", "relative"),
                           ("catalog.root", ""), ("catalog.root", "/bad\x00path"),
                           ("frontend.mode", "invalid")):
            with self.subTest(key=key, value=value), self.assertRaises(StreamError):
                config.set_config(key, value)
        for data in ({"notes": {"root": str(self.home)}}, {"cmux": {"enabled": True}},
                     {"catalog": {"root": 123}}, {"catalog": {"unknown": "x"}},
                     {"frontend": {"mode": False}}, {"catalog": []}, []):
            self.write_config(data)
            with self.subTest(data=data), self.assertRaises(StreamError):
                config.load_config()

    def test_atomic_updates_preserve_stow_symlink_and_other_settings(self):
        target = self.home / "dotfiles/config.json"
        target.parent.mkdir()
        target.write_text('{}')
        config.config_path().parent.mkdir(parents=True)
        config.config_path().symlink_to(target)
        config.set_config("catalog.root", "~/my-catalog")
        config.set_config("frontend.mode", "none")
        self.assertTrue(config.config_path().is_symlink())
        self.assertEqual(json.loads(target.read_text()), {"catalog": {"root": str(self.home / "my-catalog")},
                                                        "frontend": {"mode": "none"}})

    def test_sync_file_values_are_strict_finite_nonnegative_numbers(self):
        for name in ("stream_seconds", "session_seconds"):
            for value in (0, 0.25, 12, 1e100):
                self.write_config({"sync": {name: value}})
                with self.subTest(name=name, value=value):
                    self.assertEqual(config.load_config()["sync"][name], value)
            for value in (True, False, None, "12", {}, [], -1, float("nan"),
                          float("inf"), -float("inf"), 10 ** 400):
                self.write_config({"sync": {name: value}})
                with self.subTest(name=name, value=value), self.assertRaises(StreamError):
                    config.load_config()
        for data in ({"sync": []}, {"sync": {"cooldown_seconds": 1}},
                     {"sync": {"timeout_seconds": 1}}, {"sync": {"attempts": 1}}):
            self.write_config(data)
            with self.subTest(data=data), self.assertRaises(StreamError):
                config.load_config()

    def test_sync_set_parses_text_and_rejects_before_atomic_write(self):
        target = self.home / "dotfiles/config.json"
        target.parent.mkdir()
        target.write_text('{"frontend":{"mode":"none"}}')
        config.config_path().parent.mkdir(parents=True)
        config.config_path().symlink_to(target)
        for key in ("sync.stream_seconds", "sync.session_seconds"):
            for text, expected in (("0", 0), ("1.25", 1.25), ("2e2", 200), (" 3 ", 3)):
                config.set_config(key, text)
                section, name = key.split(".")
                self.assertEqual(config.load_config()[section][name], expected)
                self.assertEqual(json.loads(target.read_text())[section][name], expected)
            before = target.read_bytes()
            for text in ("", "no", "-1", "true", "false", "null", '"12"', "{}", "[]",
                         "NaN", "Infinity", "-Infinity", "1e999", "9" * 400):
                with self.subTest(key=key, text=text), self.assertRaises(StreamError):
                    config.set_config(key, text)
                self.assertEqual(target.read_bytes(), before)
        config.set_config("catalog.root", "~/other-catalog")
        self.assertEqual(config.load_config()["sync"], {"stream_seconds": 3, "session_seconds": 3})
        self.assertEqual(config.load_config()["frontend"]["mode"], "none")
        self.assertTrue(config.config_path().is_symlink())
        self.assertEqual(list(target.parent.glob(".catalog-*.tmp")), [])

    def test_sync_config_get_set_in_fresh_cli_processes(self):
        env = dict(os.environ, PYTHONPATH=str(Path(__file__).resolve().parents[2] / "src"))
        def cli(*args):
            result = subprocess.run([sys.executable, "-m", "_streams.cli", "config", *args, "--json"],
                                    env=env, capture_output=True, text=True, timeout=15)
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)
        self.assertEqual(cli("get", "sync.session_seconds"), Policy().session_seconds)
        self.assertEqual(list(self.home.iterdir()), [])
        self.assertEqual(cli("set", "sync.stream_seconds", "0"), 0)
        self.assertEqual(cli("set", "sync.session_seconds", "1.5"), 1.5)
        self.assertEqual(cli("get", "sync.stream_seconds"), 0)
        self.assertEqual(cli()["sync"], {"stream_seconds": 0, "session_seconds": 1.5})

    def test_local_initialization_is_explicit_stable_and_never_uses_git(self):
        config.set_config("sync.stream_seconds", "0")
        config.set_config("sync.session_seconds", "0")
        with patch("subprocess.run", side_effect=AssertionError("local invoked Git")):
            backend = config.initialize("none")
            self.assertIsInstance(backend, LocalCatalog)
            identity = config.host_id()
            self.assertEqual(str(UUID(identity)), identity)
            self.assertEqual(backend.host_id, identity)
            marker = config.catalog_root() / ".catalog-sync/cli.json"
            before = marker.read_bytes()
            self.assertEqual(json.loads(before), {"version": 1, "strategy": "none", "host": identity})
            self.assertEqual(config.initialize("none").host_id, identity)
            self.assertEqual(marker.read_bytes(), before)
            self.assertEqual(config.open_catalog().snapshot().catalog.streams, ())
            host_file = config.config_path().parent / "host.json"
            self.assertEqual(host_file.stat().st_mode & 0o777, 0o600)
            self.assertFalse(config.runtime_state_dir().exists())

    def test_concurrent_init_uses_one_identity(self):
        with ThreadPoolExecutor(max_workers=6) as executor:
            identities = list(executor.map(lambda _: config.initialize("none").host_id, range(12)))
        self.assertEqual(len(set(identities)), 1)
        self.assertEqual(config.host_id(), identities[0])

    def test_existing_contents_are_not_adopted(self):
        root = config.catalog_root()
        root.mkdir(parents=True)
        (root / "important.txt").write_text("keep")
        with self.assertRaises((StreamError, StorageError)):
            config.initialize("none")
        self.assertFalse((root / ".catalog-sync/cli.json").exists())
        self.assertEqual((root / "important.txt").read_text(), "keep")

    def test_unrecognized_bookkeeping_is_not_adopted(self):
        root = config.catalog_root()
        (root / ".catalog-sync").mkdir(parents=True)
        (root / ".catalog-sync/config.json").write_text('{"version":1,"remote":"old"}')
        with self.assertRaises((StreamError, StorageError)):
            config.initialize("none")
        self.assertFalse((root / ".catalog-sync/cli.json").exists())

    def test_missing_or_corrupt_host_is_never_regenerated_for_existing_catalog(self):
        config.initialize("none")
        host_file = config.config_path().parent / "host.json"
        for contents in (None, "not json", '{"version":1,"id":"not-uuid"}'):
            if contents is None:
                host_file.unlink()
            else:
                host_file.write_text(contents)
            for command in (config.host_id, config.open_catalog, lambda: config.initialize("none")):
                with self.subTest(contents=contents), self.assertRaises(StreamError):
                    command()
            self.assertEqual(host_file.read_text() if host_file.exists() else None, contents)

    def test_markers_fail_closed_without_overwriting(self):
        config.initialize("none")
        marker = config.catalog_root() / ".catalog-sync/cli.json"
        for data in ({"version": 2, "strategy": "none", "host": config.host_id()},
                     {"version": 1, "strategy": "other", "host": config.host_id()},
                     {"version": 1, "strategy": "none", "host": str(uuid4())},
                     {"version": 1, "strategy": "none"}):
            marker.write_text(json.dumps(data))
            before = marker.read_bytes()
            for command in (config.open_catalog, lambda: config.initialize("none")):
                with self.assertRaises((StreamError, StorageError)):
                    command()
                self.assertEqual(marker.read_bytes(), before)

    def test_no_marker_no_open_and_no_creation(self):
        config.host_id(create=True)
        root = config.catalog_root()
        root.mkdir(parents=True)
        with self.assertRaisesRegex(StreamError, "run streams init"):
            config.open_catalog()
        self.assertEqual(list(root.iterdir()), [])

    def test_root_and_machinery_symlinks_fail_closed(self):
        other = self.home / "outside"
        other.mkdir()
        root = config.catalog_root()
        root.parent.mkdir(parents=True)
        root.symlink_to(other, target_is_directory=True)
        with self.assertRaises((StreamError, StorageError)):
            config.initialize("none")
        self.assertEqual(list(other.iterdir()), [])
        root.unlink()
        root.mkdir()
        (root / ".catalog-sync").symlink_to(other, target_is_directory=True)
        with self.assertRaises((StreamError, StorageError)):
            config.initialize("none")
        self.assertEqual(list(other.iterdir()), [])

    def test_corrupt_records_and_interrupted_save_are_rejected(self):
        config.initialize("none")
        root = config.catalog_root()
        (root / "streams").mkdir()
        record = root / "streams/bad.yaml"
        record.write_text("invalid: yaml")
        with self.assertRaises((StreamError, StorageError)):
            config.initialize("none")
        with self.assertRaises((StreamError, StorageError)):
            config.open_catalog()
        record.unlink()
        (root / ".catalog-sync/save.json").write_text('{"version":1,"setup":true}')
        with self.assertRaises((StreamError, StorageError)):
            config.initialize("none")

    def test_corrupt_or_symlinked_private_local_state_fails_closed(self):
        config.initialize("none")
        state = config.catalog_root() / ".catalog-sync"
        target = self.home / "outside.json"
        target.write_text('{"version":1,"operations":{}}')
        for name in ("receipts.json", "local.json"):
            path = state / name
            for contents in ('not json', '{"version":2}', '{"version":1}'):
                path.write_text(contents)
                with self.subTest(name=name, contents=contents), self.assertRaises(StreamError):
                    config.open_catalog()
            path.unlink()
            path.symlink_to(target)
            with self.assertRaises(StreamError):
                config.open_catalog()
            path.unlink()

    def test_strategy_and_remote_validation_precedes_initialization(self):
        for strategy, kwargs in (("invalid", {}), ("none", {"remote": "remote"}), ("git", {"remote": False}),
                                 ("git", {"remote": ""})):
            with self.subTest(strategy=strategy), self.assertRaises(StreamError):
                config.initialize(strategy, **kwargs)
        self.assertEqual(list(self.home.iterdir()), [])
        config.initialize("none")
        marker = config.catalog_root() / ".catalog-sync/cli.json"
        before = marker.read_bytes()
        with self.assertRaises((StreamError, StorageError)):
            config.initialize("git", remote="remote")
        self.assertEqual(marker.read_bytes(), before)

    def test_git_initialization_is_explicit_and_repeated_settings_must_match(self):
        gitconfig = self.home / "gitconfig"
        gitconfig.write_text('[user]\n name = Test\n email = test@example.org\n[commit]\n gpgsign = false\n')
        remote = self.home / "remote.git"
        with patch.dict(os.environ, {"GIT_CONFIG_GLOBAL": str(gitconfig), "GIT_CONFIG_NOSYSTEM": "1"}):
            subprocess.run(["git", "init", "--bare", str(remote)], capture_output=True, check=True)
            backend = config.initialize("git", remote=str(remote))
            self.assertIsInstance(backend, GitCatalog)
            self.assertEqual(config.initialize("git", remote=str(remote)).host_id, config.host_id())
            with patch.object(GitCatalog, "initialize", side_effect=AssertionError("automatic init")):
                with patch.object(GitCatalog, "_fetch", side_effect=AssertionError("open fetched")):
                    self.assertIsInstance(config.open_catalog(), GitCatalog)
            marker = config.catalog_root() / ".catalog-sync/cli.json"
            before = marker.read_bytes()
            for strategy, kwargs in (("none", {}), ("git", {"remote": str(remote), "branch": "other"}),
                                     ("git", {"remote": str(self.home / "other.git")})):
                with self.assertRaises(StreamError):
                    config.initialize(strategy, **kwargs)
                self.assertEqual(marker.read_bytes(), before)
            tracked = subprocess.run(["git", "-C", str(config.catalog_root()), "ls-files"],
                                     capture_output=True, check=True).stdout
            self.assertEqual(tracked, b"")

    def test_git_context_fast_open_skips_snapshot_but_keeps_readiness_checks(self):
        from _streams.catalog_agent import context
        from _streams.catalog.yaml_store import _YamlTransaction
        gitconfig = self.home / "gitconfig"
        gitconfig.write_text('[user]\n name = Test\n email = test@example.org\n[commit]\n gpgsign = false\n')
        with patch.dict(os.environ, {"GIT_CONFIG_GLOBAL": str(gitconfig), "GIT_CONFIG_NOSYSTEM": "1"}):
            config.initialize("git")
            with patch.object(GitCatalog, "snapshot", side_effect=AssertionError("full snapshot")), \
                 patch.object(_YamlTransaction, "read", side_effect=AssertionError("full record scan")):
                backend = config.open_catalog(validate_records=False)
                self.assertIsNone(context(backend, harness="pi", native_id="unattached")["stream"])
                with self.assertRaisesRegex(AssertionError, "full snapshot"):
                    config.open_catalog(validate_records=True)
                root = config.catalog_root()
                dirty = root / "unexplained.txt"
                dirty.write_text("dirty baseline")
                with self.assertRaisesRegex(StreamError, "unexplained catalog"):
                    config.open_catalog(validate_records=False)
                self.assertEqual(dirty.read_text(), "dirty baseline")
                dirty.unlink()
                marker = root / ".catalog-sync/save.json"
                marker.write_text('{"version":1,"setup":true}')
                with self.assertRaisesRegex(StreamError, "interrupted save"):
                    config.open_catalog(validate_records=False)
                self.assertEqual(marker.read_text(), '{"version":1,"setup":true}')

    def test_git_policy_is_configured_on_initialize_and_each_reopen(self):
        gitconfig = self.home / "gitconfig"
        gitconfig.write_text('[user]\n name = Test\n email = test@example.org\n[commit]\n gpgsign = false\n')
        remote = self.home / "remote.git"
        with patch.dict(os.environ, {"GIT_CONFIG_GLOBAL": str(gitconfig), "GIT_CONFIG_NOSYSTEM": "1"}):
            subprocess.run(["git", "init", "--bare", str(remote)], capture_output=True, check=True)
            config.set_config("sync.stream_seconds", "2.5")
            config.set_config("sync.session_seconds", "10")
            backend = config.initialize("git", remote=str(remote))
            expected = Policy(stream_seconds=2.5, session_seconds=10)
            self.assertEqual(backend.policy, expected)
            self.assertEqual(config.initialize("git", remote=str(remote)).policy, expected)
            reopened = config.open_catalog()
            self.assertEqual(reopened.policy, expected)
            reopened.clock = lambda: 100
            self.assertEqual(reopened.read(refresh=True).freshness, "refreshed")
            reopened.clock = lambda: 109
            with patch.object(reopened, "_fetch", side_effect=AssertionError("read fetched too soon")):
                self.assertEqual(reopened.read().freshness, "cached")
            reopened.clock = lambda: 110
            self.assertEqual(reopened.read().freshness, "refreshed")
            config.set_config("sync.stream_seconds", "0")
            config.set_config("sync.session_seconds", "0")
            changed = config.open_catalog()
            self.assertEqual(changed.policy, Policy(stream_seconds=0, session_seconds=0))
            self.assertEqual(backend.policy, expected)
            changed.clock = lambda: 110
            self.assertEqual(changed.read().freshness, "refreshed")
            env = dict(os.environ, PYTHONPATH=str(Path(__file__).resolve().parents[2] / "src"))
            result = subprocess.run([sys.executable, "-c",
                                     "from _streams.catalog_config import open_catalog; "
                                     "p = open_catalog().policy; print(p.stream_seconds, p.session_seconds)"],
                                    env=env, capture_output=True, text=True, timeout=15)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.strip(), "0 0")

    def test_git_does_not_adopt_existing_unmarked_bookkeeping(self):
        root = config.catalog_root()
        (root / ".catalog-sync").mkdir(parents=True)
        (root / ".catalog-sync/arbitrary.json").write_text("important")
        with patch.object(GitCatalog, "initialize", side_effect=AssertionError("adopted arbitrary state")):
            with self.assertRaises(StreamError):
                config.initialize("git", remote="unused")

    def test_runtime_directory_uses_canonical_root_but_not_frontend(self):
        first = config.runtime_state_dir()
        config.set_config("frontend.mode", "none")
        self.assertEqual(config.runtime_state_dir(), first)
        config.set_config("catalog.root", str(config.catalog_root() / "../catalog"))
        self.assertEqual(config.runtime_state_dir(), first)
        config.set_config("catalog.root", str(self.home / "other"))
        self.assertNotEqual(config.runtime_state_dir(), first)


if __name__ == "__main__":
    unittest.main()
