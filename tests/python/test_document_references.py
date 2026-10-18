from __future__ import annotations

from copy import deepcopy
from dataclasses import replace
import hashlib
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import Mock

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog.codec import record_from_data, record_to_data
from _streams.catalog.model import Catalog, ValidationError
from _streams.catalog.operations import reconcile, update
from _streams.catalog.storage import StorageError
from _streams.catalog.sync.envelope import BOOTSTRAP, PREFIX, decode_operation, encode_operation
from _streams.catalog.sync.history import read_history
from _streams.catalog.sync.local import RepairRequired
from _streams.catalog.yaml_store import YamlStore


def document_data():
    return {"repository": "code", "ref": "feature", "path": "docs/design.md", "description": "Design"}


def stream_data(id="stream-1"):
    # The pre-provenance physical shape, independent of the current encoder.
    return {"id": id, "title": "Catalog", "goal": None, "owner": {"host_id": "laptop"},
            "title_mode": "automatic", "goal_short": None, "goal_mode": "automatic",
            "status": "active", "archived": False,
            "repositories": {"code": {"url": "https://example.org/code"}},
            "documents": [document_data()], "issues": [], "prs": []}


def envelope_data():
    before = stream_data()
    after = {**deepcopy(before), "status": "parked"}
    return {"version": 1, "id": "edit", "kind": "stream", "before": before, "after": after,
            "actor": "user", "ownership_token": None, "origin": {"source": "test"},
            "inputs": [{"kind": "stream", "record": stream_data("evidence")}]}


