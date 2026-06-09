from __future__ import annotations

import json
import sys
import tempfile
import unittest
from datetime import date
from pathlib import Path
from unittest.mock import patch

BIN_DIR = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(BIN_DIR))

from _streams.cli import command_list, command_show, command_start
from _streams.cmux import CmuxClient, CmuxWorkspace, color_for_status, load_stream_for_update, open_workspace, resume_command, update_workspace
from _streams.check import check_path, check_streams, findings_json
from _streams.frontmatter import render_frontmatter, split_frontmatter
from _streams.store import check_paths, list_streams, show_stream, start_stream


def write_stream(root: Path, slug: str, *, title: str = "Example", status: str = "active", state: str = "Doing work") -> Path:
    path = root / f"{slug}.md"
    frontmatter = {
        "title": title,
        "status": status,
        "description": "Overall task description",
        "state": state,
        "repo": "/repo",
        "cwd": "/repo",
        "vcs": "jj",
        "issues": [],
        "prs": [],
        "sessions": ["codex:abc"],
    }
    path.write_text(render_frontmatter(frontmatter, "# Body\n"))
    return path


class FrontmatterTest(unittest.TestCase):
    def test_round_trip_frontmatter(self) -> None:
        text = render_frontmatter({"title": "Example", "sessions": ["codex:abc"]}, "# Body\n")
        frontmatter, body = split_frontmatter(text)
        self.assertEqual(frontmatter["title"], "Example")
        self.assertEqual(frontmatter["sessions"], ["codex:abc"])
        self.assertEqual(body, "# Body\n")


