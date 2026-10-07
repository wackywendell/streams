from __future__ import annotations

import hashlib
import io
import json
import subprocess
import sys
import tempfile
import unittest
from datetime import date
from pathlib import Path
from unittest.mock import patch

SRC_DIR = Path(__file__).resolve().parents[2] / "src"
sys.path.insert(0, str(SRC_DIR))


def setUpModule() -> None:
    # Keep the developer's ~/.config/streams/config.json (e.g. obsidian.vault) out of expected output.
    config_dir = tempfile.TemporaryDirectory()
    env = patch.dict("os.environ", {"XDG_CONFIG_HOME": config_dir.name})
    env.start()
    unittest.addModuleCleanup(config_dir.cleanup)
    unittest.addModuleCleanup(env.stop)

from _streams.agent_sessions import latest_session_command
from _streams.cli import main as cli_main, build_parser, command_archive, command_check, command_cmux_update, command_format, command_list, command_rename, command_resume, command_show, command_start
from _streams.cmux import (
    CmuxClient,
    CmuxWorkspace,
    color_for_status,
    description_for_stream,
    load_stream_for_update,
    open_workspace,
    stream_cwd,
    update_all_workspaces,
    update_current_workspace,
    update_workspace,
)
from _streams.check import check_path, check_streams, findings_json
from _streams.config import VALID_STATUSES
from _streams.errors import StreamError
from _streams.frontmatter import render_frontmatter, split_frontmatter
from _streams.location import (
    PathAlias,
    StreamLocation,
    discover_home_path_aliases,
    discover_stream_location,
    format_local_path,
)
from _streams.models import Link, Stream
from _streams.resolve import resolve_check_path_ref, resolve_slug_ref, resolve_stream_ref
from _streams.resume import exec_resume, resolve_stream_for_resume, resume_command_for_stream
from _streams.store import archive_stream, check_paths, list_streams, rename_stream, show_stream, start_stream


def make_stream(path: Path, body: str = "", **fields: object) -> Stream:
    """A valid `Stream` for tests that don't need a file on disk."""
    frontmatter = {"title": "Example", "status": "active", "description": "Overall task description", "state": "Doing work", "repo": "/repo", "cwd": "/repo", "vcs": "jj", "issues": [], "prs": [], "sessions": []}
    return Stream.parse(path, {**frontmatter, **fields}, body)


def write_stream(
    root: Path,
    slug: str,
    *,
    title: str = "Example",
    status: str = "active",
    state: str = "Doing work",
    repo: str | None = "/repo",
    cwd: str = "/repo",
    vcs: str | None = "jj",
    issues: list[dict[str, str]] | None = None,
    prs: list[dict[str, str]] | None = None,
    sessions: list[str] | None = None,
) -> Path:
    path = root / f"{slug}.md"
    frontmatter = {
        "title": title,
        "status": status,
        "description": "Overall task description",
        "state": state,
        "repo": repo,
        "cwd": cwd,
        "vcs": vcs,
        "issues": issues or [],
        "prs": prs or [],
        "sessions": sessions if sessions is not None else ["codex:abc"],
    }
    path.write_text(render_frontmatter(frontmatter, "# Body\n"))
    return path


def dated(text: str) -> str:
    """`text` as `streams update` writes it: `write_stream` notes have no `updated`, so it's added, dated today."""
    return text.replace("\n---\n", f"\nupdated: {date.today().isoformat()}\n---\n", 1)


class FrontmatterTest(unittest.TestCase):
    def test_round_trip_frontmatter(self) -> None:
        text = render_frontmatter({"title": "Example", "sessions": ["codex:abc"]}, "# Body\n")
        frontmatter, body = split_frontmatter(text)
        self.assertEqual(frontmatter["title"], "Example")
        self.assertEqual(frontmatter["sessions"], ["codex:abc"])
        self.assertEqual(body, "# Body\n")


