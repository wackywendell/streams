from __future__ import annotations

from dataclasses import FrozenInstanceError, replace
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog.model import Checkout, Document, Host, Repository, ValidationError, record_key
from _streams.catalog.operations import Authority, update
from _streams.catalog.storage import StorageError
from _streams.catalog.sync.baseline import BASELINE, Baseline, decode_baseline, encode_baseline
from _streams.catalog.sync.envelope import decode_fingerprints, decode_operation, encode_operation, operation_digest
from _streams.catalog.sync.history import History
from _streams.catalog.sync.local import RepairRequired
from _streams.catalog.sync.replay import replay
from _streams.catalog.yaml_store import YamlStore
from test_catalog_history import encoded
from test_catalog_sync import contribution, create, effort


def snapshot(*records):
    return {YamlStore.record_path(record_key(record)).as_posix(): encoded(record) for record in records}


class CatalogBaselineTest(unittest.TestCase):
    def setUp(self):
        self.creation = create()
        self.origin = {"host": "laptop", "session": "original"}
        self.envelope = encode_operation(self.creation, origin=self.origin)
        self.park = update("park", effort(), {"status": "parked"})
        self.fingerprints = {op.id: operation_digest(op) for op in (self.creation, self.park, contribution())}
        self.baseline = Baseline("laptop", {"work": self.envelope}, self.fingerprints)
        self.current = replace(effort(), title="New title", status="done", archived=True)
        self.files = snapshot(self.current, contribution().after,
                              Host("laptop", "Laptop", datetime(2026, 1, 1, tzinfo=timezone.utc)))

    def message(self, **changes):
        data = json.loads(encode_baseline(self.baseline)[len(BASELINE):])
        data.update(changes)
        return BASELINE + json.dumps(data) + "\n"

    def test_mutated_snapshot_preserves_original_creation_and_origin_free_digest(self):
        baseline, catalog, authorities = decode_baseline(encode_baseline(self.baseline), self.files)
        self.assertEqual(baseline, self.baseline)
        self.assertEqual(catalog.stream("work"), self.current)
        self.assertEqual(authorities, {"work": Authority("laptop", "create")})
        self.assertEqual(decode_operation(baseline.creations["work"]), (self.creation, self.origin))
        self.assertEqual(baseline.fingerprints["create"],
                         hashlib.sha256(encode_operation(self.creation).encode()).hexdigest())
        self.assertNotEqual(baseline.fingerprints["create"], hashlib.sha256(self.envelope.encode()).hexdigest())
        self.assertEqual(baseline.fingerprints, self.fingerprints)
        history = History(root="root", baseline=baseline, catalog=catalog, authorities=authorities)
        self.assertEqual(history.operations, {})
        self.assertEqual(history.fingerprints, self.fingerprints)
        with self.assertRaises(TypeError):
            authorities["other"] = Authority("laptop", "another")

    def test_creation_optional_fields_and_captured_inputs_keep_original_bytes(self):
        stream = replace(effort(), repositories={"src": Repository("https://example.org/repo")},
                         documents=(Document("src", "main", "design.md", "Design"),))
        captured = Host("laptop", "Original laptop", datetime(2026, 1, 1, tzinfo=timezone.utc))
        operation = replace(create(stream), inputs=(captured,))
        envelope = encode_operation(operation, origin=self.origin)
        baseline = Baseline("laptop", {"work": envelope}, {operation.id: operation_digest(operation)})
        decoded, _, _ = decode_baseline(encode_baseline(baseline), snapshot(replace(stream, title="New")))
        self.assertEqual(decoded.creations["work"], envelope)
        self.assertNotIn('"added_at"', envelope)
        self.assertEqual(decode_operation(decoded.creations["work"])[0].inputs, (captured,))
        self.assertNotEqual(operation_digest(operation), operation_digest(replace(operation, inputs=())))
        explicit_null = envelope.replace('"description":"Design"', '"added_at":null,"description":"Design"')
        with self.assertRaises(ValueError):
            decode_baseline(encode_baseline(replace(baseline, creations={"work": explicit_null})), snapshot(stream))
        explicit_empty = self.envelope.replace('"kind":"stream"', '"inputs":[],"kind":"stream"')
        with self.assertRaises(ValueError):
            decode_baseline(self.message(creations={"work": explicit_empty}), self.files)

    def test_baseline_and_history_own_immutable_maps(self):
        creations, fingerprints = {"work": self.envelope}, dict(self.fingerprints)
        baseline = Baseline("laptop", creations, fingerprints)
        history = History(baseline=baseline, fingerprints=fingerprints)
        creations.clear()
        fingerprints.clear()
        self.assertEqual(baseline.creations, {"work": self.envelope})
        self.assertEqual(history.fingerprints, self.fingerprints)
        for mapping in (baseline.creations, baseline.fingerprints, history.fingerprints):
            with self.assertRaises(TypeError):
                mapping["injected"] = "bad"
        with self.assertRaises(FrozenInstanceError):
            baseline.host = "changed"

    def test_domain_host_identifiers_are_not_installation_uuids(self):
        host = "My laptop / É"
        operation = create(effort(host=host))
        baseline = Baseline(host, {"work": encode_operation(operation)}, {operation.id: operation_digest(operation)})
        self.assertEqual(decode_baseline(encode_baseline(baseline), snapshot(operation.after))[0].host, host)

    def test_strict_fingerprint_map_and_owned_copy(self):
        original = dict(self.fingerprints)
        decoded = decode_fingerprints(original)
        original.clear()
        self.assertEqual(decoded, self.fingerprints)
        for value in (None, [], "{}", {"": "a" * 64}, {" ": "a" * 64}, {"nul\x00": "a" * 64},
                      {1: "a" * 64}, {"id": "A" * 64}, {"id": "a" * 63}, {"id": "a" * 65},
                      {"id": "g" * 64}, {"id": None}, {"id": "a" * 64 + "\n"}):
            with self.subTest(value=value), self.assertRaises(ValueError):
                decode_fingerprints(value)

    def test_manifest_is_strict(self):
        invalid = ["other\n", BASELINE + "[]", BASELINE + "{", BASELINE + '{"version":1,"version":1}',
                   self.message(version=True), self.message(version=2), self.message(extra="unknown"),
                   self.message(host=""), self.message(host=" \t"), self.message(host="nul\x00"),
                   self.message(host=1), self.message(creations=[]), self.message(fingerprints=[]),
                   self.message(creations={"": self.envelope})]
        data = json.loads(self.message()[len(BASELINE):])
        for key in data:
            invalid.append(BASELINE + json.dumps({k: v for k, v in data.items() if k != key}))
        for message in invalid:
            with self.subTest(message=message), self.assertRaises(ValueError):
                decode_baseline(message, self.files)

    def test_creation_keys_must_biject_snapshot_streams(self):
        for creations in ({}, {"other": self.envelope}, {"work": self.envelope, "extra": self.envelope}):
            with self.subTest(creations=creations), self.assertRaises(ValidationError):
                decode_baseline(self.message(creations=creations), self.files)

    def test_creation_identity_owner_and_shape_are_checked(self):
        for operation in (create(effort("other")), create(effort(host="other")), contribution(),
                          update("edit", effort(), {"status": "done"})):
            message = self.message(creations={"work": encode_operation(operation)},
                                   fingerprints={operation.id: operation_digest(operation)})
            with self.subTest(operation=operation), self.assertRaises(ValidationError):
                decode_baseline(message, self.files)

    def test_creation_envelopes_must_be_canonical_and_digest_must_match(self):
        data = json.loads(self.envelope.split("\n", 2)[2])
        for envelope in (self.envelope + "\n", self.envelope.replace('"version":1', '"version":1 '),
                         self.envelope.split("\n", 2)[0] + "\n\n" + json.dumps(data) + "\n",
                         self.envelope.replace('"origin":{', '"unknown":0,"origin":{')):
            with self.subTest(envelope=envelope), self.assertRaises(ValueError):
                decode_baseline(self.message(creations={"work": envelope}), self.files)
        for digest in ("0" * 64, hashlib.sha256(self.envelope.encode()).hexdigest(), "A" * 64):
            with self.subTest(digest=digest), self.assertRaises(ValueError):
                decode_baseline(self.message(fingerprints={"create": digest}), self.files)
        with self.assertRaises(ValueError):
            decode_baseline(self.message(fingerprints={}), self.files)

    def test_snapshot_producers_paths_and_full_graph_are_checked(self):
        message = encode_baseline(self.baseline)
        foreign = (replace(self.current, owner_host_id="other"),
                   replace(contribution().after, host_id="other"),
                   Host("other", "Other", datetime(2026, 1, 1, tzinfo=timezone.utc)))
        for record in foreign:
            records = [self.current] if record_key(record).kind != "stream" else []
            with self.subTest(record=record), self.assertRaises(ValueError):
                decode_baseline(message, snapshot(*records, record))
        for session in (replace(contribution().after, stream_id="missing"),
                        replace(contribution().after, checkout=Checkout("missing", "main"))):
            with self.subTest(session=session), self.assertRaises(StorageError):
                decode_baseline(message, snapshot(self.current, session))
        for files in ({"streams/wrong.yaml": encoded(self.current)}, {**self.files, "unknown.txt": b"x"}):
            with self.subTest(files=files), self.assertRaises(StorageError):
                decode_baseline(message, files)

    def test_manual_history_derives_operation_digests_and_rejects_inconsistent_ledger(self):
        operation = contribution()
        history = History(operations={operation.id: (operation, {})})
        self.assertEqual(history.fingerprints, {operation.id: operation_digest(operation)})
        with self.assertRaises(ValueError):
            History(operations={operation.id: (operation, {})}, fingerprints={operation.id: "0" * 64})
        with self.assertRaises(ValueError):
            History(baseline=self.baseline, fingerprints={"create": "0" * 64})
        with self.assertRaises(ValueError):
            History(operations={"wrong-key": (operation, {})})

    def test_replay_requires_shared_baseline_root_but_keeps_empty_root_reconciliation(self):
        baseline, catalog, authorities = decode_baseline(encode_baseline(self.baseline), self.files)
        imported = History(root="root", baseline=baseline, catalog=catalog, authorities=authorities)
        for left, right in ((imported, History()), (History(), imported),
                            (imported, replace(imported, root="different")),
                            (replace(imported, root=None), replace(imported, root=None))):
            with self.subTest(left=left.root, right=right.root), self.assertRaises(RepairRequired):
                replay(left, right)
        self.assertEqual(replay(imported, imported).changes, ())
        local = History(root="empty-one", operations={"create": (self.creation, {})})
        self.assertEqual(len(replay(local, History(root="empty-two")).changes), 1)

    def test_replay_skips_retired_intent_and_rejects_changed_intent(self):
        baseline, catalog, authorities = decode_baseline(encode_baseline(self.baseline), self.files)
        remote = History(root="root", baseline=baseline, catalog=catalog, authorities=authorities)
        # A hand-built operation history remains compatible with the complete ledger.
        local = replace(remote, operations={self.park.id: (self.park, {})})
        self.assertEqual(replay(local, remote).changes, ())
        remote = replace(remote, baseline=None)
        for operation, error in ((self.park, False), (replace(self.park, after=effort()), True)):
            local = History(root="root", operations={operation.id: (operation, {})})
            if error:
                with self.assertRaises(RepairRequired):
                    replay(local, remote)
            else:
                self.assertEqual(replay(local, remote).changes, ())
                self.assertEqual(replay(local, remote).catalog, catalog)


if __name__ == "__main__":
    unittest.main()
