from __future__ import annotations

from dataclasses import replace
from datetime import datetime, timezone
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog.model import Catalog, Host, Link, Stream, ValidationError
from _streams.catalog.operations import Authority, Operation, reconcile, update
from test_catalog_model import session, stream


class CatalogOperationsTest(unittest.TestCase):
    def test_creation_and_retry_have_stable_identity_and_exact_target(self):
        effort = stream()
        operation = Operation(id="create-1", before=None, after=effort, actor="user")
        result = reconcile(Catalog(), operation)
        self.assertEqual((result.outcome, result.operation_id, result.record), ("changes", "create-1", effort))
        self.assertEqual(reconcile(Catalog(streams=(effort,)), operation).outcome, "no-change")
        other = replace(effort, title="Another effort")
        self.assertEqual(reconcile(Catalog(streams=(other,)), operation).outcome, "conflict")

    def test_independent_user_changes_combine_but_competing_intent_conflicts(self):
        before = stream()
        operation = update("park-1", before, {"status": "parked"})
        current = replace(before, title="Better title")
        result = reconcile(Catalog(streams=(current,)), operation)
        self.assertEqual(result.record, replace(current, status="parked"))
        result = reconcile(Catalog(streams=(replace(before, status="done"),)), operation)
        self.assertEqual(result.outcome, "conflict")
        self.assertIn("status", result.conflicts)

    def test_archive_and_restore_do_not_change_status(self):
        before = stream(status="blocked")
        archived = reconcile(Catalog(streams=(before,)), update("archive", before, {"archived": True})).record
        self.assertEqual((archived.status, archived.archived), ("blocked", True))
        restored = reconcile(Catalog(streams=(archived,)), update("restore", archived, {"archived": False})).record
        self.assertEqual((restored.status, restored.archived), ("blocked", False))

    def test_agents_cannot_change_personal_lifecycle_or_manual_modes(self):
        for changes in ({"status": "done"}, {"archived": True}, {"title_mode": "manual"}):
            with self.subTest(changes=changes), self.assertRaises(ValidationError):
                update("automatic", stream(), changes, actor="automatic")
        with self.assertRaises(ValidationError):
            Operation(id="enroll", before=None, after=stream(status="parked"), actor="automatic")

    def test_manual_names_and_goals_are_protected_from_automatic_updates(self):
        for before, changes in ((stream(title_mode="manual"), {"title": "Generated"}),
                                (stream(goal_mode="manual"), {"goal": "Generated"}),
                                (session(title_mode="manual"), {"title": "Generated"})):
            with self.subTest(before=before):
                op = update("auto", before, changes, actor="automatic",
                            ownership_token=None if isinstance(before, Stream) else "term-1")
                result = reconcile(Catalog(streams=(before,) if isinstance(before, Stream) else (stream(),),
                                           sessions=() if isinstance(before, Stream) else (before,)), op,
                                   authority=Authority(host_id="laptop", token="term-1"))
                self.assertEqual(result.outcome, "superseded")

    def test_automatic_updates_capture_all_source_fields_and_reject_late_inference(self):
        before = session(state="Designing")
        op = update("infer", before, {"state": "Draft ready", "state_short": "Drafted"},
                    actor="automatic", ownership_token="term-1")
        current = replace(before, remaining="Review new constraints")
        result = reconcile(Catalog(streams=(stream(),), sessions=(current,)), op,
                           authority=Authority(host_id="laptop", token="term-1"))
        self.assertEqual(result.outcome, "superseded")

    def test_changing_description_sources_clears_or_replaces_compact_derivatives(self):
        before = stream(goal_short="Old goal")
        self.assertIsNone(update("goal", before, {"goal": "New intended outcome"}).after.goal_short)
        contribution = session(state_short="Old state")
        for changes in ({"state": "New state"}, {"remaining": "New remaining"}, {"status": "waiting"}):
            with self.subTest(changes=changes):
                self.assertIsNone(update("state", contribution, changes, ownership_token="term-1").after.state_short)
        op = update("new", contribution, {"state": "New state", "state_short": "New"}, ownership_token="term-1")
        self.assertEqual(op.after.state_short, "New")

    def test_goal_transitions_reconcile_and_retry_without_stale_derivatives(self):
        for old_goal, new_goal in ((None, "New outcome"), ("Old outcome", None)):
            for actor in ("user", "automatic"):
                with self.subTest(old_goal=old_goal, new_goal=new_goal, actor=actor):
                    before = stream(goal=old_goal, goal_short="Old" if old_goal else None)
                    op = update("goal", before, {"goal": new_goal}, actor=actor)
                    self.assertEqual(op.after.goal, new_goal)
                    self.assertIsNone(op.after.goal_short)
                    result = reconcile(Catalog(streams=(before,)), op)
                    self.assertEqual((result.outcome, result.record), ("changes", op.after))
                    self.assertEqual(reconcile(Catalog(streams=(result.record,)), op).outcome, "no-change")

    def test_goal_transitions_protect_manual_goals_and_concurrent_overrides(self):
        for old_goal, new_goal in ((None, "Generated"), ("Existing outcome", None)):
            with self.subTest(old_goal=old_goal, new_goal=new_goal):
                before = stream(goal=old_goal, goal_mode="manual")
                op = update("automatic", before, {"goal": new_goal}, actor="automatic")
                self.assertEqual(reconcile(Catalog(streams=(before,)), op).outcome, "superseded")
                before = stream(goal=old_goal)
                current = replace(before, goal_mode="manual")
                for actor, outcome in (("user", "conflict"), ("automatic", "superseded")):
                    op = update("goal", before, {"goal": new_goal}, actor=actor)
                    self.assertEqual(reconcile(Catalog(streams=(current,)), op).outcome, outcome)
                current = replace(before, goal="Concurrent outcome")
                op = update("goal", before, {"goal": new_goal})
                result = reconcile(Catalog(streams=(current,)), op)
                self.assertEqual(result.outcome, "conflict")
                self.assertIn("goal", result.conflicts)

    def test_concurrent_manual_override_is_not_overwritten(self):
        before = stream()
        op = update("rename", before, {"title": "New title"})
        current = replace(before, title_mode="manual")
        self.assertEqual(reconcile(Catalog(streams=(current,)), op).outcome, "conflict")

    def test_session_reports_require_effective_authority_and_matching_ownership_period(self):
        before = session()
        op = update("report", before, {"state": "Ready"}, actor="automatic", ownership_token="term-1")
        catalog = Catalog(streams=(stream(),), sessions=(before,))
        for authority in (None, Authority(host_id="server", token="term-1"),
                          Authority(host_id="laptop", token="term-2")):
            with self.subTest(authority=authority):
                self.assertEqual(reconcile(catalog, op, authority=authority).outcome, "superseded")
        result = reconcile(catalog, op, authority=Authority(host_id="laptop", token="term-1"))
        self.assertEqual(result.outcome, "changes")
        self.assertEqual(catalog.streams, (stream(),))
        # User edits have conflicts, not silently dropped intent, when authority is stale.
        user_op = update("user-report", before, {"state": "Ready"}, ownership_token="term-1")
        self.assertEqual(reconcile(catalog, user_op).outcome, "conflict")

    def test_admission_checks_stream_membership_repository_and_native_identity(self):
        authority = Authority(host_id="laptop", token="term-1")
        op = Operation(id="join", before=None, after=session(), actor="automatic", ownership_token="term-1")
        self.assertEqual(reconcile(Catalog(), op, authority=authority).outcome, "conflict")
        self.assertEqual(reconcile(Catalog(streams=(stream(),)), op, authority=authority).outcome, "changes")
        duplicate = replace(session(), id="other-record")
        self.assertEqual(reconcile(Catalog(streams=(stream(),), sessions=(duplicate,)), op, authority=authority).outcome, "conflict")

    def test_generic_updates_cannot_handover_or_reattach_sessions(self):
        for before, changes in ((stream(), {"owner_host_id": "server"}),
                                (session(), {"stream_id": "another"}),
                                (session(), {"harness_session_id": "another"}),
                                (stream(), {"id": "another"})):
            with self.subTest(changes=changes), self.assertRaises(ValidationError):
                update("invalid", before, changes, ownership_token="term-1")

    def test_independent_reference_additions_combine_by_identity(self):
        before = stream()
        first = Link(url="https://example.org/1", description="First")
        second = Link(url="https://example.org/2", description="Second")
        op = update("add", before, {"issues": (first,)})
        current = replace(before, issues=(second,))
        result = reconcile(Catalog(streams=(current,)), op)
        self.assertEqual(result.record.issues, (second, first))
        conflicting = replace(before, issues=(replace(first, description="Different intent"),))
        self.assertEqual(reconcile(Catalog(streams=(conflicting,)), op).outcome, "conflict")

    def test_removing_a_reference_does_not_reinterpret_concurrent_changes(self):
        link = Link(url="https://example.org/1", description="First")
        before = stream(issues=(link,))
        op = update("remove", before, {"issues": ()})
        current = replace(before, issues=(replace(link, description="Edited"),))
        self.assertEqual(reconcile(Catalog(streams=(current,)), op).outcome, "conflict")

    def test_missing_update_target_conflicts_instead_of_recreating_it(self):
        op = update("rename", stream(), {"title": "Renamed"})
        self.assertEqual(reconcile(Catalog(), op).outcome, "conflict")

    def test_host_checkins_are_producer_owned_and_do_not_touch_sessions(self):
        host = Host(id="laptop", label="Laptop", checked_at=datetime(2026, 10, 16, tzinfo=timezone.utc))
        result = reconcile(Catalog(streams=(stream(),), sessions=(session(),)),
                           Operation(id="checkin", before=None, after=host, actor="automatic"))
        self.assertEqual(result.record, host)


if __name__ == "__main__":
    unittest.main()
