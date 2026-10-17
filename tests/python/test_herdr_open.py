from __future__ import annotations

import copy
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.errors import StreamError
from _streams.herdr import open_workspace, plugin_root
from _streams.herdr_binding import HerdrEndpoint, binding_path
from test_herdr import FakeHerdr, response, status, stream_at, workspace


def plugin(**fields: object) -> dict:
    root = plugin_root()
    return {"plugin_id": "streams.links", "enabled": True, "plugin_root": str(root),
            "manifest_path": str(root / "herdr-plugin.toml"), "version": "0.1.0", "min_herdr_version": "0.9.3",
            "source": {"kind": "local"}, "platforms": ["macos"],
            "panes": [{"id": "links", "command": ["/bin/sh", "launch.sh"], "placement": "split", "title": "Stream links"}],
            **fields}


def pane(id: str) -> dict:
    return {"pane_id": id, "workspace_id": "w9", "tab_id": "w9:t4", "cwd": "/directory"}


class HerdrOpenTest(unittest.TestCase):
    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        env = patch.dict(os.environ, {"XDG_CONFIG_HOME": str(self.root / "config"), "XDG_STATE_HOME": str(self.root / "state")}, clear=True)
        env.start()
        self.addCleanup(env.stop)
        name = patch("_streams.herdr.uuid.uuid4")
        name.start().return_value.hex = "a" * 32
        self.addCleanup(name.stop)
        self.agent_name = "streams-" + "a" * 24
        self.stream = stream_at(self.root, cwd=str(self.root))
        self.binding = binding_path(HerdrEndpoint("/actual/herdr.sock", None), "w9")
        self.list_command = ("plugin", "list", "--plugin", "streams.links", "--json")
        self.create_command = ("workspace", "create", "--cwd", str(self.root), "--label", "Example", "--focus")
        self.links_command = (
            "plugin", "pane", "open", "--plugin", "streams.links", "--entrypoint", "links", "--placement", "split",
            "--target-pane", "w9:p2", "--direction", "down", "--cwd", str(plugin_root()), "--no-focus",
            "--env", f"STREAMS_PYTHON={sys.executable}", "--env", f"STREAMS_IMPORT_ROOT={Path(__file__).resolve().parents[2] / 'src'}",
            "--env", f"STREAMS_BINDING={self.binding}",
            "--env", f"HOME={Path.home()}", "--env", f"XDG_CONFIG_HOME={(self.root / 'config').resolve()}",
        )
        self.resize_command = ("pane", "resize", "--pane", "w9:p2", "--direction", "down", "--amount", "0.3")
        self.replies = {
            ("status", "--json"): status(version="0.9.3"),
            self.list_command: response("plugin_list", plugins=[plugin()]),
            self.create_command: response("workspace_created", workspace={"workspace_id": "w9", "label": "Example"},
                                          tab={"workspace_id": "w9", "tab_id": "w9:t4"}, root_pane=pane("w9:p2")),
            ("workspace", "rename", "w9", "Example"): workspace("w9"),
            ("workspace", "report-metadata", "w9", "--source", "streams", "--token", "stream_slug=example",
             "--token", "stream_status=🟩", "--token", "stream_state=Ready", "--token", "stream_summary=🟩 Ready"): "",
            self.links_command: response("plugin_pane_opened", plugin_pane={"plugin_id": "streams.links", "entrypoint": "links", "pane": pane("w9:p8")}),
            self.resize_command: response("pane_resize", resize={"pane_id": "w9:p2", "focused_pane_id": "w9:p2", "changed": True}),
        }

    def test_shell_only_open_creates_working_tab_links_and_preserves_root_focus(self) -> None:
        client = FakeHerdr(self.replies)
        opened = open_workspace(self.stream, client)
        self.assertEqual(opened.workspace.id, "w9")
        self.assertEqual(opened.root_pane.id, "w9:p2")
        self.assertEqual(opened.links_pane.id, "w9:p8")
        self.assertEqual(opened.links_pane.tab_id, "w9:t4")
        self.assertIsNone(opened.agent_name)
        self.assertEqual(opened.binding, self.binding)
        self.assertEqual(json.loads(opened.binding.read_text()), {"stream": "example"})
        self.assertEqual(len(client.calls), 7)
        self.assertEqual(client.calls[0:3], [["status", "--json"], list(self.list_command), list(self.create_command)])

    def test_latest_session_is_passed_as_native_argv_not_shell_text(self) -> None:
        self.stream = stream_at(self.root, cwd=str(self.root), sessions=["codex:old", "pi:session with spaces"])
        command = ("agent", "start", self.agent_name, "--kind", "pi", "--pane", "w9:p2", "--timeout", "20000", "--", "--session", "session with spaces")
        self.replies[command] = response("agent_started", agent={**pane("w9:p2"), "agent": "pi", "name": self.agent_name, "interactive_ready": True})
        client = FakeHerdr(self.replies)
        self.assertEqual(open_workspace(self.stream, client).agent_name, self.agent_name)
        self.assertEqual(client.calls[-1], list(command))

    def test_invalid_cwd_and_session_arguments_fail_before_native_calls(self) -> None:
        for fields in ({"cwd": str(self.root / "missing")}, {"sessions": ["pi:bad\x00session"]}, {"sessions": ["claude:line\nfeed"]}):
            stream = stream_at(self.root, **{"cwd": str(self.root), **fields})
            client = FakeHerdr()
            with self.subTest(fields=fields), self.assertRaisesRegex(StreamError, "cwd does not exist|control characters"):
                open_workspace(stream, client)
            self.assertEqual(client.calls, [])

    def test_unsupported_platform_and_native_version_preflight_before_layout(self) -> None:
        with patch("_streams.herdr.sys.platform", "linux"), self.assertRaisesRegex(StreamError, "macOS"):
            open_workspace(self.stream, FakeHerdr())
        for version in ("0.9.2", "invalid"):
            client = FakeHerdr({("status", "--json"): status(version=version)})
            with self.subTest(version=version), self.assertRaisesRegex(StreamError, "0.9.3"):
                open_workspace(self.stream, client)
            self.assertEqual(client.calls, [["status", "--json"]])

    def test_missing_plugin_is_registered_before_layout(self) -> None:
        self.replies[self.list_command] = response("plugin_list", plugins=[])
        self.replies[("plugin", "link", str(plugin_root()))] = response("plugin_linked", plugin=plugin())
        client = FakeHerdr(self.replies)
        open_workspace(self.stream, client)
        self.assertEqual(client.calls[2:4], [["plugin", "link", str(plugin_root())], list(self.create_command)])

    def test_matching_plugin_is_reused_without_link_or_enable(self) -> None:
        client = FakeHerdr(self.replies)
        open_workspace(self.stream, client)
        self.assertFalse(any(args[:2] in (["plugin", "link"], ["plugin", "enable"]) for args in client.calls))

    def test_conflicting_or_disabled_plugin_never_overwrites_or_creates_workspace(self) -> None:
        for fields in ({"enabled": False}, {"plugin_root": "/other"}, {"manifest_path": "/other.toml"},
                       {"version": "2.0"}, {"source": {"kind": "github"}},
                       {"panes": [{"id": "links", "command": ["bad"], "placement": "split"}]}):
            self.replies[self.list_command] = response("plugin_list", plugins=[plugin(**fields)])
            client = FakeHerdr(self.replies)
            with self.subTest(fields=fields), self.assertRaisesRegex(StreamError, "disabled|conflict"):
                open_workspace(self.stream, client)
            self.assertEqual(len(client.calls), 2)

    def test_failed_registration_does_not_create_layout_or_retry(self) -> None:
        self.replies[self.list_command] = response("plugin_list", plugins=[])
        self.replies[("plugin", "link", str(plugin_root()))] = {"error": {"message": "registration failed"}}
        client = FakeHerdr(self.replies)
        with self.assertRaisesRegex(StreamError, "registration.*inspect|registration.*Inspect"):
            open_workspace(self.stream, client)
        self.assertEqual(len(client.calls), 3)

    def test_failure_after_registration_reports_global_resource(self) -> None:
        self.replies[self.list_command] = response("plugin_list", plugins=[])
        self.replies[("plugin", "link", str(plugin_root()))] = response("plugin_linked", plugin=plugin())
        self.replies[self.create_command] = {"error": {"message": "workspace unavailable"}}
        client = FakeHerdr(self.replies)
        with self.assertRaisesRegex(StreamError, "plugin streams.links.*workspace unavailable.*before retrying"):
            open_workspace(self.stream, client)
        self.assertEqual(len(client.calls), 4)

    def test_malformed_create_and_mismatched_pane_ids_report_partial_resources(self) -> None:
        for field in ("root_pane", "tab"):
            replies = copy.deepcopy(self.replies)
            replies[self.create_command]["result"][field] = {}
            client = FakeHerdr(replies)
            with self.subTest(field=field), self.assertRaisesRegex(StreamError, "partial.*w9"):
                open_workspace(self.stream, client)
            self.assertEqual(len(client.calls), 3)
        replies = copy.deepcopy(self.replies)
        replies[self.links_command]["result"]["plugin_pane"]["pane"]["tab_id"] = "another-tab"
        with self.assertRaisesRegex(StreamError, "partial.*w9.*mismatch"):
            open_workspace(self.stream, FakeHerdr(replies))

    def test_links_failure_reports_workspace_tab_root_and_binding_without_launch(self) -> None:
        self.replies[self.links_command] = {"error": {"message": "links startup failed"}}
        client = FakeHerdr(self.replies)
        with self.assertRaisesRegex(StreamError, "partial.*w9.*w9:t4.*w9:p2.*binding.*links startup failed"):
            open_workspace(self.stream, client)
        self.assertEqual(len(client.calls), 6)
        self.assertTrue(self.binding.exists())

    def test_resize_failure_does_not_launch_agent_or_reset_layout(self) -> None:
        self.replies[self.resize_command] = {"error": {"message": "resize failed"}}
        client = FakeHerdr(self.replies)
        with self.assertRaisesRegex(StreamError, "partial.*w9:p8.*resize failed"):
            open_workspace(self.stream, client)
        self.assertEqual(len(client.calls), 7)

    def test_unexpected_resize_focus_and_agent_identity_are_reported_without_retry(self) -> None:
        replies = copy.deepcopy(self.replies)
        replies[self.resize_command]["result"]["resize"]["focused_pane_id"] = "another-pane"
        with self.assertRaisesRegex(StreamError, "partial.*preserving root focus"):
            open_workspace(self.stream, FakeHerdr(replies))
        self.stream = stream_at(self.root, cwd=str(self.root), sessions=["pi:recorded"])
        command = ("agent", "start", self.agent_name, "--kind", "pi", "--pane", "w9:p2", "--timeout", "20000", "--", "--session", "recorded")
        self.replies[command] = response("agent_started", agent={**pane("w9:p2"), "agent": "codex", "name": self.agent_name, "interactive_ready": True})
        client = FakeHerdr(self.replies)
        with self.assertRaisesRegex(StreamError, "partial.*identity/readiness"):
            open_workspace(self.stream, client)
        self.assertEqual(len(client.calls), 8)


    def test_blocked_agent_start_reports_attempted_name_without_relaunch(self) -> None:
        self.stream = stream_at(self.root, cwd=str(self.root), sessions=["codex:recorded"])
        command = ("agent", "start", self.agent_name, "--kind", "codex", "--pane", "w9:p2", "--timeout", "20000", "--", "resume", "recorded")
        self.replies[command] = {"error": {"code": "agent_not_ready", "message": "approval required"}}
        client = FakeHerdr(self.replies)
        with self.assertRaisesRegex(StreamError, f"partial.*{self.agent_name}.*approval required.*before retrying"):
            open_workspace(self.stream, client)
        self.assertEqual(len(client.calls), 8)


if __name__ == "__main__":
    unittest.main()
