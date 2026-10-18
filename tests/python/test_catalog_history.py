from __future__ import annotations

from dataclasses import FrozenInstanceError, replace
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog.codec import record_to_data
from _streams.catalog.model import Catalog, Checkout, Repository, record_key
from _streams.catalog.operations import Authority, update
from _streams.catalog.storage import StorageError
from _streams.catalog.sync.envelope import BOOTSTRAP, decode_operation, encode_operation
from _streams.catalog.sync.git_io import Git
from _streams.catalog.sync.history import History, read_history
from _streams.catalog.sync.local import RepairRequired
from _streams.catalog.sync.replay import replay
from _streams.catalog.yaml_store import YamlStore
from test_catalog_sync import contribution, create, effort


def encoded(record):
    return yaml.safe_dump({"version": 1, "revision": "test-revision", "kind": record_key(record).kind,
                           "record": record_to_data(record)}).encode()


class CatalogHistoryTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        subprocess.run(["git", "init", str(self.root)], capture_output=True, check=True)
        self.git = Git(self.root, timeout=10)
        for key, value in (("user.name", "Catalog Test"), ("user.email", "catalog@example.org"),
                           ("commit.gpgsign", "false")):
            self.git.run("config", key, value)
        self.blobs = {}
        self.bootstrap = self.commit({}, BOOTSTRAP)
        self.tip, self.files = self.bootstrap, {}

    def commit(self, files, message, parent=None, *, modes=None, parents=None):
        index = self.root / "test-index"
        self.git.run("read-tree", "--empty", index=index)
        entries = []
        for path, contents in files.items():
            if contents not in self.blobs:
                self.blobs[contents] = self.git.text("hash-object", "-w", "--stdin", input=contents)
            mode = (modes or {}).get(path, "100644")
            entries.append(f"{mode} {self.blobs[contents]}\t{path}\n")
        self.git.run("update-index", "--index-info", input="".join(entries).encode(), index=index)
        tree = self.git.text("write-tree", index=index)
        if parents is not None:
            args = ["commit-tree", tree]
            for revision in parents:
                args.extend(("-p", revision))
            return self.git.text(*args, input=message.encode())
        return self.git.commit(tree, message, parent)

    def append(self, operation, *, origin=None):
        self.files = {**self.files, YamlStore.record_path(operation.target).as_posix(): encoded(operation.after)}
        self.tip = self.commit(self.files, encode_operation(operation, origin=origin), self.tip)
        return self.tip

    def chain(self, count):
        return [self.append(create(effort(f"work-{i}"), id=f"create-{i}")) for i in range(count)]

    def test_cold_chain_decodes_only_one_record_per_operation(self):
        tips = self.chain(16)
        with patch.object(YamlStore, "decode_record", wraps=YamlStore.decode_record) as decode, \
                patch.object(YamlStore, "decode_snapshot", side_effect=AssertionError("whole tree decoded")):
            history = read_history(self.git, tips[-1])
        self.assertEqual(decode.call_count, 16)
        self.assertEqual(len(history.catalog.streams), 16)
        self.assertEqual(list(self.git.histories), [tips[-1]])

    def test_confirmed_and_head_tips_survive_a_long_backlog(self):
        tips = self.chain(24)
        with patch.object(YamlStore, "decode_record", wraps=YamlStore.decode_record) as decode:
            confirmed = read_history(self.git, tips[11])
            self.assertEqual(decode.call_count, 12)
            head = read_history(self.git, tips[-1])
            self.assertEqual(decode.call_count, 24)
            with patch.object(self.git, "run", side_effect=AssertionError("cached tip rebuilt")):
                self.assertIs(read_history(self.git, tips[-1]), head)
                self.assertIs(read_history(self.git, tips[11]), confirmed)
                self.assertIs(read_history(self.git, tips[-1]), head)
        self.assertEqual(list(self.git.histories), [tips[11], tips[-1]])
        self.assertEqual(len(confirmed.operations), 12)
        self.assertEqual(len(head.operations), 24)

    def test_head_then_newly_requested_confirmed_does_not_evict_head(self):
        tips = self.chain(24)
        head = read_history(self.git, tips[-1])
        confirmed = read_history(self.git, tips[11])
        with patch.object(self.git, "run", side_effect=AssertionError("cached tip rebuilt")):
            self.assertIs(read_history(self.git, tips[-1]), head)
            self.assertIs(read_history(self.git, tips[11]), confirmed)
        self.assertEqual(set(self.git.histories), {tips[-1], tips[11]})

    def test_distinct_requested_tips_keep_fifo_cache_bounded(self):
        tips = self.chain(12)
        for tip in tips:
            read_history(self.git, tip)
            self.assertLessEqual(len(self.git.histories), 8)
        self.assertEqual(list(self.git.histories), tips[-8:])
        with patch.object(YamlStore, "decode_record", wraps=YamlStore.decode_record) as decode:
            read_history(self.git, tips[0])
        self.assertEqual(decode.call_count, 1)
        self.assertEqual(list(self.git.histories), tips[-7:] + [tips[0]])

    def test_history_owns_read_only_copies_including_origins(self):
        operation = create()
        origin = {"session": "responsible"}
        files = {"streams/work.yaml": encoded(operation.after)}
        authorities = {"work": Authority("laptop", "create")}
        operations = {operation.id: (operation, origin)}
        history = History(files=files, authorities=authorities, operations=operations)
        files.clear()
        authorities.clear()
        operations.clear()
        origin["session"] = "changed"
        self.assertIn("streams/work.yaml", history.files)
        self.assertEqual(history.authorities["work"], Authority("laptop", "create"))
        self.assertEqual(history.operations["create"][1], {"session": "responsible"})
        for name in history.__dataclass_fields__:
            with self.subTest(attribute=name), self.assertRaises(FrozenInstanceError):
                setattr(history, name, None)
        for mapping in (history.files, history.authorities, history.operations, history.operations["create"][1]):
            with self.subTest(mapping=mapping), self.assertRaises(TypeError):
                mapping["injected"] = "bad"

    def test_cached_origins_are_immutable_and_replay_can_encode_them(self):
        tip = self.append(create(), origin={"session": "responsible"})
        history = read_history(self.git, tip)
        operation, origin = history.operations["create"]
        for mapping in (history.files, history.authorities, history.operations, origin):
            with self.subTest(mapping=mapping), self.assertRaises(TypeError):
                mapping["injected"] = "changed"
        projected = replay(history, History())
        replayed, replay_origin, record = projected.changes[0]
        self.assertEqual(decode_operation(encode_operation(replayed, origin=replay_origin)),
                         (operation, {"session": "responsible"}))
        self.assertEqual(record, operation.after)
        self.assertIs(read_history(self.git, tip), history)

    def test_changed_paths_are_checked_before_decoding(self):
        first = self.append(create())
        read_history(self.git, first)
        operation = update("rename", effort(), {"title": "Changed"})
        path = "streams/work.yaml"
        invalid_files = (
            {path: encoded(operation.after), "unexpected.txt": b"not YAML"},
            {},
            {"streams/wrong.yaml": encoded(operation.after)},
            {path: self.files[path]},
        )
        for files in invalid_files:
            with self.subTest(files=files):
                tip = self.commit(files, encode_operation(operation), first)
                with patch.object(YamlStore, "decode_record", side_effect=AssertionError("decoded before path check")):
                    with self.assertRaises(RepairRequired):
                        read_history(self.git, tip)
                self.assertNotIn(tip, self.git.histories)

    def test_changed_record_identity_and_materialized_values_are_validated(self):
        operation = create()
        for record in (effort("another-id"), replace(effort(), title="Unexplained title")):
            with self.subTest(record=record):
                tip = self.commit({"streams/work.yaml": encoded(record)}, encode_operation(operation), self.bootstrap)
                with self.assertRaises(RepairRequired):
                    read_history(self.git, tip)
                self.assertNotIn(tip, self.git.histories)

    def test_decode_record_owns_canonical_paths_and_yaml_validation(self):
        record = effort("Work/É")
        path = YamlStore.record_path(record_key(record)).as_posix()
        self.assertEqual(YamlStore.decode_record(path, encoded(record)), record)
        for wrong in ("streams/Work/É.yaml", "streams/work.yaml", "hosts/" + path.split("/")[-1]):
            with self.subTest(path=wrong), self.assertRaises(StorageError):
                YamlStore.decode_record(wrong, encoded(record))
        for contents in (b"[]", b"version: 2\n", encoded(record) + b"version: 1\n"):
            with self.subTest(contents=contents), self.assertRaises(StorageError):
                YamlStore.decode_record(path, contents)

    def test_deleting_an_unchanged_record_is_rejected(self):
        self.append(create())
        parent = self.append(create(effort("other"), id="other"))
        read_history(self.git, parent)
        operation = update("rename", effort(), {"title": "Changed"})
        tip = self.commit({"streams/work.yaml": encoded(operation.after)}, encode_operation(operation), parent)
        with self.assertRaises(RepairRequired):
            read_history(self.git, tip)

    def test_symlinked_content_is_rejected_even_at_the_changed_path(self):
        operation = create()
        tip = self.commit({"streams/work.yaml": b"outside.yaml"}, encode_operation(operation), self.bootstrap,
                          modes={"streams/work.yaml": "120000"})
        with self.assertRaisesRegex(RepairRequired, "non-regular"):
            read_history(self.git, tip)

    def test_reconstructed_catalog_validates_unchanged_session_relationships(self):
        stream = replace(effort(), repositories={"code": Repository("https://example.org/code")})
        self.append(create(stream))
        session = replace(contribution().after, checkout=Checkout("code", "main"))
        parent = self.append(replace(contribution(), after=session))
        read_history(self.git, parent)
        operation = update("rename", stream, {"title": "Changed"})
        files = {**self.files, "streams/work.yaml": encoded(replace(operation.after, repositories={}))}
        tip = self.commit(files, encode_operation(operation), parent)
        with self.assertRaisesRegex(RepairRequired, "unknown checkout repository"):
            read_history(self.git, tip)

    def test_empty_bootstrap_and_linear_history_are_required(self):
        for files, message in (({"unexpected.txt": b"content"}, BOOTSTRAP), ({}, "not a bootstrap\n")):
            with self.subTest(files=files, message=message):
                with self.assertRaises(RepairRequired):
                    read_history(self.git, self.commit(files, message))
        with self.assertRaises(RepairRequired):
            read_history(self.git, self.commit({}, BOOTSTRAP, self.bootstrap))
        first = self.append(create())
        branch = self.commit({}, BOOTSTRAP + "other\n", self.bootstrap)
        merge = self.commit(self.files, encode_operation(create()), parents=(first, branch))
        with self.assertRaises(RepairRequired):
            read_history(self.git, merge)
        self.assertEqual(read_history(self.git, self.bootstrap).catalog, Catalog())
        self.assertEqual(read_history(self.git, None), History())

    def test_duplicate_ids_stale_guards_and_invalid_authority_are_rejected(self):
        first = self.append(create())
        read_history(self.git, first)
        operations = (
            update("create", effort(), {"title": "Duplicate ID"}),
            replace(contribution(), ownership_token="not-the-creation"),
            update("guarded", effort(), {"title": "Guarded"}, actor="automatic", ownership_token="create",
                   inputs=(replace(contribution().after, state="Missing input"),)),
            create(id="redundant-create"),
        )
        for operation in operations:
            with self.subTest(operation=operation.id):
                files = {**self.files, YamlStore.record_path(operation.target).as_posix(): encoded(operation.after)}
                tip = self.commit(files, encode_operation(operation), first)
                with self.assertRaises(RepairRequired):
                    read_history(self.git, tip)
                self.assertNotIn(tip, self.git.histories)


if __name__ == "__main__":
    unittest.main()
