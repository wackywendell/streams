from __future__ import annotations

from dataclasses import FrozenInstanceError, asdict, replace
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog.sync import GitCatalog, LocalCatalog, RepairRequired
from _streams.catalog.sync.state import AdoptingAttempt, GitConfig, PublicationAttempt, State, SyncMetadata
from _streams.catalog.yaml_store import YamlStore
from test_catalog_sync import create, effort


class TransportFixture(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name)

    def persist(self, state, name, data):
        # Deliberately bypass the typed writer, including JSON NaN/Infinity.
        (state.root / name).write_text(json.dumps(data, sort_keys=True))

    def bookkeeping(self, state):
        return {path.relative_to(state.root).as_posix(): path.read_bytes()
                for path in state.root.rglob("*") if path.is_file()}

    def assert_blocked(self, catalog):
        before = self.bookkeeping(catalog.state)
        for name, read in (("snapshot", catalog.snapshot),
                           ("read", lambda: catalog.read(refresh=True)),
                           ("authority", lambda: catalog.authority("work"))):
            with self.subTest(access=name):
                with self.assertRaises(RepairRequired):
                    read()
                self.assertEqual(self.bookkeeping(catalog.state), before)
        for name, action in (("save", lambda: catalog.save(create(effort("later"), "later"))),
                             ("sync", catalog.sync)):
            with self.subTest(access=name):
                result = action()
                self.assertEqual(result.local, "repair-required", result)
                self.assertNotEqual(result.sharing, "failed", result)
                self.assertEqual(self.bookkeeping(catalog.state), before)


class TypedStateTest(TransportFixture):
    def setUp(self):
        super().setUp()
        self.state = State(self.base)
        self.state.root.mkdir()

    def test_metadata_is_immutable_and_missing_file_is_clean(self):
        self.assertEqual(self.state.metadata(), SyncMetadata())
        self.assertFalse((self.state.root / "sync.json").exists())
        metadata = SyncMetadata(confirmed="a" * 40, last_sync=-2, last_push=0,
                                fetched_at=-1, last_confirmed_attempt="b" * 64)
        self.state.write_metadata(metadata)
        self.assertEqual(self.state.metadata(), metadata)
        with self.assertRaises(FrozenInstanceError):
            metadata.last_push = 10
        self.assertEqual(replace(metadata, last_push=10).last_push, 10)
        self.assertEqual(self.state.metadata().last_push, 0)

    def test_older_metadata_defaults_only_optional_fields(self):
        self.persist(self.state, "sync.json", {"version": 1, "confirmed": None,
                     "last_sync": -1, "last_push": 0, "retry_after": -2, "fetched_at": 0})
        metadata = self.state.metadata()
        self.assertEqual((metadata.last_sync, metadata.last_push, metadata.fetched_at), (-1, 0, 0))
        self.assertIsNone(metadata.last_confirmed_attempt)
        self.assertEqual((metadata.refresh_retry_after, metadata.refresh_error), (0, ""))

    def test_attempt_and_adopting_records_roundtrip_without_mutation(self):
        attempt = PublicationAttempt(base=None, candidate="a" * 40, local_tip="b" * 40,
                                     branch="main", remote="remote", operations=("create",))
        self.state.write_publication(attempt)
        loaded = self.state.publication()
        self.assertIs(type(loaded), PublicationAttempt)
        self.assertEqual(loaded, attempt)
        self.assertIsInstance(loaded.operations, tuple)
        self.assertIsNone(loaded.confirmed_at)
        with self.assertRaises(FrozenInstanceError):
            loaded.operations += ("later",)
        with self.assertRaises(TypeError):
            loaded.operations[0] = "changed"
        with self.assertRaises(ValueError):
            replace(attempt, operations=["create"])
        adopting = replace(attempt, confirmed_at=0).adopting("b" * 40, "a" * 40)
        self.assertIs(type(adopting), AdoptingAttempt)
        self.assertEqual((attempt.phase, adopting.phase), ("attempting", "adopting"))
        self.assertIsNone(attempt.confirmed_at)
        self.state.write_publication(adopting)
        self.assertEqual(self.state.publication(), adopting)
        self.assertEqual(self.state.publication().confirmed_at, 0)
        with self.assertRaises(FrozenInstanceError):
            adopting.adoption_to = "c" * 40
        # Older attempting records omit the optional confirmation timestamp.
        data = json.loads((self.state.root / "publication.json").read_text())
        for name in ("confirmed_at", "adoption_from", "adoption_to"):
            del data[name]
        data["phase"] = "attempting"
        self.persist(self.state, "publication.json", data)
        self.assertEqual(self.state.publication(), attempt)

    def test_config_is_typed_immutable_and_strictly_parsed(self):
        config = GitConfig(remote="remote", branch="main", host="laptop")
        self.state.write_config(config)
        self.assertEqual(self.state.config(), config)
        with self.assertRaises(FrozenInstanceError):
            config.branch = "elsewhere"
        baseline = {"version": 1, **asdict(config)}
        local = replace(config, remote=None)
        self.state.write_config(local)
        self.assertEqual(self.state.config(), local)
        cases = ({**baseline, "host": True}, {**baseline, "branch": " "},
                 {**baseline, "remote": ""}, {**baseline, "extra": 1},
                 {key: value for key, value in baseline.items() if key != "host"},
                 {key: value for key, value in baseline.items() if key != "remote"})
        for data in cases:
            with self.subTest(data=data):
                self.persist(self.state, "config.json", data)
                before = self.bookkeeping(self.state)
                with self.assertRaises(RepairRequired):
                    self.state.config()
                self.assertEqual(self.bookkeeping(self.state), before)


