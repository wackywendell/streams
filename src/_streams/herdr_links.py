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


def is_web_url(url: str) -> bool:
    try:
        parsed = urlsplit(url)
        return parsed.scheme in {"http", "https"} and bool(parsed.netloc) and all(char.isprintable() for char in url)
    except ValueError:
        return False


def _render(stream: Stream, rows: int | None) -> tuple[str, tuple[str, ...]]:
    external_links: list[tuple[str, str | None]] = []
    shortcut = 0
    for items, kind in ((stream.issues, "Issue"), (stream.prs, "PR")):
        for item in items:
            try:
                parsed = urlsplit(item.url)
                number = parsed.path.rstrip("/").rsplit("/", 1)[-1]
            except ValueError:
                number = "?"
            label = plain(f"{kind} #{number}: {item.description}")
            web_link = is_web_url(item.url)
            if web_link:
                shortcut += 1
                prefix = f"[{shortcut}] " if shortcut <= 9 else ""
                label = f"{prefix}\x1b[94;4m\x1b]8;;{item.url}\x1b\\{label}\x1b]8;;\x1b\\\x1b[0m"
            external_links.append((label, item.url if web_link and shortcut <= 9 else None))
    header = [
        f"[n] \x1b[1m{plain(stream.title)}\x1b[0m · {plain(stream.status)}", plain(stream.state),
    ]
    body = external_links or [("\x1b[3;90mNo linked issues or PRs.\x1b[0m", None)]
    if rows is not None and len(header) + len(body) > rows:
        available = max(0, rows - len(header) - 1)
        hidden = len(body) - available
        body = [*body[:available], (f"\x1b[90m… {hidden} more links; enlarge pane\x1b[0m", None)]
    lines = [*header, *(label for label, _ in body)]
    return "\n".join(lines[:rows]), tuple(url for _, url in body if url is not None)


def render(stream: Stream, *, rows: int | None = None) -> str:
    return _render(stream, rows)[0]


class ShortcutInput:
    """Dispatch standalone keys, not bytes inside terminal sequences or pastes."""

    def __init__(self) -> None:
        self.state = "text"
        self.parameters = ""
        self.paste = False

    def feed(self, data: bytes) -> list[str]:
        keys = []
        for byte in data:
            if self.state == "escape":
                self.state = {ord("["): "csi", ord("O"): "ss3", 27: "escape"}.get(byte, "text")
                if byte in b"]PX^_":
                    self.state = "string"
                self.parameters = ""
            elif self.state in {"csi", "ss3"}:
                if byte == 27:
                    self.state = "escape"
                elif 64 <= byte <= 126:
                    if self.state == "csi" and byte == ord("~"):
                        if self.parameters == "200":
                            self.paste = True
                        elif self.parameters == "201":
                            self.paste = False
                    self.state = "text"
                elif len(self.parameters) < 16:
                    self.parameters += chr(byte)
            elif self.state == "string":
                if byte == 7:
                    self.state = "text"
                elif byte == 27:
                    self.state = "string-escape"
            elif self.state == "string-escape":
                self.state = "text" if byte == ord("\\") else "string"
            elif byte == 27:
                self.state = "escape"
            elif not self.paste and byte in b"n123456789":
                keys.append(chr(byte))
        return keys


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
        self.rows: int | None = None
        self.shortcuts: tuple[str, ...] = ()

    def refresh(self) -> str:
        self.stream = None
        self.shortcuts = ()
        try:
            self.stream = self.runtime.stream(read_binding(self.binding))
            text, self.shortcuts = _render(self.stream, self.rows)
            return text
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

    def _link_url(self, index: int) -> str | None:
        return self.shortcuts[index - 1] if 1 <= index <= len(self.shortcuts) else None

    def open_link(self, index: int) -> None:
        displayed_url = self._link_url(index)
        self.refresh()
        if self.stream is None:
            raise StreamError("Cannot open link: stream is unavailable")
        url = self._link_url(index)
        if url != displayed_url:
            raise StreamError("Links changed; press the key again")
        if url is None:
            raise StreamError(f"No link assigned to [{index}]")
        try:
            subprocess.run(["open", url], check=True, capture_output=True, text=True, timeout=10)
        except (OSError, subprocess.SubprocessError) as exc:
            raise StreamError(f"Open link failed: {exc}") from exc


def run_view(binding: Path) -> None:
    # The generic CLI and read-only rendering do not need Unix terminal modules.
    import select
    import termios
    import tty

    view = LinksView(binding)
    original_terminal = termios.tcgetattr(sys.stdin)
    previous_view = None
    notice = ""
    key_input = ShortcutInput()
    try:
        tty.setcbreak(sys.stdin.fileno())
        sys.stdout.write("\x1b[?1049h\x1b[?25l\x1b[?7l\x1b[?2004h")
        while True:
            size = shutil.get_terminal_size()
            view.rows = size.lines - bool(notice)
            text = view.refresh()
            if notice:
                text = (text + "\n" if text else "") + notice
            rendered = (text, size)
            if rendered != previous_view:
                sys.stdout.write("\x1b[2J\x1b[H" + text)
                sys.stdout.flush()
                previous_view = rendered
            ready, _, _ = select.select([sys.stdin], [], [], 2)
            if ready:
                data = os.read(sys.stdin.fileno(), 4096)
                if not data:
                    break
                displayed_shortcuts = view.shortcuts
                for key in key_input.feed(data):
                    # Every key in this batch was pressed against the same painted map.
                    view.shortcuts = displayed_shortcuts
                    try:
                        if key == "n":
                            view.open_note()
                        else:
                            view.open_link(int(key))
                        notice = ""
                    except StreamError as exc:
                        notice = plain(str(exc))
                        break
    except KeyboardInterrupt:
        pass
    finally:
        termios.tcsetattr(sys.stdin, termios.TCSADRAIN, original_terminal)
        sys.stdout.write("\x1b[?2004l\x1b[?7h\x1b[?25h\x1b[?1049l")
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
