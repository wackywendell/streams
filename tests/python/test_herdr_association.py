from __future__ import annotations

import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.cli import main
from _streams.config import set_config
from _streams.current import CurrentHerdr
from _streams.errors import StreamError
from _streams.herdr import HerdrWorkspace, load_stream_for_update, update_workspace
from _streams.herdr_binding import HerdrEndpoint, binding_path, read_binding, write_binding
from _streams.herdr_links import LinksView
from _streams.store import rename_stream
from test_herdr import FakeHerdr, pane, response, status, stream_at, workspace


class HerdrAssociationTest(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.notes = self.root / "scratch"
        self.notes.mkdir()
        self.enterContext(patch.dict(os.environ, {
            "HOME": str(self.root), "XDG_CONFIG_HOME": str(self.root / "config"),
            "XDG_STATE_HOME": str(self.root / "state"), "HERDR_ENV": "1", "HERDR_PANE_ID": "old:pane",
        }, clear=True))
        set_config("notes.root", str(self.root))
        self.alpha = stream_at(self.notes, "alpha", title="Alpha", repo=str(self.root), cwd=str(self.notes))
        self.beta = stream_at(self.notes, "beta", title="Beta", repo=str(self.root), cwd=str(self.root))
        self.target = HerdrWorkspace("w2", "Alpha", str(self.notes))
        self.endpoint = HerdrEndpoint("/actual/herdr.sock", None)
        self.binding = binding_path(self.endpoint, "w2")

    def client(self, **replies) -> FakeHerdr:
        return FakeHerdr({("status", "--json"): status(),
                          ("workspace", "get", "w2"): workspace(label="Alpha"),
                          ("pane", "current", "--current"): pane(str(self.notes)), **replies})

    def current(self, client: FakeHerdr | None = None):
        client = client or self.client()
        return load_stream_for_update(None, "w2", False, client, root=self.notes)[0]

    def test_association_overrides_both_label_and_cwd_for_update_and_resume(self) -> None:
        write_binding(self.binding, "beta")
        self.assertEqual(self.current().slug, "beta")
        client = self.client()
        self.assertEqual(CurrentHerdr(client, self.target).infer_stream(root=self.notes).slug, "beta")
        self.assertEqual(client.calls, [["status", "--json"]])

    def test_live_caller_workspace_and_actual_endpoint_own_the_association(self) -> None:
        write_binding(self.binding, "beta")
        write_binding(binding_path(self.endpoint, "w1"), "alpha")
        write_binding(binding_path(HerdrEndpoint("/stale/socket", None), "w2"), "alpha")
        with patch.dict(os.environ, {"HERDR_WORKSPACE_ID": "w1", "HERDR_SOCKET_PATH": "/stale/socket"}):
            client = self.client()
            stream, target = load_stream_for_update(None, None, True, client, root=self.notes)
        self.assertEqual((stream.slug, target.id), ("beta", "w2"))
        self.assertEqual(client.calls, [["pane", "current", "--current"], ["workspace", "get", "w2"], ["status", "--json"]])

    def test_unassociated_workspace_uses_existing_conservative_hints_without_writing(self) -> None:
        self.assertEqual(self.current().slug, "alpha")
        client = self.client()
        client.replies[("workspace", "get", "w2")] = workspace(label="Shell")
        stream, _ = load_stream_for_update(None, None, True, client, root=self.notes)
        self.assertEqual(stream.slug, "alpha")
        self.assertFalse(self.binding.exists())
        self.assertFalse(self.binding.parent.exists())

    def test_another_server_or_session_association_is_not_used(self) -> None:
        for endpoint in (HerdrEndpoint("/other.sock", None), HerdrEndpoint("/actual/herdr.sock", "other")):
            write_binding(binding_path(endpoint, "w2"), "beta")
        self.assertEqual(self.current().slug, "alpha")
        self.assertFalse(self.binding.exists())

    def test_malformed_unreadable_and_dangling_associations_do_not_fall_back(self) -> None:
        self.binding.parent.mkdir(parents=True)
        for text in ("not json", "{}", '{"stream":"Beta"}', '{"stream":"beta.md"}', '{"stream":"../beta"}'):
            self.binding.write_text(text)
            with self.subTest(text=text), self.assertRaises(StreamError):
                self.current()
        with patch("_streams.herdr_binding.Path.read_text", side_effect=PermissionError("unreadable")), self.assertRaisesRegex(StreamError, "unreadable"):
            self.current()
        self.binding.unlink()
        self.binding.symlink_to("absent.json")
        with self.assertRaises(StreamError):
            self.current()

    def test_missing_archived_invalid_and_unsafe_bound_notes_do_not_fall_back(self) -> None:
        archive = self.notes / "archive"
        archive.mkdir()
        stream_at(self.notes, "missing-other", title="Missing")
        stream_at(self.notes, "archived").path.rename(archive / "archived.md")
        invalid = stream_at(self.notes, "invalid")
        invalid.path.write_text(invalid.path.read_text().replace("status: active", "status: invalid"))
        (self.notes / "cyclic.md").symlink_to("cyclic.md")
        (self.notes / "external.md").symlink_to(stream_at(self.root, "external").path)
        (self.notes / "old-archive.md").symlink_to("archive/archived.md")
        for slug in ("missing", "archived", "invalid", "cyclic", "external", "old-archive"):
            write_binding(self.binding, slug)
            with self.subTest(slug=slug), self.assertRaises(StreamError):
                self.current()

    def test_repeated_rename_aliases_resolve_canonical_identity(self) -> None:
        write_binding(self.binding, "beta")
        renamed = rename_stream(rename_stream(self.beta, "First"), "Last")
        self.assertEqual(self.current().path, renamed.path)
        self.assertEqual(read_binding(self.binding), "beta")

    def test_cli_resume_uses_association_and_never_executes_a_stale_label(self) -> None:
        write_binding(self.binding, "beta")
        with patch("_streams.current.HerdrClient", return_value=self.client()), patch("_streams.cli.exec_resume") as execute:
            self.assertEqual(main(["resume", "-f", "herdr"]), 0)
        self.assertEqual(execute.call_args.args[0].slug, "beta")
        write_binding(self.binding, "missing")
        with patch("_streams.current.HerdrClient", return_value=self.client()), patch("_streams.cli.exec_resume") as execute, patch("sys.stderr", io.StringIO()):
            self.assertEqual(main(["resume", "-f", "herdr"]), 2)
        execute.assert_not_called()

    def test_association_is_published_before_any_native_display_change(self) -> None:
        write_binding(self.binding, "alpha")
        client = self.client()
        client.replies.update({("workspace", "rename", "w2", "Beta"): workspace(label="Beta"),
                               ("workspace", "report-metadata", "w2", "--source", "streams", "--token", "stream_slug=beta",
                                "--token", "stream_status=🟩", "--token", "stream_state=Ready", "--token", "stream_summary=🟩 Ready"): ""})
        run = client.run
        def check_association(args):
            if args[:1] == ["workspace"]:
                self.assertEqual(read_binding(self.binding), "beta")
            return run(args)
        client.run = check_association
        update_workspace(self.beta, self.target, client)
        self.assertEqual(self.current().slug, "beta")

    def test_binding_failure_preserves_old_association_and_prevents_native_changes(self) -> None:
        write_binding(self.binding, "alpha")
        client = self.client()
        with patch("_streams.herdr_binding.os.replace", side_effect=OSError("disk unavailable")), self.assertRaisesRegex(StreamError, "disk unavailable"):
            update_workspace(self.beta, self.target, client)
        self.assertEqual(read_binding(self.binding), "alpha")
        self.assertEqual(client.calls, [["status", "--json"]])
        self.assertEqual(list(self.binding.parent.iterdir()), [self.binding])

    def test_failed_rename_keeps_new_association_for_current_lookup_and_links(self) -> None:
        write_binding(self.binding, "alpha")
        client = self.client()
        client.replies[("workspace", "rename", "w2", "Beta")] = {"error": {"message": "rename unavailable"}}
        with self.assertRaisesRegex(StreamError, "association.*beta.*display.*rename unavailable"):
            update_workspace(self.beta, self.target, client)
        self.assertEqual(self.current().slug, "beta")
        view = LinksView(self.binding)
        self.assertIn("Beta", view.refresh())  # Real, read-only pinned CLI lookup.
        self.assertEqual(view.stream.slug, "beta")
        self.assertEqual(client.calls, [["status", "--json"], ["workspace", "rename", "w2", "Beta"]])

    def test_explicit_stream_repairs_a_broken_association(self) -> None:
        self.binding.parent.mkdir(parents=True)
        self.binding.write_text("invalid association")
        client = self.client()
        client.replies.update({("workspace", "rename", "w2", "Beta"): workspace(label="Beta"),
                               ("workspace", "report-metadata", "w2", "--source", "streams", "--token", "stream_slug=beta",
                                "--token", "stream_status=🟩", "--token", "stream_state=Ready", "--token", "stream_summary=🟩 Ready"): ""})
        with patch("_streams.cli.HerdrClient", return_value=client), patch("sys.stdout", io.StringIO()):
            self.assertEqual(main(["herdr", "update", "beta", "-w", "w2"]), 0)
        self.assertEqual(read_binding(self.binding), "beta")
        self.assertEqual(self.current().slug, "beta")

    def test_invalid_binding_write_is_rejected_before_filesystem_changes(self) -> None:
        for ref in ("Beta", "beta.md", "../beta", " ", ""):
            with self.subTest(ref=ref), self.assertRaises(StreamError):
                write_binding(self.binding, ref)
        self.assertFalse(self.binding.parent.exists())


if __name__ == "__main__":
    unittest.main()
