from __future__ import annotations

import dataclasses
import json
import os
import pty
import select
import signal
import subprocess
import sys
import tempfile
import time
import tomllib
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

SRC = Path(__file__).resolve().parents[2] / "src"
sys.path.insert(0, str(SRC))

from _streams.errors import StreamError
from _streams.frontmatter import render_frontmatter
from _streams.herdr_binding import read_binding, write_binding
from _streams.herdr_links import LinksView, NotesConfig, StreamsRuntime, note_uri, render
from _streams.models import Link, Stream
from _streams.store import rename_stream


def example(root: Path = Path("/notes/scratch"), **fields: object) -> Stream:
    return Stream.parse(root / "example.md", {
        "title": "Example stream", "status": "active", "description": "Task", "state": "Ready for review.",
        "repo": None, "cwd": str(root), "vcs": None, "issues": [], "prs": [], "sessions": [],
        **fields,
    }, "")


class LinksRenderTest(unittest.TestCase):
    def test_stream_without_external_links_still_has_note_action(self) -> None:
        rendered = render(example())
        for text in ("Example stream · active", "Ready for review.", "[n] Open scratch note", "No linked issues or PRs."):
            self.assertIn(text, rendered)

    def test_issue_and_pr_labels_are_osc8_http_links(self) -> None:
        stream = example(issues=[{"url": "https://github.com/example/repo/issues/42", "description": "Fix flaky test"}],
                         prs=[{"url": "https://github.com/example/repo/pull/123", "description": "Add missing coverage"}])
        rendered = render(stream)
        for item, kind, number in ((stream.issues[0], "Issue", "42"), (stream.prs[0], "PR", "123")):
            self.assertIn(f"\x1b]8;;{item.url}\x1b\\{kind} #{number}: {item.description}\x1b]8;;\x1b\\", rendered)

    def test_render_reflects_updated_state(self) -> None:
        self.assertNotEqual(render(example()), render(example(state="Review complete.")))
        self.assertIn("Review complete.", render(example(state="Review complete.")))

    def test_note_uri_uses_current_vault_and_relative_path(self) -> None:
        stream = dataclasses.replace(example(), path=Path("/notes/scratch/a note #1.md"))
        parsed = urlsplit(note_uri(stream, NotesConfig(Path("/notes"), "My notes")))
        self.assertEqual((parsed.scheme, parsed.netloc), ("obsidian", "open"))
        self.assertEqual(parse_qs(parsed.query), {"vault": ["My notes"], "file": ["scratch/a note #1.md"]})

    def test_note_root_expands_home_and_rejects_external_notes(self) -> None:
        config = NotesConfig.parse({"notes": {"root": "~/notes"}, "obsidian": {"vault": "Notes"}})
        stream = dataclasses.replace(example(), path=Path.home() / "notes/scratch/example.md")
        self.assertEqual(parse_qs(urlsplit(note_uri(stream, config)).query)["file"], ["scratch/example.md"])
        with self.assertRaisesRegex(StreamError, "outside notes root"):
            note_uri(example(), NotesConfig(Path("/other"), "Notes"))

    def test_text_and_non_web_or_control_containing_urls_cannot_inject_controls(self) -> None:
        stream = example(title="Title\x1b\n\x07", state="State\r\x1b")
        for url in ("obsidian://open?vault=notes", "https://example.com/\x07bad", "https:///missing-host", "not a URL"):
            unsafe = dataclasses.replace(stream, issues=(Link(url, "Description\x1b\x07"),))
            rendered = render(unsafe)
            self.assertNotIn("\x1b", rendered)
            self.assertNotIn("\x07", rendered)
            self.assertNotIn("\r", rendered)
            self.assertIn("Title · active", rendered)

    def test_binding_reader_rejects_missing_malformed_or_empty_reference(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "binding.json"
            with self.assertRaises(StreamError):
                read_binding(path)
            for data in ("not json", "[]", '{"stream":null}', '{"stream":""}'):
                path.write_text(data)
                with self.subTest(data=data), self.assertRaises(StreamError):
                    read_binding(path)

    def test_configuration_decoder_fails_closed(self) -> None:
        for config in ({}, {"notes": {"root": "relative"}, "obsidian": {"vault": "Notes"}},
                       {"notes": {"root": "/notes"}, "obsidian": {"vault": None}}):
            with self.subTest(config=config), self.assertRaises(StreamError):
                NotesConfig.parse(config)


class LinksRuntimeTest(unittest.TestCase):
    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.binding = self.root / "binding.json"
        write_binding(self.binding, "example")
        env = patch.dict(os.environ, {"XDG_CONFIG_HOME": str(self.root / "config")}, clear=True)
        env.start()
        self.addCleanup(env.stop)

    def test_cli_queries_pin_interpreter_and_import_root_without_global_streams(self) -> None:
        result = subprocess.CompletedProcess([], 0, json.dumps(example().as_json()), "")
        with patch.dict(os.environ, {"PYTHONPATH": "/wrong/package", "PYTHONHOME": "/wrong/home"}), \
                patch("_streams.herdr_links.subprocess.run", return_value=result) as run:
            runtime = StreamsRuntime()
            self.assertEqual(runtime.stream("example").title, "Example stream")
            args, kwargs = run.call_args
        self.assertEqual(args[0], [sys.executable, "-P", "-m", "_streams.cli", "show", "example", "--exact", "--json"])
        self.assertEqual(Path(kwargs["env"]["PYTHONPATH"]), SRC.resolve())
        self.assertNotIn("PYTHONHOME", kwargs["env"])
        self.assertEqual(kwargs["timeout"], 10)

    def test_cli_failure_invalid_json_and_invalid_stream_are_expected_errors(self) -> None:
        for failure in (subprocess.CalledProcessError(2, [], stderr="bad note"), FileNotFoundError(),
                        subprocess.TimeoutExpired([], 10)):
            with self.subTest(failure=failure), patch("_streams.herdr_links.subprocess.run", side_effect=failure), \
                    self.assertRaises(StreamError):
                StreamsRuntime().stream("example")
        for output in ("not json", "[]", "{}", json.dumps({"error": {"message": "bad note"}})):
            with self.subTest(output=output), patch("_streams.herdr_links.subprocess.run", return_value=subprocess.CompletedProcess([], 0, output, "")), \
                    self.assertRaises(StreamError):
                StreamsRuntime().stream("example")

    def test_rebinding_and_renames_are_read_fresh(self) -> None:
        notes = self.root / "notes"
        scratch = notes / "scratch"
        scratch.mkdir(parents=True)
        stream = example(scratch)
        stream.path.write_text(render_frontmatter(stream.frontmatter, stream.body))
        from _streams.config import set_config
        set_config("notes.root", str(notes))
        view = LinksView(self.binding)
        self.assertIn("Example stream", view.refresh())
        renamed = rename_stream(stream, "Renamed stream")
        self.assertIn("Renamed stream", view.refresh())
        self.assertEqual(view.stream.path, renamed.path)
        renamed = rename_stream(renamed, "Renamed again")
        self.assertIn("Renamed again", view.refresh())
        self.assertEqual(view.stream.path, renamed.path)
        other = dataclasses.replace(example(scratch, title="Other stream"), path=scratch / "other.md")
        other.path.write_text(render_frontmatter(other.frontmatter, other.body))
        write_binding(self.binding, "other")
        self.assertIn("Other stream", view.refresh())
        self.assertEqual(view.stream.slug, "other")

    def test_note_action_refreshes_binding_stream_and_configuration_at_keypress(self) -> None:
        view = LinksView(self.binding)
        with patch.object(view.runtime, "stream", return_value=example()) as show, \
                patch.object(view.runtime, "config", return_value=NotesConfig(Path("/notes"), "Old")) as config, \
                patch("_streams.herdr_links.subprocess.run") as opener:
            view.refresh()
            write_binding(self.binding, "other")
            config.return_value = NotesConfig(Path("/notes"), "New vault")
            view.open_note()
        self.assertEqual(show.call_args_list[-1].args, ("other",))
        config.assert_called_once_with()
        uri = opener.call_args.args[0][1]
        self.assertEqual(opener.call_args.args[0][0], "open")
        self.assertEqual(parse_qs(urlsplit(uri).query)["vault"], ["New vault"])

    def test_failed_binding_or_note_refresh_disables_stale_opener(self) -> None:
        for failure in ("binding", "note"):
            with self.subTest(failure=failure):
                write_binding(self.binding, "example")
                view = LinksView(self.binding)
                with patch.object(view.runtime, "stream", return_value=example()) as show, \
                        patch("_streams.herdr_links.subprocess.run") as opener:
                    view.refresh()
                    self.assertIsNotNone(view.stream)
                    if failure == "binding":
                        self.binding.write_text("malformed")
                    else:
                        show.side_effect = StreamError("Note is unavailable")
                    self.assertIn("Stream unavailable:", view.refresh())
                    self.assertIsNone(view.stream)
                    with self.assertRaisesRegex(StreamError, "unavailable"):
                        view.open_note()
                    opener.assert_not_called()

    def test_failed_opener_or_config_does_not_discard_a_valid_view(self) -> None:
        view = LinksView(self.binding)
        with patch.object(view.runtime, "stream", return_value=example()), \
                patch.object(view.runtime, "config", return_value=NotesConfig(Path("/notes"), "Notes")) as config, \
                patch("_streams.herdr_links.subprocess.run", side_effect=subprocess.CalledProcessError(2, ["open"])):
            with self.assertRaisesRegex(StreamError, "Open note failed"):
                view.open_note()
            self.assertIsNotNone(view.stream)
            config.side_effect = StreamError("Config is unavailable")
            with self.assertRaisesRegex(StreamError, "Config is unavailable"):
                view.open_note()
            self.assertIsNotNone(view.stream)


class PackagedViewTest(unittest.TestCase):
    def test_static_manifest_launches_packaged_module(self) -> None:
        plugin = SRC / "_streams" / "herdr_plugin"
        manifest = tomllib.loads((plugin / "herdr-plugin.toml").read_text())
        self.assertEqual(manifest["id"], "streams.links")
        self.assertEqual(manifest["min_herdr_version"], "0.9.3")
        self.assertEqual(manifest["panes"][0]["command"], ["/bin/sh", "launch.sh"])
        self.assertEqual(manifest["panes"][0]["placement"], "split")

    def test_generic_cli_does_not_import_the_view_or_unix_terminal_modules(self) -> None:
        result = subprocess.run(
            [sys.executable, "-P", "-c", "import sys; import _streams.cli; assert '_streams.herdr_links' not in sys.modules; assert 'termios' not in sys.modules; assert 'tty' not in sys.modules"],
            env={"PYTHONPATH": str(SRC)}, check=True, capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(result.stdout, "")

    def test_launcher_refuses_missing_runtime_instead_of_guessing_python(self) -> None:
        plugin = SRC / "_streams" / "herdr_plugin"
        result = subprocess.run(["/bin/sh", str(plugin / "launch.sh")], env={}, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("STREAMS_PYTHON", result.stderr)

    def test_manifest_argv_resolves_launcher_from_native_plugin_cwd(self) -> None:
        import_root = Path(os.environ.get("STREAMS_TEST_IMPORT_ROOT", str(SRC)))
        interpreter = os.environ.get("STREAMS_TEST_PYTHON", sys.executable)
        plugin = import_root / "_streams" / "herdr_plugin"
        manifest = tomllib.loads((plugin / "herdr-plugin.toml").read_text())
        result = subprocess.run(
            manifest["panes"][0]["command"], cwd=plugin,
            env={"STREAMS_PYTHON": interpreter, "STREAMS_IMPORT_ROOT": str(import_root), "STREAMS_BINDING": "/absent.json",
                 "PYTHONPATH": "/misleading", "PYTHONHOME": "/misleading"}, capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("requires an interactive terminal", result.stderr)

    def test_live_view_refresh_opener_failure_and_pinned_runtime_over_pty(self) -> None:
        # Set these only when verifying the wheel in a disposable environment.
        import_root = Path(os.environ.get("STREAMS_TEST_IMPORT_ROOT", str(SRC)))
        interpreter = os.environ.get("STREAMS_TEST_PYTHON", sys.executable)
        launcher = import_root / "_streams" / "herdr_plugin" / "launch.sh"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            notes = root / "notes"
            scratch = notes / "scratch"
            scratch.mkdir(parents=True)
            config = root / "config" / "streams" / "config.json"
            config.parent.mkdir(parents=True)
            config.write_text(json.dumps({"notes": {"root": str(notes)}, "obsidian": {"vault": "Notes"}}))
            stream = example(scratch)
            stream.path.write_text(render_frontmatter(stream.frontmatter, stream.body))
            binding = root / "binding.json"
            write_binding(binding, stream.slug)
            evil = root / "_streams"
            evil.mkdir()
            (evil / "__init__.py").write_text("raise RuntimeError('wrong import root')\n")
            bin = root / "bin"
            bin.mkdir()
            for name, source in {
                "streams": "from pathlib import Path\nPath('wrong-streams').touch()\nraise SystemExit(7)\n",
                "open": "import json,sys\nfrom pathlib import Path\nif Path('fail-open').exists(): raise SystemExit(2)\nPath('opened.json').write_text(json.dumps(sys.argv[1:]))\n",
            }.items():
                path = bin / name
                path.write_text(f"#!{sys.executable}\n" + source)
                path.chmod(0o755)
            master, slave = pty.openpty()
            process = subprocess.Popen(
                ["/bin/sh", str(launcher)], stdin=slave, stdout=slave, stderr=slave, cwd=root,
                env={"PATH": str(bin), "HOME": str(root), "XDG_CONFIG_HOME": str(root / "config"),
                     "STREAMS_PYTHON": interpreter, "STREAMS_IMPORT_ROOT": str(import_root), "STREAMS_BINDING": str(binding),
                     "PYTHONPATH": str(root), "PYTHONHOME": str(root / "wrong-python")},
            )
            # Keep a slave handle open: macOS can discard unread final output
            # when the child's last slave handle closes.
            output = b""

            def await_condition(condition) -> None:
                nonlocal output
                deadline = time.monotonic() + 7
                while time.monotonic() < deadline:
                    if condition():
                        return
                    if process.poll() is not None:
                        self.fail(f"View exited early: {output!r}")
                    ready, _, _ = select.select([master], [], [], 0.05)
                    if ready:
                        try:
                            output += os.read(master, 65536)
                        except OSError:
                            self.fail(f"View closed PTY early: {output!r}")
                self.fail(f"Timed out waiting for view: {output!r}")

            try:
                await_condition(lambda: b"No linked issues or PRs." in output)
                self.assertIn(b"\x1b[?1049h\x1b[?25l\x1b[?7l", output)
                os.write(master, b"n")
                await_condition(lambda: (root / "opened.json").exists())
                uri = json.loads((root / "opened.json").read_text())[0]
                self.assertEqual(parse_qs(urlsplit(uri).query), {"vault": ["Notes"], "file": ["scratch/example.md"]})
                stream.path.write_text(render_frontmatter({**stream.frontmatter, "state": "State changed while pane is open."}, ""))
                await_condition(lambda: b"State changed while pane is open." in output)
                (root / "fail-open").touch()
                os.write(master, b"n")
                await_condition(lambda: b"Open note failed:" in output)
                self.assertIsNone(process.poll())
                self.assertFalse((root / "wrong-streams").exists())
                process.send_signal(signal.SIGINT)
                process.wait(timeout=5)
                self.assertEqual(process.returncode, 0)
                # Drain final terminal restoration without waiting for another refresh.
                deadline = time.monotonic() + 1
                while time.monotonic() < deadline and select.select([master], [], [], 0.1)[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError:
                        break
                    if not chunk:
                        break
                    output += chunk
                self.assertIn(b"\x1b[?7h\x1b[?25h\x1b[?1049l", output)
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
                os.close(slave)
                os.close(master)


if __name__ == "__main__":
    unittest.main()
