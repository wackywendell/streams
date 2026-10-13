from __future__ import annotations

import os
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.cmux import CmuxClient, CmuxWorkspace
from _streams.errors import StreamError


class CmuxIdentityTest(unittest.TestCase):
    def setUp(self) -> None:
        env = patch.dict(os.environ, {}, clear=True)
        env.start()
        self.addCleanup(env.stop)
        native = patch("_streams.cmux.subprocess.run", side_effect=AssertionError("native execution forbidden"))
        self.run = native.start()
        self.addCleanup(native.stop)
        listing = patch.object(CmuxClient, "workspaces", return_value=[])
        self.workspaces = listing.start()
        self.addCleanup(listing.stop)
        self.client = CmuxClient()

    def test_stale_caller_identity_cannot_select_another_workspaces_title(self) -> None:
        os.environ["CMUX_WORKSPACE_ID"] = "stale-id"
        self.workspaces.return_value = [
            CmuxWorkspace({"id": "live-id", "ref": "workspace:2", "title": "stale-id"}),
        ]
        with self.assertRaisesRegex(StreamError, "could not find CMUX workspace 'stale-id'"):
            self.client.resolve_workspace(None)
        self.run.assert_not_called()

    def test_live_caller_id_ignores_another_workspaces_colliding_title(self) -> None:
        os.environ["CMUX_WORKSPACE_ID"] = "live-id"
        intended = CmuxWorkspace({"id": "live-id", "ref": "workspace:1", "title": "Intended"})
        self.workspaces.return_value = [
            intended,
            CmuxWorkspace({"id": "other-id", "ref": "workspace:2", "title": "live-id"}),
        ]
        self.assertEqual(self.client.resolve_workspace(None), intended)

    def test_caller_native_ref_ignores_another_workspaces_colliding_title(self) -> None:
        os.environ["CMUX_WORKSPACE_ID"] = "workspace:1"
        intended = CmuxWorkspace({"id": "live-id", "ref": "workspace:1", "title": "Intended"})
        self.workspaces.return_value = [
            intended,
            CmuxWorkspace({"id": "other-id", "ref": "workspace:2", "title": "workspace:1"}),
        ]
        self.assertEqual(self.client.resolve_workspace(None), intended)

    def test_explicit_title_selects_workspace_without_caller_context(self) -> None:
        intended = CmuxWorkspace({"id": "live-id", "ref": "workspace:1", "title": "Chosen"})
        self.workspaces.return_value = [intended]
        self.assertEqual(self.client.resolve_workspace("Chosen"), intended)

    def test_explicit_duplicate_titles_remain_ambiguous(self) -> None:
        self.workspaces.return_value = [
            CmuxWorkspace({"id": "first-id", "ref": "workspace:1", "title": "Duplicate"}),
            CmuxWorkspace({"id": "second-id", "ref": "workspace:2", "title": "Duplicate"}),
        ]
        with self.assertRaisesRegex(StreamError, "CMUX workspace 'Duplicate' is ambiguous"):
            self.client.resolve_workspace("Duplicate")

    def test_explicit_native_id_colliding_with_title_remains_ambiguous(self) -> None:
        self.workspaces.return_value = [
            CmuxWorkspace({"id": "live-id", "ref": "workspace:1", "title": "Intended"}),
            CmuxWorkspace({"id": "other-id", "ref": "workspace:2", "title": "live-id"}),
        ]
        with self.assertRaisesRegex(StreamError, "CMUX workspace 'live-id' is ambiguous"):
            self.client.resolve_workspace("live-id")

    def test_explicit_native_id_and_ref_still_select_workspace(self) -> None:
        intended = CmuxWorkspace({"id": "live-id", "ref": "workspace:1", "title": "Chosen"})
        self.workspaces.return_value = [intended]
        for selector in ("live-id", "workspace:1"):
            with self.subTest(selector=selector):
                self.assertEqual(self.client.resolve_workspace(selector), intended)

    def test_empty_explicit_selector_fails_before_listing_even_with_caller_context(self) -> None:
        for caller in ("", "live-id"):
            with self.subTest(caller=caller):
                os.environ["CMUX_WORKSPACE_ID"] = caller
                self.workspaces.return_value = [
                    CmuxWorkspace({"id": "live-id", "ref": "workspace:1", "title": "Chosen"}),
                ]
                with self.assertRaisesRegex(StreamError, "workspace.*nonempty"):
                    self.client.resolve_workspace("")
                self.workspaces.assert_not_called()
                self.run.assert_not_called()

    def test_absent_caller_context_fails_before_listing_or_focus_lookup(self) -> None:
        with self.assertRaisesRegex(StreamError, "caller context; pass --workspace"):
            self.client.resolve_workspace(None)
        self.workspaces.assert_not_called()
        self.run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