class GitTransportStateTest(TransportFixture):
    def setUp(self):
        super().setUp()
        config = self.base / "gitconfig"
        config.write_text('[user]\n name = Catalog Test\n email = catalog@example.org\n'
                          '[commit]\n gpgsign = false\n')
        environment = {"GIT_CONFIG_GLOBAL": str(config), "GIT_CONFIG_NOSYSTEM": "1",
                       "GIT_AUTHOR_NAME": "Catalog Test", "GIT_AUTHOR_EMAIL": "catalog@example.org",
                       "GIT_COMMITTER_NAME": "Catalog Test", "GIT_COMMITTER_EMAIL": "catalog@example.org"}
        isolated = patch.dict(os.environ, environment)
        isolated.start()
        self.addCleanup(isolated.stop)
        self.remote = self.base / "remote.git"
        subprocess.run(["git", "init", "--bare", str(self.remote)], capture_output=True, check=True)
        self.catalog = GitCatalog.initialize(self.base / "catalog", remote=str(self.remote),
                                             host_id="laptop", clock=lambda: 1000)
        self.assertEqual(self.catalog.save(create()).local, "committed")
        original = self.catalog.git.run

        def no_network(*args, **kwargs):
            if args[0] in ("fetch", "push", "ls-remote"):
                self.fail("malformed-state access attempted network: " + args[0])
            return original(*args, **kwargs)

        guard = patch.object(self.catalog.git, "run", side_effect=no_network)
        guard.start()
        self.addCleanup(guard.stop)

    def test_invalid_sync_records_fail_closed_without_rewriting_any_bookkeeping(self):
        baseline = {"version": 1, **asdict(self.catalog.state.metadata())}
        cases = {
            "string timestamp": {**baseline, "last_sync": "1000"},
            "boolean timestamp": {**baseline, "last_push": False},
            "NaN timestamp": {**baseline, "fetched_at": float("nan")},
            "infinite cooldown": {**baseline, "retry_after": float("inf")},
            "negative infinite refresh cooldown": {**baseline, "refresh_retry_after": -float("inf")},
            "invalid confirmed identity": {**baseline, "confirmed": "not-an-oid"},
            "boolean identity": {**baseline, "last_confirmed_attempt": True},
            "confirmation without baseline": {**baseline, "last_confirmed_attempt": "a" * 40},
            "unknown field": {**baseline, "future": 1},
            "missing required field": {key: value for key, value in baseline.items() if key != "last_sync"},
            "string version": {**baseline, "version": "1"},
            "boolean version": {**baseline, "version": True},
            "future version": {**baseline, "version": 2},
            "missing version": {key: value for key, value in baseline.items() if key != "version"},
            "wrong schema": [],
        }
        for name, data in cases.items():
            with self.subTest(scenario=name):
                self.persist(self.catalog.state, "sync.json", data)
                self.assert_blocked(self.catalog)

    def attempt(self):
        return PublicationAttempt(base=None, candidate=self.catalog.git.head(),
                                  local_tip=self.catalog.git.head(), branch="main",
                                  remote=str(self.remote), operations=("create",))

    def test_malformed_publication_evidence_fails_closed_before_network_or_receipt_changes(self):
        baseline = {"version": 1, "phase": "attempting", **asdict(self.attempt())}
        cases = {
            "missing candidate": {key: value for key, value in baseline.items() if key != "candidate"},
            "missing operations": {key: value for key, value in baseline.items() if key != "operations"},
            "invalid candidate": {**baseline, "candidate": "bad"},
            "invalid phase": {**baseline, "phase": "published"},
            "missing phase": {key: value for key, value in baseline.items() if key != "phase"},
            "not a list": {**baseline, "operations": "create"},
            "invalid operation": {**baseline, "operations": [False]},
            "duplicate operations": {**baseline, "operations": ["create", "create"]},
            "branch mismatch": {**baseline, "branch": "elsewhere"},
            "remote mismatch": {**baseline, "remote": str(self.base / "other.git")},
            "incomplete adoption": {**baseline, "phase": "adopting", "adoption_from": "a" * 40},
            "invalid adoption": {**baseline, "phase": "adopting", "adoption_from": "bad", "adoption_to": "a" * 40},
            "wrong phase fields": {**baseline, "adoption_to": "a" * 40},
            "invalid confirmation": {**baseline, "confirmed_at": True},
        }
        for name, data in cases.items():
            with self.subTest(scenario=name):
                self.persist(self.catalog.state, "publication.json", data)
                self.assert_blocked(self.catalog)

    def test_valid_attempt_preserves_uncertainty_and_allows_later_clean_save(self):
        attempt = self.attempt()
        self.catalog.state.write_publication(attempt)
        evidence = (self.catalog.state.root / "publication.json").read_bytes()
        self.assertEqual(self.catalog.snapshot().catalog.stream("work"), effort())
        self.assertEqual(self.catalog.save(create(effort("later"), "later")).local, "committed")
        self.assertEqual((self.catalog.state.root / "publication.json").read_bytes(), evidence)
        self.assertEqual(self.catalog.state.publication(), attempt)
        self.assertIsNone(self.catalog.state.publication().confirmed_at)
        reopened = GitCatalog(self.catalog.root, host_id="laptop")
        self.assertIsNotNone(reopened.snapshot().catalog.stream("later"))
        self.assertEqual(reopened.state.publication(), attempt)

    def test_zero_confirmation_is_not_treated_as_an_unconfirmed_attempt(self):
        attempt = replace(self.attempt(), confirmed_at=0)
        self.catalog.state.write_publication(attempt)
        before = self.bookkeeping(self.catalog.state)
        self.assertEqual(self.catalog._record_confirmation(attempt), attempt)
        self.assertEqual(self.bookkeeping(self.catalog.state), before)
        self.assertIsNone(self.catalog.state.metadata().last_push)
        confirmed = self.catalog._record_confirmation(replace(attempt, confirmed_at=None))
        self.assertEqual(confirmed.confirmed_at, 1000)
        self.assertEqual(self.catalog.state.metadata().last_push, 1000)
        self.assertEqual(self.catalog.state.publication(), confirmed)

    def test_valid_adopting_marker_blocks_access_and_preserves_all_evidence(self):
        attempt = replace(self.attempt(), confirmed_at=0)
        self.catalog.state.write_publication(attempt.adopting(attempt.local_tip, attempt.candidate))
        self.assert_blocked(self.catalog)
        self.assertEqual(self.catalog.state.publication().confirmed_at, 0)

    def test_missing_sync_file_allows_clean_local_save(self):
        (self.catalog.state.root / "sync.json").unlink()
        self.assertEqual(self.catalog.snapshot().catalog.stream("work"), effort())
        self.assertEqual(self.catalog.save(create(effort("later"), "later")).local, "committed")
        self.assertEqual(self.catalog.state.metadata(), SyncMetadata())

    def test_corrupt_config_is_rejected_at_direct_backend_boundary(self):
        config = {"version": 1, **asdict(self.catalog.state.config()), "branch": False}
        self.persist(self.catalog.state, "config.json", config)
        self.assert_blocked(self.catalog)


