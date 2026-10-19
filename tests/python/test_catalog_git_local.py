from __future__ import annotations

from contextlib import ExitStack, contextmanager
from dataclasses import replace
from datetime import datetime, timezone
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog.model import Host
from _streams.catalog.operations import Operation, update
from _streams.catalog.sync import GitCatalog, LocalCatalog, Policy, RepairRequired
from _streams.catalog.sync.envelope import BOOTSTRAP
from _streams.catalog.sync.git_io import Git
from _streams.catalog.sync.history import read_history
from _streams.catalog.sync.state import PublicationAttempt, State, SyncMetadata
from _streams.catalog.yaml_store import YamlStore
from test_catalog_sync import contribution, create, effort


class LocalGitCatalogTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name)
        config = self.base / "gitconfig"
        config.write_text('[user]\n name = Local Catalog Test\n email = local@example.invalid\n'
                          '[commit]\n gpgsign = false\n')
        environment = patch.dict(os.environ, {"GIT_CONFIG_GLOBAL": str(config), "GIT_CONFIG_NOSYSTEM": "1"})
        environment.start()
        self.addCleanup(environment.stop)
        self.commands = []
        run = Git.run

        def local_run(git, *args, **kwargs):
            self.commands.append(args)
            if args[0] in {"ls-remote", "fetch", "push"}:
                raise AssertionError(f"no-remote catalog attempted network command: {args}")
            return run(git, *args, **kwargs)

        spy = patch.object(Git, "run", new=local_run)
        spy.start()
        self.addCleanup(spy.stop)

    def open(self, name="catalog", **kwargs):
        return GitCatalog.initialize(self.base / name, host_id="laptop", **kwargs)

    def remote_bookkeeping(self, catalog):
        return {name: (catalog.state.root / name).read_bytes()
                if (catalog.state.root / name).exists() else None
                for name in ("sync.json", "publication.json")}

    def private_bytes(self, catalog):
        return {path.name: path.read_bytes() for path in catalog.state.root.iterdir() if path.is_file()}

    def assert_local(self, result, catalog, *, local="unchanged", outcome=None):
        self.assertEqual((result.sharing, result.local), ("local-only", local))
        self.assertIsNone(result.remote_revision)
        if local != "repair-required":
            self.assertEqual(result.local_revision, catalog.git.head())
        if outcome is not None:
            self.assertIsNotNone(result.domain)
            self.assertEqual(result.domain.outcome, outcome)

    @contextmanager
    def no_remote_pipeline(self, catalog):
        with ExitStack() as stack:
            for target, name in ((catalog, "_fetch"), (catalog, "_candidate"),
                                 (catalog, "_failure"), (catalog, "_success"),
                                 (catalog.state, "write_metadata"), (catalog.state, "write_publication"),
                                 (Policy, "read_due")):
                stack.enter_context(patch.object(target, name, side_effect=AssertionError(f"unexpected {name}")))
            stack.enter_context(patch("_streams.catalog.sync.replay.replay",
                                      side_effect=AssertionError("unexpected replay")))
            yield

    def test_fresh_and_existing_empty_bootstrap_reopen_without_origin(self):
        for existing in (False, True):
            for explicit_remote in (False, True):
                with self.subTest(existing=existing, explicit_remote=explicit_remote):
                    name = f"catalog-{existing}-{explicit_remote}"
                    root = self.base / name
                    if existing:
                        root.mkdir()
                    kwargs = {"remote": None} if explicit_remote else {}
                    with patch.object(GitCatalog, "_fetch", side_effect=AssertionError("initialization fetched")):
                        catalog = self.open(name, **kwargs)
                        tip = catalog.git.head()
                        reopened = self.open(name, **kwargs)
                    self.assertEqual(reopened.git.head(), tip)
                    self.assertEqual(catalog.git.text("show", "-s", "--format=%B", "HEAD"), BOOTSTRAP.strip())
                    self.assertEqual(catalog.git.text("rev-list", "--count", "HEAD"), "1")
                    self.assertEqual(catalog.git.text("ls-files"), "")
                    self.assertEqual(catalog.git.text("symbolic-ref", "HEAD"), "refs/heads/main")
                    self.assertEqual(catalog.git.text("remote"), "")
                    self.assertIsNone(catalog.state.config().remote)
                    self.assertEqual(catalog.state.config().branch, "main")
                    self.assertEqual(self.remote_bookkeeping(catalog), {"sync.json": None, "publication.json": None})

    def test_save_outcomes_keep_local_sharing_revision_and_refused_intent(self):
        catalog = self.open()
        with self.no_remote_pipeline(catalog):
            self.assert_local(catalog.save(create()), catalog, local="committed", outcome="changes")
            # A distinct operation already satisfied by current records is a domain no-op.
            self.assert_local(catalog.save(create(id="same-record")), catalog, outcome="no-change")
            self.assert_local(catalog.save(create()), catalog, outcome="no-change")
            park = update("park", effort(), {"status": "parked"})
            self.assert_local(catalog.save(park), catalog, local="committed", outcome="changes")
            self.assert_local(catalog.save(update("conflict", effort(), {"status": "done"})),
                              catalog, outcome="conflict")
            stale = update("stale", effort(), {"title": "Generated"}, actor="automatic",
                           ownership_token="wrong-period")
            self.assert_local(catalog.save(stale), catalog, outcome="superseded")
            tip, evidence = catalog.git.head(), self.private_bytes(catalog)
            self.assert_local(catalog.save(update("park", effort(), {"status": "done"})),
                              catalog, local="repair-required")
            self.assertEqual(catalog.git.head(), tip)
            self.assertEqual(self.private_bytes(catalog), evidence)
            self.assertEqual(catalog.snapshot().catalog.stream("work").status, "parked")
        self.assertEqual(self.remote_bookkeeping(catalog), {"sync.json": None, "publication.json": None})
        self.assertEqual(catalog.git.text("remote"), "")

    def test_direct_and_refresh_reads_return_the_complete_saved_graph(self):
        catalog = self.open()
        host = Host("laptop", "Laptop", datetime(2026, 1, 1, tzinfo=timezone.utc))
        for operation in (Operation("host", None, host, "user"), create(), contribution()):
            self.assert_local(catalog.save(operation), catalog, local="committed", outcome="changes")
        saved = catalog.snapshot()
        before = self.private_bytes(catalog)
        with self.no_remote_pipeline(catalog):
            for refresh in (False, True):
                with self.subTest(refresh=refresh):
                    result = catalog.read(refresh=refresh)
                    self.assertEqual(result.catalog, saved.catalog)
                    self.assertEqual(result.local, saved)
                    self.assertEqual(result.catalog.hosts, (host,))
                    self.assertEqual(result.catalog.streams, (effort(),))
                    self.assertEqual(result.catalog.sessions, (contribution().after,))
                    self.assertEqual(result.freshness, "local-only")
                    self.assertIsNone(result.confirmed)
                    self.assertIsNone(result.fetched_at)
                    self.assertEqual(result.conflicts, ())
                    self.assertEqual(result.detail, "")
        self.assertEqual(self.private_bytes(catalog), before)
        self.assertEqual(self.remote_bookkeeping(catalog), {"sync.json": None, "publication.json": None})

    def test_all_sync_events_validate_but_ignore_remote_gates(self):
        catalog = self.open()
        self.assert_local(catalog.save(create()), catalog, local="committed", outcome="changes")
        before = self.private_bytes(catalog)
        due = Policy.due
        for gate in (False, True):
            checked = []

            def gated(policy, event, timing, **kwargs):
                due(policy, event, timing, **kwargs)
                checked.append(event)
                return gate

            with self.no_remote_pipeline(catalog), patch.object(Policy, "due", new=gated):
                for event in ("session", "host", "stream", "start", "attach", "shutdown", "explicit", "force"):
                    with self.subTest(gate=gate, event=event):
                        self.assert_local(catalog.sync(event=event), catalog)
                        self.assertIn(event, checked)
                with self.assertRaisesRegex(ValueError, "unsupported sync event"):
                    catalog.sync(event="unknown")
        self.assertEqual(self.private_bytes(catalog), before)
        self.assertEqual(self.remote_bookkeeping(catalog), {"sync.json": None, "publication.json": None})

    def test_local_sync_failure_never_records_publication_failure(self):
        catalog = self.open()
        before = self.private_bytes(catalog)
        with self.no_remote_pipeline(catalog), patch.object(catalog, "_saved", side_effect=OSError("local disk")):
            result = catalog.sync(event="force")
        self.assert_local(result, catalog, local="repair-required")
        self.assertIn("local disk", result.detail)
        self.assertEqual(self.private_bytes(catalog), before)
        with self.no_remote_pipeline(catalog), patch.object(catalog, "_saved", side_effect=AssertionError("invalid event read history")):
            with self.assertRaisesRegex(ValueError, "unsupported sync event"):
                catalog.sync(event="unknown")

    def test_fetch_and_remote_inspection_reject_absent_destination(self):
        catalog = self.open()
        before = self.private_bytes(catalog)
        with self.assertRaisesRegex(RepairRequired, "no remote"):
            catalog._fetch()
        with patch.object(catalog, "_fetch", side_effect=AssertionError("inspection fetched")):
            with self.assertRaisesRegex(RepairRequired, "no remote"):
                catalog.inspect_remote()
        self.assertEqual(self.private_bytes(catalog), before)

    def test_any_external_remote_fails_closed_without_probing(self):
        catalog = self.open()
        catalog.git.run("remote", "add", "unexpected", "https://example.invalid/never-probe.git")
        before = self.private_bytes(catalog)
        with self.no_remote_pipeline(catalog):
            for result in (catalog.save(create()), catalog.sync(event="force")):
                self.assert_local(result, catalog, local="repair-required")
                self.assertIn("unexpected remote", result.detail)
            with self.assertRaisesRegex(RepairRequired, "unexpected remote"):
                catalog.read(refresh=True)
            with self.assertRaisesRegex(RepairRequired, "unexpected remote"):
                self.open()
        self.assertEqual(self.private_bytes(catalog), before)

    def test_nondefault_remote_state_requires_repair_and_preserves_exact_bytes(self):
        metadata = SyncMetadata()
        variants = [replace(metadata, **{field: value}) for field, value in (
            ("confirmed", "a" * 40), ("last_sync", 1), ("last_push", 1), ("retry_after", 1),
            ("fetched_at", 1), ("refresh_retry_after", 1), ("refresh_error", "old failure"))]
        variants.append(replace(metadata, last_push=1, last_confirmed_attempt="b" * 40))
        variants.extend(("attempting", "adopting"))
        for index, value in enumerate(variants):
            with self.subTest(state=value):
                catalog = self.open(f"state-{index}")
                if isinstance(value, SyncMetadata):
                    catalog.state.write_metadata(value)
                else:
                    tip = catalog.git.head()
                    marker = PublicationAttempt(base=None, candidate=tip, local_tip=tip, branch="main",
                                                remote="https://example.invalid/unused", operations=())
                    catalog.state.write_publication(marker if value == "attempting" else marker.adopting(tip, tip))
                before = self.private_bytes(catalog)
                with self.no_remote_pipeline(catalog):
                    self.assert_local(catalog.save(create()), catalog, local="repair-required")
                    self.assert_local(catalog.sync(event="force"), catalog, local="repair-required")
                    for refresh in (False, True):
                        with self.assertRaises(RepairRequired):
                            catalog.read(refresh=refresh)
                self.assertEqual(self.private_bytes(catalog), before)

    def test_default_metadata_is_accepted_without_rewriting_it(self):
        catalog = self.open()
        catalog.state.write_metadata(SyncMetadata())
        before = self.remote_bookkeeping(catalog)
        with self.no_remote_pipeline(catalog):
            self.assert_local(catalog.save(create()), catalog, local="committed", outcome="changes")
            catalog.read(refresh=True)
            self.assert_local(catalog.sync(event="shutdown"), catalog)
        self.assertEqual(self.remote_bookkeeping(catalog), before)

    def test_ordinary_history_rehydrates_receipts_and_refuses_changed_intent_without_rewrite(self):
        catalog = self.open()
        park = update("park", effort(), {"status": "parked"})
        for operation in (create(), park, update("restore", park.after, {"status": "active"})):
            self.assert_local(catalog.save(operation), catalog, local="committed", outcome="changes")
        expected = catalog.state.fingerprints()
        receipts = catalog.state.root / "receipts.json"
        receipts.unlink()
        catalog = GitCatalog(catalog.root, host_id="laptop")
        with self.no_remote_pipeline(catalog):
            self.assertEqual(dict(read_history(catalog.git, catalog.git.head()).fingerprints), expected)
            self.assertFalse(receipts.exists())
            catalog.read(refresh=True)
            self.assertEqual(catalog.state.fingerprints(), expected)
            self.assertEqual(catalog.authority("work").token, "create")
            self.assert_local(catalog.save(park), catalog, outcome="no-change")
            before, tip = self.private_bytes(catalog), catalog.git.head()
            self.assert_local(catalog.save(update("park", effort(), {"status": "done"})),
                              catalog, local="repair-required")
            self.assertEqual(self.private_bytes(catalog), before)
            self.assertEqual(catalog.git.head(), tip)
            self.assertEqual(catalog.snapshot().catalog.stream("work").status, "active")
        self.assertEqual(self.remote_bookkeeping(catalog), {"sync.json": None, "publication.json": None})

    def test_local_readiness_prioritizes_interrupted_save_over_git_and_config_leftovers(self):
        for leftover in ("git", "config", "both"):
            with self.subTest(leftover=leftover):
                root = self.base / leftover
                local = LocalCatalog(YamlStore(root), host_id="laptop")
                self.assertEqual(local.save(create()).local, "persisted")
                local.state.write("save.json", {"adoption": "interrupted"})
                if leftover in ("git", "both"):
                    (root / ".git").mkdir()
                if leftover in ("config", "both"):
                    local.state.write("config.json", {"remote": None, "branch": "main", "host": "laptop"})
                before = self.private_bytes(local)
                with self.assertRaisesRegex(RepairRequired, r"interrupted save; inspect .*save\.json"):
                    local.snapshot()
                for result in (local.save(create()), local.sync()):
                    self.assertEqual((result.sharing, result.local), ("local-only", "repair-required"))
                    self.assertIn("interrupted save", result.detail)
                self.assertEqual(self.private_bytes(local), before)

    def test_digest_map_checks_every_collision_before_writing_missing_entries(self):
        state = State(self.base / "state")
        original = {"first": "a" * 64, "last": "b" * 64}
        state.validate_fingerprints(original, remember=True)
        before = (state.root / "receipts.json").read_bytes()
        for collision in original:
            incoming = {"missing": "c" * 64, **original, collision: "d" * 64}
            with self.subTest(collision=collision), patch.object(state, "write", side_effect=AssertionError("partial receipt write")):
                with self.assertRaisesRegex(RepairRequired, "different intent"):
                    state.validate_fingerprints(incoming, remember=True)
            self.assertEqual((state.root / "receipts.json").read_bytes(), before)
            self.assertEqual(state.fingerprints(), original)
        state.validate_fingerprints({"missing": "c" * 64}, remember=False)
        self.assertEqual((state.root / "receipts.json").read_bytes(), before)
        state.validate_fingerprints({"missing": "c" * 64}, remember=True)
        self.assertEqual(state.fingerprints(), {**original, "missing": "c" * 64})


if __name__ == "__main__":
    unittest.main()
