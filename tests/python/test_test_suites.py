"""The aggregate gates partition discovery without losing failed imports."""
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

from suites import suite, test_files


class TestSuites(unittest.TestCase):
    def test_groups_partition_every_test_module_once(self):
        directory = Path(__file__).parent
        reference = set(test_files(directory, "reference"))
        differential = set(test_files(directory, "differential"))
        self.assertFalse(reference & differential)
        self.assertEqual(reference | differential, set(directory.glob("test*.py")))
        self.assertIn(directory / "test_rust_git_differential.py", differential)
        self.assertIn(directory / "test_catalog_runtime.py", reference)
        self.assertIn(directory / "test_test_suites.py", reference)

    def test_selected_import_errors_are_not_silently_filtered(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(sys, "path", list(sys.path)):
            root = Path(directory)
            (root / "test_suites_broken_reference.py").write_text("raise ImportError(\"fixture failure\")")
            result = unittest.TestResult()
            suite(root, "reference").run(result)
            self.assertEqual(result.testsRun, 1)
            self.assertEqual(len(result.errors), 1)
            self.assertIn("fixture failure", result.errors[0][1])


if __name__ == "__main__":
    unittest.main()
