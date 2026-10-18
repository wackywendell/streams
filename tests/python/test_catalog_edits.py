from __future__ import annotations

from dataclasses import replace
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog.codec import record_to_data
from _streams.catalog.model import Catalog, Link, Session, Stream, ValidationError
from _streams.catalog.operations import Authority, Operation, normalize_changes, reconcile, update
from _streams.catalog.sync.envelope import decode_operation, encode_operation
from _streams.catalog_agent import parse_update
from _streams.cli import build_parser, scalar_changes
from test_catalog_model import session, stream


class CatalogEditsTest(unittest.TestCase):
    def parse(self, before, changes, *, actor="user", inputs=()):
        kind = "stream" if isinstance(before, Stream) else "session"
        return parse_update(json.dumps({
            "operation_id": "edit", "catalog_source": "captured", "kind": kind,
            "actor": actor, "before": record_to_data(before), "changes": changes,
            "ownership_token": "term" if actor == "automatic" or isinstance(before, Session) else None,
            "inputs": [{"kind": "stream" if isinstance(item, Stream) else "session",
                        "record": record_to_data(item)} for item in inputs],
        }), catalog_source="captured")

    def domain(self, before, changes, *, actor="user", inputs=()):
        return update("edit", before, changes, actor=actor, inputs=inputs,
                      ownership_token="term" if actor == "automatic" or isinstance(before, Session) else None)

    def cli(self, before, *flags):
        command = ["update", before.id] if isinstance(before, Stream) else ["session", "update", before.id]
        args = build_parser().parse_args([*command, *flags])
        fields = ("title", "goal", "status") if isinstance(before, Stream) else ("title", "state", "remaining", "status")
        return scalar_changes(args, fields)

    def test_source_changes_clear_derivatives_through_domain_and_agent(self):
        cases = [(stream(goal_short="Compact"), {"goal": goal}, "goal_short")
                 for goal in (None, "New goal")]
        cases += [(session(state="Old", remaining="Work", state_short="Compact"), changes, "state_short")
                  for changes in ({"state": "New"}, {"state": ""}, {"remaining": ""}, {"status": "done"})]
        for before, changes, derivative in cases:
            for actor in ("user", "automatic"):
                with self.subTest(changes=changes, actor=actor):
                    domain = self.domain(before, changes, actor=actor)
                    self.assertEqual(self.parse(before, changes, actor=actor), domain)
                    self.assertIsNone(getattr(domain.after, derivative))

    def test_unchanged_sources_retain_derivatives_unless_explicitly_replaced(self):
        for before, changes, derivative in (
            (stream(goal_short="Compact"), {"goal": stream().goal}, "goal_short"),
            (session(state="Old", state_short="Compact"), {"state": "Old"}, "state_short"),
        ):
            for build in (self.domain, self.parse):
                with self.subTest(record=before, build=build):
                    self.assertEqual(getattr(build(before, changes).after, derivative), "Compact")
                    replaced = {**changes, derivative: "Replacement"}
                    self.assertEqual(getattr(build(before, replaced).after, derivative), "Replacement")
        self.assertEqual(self.parse(stream(goal_short="Old"), {"goal": "New", "goal_short": "New compact"}).after.goal_short, "New compact")
        self.assertEqual(self.parse(session(state_short="Old"), {"state": "New", "state_short": "New compact"}).after.state_short, "New compact")

    def test_supplied_changed_or_unchanged_user_text_selects_manual_mode(self):
        for before in (stream(), session()):
            for title in (before.title, "New title"):
                with self.subTest(record=before, title=title):
                    changes = {"title": title}
                    cli_changes = self.cli(before, "--title", title)
                    self.assertEqual(cli_changes, changes)
                    expected = self.domain(before, changes)
                    self.assertEqual(expected.after.title_mode, "manual")
                    self.assertEqual(self.parse(before, changes), expected)
                    self.assertEqual(self.domain(before, cli_changes), expected)
        for goal in (stream().goal, "New goal", None):
            with self.subTest(goal=goal):
                flags = ("--clear-goal",) if goal is None else ("--goal", goal)
                expected = self.domain(stream(), {"goal": goal})
                self.assertEqual(expected.after.goal_mode, "manual")
                self.assertEqual(self.parse(stream(), {"goal": goal}), expected)
                self.assertEqual(self.domain(stream(), self.cli(stream(), *flags)), expected)

    def test_explicit_modes_override_supplied_user_text_including_clear_goal(self):
        for before in (stream(), session()):
            changes = {"title": before.title, "title_mode": "automatic"}
            self.assertEqual(self.parse(before, changes), self.domain(before, changes))
            self.assertEqual(self.domain(before, changes).after.title_mode, "automatic")
            self.assertEqual(self.cli(before, "--title", before.title, "--title-mode", "automatic"), changes)
        for mode in ("automatic", "manual"):
            changes = {"goal": None, "goal_mode": mode}
            before = stream(goal_short="Compact")
            self.assertEqual(self.cli(before, "--clear-goal", "--goal-mode", mode), changes)
            self.assertEqual(self.parse(before, changes), self.domain(before, changes))
            self.assertEqual(self.domain(before, changes).after.goal_mode, mode)

    def test_normalization_is_pure_and_idempotent_without_filtering_automatic_intent(self):
        before = stream(goal_mode="manual", goal_short="Compact")
        changes = {"goal": "New"}
        for actor in ("user", "automatic"):
            normalized = normalize_changes(before, changes, actor=actor)
            self.assertEqual(changes, {"goal": "New"})
            self.assertEqual(normalize_changes(before, normalized, actor=actor), normalized)
            self.assertEqual(normalized["goal"], "New")
            self.assertIsNone(normalized["goal_short"])
            self.assertEqual("goal_mode" in normalized, actor == "user")

    def test_automatic_harness_proposals_filter_manual_text_but_domain_intent_is_superseded(self):
        for before, changes in (
            (stream(title_mode="manual"), {"title": "Generated"}),
            (stream(goal_mode="manual", goal_short="Human"), {"goal": None}),
            (stream(goal_mode="manual"), {"goal_short": "Generated"}),
            (session(title_mode="manual"), {"title": "Generated"}),
        ):
            with self.subTest(record=before, changes=changes):
                parsed = self.parse(before, changes, actor="automatic")
                self.assertEqual(parsed.after, before)
                operation = self.domain(before, changes, actor="automatic")
                catalog = Catalog(streams=(before,) if isinstance(before, Stream) else (stream(),),
                                  sessions=(before,) if isinstance(before, Session) else ())
                result = reconcile(catalog, operation, authority=Authority("laptop", "term"))
                self.assertEqual(result.outcome, "superseded")
                self.assertTrue(operation.changed_fields)
        for before, changes in ((stream(), {"title": "Generated", "goal": "New"}),
                                (session(), {"title": "Generated", "state": "New"})):
            operation = self.parse(before, changes, actor="automatic")
            self.assertEqual(operation.after.title_mode, "automatic")
            self.assertEqual(operation, self.domain(before, changes, actor="automatic"))

    def test_empty_edits_and_invalid_fields_keep_validation(self):
        for before in (stream(), session()):
            self.assertEqual(self.domain(before, {}).after, before)
            self.assertEqual(self.parse(before, {}).after, before)
        for changes in ({"title": ""}, {"title": None}, {"goal": ""}, {"goal": 42},
                        {"title_mode": "unknown"}, {"status": "unknown"}, {"extra": "value"},
                        {"goal": None, "goal_short": "Invalid without goal"}):
            for build in (self.domain, self.parse):
                with self.subTest(changes=changes, build=build), self.assertRaises(ValidationError):
                    build(stream(goal_short="Old"), changes)
        for changes in ({"repositories": {}}, {"owner_host_id": "other"}):
            with self.subTest(changes=changes), self.assertRaises(ValidationError):
                self.parse(stream(), changes)
        for changes in ({"title_mode": "manual"}, {"status": "done"}):
            for build in (self.domain, self.parse):
                with self.subTest(changes=changes, build=build), self.assertRaises(ValidationError):
                    build(stream(), changes, actor="automatic")

    def test_codec_still_decodes_and_validates_nested_references(self):
        operation = self.parse(stream(), {"issues": [{"url": "https://example.org/issue", "description": ""}]})
        self.assertEqual(operation.after.issues, (Link("https://example.org/issue", ""),))
        for issues in ({}, [{"url": "https://example.org/issue", "description": "", "extra": True}]):
            with self.subTest(issues=issues), self.assertRaises(ValidationError):
                self.parse(stream(), {"issues": issues})

    def test_captured_inputs_operation_identity_and_historical_replay_are_preserved(self):
        before = session(state_short="Old")
        captured = stream(goal_short="Captured")
        changes = {"state": "New"}
        operation = self.parse(before, changes, inputs=(captured,))
        self.assertEqual(operation, self.domain(before, changes, inputs=(captured,)))
        self.assertEqual((operation.id, operation.before, operation.inputs, operation.ownership_token),
                         ("edit", before, (captured,), "term"))
        self.assertEqual(self.parse(before, changes, inputs=(captured,)), operation)
        historical = Operation("historical", stream(), replace(stream(), title="Old intent"), "user", inputs=(session(),))
        encoded = encode_operation(historical)
        decoded, _ = decode_operation(encoded)
        self.assertEqual(decoded, historical)
        self.assertEqual(decoded.after.title_mode, "automatic")
        self.assertEqual(encode_operation(decoded), encoded)


if __name__ == "__main__":
    unittest.main()
