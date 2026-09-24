"""Command-line interface for scratch streams."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from .errors import StreamError
from .models import Stream
from .store import archive_stream, list_streams, preview_start, rename_stream, start_stream, update_stream
from .cmux import CmuxClient, load_stream_for_update, markdown_link_for_item, open_workspace, update_all_workspaces, update_current_workspace, update_workspace
from .check import check_streams, findings_json
from .store import check_paths
from .resolve import resolve_check_path_ref, resolve_stream_ref
from .resume import exec_resume, resolve_stream_for_resume


def render_list(streams: list[Stream]) -> str:
    rows = [(stream.title, stream.status, stream.state, stream.slug) for stream in streams]
    headers = ("Title", "Status", "State", "Slug")
    widths = [
        max(len(headers[index]), *(len(row[index]) for row in rows)) if rows else len(headers[index])
        for index in range(len(headers))
    ]
    lines = [
        "  ".join(headers[index].ljust(widths[index]) for index in range(len(headers))),
        "  ".join("-" * width for width in widths),
    ]
    lines.extend("  ".join(row[index].ljust(widths[index]) for index in range(len(row))) for row in rows)
    return "\n".join(lines)


def render_show(stream: Stream) -> str:
    issues = ", ".join(markdown_link_for_item(item, "I") for item in stream.frontmatter.get("issues") or []) or "-"
    prs = ", ".join(markdown_link_for_item(item, "PR") for item in stream.frontmatter.get("prs") or []) or "-"
    sessions = ", ".join(str(item) for item in stream.frontmatter.get("sessions") or []) or "-"
    return "\n".join(
        [
            f"title:       {stream.title}",
            f"slug:        {stream.slug}",
            f"status:      {stream.status}",
            f"state:       {stream.state}",
            f"description: {stream.frontmatter.get('description') or ''}",
            f"repo:        {stream.frontmatter.get('repo') or ''}",
            f"cwd:         {stream.frontmatter.get('cwd') or ''}",
            f"vcs:         {stream.frontmatter.get('vcs') or ''}",
            f"issues:      {issues}",
            f"prs:         {prs}",
            f"sessions:    {sessions}",
            f"path:        {stream.path}",
        ]
    )


def command_list(args: argparse.Namespace) -> int:
    streams = list_streams()
    if args.json:
        print(json.dumps([stream.as_json() for stream in streams], indent=2, sort_keys=True))
    else:
        print(render_list(streams))
    return 0


def command_show(args: argparse.Namespace) -> int:
    stream = resolve_stream_ref(args.slug)
    if args.json:
        print(json.dumps(stream.as_json(), indent=2, sort_keys=True))
    else:
        print(render_show(stream))
    return 0


def command_start(args: argparse.Namespace) -> int:
    if getattr(args, "dry_run", False) and args.current:
        raise StreamError("start --dry-run cannot update the current CMUX workspace")
    stream = preview_start(args.title) if getattr(args, "dry_run", False) else start_stream(args.title)
    workspace = None
    if args.current:
        workspace = update_current_workspace(stream)
    if args.json:
        payload = stream.as_json()
        if workspace is not None:
            payload["updated_workspace"] = workspace.display_ref
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        print(f"{'would create' if getattr(args, 'dry_run', False) else 'created'}: {stream.slug} {stream.path}")
        if workspace is not None:
            print(f"updated: {workspace.display_ref} {stream.slug}")
    return 0


def command_rename(args: argparse.Namespace) -> int:
    stream = resolve_stream_ref(args.stream_ref)
    old_slug = stream.slug
    renamed = rename_stream(stream, args.title)
    workspace = update_current_workspace(renamed) if args.current else None
    print(f"renamed: {old_slug} -> {renamed.slug} {renamed.path}")
    if workspace is not None:
        print(f"updated: {workspace.display_ref} {renamed.slug}")
    return 0


def command_cmux_open(args: argparse.Namespace) -> int:
    stream = resolve_stream_ref(args.slug)
    open_workspace(stream)
    print(f"opened: {stream.slug}")
    return 0


def command_cmux_update(args: argparse.Namespace) -> int:
    cmux = CmuxClient()
    if args.all:
        if args.slug or args.current or args.workspace:
            raise StreamError("update --all does not accept a slug, --current, or --workspace")
        updated, warnings = update_all_workspaces(cmux)
        for workspace, stream in updated:
            print(f"updated: {workspace.display_ref} {stream.slug}")
        for warning in warnings:
            print(warning)
        return 0
    stream, workspace = load_stream_for_update(args.slug, args.workspace, args.current, cmux)
    update_workspace(stream, workspace, cmux)
    print(f"updated: {workspace.display_ref} {stream.slug}")
    return 0


def command_check(args: argparse.Namespace) -> int:
    if args.slug:
        paths = [resolve_check_path_ref(args.slug)]
    else:
        paths = check_paths()
    findings = check_streams(paths)
    if args.json:
        print(findings_json(findings))
    elif findings:
        for finding in findings:
            print(f"{finding.level}: {finding.slug}: {finding.message}")
    else:
        target = args.slug or "all streams"
        print(f"ok: {target}")
    return 1 if any(finding.level == "error" for finding in findings) else 0


def command_update(args: argparse.Namespace) -> int:
    stream = resolve_stream_ref(args.slug)
    candidate = sys.stdin.read()
    findings = update_stream(stream, candidate, dry_run=args.dry_run, if_match=args.if_match)
    payload = {"slug": stream.slug, "path": str(stream.path), "updated": not args.dry_run, "findings": [finding.as_json() for finding in findings]}
    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        print(f"{'validated' if args.dry_run else 'updated'}: {stream.slug} {stream.path}")
        for finding in findings:
            print(f"{finding.level}: {finding.slug}: {finding.message}")
    return 0


def command_format(args: argparse.Namespace) -> int:
    print((Path(__file__).with_name("FORMAT.md")).read_text().rstrip())
    return 0


def command_resume(args: argparse.Namespace) -> int:
    stream = resolve_stream_for_resume(args.slug)
    exec_resume(stream)
    return 0


def command_archive(args: argparse.Namespace) -> int:
    stream = resolve_stream_ref(args.slug)
    destination = archive_stream(stream, month=args.month, force=args.force, dry_run=args.dry_run)
    verb = "would archive" if args.dry_run else "archived"
    print(f"{verb}: {stream.slug} {destination}")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Manage scratch-first work streams.")
    subparsers = parser.add_subparsers(dest="command", required=True)

    list_parser = subparsers.add_parser("list", help="list current streams")
    list_parser.add_argument("--json", action="store_true", help="print stream frontmatter as JSON")
    list_parser.set_defaults(func=command_list)

    show_parser = subparsers.add_parser("show", help="show one stream")
    show_parser.add_argument("slug", help="stream filename stem")
    show_parser.add_argument("--json", action="store_true", help="print stream frontmatter as JSON")
    show_parser.set_defaults(func=command_show)

    check_parser = subparsers.add_parser("check", help="check stream frontmatter")
    check_parser.add_argument("slug", nargs="?", help="stream filename stem")
    check_parser.add_argument("--json", action="store_true", help="print findings as JSON")
    check_parser.set_defaults(func=command_check)

    update_parser = subparsers.add_parser("update", help="validate and replace a stream from markdown on stdin")
    update_parser.add_argument("slug", help="existing stream title or slug")
    update_parser.add_argument("-n", "--dry-run", action="store_true", help="validate without writing")
    update_parser.add_argument("--if-match", help="require the current file's SHA-256 to match this digest")
    update_parser.add_argument("--json", action="store_true", help="print update result as JSON")
    update_parser.set_defaults(func=command_update)

    start_parser = subparsers.add_parser("start", aliases=["s"], help="create a new active stream")
    start_parser.add_argument("title", help="stream title")
    start_parser.add_argument("--json", action="store_true", help="print stream frontmatter as JSON")
    start_parser.add_argument("-n", "--dry-run", action="store_true", help="preview the starter note without creating it")
    start_parser.add_argument("-c", "--current", action="store_true", help="update the current CMUX workspace from the new stream")
    start_parser.set_defaults(func=command_start)

    rename_parser = subparsers.add_parser("rename", help="rename a stream")
    rename_parser.add_argument("stream_ref", help="current stream title or slug")
    rename_parser.add_argument("title", help="new stream title")
    rename_parser.add_argument("-c", "--current", action="store_true", help="update the current CMUX workspace from the renamed stream")
    rename_parser.set_defaults(func=command_rename)

    format_parser = subparsers.add_parser("format", help="print scratch stream format documentation")
    format_parser.set_defaults(func=command_format)

    resume_parser = subparsers.add_parser("resume", help="resume a stream's latest recorded AI session")
    resume_parser.add_argument("slug", nargs="?", help="stream filename stem; defaults from current CMUX workspace")
    resume_parser.set_defaults(func=command_resume)

    archive_parser = subparsers.add_parser("archive", aliases=["a"], help="move a done stream to scratch/archive")
    archive_parser.add_argument("slug", help="stream filename stem")
    archive_parser.add_argument("--month", help="archive month in YYYY-MM form; defaults to the current month")
    archive_parser.add_argument("--dry-run", action="store_true", help="show destination without moving the file")
    archive_parser.add_argument("--force", action="store_true", help="archive a stream whose status is not done")
    archive_parser.set_defaults(func=command_archive)

    cmux_parser = subparsers.add_parser("cmux", aliases=["c"], help="project streams into CMUX")
    cmux_subparsers = cmux_parser.add_subparsers(dest="cmux_command", required=True)

    cmux_open = cmux_subparsers.add_parser("open", aliases=["o"], help="open a CMUX workspace for a stream")
    cmux_open.add_argument("slug", help="stream filename stem")
    cmux_open.set_defaults(func=command_cmux_open)

    cmux_update = cmux_subparsers.add_parser("update", aliases=["u"], help="update a CMUX workspace from a stream")
    cmux_update.add_argument("slug", nargs="?", help="stream filename stem; defaults from current workspace title with --current")
    cmux_update.add_argument("-a", "--all", action="store_true", help="update all matching CMUX workspaces")
    cmux_update.add_argument("-c", "--current", action="store_true", help="target the current CMUX workspace")
    cmux_update.add_argument("-w", "--workspace", help="target a CMUX workspace ref, UUID, or title")
    cmux_update.set_defaults(func=command_cmux_update)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        return args.func(args)
    except StreamError as exc:
        print(f"streams: {exc}", file=sys.stderr)
        return 2
