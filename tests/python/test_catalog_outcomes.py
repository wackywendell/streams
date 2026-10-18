from dataclasses import FrozenInstanceError
import json
from pathlib import Path
import sys
from types import MappingProxyType
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog.operations import Reconciliation
from _streams.catalog.sync import Result
from _streams.cli import result_data


class CatalogOutcomesTest(unittest.TestCase):
    def test_only_reconciled_saves_can_be_accepted(self):
        for sharing in ("local-only", "pending", "confirmed", "failed", "blocked", "uncertain"):
            with self.subTest(sharing=sharing):
                self.assertFalse(Result(sharing).accepted)
                for outcome in ("changes", "no-change", "superseded", "conflict"):
                    domain = Reconciliation("operation", outcome)
                    result = Result(sharing, domain=domain)
                    self.assertEqual(result.accepted, outcome in ("changes", "no-change"))
                    self.assertEqual(result.needs_attention, sharing in ("failed", "blocked", "uncertain"))
                    repair = Result(sharing, local="repair-required", domain=domain)
                    self.assertFalse(repair.accepted)
                    self.assertTrue(repair.requires_repair)
                    self.assertTrue(repair.needs_attention)

    def test_unknown_outcomes_cannot_escape_the_result_boundary(self):
        for fields in ({"sharing": "offline"}, {"sharing": "pending", "local": "failed"}):
            with self.subTest(fields=fields), self.assertRaises(ValueError):
                Result(**fields)
        with self.assertRaises(FrozenInstanceError):
            Result("pending").local = "repair-required"

    def test_conflict_origin_mappings_are_materialized_only_at_output(self):
        origin = MappingProxyType({"host": "laptop", "session": "pi-conversation"})
        result = Result("blocked", conflicts=({"operation": "edit", "origin": origin},))
        data = json.loads(json.dumps(result_data(result)))
        self.assertEqual(data["conflicts"][0]["origin"], dict(origin))
        self.assertIs(result.conflicts[0]["origin"], origin)


if __name__ == "__main__":
    unittest.main()
