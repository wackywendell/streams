from __future__ import annotations

from dataclasses import replace
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog.model import record_key
from _streams.catalog.operations import Operation, update
from _streams.catalog.storage import StorageError
from _streams.catalog.yaml_store import _YamlTransaction
from _streams.catalog_agent import context, session_id_for
from _streams.errors import StreamError
from _streams.catalog.sync import LocalCatalog, Policy
from _streams.catalog.yaml_store import YamlStore
from test_catalog_sync import create, contribution, effort
import test_catalog_git as fixtures
from test_catalog_git import git


def assert_context_contract(case, backend):
    backend.save(create())
    backend.save(create(effort("other"), id="other"))
    own = replace(contribution().after, id=session_id_for("laptop", "pi", "native"))
    sibling = replace(own, id="sibling", harness_session_id="sibling-native")
    unrelated = replace(own, id="unrelated", stream_id="other", harness_session_id="other-native")
    for identity, record, token in (("own", own, "create"), ("sibling", sibling, "create"), ("unrelated", unrelated, "other")):
        case.assertTrue(backend.save(Operation(identity, None, record, "automatic", ownership_token=token)).accepted)
    paths = []
    original = _YamlTransaction._read_single_file

    def read_record(transaction, path):
        paths.append(path.relative_to(backend.store.root).as_posix())
        return original(transaction, path)

    with patch.object(backend, "snapshot", side_effect=AssertionError("full snapshot")), \
         patch.object(backend, "read", side_effect=AssertionError("projected read")), \
         patch.object(backend, "sync", side_effect=AssertionError("publication")), \
         patch.object(_YamlTransaction, "read", side_effect=AssertionError("full record scan")), \
         patch.object(_YamlTransaction, "_read_single_file", autospec=True, side_effect=read_record):
        captured = context(backend, harness="pi", native_id="native")
    case.assertEqual(captured["session"], own)
    case.assertEqual(set(captured["sessions"]), {own, sibling})
    case.assertEqual(captured["authority"].token, "create")
    case.assertTrue(paths)
    case.assertFalse(any("other" in path for path in paths), paths)
    # Non-deterministic existing IDs remain discoverable, too.
    case.assertEqual(context(backend, harness="pi", native_id="sibling-native")["session"], sibling)
    for hints in ({"stream_id": "other"}, {"workspace_stream_id": "other"}):
        with case.assertRaisesRegex(StreamError, "move"):
            context(backend, harness="pi", native_id="native", **hints)
    own_path = backend.store.root / backend.store.record_path(record_key(own))
    own_path.write_text("not: a session\n")
    with case.assertRaises(StorageError):
        context(backend, harness="pi", native_id="native")


class ReadPolicyTest(unittest.TestCase):
    def test_reads_use_the_session_interval_and_an_independent_overrideable_cooldown(self):
        policy = Policy(stream_seconds=5, session_seconds=60)
        self.assertTrue(policy.read_due(fetched_at=None, now=10))
        self.assertFalse(policy.read_due(fetched_at=10, now=69))
        self.assertTrue(policy.read_due(fetched_at=10, now=70))
        self.assertFalse(policy.read_due(fetched_at=10, now=70, retry_after=90))
        self.assertTrue(policy.read_due(fetched_at=10, now=70, retry_after=90, force=True))
        self.assertTrue(policy.read_due(fetched_at=10, now=11, force=True))


class BackendLocalReadTest(unittest.TestCase):
    def test_context_preserves_narrow_validated_own_session_and_siblings(self):
        with tempfile.TemporaryDirectory() as directory:
            assert_context_contract(self, LocalCatalog(YamlStore(Path(directory)), host_id="laptop"))

    def test_local_reads_and_sync_never_call_git(self):
        with tempfile.TemporaryDirectory() as directory, patch("subprocess.run", side_effect=AssertionError("Git called")):
            backend = LocalCatalog(YamlStore(Path(directory)), host_id="laptop")
            backend.save(create())
            for refresh in (False, True):
                result = backend.read(refresh=refresh)
                self.assertEqual(result.freshness, "local-only")
                self.assertEqual(result.catalog.stream("work"), effort())
                self.assertIsNone(result.confirmed)
            self.assertEqual(backend.sync(event="stream").sharing, "local-only")


