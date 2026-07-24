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

from _streams.cli import build_parser, command_archive, command_format, command_list, command_resume, command_show, command_start
from _streams.cmux import (
    CmuxClient,
    CmuxWorkspace,
    color_for_status,
    description_for_stream,
    load_stream_for_update,
    open_workspace,
    resume_command,
    stream_cwd,
    update_current_workspace,
    update_workspace,
)
from _streams.check import check_path, check_streams, findings_json
from _streams.config import VALID_STATUSES
from _streams.errors import StreamError
from _streams.frontmatter import render_frontmatter, split_frontmatter
from _streams.resolve import resolve_check_path_ref, resolve_slug_ref, resolve_stream_ref
from _streams.resume import exec_resume, resolve_stream_for_resume, resume_command_for_stream
from _streams.store import archive_stream, check_paths, list_streams, show_stream, start_stream


def write_stream(
    root: Path,
    slug: str,
    *,
    title: str = "Example",
    status: str = "active",
    state: str = "Doing work",
    cwd: str = "/repo",
    issues: list[str] | None = None,
    prs: list[str] | None = None,
    sessions: list[str] | None = None,
) -> Path:
    path = root / f"{slug}.md"
    frontmatter = {
        "title": title,
        "status": status,
        "description": "Overall task description",
        "state": state,
        "repo": "/repo",
        "cwd": cwd,
        "vcs": "jj",
        "issues": issues or [],
        "prs": prs or [],
        "sessions": sessions if sessions is not None else ["codex:abc"],
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

    def test_archive_stream_moves_done_stream_to_month(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "done-stream", status="done").stem, root)
            destination = archive_stream(stream, month="2026-06")
            self.assertEqual(destination, root / "archive" / "2026-06" / "done-stream.md")
            self.assertFalse(root.joinpath("done-stream.md").exists())
            self.assertTrue(destination.exists())

    def test_archive_stream_requires_done_without_force(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream", status="active").stem, root)
            with self.assertRaisesRegex(Exception, "use --force"):
                archive_stream(stream, month="2026-06")
            self.assertTrue(root.joinpath("active-stream.md").exists())

    def test_archive_stream_force_allows_non_done(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream", status="active").stem, root)
            destination = archive_stream(stream, month="2026-06", force=True)
            self.assertTrue(destination.exists())

    def test_archive_stream_dry_run_does_not_move(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "done-stream", status="done").stem, root)
            destination = archive_stream(stream, month="2026-06", dry_run=True)
            self.assertEqual(destination, root / "archive" / "2026-06" / "done-stream.md")
            self.assertTrue(root.joinpath("done-stream.md").exists())
            self.assertFalse(destination.exists())

    def test_archive_stream_refuses_overwrite(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "done-stream", status="done").stem, root)
            destination = root / "archive" / "2026-06" / "done-stream.md"
            destination.parent.mkdir(parents=True)
            destination.write_text("existing")
            with self.assertRaisesRegex(Exception, "already exists"):
                archive_stream(stream, month="2026-06")

    def test_archive_stream_validates_month(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "done-stream", status="done").stem, root)
            with self.assertRaisesRegex(Exception, "YYYY-MM"):
                archive_stream(stream, month="2026-13")


