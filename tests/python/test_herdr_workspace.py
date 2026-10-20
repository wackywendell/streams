from __future__ import annotations

import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'src'))

from _streams.errors import StreamError
from _streams.herdr import HerdrClient, HerdrWorkspace


class HerdrWorkspaceTest(unittest.TestCase):
    def setUp(self):
        self.enterContext(patch.dict(os.environ, {}, clear=True))
        self.client = HerdrClient()
        self.result = self.enterContext(patch.object(self.client, 'command_result'))
        self.current = self.enterContext(patch.object(self.client, 'current_pane', side_effect=AssertionError('implicit caller lookup')))
        self.result.return_value = {'workspace': {'workspace_id': 'w1', 'label': 'Work'}}

    def test_explicit_workspace_resolves_without_caller_context(self):
        self.assertEqual(self.client.resolve_workspace('w1'), HerdrWorkspace('w1', 'Work'))
        self.result.assert_called_once_with(['workspace', 'get', 'w1'], 'workspace_info')
        self.current.assert_not_called()

    def test_empty_selector_fails_before_native_query(self):
        for selector in ('', ' '):
            with self.subTest(selector=selector), self.assertRaisesRegex(StreamError, 'nonempty workspace ID'):
                self.client.resolve_workspace(selector)
        self.result.assert_not_called()
        self.current.assert_not_called()

    def test_response_must_match_exact_selected_workspace_id(self):
        self.result.return_value = {'workspace': {'workspace_id': 'other', 'label': 'Work'}}
        with self.assertRaisesRegex(StreamError, 'workspace ID mismatch'):
            self.client.resolve_workspace('w1')


if __name__ == '__main__':
    unittest.main()
