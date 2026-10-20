"""Compare actual local context on one physical catalog; never infer enrollment.

The differential task builds the Rust probe. All context fields, nulls, record
order, authority and physical source identity are compared without normalization.
Expected failures compare rejection and the relevant reason, not diagnostic prose.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "src"))

from _streams import catalog_agent
from _streams.catalog.codec import record_to_data
from _streams.catalog.model import Session, Stream
from _streams.catalog.operations import Operation
from _streams.catalog.sync import LocalCatalog
from _streams.catalog.yaml_store import YamlStore
from _streams.errors import StreamError

DRIVER = ROOT / "target/debug/examples/agent_context"
HOST = "01234567-89ab-4def-8123-456789abcdef"


@unittest.skipUnless(DRIVER.is_file(), "build Rust examples before context differential tests")
class RustContextDifferentialTest(unittest.TestCase):
    maxDiff = None

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name) / "catalog-caf\u00e9-\U0001f680"
        self.root.mkdir()
        self.store = YamlStore(self.root)
        self.backend = LocalCatalog(self.store, host_id=HOST)
        for identity in ("work", "other"):
            record = Stream(identity, "Not a conversation identity", None, HOST)
            receipt = self.backend.save(Operation("create-" + identity, None, record, "user"))
            self.assertTrue(receipt.accepted)
        sessions = (
            Session(catalog_agent.session_id_for(HOST, "pi", "local-native"),
                    HOST, "work", "pi", "local-native", "Local"),
            Session(catalog_agent.session_id_for("remote-host", "pi", "remote-native"),
                    "remote-host", "work", "pi", "remote-native", "Remote"),
        )
        # Physical records need not have been created by this process.
        with self.store.transaction() as transaction:
            for session in sessions:
                transaction.save(session, expected=transaction.read())

    def files(self):
        return {path.relative_to(self.root).as_posix(): path.read_bytes()
                for path in self.root.rglob("*") if path.is_file()}

    def expected(self, request):
        with patch.dict(os.environ):
            os.environ.pop("STREAMS_STREAM_ID", None)
            if request.get("environment_stream_id") is not None:
                os.environ["STREAMS_STREAM_ID"] = request["environment_stream_id"]
            try:
                value = catalog_agent.context(
                    self.backend, harness=request["harness"], native_id=request["native_id"],
                    stream_id=request.get("stream_id"),
                    workspace_stream_id=request.get("workspace_stream_id"),
                )
            except (ValueError, StreamError) as error:
                return {"error": str(error)}
        authority = value["authority"]
        return {"context": {
            **value,
            "stream": record_to_data(value["stream"]) if value["stream"] else None,
            "session": record_to_data(value["session"]) if value["session"] else None,
            "sessions": [record_to_data(session) for session in value["sessions"]],
            "authority": {"host_id": authority.host_id, "token": authority.token}
                         if authority else None,
        }}

    def compare(self, **overrides):
        request = {"harness": "pi", "native_id": "not-enrolled", **overrides}
        before = self.files()
        expected = self.expected(request)
        completed = subprocess.run(
            [str(DRIVER), str(self.root), HOST], input=json.dumps(request), text=True,
            capture_output=True, check=True, timeout=15,
        )
        actual = json.loads(completed.stdout)
        if "error" in expected:
            self.assertEqual(set(actual), {"error"})
            for reason in ("explicit move or fork", "not in the local catalog",
                           "stream_id", "harness_session_id"):
                if reason in expected["error"]:
                    self.assertIn(reason, actual["error"])
        else:
            self.assertEqual(actual, expected)
        self.assertEqual(self.files(), before, "context must not persist or enroll")
        return actual

    def test_enrolled_and_unenrolled_context_preserve_all_fields_and_nulls(self):
        for request in ({}, {"native_id": "local-native"},
                        {"native_id": "remote-native"},
                        {"harness": "codex", "native_id": "local-native"},
                        {"native_id": "Not a conversation identity"}):
            with self.subTest(request=request):
                self.compare(**request)

    def test_matching_explicit_associations_do_not_attach_the_conversation(self):
        response = self.compare(stream_id="work", environment_stream_id="work",
                                workspace_stream_id="work")
        self.assertIsNone(response["context"]["session"])
        self.assertEqual(len(response["context"]["sessions"]), 2)
        self.assertEqual(response["context"]["authority"]["token"], "create-work")

    def test_conflicting_sources_cannot_move_or_reassign_existing_work(self):
        for source in ("stream_id", "environment_stream_id", "workspace_stream_id"):
            with self.subTest(source=source):
                self.compare(native_id="local-native", **{source: "other"})
        self.compare(stream_id="work", workspace_stream_id="other")

    def test_unknown_and_malformed_identities_are_rejected_without_side_effects(self):
        for identity in ("unknown", "", " ", "work\n"):
            with self.subTest(identity=identity):
                self.compare(stream_id=identity)
        self.compare(native_id=" ")


if __name__ == "__main__":
    unittest.main()
