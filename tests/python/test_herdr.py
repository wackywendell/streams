from __future__ import annotations

import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.cli import main
from _streams.cmux import CmuxClient
from _streams.config import VALID_STATUSES
from _streams.errors import StreamError
from _streams.herdr import HerdrClient, HerdrWorkspace, load_stream_for_update, update_workspace
from _streams.herdr_binding import HerdrEndpoint, binding_path, write_binding
from _streams.frontmatter import render_frontmatter
from _streams.models import Stream
from _streams.store import rename_stream


def stream_at(root: Path, slug: str = "example", **fields: object) -> Stream:
    frontmatter = {
        "title": "Example", "status": "active", "description": "Task", "state": "Ready",
        "vcs": "jj", "repo": "/repo", "cwd": "/repo", "issues": [], "prs": [], "sessions": [],
        **fields,
    }
    path = root / f"{slug}.md"
    path.write_text(render_frontmatter(frontmatter, "# Body\n"))
    return Stream.parse(path, frontmatter, "# Body\n")


def response(kind: str, **fields: object) -> dict:
    return {"result": {"type": kind, **fields}}


def status(socket: str = "/actual/herdr.sock", session: str | None = None, **fields: object) -> dict:
    return {"server": {"version": "0.9.3", "running": True, "compatible": True, "endpoint_compatible": True,
                       "socket": socket, "session": session, **fields}}


def workspace(id: str = "w2", label: str = "Example") -> dict:
    return response("workspace_info", workspace={"workspace_id": id, "label": label, "agent_status": "working"})


def pane(cwd: str = "/repo/topic") -> dict:
    return response("pane_current", pane={"pane_id": "w2:p7", "tab_id": "w2:t3", "workspace_id": "w2",
                                         "cwd": "/repo", "foreground_cwd": cwd})


class FakeHerdr(HerdrClient):
    def __init__(self, replies: dict[tuple[str, ...], dict | str | StreamError] | None = None) -> None:
        super().__init__()
        self.calls: list[list[str]] = []
        self.replies = replies or {}

    def run(self, args: list[str]) -> subprocess.CompletedProcess[str]:
        self.calls.append(args)
        key = tuple(args)
        if key not in self.replies:
            raise AssertionError(f"unexpected native call: {args}")
        reply = self.replies[key]
        if isinstance(reply, StreamError):
            raise reply
        return subprocess.CompletedProcess(args, 0, reply if isinstance(reply, str) else json.dumps(reply), "")


