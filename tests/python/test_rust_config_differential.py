"""Physical source IDs must use the exact Python JSON hash preimage."""

from __future__ import annotations

from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "src"))

from _streams.catalog_agent import source_id


class RustConfigDifferentialTest(unittest.TestCase):
    def test_physical_source_identity_including_ascii_escaping(self):
        probe = ROOT / "target/debug/examples/config_source_id"
        self.assertTrue(probe.is_file(), "build Rust examples before this test")
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            for name in ("ascii", "caf\u00e9-\U0001f680-\x7f", 'quotes"-backslash\\-newline\n'):
                root = home / name
                root.mkdir()
                alias = home / "alias"
                alias.symlink_to(root, target_is_directory=True)
                try:
                    for path in (root, alias, alias / ".." / "alias"):
                        host = "01234567-89ab-4def-8123-456789abcdef"
                        backend = SimpleNamespace(store=SimpleNamespace(root=path), host_id=host)
                        result = subprocess.run([str(probe), str(path), host], check=True,
                                                capture_output=True, text=True, timeout=15)
                        self.assertEqual(result.stdout.strip(), source_id(backend))
                finally:
                    alias.unlink()


if __name__ == "__main__":
    unittest.main()
