from __future__ import annotations

from dataclasses import replace
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog.codec import record_to_data
from _streams.catalog.model import Catalog, ValidationError
from _streams.catalog.operations import Authority, Operation, reconcile, update
from _streams.catalog.sync.envelope import PREFIX, decode_operation, encode_operation
from _streams.catalog.sync.local import State
from test_catalog_model import session, stream


class CapturedInputsTest(unittest.TestCase):
    def test_automatic_report_requires_the_captured_stream_not_just_its_own_record(self):
        effort, contribution = stream(), session()
        report = update("report", contribution, {"state": "Design ready"}, actor="automatic",
                        ownership_token="term", inputs=(effort,))
        authority = Authority("laptop", "term")
        catalog = Catalog(streams=(effort,), sessions=(contribution,))
        self.assertEqual(reconcile(catalog, report, authority=authority).outcome, "changes")
        for changed in (replace(effort, goal="A different goal"), replace(effort, status="parked")):
            with self.subTest(changed=changed):
                result = reconcile(replace(catalog, streams=(changed,)), report, authority=authority)
                self.assertEqual(result.outcome, "superseded")
                self.assertIn("inputs", result.conflicts)
        self.assertEqual(report.inputs, (effort,))

    def test_stale_inputs_are_checked_before_already_satisfied_target_shortcut(self):
        effort, contribution = stream(), session()
        for actor, expected in (("automatic", "superseded"), ("user", "conflict")):
            with self.subTest(actor=actor):
                operation = update("edit", effort, {"title": "New title"}, actor=actor, inputs=(contribution,))
                current = Catalog(streams=(operation.after,), sessions=(replace(contribution, state="Changed"),))
                self.assertEqual(reconcile(current, operation).outcome, expected)

    def test_missing_captured_record_does_not_become_an_unconditional_edit(self):
        effort = stream()
        operation = update("edit", effort, {"title": "New title"}, inputs=(session(),))
        self.assertEqual(reconcile(Catalog(streams=(effort,)), operation).outcome, "conflict")

    def test_unrelated_catalog_edits_do_not_invalidate_captured_inputs(self):
        effort, contribution = stream(), session()
        operation = update("edit", effort, {"title": "New title"}, inputs=(contribution,))
        other = stream(id="other", goal="Unrelated")
        result = reconcile(Catalog(streams=(effort, other), sessions=(contribution,)), operation)
        self.assertEqual(result.outcome, "changes")

    def test_input_records_are_an_immutable_unique_typed_collection(self):
        for inputs in ([stream()], (stream(),), (None,), ({"id": "stream-1"},),
                       (stream(), stream()), (stream(), stream(title="Conflicting input"))):
            with self.subTest(inputs=inputs), self.assertRaises(ValidationError):
                Operation("bad", stream(), stream(title="New"), "user", inputs=inputs)


class InputEnvelopeTest(unittest.TestCase):
    def test_guarded_envelope_retains_record_kinds_and_exact_inputs(self):
        operation = update("edit", stream(), {"title": "New"}, inputs=(stream(id="source"), session()))
        encoded = encode_operation(operation, origin={"harness": "pi"})
        decoded, origin = decode_operation(encoded)
        self.assertEqual(decoded, operation)
        self.assertEqual(origin, {"harness": "pi"})
        data = json.loads(encoded[len(PREFIX):])
        self.assertEqual([item["kind"] for item in data["inputs"]], ["stream", "session"])
        self.assertEqual(encode_operation(decoded, origin=origin), encoded)

    def test_empty_inputs_do_not_change_existing_envelope_bytes_or_retry_digest(self):
        operation = update("edit", stream(), {"title": "New"})
        data = {"version": 1, "id": operation.id, "kind": "stream", "before": record_to_data(operation.before),
                "after": record_to_data(operation.after), "actor": "user", "ownership_token": None, "origin": {}}
        original = PREFIX + json.dumps(data, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n"
        self.assertEqual(operation.inputs, ())
        self.assertEqual(encode_operation(operation), original)
        self.assertEqual(decode_operation(original)[0], operation)

    def test_changed_guards_change_the_retry_digest_even_when_target_is_identical(self):
        operation = update("edit", stream(), {"title": "New"}, inputs=(session(),))
        changed = replace(operation, inputs=(session(state="New evidence"),))
        self.assertNotEqual(State._digest(operation), State._digest(changed))

    def test_malformed_duplicate_or_unknown_guard_input_is_not_ignored(self):
        data = json.loads(encode_operation(update("edit", stream(), {"title": "New"}))[len(PREFIX):])
        item = {"kind": "session", "record": record_to_data(session())}
        for inputs in (None, {}, [item, item], [{"kind": "alien", "record": {}}],
                       [{"kind": "session"}], [{**item, "unexpected": True}]):
            with self.subTest(inputs=inputs), self.assertRaises(ValidationError):
                decode_operation(PREFIX + json.dumps({**data, "inputs": inputs}))


if __name__ == "__main__":
    unittest.main()
