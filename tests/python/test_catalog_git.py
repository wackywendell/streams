from __future__ import annotations

from dataclasses import replace
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog.sync import GitCatalog, Policy, RepairRequired
from _streams.catalog.sync.envelope import decode_operation
from _streams.catalog.sync.git_io import GitError
from _streams.catalog.operations import update
from _streams.catalog.yaml_store import YamlStore
from test_catalog_sync import create, effort, contribution


def git(root, *args):
    result = subprocess.run(["git", "-C", str(root), *args], capture_output=True, check=True)
    return result.stdout.decode().strip()


class GitCatalogTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name)
        config = self.base / "gitconfig"
        config.write_text('[user]\n name = Catalog Test\n email = catalog@example.org\n[commit]\n gpgsign = false\n')
        env = patch.dict(os.environ, {"GIT_CONFIG_GLOBAL": str(config), "GIT_CONFIG_NOSYSTEM": "1"})
        env.start()
        self.addCleanup(env.stop)
        self.remote = self.base / "remote.git"
        subprocess.run(["git", "init", "--bare", str(self.remote)], capture_output=True, check=True)
        self.now = 1000
        self.a = self.open("a")

    def open(self, name, host="laptop"):
        return GitCatalog.initialize(self.base / name, remote=str(self.remote), host_id=host,
                                     clock=lambda: self.now)

    def test_local_save_and_creation_authority_precede_publication(self):
        result = self.a.save(create())
        self.assertEqual((result.local, result.sharing), ("committed", "pending"))
        self.assertEqual(self.a.authority("work").token, "create")
        self.assertEqual(self.a.save(contribution()).local, "committed")
        self.assertEqual(self.a.snapshot().catalog.sessions, (contribution().after,))
        self.assertEqual(git(self.a.root, "rev-list", "--count", "HEAD"), "3")
        op, _ = decode_operation(git(self.a.root, "show", "-s", "--format=%B", "HEAD") + "\n")
        self.assertEqual(op, contribution())
        self.assertEqual(git(self.a.root, "ls-files").splitlines(),
                         ["sessions/work/laptop/session.yaml", "streams/work.yaml"])
        self.assertEqual(self.a.save(contribution()).local, "unchanged")
        self.assertFalse(self.remote_has_branch())

    def remote_has_branch(self):
        return bool(git(self.remote, "for-each-ref", "--format=%(refname)", "refs/heads/main"))

    def test_save_failure_blocks_later_writes_without_resetting_files(self):
        with patch.object(self.a, "_commit", side_effect=OSError("commit disk failure")):
            failed = self.a.save(create())
        self.assertEqual(failed.local, "repair-required")
        written = (self.a.root / "streams/work.yaml").read_bytes()
        self.assertEqual(self.a.save(create()).local, "repair-required")
        self.assertEqual(self.a.sync().local, "repair-required")
        self.assertEqual((self.a.root / "streams/work.yaml").read_bytes(), written)
        self.assertTrue((self.a.root / ".catalog-sync/save.json").exists())

    def test_commit_success_before_cleanup_is_still_visible_as_interrupted(self):
        with patch.object(self.a.state, "clear", side_effect=OSError("cleanup failure")):
            result = self.a.save(create())
        self.assertEqual(result.local, "repair-required")
        self.assertEqual(git(self.a.root, "rev-list", "--count", "HEAD"), "2")
        self.assertEqual(self.a.sync().local, "repair-required")

    def test_unexpected_or_ignored_files_are_never_automatically_committed(self):
        for name in ("manual.txt", ".gitignore"):
            path = self.a.root / name
            path.write_text("*.txt\n")
            self.assertEqual(self.a.save(create()).local, "repair-required")
            self.assertEqual(self.a.sync().local, "repair-required")
            path.unlink()
        self.assertEqual(self.a.save(create()).local, "committed")

    def test_reusing_operation_id_never_repeats_or_changes_its_meaning(self):
        self.a.save(create())
        parked = update("park", effort(), {"status": "parked"})
        self.a.save(parked)
        self.a.save(update("restore", parked.after, {"status": "active"}))
        self.assertEqual(self.a.save(parked).domain.outcome, "no-change")
        self.assertEqual(self.a.snapshot().catalog.stream("work").status, "active")
        self.assertEqual(self.a.save(update("park", effort(), {"status": "done"})).local, "repair-required")

    def test_canonical_adapter_paths_and_lock_inode_survive_sync(self):
        record = effort("Work/É")
        inode = (self.a.root / ".catalog.lock").stat().st_ino
        self.a.save(create(record))
        self.assertEqual(git(self.a.root, "ls-files"), 'streams/%57ork%2F%C3%89.yaml')
        self.assertEqual(self.a.sync().sharing, "confirmed")
        self.assertEqual((self.a.root / ".catalog.lock").stat().st_ino, inode)

    def publish_initial(self):
        self.a.save(create())
        self.assertEqual(self.a.sync().sharing, "confirmed")

    def test_two_clones_reconcile_independent_changes_and_preserve_operation_identity(self):
        self.publish_initial()
        b = self.open("b", "server")
        self.a.save(update("park", effort(), {"status": "parked"}))
        b.save(update("rename", effort(), {"title": "New title"}))
        self.assertEqual(b.sync().sharing, "confirmed")
        self.assertEqual(self.a.sync().sharing, "confirmed")
        current = self.a.snapshot().catalog.stream("work")
        self.assertEqual((current.status, current.title), ("parked", "New title"))
        self.assertEqual(b.sync().sharing, "confirmed")
        self.assertEqual(b.snapshot().catalog, self.a.snapshot().catalog)
        op, _ = decode_operation(git(self.a.root, "show", "-s", "--format=%B", "HEAD") + "\n")
        self.assertEqual(op, update("park", effort(), {"status": "parked"}))
        self.assertTrue(git(self.a.root, "for-each-ref", "--format=%(refname)", "refs/catalog/retained"))

    def test_one_conflict_blocks_entire_batch_but_allows_more_local_saves(self):
        self.publish_initial()
        b = self.open("b", "server")
        b.save(update("done", effort(), {"status": "done"}))
        b.sync()
        self.a.save(update("park", effort(), {"status": "parked"}), origin={"session": "responsible"})
        self.a.save(create(effort("independent"), "new-stream"))
        before = git(self.remote, "rev-parse", "main")
        blocked = self.a.sync()
        self.assertEqual(blocked.sharing, "blocked")
        self.assertEqual(blocked.conflicts[0]["operation"], "park")
        self.assertEqual(blocked.conflicts[0]["origin"]["session"], "responsible")
        self.assertEqual(git(self.remote, "rev-parse", "main"), before)
        self.assertEqual(self.a.confirmed_snapshot().catalog.stream("work").status, "done")
        self.assertEqual(self.a.snapshot().catalog.stream("work").status, "parked")
        self.assertEqual(self.a.save(create(effort("later"), "later")).local, "committed")

    def test_retired_intent_keeps_retry_identity_after_adoption_and_restart(self):
        self.publish_initial()
        b = self.open("b", "server")
        park = update("park-a", effort(), {"status": "parked"})
        self.a.save(park)
        b.save(update("park-b", effort(), {"status": "parked"}))
        b.sync()
        self.assertEqual(self.a.sync().sharing, "confirmed")
        self.a.save(update("restore", park.after, {"status": "active"}))
        self.a.sync()
        a = GitCatalog(self.a.root, host_id="laptop", clock=lambda: self.now)
        self.assertEqual(a.save(park).domain.outcome, "no-change")
        self.assertEqual(a.snapshot().catalog.stream("work").status, "active")
        changed = a.save(update("park-a", effort(), {"status": "done"}))
        self.assertEqual(changed.local, "repair-required")

    def test_remote_cannot_reinterpret_a_locally_retired_operation_id(self):
        self.publish_initial()
        b = self.open("b", "server")
        park = update("park-a", effort(), {"status": "parked"})
        self.a.save(park)
        b.save(update("park-b", effort(), {"status": "parked"}))
        b.sync()
        self.a.sync()
        last_confirmed = self.a._metadata().confirmed
        b.save(update("park-a", park.after, {"status": "done"}))
        b.sync()
        result = self.a.sync()
        self.assertEqual(result.local, "repair-required")
        self.assertEqual(self.a._metadata().confirmed, last_confirmed)
        self.assertEqual(self.a.snapshot().catalog.stream("work").status, "parked")

    def test_repaired_commit_establishes_missing_receipt_before_retirement(self):
        self.publish_initial()
        b = self.open("b", "server")
        park = update("park-a", effort(), {"status": "parked"})
        with patch.object(self.a.state, "remember", side_effect=OSError("crash before receipt")):
            self.assertEqual(self.a.save(park).local, "repair-required")
        # Explicit manual repair: verify the clean supported commit then clear
        # the marker. There is deliberately no receipt for park-a yet.
        from _streams.catalog.sync.history import read_history
        self.assertEqual(read_history(self.a.git, self.a.git.head()).operations["park-a"][0], park)
        self.a.state.clear("save.json")
        b.save(update("park-b", effort(), {"status": "parked"}))
        b.sync()
        self.assertEqual(self.a.sync().sharing, "confirmed")
        self.a.save(update("restore", park.after, {"status": "active"}))
        self.a.sync()
        a = GitCatalog(self.a.root, host_id="laptop")
        self.assertEqual(a.save(park).domain.outcome, "no-change")
        self.assertEqual(a.snapshot().catalog.stream("work").status, "active")

    def test_confirmed_lost_push_counts_only_once_despite_later_conflicts(self):
        self.a.save(create())
        with patch.object(self.a.git, "run", side_effect=self.lose_push_response()):
            self.assertEqual(self.a.sync().sharing, "uncertain")
        b = self.open("b", "server")
        b.save(update("remote-title", effort(), {"title": "Remote"}))
        b.sync()
        self.a.save(update("local-title", effort(), {"title": "Local"}))
        self.now += 100
        self.assertEqual(self.a.sync().sharing, "blocked")
        confirmed_at = self.a._metadata().last_push
        self.now += 100
        self.assertEqual(self.a.sync().sharing, "blocked")
        self.assertEqual(self.a._metadata().last_push, confirmed_at)

    def test_automatic_stale_update_is_retired_instead_of_retrying_forever(self):
        self.publish_initial()
        b = self.open("b", "server")
        self.a.save(update("infer", effort(), {"title": "Generated title"}, actor="automatic", ownership_token="create"))
        b.save(update("human", effort(), {"title": "Human title", "title_mode": "manual"}))
        b.sync()
        self.assertEqual(self.a.sync().sharing, "confirmed")
        self.assertEqual(self.a.snapshot().catalog.stream("work").title, "Human title")
        tip = git(self.a.root, "rev-parse", "HEAD")
        self.assertEqual(self.a.sync().sharing, "confirmed")
        self.assertEqual(git(self.a.root, "rev-parse", "HEAD"), tip)

    def test_stale_cross_record_input_retires_automatic_output_before_publication(self):
        self.publish_initial()
        self.a.save(contribution())
        self.a.sync()
        b = self.open("b", "server")
        before = contribution().after
        report = update("guarded-report", before, {"state": "Generated"}, actor="automatic",
                        ownership_token="create", inputs=(effort(),))
        self.assertEqual(self.a.save(report).domain.outcome, "changes")
        pending = git(self.a.root, "show", "-s", "--format=%B", "HEAD") + "\n"
        self.assertEqual(decode_operation(pending)[0], report)
        b.save(update("new-goal", effort(), {"goal": "Different scope"}))
        b.sync()
        remote_tip = git(self.remote, "rev-parse", "refs/heads/main")
        self.assertEqual(self.a.sync().sharing, "confirmed")
        self.assertEqual(self.a.snapshot().catalog.session("laptop", "session").state, "")
        self.assertEqual(git(self.remote, "rev-parse", "refs/heads/main"), remote_tip)

    def test_guarded_report_inputs_survive_rebase_over_an_independent_remote_edit(self):
        self.publish_initial()
        self.a.save(contribution())
        self.a.sync()
        b = self.open("b", "server")
        report = update("guarded-report", contribution().after, {"state": "Generated"}, actor="automatic",
                        ownership_token="create", inputs=(effort(),))
        self.a.save(report)
        b.save(create(effort("other", host="server"), id="create-other"))
        b.sync()
        self.assertEqual(self.a.sync().sharing, "confirmed")
        saved, _ = decode_operation(git(self.a.root, "show", "-s", "--format=%B", "HEAD") + "\n")
        self.assertEqual(saved, report)
        self.assertEqual(self.a.snapshot().catalog.session("laptop", "session").state, "Generated")

    def test_identical_remote_stream_with_different_creation_is_not_local_authority(self):
        b = self.open("b")
        self.a.save(create())
        b.save(create(id="different-creation"))
        b.sync()
        result = self.a.sync()
        self.assertEqual(result.sharing, "blocked")
        self.assertIn("identity", result.conflicts[0]["fields"])

    def test_event_gates_share_state_across_reopen_and_later_session_uses_M(self):
        self.publish_initial()
        self.a.save(contribution())
        self.now += 1
        self.assertEqual(self.a.sync(event="session").sharing, "pending")
        self.a.save(update("rename", effort(), {"title": "Changed"}))
        self.now += 298
        a = GitCatalog(self.a.root, host_id="laptop", clock=lambda: self.now)
        self.assertEqual(a.sync(event="session").sharing, "pending")
        self.now += 1
        self.assertEqual(a.sync(event="session").sharing, "confirmed")
        self.assertEqual(len(self.open("reader").snapshot().catalog.sessions), 1)

    def test_fetch_only_does_not_postpone_pending_stream_publication(self):
        self.publish_initial()
        self.a.save(update("park", effort(), {"status": "parked"}))
        self.now += 301
        self.a.inspect_remote()
        self.assertEqual(self.a.sync(event="session").sharing, "confirmed")
        self.assertEqual(self.a.snapshot().catalog.stream("work").status, "parked")

    def test_successful_no_push_sync_advances_only_full_sync_time(self):
        self.publish_initial()
        pushed_at = self.a._metadata().last_push
        self.now += 301
        self.assertEqual(self.a.sync(event="start").sharing, "confirmed")
        self.assertEqual(self.a._metadata().last_push, pushed_at)
        self.assertEqual(self.a._metadata().last_sync, self.now)

    def test_push_race_fetches_and_reconciles_again(self):
        self.publish_initial()
        b = self.open("b", "server")
        self.a.save(update("park", effort(), {"status": "parked"}))
        b.save(update("rename", effort(), {"title": "Competitor"}))
        original = self.a.git.run
        raced = False
        def race(*args, **kwargs):
            nonlocal raced
            if args[0] == "push" and not raced:
                raced = True
                self.assertEqual(b.sync().sharing, "confirmed")
            return original(*args, **kwargs)
        with patch.object(self.a.git, "run", side_effect=race):
            result = self.a.sync()
        self.assertEqual(result.sharing, "confirmed", result.detail)
        self.assertEqual(self.a.snapshot().catalog.stream("work").title, "Competitor")
        self.assertEqual(self.a.snapshot().catalog.stream("work").status, "parked")

    def lose_push_response(self):
        original = self.a.git.run
        def lost(*args, **kwargs):
            if args[0] == "push":
                original(*args, **kwargs)
                raise GitError("lost push response")
            if args[0] in ("ls-remote", "fetch") and self.remote_has_branch():
                raise GitError("cannot confirm")
            return original(*args, **kwargs)
        return lost

    def test_lost_response_then_restart_preserves_later_local_saves(self):
        self.a.save(create())
        with patch.object(self.a.git, "run", side_effect=self.lose_push_response()):
            uncertain = self.a.sync()
        self.assertEqual(uncertain.sharing, "uncertain")
        self.assertEqual(self.a.save(contribution()).local, "committed")
        a = GitCatalog(self.a.root, host_id="laptop", clock=lambda: self.now)
        self.assertEqual(a.sync().sharing, "confirmed")
        self.assertEqual(len(self.open("reader").snapshot().catalog.sessions), 1)
        history = git(self.remote, "log", "--format=%B", "main")
        self.assertEqual(history.count('"id":"create"'), 1)
        self.assertEqual(history.count('"id":"join"'), 1)

    def test_definite_remote_failure_keeps_commit_and_shared_cooldown(self):
        self.publish_initial()
        self.a.save(update("park", effort(), {"status": "parked"}))
        before = self.a._metadata()
        original = self.a.git.run
        def denied(*args, **kwargs):
            if args[0] == "push":
                raise GitError("permission denied", code=1)
            return original(*args, **kwargs)
        with patch.object(self.a.git, "run", side_effect=denied):
            self.assertEqual(self.a.sync().sharing, "failed")
        self.assertEqual(self.a._metadata().last_sync, before.last_sync)
        self.assertEqual(self.a.snapshot().catalog.stream("work").status, "parked")
        self.now += 301
        self.assertEqual(self.a.sync(event="session").sharing, "confirmed")

    def test_remote_success_with_failed_adoption_is_confirmed_but_needs_repair(self):
        self.a.save(create())
        with patch.object(self.a, "_adopt", side_effect=OSError("adoption failure")):
            result = self.a.sync()
        self.assertEqual((result.sharing, result.local), ("confirmed", "repair-required"))
        self.assertEqual(self.open("reader").snapshot().catalog.stream("work"), effort())
        self.assertTrue((self.a.root / ".catalog-sync/publication.json").exists())

    def test_adoption_interruption_after_worktree_update_preserves_both_histories(self):
        self.publish_initial()
        b = self.open("b", "server")
        b.save(update("remote-title", effort(), {"title": "Remote title"}))
        b.sync()
        self.a.save(update("park", effort(), {"status": "parked"}))
        old_tip = git(self.a.root, "rev-parse", "HEAD")
        original = self.a.git.run
        def interrupt(*args, **kwargs):
            if args[:2] == ("update-ref", "refs/heads/main"):
                raise GitError("interrupted ref adoption", code=1)
            return original(*args, **kwargs)
        with patch.object(self.a.git, "run", side_effect=interrupt):
            result = self.a.sync()
        self.assertEqual((result.sharing, result.local), ("confirmed", "repair-required"))
        self.assertEqual(git(self.a.root, "rev-parse", "HEAD"), old_tip)
        self.assertIn("Remote title", (self.a.root / "streams/work.yaml").read_text())
        self.assertEqual(self.a.save(create(effort("later"), "later")).local, "repair-required")
        reader = self.open("reader").snapshot().catalog.stream("work")
        self.assertEqual((reader.status, reader.title), ("parked", "Remote title"))

    def test_unaccepted_uncertain_push_replays_later_saves_with_remote_changes(self):
        self.publish_initial()
        b = self.open("b", "server")
        self.a.save(update("park", effort(), {"status": "parked"}))
        original = self.a.git.run
        pushed = False
        def unavailable(*args, **kwargs):
            nonlocal pushed
            if args[0] == "push":
                pushed = True
                raise GitError("request lost before acceptance")
            if pushed and args[0] in ("ls-remote", "fetch"):
                raise GitError("cannot inspect")
            return original(*args, **kwargs)
        with patch.object(self.a.git, "run", side_effect=unavailable):
            self.assertEqual(self.a.sync().sharing, "uncertain")
        self.assertEqual(self.a.save(create(effort("later"), "later")).local, "committed")
        b.save(update("rename", effort(), {"title": "Concurrent remote"}))
        b.sync()
        self.assertEqual(self.a.sync().sharing, "confirmed")
        catalog = self.open("reader").snapshot().catalog
        self.assertEqual((catalog.stream("work").status, catalog.stream("work").title),
                         ("parked", "Concurrent remote"))
        self.assertIsNotNone(catalog.stream("later"))
        history = git(self.remote, "log", "--format=%B", "main")
        self.assertEqual(history.count('"id":"park"'), 1)

    def test_remote_history_replacement_is_not_silently_adopted(self):
        self.publish_initial()
        # Delete the published branch to simulate unsupported external surgery.
        git(self.remote, "update-ref", "-d", "refs/heads/main")
        self.assertEqual(self.a.sync().local, "repair-required")
        self.assertEqual(self.a.snapshot().catalog.stream("work"), effort())

    def test_history_validation_reuses_immutable_objects_instead_of_rereading_every_tree(self):
        # A supported fixture without timing each preceding coordinator save.
        from _streams.catalog.sync import encode_operation
        for number in range(40):
            operation = create(effort(str(number)), id="create-" + str(number))
            with self.a.store.transaction() as transaction:
                transaction.save(operation.after, expected=transaction.read())
            git(self.a.root, "add", self.a.store.record_path(operation.target).as_posix())
            git(self.a.root, "commit", "-m", encode_operation(operation))
        a = GitCatalog(self.a.root, host_id="laptop")
        with patch.object(a.git, "run", wraps=a.git.run) as calls:
            self.assertEqual(a.save(create(effort("cold"), "cold")).local, "committed")
            self.assertLess(calls.call_count, 120, "cold validation should not reread unchanged historical blobs")
        with patch.object(a.git, "run", wraps=a.git.run) as calls:
            self.assertEqual(a.save(create(effort("warm"), "warm")).local, "committed")
            self.assertLess(calls.call_count, 40, "warm saves should validate only new immutable commits")

    def test_signing_failure_preserves_save_without_unsigned_fallback(self):
        git(self.a.root, "config", "commit.gpgsign", "true")
        git(self.a.root, "config", "gpg.program", str(self.base / "missing-signer"))
        result = self.a.save(create())
        self.assertEqual(result.local, "repair-required")
        self.assertEqual(git(self.a.root, "rev-list", "--count", "HEAD"), "1")
        self.assertTrue((self.a.root / "streams/work.yaml").exists())
        self.assertTrue((self.a.root / ".catalog-sync/save.json").exists())

    def test_separate_processes_serialize_record_index_and_commit(self):
        source = str(Path(__file__).resolve().parents[2] / "src")
        script = '''
import sys
sys.path.insert(0, sys.argv[1])
from pathlib import Path
from _streams.catalog.model import Stream
from _streams.catalog.operations import Operation
from _streams.catalog.sync import GitCatalog
catalog = GitCatalog(Path(sys.argv[2]), host_id="laptop")
record = Stream(id=sys.argv[3], title="Parallel", goal="Keep both updates", owner_host_id="laptop")
result = catalog.save(Operation(id=sys.argv[3], before=None, after=record, actor="user"))
assert result.local == "committed", result
'''
        children = [subprocess.Popen([sys.executable, "-c", script, source, str(self.a.root), name],
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE) for name in ("first", "second")]
        for child in children:
            out, err = child.communicate(timeout=30)
            self.assertEqual(child.returncode, 0, err.decode())
        self.assertEqual({s.id for s in self.a.snapshot().catalog.streams}, {"first", "second"})
        self.assertEqual(git(self.a.root, "rev-list", "--count", "HEAD"), "3")

    def test_manual_record_or_commit_envelope_tampering_fails_closed(self):
        self.a.save(create())
        path = self.a.root / "streams/work.yaml"
        path.write_text(path.read_text().replace("Sync design", "Tampered"))
        git(self.a.root, "add", "streams/work.yaml")
        git(self.a.root, "commit", "-m", "manual change")
        result = self.a.sync()
        self.assertEqual(result.local, "repair-required")


if __name__ == "__main__":
    unittest.main()