class LocalTransportStateTest(TransportFixture):
    def test_direct_local_backend_detects_corrupt_bookkeeping_without_cli_or_network(self):
        with patch("subprocess.run", side_effect=AssertionError("local-only attempted a subprocess")):
            catalog = LocalCatalog(YamlStore(self.base / "catalog"), host_id="laptop")
            self.assertEqual(catalog.save(create()).local, "persisted")
            baseline = self.bookkeeping(catalog.state)
            cases = (
                ("local.json", {"version": 1, "creations": []}),
                ("local.json", {"version": 1}),
                ("local.json", {"version": 1, "creations": {"work": "not an envelope"}}),
                ("local.json", {"version": 1, "creations": {}, "extra": True}),
                ("receipts.json", {"version": 1, "operations": {"create": "bad digest"}}),
                ("receipts.json", {"version": 1, "operations": []}),
                ("unexpected.json", {"version": 1}),
            )
            for name, data in cases:
                with self.subTest(file=name, data=data):
                    self.persist(catalog.state, name, data)
                    self.assert_blocked(catalog)
                    for path in catalog.state.root.iterdir():
                        if path.name not in baseline:
                            path.unlink()
                    for path, contents in baseline.items():
                        (catalog.state.root / path).write_bytes(contents)
            self.assertEqual(catalog.sync().sharing, "local-only")


if __name__ == "__main__":
    unittest.main()