class StreamLocationTest(unittest.TestCase):
    def test_format_local_path_uses_tilde_for_paths_under_home(self) -> None:
        self.assertEqual(
            format_local_path(Path("/home/example/repo"), home=Path("/home/example")),
            "~/repo",
        )
        self.assertEqual(
            format_local_path(Path("/home/example"), home=Path("/home/example")),
            "~",
        )

    def test_format_local_path_preserves_paths_outside_home(self) -> None:
        self.assertEqual(
            format_local_path(Path("/work/repo"), home=Path("/home/example")),
            "/work/repo",
        )

    def test_format_local_path_prefers_matching_home_symlink(self) -> None:
        alias = PathAlias(
            display_root="~/work",
            canonical_root=Path("/home/example/go/src/github.com/example-org"),
        )
        self.assertEqual(
            format_local_path(
                Path("/home/example/go/src/github.com/example-org/monorepo"),
                home=Path("/home/example"),
                aliases=(alias,),
            ),
            "~/work/monorepo",
        )

    def test_format_local_path_prefers_most_specific_matching_alias(self) -> None:
        aliases = (
            PathAlias(
                display_root="~/github",
                canonical_root=Path("/home/example/go/src/github.com"),
            ),
            PathAlias(
                display_root="~/work",
                canonical_root=Path("/home/example/go/src/github.com/example-org"),
            ),
        )
        self.assertEqual(
            format_local_path(
                Path("/home/example/go/src/github.com/example-org/monorepo"),
                home=Path("/home/example"),
                aliases=aliases,
            ),
            "~/work/monorepo",
        )

    def test_discover_home_path_aliases_finds_direct_directory_symlinks(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp) / "home"
            target = home / "go" / "src" / "github.com" / "example-org"
            target.mkdir(parents=True)
            (home / "work").symlink_to(target, target_is_directory=True)

            self.assertEqual(
                discover_home_path_aliases(home),
                (
                    PathAlias(
                        display_root="~/work",
                        canonical_root=target.resolve(),
                    ),
                ),
            )

    def test_location_is_dedicated_when_working_root_differs_from_repo_root(self) -> None:
        location = StreamLocation(vcs="jj", repo_root=Path("/repo"), working_root=Path("/repo/workspaces/topic"))
        self.assertTrue(location.is_dedicated)

    def test_location_is_main_when_roots_are_the_same(self) -> None:
        location = StreamLocation(vcs="git", repo_root=Path("/repo"), working_root=Path("/repo"))
        self.assertFalse(location.is_dedicated)

    def test_discover_jj_location_uses_default_and_current_workspace_roots(self) -> None:
        def run(args: list[str], cwd: Path) -> str:
            if args[:3] == ["jj", "workspace", "root"]:
                return "/repo/workspaces/topic\n"
            if args[:3] == ["jj", "workspace", "list"]:
                return "/repo\n"
            raise AssertionError(args)

        location = discover_stream_location(Path("/repo/workspaces/topic/subdir"), runner=run)
        self.assertEqual(
            location,
            StreamLocation(vcs="jj", repo_root=Path("/repo"), working_root=Path("/repo/workspaces/topic")),
        )

    def test_discover_jj_location_supports_legacy_workspace_root_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = root / "repo"
            working = root / "workspaces" / "topic"
            repo.joinpath(".jj", "repo").mkdir(parents=True)
            working.joinpath(".jj").mkdir(parents=True)
            pointer = Path("../../../repo/.jj/repo")
            working.joinpath(".jj", "repo").write_text(str(pointer))

            def run(args: list[str], cwd: Path) -> str:
                if args[:3] == ["jj", "workspace", "root"]:
                    return f"{working}\n"
                if args[:3] == ["jj", "workspace", "list"]:
                    return "<Error: No FsPath available>\n"
                raise AssertionError(args)

            location = discover_stream_location(working, runner=run)
            self.assertEqual(location.repo_root, repo.resolve())
            self.assertEqual(location.working_root, working.resolve())
            self.assertTrue(location.is_dedicated)

    def test_discover_location_without_vcs_keeps_the_current_directory(self) -> None:
        def run(args: list[str], cwd: Path) -> str:
            raise StreamError("not a repository")

        location = discover_stream_location(Path("/tmp/not-a-repo"), runner=run)
        self.assertEqual(location, StreamLocation(vcs=None, repo_root=None, working_root=Path("/tmp/not-a-repo").resolve()))
        self.assertFalse(location.is_dedicated)

    def test_discover_location_does_not_hide_git_worktree_failures(self) -> None:
        def run(args: list[str], cwd: Path) -> str:
            if args[0] == "jj":
                raise StreamError("not a jj repository")
            if args[1] == "rev-parse":
                return "/repo\n"
            raise StreamError("worktree metadata unavailable")

        with self.assertRaisesRegex(StreamError, "worktree metadata unavailable"):
            discover_stream_location(Path("/repo"), runner=run)

    def test_discover_location_preserves_root_lookup_failures_when_metadata_exists(self) -> None:
        for vcs, marker in [("git", ".git"), ("jj", ".jj")]:
            with self.subTest(vcs=vcs), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                (root / marker).mkdir()
                cwd = root / "subdir"
                cwd.mkdir()

                def run(args: list[str], cwd: Path) -> str:
                    if args[0] == vcs:
                        raise StreamError("repository metadata is inaccessible")
                    raise StreamError("not a repository")

                with self.assertRaisesRegex(StreamError, "repository metadata is inaccessible"):
                    discover_stream_location(cwd, runner=run)

    def test_discover_git_location_uses_first_worktree_as_repo_root(self) -> None:
        def run(args: list[str], cwd: Path) -> str:
            if args[0] == "jj":
                raise StreamError("not a jj repository")
            if args == ["git", "rev-parse", "--path-format=absolute", "--show-toplevel"]:
                return "/repo/worktrees/topic\n"
            if args == ["git", "worktree", "list", "--porcelain"]:
                return "worktree /repo\nHEAD abc\n\nworktree /repo/worktrees/topic\nHEAD def\n"
            raise AssertionError(args)

        location = discover_stream_location(Path("/repo/worktrees/topic/subdir"), runner=run)
        self.assertEqual(
            location,
            StreamLocation(vcs="git", repo_root=Path("/repo"), working_root=Path("/repo/worktrees/topic")),
        )


