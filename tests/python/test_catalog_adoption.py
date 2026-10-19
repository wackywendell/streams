from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from dataclasses import replace
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams import catalog_config as config
from _streams.catalog_agent import source_id
from _streams.catalog.model import Session
from _streams.catalog.operations import Operation, update
from _streams.catalog.sync import GitCatalog, LocalCatalog
from _streams.catalog.sync.git_io import Git, GitError
from _streams.catalog.sync.history import read_history
from _streams.catalog.sync.state import RepairRequired, State, SyncMetadata
from _streams.cli import main
from _streams.errors import StreamError
from test_catalog_sync import create, effort


class CatalogAdoptionTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.home = Path(temporary.name)
        gitconfig = self.home / "gitconfig"
        gitconfig.write_text('[user]\n name = Adoption Test\n email = adoption@example.org\n[commit]\n gpgsign = false\n')
        self.enterContext(patch.dict(os.environ, {
            "HOME": str(self.home), "PATH": os.environ.get("PATH", os.defpath),
            "XDG_CONFIG_HOME": str(self.home / "config"), "XDG_DATA_HOME": str(self.home / "data"),
            "XDG_STATE_HOME": str(self.home / "state"), "GIT_CONFIG_GLOBAL": str(gitconfig),
            "GIT_CONFIG_NOSYSTEM": "1",
        }, clear=True))
        self.local = config.initialize("none")
        self.root, self.host = config.catalog_root(), self.local.host_id
        self.creation = create(effort(host=self.host))
        self.assertTrue(self.local.save(self.creation, origin={"command": "streams", "session": "native"}).accepted)
        session = Session(id="session", host_id=self.host, stream_id="work", harness="pi",
                          harness_session_id="saved-native-id", title="Existing conversation")
        self.join = Operation("join", None, session, "automatic", ownership_token="create")
        self.assertTrue(self.local.save(self.join).accepted)
        self.report = update("report", session, {"state": "Working"}, actor="automatic", ownership_token="create",
                             inputs=(self.creation.after,))
        self.assertTrue(self.local.save(self.report).accepted)
        self.rename = update("rename", self.creation.after, {"title": "Current title"})
        self.assertTrue(self.local.save(self.rename).accepted)
        self.park = update("park", self.rename.after, {"status": "parked"})
        self.assertTrue(self.local.save(self.park).accepted)
        self.restore = update("restore", self.park.after, {"status": "active"})
        self.assertTrue(self.local.save(self.restore).accepted)
        self.noop = update("noop", self.restore.after, {"title": "Current title"})
        self.assertTrue(self.local.save(self.noop).accepted)

    def bytes_under(self, root):
        return {str(path.relative_to(root)): path.read_bytes() for path in root.rglob("*") if path.is_file()}

    def cli(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with patch("sys.stdout", out), patch("sys.stderr", err):
            code = main(list(args))
        return code, out.getvalue(), err.getvalue()

    def no_network(self):
        original = Git.run
        def run(git, *args, **kwargs):
            if args[0] in {"ls-remote", "fetch", "push", "credential"}:
                raise AssertionError(f"adoption attempted network or credentials: {args}")
            return original(git, *args, **kwargs)
        return patch.object(Git, "run", run)

    def test_adoption_preserves_bytes_identity_authority_and_host_local_bindings(self):
        before = self.local.snapshot()
        record_bytes = {str(path.relative_to(self.root)): path.read_bytes()
                        for path in self.root.rglob("*.yaml")}
        private = {name: (self.root / ".catalog-sync" / name).read_bytes() for name in ("local.json", "receipts.json")}
        identity = (config.config_path().parent / "host.json").read_bytes()
        root_inode, lock_inode = self.root.stat().st_ino, (self.root / ".catalog.lock").stat().st_ino
        source = source_id(self.local)
        runtime = config.runtime_state_dir()
        runtime.mkdir(parents=True)
        (runtime / "binding-evidence.json").write_text('{"native":"saved-native-id","workspace":"existing-pane"}')
        checkouts = config.checkout_state_dir()
        checkouts.mkdir(parents=True)
        (checkouts / "checkout-evidence.json").write_text('{"checkout_id":"existing-checkout","root":"/saved/root"}')
        bindings, checkout_bytes = self.bytes_under(runtime), self.bytes_under(checkouts)
        with self.no_network():
            backend, revision = config.adopt_local_git()
            self.assertEqual(backend.snapshot(), before)
            self.assertEqual(backend.authority("work"), self.local._creations()["work"])
            self.assertEqual(source_id(backend), source)
            self.assertEqual(backend.read(refresh=True).catalog, before.catalog)
            self.assertEqual(backend.read().freshness, "local-only")
            self.assertEqual(backend.sync(event="shutdown").sharing, "local-only")
        self.assertEqual((self.root.stat().st_ino, (self.root / ".catalog.lock").stat().st_ino), (root_inode, lock_inode))
        self.assertEqual({str(path.relative_to(self.root)): path.read_bytes() for path in self.root.rglob("*.yaml")}, record_bytes)
        for name, contents in private.items():
            self.assertEqual((self.root / ".catalog-sync" / name).read_bytes(), contents)
        self.assertEqual((config.config_path().parent / "host.json").read_bytes(), identity)
        self.assertEqual(self.bytes_under(runtime), bindings)
        self.assertEqual(self.bytes_under(checkouts), checkout_bytes)
        history = read_history(backend.git, revision)
        self.assertEqual(history.operations, {})
        self.assertEqual(history.baseline.creations, json.loads(private["local.json"])["creations"])
        self.assertEqual(dict(history.fingerprints), json.loads(private["receipts.json"])["operations"])
        self.assertEqual(backend.git.text("remote"), "")
        self.assertEqual(backend.state.metadata(), SyncMetadata())
        self.assertFalse((backend.state.root / "sync.json").exists())
        with self.assertRaises(RepairRequired):
            self.local.snapshot()

    def test_retired_retries_clone_rehydration_and_normal_post_baseline_history(self):
        backend, baseline = config.adopt_local_git()
        for operation in (self.creation, self.join, self.report, self.rename, self.park, self.restore, self.noop):
            with self.subTest(operation=operation.id):
                result = backend.save(operation)
                self.assertTrue(result.accepted)
                self.assertEqual((result.domain.outcome, result.sharing), ("no-change", "local-only"))
                self.assertEqual(result.local_revision, baseline)
        self.assertEqual(backend.snapshot().catalog.stream("work").status, "active")
        changed = update("next", self.restore.after, {"goal": "After adoption"})
        saved = backend.save(changed)
        self.assertEqual((saved.local, saved.sharing), ("committed", "local-only"))
        history = read_history(backend.git, saved.local_revision)
        self.assertEqual(history.root, baseline)
        self.assertEqual(tuple(history.operations), ("next",))
        clone = self.home / "clone"
        subprocess.run(["git", "clone", "--no-hardlinks", str(self.root), str(clone)], check=True, capture_output=True)
        self.assertFalse((clone / ".catalog-sync").exists())
        pure = read_history(Git(clone, 10), Git(clone, 10).head())
        self.assertEqual(pure.fingerprints, history.fingerprints)
        reopened = GitCatalog(clone, host_id=self.host)
        reopened.git.run("remote", "remove", "origin")
        reopened.state.write_config(backend.state.config())
        self.assertEqual(reopened.snapshot().catalog, backend.snapshot().catalog)
        self.assertEqual(reopened.state.fingerprints(), dict(history.fingerprints))
        evidence = (reopened.state.root / "receipts.json").read_bytes()
        refused = reopened.save(replace(self.park, after=replace(self.park.after, goal="Changed intent")))
        self.assertEqual((refused.local, refused.sharing), ("repair-required", "local-only"))
        self.assertEqual((reopened.state.root / "receipts.json").read_bytes(), evidence)

    def test_cli_adoption_is_explicit_and_repeats_return_baseline_not_tip(self):
        with self.no_network():
            code, out, err = self.cli("init", "--sync", "git", "--adopt", "--json")
            self.assertEqual((code, err), (0, ""), out + err)
            result = json.loads(out)
            self.assertEqual(result["host_id"], self.host)
            baseline = result["baseline_revision"]
            backend = config.open_catalog()
            self.assertTrue(backend.save(update("later", self.restore.after, {"goal": "Later"})).accepted)
            self.assertNotEqual(backend.git.head(), baseline)
            code, out, err = self.cli("init", "--sync", "git", "--adopt", "--json")
            self.assertEqual((code, err), (0, ""), out + err)
            self.assertEqual(json.loads(out)["baseline_revision"], baseline)
            self.assertEqual(json.loads(self.cli("list", "--refresh", "--json")[1])["streams"][0]["id"], "work")
        with self.assertRaises(StreamError):
            config.adopt_local_git(branch="different")

    def test_invalid_arguments_and_missing_identity_never_convert_or_regenerate(self):
        evidence = self.bytes_under(self.root)
        for args in (("--sync", "none", "--adopt"), ("--sync", "git", "--adopt", "--remote", "dummy")):
            self.assertEqual(self.cli("init", *args, "--json")[0], 2)
            self.assertEqual(self.bytes_under(self.root), evidence)
        with self.assertRaises(StreamError):
            config.initialize("git")
        host = config.config_path().parent / "host.json"
        host.unlink()
        with self.assertRaises(StreamError):
            config.adopt_local_git()
        self.assertFalse(host.exists())
        self.assertEqual(self.bytes_under(self.root), evidence)

    def test_signing_failure_before_marker_preserves_usable_local_catalog(self):
        before = self.bytes_under(self.root)
        with patch.object(Git, "commit", side_effect=GitError("signer refused", code=1)):
            with self.assertRaisesRegex(StreamError, "signer refused"):
                config.adopt_local_git()
        self.assertEqual(self.bytes_under(self.root), before)
        self.assertEqual(config.open_catalog().snapshot(), self.local.snapshot())
        self.assertFalse((self.root / ".git").exists())
        self.assertFalse((self.root / ".catalog-sync/save.json").exists())
        self.assertEqual(list((self.root / ".catalog-staging").iterdir()), [])

    def test_final_path_policy_divergence_preserves_evidence_and_refuses_switch(self):
        gitconfig = Path(os.environ["GIT_CONFIG_GLOBAL"])
        scoped = self.home / "final-policy"
        scoped.write_text('[user]\n email = final-path@example.org\n')
        with gitconfig.open("a") as handle:
            handle.write(f'[includeIf "gitdir:{self.root.resolve()}/.git"]\n path = {scoped}\n')
        with self.assertRaisesRegex(StreamError, "signing policy differs"):
            config.adopt_local_git()
        self.assertEqual(State(self.root).read("cli.json")["strategy"], "none")
        self.assertIsNotNone(State(self.root).read("save.json"))
        self.assertTrue((self.root / ".git").is_dir())
        with self.assertRaisesRegex(StreamError, "interrupted save"):
            config.open_catalog()

    def test_invalid_creation_fingerprint_is_refused_before_git_or_marker(self):
        state = State(self.root)
        receipts = state.fingerprints()
        state.write("receipts.json", {"operations": {**receipts, "create": "0" * 64}})
        before = self.bytes_under(self.root)
        with patch.object(Git, "run", side_effect=AssertionError("invalid provenance reached Git")):
            with self.assertRaisesRegex(StreamError, "fingerprint"):
                config.adopt_local_git()
        self.assertEqual(self.bytes_under(self.root), before)

    def test_waiting_old_writer_is_fenced_by_the_same_lock_after_adoption(self):
        started = threading.Event()
        futures = []
        original_commit = Git.commit
        operation = update("waiting", self.restore.after, {"goal": "Stale backend write"})
        def write_old():
            started.set()
            return self.local.save(operation)
        with ThreadPoolExecutor(max_workers=1) as executor:
            def commit(git, *args, **kwargs):
                futures.append(executor.submit(write_old))
                self.assertTrue(started.wait(5))
                self.assertFalse(futures[0].done())
                return original_commit(git, *args, **kwargs)
            with patch.object(Git, "commit", commit):
                backend, _ = config.adopt_local_git()
            result = futures[0].result(timeout=5)
        self.assertEqual((result.local, result.sharing), ("repair-required", "local-only"))
        self.assertEqual(backend.snapshot().catalog.stream("work").goal, self.restore.after.goal)
        self.assertNotIn("waiting", backend.state.fingerprints())

    def test_failure_after_marker_unlink_restores_uncertainty_evidence_before_unlock(self):
        clear = State.clear
        def fail_after_clear(state, name):
            clear(state, name)
            if name == "save.json":
                raise OSError("injected marker fsync failure")
        with patch.object(State, "clear", fail_after_clear):
            with self.assertRaisesRegex(StreamError, "injected marker fsync failure"):
                config.adopt_local_git()
        evidence = State(self.root).read("save.json")
        self.assertEqual(evidence["completion_error"], "injected marker fsync failure")
        self.assertEqual(evidence["adoption"]["host"], self.host)
        with self.assertRaisesRegex(StreamError, "interrupted save"):
            config.open_catalog()

    def test_uninitialized_and_plain_git_catalogs_are_not_completed_adoptions(self):
        target = self.home / "uninitialized"
        config.set_config("catalog.root", str(target))
        host_file = config.config_path().parent / "host.json"
        host_bytes = host_file.read_bytes()
        with self.assertRaisesRegex(StreamError, "already initialized"):
            config.adopt_local_git()
        self.assertFalse(target.exists())
        self.assertEqual(host_file.read_bytes(), host_bytes)
        with self.no_network():
            backend = config.initialize("git")
            self.assertIsNone(backend.state.config().remote)
            self.assertIsNone(read_history(backend.git, backend.git.head()).baseline)
            with self.assertRaisesRegex(StreamError, "not a completed snapshot adoption"):
                config.adopt_local_git()
        self.assertFalse((target / ".catalog-sync/save.json").exists())

    def test_ancestor_scoped_git_identity_is_used_for_baseline_and_later_saves(self):
        scoped = self.home / "ancestor-policy"
        scoped.write_text('[user]\n name = Scoped Author\n email = scoped@example.org\n')
        with Path(os.environ["GIT_CONFIG_GLOBAL"]).open("a") as handle:
            handle.write(f'[includeIf "gitdir:{self.root.resolve()}/"]\n path = {scoped}\n')
        backend, baseline = config.adopt_local_git()
        self.assertEqual(backend.git.text("show", "-s", "--format=%an <%ae>", baseline), "Scoped Author <scoped@example.org>")
        saved = backend.save(update("scoped-save", self.restore.after, {"goal": "Next goal"}))
        self.assertTrue(saved.accepted)
        self.assertEqual(backend.git.text("show", "-s", "--format=%an <%ae>", saved.local_revision),
                         "Scoped Author <scoped@example.org>")

    def test_failures_after_fence_keep_marker_original_records_and_candidate(self):
        import _streams.catalog.sync.adoption as adoption
        for failure in ("install", "config", "strategy", "clear"):
            with self.subTest(failure=failure):
                # Each failure gets its own isolated copy and the same producer identity.
                target = self.home / ("fault-" + failure)
                shutil.copytree(self.root, target)
                config.set_config("catalog.root", str(target))
                original = self.bytes_under(target)
                state_write, state_clear, replace_file = State.write, State.clear, adoption.os.replace
                def write(state, name, data):
                    if name == "config.json" and failure == "config":
                        raise OSError("injected config failure")
                    if name == "cli.json" and failure == "strategy":
                        raise OSError("injected strategy failure")
                    return state_write(state, name, data)
                def move(source, destination):
                    if Path(destination) == target / ".git" and failure == "install":
                        raise OSError("injected install failure")
                    return replace_file(source, destination)
                def clear(state, name):
                    if name == "save.json" and failure == "clear":
                        raise OSError("injected clear failure")
                    return state_clear(state, name)
                with patch.object(State, "write", write), patch.object(State, "clear", clear), patch.object(adoption.os, "replace", move):
                    with self.assertRaisesRegex(StreamError, "injected"):
                        config.adopt_local_git()
                self.assertIsNotNone(State(target).read("save.json"))
                for path, contents in original.items():
                    if path != ".catalog-sync/cli.json":
                        self.assertEqual((target / path).read_bytes(), contents)
                with self.assertRaisesRegex(StreamError, "interrupted save"):
                    config.open_catalog()
                if failure == "install":
                    self.assertTrue(list((target / ".catalog-staging").glob("adoption-*/.git")))


if __name__ == "__main__":
    unittest.main()