class ResolveTest(unittest.TestCase):
    def test_exact_match_wins_over_substring(self) -> None:
        self.assertEqual(resolve_slug_ref("zero", ["zero", "zero-arg-functions"]), "zero")

    def test_multi_word_ref_normalizes_to_slug(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "zero-arg-functions", title="Zero Arg Functions")
            self.assertEqual(resolve_stream_ref("Zero arg", root=root).slug, "zero-arg-functions")

    def test_unique_substring_match_succeeds(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "zero-arg-functions")
            write_stream(root, "other-stream")
            self.assertEqual(resolve_stream_ref("arg", root=root).slug, "zero-arg-functions")

    def test_zero_matches_fails_clearly(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "zero-arg-functions")
            with self.assertRaisesRegex(Exception, "no stream matches"):
                resolve_stream_ref("missing", root=root)

    def test_multiple_matches_fail_clearly(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "zero-arg-functions")
            write_stream(root, "zero-arg-review")
            with self.assertRaisesRegex(Exception, "ambiguous"):
                resolve_stream_ref("zero arg", root=root)

    def test_check_path_ref_resolves_legacy_file_by_substring(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "legacy-note.md").write_text("# Legacy scratch\n")
            self.assertEqual(resolve_check_path_ref("legacy", root=root).name, "legacy-note.md")


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
        with tempfile.TemporaryDirectory() as tmp, patch("_streams.cli.resolve_stream_ref") as show_mock, patch("builtins.print") as print_mock:
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
            args = type("Args", (), {"title": "Active Stream", "json": True, "current": False})()
            command_start(args)
            self.assertEqual(json.loads(print_mock.call_args.args[0])["slug"], "active-stream")

    def test_start_current_updates_current_workspace(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, patch("_streams.cli.start_stream") as start_mock, patch("_streams.cli.update_current_workspace") as update_mock, patch("builtins.print") as print_mock:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream").stem, root)
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "Old"})
            start_mock.return_value = stream
            update_mock.return_value = workspace
            args = type("Args", (), {"title": "Active Stream", "json": False, "current": True})()
            command_start(args)
            update_mock.assert_called_once_with(stream)
            self.assertEqual(print_mock.call_args_list[0].args[0], f"created: active-stream {stream.path}")
            self.assertEqual(print_mock.call_args_list[1].args[0], "updated: workspace:2 active-stream")

    def test_parser_accepts_shortcuts(self) -> None:
        parser = build_parser()
        start_args = parser.parse_args(["s", "Active Stream", "-c"])
        self.assertEqual(start_args.command, "s")
        self.assertEqual(start_args.title, "Active Stream")
        self.assertTrue(start_args.current)

        cmux_args = parser.parse_args(["c", "u", "active", "-c"])
        self.assertEqual(cmux_args.command, "c")
        self.assertEqual(cmux_args.cmux_command, "u")
        self.assertEqual(cmux_args.slug, "active")
        self.assertTrue(cmux_args.current)

        workspace_args = parser.parse_args(["cmux", "update", "active", "-w", "workspace:2"])
        self.assertEqual(workspace_args.workspace, "workspace:2")

        archive_args = parser.parse_args(["a", "done-stream", "--dry-run"])
        self.assertEqual(archive_args.command, "a")
        self.assertEqual(archive_args.slug, "done-stream")
        self.assertTrue(archive_args.dry_run)

    def test_archive_prints_destination(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, patch("_streams.cli.resolve_stream_ref") as resolve_mock, patch("builtins.print") as print_mock:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "done-stream", status="done").stem, root)
            resolve_mock.return_value = stream
            args = type("Args", (), {"slug": "done", "month": "2026-06", "force": False, "dry_run": True})()
            self.assertEqual(command_archive(args), 0)
            self.assertEqual(print_mock.call_args.args[0], f"would archive: done-stream {root / 'archive' / '2026-06' / 'done-stream.md'}")

    def test_check_json_reports_findings(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, patch("_streams.cli.resolve_check_path_ref") as resolve_mock, patch("builtins.print") as print_mock:
            root = Path(tmp)
            path = write_stream(root, "active-stream")
            text = path.read_text().replace("status: active", "status: nope")
            path.write_text(text)
            resolve_mock.return_value = path
            args = type("Args", (), {"slug": "active-stream", "json": True})()
            from _streams.cli import command_check

            self.assertEqual(command_check(args), 1)
            payload = json.loads(print_mock.call_args.args[0])
            self.assertEqual(payload[0]["level"], "error")

    def test_format_prints_markdown_spec(self) -> None:
        with patch("builtins.print") as print_mock:
            args = type("Args", (), {})()
            self.assertEqual(command_format(args), 0)
            output = print_mock.call_args.args[0]
            self.assertIn("# Streams Scratch Format", output)
            self.assertIn("https://github.com/<owner>/<repo>/pull/<number>", output)

    def test_resume_execs_resolved_stream(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, patch("_streams.cli.resolve_stream_for_resume") as resolve_mock, patch("_streams.cli.exec_resume") as exec_mock:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream").stem, root)
            resolve_mock.return_value = stream
            args = type("Args", (), {"slug": "active"})()
            self.assertEqual(command_resume(args), 0)
            resolve_mock.assert_called_once_with("active")
            exec_mock.assert_called_once_with(stream)


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

    def test_review_is_not_a_status(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "review-stream", status="review")
            findings = check_path(path)
            self.assertTrue(any("invalid status" in finding.message for finding in findings))

    def test_statuses_describe_actionability(self) -> None:
        self.assertEqual(VALID_STATUSES, {"active", "waiting", "blocked", "parked", "done"})

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

    def test_issue_must_be_github_issue_url(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "active-stream", issues=["https://github.com/example-org/widget-lib/pull/150"])
            findings = check_path(path)
            self.assertTrue(any("issues[0] must be a GitHub issue URL" in finding.message for finding in findings))

    def test_pr_must_be_github_pull_url(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "active-stream", prs=["widget-lib#150"])
            findings = check_path(path)
            self.assertTrue(any("prs[0] must be a GitHub pull request URL" in finding.message for finding in findings))

    def test_github_issue_and_pr_urls_are_valid(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(
                root,
                "active-stream",
                title="Active Stream",
                issues=["https://github.com/example-org/widget-lib/issues/124"],
                prs=["https://github.com/example-org/widget-lib/pull/150"],
            )
            self.assertEqual(check_path(path), [])

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
        self.assertEqual(color_for_status("waiting"), "Amber")
        self.assertEqual(color_for_status("blocked"), "Orange")
        self.assertEqual(color_for_status("unknown"), "Amber")

    def test_resume_command_quotes_session_ids(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream").stem, root)
            stream.frontmatter["sessions"] = ["claude:session with spaces"]
            self.assertEqual(resume_command(stream), "claude --resume 'session with spaces'")

    def test_resume_command_uses_last_session(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(
                write_stream(root, "active-stream", sessions=["codex:old", "claude:new session"]).stem,
                root,
            )
            self.assertEqual(resume_command(stream), "claude --resume 'new session'")

    def test_open_workspace_uses_stream_fields(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            vault = Path(tmp)
            root = vault / "scratch"
            root.mkdir()
            cwd = root / "repo"
            cwd.mkdir()
            stream = show_stream(write_stream(root, "active-stream", title="Active Stream", state="Ready to work", cwd=str(cwd)).stem, root)
            cmux = FakeCmux()
            with patch("_streams.cmux.WORKNOTES_ROOT", vault):
                open_workspace(stream, cmux)
            self.assertEqual(
                cmux.commands[0],
                [
                    "cmux",
                    "new-workspace",
                    "--name",
                    "Active Stream",
                    "--description",
                    "[🔗](obsidian://open?vault=worknotes&file=scratch%2Factive-stream.md) _Ready to work_",
                    "--cwd",
                    str(cwd),
                    "--command",
                    "codex resume abc",
                ],
            )

    def test_open_workspace_requires_existing_cwd(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream", cwd=str(root / "missing")).stem, root)
            with self.assertRaisesRegex(Exception, "cwd does not exist"):
                open_workspace(stream, FakeCmux())

    def test_stream_cwd_requires_directory(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            file_path = root / "file"
            file_path.write_text("not a directory")
            stream = show_stream(write_stream(root, "active-stream", cwd=str(file_path)).stem, root)
            with self.assertRaisesRegex(Exception, "cwd is not a directory"):
                stream_cwd(stream)

    def test_update_workspace_sets_title_description_and_color(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            vault = Path(tmp)
            root = vault / "scratch"
            root.mkdir()
            stream = show_stream(write_stream(root, "active-stream", title="Active Stream", state="Ready to work").stem, root)
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "Old"})
            cmux = FakeCmux()
            with patch("_streams.cmux.WORKNOTES_ROOT", vault):
                update_workspace(stream, workspace, cmux)
            self.assertEqual(cmux.commands[0], ["cmux", "workspace-action", "--workspace", "workspace:2", "--action", "rename", "--title", "Active Stream"])
            self.assertEqual(cmux.commands[1], ["cmux", "workspace-action", "--workspace", "workspace:2", "--action", "set-description", "--description", "[🔗](obsidian://open?vault=worknotes&file=scratch%2Factive-stream.md) _Ready to work_"])
            self.assertEqual(cmux.commands[2], ["cmux", "workspace-action", "--workspace", "workspace:2", "--action", "set-color", "--color", "Green"])

    def test_update_current_workspace_resolves_and_updates_current(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            vault = Path(tmp)
            root = vault / "scratch"
            root.mkdir()
            stream = show_stream(write_stream(root, "active-stream", title="Active Stream", state="Ready to work").stem, root)
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "Old"})
            cmux = FakeCmux([workspace])
            with patch("_streams.cmux.WORKNOTES_ROOT", vault):
                resolved = update_current_workspace(stream, cmux)
            self.assertEqual(resolved.display_ref, "workspace:2")
            self.assertEqual(cmux.commands[0], ["cmux", "workspace-action", "--workspace", "workspace:2", "--action", "rename", "--title", "Active Stream"])

    def test_description_rejects_stream_outside_worknotes_root(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream").stem, root)
            with patch("_streams.cmux.WORKNOTES_ROOT", root / "worknotes"):
                with self.assertRaisesRegex(StreamError, "outside worknotes root"):
                    description_for_stream(stream)

    def test_description_includes_italic_state_and_all_links(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            vault = Path(tmp)
            root = vault / "scratch"
            root.mkdir()
            stream = show_stream(
                write_stream(
                    root,
                    "active-stream",
                    state="Issue #124 literal conversion helpers",
                    issues=[
                        "https://github.com/example-org/widget-lib/issues/124",
                        "https://github.com/openai/codex/issues/7",
                    ],
                    prs=[
                        "https://github.com/example-org/widget-lib/pull/150",
                        "https://github.com/openai/codex/pull/42",
                    ],
                ).stem,
                root,
            )
            with patch("_streams.cmux.WORKNOTES_ROOT", vault):
                description = description_for_stream(stream)
            self.assertEqual(
                description,
                "\n".join(
                    [
                        "[🔗](obsidian://open?vault=worknotes&file=scratch%2Factive-stream.md) _Issue #124 literal conversion helpers_",
                        "[widget-lib#124](https://github.com/example-org/widget-lib/issues/124)",
                        "[codex#7](https://github.com/openai/codex/issues/7)",
                        "[widget-lib#150](https://github.com/example-org/widget-lib/pull/150)",
                        "[codex#42](https://github.com/openai/codex/pull/42)",
                    ]
                ),
            )

    def test_update_current_resolves_stream_from_workspace_title(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "active-stream", title="Active Stream")
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "Active Stream"})
            cmux = FakeCmux([workspace])
            stream, resolved = load_stream_for_update(None, None, True, cmux, root=root)
            self.assertEqual(stream.slug, "active-stream")
            self.assertEqual(resolved.display_ref, "workspace:2")

    def test_update_current_with_explicit_slug_uses_current_workspace_target_only(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "active-stream", title="Active Stream")
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "Dotfiles"})
            cmux = FakeCmux([workspace])
            stream, resolved = load_stream_for_update("active", None, True, cmux, root=root)
            self.assertEqual(stream.slug, "active-stream")
            self.assertEqual(resolved.title, "Dotfiles")

    def test_update_current_falls_back_to_unique_cwd(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "active-stream", title="Active Stream", cwd="/repo/active")
            write_stream(root, "other-stream", title="Other Stream", cwd="/repo/other")
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "streams cmux update --current", "current_directory": "/repo/active"})
            cmux = FakeCmux([workspace])
            stream, resolved = load_stream_for_update(None, None, True, cmux, root=root)
            self.assertEqual(stream.slug, "active-stream")
            self.assertEqual(resolved.display_ref, "workspace:2")

    def test_update_current_without_identifying_hint_fails_clearly(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "active-stream", title="Active Stream", cwd="/repo/active")
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "streams cmux update --current", "current_directory": "/repo/missing"})
            cmux = FakeCmux([workspace])
            with self.assertRaisesRegex(Exception, "pass a slug"):
                load_stream_for_update(None, None, True, cmux, root=root)


