"""Command-line interface for scratch streams."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from .errors import StreamError
from .models import Stream
from .store import list_streams, start_stream
from .cmux import CmuxClient, load_stream_for_update, open_workspace, update_workspace
from .check import check_streams, findings_json
from .store import check_paths
from .resolve import resolve_check_path_ref, resolve_stream_ref


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
    issues = ", ".join(str(item) for item in stream.frontmatter.get("issues") or []) or "-"
    prs = ", ".join(str(item) for item in stream.frontmatter.get("prs") or []) or "-"
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
    stream = start_stream(args.title)
    if args.json:
        print(json.dumps(stream.as_json(), indent=2, sort_keys=True))
    else:
        print(f"created: {stream.slug} {stream.path}")
    return 0


def command_cmux_open(args: argparse.Namespace) -> int:
    stream = resolve_stream_ref(args.slug)
    open_workspace(stream)
    print(f"opened: {stream.slug}")
    return 0


def command_cmux_update(args: argparse.Namespace) -> int:
    cmux = CmuxClient()
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


def command_format(args: argparse.Namespace) -> int:
    print((Path(__file__).with_name("FORMAT.md")).read_text().rstrip())
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

    start_parser = subparsers.add_parser("start", help="create a new active stream")
    start_parser.add_argument("title", help="stream title")
    start_parser.add_argument("--json", action="store_true", help="print stream frontmatter as JSON")
    start_parser.set_defaults(func=command_start)

    format_parser = subparsers.add_parser("format", help="print scratch stream format documentation")
    format_parser.set_defaults(func=command_format)

    cmux_parser = subparsers.add_parser("cmux", help="project streams into CMUX")
    cmux_subparsers = cmux_parser.add_subparsers(dest="cmux_command", required=True)

    cmux_open = cmux_subparsers.add_parser("open", help="open a CMUX workspace for a stream")
    cmux_open.add_argument("slug", help="stream filename stem")
    cmux_open.set_defaults(func=command_cmux_open)

    cmux_update = cmux_subparsers.add_parser("update", help="update a CMUX workspace from a stream")
    cmux_update.add_argument("slug", nargs="?", help="stream filename stem; defaults from current workspace title with --current")
    cmux_update.add_argument("--current", action="store_true", help="target the current CMUX workspace")
    cmux_update.add_argument("--workspace", help="target a CMUX workspace ref, UUID, or title")
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
