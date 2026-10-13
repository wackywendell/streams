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

from _streams.cli import build_parser, main
from _streams.cmux import CmuxWorkspace
from _streams.errors import StreamError
from _streams.frontend import FrontendMode
from _streams.herdr import HerdrWorkspace
from _streams.herdr_binding import HerdrEndpoint
from _streams.location import StreamLocation
from test_streams import write_stream


CMUX = {"CMUX_WORKSPACE_ID": "workspace:caller"}
HERDR = {"HERDR_ENV": "1", "HERDR_PANE_ID": "old:pane"}


class CurrentSelectorParserTest(unittest.TestCase):
    commands = (["start", "New"], ["rename", "old", "New"], ["resume"])

    def parse(self, args):
        with patch("sys.stderr", io.StringIO()):
            return build_parser().parse_args(args)

    def test_one_normalized_optional_selector(self):
        forms = [((), None), (("-c",), "auto"), (("--current",), "auto"),
                 (("-f", "auto"), "auto"), (("--frontend", "auto"), "auto"),
                 (("-f", "cmux"), "cmux"), (("-f", "herdr"), "herdr")]
        for command in self.commands:
            for flags, expected in forms:
                with self.subTest(command=command, flags=flags):
                    args = self.parse([*command, *flags])
                    self.assertEqual(args.frontend, expected)
                    self.assertFalse(hasattr(args, "current"))

    def test_missing_invalid_repeated_and_conflicting_forms_fail(self):
        flags = [("-f",), ("--frontend",), ("-f", "none"), ("-f", "unknown"),
                 ("-c", "--current"), ("-c", "-c"), ("-c", "-f", "auto"),
                 ("-f", "herdr", "-c"), ("-f", "cmux", "-f", "herdr"),
                 ("-f", "auto", "--frontend", "auto")]
        for command in self.commands:
            for form in flags:
                with self.subTest(command=command, form=form):
                    with self.assertRaises(SystemExit) as error:
                        self.parse([*command, *form])
                    self.assertEqual(error.exception.code, 2)

    def test_named_groups_remain_explicit_and_current_is_only_a_target(self):
        for name in ("cmux", "herdr"):
            args = self.parse([name, "update", "stream", "-c"])
            self.assertTrue(args.current)
            self.assertFalse(hasattr(args, "frontend"))
            with self.assertRaises(SystemExit):
                self.parse([name, "update", "stream", "-f", "herdr"])


class CurrentCliTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.notes = self.root / "notes"
        self.notes.mkdir()
        self.enterContext(patch.dict(os.environ, {"HOME": str(self.root), "XDG_CONFIG_HOME": str(self.root / "config")}, clear=True))
        self.enterContext(patch("_streams.store.scratch_root", return_value=self.notes))
        self.enterContext(patch("_streams.resolve.scratch_root", return_value=self.notes))
        self.enterContext(patch("_streams.store.discover_stream_location", return_value=StreamLocation(None, None, self.root)))
        # Native execution is always forbidden in this suite, including inherited caller context.
        self.native = self.enterContext(patch("subprocess.run", side_effect=AssertionError("unexpected native call")))

    def cli(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with patch("sys.stdout", out), patch("sys.stderr", err):
            try:
                code = main(list(args))
            except SystemExit as error:
                code = error.code
        return code, out.getvalue(), err.getvalue()

    def backends(self):
        cmux = self.enterContext(patch("_streams.current.CmuxClient")).return_value
        cmux.resolve_workspace.return_value = CmuxWorkspace({"ref": "workspace:caller", "title": "Old"})
        herdr = self.enterContext(patch("_streams.current.HerdrClient")).return_value
        herdr.resolve_workspace.return_value = HerdrWorkspace("live:workspace", "Old", str(self.root))
        herdr.endpoint.return_value = HerdrEndpoint("/actual/herdr.sock", None)
        update_cmux = self.enterContext(patch("_streams.current.update_cmux_workspace"))
        update_herdr = self.enterContext(patch("_streams.current.update_herdr_workspace"))
        return cmux, herdr, update_cmux, update_herdr

    def test_all_policy_aliases_follow_configured_mode(self):
        cmux, herdr, update_cmux, update_herdr = self.backends()
        for mode in FrontendMode:
            for index, form in enumerate((("-c",), ("--current",), ("-f", "auto"), ("--frontend", "auto"))):
                title = f"{mode} {index}"
                with self.subTest(mode=mode, form=form), patch("_streams.cli.get_frontend_mode", return_value=mode), patch.dict(os.environ, HERDR):
                    cmux.reset_mock(); herdr.reset_mock(); update_cmux.reset_mock(); update_herdr.reset_mock()
                    code, out, err = self.cli("start", title, *form, "--json")
                    if mode == FrontendMode.NONE:
                        self.assertEqual(code, 2)
                        self.assertFalse((self.notes / f"{mode}-{index}.md").exists())
                        cmux.resolve_workspace.assert_not_called()
                        herdr.resolve_workspace.assert_not_called()
                    else:
                        self.assertEqual((code, err), (0, ""))
                        is_cmux = mode == FrontendMode.CMUX
                        expected = "workspace:caller" if is_cmux else "live:workspace"
                        self.assertEqual(json.loads(out)["updated_workspace"], expected)
                        selected, other = (cmux, herdr) if is_cmux else (herdr, cmux)
                        selected.resolve_workspace.assert_called_once_with(None)
                        other.resolve_workspace.assert_not_called()
                        updater = update_cmux if is_cmux else update_herdr
                        updater.assert_called_once()
                        self.assertIs(updater.call_args.args[2], selected)

    def test_named_selectors_override_none_and_need_no_current_flag(self):
        cmux, herdr, update_cmux, update_herdr = self.backends()
        for name, context, client, update in (("cmux", CMUX, cmux, update_cmux), ("herdr", HERDR, herdr, update_herdr)):
            with self.subTest(name=name), patch("_streams.cli.get_frontend_mode", return_value=FrontendMode.NONE), patch.dict(os.environ, context):
                code, out, err = self.cli("start", name, "-f", name)
                self.assertEqual((code, err), (0, ""))
                client.resolve_workspace.assert_called_once_with(None)
                update.assert_called_once()
                self.assertIn("updated:", out)

    def test_note_only_start_and_rename_never_resolve_policy_or_probe_native(self):
        with patch("_streams.cli.get_frontend_mode", side_effect=AssertionError("note-only policy lookup")):
            self.assertEqual(self.cli("start", "New")[0], 0)
            self.assertEqual(self.cli("rename", "new", "Renamed")[0], 0)
            code, out, err = self.cli("start", "Preview", "--dry-run", "--json")
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(json.loads(out)["slug"], "preview")
        self.assertFalse((self.notes / "preview.md").exists())
        self.native.assert_not_called()

    def test_dry_run_with_selector_is_rejected_without_writes_or_target_lookup(self):
        with patch("_streams.cli.get_frontend_mode", side_effect=AssertionError("dry-run lookup")):
            for form in (("-c",), ("-f", "cmux"), ("-f", "herdr")):
                code, out, err = self.cli("start", "New", "--dry-run", *form)
                self.assertEqual((code, out), (2, ""))
                self.assertIn("dry-run", err)
        self.assertEqual(list(self.notes.iterdir()), [])

    def test_no_or_bad_auto_context_fails_before_start_or_rename(self):
        original = write_stream(self.notes, "old", title="Old")
        content = original.read_bytes()
        for context in ({}, CMUX | HERDR, {"HERDR_ENV": "1"}):
            for command in (("start", "New"), ("rename", "old", "New")):
                with self.subTest(context=context, command=command), patch.dict(os.environ, context):
                    code, out, err = self.cli(*command, "-c")
                    self.assertEqual((code, out), (2, ""))
                    self.assertTrue(err)
                    self.assertEqual(original.read_bytes(), content)
                    self.assertEqual(list(self.notes.iterdir()), [original])
        self.native.assert_not_called()

    def test_named_current_requires_its_own_caller_before_any_write(self):
        original = write_stream(self.notes, "old", title="Old")
        content = original.read_bytes()
        for name in ("cmux", "herdr"):
            for command in (("start", "New"), ("rename", "old", "New")):
                with self.subTest(name=name, command=command):
                    code, out, err = self.cli(*command, "-f", name)
                    self.assertEqual((code, out), (2, ""))
                    self.assertIn("caller context", err)
                    self.assertEqual(original.read_bytes(), content)
                    self.assertEqual(list(self.notes.iterdir()), [original])
        self.native.assert_not_called()

    def test_native_target_failure_precedes_note_write(self):
        cmux, herdr, _, _ = self.backends()
        original = write_stream(self.notes, "old", title="Old")
        for name, client in (("cmux", cmux), ("herdr", herdr)):
            client.resolve_workspace.side_effect = StreamError("caller moved or unavailable")
            for command in (("start", "New"), ("rename", "old", "New")):
                with self.subTest(name=name, command=command):
                    self.assertEqual(self.cli(*command, "-f", name)[0], 2)
                    self.assertEqual(list(self.notes.iterdir()), [original])

    def test_rename_updates_the_preflighted_target_in_either_frontend(self):
        cmux, herdr, update_cmux, update_herdr = self.backends()
        for name, client, update in (("cmux", cmux, update_cmux), ("herdr", herdr, update_herdr)):
            with self.subTest(name=name):
                write_stream(self.notes, f"old-{name}", title=f"Old {name}")
                code, out, err = self.cli("rename", f"old-{name}", f"New {name}", "-f", name)
                self.assertEqual((code, err), (0, ""))
                self.assertIn(f"renamed: old-{name} -> new-{name}", out)
                self.assertIn(f"updated: {client.resolve_workspace.return_value.display_ref if name == 'cmux' else 'live:workspace'}", out)
                stream, workspace, used_client = update.call_args.args
                self.assertEqual(stream.slug, f"new-{name}")
                self.assertIs(workspace, client.resolve_workspace.return_value)
                self.assertIs(used_client, client)
                self.assertTrue((self.notes / f"old-{name}.md").is_symlink())

    def test_failed_projection_retains_created_note_and_reports_actual_slug_and_path(self):
        _, _, _, update = self.backends()
        update.side_effect = StreamError("native refresh may be partial")
        code, out, err = self.cli("start", "New", "-f", "herdr", "--json")
        self.assertEqual((code, err), (2, ""))
        note = self.notes / "new.md"
        self.assertTrue(note.is_file())
        message = json.loads(out)["error"]["message"]
        self.assertIn("created: new", message)
        self.assertIn(str(note), message)
        self.assertIn("native refresh may be partial", message)
        self.assertIn("live:workspace", message)
        update.assert_called_once()

    def test_failed_projection_retains_rename_and_alias_without_retry(self):
        _, _, update, _ = self.backends()
        write_stream(self.notes, "old", title="Old")
        update.side_effect = StreamError("metadata failed")
        code, out, err = self.cli("rename", "old", "New", "-f", "cmux")
        self.assertEqual((code, out), (2, ""))
        self.assertTrue((self.notes / "new.md").is_file())
        self.assertTrue((self.notes / "old.md").is_symlink())
        self.assertIn("renamed: old -> new", err)
        self.assertIn(str(self.notes / "new.md"), err)
        self.assertIn("workspace:caller", err)
        update.assert_called_once()

    def test_resume_requires_selector_when_no_ref_and_rejects_selector_with_ref(self):
        with patch("_streams.cli.exec_resume") as execute, patch("_streams.cli.get_frontend_mode", side_effect=AssertionError("invalid resume lookup")):
            for arguments in ((), ("old", "-c"), ("old", "-f", "herdr")):
                code, out, err = self.cli("resume", *arguments)
                self.assertEqual((code, out), (2, ""))
                self.assertIn("selector", err)
            execute.assert_not_called()

    def test_explicit_resume_is_frontend_independent_even_with_mixed_context(self):
        write_stream(self.notes, "old", title="Old")
        with patch.dict(os.environ, CMUX | HERDR), patch("_streams.cli.get_frontend_mode", side_effect=AssertionError("explicit resume policy")), patch("_streams.cli.exec_resume") as execute:
            self.assertEqual(self.cli("resume", "old")[0], 0)
            self.assertEqual(execute.call_args.args[0].slug, "old")
        self.native.assert_not_called()

    def test_resume_selector_infers_from_live_target_without_projection(self):
        cmux, herdr, update_cmux, update_herdr = self.backends()
        write_stream(self.notes, "old", title="Old")
        for name, client in (("cmux", cmux), ("herdr", herdr)):
            with self.subTest(name=name), patch("_streams.cli.exec_resume") as execute:
                self.assertEqual(self.cli("resume", "-f", name)[0], 0)
                self.assertEqual(execute.call_args.args[0].slug, "old")
                client.resolve_workspace.assert_called_once_with(None)
        update_cmux.assert_not_called(); update_herdr.assert_not_called()

    def test_resume_auto_obeys_policy_and_fails_without_a_selected_frontend(self):
        cmux, herdr, update_cmux, update_herdr = self.backends()
        write_stream(self.notes, "old", title="Old")
        for mode in FrontendMode:
            with self.subTest(mode=mode), patch("_streams.cli.get_frontend_mode", return_value=mode), patch.dict(os.environ, HERDR), patch("_streams.cli.exec_resume") as execute:
                cmux.reset_mock(); herdr.reset_mock()
                code, out, err = self.cli("resume", "-c")
                self.assertEqual(code, 2 if mode == FrontendMode.NONE else 0)
                if mode == FrontendMode.NONE:
                    execute.assert_not_called()
                else:
                    self.assertEqual(execute.call_args.args[0].slug, "old")
                    (cmux if mode == FrontendMode.CMUX else herdr).resolve_workspace.assert_called_once_with(None)
        update_cmux.assert_not_called(); update_herdr.assert_not_called()

    def test_resume_inference_fails_conservatively(self):
        cmux, herdr, _, _ = self.backends()
        write_stream(self.notes, "old", title="Different", repo=str(self.root), cwd=str(self.root))
        cmux.resolve_workspace.return_value = CmuxWorkspace({"ref": "workspace:caller", "title": "Old", "current_directory": str(self.root)})
        with patch("_streams.cli.exec_resume") as execute:
            for name in ("cmux", "herdr"):
                code, out, err = self.cli("resume", "-f", name)
                self.assertEqual((code, out), (2, ""))
                self.assertIn("pass a slug", err)
            execute.assert_not_called()


if __name__ == "__main__":
    unittest.main()