class ResumeTest(unittest.TestCase):
    def test_resume_command_for_stream_uses_last_session(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(
                write_stream(root, "active-stream", sessions=["codex:first", "pi:last"]).stem,
                root,
            )
            self.assertEqual(resume_command_for_stream(stream).argv, ("pi", "--session", "last"))

    def test_exec_resume_changes_to_stream_cwd_before_exec(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, patch("_streams.resume.os.chdir") as chdir_mock, patch("_streams.resume.os.execvp") as exec_mock:
            root = Path(tmp)
            cwd = root / "repo"
            cwd.mkdir()
            stream = show_stream(write_stream(root, "active-stream", cwd=str(cwd), sessions=["codex:abc"]).stem, root)
            exec_resume(stream)
            chdir_mock.assert_called_once_with(cwd)
            exec_mock.assert_called_once_with("codex", ["codex", "resume", "abc"])

    def test_exec_resume_requires_existing_cwd(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, patch("_streams.resume.os.execvp") as exec_mock:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream", cwd=str(root / "missing")).stem, root)
            with self.assertRaisesRegex(Exception, "cwd does not exist"):
                exec_resume(stream)
            exec_mock.assert_not_called()

    def test_resume_command_for_stream_requires_session(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream", sessions=[]).stem, root)
            with self.assertRaisesRegex(Exception, "no recorded AI sessions"):
                resume_command_for_stream(stream)

    def test_resolve_resume_with_explicit_ref(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "active-stream", title="Active Stream")
            self.assertEqual(resolve_stream_for_resume("active", root=root).slug, "active-stream")

    def test_resolve_resume_without_ref_uses_current_cmux_workspace(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "active-stream", title="Active Stream")
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "Active Stream"})
            cmux = FakeCmux([workspace])
            self.assertEqual(resolve_stream_for_resume(None, cmux=cmux, root=root).slug, "active-stream")

    def test_resolve_resume_without_ref_fails_clearly(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "active-stream", title="Active Stream", cwd="/repo/active")
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "unknown", "current_directory": "/repo/missing"})
            cmux = FakeCmux([workspace])
            with self.assertRaisesRegex(Exception, "could not infer stream to resume"):
                resolve_stream_for_resume(None, cmux=cmux, root=root)


if __name__ == "__main__":
    unittest.main()
