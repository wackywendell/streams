from __future__ import annotations

from copy import deepcopy
import io
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog.codec import record_to_data
from _streams.catalog.model import Session, Stream, ValidationError
from _streams.catalog_preview import preview
from _streams.cli import build_parser, main
from _streams.errors import StreamError


class CatalogPreviewTest(unittest.TestCase):
    def setUp(self):
        self.stream = record_to_data(Stream("effort", "Effort", "Detailed goal", "host", goal_short="Goal"))
        self.session = record_to_data(Session("session", "host", "effort", "pi", "native", "Contribution",
                                              state="Detailed state", state_short="State"))
        self.other = record_to_data(Session("other", "host", "effort", "pi", "other-native", "Other"))
        self.context = {"catalog_source": "[redacted-source]", "host_id": "host", "stream": self.stream,
                        "session": self.session, "sessions": [self.other, self.session],
                        "authority": {"host_id": "host", "token": "[redacted-token]"},
                        "checkouts": [], "native": {"opaque": "preserved, not attested"}}

    def apply(self, changes, *, kind="session", actor="automatic"):
        return {"operation_id": "update", "catalog_source": self.context["catalog_source"], "kind": kind,
                "actor": actor, "before": deepcopy(self.context[kind]), "changes": changes,
                "ownership_token": self.context["authority"]["token"] if kind == "session" or actor == "automatic" else None,
                "inputs": [{"kind": "stream", "record": deepcopy(self.context["stream"])}] if kind == "session" else []}

    def enroll(self, **changes):
        return {"operation_id": "enroll", "catalog_source": self.context["catalog_source"], "harness": "pi",
                "harness_session_id": "native", "session": {"title": "Contribution"}, "actor": "automatic",
                "cwd": "/does/not/exist/and/is/not/read", **changes}

    def project(self, request, action="apply", context=None):
        return preview(json.dumps({"catalog": context if context is not None else self.context,
                                   "action": action, "request": request}))

    def test_apply_uses_domain_derivative_clearing_and_preserves_snapshot(self):
        captured = deepcopy(self.context)
        result = self.project(self.apply({"remaining": "Next"}))
        self.assertIsNone(result["catalog"]["session"]["state_short"])
        self.assertEqual(result["catalog"]["sessions"], [self.other, result["catalog"]["session"]])
        self.assertEqual(result["catalog"]["native"], self.context["native"])
        self.assertEqual(self.context, captured)
        self.assertTrue(result["receipt"]["simulated"])
        self.assertTrue(result["receipt"]["save"]["simulated"])
        self.assertEqual(result["receipt"]["save"]["local"], "unchanged")
        self.assertEqual(result["receipt"]["save"]["domain"]["outcome"], "changes")
        self.assertNotIn("publication", result["receipt"])

    def test_goal_clear_and_user_naming_modes_are_domain_owned(self):
        result = self.project(self.apply({"goal": None}, kind="stream", actor="user"))
        self.assertIsNone(result["catalog"]["stream"]["goal_short"])
        self.assertEqual(result["catalog"]["stream"]["goal_mode"], "manual")
        self.assertEqual(result["catalog"]["sessions"], self.context["sessions"])
        result = self.project(self.apply({"title": self.session["title"]}, actor="user"))
        self.assertEqual(result["catalog"]["session"]["title_mode"], "manual")

    def test_automatic_manual_text_filter_and_explicit_derivatives(self):
        self.context["stream"]["title_mode"] = "manual"
        result = self.project(self.apply({"title": "Ignored"}, kind="stream"))
        self.assertEqual(result["catalog"]["stream"]["title"], "Effort")
        self.assertEqual(result["receipt"]["save"]["domain"]["outcome"], "no-change")
        result = self.project(self.apply({"status": "done", "state_short": "Finished"}))
        self.assertEqual(result["catalog"]["session"]["state_short"], "Finished")

    def test_source_target_inputs_and_capabilities_must_match_exactly(self):
        for mutate in (
            lambda r: r.update(catalog_source="elsewhere"),
            lambda r: r["before"].update(state="Stale independent field"),
            lambda r: r["before"].update(id="other"),
            lambda r: r["inputs"][0]["record"].update(goal="Stale goal"),
            lambda r: r.update(ownership_token="other-period"),
            lambda r: r.update(ownership_token=None),
            lambda r: r["inputs"].append(deepcopy(r["inputs"][0])),
        ):
            request = self.apply({"title": "User intent"}, actor="user")
            mutate(request)
            with self.subTest(request=request), self.assertRaises((ValidationError, StreamError)):
                self.project(request)

    def test_context_identity_relationships_and_host_are_validated(self):
        for mutate in (
            lambda c: c["session"].update(host_id="foreign"),
            lambda c: c.update(sessions=[self.other]),
            lambda c: c["sessions"][0].update(stream_id="elsewhere"),
            lambda c: c.update(sessions=c["sessions"] * 2),
            lambda c: c.update(host_id="foreign"),
        ):
            context = deepcopy(self.context)
            mutate(context)
            with self.subTest(context=context), self.assertRaises(ValidationError):
                self.project(self.apply({"state": "Next"}), context=context)
        self.context["session"] = None
        self.context["authority"] = {"host_id": "foreign", "token": "[redacted-token]"}
        with self.assertRaises(ValidationError):
            self.project(self.enroll(harness_session_id="new-native"), "enroll")

    def test_invalid_edit_shapes_and_domain_values_are_rejected(self):
        for changes, kind, actor in (({"state": "", "state_short": ""}, "session", "automatic"),
                                      ({"stream_id": "elsewhere"}, "session", "user"),
                                      ({"repositories": {}}, "stream", "user"),
                                      ({"status": "done"}, "stream", "automatic")):
            with self.subTest(changes=changes), self.assertRaises(ValidationError):
                self.project(self.apply(changes, kind=kind, actor=actor))

    def test_new_enrollment_is_deterministic_and_ephemeral(self):
        empty = {**self.context, "stream": None, "session": None, "sessions": [], "authority": None}
        request = self.enroll(stream={"title": "New", "goal": "Goal", "goal_short": "Compact"}, actor="user")
        first = self.project(request, "enroll", empty)
        self.assertEqual(first, self.project(request, "enroll", empty))
        self.assertEqual(first["catalog"]["stream"]["title_mode"], "manual")
        self.assertEqual(first["catalog"]["stream"]["goal_mode"], "manual")
        self.assertEqual(first["catalog"]["session"]["harness_session_id"], "native")
        self.assertIn("redacted", first["catalog"]["authority"]["token"])
        self.assertEqual(len(first["receipt"]["saves"]), 2)
        self.assertTrue(all(item["save"]["simulated"] for item in first["receipt"]["saves"]))
        self.assertEqual(len(first["catalog"]["sessions"]), 1)
        self.assertEqual(empty["sessions"], [])
        # Returned fake capability supports a bounded follow-on proposal.
        self.context = first["catalog"]
        result = self.project(self.apply({"state": "Follow-on", "state_short": "Ready"}))
        self.assertEqual(result["catalog"]["session"]["state_short"], "Ready")

    def test_existing_enrollment_preserves_other_sessions_and_is_idempotent(self):
        context = {**self.context, "session": None}
        request = self.enroll(harness_session_id="third-native", stream_id="effort")
        result = self.project(request, "enroll", context)
        self.assertEqual(result["catalog"]["sessions"][:2], self.context["sessions"])
        self.assertEqual(len(result["catalog"]["sessions"]), 3)
        repeated = self.project(request, "enroll", result["catalog"])
        self.assertEqual(repeated["catalog"], result["catalog"])
        self.assertEqual(repeated["receipt"]["saves"], [])
        self.assertTrue(repeated["receipt"]["publication"]["simulated"])

    def test_enrollment_cannot_rebind_or_impersonate_existing_context(self):
        for request in (self.enroll(stream_id="other"), self.enroll(harness_session_id="other-native"),
                        self.enroll(actor="user", stream={"title": "Replacement"})):
            with self.subTest(request=request), self.assertRaises(ValidationError):
                self.project(request, "enroll")
        with self.assertRaises(ValidationError):
            self.project(self.enroll(), "enroll", {**self.context, "session": None})

    def test_enrollment_strict_shape_matches_live_request_parser(self):
        for request in (self.enroll(unexpected=True), self.enroll(session={"title": "X", "state": "No"}),
                        self.enroll(cwd=None), self.enroll(harness="unknown"), self.enroll(actor="unknown"),
                        self.enroll(stream={"title": "X", "status": "done"}), self.enroll(catalog_source="wrong")):
            with self.subTest(request=request), self.assertRaises((ValidationError, StreamError)):
                self.project(request, "enroll")

    def test_cli_preview_is_a_dedicated_effect_free_handler(self):
        args = build_parser().parse_args(["agent", "preview", "--json"])
        self.assertEqual(args.func.__name__, "command_preview")
        for action, request in (("apply", self.apply({"state": "Proposal"})),
                                ("enroll", self.enroll())):
            out = io.StringIO()
            with patch("sys.stdin", io.StringIO(json.dumps({"catalog": self.context, "action": action, "request": request}))), \
                 patch("sys.stdout", out), \
                 patch("_streams.cli.config.open_catalog", side_effect=AssertionError("catalog opened")), \
                 patch("_streams.cli.runtime", side_effect=AssertionError("runtime opened")), \
                 patch("_streams.cli.maybe_sync", side_effect=AssertionError("sync")), \
                 patch("_streams.cli.config.host_id", side_effect=AssertionError("config read")), \
                 patch("builtins.open", side_effect=AssertionError("file opened")), \
                 patch("os.open", side_effect=AssertionError("file opened")), \
                 patch.object(Path, "resolve", side_effect=AssertionError("path resolved")), \
                 patch.object(Path, "stat", side_effect=AssertionError("path inspected")), \
                 patch.object(Path, "write_text", side_effect=AssertionError("file written")):
                self.assertEqual(main(["agent", "preview", "--json"]), 0)
            self.assertTrue(json.loads(out.getvalue())["receipt"]["simulated"])

    def test_malformed_envelope_and_duplicate_json_keys(self):
        for source in ('null', '[]', '{}', '{"catalog":{},"catalog":{}}'):
            with self.subTest(source=source), self.assertRaises(ValidationError):
                preview(source)
        with self.assertRaises(ValidationError):
            self.project(self.apply({}), "handover")


if __name__ == "__main__":
    unittest.main()