class StoreTest(unittest.TestCase):
    def start_main_stream(self, title: str, root: Path) -> Stream:
        location = StreamLocation(vcs="jj", repo_root=root, working_root=root)
        with patch("_streams.store.discover_stream_location", return_value=location):
            return start_stream(title, root=root, cwd=root)

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

    def test_start_stream_creates_active_stream_with_discovered_location(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, patch("_streams.store.discover_stream_location") as discover_mock:
            root = Path(tmp)
            cwd = root / "repo" / "workspaces" / "topic"
            cwd.mkdir(parents=True)
            discover_mock.return_value = StreamLocation(
                vcs="jj",
                repo_root=Path.home() / "repo",
                working_root=Path.home() / "repo" / "workspaces" / "topic",
            )
            stream = start_stream("New Stream", root=root, cwd=cwd / "subdir")
            self.assertEqual(stream.slug, "new-stream")
            self.assertEqual(stream.status, "active")
            self.assertEqual(stream.frontmatter["repo"], "~/repo")
            self.assertEqual(stream.frontmatter["cwd"], "~/repo/workspaces/topic")
            self.assertEqual(stream.frontmatter["vcs"], "jj")
            self.assertEqual(stream.frontmatter["sessions"], [])
            self.assertTrue((root / "new-stream.md").exists())

    def test_preview_start_has_same_identity_as_start_without_writing(self) -> None:
        from _streams.store import preview_start

        with tempfile.TemporaryDirectory() as tmp, patch("_streams.store.discover_stream_location") as discover_mock:
            root = Path(tmp)
            discover_mock.return_value = StreamLocation(vcs="jj", repo_root=Path.home() / "repo", working_root=Path.home() / "repo")
            preview = preview_start("New Stream", root=root, cwd=root)
            self.assertEqual(preview.slug, "new-stream")
            self.assertEqual(preview.frontmatter["cwd"], "~/repo")
            self.assertFalse(preview.path.exists())
            created = start_stream("New Stream", root=root, cwd=root)
            self.assertEqual(preview.as_json(), created.as_json())

    def test_start_without_repo_records_explicit_nulls_and_valid_cwd(self) -> None:
        from _streams.store import preview_start

        with tempfile.TemporaryDirectory() as tmp:
            cwd = Path(tmp).resolve()
            root = cwd / "scratch"
            preview = preview_start("Writing Notes", root=root, cwd=cwd)
            self.assertFalse(root.exists())
            created = start_stream("Writing Notes", root=root, cwd=cwd)
            self.assertEqual(created.as_json(), preview.as_json())
            self.assertIsNone(created.frontmatter["repo"])
            self.assertIsNone(created.frontmatter["vcs"])
            self.assertEqual(created.location.working_root, cwd)
            self.assertFalse(created.location.is_dedicated)
            self.assertEqual(check_path(created.path), [])
            self.assertEqual(list_streams(root)[0].as_json(), created.as_json())
            self.assertEqual(show_stream(created.slug, root).as_json(), created.as_json())

    def test_start_stream_writes_discovered_home_symlink_aliases(self) -> None:
        with (
            tempfile.TemporaryDirectory() as tmp,
            patch("_streams.store.discover_stream_location") as discover_location,
            patch("_streams.store.discover_home_path_aliases") as discover_aliases,
        ):
            root = Path(tmp)
            canonical_root = Path.home() / "go" / "src" / "github.com" / "example-org"
            discover_location.return_value = StreamLocation(
                vcs="jj",
                repo_root=canonical_root / "monorepo",
                working_root=canonical_root / "workspaces" / "topic",
            )
            discover_aliases.return_value = (
                PathAlias(display_root="~/work", canonical_root=canonical_root),
            )

            stream = start_stream("New Stream", root=root, cwd=root)

            self.assertEqual(stream.frontmatter["repo"], "~/work/monorepo")
            self.assertEqual(stream.frontmatter["cwd"], "~/work/workspaces/topic")

    def test_start_stream_refuses_overwrite(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.start_main_stream("New Stream", root)
            with self.assertRaisesRegex(Exception, "already exists"):
                self.start_main_stream("New Stream", root)

    def test_rename_stream_updates_identity_and_leaves_relative_symlinks(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = self.start_main_stream("Old Stream", root)
            stream.path.write_text(
                stream.path.read_text().replace(
                    "## Goal",
                    "[[scratch/artifacts/old-stream/notes.md]]\n\n## Goal",
                )
            )
            stream = show_stream("old-stream", root)
            old_path = stream.path
            old_artifacts = root / "artifacts" / "old-stream"
            old_artifacts.mkdir(parents=True)
            old_artifacts.joinpath("notes.md").write_text("notes")

            renamed = rename_stream(stream, "Focused Review")

            new_path = root / "focused-review.md"
            new_artifacts = root / "artifacts" / "focused-review"
            self.assertEqual(renamed.path, new_path)
            self.assertEqual(renamed.title, "Focused Review")
            self.assertEqual(renamed.frontmatter["updated"], date.today().isoformat())
            self.assertTrue(old_path.is_symlink())
            self.assertEqual(old_path.readlink(), Path("focused-review.md"))
            self.assertEqual(old_path.resolve(), new_path.resolve())
            self.assertTrue(old_artifacts.is_symlink())
            self.assertEqual(old_artifacts.readlink(), Path("focused-review"))
            self.assertEqual(old_artifacts.resolve(), new_artifacts.resolve())
            self.assertEqual(new_artifacts.joinpath("notes.md").read_text(), "notes")
            self.assertTrue(renamed.body.startswith("# Focused Review\n"))
            self.assertIn("scratch/artifacts/focused-review/notes.md", renamed.body)
            self.assertEqual([item.slug for item in list_streams(root)], ["focused-review"])
            self.assertEqual([path.name for path in check_paths(root)], ["focused-review.md"])

    def test_rename_stream_refuses_stream_destination_collision(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = self.start_main_stream("Old Stream", root)
            self.start_main_stream("Focused Review", root)

            with self.assertRaisesRegex(Exception, "already exists"):
                rename_stream(stream, "Focused Review")

            self.assertFalse(stream.path.is_symlink())
            self.assertEqual(show_stream("old-stream", root).title, "Old Stream")

    def test_rename_stream_refuses_artifact_destination_collision(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = self.start_main_stream("Old Stream", root)
            old_artifacts = root / "artifacts" / "old-stream"
            new_artifacts = root / "artifacts" / "focused-review"
            old_artifacts.mkdir(parents=True)
            new_artifacts.mkdir(parents=True)

            with self.assertRaisesRegex(Exception, "artifact destination already exists"):
                rename_stream(stream, "Focused Review")

            self.assertFalse(stream.path.is_symlink())
            self.assertTrue(old_artifacts.is_dir())
            self.assertFalse(old_artifacts.is_symlink())

    def test_rename_stream_rolls_back_when_artifact_symlink_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = self.start_main_stream("Old Stream", root)
            old_path = stream.path
            new_path = root / "focused-review.md"
            old_artifacts = root / "artifacts" / "old-stream"
            new_artifacts = root / "artifacts" / "focused-review"
            old_artifacts.mkdir(parents=True)
            old_artifacts.joinpath("notes.md").write_text("notes")
            symlink_to = Path.symlink_to

            def fail_artifact_symlink(path: Path, target: str, target_is_directory: bool = False) -> None:
                if path == old_artifacts:
                    raise OSError("simulated artifact symlink failure")
                symlink_to(path, target, target_is_directory)

            with patch.object(Path, "symlink_to", new=fail_artifact_symlink):
                with self.assertRaisesRegex(StreamError, "simulated artifact symlink failure"):
                    rename_stream(stream, "Focused Review")

            self.assertTrue(old_path.is_file())
            self.assertFalse(old_path.is_symlink())
            self.assertEqual(show_stream("old-stream", root).title, "Old Stream")
            self.assertFalse(new_path.exists())
            self.assertTrue(old_artifacts.is_dir())
            self.assertFalse(old_artifacts.is_symlink())
            self.assertEqual(old_artifacts.joinpath("notes.md").read_text(), "notes")
            self.assertFalse(new_artifacts.exists())

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

    def test_exact_title_resolves_when_title_and_slug_differ(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "legacy-name", title="Displayed Review")
            self.assertEqual(resolve_stream_ref("Displayed Review", root=root).slug, "legacy-name")

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
        with tempfile.TemporaryDirectory() as tmp, patch("_streams.cli.scan_streams") as scan_mock, patch("builtins.print") as print_mock:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream").stem, root)
            scan_mock.return_value = ([stream], [])
            args = type("Args", (), {"json": True})()
            self.assertEqual(command_list(args), 0)
            payload = json.loads(print_mock.call_args.args[0])
            self.assertEqual(payload["streams"][0]["slug"], "active-stream")
            self.assertEqual(payload["streams"][0]["path"], str(stream.path))
            self.assertEqual(payload["findings"], [])

    def test_show_human_includes_slug_and_link_descriptions(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, patch("_streams.cli.resolve_stream_ref") as show_mock, patch("builtins.print") as print_mock:
            root = Path(tmp)
            stream = show_stream(
                write_stream(
                    root,
                    "active-stream",
                    issues=[{"url": "https://github.com/example-org/widget-lib/issues/124", "description": "Literal conversion helpers"}],
                    prs=[{"url": "https://github.com/example-org/widget-lib/pull/150", "description": "Literal helper changes"}],
                ).stem,
                root,
            )
            show_mock.return_value = stream
            args = type("Args", (), {"slug": "active-stream", "json": False})()
            command_show(args)
            output = print_mock.call_args.args[0]
            self.assertIn("slug:        active-stream", output)
            self.assertIn("issues:      [I#124](https://github.com/example-org/widget-lib/issues/124): Literal conversion helpers", output)
            self.assertIn("prs:         [PR#150](https://github.com/example-org/widget-lib/pull/150): Literal helper changes", output)

    def test_start_dry_run_previews_slug_and_location_without_creating_scratch(self) -> None:
        from _streams.cli import command_start

        with tempfile.TemporaryDirectory() as tmp, patch("_streams.cli.preview_start") as preview_mock, patch("builtins.print") as print_mock:
            root = Path(tmp)
            preview = make_stream(root / "new-stream.md", "# New Stream\n", title="New Stream", state="Started")
            preview_mock.return_value = preview
            args = build_parser().parse_args(["start", "New Stream", "--dry-run", "--json"])
            self.assertEqual(command_start(args), 0)
            preview_mock.assert_called_once_with("New Stream")
            self.assertFalse(preview.path.exists())
            self.assertEqual(json.loads(print_mock.call_args.args[0])["slug"], "new-stream")

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

    def test_rename_current_resolves_ref_and_updates_current_workspace(self) -> None:
        with patch("_streams.cli.resolve_stream_ref") as resolve_mock, patch("_streams.cli.rename_stream") as rename_mock, patch("_streams.cli.update_current_workspace") as update_mock, patch("builtins.print") as print_mock:
            original = make_stream(Path("/scratch/old-stream.md"), title="Old Stream")
            renamed = make_stream(Path("/scratch/focused-review.md"), title="Focused Review")
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "Old Stream"})
            resolve_mock.return_value = original
            rename_mock.return_value = renamed
            update_mock.return_value = workspace
            args = type("Args", (), {"stream_ref": "Old Stream", "title": "Focused Review", "current": True})()

            self.assertEqual(command_rename(args), 0)

            resolve_mock.assert_called_once_with("Old Stream")
            rename_mock.assert_called_once_with(original, "Focused Review")
            update_mock.assert_called_once_with(renamed)
            self.assertEqual(print_mock.call_args_list[0].args[0], "renamed: old-stream -> focused-review /scratch/focused-review.md")
            self.assertEqual(print_mock.call_args_list[1].args[0], "updated: workspace:2 focused-review")

    def test_update_all_prints_updates_and_warnings(self) -> None:
        with patch("_streams.cli.CmuxClient") as cmux_class, patch("_streams.cli.update_all_workspaces") as update_mock, patch("builtins.print") as print_mock:
            matching = CmuxWorkspace({"ref": "workspace:2", "title": "Active Stream"})
            stream = make_stream(Path("/scratch/active-stream.md"), title="Active Stream")
            cmux = cmux_class.return_value
            update_mock.return_value = ([(matching, stream)], ["Warning: workspace:3 'Other' has no matching scratch stream"])
            args = type("Args", (), {"slug": None, "all": True, "current": False, "workspace": None})()
            self.assertEqual(command_cmux_update(args), 0)
            update_mock.assert_called_once_with(cmux)
            self.assertEqual(
                [call.args[0] for call in print_mock.call_args_list],
                ["updated: workspace:2 active-stream", "Warning: workspace:3 'Other' has no matching scratch stream"],
            )

    def test_update_all_stream_with_target_uses_regular_update(self) -> None:
        with patch("_streams.cli.CmuxClient") as cmux_class, patch("_streams.cli.load_stream_for_update") as load_mock, patch("_streams.cli.update_workspace") as update_mock, patch("builtins.print") as print_mock:
            cmux = cmux_class.return_value
            stream = make_stream(Path("/scratch/all.md"), title="all")
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "Other"})
            load_mock.return_value = (stream, workspace)
            args = type("Args", (), {"slug": "all", "all": False, "current": True, "workspace": None})()
            self.assertEqual(command_cmux_update(args), 0)
            load_mock.assert_called_once_with("all", None, True, cmux)
            update_mock.assert_called_once_with(stream, workspace, cmux)
            print_mock.assert_called_once_with("updated: workspace:2 all")

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

        all_args = parser.parse_args(["cmux", "update", "--all"])
        self.assertTrue(all_args.all)

        workspace_args = parser.parse_args(["cmux", "update", "active", "-w", "workspace:2"])
        self.assertEqual(workspace_args.workspace, "workspace:2")

        archive_args = parser.parse_args(["a", "done-stream", "--dry-run"])
        self.assertEqual(archive_args.command, "a")
        self.assertEqual(archive_args.slug, "done-stream")
        self.assertTrue(archive_args.dry_run)

        rename_args = parser.parse_args(["rename", "Old Stream", "Focused Review", "-c"])
        self.assertEqual(rename_args.stream_ref, "Old Stream")
        self.assertEqual(rename_args.title, "Focused Review")
        self.assertTrue(rename_args.current)

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

    def test_update_reads_stdin_and_reports_dry_run_without_writing(self) -> None:
        from _streams.cli import command_update

        with tempfile.TemporaryDirectory() as tmp, patch("_streams.cli.resolve_note_path") as resolve_mock, patch("sys.stdin", new_callable=io.StringIO) as input_mock, patch("builtins.print") as print_mock:
            root = Path(tmp)
            path = write_stream(root, "active-stream", title="Active Stream")
            original = path.read_text()
            input_mock.write(original.replace("Doing work", "Ready for review"))
            input_mock.seek(0)
            resolve_mock.return_value = path
            args = build_parser().parse_args(["update", "Active Stream", "-n", "--json"])
            self.assertEqual(command_update(args), 0)
            self.assertEqual(path.read_text(), original)
            payload = json.loads(print_mock.call_args.args[0])
            self.assertEqual(payload["updated"], False)
            would_write = dated(original.replace("Doing work", "Ready for review"))
            self.assertEqual(payload["sha256"], hashlib.sha256(would_write.encode()).hexdigest())

    def test_update_writes_validated_stdin_with_base_precondition(self) -> None:
        from _streams.cli import command_update

        with tempfile.TemporaryDirectory() as tmp, patch("_streams.cli.resolve_note_path") as resolve_mock, patch("sys.stdin", new_callable=io.StringIO) as input_mock:
            root = Path(tmp)
            path = write_stream(root, "active-stream", title="Active Stream")
            original = path.read_text()
            candidate = original.replace("Doing work", "Ready for review")
            digest = hashlib.sha256(original.encode()).hexdigest()
            input_mock.write(candidate)
            input_mock.seek(0)
            resolve_mock.return_value = path
            args = build_parser().parse_args(["update", "active-stream", "--if-match", digest])
            self.assertEqual(command_update(args), 0)
            self.assertEqual(path.read_text(), dated(candidate))

    def test_format_prints_markdown_spec(self) -> None:
        with patch("builtins.print") as print_mock:
            args = type("Args", (), {})()
            self.assertEqual(command_format(args), 0)
            output = print_mock.call_args.args[0]
            self.assertIn("# Streams Scratch Format", output)
            self.assertIn("https://github.com/<owner>/<repo>/pull/<number>", output)
            self.assertIn('streams rename <stream-ref> "New Title"', output)

    def test_resume_execs_resolved_stream(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, patch("_streams.cli.resolve_stream_for_resume") as resolve_mock, patch("_streams.cli.exec_resume") as exec_mock:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream").stem, root)
            resolve_mock.return_value = stream
            args = type("Args", (), {"slug": "active"})()
            self.assertEqual(command_resume(args), 0)
            resolve_mock.assert_called_once_with("active")
            exec_mock.assert_called_once_with(stream)


def run_cli(root: Path, *argv: str, stdin: str = "") -> tuple[int, bytes, str]:
    stdout = io.TextIOWrapper(io.BytesIO(), encoding="utf-8")
    stderr = io.StringIO()
    with patch("_streams.store.scratch_root", return_value=root), patch("sys.stdout", stdout), patch("sys.stderr", stderr), patch("sys.stdin", io.StringIO(stdin)):
        code = cli_main(list(argv))
    stdout.flush()
    return code, stdout.buffer.getvalue(), stderr.getvalue()


def write_invalid_status(root: Path, slug: str) -> Path:
    path = write_stream(root, slug, title=slug.replace("-", " ").title())
    path.write_text(path.read_text().replace("status: active", "status: nope"))
    return path


class StreamModelTest(unittest.TestCase):
    def test_parse_exposes_typed_fields(self) -> None:
        issue = {"url": "https://github.com/o/r/issues/1", "description": "Fix the thing"}
        stream = make_stream(Path("/scratch/example.md"), issues=[issue], sessions=["pi:abc"], cwd="~/work")
        self.assertEqual(stream.issues, (Link(issue["url"], "Fix the thing"),))
        self.assertEqual((stream.prs, stream.sessions), ((), ("pi:abc",)))
        self.assertEqual(stream.location.working_root, Path("~/work").expanduser())

    def test_parse_rejects_invalid_frontmatter(self) -> None:
        with self.assertRaises(StreamError) as caught:
            make_stream(Path("/scratch/example.md"), status="nope")
        self.assertEqual(caught.exception.kind, "invalid-stream")


class CliContractTest(unittest.TestCase):
    """End-to-end CLI behaviour that other tools, like the Pi extension, rely on."""

    def test_list_reports_invalid_notes_without_hiding_valid_ones(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "good-stream", title="Good Stream")
            write_invalid_status(root, "broken-stream")
            code, stdout, _ = run_cli(root, "list", "--json")
            payload = json.loads(stdout)
            self.assertEqual(code, 1)
            self.assertEqual([stream["slug"] for stream in payload["streams"]], ["good-stream"])
            self.assertEqual({(finding["level"], finding["slug"]) for finding in payload["findings"]}, {("error", "broken-stream")})

            code, stdout, stderr = run_cli(root, "list")
            self.assertEqual(code, 1)
            self.assertIn(b"good-stream", stdout)
            self.assertIn("error: broken-stream: invalid status 'nope'", stderr)

    def test_list_exits_zero_when_every_note_is_valid(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "good-stream", title="Good Stream")
            code, stdout, _ = run_cli(root, "list", "--json")
            self.assertEqual(code, 0)
            self.assertEqual(json.loads(stdout)["findings"], [])

    def test_other_commands_skip_unrelated_invalid_notes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "good-stream", title="Good Stream")
            write_invalid_status(root, "broken-stream")
            code, stdout, _ = run_cli(root, "show", "good", "--json")
            self.assertEqual(code, 0)
            self.assertEqual(json.loads(stdout)["slug"], "good-stream")

    def test_json_errors_name_their_kind(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "auth-tokens", title="Auth Tokens")
            write_stream(root, "oauth-cleanup", title="Oauth Cleanup")
            write_invalid_status(root, "broken-stream")
            cases = {
                "!!!": "invalid-ref",
                "missing": "not-found",
                "auth": "ambiguous",
                "broken-stream": "invalid-stream",
                "broken": "not-found",  # only the exact slug reaches an invalid note
            }
            for ref, kind in cases.items():
                with self.subTest(ref=ref):
                    code, stdout, stderr = run_cli(root, "show", ref, "--json")
                    self.assertEqual(code, 2)
                    self.assertEqual(stderr, "")
                    self.assertEqual(json.loads(stdout)["error"]["kind"], kind)
            _, stdout, _ = run_cli(root, "show", "auth", "--json")
            self.assertEqual(json.loads(stdout)["error"]["candidates"], ["auth-tokens", "oauth-cleanup"])

    def test_plain_errors_stay_one_line_on_stderr(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            code, stdout, stderr = run_cli(Path(tmp), "show", "!!!")
            self.assertEqual((code, stdout), (2, b""))
            self.assertEqual(stderr, "streams: invalid slug ''; use lowercase kebab-case\n")

    def test_read_prints_exact_bytes_and_hash(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            raw = write_stream(root, "good-stream", title="Good Stream").read_bytes()
            self.assertEqual(run_cli(root, "read", "Good Stream"), (0, raw, ""))
            code, stdout, _ = run_cli(root, "read", "good-stream", "--hash")
            digest, _, rest = stdout.partition(b"\n")
            self.assertEqual((code, digest.decode(), rest), (0, hashlib.sha256(raw).hexdigest(), raw))

    def test_read_json_splits_frontmatter_and_contents(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            raw = write_stream(root, "good-stream", title="Good Stream").read_bytes()
            code, stdout, _ = run_cli(root, "read", "good-stream", "--json")
            payload = json.loads(stdout)
            self.assertEqual(code, 0)
            self.assertEqual((payload["slug"], payload["frontmatter"]["title"], payload["contents"]), ("good-stream", "Good Stream", "# Body\n"))
            self.assertEqual((payload["sha256"], payload["findings"]), (hashlib.sha256(raw).hexdigest(), []))

    def test_read_shows_invalid_notes_so_they_can_be_fixed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            raw = write_invalid_status(root, "broken-stream").read_bytes()
            code, stdout, stderr = run_cli(root, "read", "broken-stream")
            self.assertEqual((code, stdout), (1, raw))
            self.assertIn("error: broken-stream: invalid status 'nope'", stderr)
            payload = json.loads(run_cli(root, "read", "broken-stream", "--json")[1])
            self.assertEqual((payload["frontmatter"]["status"], payload["contents"]), ("nope", "# Body\n"))
            self.assertEqual(payload["findings"][0]["level"], "error")

    def test_read_json_keeps_unparseable_yaml_in_contents(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            text = "---\ntitle: [unclosed\n---\n# Body\n"
            (root / "bad-yaml.md").write_text(text)
            code, stdout, _ = run_cli(root, "read", "bad-yaml", "--json")
            payload = json.loads(stdout)
            self.assertEqual((code, payload["frontmatter"], payload["contents"]), (1, None, text))
            self.assertIn("could not parse frontmatter", payload["findings"][0]["message"])

    def test_update_replaces_an_invalid_note_only_with_its_hash(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_invalid_status(root, "broken-stream")
            broken = path.read_text()
            fixed = broken.replace("status: nope", "status: active")
            code, stdout, _ = run_cli(root, "update", "broken-stream", "--json", stdin=fixed)
            self.assertEqual((code, json.loads(stdout)["error"]["kind"]), (2, "invalid-stream"))
            self.assertEqual(path.read_text(), broken)

            digest = run_cli(root, "read", "broken-stream", "--hash")[1].partition(b"\n")[0].decode()
            code, _, _ = run_cli(root, "update", "broken-stream", "--if-match", digest, stdin=fixed)
            self.assertEqual((code, path.read_text()), (0, dated(fixed)))


class UpdateTest(unittest.TestCase):
    def test_invalid_candidate_never_writes_even_without_dry_run(self) -> None:
        from _streams.store import update_stream

        with tempfile.TemporaryDirectory() as tmp:
            path = write_stream(Path(tmp), "active-stream", title="Active Stream")
            original = path.read_text()
            with self.assertRaisesRegex(StreamError, "invalid status"):
                update_stream(path, original.replace("status: active", "status: nope"))
            self.assertEqual(path.read_text(), original)

    def test_dry_run_validates_without_writing(self) -> None:
        from _streams.store import update_stream

        with tempfile.TemporaryDirectory() as tmp:
            path = write_stream(Path(tmp), "active-stream", title="Active Stream")
            original = path.read_text()
            candidate = original.replace("Doing work", "Ready for review")
            self.assertEqual(update_stream(path, candidate, dry_run=True), ([], dated(candidate)))
            self.assertEqual(path.read_text(), original)

    def test_stale_digest_rejects_valid_candidate(self) -> None:
        from _streams.store import update_stream

        with tempfile.TemporaryDirectory() as tmp:
            path = write_stream(Path(tmp), "active-stream", title="Active Stream")
            original = path.read_text()
            candidate = original.replace("Doing work", "Ready for review")
            path.write_text(original.replace("Doing work", "Changed elsewhere"))
            with self.assertRaisesRegex(StreamError, "changed since"):
                update_stream(path, candidate, if_match=hashlib.sha256(original.encode()).hexdigest())
            self.assertIn("Changed elsewhere", path.read_text())

    def test_update_dates_the_candidate_and_keeps_its_other_bytes_and_file_permissions(self) -> None:
        from _streams.store import update_stream

        with tempfile.TemporaryDirectory() as tmp:
            path = write_stream(Path(tmp), "active-stream", title="Active Stream")
            path.chmod(0o600)
            original = path.read_text().replace("title: Active Stream\n", "title: Active Stream\nupdated: '2020-01-01'  # old\n") + "updated: in the body\n"
            path.write_text(original)
            candidate = original.replace("Doing work", "Ready for review")
            written = candidate.replace("updated: '2020-01-01'  # old", f"updated: '{date.today().isoformat()}'")
            self.assertEqual(update_stream(path, candidate), ([], written))
            self.assertEqual(path.read_text(), written)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)

    def test_update_adds_a_missing_updated_date(self) -> None:
        from _streams.store import update_stream

        with tempfile.TemporaryDirectory() as tmp:
            path = write_stream(Path(tmp), "active-stream", title="Active Stream")
            candidate = path.read_text().replace("Doing work", "Ready for review")
            update_stream(path, candidate)
            self.assertEqual(path.read_text(), dated(candidate))

    def test_unchanged_candidate_keeps_its_date(self) -> None:
        from _streams.store import update_stream

        with tempfile.TemporaryDirectory() as tmp:
            path = write_stream(Path(tmp), "active-stream", title="Active Stream")
            original = path.read_text()
            self.assertEqual(update_stream(path, original), ([], original))
            self.assertEqual(path.read_text(), original)


class CheckTest(unittest.TestCase):
    def test_valid_stream_has_no_findings(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "active-stream", title="Active Stream")
            self.assertEqual(check_path(path), [])

    def test_repo_and_vcs_nulls_are_valid_only_together(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "active-stream", title="Active Stream", repo=None, vcs=None)
            self.assertEqual(check_path(path), [])
            for repo, vcs in [(None, "git"), ("/repo", None)]:
                with self.subTest(repo=repo, vcs=vcs):
                    write_stream(root, "active-stream", title="Active Stream", repo=repo, vcs=vcs)
                    self.assertTrue(any(f.level == "error" and "repo and vcs must both be null" in f.message for f in check_path(path)))

    def test_repo_less_notes_still_require_explicit_fields_and_a_local_cwd(self) -> None:
        from _streams.check import check_content

        with tempfile.TemporaryDirectory() as tmp:
            path = write_stream(Path(tmp), "active-stream", title="Active Stream", repo=None, vcs=None)
            content = path.read_text()
            for field in ("repo", "vcs"):
                with self.subTest(field=field):
                    findings = check_content(path, content.replace(f"{field}: null\n", ""))
                    self.assertTrue(any(f.level == "error" and f"missing required field '{field}'" in f.message for f in findings))
            for cwd in (None, "", "relative/path"):
                with self.subTest(cwd=cwd):
                    frontmatter, body = split_frontmatter(content)
                    frontmatter["cwd"] = cwd
                    findings = check_content(path, render_frontmatter(frontmatter, body))
                    self.assertTrue(any(f.level == "error" and "cwd" in f.message for f in findings))

    def test_candidate_uses_same_findings_as_saved_content(self) -> None:
        from _streams.check import check_content

        with tempfile.TemporaryDirectory() as tmp:
            path = write_stream(Path(tmp), "active-stream", title="Active Stream")
            content = path.read_text().replace("status: active", "status: nope")
            self.assertEqual(check_path(path), [])
            path.write_text(content)
            self.assertEqual(check_content(path, content), check_path(path))
            self.assertTrue(any(finding.level == "error" for finding in check_content(path, content)))

    def test_check_paths_include_legacy_files_without_frontmatter(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "active-stream")
            (root / "legacy.md").write_text("# Legacy scratch\n")
            self.assertEqual([path.name for path in check_paths(root)], ["active-stream.md", "legacy.md"])

    def test_repo_must_be_a_local_root_directory_path(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "active-stream", repo="example-org/monorepo")
            findings = check_path(path)
            self.assertTrue(any("repo must be an absolute or home-relative local path" in finding.message for finding in findings))

    def test_cwd_must_be_a_local_root_directory_path(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "active-stream", cwd="multi-repo: worknotes, monorepo")
            findings = check_path(path)
            self.assertTrue(any("cwd must be an absolute or home-relative local path" in finding.message for finding in findings))

    def test_vcs_must_identify_jj_or_git(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "active-stream", vcs="svn")
            findings = check_path(path)
            self.assertTrue(any("invalid vcs" in finding.message for finding in findings))

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
            path = write_stream(root, "active-stream", issues=[{"url": "https://github.com/example-org/widget-lib/pull/150", "description": "Invalid issue URL"}])
            findings = check_path(path)
            self.assertTrue(any("issues[0].url must be a GitHub issue URL" in finding.message for finding in findings))

    def test_pr_must_be_github_pull_url(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "active-stream", prs=[{"url": "widget-lib#150", "description": "Invalid pull request"}])
            findings = check_path(path)
            self.assertTrue(any("prs[0].url must be a GitHub pull request URL" in finding.message for finding in findings))

    def test_github_issue_and_pr_urls_are_valid(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(
                root,
                "active-stream",
                title="Active Stream",
                issues=[{"url": "https://github.com/example-org/widget-lib/issues/124", "description": "Literal conversion helpers"}],
                prs=[{"url": "https://github.com/example-org/widget-lib/pull/150", "description": "Literal helper changes"}],
            )
            self.assertEqual(check_path(path), [])

    def test_link_description_must_have_two_to_five_words(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "active-stream", issues=[{"url": "https://github.com/example-org/widget-lib/issues/124", "description": "Literals"}])
            findings = check_path(path)
            self.assertTrue(any("issues[0].description must contain 2 to 5 words" in finding.message for finding in findings))

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
    def test_successful_cmux_stderr_is_forwarded_without_polluting_stdout(self) -> None:
        completed = subprocess.CompletedProcess(["cmux", "rpc"], 0, '{"ok":true}', "CMUX warning\n")
        output = io.StringIO()
        with patch("_streams.cmux.subprocess.run", return_value=completed), patch("sys.stderr", output):
            result = CmuxClient().run(["cmux", "rpc"])
        self.assertEqual(result.stdout, '{"ok":true}')
        self.assertEqual(output.getvalue(), "CMUX warning\n")

    def test_color_for_status(self) -> None:
        self.assertEqual(color_for_status("active"), "Green")
        self.assertEqual(color_for_status("waiting"), "Amber")
        self.assertEqual(color_for_status("blocked"), "Orange")
        self.assertEqual(color_for_status("unknown"), "Amber")

    def test_resume_command_quotes_session_ids(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream", sessions=["claude:session with spaces"]).stem, root)
            self.assertEqual(latest_session_command(stream).command_text, "claude --resume 'session with spaces'")

    def test_resume_command_uses_last_session(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(
                write_stream(root, "active-stream", sessions=["codex:old", "claude:new session"]).stem,
                root,
            )
            self.assertEqual(latest_session_command(stream).command_text, "claude --resume 'new session'")

    def test_open_workspace_uses_stream_fields(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            vault = Path(tmp)
            root = vault / "scratch"
            root.mkdir()
            cwd = root / "repo"
            cwd.mkdir()
            stream = show_stream(write_stream(root, "active-stream", title="Active Stream", state="Ready to work", cwd=str(cwd)).stem, root)
            cmux = FakeCmux()
            with patch("_streams.cmux.notes_root", return_value=vault):
                open_workspace(stream, cmux)
            self.assertEqual(
                cmux.commands[0],
                [
                    "cmux",
                    "new-workspace",
                    "--name",
                    "Active Stream",
                    "--description",
                    "[🗒︎](obsidian://open?vault=streams&file=scratch%2Factive-stream.md) _Ready to work_",
                    "--cwd",
                    str(cwd),
                    "--command",
                    "codex resume abc",
                ],
            )

    def test_open_repo_less_workspace_uses_the_recorded_directory(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = write_stream(root, "writing-notes", title="Writing Notes", repo=None, vcs=None, cwd=str(root))
            cmux = FakeCmux()
            with patch("_streams.cmux.notes_root", return_value=root):
                open_workspace(show_stream(path.stem, root), cmux)
            command = cmux.commands[0]
            self.assertEqual(command[command.index("--cwd") + 1], str(root))

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
            with patch("_streams.cmux.notes_root", return_value=vault):
                update_workspace(stream, workspace, cmux)
            self.assertEqual(cmux.commands[0], ["cmux", "workspace-action", "--workspace", "workspace:2", "--action", "rename", "--title", "Active Stream"])
            self.assertEqual(cmux.commands[1], ["cmux", "workspace-action", "--workspace", "workspace:2", "--action", "set-description", "--description", "[🗒︎](obsidian://open?vault=streams&file=scratch%2Factive-stream.md) _Ready to work_"])
            self.assertEqual(cmux.commands[2], ["cmux", "workspace-action", "--workspace", "workspace:2", "--action", "set-color", "--color", "Green"])

    def test_update_all_workspaces_skips_tasks_and_warns_for_unmatched_workspaces(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            vault = Path(tmp)
            root = vault / "scratch"
            root.mkdir()
            stream = show_stream(write_stream(root, "active-stream", title="Active Stream", state="Ready to work").stem, root)
            write_stream(root, "code-review", title="Code Review", state="Ready to work")
            tasks = CmuxWorkspace({"ref": "workspace:1", "index": 0, "title": "Tasks"})
            matching = CmuxWorkspace({"ref": "workspace:2", "index": 1, "title": "Active Stream"})
            unmatched = CmuxWorkspace({"ref": "workspace:3", "index": 2, "title": "Other"})
            partial_title = CmuxWorkspace({"ref": "workspace:4", "index": 3, "title": "review"})
            cmux = FakeCmux([tasks, matching, unmatched, partial_title])
            with patch("_streams.cmux.notes_root", return_value=vault):
                updated, warnings = update_all_workspaces(cmux, root=root)
            self.assertEqual(updated, [(matching, stream)])
            self.assertEqual(
                warnings,
                [
                    "Warning: workspace:3 'Other' has no matching scratch stream",
                    "Warning: workspace:4 'review' has no matching scratch stream",
                ],
            )
            self.assertEqual(cmux.commands[0], ["cmux", "workspace-action", "--workspace", "workspace:2", "--action", "rename", "--title", "Active Stream"])

    def test_update_current_workspace_resolves_and_updates_current(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            vault = Path(tmp)
            root = vault / "scratch"
            root.mkdir()
            stream = show_stream(write_stream(root, "active-stream", title="Active Stream", state="Ready to work").stem, root)
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "Old"})
            cmux = FakeCmux([workspace])
            with patch("_streams.cmux.notes_root", return_value=vault):
                resolved = update_current_workspace(stream, cmux)
            self.assertEqual(resolved.display_ref, "workspace:2")
            self.assertEqual(cmux.commands[0], ["cmux", "workspace-action", "--workspace", "workspace:2", "--action", "rename", "--title", "Active Stream"])

    def test_description_rejects_stream_outside_notes_root(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = show_stream(write_stream(root, "active-stream").stem, root)
            with patch("_streams.cmux.notes_root", return_value=root / "worknotes"):
                with self.assertRaisesRegex(StreamError, "outside notes root"):
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
                        {"url": "https://github.com/example-org/widget-lib/issues/124", "description": "Literal conversion helpers"},
                        {"url": "https://github.com/openai/codex/issues/7", "description": "Codex issue"},
                    ],
                    prs=[
                        {"url": "https://github.com/example-org/widget-lib/pull/150", "description": "Literal helper changes"},
                        {"url": "https://github.com/openai/codex/pull/42", "description": "Codex pull request"},
                    ],
                ).stem,
                root,
            )
            with patch("_streams.cmux.notes_root", return_value=vault):
                description = description_for_stream(stream)
            self.assertEqual(
                description,
                "\n".join(
                    [
                        "[🗒︎](obsidian://open?vault=streams&file=scratch%2Factive-stream.md) _Issue #124 literal conversion helpers_",
                        "[ℹ︎#124](https://github.com/example-org/widget-lib/issues/124): Literal conversion helpers",
                        "[ℹ︎#7](https://github.com/openai/codex/issues/7): Codex issue",
                        "[⑂︎#150](https://github.com/example-org/widget-lib/pull/150): Literal helper changes",
                        "[⑂︎#42](https://github.com/openai/codex/pull/42): Codex pull request",
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

    def test_update_current_falls_back_to_unique_dedicated_cwd(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "active-stream", title="Active Stream", repo="/repo", cwd="/repo/workspaces/active")
            write_stream(root, "other-stream", title="Other Stream", repo="/repo", cwd="/repo/workspaces/other")
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "streams cmux update --current", "current_directory": "/repo/workspaces/active"})
            cmux = FakeCmux([workspace])
            stream, resolved = load_stream_for_update(None, None, True, cmux, root=root)
            self.assertEqual(stream.slug, "active-stream")
            self.assertEqual(resolved.display_ref, "workspace:2")

    def test_update_current_does_not_infer_unique_stream_in_main_repo(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "old-stream", title="Old Stream", repo="/repo", cwd="/repo")
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "π - repo", "current_directory": "/repo"})
            cmux = FakeCmux([workspace])
            with self.assertRaisesRegex(Exception, "main checkout"):
                load_stream_for_update(None, None, True, cmux, root=root)

    def test_update_current_does_not_infer_repo_less_stream_from_cwd(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "writing-notes", title="Writing Notes", repo=None, vcs=None, cwd=str(root))
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "Unrelated", "current_directory": str(root)})
            with self.assertRaisesRegex(StreamError, "pass a slug"):
                load_stream_for_update(None, None, True, FakeCmux([workspace]), root=root)

    def test_update_current_does_not_fuzzy_match_workspace_title(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stream(root, "code-review", title="Code Review", repo="/repo", cwd="/repo")
            workspace = CmuxWorkspace({"ref": "workspace:2", "title": "review", "current_directory": "/missing"})
            cmux = FakeCmux([workspace])
            with self.assertRaisesRegex(Exception, "pass a slug"):
                load_stream_for_update(None, None, True, cmux, root=root)

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

    def test_repo_less_stream_can_resume_in_its_cwd(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, patch("_streams.resume.os.chdir") as chdir_mock, patch("_streams.resume.os.execvp") as exec_mock:
            root = Path(tmp)
            write_stream(root, "writing-notes", title="Writing Notes", repo=None, vcs=None, cwd=str(root), sessions=["pi:last"])
            stream = resolve_stream_for_resume("writing-notes", root=root)
            exec_resume(stream)
            chdir_mock.assert_called_once_with(root)
            exec_mock.assert_called_once_with("pi", ["pi", "--session", "last"])

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