class HerdrTest(unittest.TestCase):
    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        env = patch.dict(os.environ, {"XDG_CONFIG_HOME": str(self.root / "config"),
                                      "XDG_STATE_HOME": str(self.root / "state")}, clear=True)
        env.start()
        self.addCleanup(env.stop)

    def test_current_requires_complete_caller_context_before_native_call(self) -> None:
        for context in ({}, {"HERDR_SOCKET_PATH": "/socket"}, {"HERDR_ENV": "1"}, {"HERDR_PANE_ID": "w1:p1"}):
            with self.subTest(context=context), patch.dict(os.environ, context, clear=True):
                client = FakeHerdr()
                with self.assertRaisesRegex(StreamError, "caller.*--workspace"):
                    client.resolve_workspace(None)
                self.assertEqual(client.calls, [])

    def test_current_uses_live_moved_pane_not_inherited_workspace_or_tab(self) -> None:
        with patch.dict(os.environ, {"HERDR_ENV": "1", "HERDR_PANE_ID": "w1:p1",
                                     "HERDR_WORKSPACE_ID": "w1", "HERDR_TAB_ID": "w1:t1"}):
            client = FakeHerdr({("pane", "current", "--current"): pane(),
                                ("workspace", "get", "w2"): workspace()})
            target = client.resolve_workspace(None)
        self.assertEqual(target, HerdrWorkspace("w2", "Example", "/repo/topic"))
        self.assertEqual(client.calls, [["pane", "current", "--current"], ["workspace", "get", "w2"]])

    def test_explicit_workspace_works_outside_herdr_without_pane_discovery(self) -> None:
        client = FakeHerdr({("workspace", "get", "w9"): workspace("w9")})
        self.assertEqual(client.resolve_workspace("w9"), HerdrWorkspace("w9", "Example"))
        self.assertEqual(client.calls, [["workspace", "get", "w9"]])

    def test_stale_caller_failure_does_not_fall_back_to_focus(self) -> None:
        with patch.dict(os.environ, {"HERDR_ENV": "1", "HERDR_PANE_ID": "old"}):
            client = FakeHerdr({("pane", "current", "--current"): {"error": {"code": "not_found", "message": "stale caller"}}})
            with self.assertRaisesRegex(StreamError, "stale caller"):
                client.resolve_workspace(None)
            self.assertEqual(client.calls, [["pane", "current", "--current"]])

    def test_native_shapes_and_ids_are_parsed_not_coerced(self) -> None:
        for malformed in ({}, response("workspace_info", workspace=None), workspace(id=""),
                          response("workspace_info", workspace={"workspace_id": 3, "label": "Example"}),
                          response("workspace_info", workspace={"workspace_id": "w2", "label": None})):
            with self.subTest(response=malformed), self.assertRaisesRegex(StreamError, "unexpected|invalid"):
                FakeHerdr({("workspace", "get", "w2"): malformed}).resolve_workspace("w2")

    def test_invalid_pane_response_never_targets_a_workspace(self) -> None:
        with patch.dict(os.environ, {"HERDR_ENV": "1", "HERDR_PANE_ID": "old"}):
            for field in ("pane_id", "workspace_id", "tab_id"):
                reply = pane()
                reply["result"]["pane"][field] = ""
                client = FakeHerdr({("pane", "current", "--current"): reply})
                with self.subTest(field=field), self.assertRaisesRegex(StreamError, "invalid"):
                    client.resolve_workspace(None)
                self.assertEqual(len(client.calls), 1)

    def test_malformed_pane_cwd_and_empty_selector_fail_closed(self) -> None:
        with patch.dict(os.environ, {"HERDR_ENV": "1", "HERDR_PANE_ID": "old"}):
            for field in ("cwd", "foreground_cwd"):
                reply = pane()
                reply["result"]["pane"][field] = 0
                with self.subTest(field=field), self.assertRaisesRegex(StreamError, "invalid pane cwd"):
                    FakeHerdr({("pane", "current", "--current"): reply}).resolve_workspace(None)
        client = FakeHerdr()
        with self.assertRaisesRegex(StreamError, "nonempty"):
            client.resolve_workspace("")
        self.assertEqual(client.calls, [])

    def test_mismatched_native_workspace_response_is_rejected(self) -> None:
        with self.assertRaisesRegex(StreamError, "workspace.*mismatch"):
            FakeHerdr({("workspace", "get", "w2"): workspace("w3")}).resolve_workspace("w2")

    def test_binary_override_and_native_environment_are_preserved(self) -> None:
        with patch.dict(os.environ, {"HERDR_BIN_PATH": "/native/bin/herdr", "HERDR_SOCKET_PATH": "/socket"}), \
                patch("_streams.herdr.subprocess.run", return_value=subprocess.CompletedProcess([], 0, "{}", "")) as run:
            client = HerdrClient()
            client.run(["status", "--json"])
            args, kwargs = run.call_args
        self.assertEqual(args[0], ["/native/bin/herdr", "status", "--json"])
        self.assertEqual(kwargs["env"]["HERDR_SOCKET_PATH"], "/socket")
        self.assertEqual(kwargs["timeout"], 30)
        self.assertTrue(kwargs["check"])

    def test_missing_binary_failed_command_and_timeout_are_expected_errors(self) -> None:
        failures = ((FileNotFoundError(), "missing executable"),
                    (subprocess.CalledProcessError(1, ["herdr"], stderr='{"error":{"message":"offline"}}'), "offline"),
                    (subprocess.TimeoutExpired(["herdr"], 30), "timed out.*before retrying"))
        for failure, message in failures:
            with self.subTest(failure=failure), patch("_streams.herdr.subprocess.run", side_effect=failure), \
                    self.assertRaisesRegex(StreamError, message):
                HerdrClient().run(["workspace", "get", "w2"])

    def test_invalid_json_is_an_expected_error(self) -> None:
        with patch("_streams.herdr.subprocess.run", return_value=subprocess.CompletedProcess([], 0, "not json", "")), \
                self.assertRaisesRegex(StreamError, "JSON"):
            HerdrClient().resolve_workspace("w2")

    def test_successful_native_stderr_stays_out_of_stdout(self) -> None:
        stderr = io.StringIO()
        with patch("_streams.herdr.subprocess.run", return_value=subprocess.CompletedProcess([], 0, "{}", "Warning\n")), \
                patch("sys.stderr", stderr):
            self.assertEqual(HerdrClient().run(["status", "--json"]).stdout, "{}")
        self.assertEqual(stderr.getvalue(), "Warning\n")

    def test_endpoint_uses_actual_status_not_inherited_socket(self) -> None:
        with patch.dict(os.environ, {"HERDR_SOCKET_PATH": "/stale/socket"}):
            client = FakeHerdr({("status", "--json"): status(session="named")})
            self.assertEqual(client.endpoint(), HerdrEndpoint("/actual/herdr.sock", "named"))

    def test_endpoint_rejects_unavailable_incompatible_or_malformed_status(self) -> None:
        for reply in ({}, status(running=False), status(compatible=False), status(endpoint_compatible=False),
                      status(socket=""), status(socket="relative.sock"), status(session=3)):
            with self.subTest(reply=reply), self.assertRaises(StreamError):
                FakeHerdr({("status", "--json"): reply}).endpoint()

    def test_explicit_stream_can_rebind_a_differently_named_workspace(self) -> None:
        desired = stream_at(self.root)
        client = FakeHerdr({("workspace", "get", "w2"): workspace(label="Other")})
        stream, target = load_stream_for_update(desired.slug, "w2", False, client, root=self.root)
        self.assertEqual(stream.slug, desired.slug)
        self.assertEqual(target.label, "Other")

    def test_exact_label_inference_and_unique_dedicated_caller_cwd(self) -> None:
        desired = stream_at(self.root, cwd="/repo/topic")
        explicit = FakeHerdr({("workspace", "get", "w2"): workspace(), ("status", "--json"): status()})
        self.assertEqual(load_stream_for_update(None, "w2", False, explicit, root=self.root)[0].slug, desired.slug)
        with patch.dict(os.environ, {"HERDR_ENV": "1", "HERDR_PANE_ID": "old"}):
            current = FakeHerdr({("pane", "current", "--current"): pane(),
                                 ("workspace", "get", "w2"): workspace(label="Shell"), ("status", "--json"): status()})
            self.assertEqual(load_stream_for_update(None, None, False, current, root=self.root)[0].slug, desired.slug)

    def test_duplicate_labels_dedicated_cwds_and_shared_cwds_are_rejected(self) -> None:
        first = stream_at(self.root, "first", cwd="/repo/topic")
        stream_at(self.root, "second", cwd="/repo/topic")
        with self.assertRaisesRegex(StreamError, "multiple streams"):
            load_stream_for_update(None, "w2", False,
                                   FakeHerdr({("workspace", "get", "w2"): workspace(), ("status", "--json"): status()}), root=self.root)
        with patch.dict(os.environ, {"HERDR_ENV": "1", "HERDR_PANE_ID": "old"}):
            for cwd in ("/repo/topic", "/repo"):
                if cwd == "/repo":
                    stream_at(self.root, "first", cwd="/repo", vcs=None, repo=None)
                client = FakeHerdr({("pane", "current", "--current"): pane(cwd),
                                    ("workspace", "get", "w2"): workspace(label="Shell"), ("status", "--json"): status()})
                with self.subTest(cwd=cwd), self.assertRaisesRegex(StreamError, "pass a slug"):
                    load_stream_for_update(None, None, True, client, root=self.root)
        self.assertEqual(first.slug, "first")

    def test_current_and_workspace_conflict_is_rejected_before_native_calls(self) -> None:
        client = FakeHerdr()
        with self.assertRaisesRegex(StreamError, "either --current or --workspace"):
            load_stream_for_update("example", "w2", True, client, root=self.root)
        self.assertEqual(client.calls, [])

    def test_native_projection_accepts_workspace_info_and_silent_metadata(self) -> None:
        stream = stream_at(self.root)
        replies = [subprocess.CompletedProcess([], 0, json.dumps(status()), ""),
                   subprocess.CompletedProcess([], 0, json.dumps(workspace()), ""),
                   subprocess.CompletedProcess([], 0, "", "")]
        with patch("_streams.herdr.subprocess.run", side_effect=replies) as run:
            path = update_workspace(stream, HerdrWorkspace("w2", "Old"), HerdrClient())
        self.assertEqual(json.loads(path.read_text()), {"stream": "example"})
        self.assertEqual(run.call_count, 3)

    def test_rename_response_must_identify_the_updated_workspace(self) -> None:
        stream = stream_at(self.root)
        replies = [subprocess.CompletedProcess([], 0, json.dumps(status()), ""),
                   subprocess.CompletedProcess([], 0, json.dumps(workspace("other")), "")]
        with patch("_streams.herdr.subprocess.run", side_effect=replies) as run, self.assertRaisesRegex(StreamError, "workspace ID mismatch"):
            update_workspace(stream, HerdrWorkspace("w2", "Old"), HerdrClient())
        self.assertEqual(run.call_count, 2)

    def test_status_metadata_uses_symbols_and_compact_summary_without_changing_note(self) -> None:
        symbols = {"active": "🟩", "waiting": "◷", "blocked": "!", "parked": "Ⅱ", "done": "✓"}
        self.assertEqual(set(symbols), VALID_STATUSES)
        for stream_status, symbol in symbols.items():
            with self.subTest(status=stream_status):
                stream = stream_at(self.root, status=stream_status, state=" Ready ")
                original_note = stream.path.read_text()
                metadata = ("workspace", "report-metadata", "w2", "--source", "streams",
                            "--token", "stream_slug=example", "--token", f"stream_status={symbol}",
                            "--token", "stream_state= Ready ", "--token", f"stream_summary={symbol} Ready")
                client = FakeHerdr({("status", "--json"): status(),
                                    ("workspace", "rename", "w2", "Example"): workspace(),
                                    metadata: ""})
                update_workspace(stream, HerdrWorkspace("w2", "Old"), client)
                self.assertEqual(client.calls[-1], list(metadata))
                self.assertEqual(stream.as_json()["status"], stream_status)
                self.assertEqual(stream.as_json()["state"], " Ready ")
                self.assertEqual(stream.path.read_text(), original_note)

    def test_metadata_update_preserves_focus_layout_and_agent_state(self) -> None:
        stream = stream_at(self.root, state="Waiting for review", status="waiting")
        target = HerdrWorkspace("w2", "Old", "/repo/topic")
        metadata = ["workspace", "report-metadata", "w2", "--source", "streams",
                    "--token", "stream_slug=example", "--token", "stream_status=◷",
                    "--token", "stream_state=Waiting for review", "--token", "stream_summary=◷ Waiting for review"]
        client = FakeHerdr({("status", "--json"): status(),
                            ("workspace", "rename", "w2", "Example"): workspace(),
                            tuple(metadata): ""})
        path = update_workspace(stream, target, client)
        self.assertEqual(client.calls, [["status", "--json"], ["workspace", "rename", "w2", "Example"], metadata])
        self.assertEqual(path, binding_path(HerdrEndpoint("/actual/herdr.sock", None), "w2"))
        self.assertEqual(json.loads(path.read_text()), {"stream": "example"})
        self.assertEqual(stream.path.read_text(), render_frontmatter(stream.frontmatter, "# Body\n"))

    def test_failed_metadata_retains_new_association_without_retry(self) -> None:
        stream = stream_at(self.root)
        path = binding_path(HerdrEndpoint("/actual/herdr.sock", None), "w2")
        write_binding(path, "old-stream")
        client = FakeHerdr({("status", "--json"): status(),
                            ("workspace", "rename", "w2", "Example"): workspace(),
                            ("workspace", "report-metadata", "w2", "--source", "streams", "--token", "stream_slug=example",
                             "--token", "stream_status=🟩", "--token", "stream_state=Ready", "--token", "stream_summary=🟩 Ready"):
                                StreamError("metadata unavailable")})
        with self.assertRaisesRegex(StreamError, "partial.*w2.*label.*metadata unavailable"):
            update_workspace(stream, HerdrWorkspace("w2", "Old"), client)
        self.assertEqual(json.loads(path.read_text()), {"stream": "example"})
        self.assertEqual(len(client.calls), 3)

    def test_binding_failure_prevents_native_display_updates(self) -> None:
        stream = stream_at(self.root)
        client = FakeHerdr({("status", "--json"): status(),
                            ("workspace", "rename", "w2", "Example"): workspace(),
                            ("workspace", "report-metadata", "w2", "--source", "streams", "--token", "stream_slug=example",
                             "--token", "stream_status=🟩", "--token", "stream_state=Ready", "--token", "stream_summary=🟩 Ready"): ""})
        with patch("_streams.herdr.write_binding", side_effect=StreamError("disk unavailable")), \
                self.assertRaisesRegex(StreamError, "not changed.*no native display update.*disk unavailable"):
            update_workspace(stream, HerdrWorkspace("w2", "Old"), client)
        self.assertEqual(client.calls, [["status", "--json"]])

    def test_failed_endpoint_preflight_does_not_mutate(self) -> None:
        stream = stream_at(self.root)
        client = FakeHerdr({("status", "--json"): status(running=False)})
        with self.assertRaises(StreamError):
            update_workspace(stream, HerdrWorkspace("w2", "Old"), client)
        self.assertEqual(client.calls, [["status", "--json"]])

    def test_existing_association_overrides_workspace_label(self) -> None:
        stream_at(self.root)
        path = binding_path(HerdrEndpoint("/actual/herdr.sock", None), "w2")
        write_binding(path, "example")
        client = FakeHerdr({("workspace", "get", "w2"): workspace(label="Unknown"), ("status", "--json"): status()})
        self.assertEqual(load_stream_for_update(None, "w2", False, client, root=self.root)[0].slug, "example")

    def test_explicit_group_ignores_configured_frontend(self) -> None:
        stream = stream_at(self.root)
        config = self.root / "config" / "streams" / "config.json"
        config.parent.mkdir(parents=True)
        config.write_text(json.dumps({"cmux": {"enabled": False}}))
        output = io.StringIO()
        client = FakeHerdr({("workspace", "get", "w2"): workspace()})
        with patch("_streams.cli.HerdrClient", return_value=client), \
                patch("_streams.herdr.resolve_stream_ref", return_value=stream), \
                patch("_streams.cli.update_herdr_workspace") as update, patch("sys.stdout", output):
            self.assertEqual(main(["herdr", "update", "example", "--workspace", "w2"]), 0)
        update.assert_called_once_with(stream, HerdrWorkspace("w2", "Example"), client)
        self.assertEqual(output.getvalue(), "updated: w2 example\n")

    def test_cli_current_without_context_fails_without_native_calls(self) -> None:
        with patch("_streams.herdr.subprocess.run") as run, patch("sys.stderr", io.StringIO()) as error:
            self.assertEqual(main(["herdr", "update", "example", "--current"]), 2)
        run.assert_not_called()
        self.assertIn("--workspace", error.getvalue())

    def test_cmux_current_requires_caller_instead_of_global_focus(self) -> None:
        with patch("_streams.cmux.subprocess.run") as run, self.assertRaisesRegex(StreamError, "caller.*--workspace"):
            CmuxClient().resolve_workspace(None)
        run.assert_not_called()

    def test_cmux_caller_context_and_explicit_target_work_but_stale_context_fails(self) -> None:
        reply = subprocess.CompletedProcess([], 0, '{"workspaces":[{"id":"id2","ref":"workspace:2","title":"Example"}]}', "")
        with patch.dict(os.environ, {"CMUX_WORKSPACE_ID": "id2"}), patch("_streams.cmux.subprocess.run", return_value=reply) as run:
            self.assertEqual(CmuxClient().resolve_workspace(None).ref, "workspace:2")
            self.assertEqual(len(run.call_args_list), 1)
        with patch("_streams.cmux.subprocess.run", return_value=reply):
            self.assertEqual(CmuxClient().resolve_workspace("workspace:2").id, "id2")
        with patch.dict(os.environ, {"CMUX_WORKSPACE_ID": "stale"}), patch("_streams.cmux.subprocess.run", return_value=reply) as run:
            with self.assertRaisesRegex(StreamError, "could not find"):
                CmuxClient().resolve_workspace(None)
            self.assertEqual(len(run.call_args_list), 1)


