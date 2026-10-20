"""Black-box captured-update parity; the request and canonical intent stay unchanged."""

from __future__ import annotations

from dataclasses import replace
from datetime import datetime, timezone
import json
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "src"))

from _streams.catalog.codec import record_to_data
from _streams.catalog.model import Host, Session, Stream
from _streams.catalog.sync.envelope import encode_operation
from _streams.catalog_agent import parse_update
from _streams.errors import StreamError

DRIVER = ROOT / "target/debug/examples/agent_request"
SOURCE = "captured-physical-source"
HOST = "Laptop/é"


def request(before, changes, *, actor="user", inputs=()):
    return {
        "operation_id": "captured-operation",
        "catalog_source": SOURCE,
        "kind": "session" if isinstance(before, Session) else "stream",
        "actor": actor,
        "before": record_to_data(before),
        "changes": changes,
        "ownership_token": "create" if isinstance(before, Session) or actor == "automatic" else None,
        "inputs": [{"kind": "host", "record": record_to_data(item)} for item in inputs],
    }


@unittest.skipUnless(DRIVER.is_file(), "Run mise run test:differential to build the adapter")
class AgentRequestDifferentialTest(unittest.TestCase):
    maxDiff = None

    def compare(self, wire, *, valid):
        source = wire if isinstance(wire, str) else json.dumps(wire, ensure_ascii=False)
        try:
            expected = {"message": encode_operation(parse_update(source, catalog_source=SOURCE))}
        except (ValueError, StreamError):
            expected = {"error": True}
        process = subprocess.run([str(DRIVER), SOURCE], input=source, capture_output=True,
                                 text=True, encoding="utf-8", timeout=30)
        self.assertEqual(process.returncode, 0, process.stderr)
        actual = json.loads(process.stdout)
        if valid:
            self.assertNotIn("error", expected)
            self.assertEqual(actual, expected)
        else:
            self.assertIn("error", expected)
            self.assertIn("error", actual)
            self.assertNotIn("message", actual)

    def test_normalization_and_manual_protection_preserve_canonical_intent(self):
        stream = Stream("work", "Captured", "Old goal", HOST, goal_short="Old summary")
        session = Session("session", HOST, "work", "pi", "native", "Captured", state_short="Old summary")
        supporting = Host(HOST, "Captured host", datetime(2026, 1, 1, microsecond=123000, tzinfo=timezone.utc))
        cases = [
            request(stream, {"title": "Chosen 🧭"}),
            request(stream, {"title": "Inferred", "title_mode": "automatic"}),
            request(stream, {"goal": None}),
            request(stream, {"goal": "New goal", "goal_short": "New summary"}),
            request(stream, {"goal": "Generated"}, actor="automatic", inputs=[supporting]),
            request(replace(stream, title_mode="manual"), {"title": "Ignore", "goal": "Generated"}, actor="automatic"),
            request(replace(stream, goal_mode="manual"), {"goal": "Ignore", "goal_short": "Ignore"}, actor="automatic"),
            request(session, {"state": "進行中\nnext", "remaining": "Tests"}, actor="automatic"),
            request(session, {"title": "Chosen", "status": "done"}),
        ]
        for index, wire in enumerate(cases):
            with self.subTest(case=index):
                self.compare(wire, valid=True)

    def test_invalid_sources_fields_and_captures_are_rejected(self):
        stream = Stream("work", "Captured", None, HOST)
        valid = request(stream, {})
        cases = [
            {**valid, "catalog_source": "different-source"},
            {**valid, "unexpected": True},
            {**valid, "kind": "host"},
            {**valid, "changes": {"owner_host_id": "other"}},
            {**valid, "changes": {"repositories": {}}},
            request(stream, {"status": "done"}, actor="automatic"),
            {**valid, "inputs": None},
            {**valid, "inputs": [{"kind": "stream", "record": record_to_data(stream)}]},
        ]
        for field in ("operation_id", "catalog_source", "kind", "actor", "before", "changes"):
            cases.append({key: value for key, value in valid.items() if key != field})
        for index, wire in enumerate(cases):
            with self.subTest(case=index):
                self.compare(wire, valid=False)
        duplicate = json.dumps(valid).replace('"changes": {}', '"changes": {"title": "A", "title": "B"}')
        self.assertNotEqual(duplicate, json.dumps(valid))
        self.compare(duplicate, valid=False)
