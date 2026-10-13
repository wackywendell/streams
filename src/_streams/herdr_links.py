"""Tab-local Herdr links view, using the opening distribution's CLI runtime."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from .errors import StreamError
from .herdr_binding import read_binding
from .models import Stream
from .projection import obsidian_uri_for_stream


def plain(text: str) -> str:
    return "".join(char for char in text if char.isprintable())


def render(stream: Stream) -> str:
    external_links: list[str] = []
    for items, kind in ((stream.issues, "Issue"), (stream.prs, "PR")):
        for item in items:
            try:
                parsed = urlsplit(item.url)
                number = parsed.path.rstrip("/").rsplit("/", 1)[-1]
                web_link = parsed.scheme in {"http", "https"} and bool(parsed.netloc)
            except ValueError:
                number, web_link = "?", False
            label = plain(f"{kind} #{number}: {item.description}")
            if web_link and all(char.isprintable() for char in item.url):
                label = f"\x1b]8;;{item.url}\x1b\\{label}\x1b]8;;\x1b\\"
            external_links.append(label)
    return "\n".join([
        plain(f"{stream.title} · {stream.status}"), plain(stream.state),
        "[n] Open scratch note in Obsidian · Ctrl-click issue/PR links",
        " · ".join(external_links) or "No linked issues or PRs.",
    ])


@dataclass(frozen=True)
class NotesConfig:
    root: Path
    vault: str

    @classmethod
    def parse(cls, data: dict[str, Any]) -> NotesConfig:
        notes, obsidian = data.get("notes"), data.get("obsidian")
        root = notes.get("root") if isinstance(notes, dict) else None
        vault = obsidian.get("vault") if isinstance(obsidian, dict) else None
        if not isinstance(root, str) or not root.strip() or not isinstance(vault, str) or not vault.strip():
            raise StreamError("streams config returned invalid notes/Obsidian settings")
        path = Path(root).expanduser()
        if not path.is_absolute():
            raise StreamError("streams config returned a non-absolute notes root")
        return cls(path.resolve(), vault)


def note_uri(stream: Stream, config: NotesConfig) -> str:
    return obsidian_uri_for_stream(replace(stream, path=stream.path.resolve()), root=config.root.resolve(), vault=config.vault)


class StreamsRuntime:
    def __init__(self) -> None:
        self.environment = {**os.environ, "PYTHONPATH": str(Path(__file__).resolve().parents[1])}
        self.environment.pop("PYTHONHOME", None)

    def json(self, *args: str) -> dict[str, Any]:
        try:
            result = subprocess.run(
                [sys.executable, "-P", "-m", "_streams.cli", *args, "--json"],
                env=self.environment, check=True, capture_output=True, text=True, timeout=10,
            )
            data = json.loads(result.stdout)
        except (OSError, subprocess.SubprocessError, ValueError) as exc:
            # Do not include arbitrary CLI output or note contents in the terminal.
            raise StreamError(f"streams {' '.join(args[:1])} query failed: {exc}") from exc
        if not isinstance(data, dict):
            raise StreamError("streams query returned an unexpected JSON shape")
        if "error" in data:
            raise StreamError("streams query reported an error")
        return data

    def stream(self, ref: str) -> Stream:
        data = self.json("show", ref, "--exact")
        path, slug = data.pop("path", None), data.pop("slug", None)
        if not isinstance(path, str) or not Path(path).is_absolute() or not isinstance(slug, str) or Path(path).stem != slug:
            raise StreamError("streams show returned an invalid path/slug")
        return Stream.parse(Path(path), data, "")

    def config(self) -> NotesConfig:
        return NotesConfig.parse(self.json("config"))


class LinksView:
    def __init__(self, binding: Path) -> None:
        self.binding = binding
        self.runtime = StreamsRuntime()
        self.stream: Stream | None = None

    def refresh(self) -> str:
        self.stream = None
        try:
            self.stream = self.runtime.stream(read_binding(self.binding))
            return render(self.stream)
        except StreamError as exc:
            return "Stream unavailable: " + plain(str(exc))

    def open_note(self) -> None:
        # Recheck the binding and note at keypress, not just on the refresh timer.
        self.refresh()
        if self.stream is None:
            raise StreamError("Cannot open note: stream is unavailable")
        uri = note_uri(self.stream, self.runtime.config())
        try:
            subprocess.run(["open", uri], check=True, capture_output=True, text=True, timeout=10)
        except (OSError, subprocess.SubprocessError) as exc:
            raise StreamError(f"Open note failed: {exc}") from exc


def run_view(binding: Path) -> None:
    # The generic CLI and read-only rendering do not need Unix terminal modules.
    import select
    import termios
    import tty

    view = LinksView(binding)
    original_terminal = termios.tcgetattr(sys.stdin)
    previous_view = None
    notice = ""
    try:
        tty.setcbreak(sys.stdin.fileno())
        sys.stdout.write("\x1b[?1049h\x1b[?25l\x1b[?7l")
        while True:
            text = view.refresh()
            if notice:
                text += "\n" + notice
            rendered = (text, shutil.get_terminal_size())
            if rendered != previous_view:
                sys.stdout.write("\x1b[2J\x1b[H" + text)
                sys.stdout.flush()
                previous_view = rendered
            ready, _, _ = select.select([sys.stdin], [], [], 2)
            if ready:
                key = sys.stdin.read(1)
                if not key:
                    break
                if key == "n":
                    try:
                        view.open_note()
                        notice = ""
                    except StreamError as exc:
                        notice = plain(str(exc))
    except KeyboardInterrupt:
        pass
    finally:
        termios.tcsetattr(sys.stdin, termios.TCSADRAIN, original_terminal)
        sys.stdout.write("\x1b[?7h\x1b[?25h\x1b[?1049l")
        sys.stdout.flush()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Display links for an explicitly bound Herdr workspace.")
    parser.add_argument("--binding", type=Path, required=True)
    args = parser.parse_args(argv)
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        print("streams links view requires an interactive terminal", file=sys.stderr)
        return 2
    run_view(args.binding)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