# Reuse the bare-remote fixture without inheriting the earlier test methods.
class BackendGitReadTest(unittest.TestCase):
    setUp = fixtures.GitCatalogTest.setUp
    open = fixtures.GitCatalogTest.open
    remote_has_branch = fixtures.GitCatalogTest.remote_has_branch
    publish_initial = fixtures.GitCatalogTest.publish_initial

    def test_context_preserves_narrow_validated_own_session_and_siblings(self):
        assert_context_contract(self, self.a)
        self.assertFalse(hasattr(self.a, "handover"))

    def test_scheduled_read_fetches_under_N_and_explicit_refresh_overrides(self):
        self.publish_initial()
        b = self.open("b", "server")
        b.save(update("rename", effort(), {"title": "Remote title"}))
        b.sync()
        with patch.object(self.a.git, "run", wraps=self.a.git.run) as calls:
            self.now += 3599
            cached = self.a.read()
            self.assertEqual(cached.freshness, "cached")
            self.assertEqual(cached.catalog.stream("work").title, "Sync design")
            self.assertFalse(any(call.args[0] == "fetch" for call in calls.call_args_list))
        self.now += 1
        fresh = self.a.read()
        self.assertEqual(fresh.freshness, "refreshed")
        self.assertEqual(fresh.catalog.stream("work").title, "Remote title")
        self.assertEqual(self.a.read().freshness, "cached")
        self.assertEqual(self.a.read(refresh=True).freshness, "refreshed")

    def test_read_refresh_preserves_local_pending_changes_and_never_pushes(self):
        self.publish_initial()
        b = self.open("b", "server")
        self.a.save(update("park", effort(), {"status": "parked"}))
        b.save(update("rename", effort(), {"title": "Remote title"}))
        b.sync()
        local_before = git(self.a.root, "rev-parse", "HEAD")
        remote_before = git(self.remote, "rev-parse", "main")
        timing_before = self.a._metadata()
        with patch.object(self.a.git, "run", wraps=self.a.git.run) as calls:
            result = self.a.read(refresh=True)
        current = result.catalog.stream("work")
        self.assertEqual((current.title, current.status), ("Remote title", "parked"))
        self.assertEqual(result.local.catalog.stream("work").title, "Sync design")
        self.assertEqual(result.confirmed.catalog.stream("work").status, "active")
        self.assertEqual(git(self.a.root, "rev-parse", "HEAD"), local_before)
        self.assertEqual(git(self.remote, "rev-parse", "main"), remote_before)
        self.assertFalse(any(call.args[0] == "push" for call in calls.call_args_list))
        self.assertEqual(self.a._metadata().last_sync, timing_before.last_sync)
        self.assertEqual(self.a._metadata().last_push, timing_before.last_push)

    def test_conflicted_read_keeps_local_view_and_exposes_confirmed_alternative(self):
        self.publish_initial()
        b = self.open("b", "server")
        self.a.save(update("park", effort(), {"status": "parked"}))
        b.save(update("done", effort(), {"status": "done"}))
        b.sync()
        result = self.a.read(refresh=True)
        self.assertEqual(result.freshness, "refreshed")
        self.assertEqual(result.catalog.stream("work").status, "parked")
        self.assertEqual(result.confirmed.catalog.stream("work").status, "done")
        self.assertEqual(result.conflicts[0]["operation"], "park")
        self.assertEqual(self.a.read().conflicts, result.conflicts)

    def test_failed_read_reports_stale_view_without_resetting_freshness_or_publication(self):
        from _streams.catalog.sync.git_io import GitError
        self.publish_initial()
        before = self.a._metadata()
        original = self.a.git.run
        def unavailable(*args, **kwargs):
            if args[0] == "ls-remote":
                raise GitError("offline")
            return original(*args, **kwargs)
        self.now += 3601
        with patch.object(self.a.git, "run", side_effect=unavailable):
            result = self.a.read()
        self.assertEqual(result.freshness, "failed")
        self.assertIn("offline", result.detail)
        self.assertEqual(result.catalog.stream("work"), effort())
        self.assertEqual(self.a._metadata().fetched_at, before.fetched_at)
        self.assertEqual(self.a._metadata().last_sync, before.last_sync)
        self.assertEqual(self.a._metadata().retry_after, before.retry_after)
        self.assertEqual(self.a.read().freshness, "cached")
        self.assertEqual(self.a.read(refresh=True).freshness, "refreshed")

    def test_due_readers_share_one_refresh_under_the_store_lock(self):
        from concurrent.futures import ThreadPoolExecutor
        self.publish_initial()
        self.now += 3601
        with patch.object(self.a, "_fetch", wraps=self.a._fetch) as fetch:
            with ThreadPoolExecutor(max_workers=2) as pool:
                results = list(pool.map(lambda _: self.a.read(), range(2)))
        self.assertEqual(fetch.call_count, 1)
        self.assertEqual(sorted(r.freshness for r in results), ["cached", "refreshed"])

    def test_read_keeps_uncertain_publication_evidence_without_pushing(self):
        self.a.save(create())
        # Lose only the response and confirmation, not the accepted remote push.
        lost = fixtures.GitCatalogTest.lose_push_response(self)
        with patch.object(self.a.git, "run", side_effect=lost):
            self.assertEqual(self.a.sync().sharing, "uncertain")
        marker = self.a.state.read("publication.json")
        clocks = self.a._metadata()
        result = self.a.read(refresh=True)
        self.assertEqual(result.freshness, "refreshed")
        self.assertEqual(self.a.state.read("publication.json"), marker)
        self.assertEqual(self.a._metadata().last_push, clocks.last_push)
        self.assertEqual(self.a.sync(event="explicit").sharing, "confirmed")

    def test_expired_read_cooldown_keeps_failure_visible_until_success(self):
        from _streams.catalog.sync.git_io import GitError
        self.publish_initial()
        self.now += 3601
        original = self.a.git.run
        def unavailable(*args, **kwargs):
            if args[0] == "ls-remote":
                raise GitError("offline")
            return original(*args, **kwargs)
        with patch.object(self.a.git, "run", side_effect=unavailable):
            self.assertEqual(self.a.read().freshness, "failed")
        cached = self.a.read()
        self.assertIn("offline", cached.detail)
        self.now += 61
        self.assertEqual(self.a.read().freshness, "refreshed")
        self.assertEqual(self.a.read().detail, "")

    def test_write_fetch_counts_for_reads_even_if_push_fails(self):
        from _streams.catalog.sync.git_io import GitError
        self.publish_initial()
        self.a.save(update("park", effort(), {"status": "parked"}))
        self.now += 3601
        original = self.a.git.run
        def denied(*args, **kwargs):
            if args[0] == "push":
                raise GitError("denied", code=1)
            return original(*args, **kwargs)
        with patch.object(self.a.git, "run", side_effect=denied):
            self.assertEqual(self.a.sync(event="stream").sharing, "failed")
        result = self.a.read()
        self.assertEqual(result.freshness, "cached")
        self.assertEqual(result.fetched_at, self.now)

    def test_failed_read_cooldown_does_not_delay_due_publication(self):
        from _streams.catalog.sync.git_io import GitError
        self.publish_initial()
        self.a.save(update("park", effort(), {"status": "parked"}))
        self.now += 3601
        original = self.a.git.run
        def unavailable(*args, **kwargs):
            if args[0] == "ls-remote":
                raise GitError("offline")
            return original(*args, **kwargs)
        with patch.object(self.a.git, "run", side_effect=unavailable):
            self.assertEqual(self.a.read().freshness, "failed")
        self.assertEqual(self.a.sync(event="session").sharing, "confirmed")
        self.assertEqual(self.a.read().detail, "")

    def test_read_fetch_does_not_postpone_stream_publication(self):
        self.publish_initial()
        self.a.save(update("park", effort(), {"status": "parked"}))
        self.now += 301
        self.a.read(refresh=True)
        self.assertEqual(self.a.sync(event="session").sharing, "confirmed")

    def test_creation_and_session_saves_then_one_sync(self):
        self.a.save(create())
        self.a.save(contribution())
        with patch.object(self.a.git, "run", wraps=self.a.git.run) as calls:
            result = self.a.sync(event="session")
        self.assertEqual(result.sharing, "confirmed")
        self.assertEqual(sum(call.args[0] == "push" for call in calls.call_args_list), 1)


if __name__ == "__main__":
    unittest.main()
