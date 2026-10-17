from __future__ import annotations

from dataclasses import FrozenInstanceError
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.cli import main
from _streams import catalog_config as config
from _streams.errors import StreamError


CMUX_CONTEXT = {"CMUX_WORKSPACE_ID": "workspace:1"}
HERDR_CONTEXT = {"HERDR_ENV": "1", "HERDR_PANE_ID": "w1:p2"}


class FrontendPolicyTest(unittest.TestCase):
    def resolve(self, configured="auto", requested=None, context=None):
        from _streams.frontend import Frontend, FrontendMode, resolve_frontend

        request = requested if requested in (None, "auto") else Frontend(requested)
        return resolve_frontend(FrontendMode(configured), request, context or {})

    def test_auto_detects_only_caller_identity(self):
        cases = [
            ({}, None),
            (CMUX_CONTEXT, "cmux"),
            (HERDR_CONTEXT, "herdr"),
            ({"CMUX_SOCKET_PATH": "/tmp/cmux.sock", "HERDR_SOCKET_PATH": "/tmp/herdr.sock", "TERM_PROGRAM": "herdr"}, None),
            ({"CMUX_WORKSPACE_ID": "", "HERDR_ENV": "0", "HERDR_PANE_ID": ""}, None),
        ]
        for context, expected in cases:
            with self.subTest(context=context):
                self.assertEqual(self.resolve(context=context).as_json(), {"configured": "auto", "requested": None, "resolved": expected})

    def test_missing_and_auto_requests_follow_configured_policy(self):
        for requested in (None, "auto"):
            for configured, expected in (("auto", "herdr"), ("cmux", "cmux"), ("herdr", "herdr"), ("none", None)):
                with self.subTest(configured=configured, requested=requested):
                    self.assertEqual(self.resolve(configured, requested, HERDR_CONTEXT).as_json(), {"configured": configured, "requested": requested, "resolved": expected})

    def test_named_requests_override_configuration_and_mixed_context(self):
        context = CMUX_CONTEXT | HERDR_CONTEXT
        for configured in ("auto", "cmux", "herdr", "none"):
            for requested in ("cmux", "herdr"):
                with self.subTest(configured=configured, requested=requested):
                    self.assertEqual(self.resolve(configured, requested, context).as_json(), {"configured": configured, "requested": requested, "resolved": requested})

    def test_mixed_auto_context_is_an_error_not_precedence(self):
        with self.assertRaises(StreamError) as raised:
            self.resolve(context=CMUX_CONTEXT | HERDR_CONTEXT)
        self.assertEqual(raised.exception.kind, "ambiguous")
        self.assertEqual(raised.exception.candidates, ["cmux", "herdr"])

    def test_incomplete_herdr_context_fails_even_when_cmux_is_present(self):
        for context in ({"HERDR_ENV": "1"}, {"HERDR_PANE_ID": "w1:p2"}, {"HERDR_ENV": "0", "HERDR_PANE_ID": "w1:p2"}):
            for extra in ({}, CMUX_CONTEXT):
                with self.subTest(context=context, extra=extra):
                    with self.assertRaisesRegex(StreamError, "incomplete Herdr"):
                        self.resolve(context=context | extra)

    def test_named_and_disabled_policies_do_not_require_auto_detection(self):
        for configured, expected in (("cmux", "cmux"), ("herdr", "herdr"), ("none", None)):
            with self.subTest(configured=configured):
                self.assertEqual(self.resolve(configured, context={"HERDR_ENV": "1"}).as_json()["resolved"], expected)

    def test_result_is_immutable_and_resolved_frontend_is_typed(self):
        from _streams.frontend import Frontend

        selection = self.resolve(context=HERDR_CONTEXT)
        self.assertIs(selection.resolved, Frontend.HERDR)
        with self.assertRaises(FrozenInstanceError):
            selection.resolved = None


class FrontendQueryTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        environment = patch.dict(os.environ, {"HOME": str(self.root), "XDG_CONFIG_HOME": str(self.root / "config")}, clear=True)
        environment.start()
        self.addCleanup(environment.stop)
        self.path = self.root / "config/streams/config.json"

    def write_config(self, data):
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(json.dumps(data))

    def cli(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with patch("sys.stdout", out), patch("sys.stderr", err):
            try:
                code = main(list(args))
            except SystemExit as error:
                code = error.code
        return code, out.getvalue(), err.getvalue()

    def test_absent_setting_uses_auto_without_creating_configuration(self):
        self.assertEqual(str(config.get_frontend_mode()), "auto")
        code, out, err = self.cli("frontend", "get", "--json")
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(json.loads(out), {"configured": "auto", "requested": None, "resolved": None})
        self.assertFalse(self.path.exists())

    def test_policy_is_fresh_and_distinguishes_none_from_absence(self):
        for data, expected in (({}, "auto"), ({"frontend": {}}, "auto"), ({"frontend": {"mode": "cmux"}}, "cmux"), ({"frontend": {"mode": "none"}}, "none")):
            with self.subTest(data=data):
                self.write_config(data)
                original = self.path.read_bytes()
                self.assertEqual(str(config.get_frontend_mode()), expected)
                self.assertEqual(self.path.read_bytes(), original)

    def test_query_uses_configuration_for_auto_and_named_choice_for_override(self):
        self.write_config({"frontend": {"mode": "cmux"}})
        with patch.dict(os.environ, HERDR_CONTEXT):
            for selector, requested, expected in (((), None, "cmux"), (("-f", "auto"), "auto", "cmux"), (("--frontend", "herdr"), "herdr", "herdr")):
                with self.subTest(selector=selector):
                    code, out, err = self.cli("frontend", "get", *selector, "--json")
                    self.assertEqual((code, err), (0, ""))
                    self.assertEqual(json.loads(out), {"configured": "cmux", "requested": requested, "resolved": expected})

    def test_disabled_policy_can_be_overridden_by_named_query(self):
        self.write_config({"frontend": {"mode": "none"}})
        for selector, expected in ((("-f", "auto"), None), (("-f", "herdr"), "herdr")):
            code, out, err = self.cli("frontend", "get", *selector, "--json")
            self.assertEqual((code, err), (0, ""))
            self.assertEqual(json.loads(out)["resolved"], expected)

    def test_query_uses_catalog_configuration(self):
        from _streams.frontend import FrontendMode

        with patch("_streams.catalog_config.get_frontend_mode", return_value=FrontendMode.NONE) as mode:
            code, out, err = self.cli("frontend", "get", "--json")
        self.assertEqual((code, err), (0, ""))
        self.assertIsNone(json.loads(out)["resolved"])
        mode.assert_called_once_with()

    def test_query_never_probes_binaries_or_backend_connectivity(self):
        with patch.dict(os.environ, HERDR_CONTEXT), patch("subprocess.run", side_effect=AssertionError("native execution")), patch("subprocess.Popen", side_effect=AssertionError("native execution")):
            code, out, err = self.cli("frontend", "get", "--json")
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(json.loads(out)["resolved"], "herdr")

    def test_query_reports_ambiguity_and_partial_context_as_json_errors(self):
        for context, kind in ((CMUX_CONTEXT | HERDR_CONTEXT, "ambiguous"), ({"HERDR_ENV": "1"}, "error")):
            with self.subTest(context=context), patch.dict(os.environ, context):
                code, out, err = self.cli("frontend", "get", "--json")
                self.assertEqual((code, err), (2, ""))
                self.assertEqual(json.loads(out)["error"]["kind"], kind)

    def test_bad_configuration_fails_closed(self):
        for value in ("true", None, 1):
            with self.subTest(value=value):
                self.write_config({"frontend": {"mode": value}})
                with self.assertRaises(StreamError):
                    config.get_frontend_mode()
                code, out, err = self.cli("frontend", "get", "--json")
                self.assertEqual((code, err), (2, ""))
                self.assertIn("frontend.mode", json.loads(out)["error"]["message"])

    def test_frontend_value_is_required_and_invalid_values_are_rejected(self):
        for arguments in (("-f",), ("--frontend", "none"), ("-f", "unknown")):
            with self.subTest(arguments=arguments):
                code, out, err = self.cli("frontend", "get", *arguments)
                self.assertEqual((code, out), (2, ""))
                self.assertTrue(err)
        self.assertFalse(self.path.exists())

    def test_human_query_reports_selection(self):
        with patch.dict(os.environ, HERDR_CONTEXT):
            code, out, err = self.cli("frontend", "get")
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(yaml.safe_load(out), {"configured": "auto", "requested": None, "resolved": "herdr"})

    def test_module_entrypoint_matches_console_entrypoint_outside_checkout(self):
        result = subprocess.run([sys.executable, "-m", "_streams.cli", "frontend", "get", "--json"], cwd=self.root, env=dict(os.environ), capture_output=True, text=True)
        self.assertEqual((result.returncode, result.stderr), (0, ""))
        self.assertEqual(json.loads(result.stdout), json.loads(self.cli("frontend", "get", "--json")[1]))