class BindingTest(unittest.TestCase):
    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        env = patch.dict(os.environ, {"XDG_STATE_HOME": str(self.root)}, clear=True)
        env.start()
        self.addCleanup(env.stop)

    def test_namespace_distinguishes_socket_session_and_workspace(self) -> None:
        paths = {binding_path(endpoint, id) for endpoint in (
            HerdrEndpoint("/one.sock", None), HerdrEndpoint("/two.sock", None), HerdrEndpoint("/one.sock", "named")
        ) for id in ("w1", "w2")}
        self.assertEqual(len(paths), 6)
        self.assertTrue(all(path.is_relative_to(self.root) for path in paths))

    def test_workspace_id_cannot_escape_binding_directory(self) -> None:
        path = binding_path(HerdrEndpoint("/one.sock", None), "../../notes")
        self.assertTrue(path.is_relative_to(self.root / "streams" / "herdr"))

    def test_atomic_rebinding_keeps_old_file_on_replace_failure(self) -> None:
        path = binding_path(HerdrEndpoint("/one.sock", None), "w1")
        write_binding(path, "first")
        with patch("_streams.herdr_binding.os.replace", side_effect=OSError("replace failed")), \
                self.assertRaisesRegex(StreamError, "replace failed"):
            write_binding(path, "second")
        self.assertEqual(json.loads(path.read_text()), {"stream": "first"})
        self.assertEqual(list(path.parent.iterdir()), [path])
        write_binding(path, "second")
        self.assertEqual(json.loads(path.read_text()), {"stream": "second"})

    def test_binding_follows_rename_alias_and_accepts_explicit_rebinding(self) -> None:
        notes = self.root / "notes"
        notes.mkdir()
        stream = stream_at(notes)
        path = binding_path(HerdrEndpoint("/one.sock", None), "w1")
        write_binding(path, stream.slug)
        renamed = rename_stream(stream, "Renamed")
        from _streams.resolve import resolve_stream_ref
        self.assertEqual(resolve_stream_ref(json.loads(path.read_text())["stream"], root=notes, exact=True).slug, renamed.slug)
        other = stream_at(notes, "other", title="Other")
        write_binding(path, other.slug)
        self.assertEqual(resolve_stream_ref(json.loads(path.read_text())["stream"], root=notes, exact=True).slug, "other")


if __name__ == "__main__":
    unittest.main()