class StoreTest(unittest.TestCase):
    def test_list_excludes_archive(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "active-stream")
            archive = root / "archive" / "2026-06"
            archive.mkdir(parents=True)
            write_stream(archive, "old-stream")
            self.assertEqual([stream.slug for stream in list_streams(root)], ["active-stream"])

    def test_list_skips_legacy_files_without_frontmatter(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "active-stream")
            (root / "legacy.md").write_text("# Legacy scratch\n")
            self.assertEqual([stream.slug for stream in list_streams(root)], ["active-stream"])

    def test_show_stream_loads_by_slug(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "active-stream", title="Active Stream")
            self.assertEqual(show_stream("active-stream", root).title, "Active Stream")

    def test_json_payload_converts_yaml_dates(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream").stem, root)
            stream.frontmatter["created"] = date(2026, 6, 8)
            self.assertEqual(stream.as_json()["created"], "2026-06-08")

    def test_start_stream_creates_active_stream(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cwd = root / "repo"
            cwd.mkdir()
            stream = start_stream("New Stream", root=root, cwd=cwd)
            self.assertEqual(stream.slug, "new-stream")
            self.assertEqual(stream.status, "active")
            self.assertEqual(stream.frontmatter["sessions"], [])
            self.assertTrue((root / "new-stream.md").exists())

    def test_start_stream_refuses_overwrite(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            start_stream("New Stream", root=root, cwd=root)
            with self.assertRaisesRegex(Exception, "already exists"):
                start_stream("New Stream", root=root, cwd=root)


class CliTest(unittest.TestCase):
    def test_list_json_shape(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, patch("_streams.cli.list_streams") as list_mock, patch("builtins.print") as print_mock:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream").stem, root)
            list_mock.return_value = [stream]
            args = type("Args", (), {"json": True})()
            command_list(args)
            payload = json.loads(print_mock.call_args.args[0])
            self.assertEqual(payload[0]["slug"], "active-stream")
            self.assertEqual(payload[0]["path"], str(stream.path))

    def test_show_human_includes_slug(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, patch("_streams.cli.show_stream") as show_mock, patch("builtins.print") as print_mock:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream").stem, root)
            show_mock.return_value = stream
            args = type("Args", (), {"slug": "active-stream", "json": False})()
            command_show(args)
            self.assertIn("slug:        active-stream", print_mock.call_args.args[0])

    def test_start_json_shape(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, patch("_streams.cli.start_stream") as start_mock, patch("builtins.print") as print_mock:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream").stem, root)
            start_mock.return_value = stream
            args = type("Args", (), {"title": "Active Stream", "json": True})()
            command_start(args)
            self.assertEqual(json.loads(print_mock.call_args.args[0])["slug"], "active-stream")

    def test_check_json_reports_findings(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, patch("_streams.cli.SCRATCH_ROOT", Path(tmp)), patch("builtins.print") as print_mock:
            root = Path(tmp)
            path = write_stream(root, "active-stream")
            text = path.read_text().replace("status: active", "status: nope")
            path.write_text(text)
            args = type("Args", (), {"slug": "active-stream", "json": True})()
            from _streams.cli import command_check

            self.assertEqual(command_check(args), 1)
            payload = json.loads(print_mock.call_args.args[0])
            self.assertEqual(payload[0]["level"], "error")


class CheckTest(unittest.TestCase):
    def test_valid_stream_has_no_findings(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "active-stream", title="Active Stream")
            self.assertEqual(check_path(path), [])

    def test_check_paths_include_legacy_files_without_frontmatter(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "active-stream")
            (root / "legacy.md").write_text("# Legacy scratch\n")
            self.assertEqual([path.name for path in check_paths(root)], ["active-stream.md", "legacy.md"])

    def test_invalid_status_is_error(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "active-stream")
            path.write_text(path.read_text().replace("status: active", "status: nope"))
            findings = check_path(path)
            self.assertTrue(any("invalid status" in finding.message for finding in findings))

    def test_forbidden_slug_field_is_error(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "active-stream")
            path.write_text(path.read_text().replace("title: Example", "slug: active-stream\ntitle: Example"))
            findings = check_path(path)
            self.assertTrue(any("slug" in finding.message for finding in findings))

    def test_invalid_session_shape_is_error(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "active-stream")
            path.write_text(path.read_text().replace("- codex:abc", "- abc"))
            findings = check_path(path)
            self.assertTrue(any("sessions[0]" in finding.message for finding in findings))

    def test_slug_title_mismatch_is_warning(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "not-title", title="Different Title")
            findings = check_path(path)
            self.assertTrue(any(finding.level == "warn" for finding in findings))

    def test_findings_json(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "active-stream")
            path.write_text(path.read_text().replace("status: active", "status: nope"))
            payload = json.loads(findings_json(check_streams([path])))
            self.assertEqual(payload[0]["slug"], "active-stream")


class FakeCmux(CmuxClient):
    def __init__(self, workspaces: list[CmuxWorkspace] | None = None) -> None:
        self.commands: list[list[str]] = []
        self._workspaces = workspaces or []

    def run(self, args: list[str]):
        self.commands.append(args)
        return type("Completed", (), {"stdout": "workspace:1\n"})()

    def workspaces(self) -> list[CmuxWorkspace]:
        return self._workspaces

    def current_workspace_ref(self) -> str:
        return self._workspaces[0].display_ref if self._workspaces else "workspace:1"


class CmuxTest(unittest.TestCase):
    def test_color_for_status(self) -> None:
        self.assertEqual(color_for_status("active"), "Green")
        self.assertEqual(color_for_status("blocked"), "Orange")
        self.assertEqual(color_for_status("unknown"), "Amber")

    def test_resume_command_quotes_session_ids(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream").stem, root)
            stream.frontmatter["sessions"] = ["claude:session with spaces"]
            self.assertEqual(resume_command(stream), "claude --resume 'session with spaces'")

    def test_open_workspace_uses_stream_fields(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream", title="Active Stream", state="Ready to work").stem, root)
            cmux = FakeCmux()
            open_workspace(stream, cmux)
            self.assertEqual(
                cmux.commands[0],
                [
                    "cmux",
                    "new-workspace",
                    "--name",
                    "Active Stream",
                    "--description",
                    "Ready to work",
                    "--cwd",
                    "/repo",
                    "--command",
                    "codex --no-sandbox resume abc",
                ],
            )

    def test_update_workspace_sets_title_description_and_color(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream", title="Active Stream", state="Ready to work").stem, root)
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "Old"})
            cmux = FakeCmux()
            update_workspace(stream, workspace, cmux)
            self.assertEqual(cmux.commands[0], ["cmux", "workspace-action", "--workspace", "workspace:2", "--action", "rename", "--title", "Active Stream"])
            self.assertEqual(cmux.commands[1], ["cmux", "workspace-action", "--workspace", "workspace:2", "--action", "set-description", "--description", "Ready to work"])
            self.assertEqual(cmux.commands[2], ["cmux", "workspace-action", "--workspace", "workspace:2", "--action", "set-color", "--color", "Green"])

    def test_update_current_resolves_stream_from_workspace_title(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "active-stream", title="Active Stream")
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "Active Stream"})
            cmux = FakeCmux([workspace])
            with patch("_streams.cmux.show_stream", side_effect=lambda slug: show_stream(slug, root)):
                stream, resolved = load_stream_for_update(None, None, True, cmux)
            self.assertEqual(stream.slug, "active-stream")
            self.assertEqual(resolved.display_ref, "workspace:2")

    def test_update_current_with_explicit_slug_uses_current_workspace_target_only(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "active-stream", title="Active Stream")
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "Dotfiles"})
            cmux = FakeCmux([workspace])
            with patch("_streams.cmux.show_stream", side_effect=lambda slug: show_stream(slug, root)):
                stream, resolved = load_stream_for_update("active-stream", None, True, cmux)
            self.assertEqual(stream.slug, "active-stream")
            self.assertEqual(resolved.title, "Dotfiles")


if __name__ == "__main__":
    unittest.main()
