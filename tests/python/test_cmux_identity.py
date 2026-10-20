from __future__ import annotations

import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog_runtime import Runtime
from _streams.cmux import CmuxClient, CmuxWorkspace
from _streams.errors import StreamError
from _streams.frontend import Frontend, FrontendMode


class CmuxCallerIdentityTest(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.runtime = Runtime(Path(temporary.name), "local", FrontendMode.CMUX)
        self.enterContext(patch.dict(os.environ, {}, clear=True))
        self.run = self.enterContext(patch("_streams.cmux.subprocess.run", side_effect=AssertionError("native execution forbidden")))
        self.workspaces = self.enterContext(patch.object(CmuxClient, "workspaces", return_value=[]))
        self.client = CmuxClient()

    def caller(self):
        return self.runtime._caller_workspace(Frontend.CMUX, self.client)

    def test_stale_caller_id_cannot_select_another_workspaces_title(self) -> None:
        os.environ["CMUX_WORKSPACE_ID"] = "stale-id"
        self.workspaces.return_value = [CmuxWorkspace({"id": "live-id", "title": "stale-id"})]
        with self.assertRaisesRegex(StreamError, "exact CMUX caller workspace ID"):
            self.caller()
        self.run.assert_not_called()

    def test_live_caller_id_ignores_another_workspaces_colliding_title(self) -> None:
        os.environ["CMUX_WORKSPACE_ID"] = "live-id"
        self.workspaces.return_value = [
            CmuxWorkspace({"id": "live-id", "ref": "workspace:1", "title": "Intended"}),
            CmuxWorkspace({"id": "other-id", "title": "live-id"}),
        ]
        self.assertEqual(self.caller(), "live-id")
        self.run.assert_not_called()

    def test_caller_native_ref_is_not_an_exact_workspace_id(self) -> None:
        os.environ["CMUX_WORKSPACE_ID"] = "workspace:1"
        self.workspaces.return_value = [CmuxWorkspace({"id": "live-id", "ref": "workspace:1"})]
        with self.assertRaisesRegex(StreamError, "exact CMUX caller workspace ID"):
            self.caller()
        self.run.assert_not_called()

    def test_duplicate_native_ids_are_rejected(self) -> None:
        os.environ["CMUX_WORKSPACE_ID"] = "live-id"
        self.workspaces.return_value = [CmuxWorkspace({"id": "live-id"}), CmuxWorkspace({"id": "live-id"})]
        with self.assertRaisesRegex(StreamError, "exact CMUX caller workspace ID"):
            self.caller()

    def test_absent_or_empty_context_fails_before_listing_or_focus_lookup(self) -> None:
        for caller in (None, "", " "):
            with self.subTest(caller=caller):
                if caller is not None:
                    os.environ["CMUX_WORKSPACE_ID"] = caller
                with self.assertRaisesRegex(StreamError, "exact caller workspace context"):
                    self.caller()
                self.workspaces.assert_not_called()
                self.run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
