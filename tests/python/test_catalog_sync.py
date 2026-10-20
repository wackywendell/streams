from __future__ import annotations

from datetime import datetime, timezone
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog.model import Checkout, Document, Host, Link, Repository, Session, Stream, ValidationError
from _streams.catalog.operations import Operation, update
from _streams.catalog.sync import LocalCatalog, Policy, Timing, decode_operation, encode_operation
from _streams.catalog.yaml_store import YamlStore


def effort(id="work", host="laptop"):
    return Stream(id=id, title="Sync design", goal="Share work safely", owner_host_id=host)


def create(record=None, id="create"):
    return Operation(id=id, before=None, after=record or effort(), actor="user")


def contribution(token="create", id="join"):
    return Operation(id=id, before=None, actor="automatic", ownership_token=token,
                     after=Session(id="session", host_id="laptop", stream_id="work",
                                   harness="pi", harness_session_id="native", title="Implement sync"))


class PolicyTest(unittest.TestCase):
    def test_event_gates_and_missing_baselines(self):
        policy = Policy()
        timing = Timing(last_sync=100, last_push=100)
        self.assertFalse(policy.due("session", timing, now=399))
        self.assertFalse(policy.due("stream", timing, now=399))
        self.assertTrue(policy.due("stream", timing, now=400))
        self.assertTrue(policy.due("session", timing, now=400, pending_stream=True))
        self.assertFalse(policy.due("session", timing, now=400))
        self.assertTrue(policy.due("session", timing, now=3700))
        for event in ("start", "attach"):
            self.assertTrue(policy.due(event, timing, now=400))
        for event in ("shutdown", "explicit", "force"):
            self.assertTrue(policy.due(event, timing, now=101))
        self.assertTrue(policy.due("session", Timing(), now=0))

    def test_failure_cooldown_is_shared_but_explicit_can_bypass(self):
        timing = Timing(last_sync=0, last_push=0, retry_after=1000)
        self.assertFalse(Policy().due("stream", timing, now=500))
        self.assertTrue(Policy().due("explicit", timing, now=500))
        self.assertTrue(Policy().due("shutdown", timing, now=500))
        self.assertFalse(Policy().due("session", Timing(last_push=1000), now=500))

    def test_invalid_policy_and_event_fail_visibly(self):
        for kwargs in ({"stream_seconds": -1}, {"session_seconds": float("nan")}, {"attempts": 0}):
            with self.assertRaises(ValueError):
                Policy(**kwargs)
        with self.assertRaises(ValueError):
            Policy().due("typo", Timing(), now=1)


class EnvelopeTest(unittest.TestCase):
    def test_roundtrip_preserves_all_operation_inputs_and_origin(self):
        for operation in (create(), contribution(), update("edit", effort(), {"status": "parked"})):
            encoded = encode_operation(operation, origin={"host": "laptop", "session": "native"})
            decoded, origin = decode_operation(encoded)
            self.assertEqual(decoded, operation)
            self.assertEqual(origin, {"host": "laptop", "session": "native"})

    def test_codec_roundtrips_nested_references_and_evidence_times(self):
        stream = Stream(id="work", title="Title", goal="Goal", owner_host_id="laptop",
                        repositories={"src": Repository("ssh://example.org/repo")},
                        documents=(Document("src", "main", "docs/design.md", "Design"),),
                        issues=(Link("https://example.org/ticket", "Ticket"),))
        host = Host("laptop", "Laptop", datetime(2026, 10, 16, tzinfo=timezone.utc))
        session = Session("s", "laptop", "work", "pi", "native", "Title",
                          checkout=Checkout("src", "main", "src"))
        for record in (stream, host, session):
            operation = Operation("create", None, record, "user",
                                  ownership_token="term" if isinstance(record, Session) else None)
            self.assertEqual(decode_operation(encode_operation(operation))[0], operation)

    def test_unsupported_or_ambiguous_envelopes_are_rejected(self):
        for message in ('{}', '{"version":2}', '{"version":1,"version":1}',
                        encode_operation(create()).replace('"actor":"user"', '"actor":"robot"')):
            with self.subTest(message=message), self.assertRaises(ValidationError):
                decode_operation(message)


class LocalCatalogTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / "catalog"

    def test_local_creation_and_attachment_need_no_git_or_publication(self):
        with patch("subprocess.run", side_effect=AssertionError("local-only invoked Git")):
            local = LocalCatalog(YamlStore(self.root), host_id="laptop")
            result = local.save(create())
            self.assertEqual((result.local, result.sharing), ("persisted", "local-only"))
            self.assertEqual(local.authority("work").token, "create")
            # Process restart retains the creation token.
            local = LocalCatalog(YamlStore(self.root), host_id="laptop")
            joined = local.save(contribution())
            self.assertEqual(joined.domain.outcome, "changes")
            self.assertEqual(local.snapshot().catalog.sessions, (contribution().after,))
            self.assertEqual(local.sync().sharing, "local-only")

    def test_no_change_conflict_and_foreign_producers_are_distinct(self):
        local = LocalCatalog(YamlStore(self.root), host_id="laptop")
        local.save(create())
        self.assertEqual(local.save(create()).domain.outcome, "no-change")
        foreign = create(effort("foreign", host="server"), "foreign")
        with self.assertRaises(ValidationError):
            local.save(foreign)
        desired = update("park", effort(), {"status": "parked"})
        local.save(update("done", effort(), {"status": "done"}))
        result = local.save(desired)
        self.assertEqual((result.domain.outcome, result.local), ("conflict", "unchanged"))

    def test_local_retry_identity_survives_restore_and_process_restart(self):
        local = LocalCatalog(YamlStore(self.root), host_id="laptop")
        local.save(create())
        park = update("park", effort(), {"status": "parked"})
        local.save(park)
        local.save(update("restore", park.after, {"status": "active"}))
        local = LocalCatalog(YamlStore(self.root), host_id="laptop")
        retried = local.save(park)
        self.assertEqual(retried.domain.outcome, "no-change")
        self.assertEqual(local.snapshot().catalog.stream("work").status, "active")
        changed = local.save(update("park", effort(), {"status": "done"}))
        self.assertEqual(changed.local, "repair-required")
        self.assertEqual(local.snapshot().catalog.stream("work").status, "active")

    def test_local_save_failure_preserves_marker_and_requires_repair(self):
        local = LocalCatalog(YamlStore(self.root), host_id="laptop")
        with patch("_streams.catalog.yaml_store._YamlTransaction.save", side_effect=OSError("disk")):
            result = local.save(create())
        self.assertEqual(result.local, "repair-required")
        self.assertEqual(local.save(create()).local, "repair-required")
        self.assertTrue((self.root / ".catalog-sync" / "save.json").exists())

    def test_backend_does_not_expose_domain_handover(self):
        local = LocalCatalog(YamlStore(self.root), host_id="laptop")
        self.assertFalse(hasattr(local, "handover"))


if __name__ == "__main__":
    unittest.main()
