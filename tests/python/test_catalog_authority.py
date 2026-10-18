from __future__ import annotations

from dataclasses import replace
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog.model import Catalog, ValidationError
from _streams.catalog.operations import Authority, Operation, reconcile, update
from _streams.catalog.sync import LocalCatalog, decode_operation, encode_operation
from _streams.catalog.yaml_store import YamlStore
from test_catalog_model import stream


class StreamInferenceAuthorityTest(unittest.TestCase):
    def test_stream_inference_requires_a_captured_ownership_period(self):
        for token in (None, "", " "):
            with self.subTest(token=token), self.assertRaises(ValidationError):
                update("infer", stream(), {"goal": "Refined"}, actor="automatic", ownership_token=token)

    def test_same_record_in_a_new_ownership_period_does_not_accept_old_inference(self):
        before = stream()
        operation = update("infer", before, {"goal": "Refined"}, actor="automatic", ownership_token="term-1")
        for current in (before, operation.after):
            for authority in (None, Authority("server", "term-1"), Authority("laptop", "term-3")):
                with self.subTest(current=current, authority=authority):
                    result = reconcile(Catalog(streams=(current,)), operation, authority=authority)
                    self.assertEqual(result.outcome, "superseded")
                    self.assertIn("ownership", result.conflicts)
        result = reconcile(Catalog(streams=(before,)), operation, authority=Authority("laptop", "term-1"))
        self.assertEqual(result.outcome, "changes")
        self.assertEqual(reconcile(Catalog(streams=(result.record,)), operation,
                                   authority=Authority("laptop", "term-1")).outcome, "no-change")
        self.assertEqual(decode_operation(encode_operation(operation))[0], operation)

    def test_ordinary_user_management_and_initial_creation_need_no_execution_authority(self):
        operation = update("user", stream(), {"status": "parked"})
        self.assertEqual(reconcile(Catalog(streams=(stream(),)), operation).outcome, "changes")
        creation = Operation("create", None, stream(), "automatic")
        self.assertEqual(reconcile(Catalog(), creation).outcome, "changes")
        for operation in (creation, update("user", stream(), {"title": "Explicit"})):
            with self.subTest(operation=operation), self.assertRaises(ValidationError):
                replace(operation, ownership_token="term")

    def test_local_backend_supplies_authority_but_cannot_impersonate_another_host(self):
        with tempfile.TemporaryDirectory() as root:
            store = YamlStore(Path(root))
            local = LocalCatalog(store, host_id="laptop")
            local.save(Operation("create", None, stream(), "user"))
            operation = update("infer", stream(), {"goal": "Refined"}, actor="automatic", ownership_token="create")
            self.assertEqual(local.save(operation).domain.outcome, "changes")
            other = LocalCatalog(store, host_id="server")
            with self.assertRaises(ValidationError):
                other.save(update("foreign", operation.after, {"goal": "Impersonated"},
                                  actor="automatic", ownership_token="create"))
            user_edit = update("foreign-user", operation.after, {"status": "waiting"})
            self.assertEqual(other.save(user_edit).domain.outcome, "changes")


if __name__ == "__main__":
    unittest.main()
