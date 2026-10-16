from __future__ import annotations

from dataclasses import replace
from datetime import datetime, timezone
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog.codec import record_from_data, record_to_data
from _streams.catalog.model import Checkout, Document, Host, Link, RecordKey, Repository, ValidationError
from _streams.catalog.operations import Authority, Operation, update
from _streams.catalog.storage import RevisionConflict, StorageError, apply_operation
from _streams.catalog.yaml_store import YamlStore
from test_catalog_model import session, stream


class CatalogStorageTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / "catalog"
        self.store = YamlStore(self.root)

    def create(self, record):
        with self.store.transaction() as tx:
            snapshot = tx.read()
            return tx.save(record, expected=snapshot)

    def test_json_compatible_mapping_and_yaml_round_trip(self):
        effort = stream(repositories={"code": Repository(url="https://example.org/code")},
                        documents=(Document(repository="code", ref="main", path="docs/design.md", description="Design"),),
                        issues=(Link(url="https://example.org/1", description="Issue"),), goal_short="Understand work")
        contribution = session(checkout=Checkout(repository="code", ref="feature"), state="Draft ready", state_short="Drafted")
        host = Host(id="laptop", label="Laptop", checked_at=datetime(2026, 10, 16, tzinfo=timezone.utc))
        for kind, record in (("stream", effort), ("session", contribution), ("host", host)):
            with self.subTest(kind=kind):
                data = json.loads(json.dumps(record_to_data(record)))
                self.assertEqual(record_from_data(kind, data), record)
                self.create(record)
        with self.store.transaction() as tx:
            snapshot = tx.read()
        self.assertEqual(snapshot.catalog.streams, (effort,))
        self.assertEqual(snapshot.catalog.sessions, (contribution,))
        self.assertEqual(snapshot.catalog.hosts, (host,))
        self.assertEqual(sorted(str(p.relative_to(self.root)) for p in self.root.rglob("*.yaml")),
                         ["hosts/laptop.yaml", "sessions/laptop/session-1.yaml", "streams/stream-1.yaml"])
        self.assertFalse((self.root / ".git").exists())

    def test_minimal_physical_mapping_and_strict_unknown_fields(self):
        data = {"id": "s", "title": "Work", "goal": "Understand it", "owner": {"host_id": "laptop"}}
        self.assertEqual(record_from_data("stream", data).status, "active")
        for changes in ({"state": "Legacy field"}, {"version": 1}, {"archived": 1}, {"owner": {}},
                        {"repositories": []}, {"issues": None}, {"goal": None}):
            with self.subTest(changes=changes), self.assertRaises(ValidationError):
                record_from_data("stream", data | changes)
        for value in (None, [], "record", {"id": "only-an-id"}):
            with self.subTest(value=value), self.assertRaises(ValidationError):
                record_from_data("session", value)

    def test_rename_and_archive_do_not_move_files_or_alter_status(self):
        effort = stream(status="blocked")
        snapshot = self.create(effort)
        with self.store.transaction() as tx:
            tx.save(replace(effort, title="A new title", archived=True), expected=snapshot)
        self.assertEqual([p.name for p in (self.root / "streams").iterdir()], ["stream-1.yaml"])
        with self.store.transaction() as tx:
            self.assertEqual(tx.read().catalog.streams[0].status, "blocked")

    def test_revisions_are_opaque_and_aba_safe_and_noops_do_not_write(self):
        effort = stream()
        original = self.create(effort)
        with self.store.transaction() as tx:
            unchanged = tx.save(effort, expected=original)
            self.assertEqual(unchanged.revisions, original.revisions)
            changed = tx.save(replace(effort, title="Changed"), expected=original)
            returned = tx.save(effort, expected=changed)
            self.assertNotEqual(returned.revisions, original.revisions)
            with self.assertRaises(RevisionConflict):
                tx.save(replace(effort, status="parked"), expected=original)

    def test_unrelated_record_change_invalidates_the_captured_snapshot(self):
        snapshot = self.create(stream())
        self.create(Host(id="laptop", label="Laptop", checked_at=datetime(2026, 10, 16, tzinfo=timezone.utc)))
        with self.store.transaction() as tx:
            with self.assertRaises(RevisionConflict):
                tx.save(replace(stream(), title="Changed"), expected=snapshot)
            self.assertEqual(tx.read().catalog.streams[0], stream())

    def test_relationship_and_native_identity_validation_is_atomic(self):
        self.create(stream())
        first = self.create(session())
        with self.store.transaction() as tx:
            with self.assertRaises(ValidationError):
                tx.save(replace(session(), id="duplicate"), expected=first)
            with self.assertRaises(ValidationError):
                tx.save(replace(session(), stream_id="missing"), expected=first)
            self.assertEqual(tx.read(), first)

    def test_domain_operation_runs_inside_the_same_transaction_as_persistence(self):
        self.create(stream())
        authority = Authority(host_id="laptop", token="term-1")
        op = Operation(id="join", before=None, after=session(), actor="automatic", ownership_token="term-1")
        with self.store.transaction() as tx:
            result, snapshot = apply_operation(tx, op, authority=authority)
            self.assertEqual(result.outcome, "changes")
            self.assertEqual(snapshot.catalog.sessions, (session(),))
            retry, repeated = apply_operation(tx, op, authority=authority)
            self.assertEqual(retry.outcome, "no-change")
            self.assertEqual(snapshot.revisions, repeated.revisions)
        with self.store.transaction() as tx:
            result, snapshot = apply_operation(tx, update("report", session(), {"state": "Ready"},
                                                         actor="automatic", ownership_token="term-1"))
            self.assertEqual(result.outcome, "superseded")
            self.assertEqual(snapshot.catalog.sessions[0].state, "")

    def test_closed_transaction_cannot_be_reused(self):
        with self.store.transaction() as tx:
            snapshot = tx.read()
        with self.assertRaises(RuntimeError):
            tx.read()
        with self.assertRaises(RuntimeError):
            tx.save(stream(), expected=snapshot)

    def test_ids_cannot_escape_the_store_and_are_round_trippable(self):
        effort = stream(id="../outside")
        self.create(effort)
        self.create(session(host_id="..", stream_id=effort.id, id="nested/name"))
        with self.store.transaction() as tx:
            self.assertEqual(tx.read().catalog.streams[0], effort)
        self.assertFalse((self.root.parent / "outside.yaml").exists())
        for path in self.root.rglob("*.yaml"):
            self.assertTrue(path.resolve().is_relative_to(self.root.resolve()))

    def test_case_and_unicode_distinct_ids_never_alias_physical_records(self):
        ids = ("stream-a", "STREAM-A", "café", "cafe\u0301")
        for id in ids:
            self.create(stream(id=id))
        for host in ("laptop", "LAPTOP"):
            for id in ("session-a", "SESSION-A"):
                self.create(session(host_id=host, id=id, stream_id="stream-a", harness_session_id=id))
        with self.store.transaction() as tx:
            snapshot = tx.read()
        self.assertEqual({item.id for item in snapshot.catalog.streams}, set(ids))
        self.assertEqual(len(snapshot.catalog.sessions), 4)
        self.assertEqual(len(list((self.root / "streams").iterdir())), 4)
        self.assertEqual(len(list((self.root / "sessions").iterdir())), 2)

    def test_confirmation_read_failure_after_replace_is_uncertain(self):
        snapshot = self.create(stream())
        read_bytes = Path.read_bytes
        calls = 0

        def fail_confirmation(path):
            nonlocal calls
            calls += 1
            if calls == 2:
                raise OSError("post-replace read failed")
            return read_bytes(path)

        with self.store.transaction() as tx, patch.object(Path, "read_bytes", fail_confirmation):
            with self.assertRaises(StorageError) as raised:
                tx.save(replace(stream(), title="Saved"), expected=snapshot)
        self.assertTrue(raised.exception.uncertain)
        self.assertIsNotNone(raised.exception.__cause__)
        with self.store.transaction() as tx:
            self.assertEqual(tx.read().catalog.streams[0].title, "Saved")

    def test_interruption_before_replace_does_not_block_subsequent_reads_and_saves(self):
        self.create(stream())
        code = """
from dataclasses import replace
from pathlib import Path
import os
import sys
sys.path.insert(0, sys.argv[1])
from _streams.catalog.yaml_store import YamlStore
with YamlStore(Path(sys.argv[2])).transaction() as tx:
    snapshot = tx.read()
    os.replace = lambda *args: os._exit(0)
    tx.save(replace(snapshot.catalog.streams[0], title='Interrupted'), expected=snapshot)
"""
        process = subprocess.run([sys.executable, "-c", code, str(Path(__file__).resolve().parents[2] / "src"),
                                  str(self.root)], capture_output=True, timeout=30)
        self.assertEqual(process.returncode, 0, process.stderr)
        with self.store.transaction() as tx:
            snapshot = tx.read()
            self.assertEqual(snapshot.catalog.streams[0], stream())
            tx.save(replace(stream(), title="After interruption"), expected=snapshot)
        with self.store.transaction() as tx:
            self.assertEqual(tx.read().catalog.streams[0].title, "After interruption")

    def test_failed_replace_preserves_record_and_cleans_up_temporary_file(self):
        original = self.create(stream())
        path = self.root / "streams/stream-1.yaml"
        contents = path.read_bytes()
        with self.store.transaction() as tx, patch("_streams.catalog.yaml_store.os.replace", side_effect=OSError("disk full")):
            with self.assertRaises(StorageError) as raised:
                tx.save(replace(stream(), title="Changed"), expected=original)
        self.assertFalse(raised.exception.uncertain)
        self.assertEqual(path.read_bytes(), contents)
        self.assertEqual(list(path.parent.iterdir()), [path])

    def test_post_replace_failure_is_reported_as_uncertain_not_unsaved(self):
        snapshot = self.create(stream())
        with self.store.transaction() as tx, patch("_streams.catalog.yaml_store._sync_directory", side_effect=OSError("I/O")):
            with self.assertRaises(StorageError) as raised:
                tx.save(replace(stream(), title="Changed"), expected=snapshot)
        self.assertTrue(raised.exception.uncertain)
        with self.store.transaction() as tx:
            self.assertEqual(tx.read().catalog.streams[0].title, "Changed")

    def test_corrupt_or_unrecognised_records_fail_closed_without_overwriting(self):
        self.create(stream())
        path = self.root / "streams/stream-1.yaml"
        original = path.read_text()
        bad_values = ["[not, a, record]", "version: 99\n", original.replace("version: 1", "version: 2"),
                      original + "version: 1\n", original.replace("id: stream-1", "id: another"),
                      original + "unexpected: field\n", "!!python/object:danger {}"]
        for value in bad_values:
            with self.subTest(value=value):
                path.write_text(value)
                with self.store.transaction() as tx:
                    with self.assertRaises(StorageError):
                        tx.read()
                self.assertEqual(path.read_text(), value)
        path.write_text(original)
        (self.root / "streams/ignored.json").write_text("{}")
        with self.store.transaction() as tx, self.assertRaises(StorageError):
            tx.read()

    def test_symlinked_records_directories_and_lock_files_are_rejected(self):
        outside = Path(self.tmp.name) / "outside"
        outside.mkdir()
        self.root.mkdir()
        directory = self.root / "streams"
        directory.symlink_to(outside, target_is_directory=True)
        with self.store.transaction() as tx, self.assertRaises(StorageError):
            tx.read()
        directory.unlink()
        self.create(stream())
        path = self.root / "streams/stream-1.yaml"
        contents = path.read_text()
        path.unlink()
        target = outside / "record.yaml"
        target.write_text(contents)
        path.symlink_to(target)
        with self.store.transaction() as tx, self.assertRaises(StorageError):
            tx.read()
        path.unlink()
        lock = self.root / ".catalog.lock"
        lock.unlink()
        lock.symlink_to(target)
        with self.assertRaises(StorageError):
            with self.store.transaction():
                pass
        self.assertEqual(target.read_text(), contents)

    def test_second_process_stays_blocked_after_save_until_scope_exit(self):
        self.create(stream())
        code = """
from pathlib import Path
import sys
sys.path.insert(0, sys.argv[1])
from _streams.catalog.yaml_store import YamlStore
print('waiting', flush=True)
with YamlStore(Path(sys.argv[2])).transaction():
    print('acquired', flush=True)
"""
        with self.store.transaction() as tx:
            tx.save(replace(stream(), title="Saved"), expected=tx.read())
            process = subprocess.Popen([sys.executable, "-c", code, str(Path(__file__).resolve().parents[2] / "src"),
                                        str(self.root)], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            self.addCleanup(lambda: process.kill() if process.poll() is None else None)
            self.assertEqual(process.stdout.readline().strip(), "waiting")
            with self.assertRaises(subprocess.TimeoutExpired):
                process.communicate(timeout=0.2)
        out, err = process.communicate(timeout=10)
        self.assertEqual((process.returncode, out.strip(), err), (0, "acquired", ""))

    def test_concurrent_processes_serialize_read_reconcile_and_save(self):
        self.create(stream())
        code = """
from pathlib import Path
import sys
sys.path.insert(0, sys.argv[1])
from _streams.catalog.yaml_store import YamlStore
from _streams.catalog.model import Link
from _streams.catalog.operations import update
from _streams.catalog.storage import apply_operation
with YamlStore(Path(sys.argv[2])).transaction() as tx:
    before = tx.read().catalog.streams[0]
    op = update(sys.argv[3], before, {'issues': before.issues + (Link(url=sys.argv[3], description='Issue'),)})
    result, _ = apply_operation(tx, op)
    assert result.outcome == 'changes', result
"""
        processes = [subprocess.Popen([sys.executable, "-c", code, str(Path(__file__).resolve().parents[2] / "src"),
                                       str(self.root), f"https://example.org/{i}"], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                     for i in range(4)]
        for process in processes:
            out, err = process.communicate(timeout=30)
            self.assertEqual(process.returncode, 0, (out, err))
        with self.store.transaction() as tx:
            self.assertEqual(len(tx.read().catalog.streams[0].issues), 4)


if __name__ == "__main__":
    unittest.main()
