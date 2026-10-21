"""Disjoint reference/differential gates using standard unittest discovery."""
from __future__ import annotations

import argparse
from fnmatch import fnmatch
from pathlib import Path
import unittest


def test_files(directory: Path, group: str) -> list[Path]:
    return [path for path in sorted(directory.glob("test*.py"))
            if fnmatch(path.name, "test_rust*differential.py") == (group == "differential")]


def suite(directory: Path, group: str) -> unittest.TestSuite:
    loader = unittest.TestLoader()
    return unittest.TestSuite(loader.discover(str(directory), pattern=path.name)
                              for path in test_files(directory, group))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("group", choices=("reference", "differential"))
    args = parser.parse_args()
    result = unittest.TextTestRunner(verbosity=1).run(suite(Path(__file__).parent, args.group))
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