def message(data):
    return PREFIX + json.dumps(data, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n"


def physical_record(data):
    return yaml.safe_dump({"version": 1, "revision": "original", "kind": "stream", "record": data}).encode()


class DocumentReferencesTest(unittest.TestCase):
    def setUp(self):
        self.before = record_from_data("stream", stream_data())
        self.document = self.before.documents[0]

    def test_identity_excludes_recovery_ref_description_and_provenance(self):
        changed = replace(self.document, ref="other", description="Updated", added_at="commit:opaque")
        self.assertEqual(self.document.identity, ("code", "docs/design.md"))
        self.assertEqual(changed.identity, self.document.identity)
        with self.assertRaisesRegex(ValidationError, "duplicate documents"):
            replace(self.before, documents=(self.document, changed))
        for other in (replace(self.document, path="docs/other.md"),
                      replace(self.document, repository="other")):
            with self.subTest(other=other):
                repositories = {**self.before.repositories, "other": self.before.repositories["code"]}
                effort = replace(self.before, repositories=repositories, documents=(self.document, other))
                self.assertEqual(len(effort.documents), 2)

    def test_provenance_is_optional_opaque_nonempty_text(self):
        self.assertIsNone(self.document.added_at)
        for value in ("", " \t\n", 42, False, [], {}, "bad\x00commit"):
            with self.subTest(value=value), self.assertRaisesRegex(ValidationError, "document.added_at"):
                replace(self.document, added_at=value)
        for value in ("commit:opaque", "sha256:not-a-git-hash", "非Git commit"):
            with self.subTest(value=value):
                document = replace(self.document, added_at=value)
                effort = replace(self.before, documents=(document,))
                data = record_to_data(effort)
                self.assertEqual(data["documents"][0]["added_at"], value)
                self.assertEqual(record_from_data("stream", data), effort)
                self.assertEqual(YamlStore.decode_record("streams/stream-1.yaml", physical_record(data)), effort)

    def test_absent_provenance_preserves_old_physical_shape(self):
        self.assertEqual(record_to_data(self.before), stream_data())
        explicit_none = stream_data()
        explicit_none["documents"][0]["added_at"] = None
        self.assertEqual(record_from_data("stream", explicit_none), self.before)
        self.assertEqual(record_to_data(record_from_data("stream", explicit_none)), stream_data())
        self.assertEqual(YamlStore.decode_record("streams/stream-1.yaml", physical_record(stream_data())), self.before)

    def test_document_decoding_remains_strict(self):
        for changes in ({"added_at": ""}, {"added_at": 17}, {"unexpected": "value"}, {"ref": None}):
            data = stream_data()
            data["documents"][0].update(changes)
            with self.subTest(changes=changes):
                with self.assertRaises(ValidationError):
                    record_from_data("stream", data)
                with self.assertRaises(StorageError):
                    YamlStore.decode_record("streams/stream-1.yaml", physical_record(data))

    def test_reference_metadata_edits_are_guarded_replacements(self):
        for changes in ({"ref": "recovered"}, {"description": "New purpose"},
                        {"added_at": "commit:opaque"},
                        {"ref": "recovered", "description": "New purpose", "added_at": "commit:opaque"}):
            with self.subTest(changes=changes):
                edited = replace(self.document, **changes)
                operation = update("edit-document", self.before, {"documents": (edited,)})
                result = reconcile(Catalog(streams=(self.before,)), operation)
                self.assertEqual((result.outcome, result.record.documents), ("changes", (edited,)))
                self.assertEqual(reconcile(Catalog(streams=(result.record,)), operation).outcome, "no-change")
                # Unrelated stream edits still combine, without creating another document.
                current = replace(self.before, status="waiting")
                result = reconcile(Catalog(streams=(current,)), operation)
                self.assertEqual(result.record, replace(current, documents=(edited,)))
        with_provenance = replace(self.before, documents=(replace(self.document, added_at="commit:old"),))
        operation = update("clear-provenance", with_provenance, {"documents": (self.document,)})
        self.assertEqual(reconcile(Catalog(streams=(with_provenance,)), operation).record, self.before)

    def test_divergent_reference_edits_conflict_without_rewriting_intent(self):
        operation = update("edit-document", self.before,
                           {"documents": (replace(self.document, ref="desired"),)})
        encoded = encode_operation(operation)
        for changes in ({"ref": "concurrent"}, {"description": "Concurrent purpose"},
                        {"added_at": "commit:concurrent"}):
            with self.subTest(changes=changes):
                current = replace(self.before, documents=(replace(self.document, **changes),))
                result = reconcile(Catalog(streams=(current,)), operation)
                self.assertEqual((result.outcome, result.conflicts), ("conflict", ("documents",)))
                self.assertEqual(encode_operation(operation), encoded)

    def test_independent_additions_merge_but_same_identity_different_metadata_conflicts(self):
        before = replace(self.before, documents=())
        operation = update("add-document", before, {"documents": (self.document,)})
        independent = replace(self.document, path="docs/independent.md")
        current = replace(before, documents=(independent,))
        result = reconcile(Catalog(streams=(current,)), operation)
        self.assertEqual((result.outcome, result.record.documents), ("changes", (independent, self.document)))
        for changes in ({"ref": "other"}, {"description": "Other purpose"}, {"added_at": "commit:other"}):
            with self.subTest(changes=changes):
                current = replace(before, documents=(replace(self.document, **changes),))
                result = reconcile(Catalog(streams=(current,)), operation)
                self.assertEqual((result.outcome, result.conflicts), ("conflict", ("documents",)))

    def test_envelope_round_trip_preserves_old_canonical_bytes_and_digest(self):
        original = message(envelope_data())
        operation, origin = decode_operation(original)
        self.assertIsNone(operation.before.documents[0].added_at)
        self.assertIsNone(operation.after.documents[0].added_at)
        self.assertIsNone(operation.inputs[0].documents[0].added_at)
        encoded = encode_operation(operation, origin=origin)
        self.assertEqual(encoded, original)
        self.assertEqual(hashlib.sha256(encoded.encode()).digest(), hashlib.sha256(original.encode()).digest())
        self.assertNotIn("added_at", encoded)

    def test_provenance_round_trips_in_all_envelope_records(self):
        data = envelope_data()
        for record in (data["before"], data["after"], data["inputs"][0]["record"]):
            record["documents"][0]["added_at"] = "commit:known"
        original = message(data)
        operation, origin = decode_operation(original)
        self.assertEqual(encode_operation(operation, origin=origin), original)

    def test_original_duplicate_data_fails_without_deduplication(self):
        data = stream_data()
        data["documents"].append({**document_data(), "ref": "different-branch"})
        with self.assertRaisesRegex(StorageError, "duplicate documents"):
            YamlStore.decode_record("streams/stream-1.yaml", physical_record(data))
        for location in ("before", "after", "inputs"):
            envelope = envelope_data()
            record = envelope[location] if location != "inputs" else envelope["inputs"][0]["record"]
            record["documents"] = deepcopy(data["documents"])
            original = message(envelope)
            with self.subTest(location=location), self.assertRaisesRegex(ValidationError, "duplicate documents"):
                decode_operation(original)
            self.assertEqual(message(envelope), original)

    def test_historical_duplicate_intent_or_materialization_requires_repair(self):
        data = stream_data()
        creation = {**envelope_data(), "before": None, "after": data, "inputs": []}
        duplicate = deepcopy(data)
        duplicate["documents"].append({**document_data(), "ref": "other-branch"})
        for invalid_intent in (False, True):
            with self.subTest(invalid_intent=invalid_intent):
                envelope = {**creation, "after": duplicate if invalid_intent else data}
                git = Mock()
                git.histories = {}
                git.run.return_value = ("root\0\0" + BOOTSTRAP + "\0tip\0root\0" + message(envelope) + "\0").encode()
                git.files.side_effect = [{}, {"streams/stream-1.yaml": physical_record(duplicate)}]
                with self.assertRaisesRegex(RepairRequired, "duplicate documents"):
                    read_history(git, "tip")
                self.assertNotIn("tip", git.histories)


if __name__ == "__main__":
    unittest.main()
