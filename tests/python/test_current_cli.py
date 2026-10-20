from __future__ import annotations

import io
import json
from contextlib import chdir
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams import catalog_config as config
from _streams.catalog_runtime import Runtime
from _streams.cli import build_parser, main
from _streams.cmux import CmuxWorkspace
from _streams.errors import StreamError
from _streams.herdr import HerdrPane, HerdrWorkspace
from _streams.herdr import HerdrEndpoint


CMUX = {"CMUX_WORKSPACE_ID": "caller-id"}
HERDR = {"HERDR_ENV": "1", "HERDR_PANE_ID": "old:pane"}


class CurrentSelectorParserTest(unittest.TestCase):
    def parse(self, args):
        with patch("sys.stderr", io.StringIO()):
            return build_parser().parse_args(args)

    def test_current_and_frontend_are_independent(self):
        for command in ("show", "update", "archive", "restore", "open", "resume"):
            for frontend in ("auto", "cmux", "herdr"):
                with self.subTest(command=command, frontend=frontend):
                    args = self.parse([command, "--current", "--frontend", frontend])
                    self.assertTrue(args.current)
                    self.assertEqual(args.frontend, frontend)
                    self.assertIsNone(args.ref)
                    explicit = self.parse([command, "stable-id", "-f", frontend])
                    self.assertFalse(explicit.current)
                    self.assertEqual(explicit.ref, "stable-id")

    def test_frontend_alone_does_not_select_current(self):
        args = self.parse(["resume", "-f", "herdr"])
        self.assertFalse(args.current)
        self.assertIsNone(args.ref)
        self.assertEqual(args.frontend, "herdr")

    def test_start_opens_a_new_stream_not_the_current_target(self):
        args = self.parse(["start", "New", "-f", "herdr"])
        self.assertEqual(args.frontend, "herdr")
        self.assertFalse(hasattr(args, "current"))
        with self.assertRaises(SystemExit):
            self.parse(["start", "New", "-c"])

    def test_removed_groups_and_invalid_frontends_fail(self):
        for arguments in (("herdr", "update"), ("cmux", "open"),
                          ("show", "-f"), ("show", "-f", "none"), ("show", "-f", "unknown")):
            with self.subTest(arguments=arguments), self.assertRaises(SystemExit) as error:
                self.parse(list(arguments))
            self.assertEqual(error.exception.code, 2)


class CurrentCliTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.enterContext(chdir(self.root))
        self.session_file = self.root / '.pi/agent/sessions/project/conversation_native session.jsonl'
        self.session_file.parent.mkdir(parents=True)
        self.session_file.write_text(json.dumps({'type': 'session', 'id': 'native session'}) + '\n')
        self.enterContext(patch.dict(os.environ, {
            "HOME": str(self.root), "XDG_CONFIG_HOME": str(self.root / "config"),
            "XDG_DATA_HOME": str(self.root / "data"), "XDG_STATE_HOME": str(self.root / "state"),
        }, clear=True))
        self.native = self.enterContext(patch("subprocess.run", side_effect=AssertionError("unexpected native call")))
        self.assertEqual(self.cli("init", "--sync", "none")[0], 0)
        self.herdr = self.enterContext(patch("_streams.catalog_runtime.HerdrClient")).return_value
        self.herdr.endpoint.return_value = HerdrEndpoint("/actual/herdr.sock", None)
        self.herdr.resolve_workspace.return_value = HerdrWorkspace("live-workspace", "Stale title", str(self.root))
        self.herdr.current_pane.return_value = HerdrPane("live-pane", "live-tab", "live-workspace", str(self.root))
        self.herdr.command_result.side_effect = lambda args, kind: {'snapshot': {
            'workspaces': [{'workspace_id': 'live-workspace'}],
            'tabs': [{'tab_id': 'live-tab', 'workspace_id': 'live-workspace', 'label': '1'}],
            'panes': [{'pane_id': 'live-pane', 'tab_id': 'live-tab', 'workspace_id': 'live-workspace', 'terminal_id': 'terminal'}],
            'layouts': [{'tab_id': 'live-tab', 'workspace_id': 'live-workspace', 'panes': [{'pane_id': 'live-pane'}]}],
            'agents': [],
        }} if args == ['api', 'snapshot'] else self.herdr.command_result.return_value
        self.herdr.command_result.return_value = {
            "workspace": {"workspace_id": "live-workspace", "label": "New"},
            "tab": {"tab_id": "live-tab", "workspace_id": "live-workspace"},
            "root_pane": {"pane_id": "live-pane", "tab_id": "live-tab", "workspace_id": "live-workspace"},
        }
        self.cmux = self.enterContext(patch("_streams.catalog_runtime.CmuxClient")).return_value
        self.cmux.json_from_command.return_value = {"workspace_id": "caller-id"}
        self.cmux.workspaces.return_value = [CmuxWorkspace({
            "id": "caller-id", "ref": "workspace:1", "title": "Stale title", "current_directory": str(self.root),
        })]

    def cli(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with patch("sys.stdout", out), patch("sys.stderr", err):
            try:
                code = main(list(args))
            except SystemExit as error:
                code = error.code
        return code, out.getvalue(), err.getvalue()

    def create(self, title="Example"):
        code, out, err = self.cli("create", title, "--cwd", str(self.root), "--json")
        self.assertEqual((code, err), (0, ""), out)
        return json.loads(out)["stream"]

    def attach(self, stream):
        code, out, err = self.cli("session", "attach", stream["id"], "--harness", "pi",
                                  "--session-id", "native session", "--title", "Implementation", "--cwd", str(self.root), "--json")
        self.assertEqual((code, err), (0, ""), out)
        return json.loads(out)["session"]

    def open(self, stream, frontend):
        code, out, err = self.cli("open", stream["id"], "-f", frontend, "--json")
        self.assertEqual((code, err), (0, ""), out)
        return json.loads(out)["runtime"]

    def test_create_does_not_launch_but_start_opens_with_explicit_frontend(self):
        self.create()
        self.herdr.command_result.assert_not_called()
        self.cmux.json_from_command.assert_not_called()
        code, out, err = self.cli("start", "New", "-f", "herdr", "--cwd", str(self.root), "--json")
        self.assertEqual((code, err), (0, ""), out)
        payload = json.loads(out)
        self.assertEqual(payload["runtime"]["workspace_id"], "live-workspace")
        self.assertIn(f'STREAMS_STREAM_ID={payload["stream"]["id"]}', self.herdr.command_result.call_args.args[0])
        self.native.assert_not_called()

    def test_missing_ref_never_uses_frontend_or_matching_title_and_cwd(self):
        self.create("Stale title")
        with patch.dict(os.environ, HERDR):
            for command in ("show", "open", "resume", "update"):
                code, out, err = self.cli(command, "-f", "herdr", "--json")
                self.assertEqual((code, err), (2, ""), out)
                self.assertEqual(json.loads(out)["error"]["kind"], "invalid-ref")
        self.herdr.current_pane.assert_not_called()
        self.herdr.command_result.assert_not_called()

    def test_unassociated_current_never_infers_from_matching_title_and_cwd(self):
        self.create("Stale title")
        for frontend, context in (("cmux", CMUX), ("herdr", HERDR)):
            with self.subTest(frontend=frontend), patch.dict(os.environ, context):
                code, out, err = self.cli("show", "--current", "-f", frontend, "--json")
                self.assertEqual((code, err), (2, ""))
                self.assertIn("no catalog association", json.loads(out)["error"]["message"])
        self.herdr.command_result.assert_not_called()
        self.cmux.json_from_command.assert_not_called()

    def test_explicit_ref_and_current_conflict_before_native_lookup(self):
        stream = self.create()
        code, out, err = self.cli("show", stream["id"], "-c", "-f", "herdr", "--json")
        self.assertEqual((code, err), (2, ""))
        self.assertIn("not both", json.loads(out)["error"]["message"])
        self.herdr.endpoint.assert_not_called()

    def test_explicit_environment_binding_needs_no_native_lookup(self):
        stream = self.create()
        with patch.dict(os.environ, {"STREAMS_STREAM_ID": stream["id"]} | CMUX | HERDR):
            code, out, err = self.cli("show", "-c", "--json")
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(json.loads(out)["stream"]["id"], stream["id"])
        self.herdr.endpoint.assert_not_called()
        self.cmux.workspaces.assert_not_called()

    def test_dangling_current_identity_never_falls_back_to_another_stream_title(self):
        missing_id = "00000000-0000-4000-8000-000000000000"
        competitor = self.create(missing_id)
        with patch.dict(os.environ, {"STREAMS_STREAM_ID": missing_id}):
            code, out, err = self.cli("update", "--current", "--status", "done", "--json")
        self.assertEqual((code, err), (2, ""), out)
        self.assertEqual(json.loads(out)["error"]["kind"], "not-found")
        view = json.loads(self.cli("show", competitor["id"], "--json")[1])
        self.assertEqual(view["stream"]["status"], "active")

    def test_current_association_survives_title_update_and_native_labels(self):
        for frontend, context in (("cmux", CMUX), ("herdr", HERDR)):
            stream = self.create()
            with self.subTest(frontend=frontend), patch.dict(os.environ, context):
                self.open(stream, frontend)
                self.assertEqual(self.cli("update", stream["id"], "--title", f"Renamed {frontend}")[0], 0)
                code, out, err = self.cli("show", "-c", "-f", frontend, "--json")
                self.assertEqual((code, err), (0, ""))
                self.assertEqual(json.loads(out)["stream"]["id"], stream["id"])
                self.assertEqual(json.loads(out)["stream"]["title"], f"Renamed {frontend}")
        self.herdr.resolve_workspace.assert_called_with("live-workspace")

    def test_malformed_association_fails_closed_without_overwriting_or_relaunch(self):
        stream = self.create("Stale title")
        self.open(stream, "herdr")
        binding = next(config.runtime_state_dir().rglob("associations/*/*.json"))
        binding.write_text("malformed")
        self.herdr.command_result.reset_mock()
        with patch.dict(os.environ, HERDR):
            self.assertEqual(self.cli("show", "-c", "-f", "herdr")[0], 2)
        self.assertEqual(self.cli("open", stream["id"], "-f", "herdr")[0], 2)
        self.assertEqual(binding.read_text(), "malformed")
        self.herdr.command_result.assert_not_called()

    def test_failed_start_keeps_durable_stream_and_reports_retry_identity(self):
        with patch.object(Runtime, "open", side_effect=StreamError("native open may be partial")) as launch:
            code, out, err = self.cli("start", "New", "-f", "herdr", "--json")
        self.assertEqual((code, err), (2, ""))
        payload = json.loads(out)
        self.assertIn("native open may be partial", payload["runtime"]["error"]["message"])
        self.assertIn("retry open with its ID", payload["runtime"]["detail"])
        self.assertEqual(self.cli("show", payload["stream"]["id"])[0], 0)
        launch.assert_called_once()

    def test_explicit_resume_is_frontend_independent_even_in_mixed_context(self):
        stream = self.create()
        self.attach(stream)
        with patch.dict(os.environ, CMUX | HERDR), patch("_streams.catalog_runtime.os.chdir") as chdir, \
                patch("_streams.catalog_runtime.os.execvp") as execute:
            self.assertEqual(self.cli("resume", stream["id"])[0], 0)
        chdir.assert_called_once_with(self.root.resolve())
        execute.assert_called_once_with("pi", ["pi", "--session", str(self.session_file.resolve())])
        self.herdr.command_result.assert_called_with(["api", "snapshot"], "session_snapshot")
        self.herdr.run.assert_not_called()
        self.cmux.workspaces.assert_not_called()

    def test_resume_current_uses_association_not_native_label(self):
        stream = self.create()
        self.open(stream, "herdr")
        self.attach(stream)
        with patch.dict(os.environ, HERDR), patch("_streams.catalog_runtime.os.chdir"), \
                patch("_streams.catalog_runtime.os.execvp") as execute:
            self.assertEqual(self.cli("resume", "--current", "-f", "herdr")[0], 0)
        execute.assert_called_once_with("pi", ["pi", "--session", str(self.session_file.resolve())])


if __name__ == "__main__":
    unittest.main()
