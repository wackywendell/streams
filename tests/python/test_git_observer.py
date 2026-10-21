"""Batching preserves exact object bytes and the observer caches only Git OIDs."""
import unittest
from unittest.mock import patch

from test_rust_git_differential import Observations


class GitObserverTests(unittest.TestCase):
    def test_batch_preserves_newlines_binary_bytes_and_unchanged_blobs(self):
        first, second = b"a" * 40, b"b" * 40
        contents = {first: b"\nfirst\x00body\n", second: "Unicode 会話\n\n".encode()}
        tree = b"".join(b"100644 blob " + oid + b"\t" + name + b"\0"
                        for oid, name in [(first, b"a.yaml"), (second, b"b.yaml")])
        batch = b"".join(oid + b" blob " + str(len(value)).encode() + b"\n" + value + b"\n"
                         for oid, value in contents.items())
        observer = Observations()
        with patch("test_rust_git_differential.git_bytes", side_effect=[tree, batch, tree]) as command:
            expected = {"a.yaml": contents[first], "b.yaml": contents[second]}
            self.assertEqual(observer.files("repo", "tip"), expected)
            self.assertEqual(observer.files("repo", "another-tip"), expected)
            self.assertEqual(command.call_count, 3)
            self.assertEqual(command.call_args_list[1].kwargs["input"], first + b"\n" + second + b"\n")

    def test_truncated_or_contradictory_batch_is_not_observed_as_a_valid_file(self):
        oid = b"a" * 40
        tree = b"100644 blob " + oid + b"\ta.yaml\0"
        for batch in [oid + b" blob 9\nx\n", b"b" * 40 + b" blob 1\nx\n", oid + b" tree 1\nx\n", oid + b" blob 1\nx\ntrailing"]:
            with self.subTest(batch=batch), patch("test_rust_git_differential.git_bytes", side_effect=[tree, batch]):
                with self.assertRaises(AssertionError):
                    Observations().files("repo", "tip")


if __name__ == "__main__":
    unittest.main()
