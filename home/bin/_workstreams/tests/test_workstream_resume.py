from __future__ import annotations

import importlib.util
import sys
import unittest
from importlib.machinery import SourceFileLoader
from pathlib import Path
from unittest.mock import patch


BIN_DIR = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(BIN_DIR))

from _workstreams.models import Task


def load_workstream_resume():
    script = BIN_DIR / "workstream-resume"
    loader = SourceFileLoader("workstream_resume", str(script))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    if not spec or not spec.loader:
        raise RuntimeError(f"could not load {script}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


resume = load_workstream_resume()


class CommandForSessionTest(unittest.TestCase):
    def test_prefixed_codex_session(self) -> None:
        task = Task(uuid="task-uuid", aisession="codex:abc")
        self.assertEqual(resume.AgentSession.from_task(task).argv, ("codex", "resume", "abc"))

    def test_prefixed_claude_session(self) -> None:
        task = Task(uuid="task-uuid", aisession="claude:lalrpop")
        self.assertEqual(resume.AgentSession.from_task(task).argv, ("claude", "--resume", "lalrpop"))

    def test_prefixed_pi_session(self) -> None:
        task = Task(uuid="task-uuid", aisession="pi:019e")
        self.assertEqual(resume.AgentSession.from_task(task).argv, ("pi", "--session", "019e"))

    def test_legacy_uuid_defaults_to_codex(self) -> None:
        task = Task(uuid="task-uuid", aisession="019de02e-19b2-7a50-9a01-ac533b396f12")
        self.assertEqual(
            resume.AgentSession.from_task(task).argv,
            ("codex", "resume", "019de02e-19b2-7a50-9a01-ac533b396f12"),
        )

    def test_claude_tag_wins_for_legacy_unprefixed_session(self) -> None:
        task = Task(
            uuid="task-uuid",
            tags=frozenset({"claude"}),
            aisession="019de02e-19b2-7a50-9a01-ac533b396f12",
        )
        self.assertEqual(
            resume.AgentSession.from_task(task).argv,
            ("claude", "--resume", "019de02e-19b2-7a50-9a01-ac533b396f12"),
        )

    def test_unprefixed_claude_slug_uses_claude_tag(self) -> None:
        task = Task(uuid="task-uuid", tags=frozenset({"claude"}), aisession="lalrpop-grammar-2")
        self.assertEqual(
            resume.AgentSession.from_task(task).argv,
            ("claude", "--resume", "lalrpop-grammar-2"),
        )

    def test_unknown_unprefixed_session_fails(self) -> None:
        task = Task(uuid="task-uuid", aisession="lalrpop-grammar-2")
        with self.assertRaisesRegex(resume.WorkstreamResumeError, "do not know whether"):
            resume.AgentSession.from_task(task)

    def test_empty_prefixed_session_fails(self) -> None:
        task = Task(uuid="task-uuid", aisession="codex:")
        with self.assertRaisesRegex(resume.WorkstreamResumeError, "codex aisession is empty"):
            resume.AgentSession.from_task(task)

    def test_command_text_quotes_arguments(self) -> None:
        self.assertEqual(
            resume.AgentSession(resume.Agent.CLAUDE, "session with spaces").command_text,
            "claude --resume 'session with spaces'",
        )


class CmuxTargetTest(unittest.TestCase):
    def plan(
        self,
        *,
        target: object | None = None,
        session: object | None = None,
    ):
        workspace = resume.CmuxWorkspace.from_raw({"ref": "workspace:2", "title": "Task"})
        task = Task(uuid="task-uuid", aisession="codex:abc")
        return resume.ResumePlan(
            workspace=workspace,
            task=task,
            session=session or resume.AgentSession(resume.Agent.CODEX, "abc"),
            target=target or resume.ResumeTarget(resume.TargetMode.NEW_PANE),
        )

    def test_target_from_args_current_terminal(self) -> None:
        target = resume.ResumeTarget.from_args(workspace=None, panel=None)
        self.assertEqual(target.mode, resume.TargetMode.CURRENT_TERMINAL)
        self.assertEqual(target.label, "current-terminal")

    def test_target_from_args_new_pane(self) -> None:
        target = resume.ResumeTarget.from_args(workspace="workspace:2", panel=None)
        self.assertEqual(target.mode, resume.TargetMode.NEW_PANE)
        self.assertEqual(target.label, "new-pane")

    def test_target_from_args_existing_panel(self) -> None:
        target = resume.ResumeTarget.from_args(workspace="workspace:2", panel="surface:3")
        self.assertEqual(target.mode, resume.TargetMode.EXISTING_PANEL)
        self.assertEqual(target.panel, "surface:3")

    def test_existing_panel_target_requires_panel(self) -> None:
        with self.assertRaisesRegex(ValueError, "requires a panel"):
            resume.ResumeTarget(resume.TargetMode.EXISTING_PANEL)

    def test_existing_panel_requires_force(self) -> None:
        plan = self.plan(target=resume.ResumeTarget(resume.TargetMode.EXISTING_PANEL, "surface:3"))
        cmux = resume.CmuxClient()
        with self.assertRaisesRegex(resume.WorkstreamResumeError, "without --force"):
            cmux.send_to_existing_panel(plan, force=False)

    def test_existing_panel_with_force_sends_quoted_command(self) -> None:
        plan = self.plan(
            target=resume.ResumeTarget(resume.TargetMode.EXISTING_PANEL, "surface:3"),
            session=resume.AgentSession(resume.Agent.CLAUDE, "session with spaces"),
        )
        cmux = resume.CmuxClient()
        with patch.object(cmux, "run") as run:
            cmux.send_to_existing_panel(plan, force=True)
        run.assert_called_once_with(
            [
                "cmux",
                "send-panel",
                "--workspace",
                "workspace:2",
                "--panel",
                "surface:3",
                "claude --resume 'session with spaces'\\n",
            ]
        )

    def test_workspace_target_creates_fresh_pane_before_respawning(self) -> None:
        cmux = resume.CmuxClient()
        with patch.object(cmux, "run") as run:
            cmux.run_in_new_pane(self.plan())
        self.assertEqual(
            run.call_args_list[0].args[0],
            [
                "cmux",
                "new-pane",
                "--type",
                "terminal",
                "--workspace",
                "workspace:2",
                "--focus",
                "true",
            ],
        )
        self.assertEqual(
            run.call_args_list[1].args[0],
            [
                "cmux",
                "respawn-pane",
                "--workspace",
                "workspace:2",
                "--command",
                "codex resume abc",
            ],
        )


if __name__ == "__main__":
    unittest.main()
