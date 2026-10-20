from __future__ import annotations

from dataclasses import FrozenInstanceError
import copy
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog_checkouts import CheckoutRegistry, inspect_directory
from _streams.errors import StreamError


class CheckoutRegistryTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.home = Path(temporary.name).resolve()
        self.state_dir = self.home / "state"
        self.registry = CheckoutRegistry(self.state_dir, "test-host")
        self.environment = patch.dict(os.environ, {"GIT_CONFIG_GLOBAL": os.devnull,
                                                   "GIT_CONFIG_NOSYSTEM": "1",
                                                   "JJ_CONFIG": os.devnull})
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.root = self.repo("first")

    def command(self, *args, cwd=None):
        return subprocess.run(args, cwd=cwd, check=True, capture_output=True, text=True, timeout=30).stdout

    def repo(self, name):
        root = self.home / name
        self.command("git", "init", "--quiet", str(root))
        return root

    def snapshot(self, path):
        return {str(item.relative_to(path)): (item.stat().st_mtime_ns, item.read_bytes())
                for item in path.rglob("*") if item.is_file()}

    def test_same_path_and_symlink_alias_reuse_identity(self):
        first = self.registry.register(self.root)
        self.assertEqual(self.registry.register(self.root), first)
        alias = self.home / "alias"
        alias.symlink_to(self.root, target_is_directory=True)
        self.assertEqual(self.registry.register(alias), first)
        self.assertEqual(self.registry.find(alias), first)
        self.assertEqual(self.registry.get(first.id), first)
        self.assertEqual(first.directory, str(self.root))
        self.assertEqual(self.registry.list(), (first,))
        with self.assertRaises(FrozenInstanceError):
            first.id = "changed"

    def test_root_path_spaces_are_not_stripped_from_inspection_output(self):
        root = self.repo("trailing space ")
        self.assertEqual(inspect_directory(root), (root, "."))
        self.assertEqual(self.registry.register(root).directory, str(root))

    def test_case_alias_reuses_identity_on_case_insensitive_filesystem(self):
        alias = self.root.with_name(self.root.name.upper())
        if not alias.exists():
            self.skipTest("case-sensitive filesystem")
        first = self.registry.register(self.root)
        self.assertEqual(self.registry.register(alias), first)

    def test_case_alias_ignores_broken_and_resolving_symlink_siblings(self):
        alias = self.root.with_name(self.root.name.upper())
        if not alias.exists():
            self.skipTest("case-sensitive filesystem")
        broken = self.home / "broken"
        broken.symlink_to(self.home / "absent")
        link = self.home / "link"
        link.symlink_to(self.root, target_is_directory=True)
        iterdir = Path.iterdir
        def entries(path):
            return iter((broken, link, self.root)) if path == self.home else iterdir(path)
        with patch.object(Path, "iterdir", entries):
            self.assertEqual(self.registry.register(alias).directory, str(self.root))

    def test_unbound_selected_checkout_cannot_silently_use_a_default(self):
        selected = self.registry.register(self.root)
        default = self.registry.register(self.repo("default"), repository_url="repo")
        self.registry.set_default("repo", default.id)
        with self.assertRaisesRegex(StreamError, "explicit repository URL binding"):
            self.registry.resolve("repo", selected_checkout_id=selected.id)

    def test_bindings_are_separate_explicit_exact_locator_calls(self):
        self.command("git", "remote", "add", "origin", "https://fork/repo", cwd=self.root)
        self.command("git", "remote", "add", "upstream", "https://upstream/repo", cwd=self.root)
        checkout = self.registry.register(self.root)
        self.assertEqual(checkout.repository_urls, ())
        fork = self.registry.register(self.root, repository_url="https://fork/repo")
        both = self.registry.register(self.root, repository_url="https://upstream/repo")
        self.assertEqual(both.id, checkout.id)
        self.assertEqual(fork.repository_urls, ("https://fork/repo",))
        self.assertEqual(both.repository_urls, ("https://fork/repo", "https://upstream/repo"))
        self.assertEqual(self.registry.register(self.root, repository_url="https://fork/repo"), both)
        for url in both.repository_urls:
            with self.assertRaisesRegex(StreamError, "no explicit checkout default"):
                self.registry.resolve(url)
        with self.assertRaises(StreamError):
            self.registry.set_default("https://fork/repo.git", both.id)
        self.registry.set_default("https://fork/repo", both.id)
        self.assertEqual(self.registry.resolve("https://fork/repo"), both)
        with self.assertRaises(StreamError):
            self.registry.resolve("https://upstream/repo")

    def test_selected_precedence_and_external_repository_default(self):
        selected = self.registry.register(self.root, repository_url="repo")
        fallback_root = self.repo("fallback")
        fallback = self.registry.register(fallback_root, repository_url="repo")
        external = self.registry.register(self.repo("external"), repository_url="external")
        self.registry.set_default("repo", fallback.id)
        self.registry.set_default("external", external.id)
        kwargs = {"selected_checkout_id": selected.id, "selected_repository_url": "repo"}
        self.assertEqual(self.registry.resolve("repo", **kwargs), selected)
        self.assertEqual(self.registry.resolve("repo", selected_checkout_id=selected.id), selected)
        self.assertEqual(self.registry.resolve("external", **kwargs), external)
        with self.assertRaises(StreamError):
            self.registry.resolve("external", selected_checkout_id=selected.id, selected_repository_url="external")
        self.assertEqual(self.registry.resolve("repo"), fallback)
        with self.assertRaises(StreamError):
            self.registry.resolve("repo", selected_repository_url="repo")

    def test_missing_selected_root_never_uses_default(self):
        selected = self.registry.register(self.root, repository_url="repo")
        fallback = self.registry.register(self.repo("fallback"), repository_url="repo")
        self.registry.set_default("repo", fallback.id)
        self.root.rename(self.home / "moved")
        self.assertEqual(self.registry.list(), (selected, fallback))
        for kwargs in ({"selected_checkout_id": selected.id},
                       {"selected_checkout_id": selected.id, "selected_repository_url": "repo"}):
            with self.subTest(kwargs=kwargs), self.assertRaises(StreamError):
                self.registry.resolve("repo", **kwargs)
        self.assertEqual(self.registry.resolve("repo"), fallback)
        with self.assertRaises(StreamError):
            self.registry.get(selected.id)

    def test_document_reads_current_worktree_and_does_not_fallback(self):
        selected = self.registry.register(self.root, repository_url="repo")
        fallback_root = self.repo("fallback")
        fallback = self.registry.register(fallback_root, repository_url="repo")
        self.registry.set_default("repo", fallback.id)
        (fallback_root / "doc.md").write_text("fallback")
        kwargs = {"selected_checkout_id": selected.id, "selected_repository_url": "repo"}
        with self.assertRaises(StreamError):
            self.registry.document("repo", "doc.md", **kwargs)
        (self.root / "doc.md").write_text("uncommitted current contents")
        checkout, path = self.registry.document("repo", "doc.md", **kwargs)
        self.assertEqual(checkout, selected)
        self.assertEqual(path.read_text(), "uncommitted current contents")
        (self.root / "doc.md").write_text("edited again")
        self.assertEqual(self.registry.document("repo", "doc.md", **kwargs)[1].read_text(), "edited again")

    def test_revision_document_returns_nested_cache_root_and_reuses_immutable_content(self):
        checkout = self.registry.register(self.root)
        cache = self.home / "revisions"
        revision = "a" * 40
        expected_root = cache / checkout.id / revision[:12]
        with patch("_streams.catalog_checkouts._run", return_value="historical") as run:
            target = self.registry.revision_document(checkout.id, "docs/deep/design.md", revision, cache_dir=cache)
            self.assertEqual(target, {"checkout_id": checkout.id, "root": str(expected_root),
                                      "file": str(expected_root / "docs/deep/design.md"), "revision": revision})
            self.assertEqual(Path(target["file"]).read_text(), "historical\n")
            self.assertEqual(self.registry.revision_document(checkout.id, "docs/deep/design.md", revision,
                                                            cache_dir=cache), target)
            run.assert_called_once_with(["git", "show", f"{revision}:docs/deep/design.md"], self.root)
            self.root.rename(self.home / "moved")
            with self.assertRaises(StreamError):
                self.registry.revision_document(checkout.id, "docs/deep/design.md", revision, cache_dir=cache)
            self.assertEqual(run.call_count, 1)

    def test_revision_document_resolves_existing_refs_and_keeps_revision_label(self):
        checkout = self.registry.register(self.root)
        revision = "feature/design"
        cache = self.home / "revisions"
        for commit in ("a" * 40, "b" * 40):
            with self.subTest(commit=commit), patch("_streams.catalog_checkouts._run",
                                                   side_effect=[commit, "historical"]) as run:
                target = self.registry.revision_document(checkout.id, "doc.md", revision, cache_dir=cache)
                self.assertEqual(target["root"], str(cache / checkout.id / commit[:12]))
                self.assertEqual(target["revision"], revision)
                self.assertEqual(run.call_args_list[0].args,
                                 (["git", "rev-parse", "--verify", f"{revision}^{{commit}}"], self.root))
                self.assertEqual(run.call_args_list[1].args,
                                 (["git", "show", f"{commit}:doc.md"], self.root))
        with patch("_streams.catalog_checkouts._run", return_value="b" * 40) as run:
            self.registry.revision_document(checkout.id, "doc.md", revision, cache_dir=cache)
            self.assertEqual(run.call_count, 1)

    def test_revision_document_negative_cache_only_for_immutable_or_resolved_revision(self):
        checkout = self.registry.register(self.root)
        cache = self.home / "revisions"
        for revision, resolved in (("a" * 40, None), ("feature", "b" * 40), ("unknown", None)):
            def unavailable(args, cwd):
                if args[1] == "rev-parse" and resolved is not None:
                    return resolved
                raise StreamError("unavailable")
            with self.subTest(revision=revision), patch("_streams.catalog_checkouts._run",
                                                       side_effect=unavailable) as run:
                for _ in range(2):
                    with self.assertRaisesRegex(StreamError, "document revision is unavailable"):
                        self.registry.revision_document(checkout.id, "docs/missing.md", revision, cache_dir=cache)
                self.assertEqual(run.call_count, 1 if revision == "a" * 40 else 3 if resolved else 4)
        self.assertEqual(len(list(cache.rglob("*.missing"))), 2)

    def test_revision_document_rejects_unsafe_paths_and_revisions_before_vcs(self):
        checkout = self.registry.register(self.root)
        cache = self.home / "revisions"
        with patch("_streams.catalog_checkouts._run", side_effect=AssertionError("must not invoke VCS")):
            for path in ("../escape", "/absolute", "docs/../doc.md", "docs\\doc.md", "."):
                with self.subTest(path=path), self.assertRaises(StreamError):
                    self.registry.revision_document(checkout.id, path, "a" * 40, cache_dir=cache)
            for revision in ("", " ", "-option", " -option", "bad\nrevision", None):
                with self.subTest(revision=revision), self.assertRaises(StreamError):
                    self.registry.revision_document(checkout.id, "doc.md", revision, cache_dir=cache)
        self.assertFalse(cache.exists())

    def test_revision_document_rejects_symlinked_cache_entries(self):
        checkout = self.registry.register(self.root)
        revision = "a" * 40
        outside = self.home / "outside"
        outside.mkdir()
        (outside / "doc.md").write_text("outside")
        for entry in ("root", "directory", "file", "missing"):
            cache = self.home / f"cache-{entry}"
            root = cache / checkout.id / revision[:12]
            root.parent.mkdir(parents=True)
            if entry == "root":
                root.symlink_to(outside, target_is_directory=True)
            else:
                root.mkdir()
                if entry == "directory":
                    (root / "docs").symlink_to(outside, target_is_directory=True)
                else:
                    (root / "docs").mkdir()
                    name = "doc.md" if entry == "file" else ".doc.md.missing"
                    (root / "docs" / name).symlink_to(outside / "doc.md")
            with self.subTest(entry=entry), patch("_streams.catalog_checkouts._run",
                                                  side_effect=AssertionError("must not invoke VCS")):
                with self.assertRaises(StreamError):
                    self.registry.revision_document(checkout.id, "docs/doc.md", revision, cache_dir=cache)
        self.assertEqual((outside / "doc.md").read_text(), "outside")

    def test_revision_document_extraction_has_subprocess_timeout(self):
        checkout = self.registry.register(self.root)
        with patch("_streams.catalog_checkouts.subprocess.run",
                   side_effect=subprocess.TimeoutExpired("git", 15)) as run:
            with self.assertRaisesRegex(StreamError, "document revision is unavailable"):
                self.registry.revision_document(checkout.id, "doc.md", "a" * 40,
                                                cache_dir=self.home / "revisions")
            self.assertEqual(run.call_args.kwargs["timeout"], 15)

    def test_cwd_subdir_and_nested_repositories(self):
        subdir = self.root / "src/deep"
        subdir.mkdir(parents=True)
        self.assertEqual(inspect_directory(subdir), (self.root, "src/deep"))
        self.assertEqual(inspect_directory(self.root), (self.root, "."))
        self.assertIsNone(inspect_directory(self.home))
        with self.assertRaises(StreamError):
            inspect_directory(self.home / "missing")
        with self.assertRaises(StreamError):
            self.registry.register(subdir)
        nested = self.repo("first/nested")
        self.assertEqual(inspect_directory(nested), (nested, "."))
        a, b = self.registry.register(self.root), self.registry.register(nested)
        self.assertNotEqual(a.id, b.id)
        self.assertEqual(self.registry.directory(a.id, "src/deep"), subdir)

    def test_malformed_nested_marker_is_not_skipped(self):
        nested = self.root / "nested"
        nested.mkdir()
        for marker, text in ((".git", "not a gitdir pointer"), (".jj", "invalid")):
            path = nested / marker
            path.write_text(text)
            with self.subTest(marker=marker), self.assertRaises(StreamError):
                inspect_directory(nested)
            path.unlink()
        (nested / ".git").mkdir()
        with self.assertRaises(StreamError):
            inspect_directory(nested)

    def test_inspection_is_bounded_and_ignores_git_routing(self):
        other = self.repo("other")
        with patch.dict(os.environ, {"GIT_DIR": str(other / ".git"), "GIT_WORK_TREE": str(other)}):
            self.assertEqual(inspect_directory(self.root), (self.root, "."))
        with patch("_streams.catalog_checkouts.subprocess.run", side_effect=subprocess.TimeoutExpired("git", 15)):
            with self.assertRaises(StreamError):
                inspect_directory(self.root)

    def test_git_worktrees_are_distinct_not_common_storage(self):
        self.command("git", "-c", "user.name=Test", "-c", "user.email=test@example.org",
                     "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-m", "initial", cwd=self.root)
        second = self.home / "worktree"
        self.command("git", "worktree", "add", "--detach", str(second), cwd=self.root)
        self.assertTrue((second / ".git").is_file())
        first = self.registry.register(self.root, repository_url="repo")
        other = self.registry.register(second, repository_url="repo")
        self.assertNotEqual(first.id, other.id)
        self.assertEqual(inspect_directory(second), (second, "."))
        self.assertEqual(self.registry.get(other.id).directory, str(second))
        common = self.root / ".git"
        with self.assertRaises(StreamError):
            self.registry.register(common)

    @unittest.skipUnless(shutil.which("jj"), "jj is not available")
    def test_standalone_jj_workspaces_and_no_snapshot(self):
        root = self.home / "jj-main"
        self.command("jj", "git", "init", "--no-colocate", str(root))
        second = self.home / "jj-workspace"
        self.command("jj", "workspace", "add", str(second), cwd=root)
        self.assertFalse((root / ".git").exists())
        a, b = self.registry.register(root), self.registry.register(second)
        self.assertNotEqual(a.id, b.id)
        (second / "untracked").write_text("must not snapshot")
        before = self.snapshot(root / ".jj"), self.snapshot(second / ".jj")
        self.assertEqual(inspect_directory(second), (second, "."))
        self.assertEqual(self.registry.get(b.id), b)
        self.assertEqual(before, (self.snapshot(root / ".jj"), self.snapshot(second / ".jj")))

    def test_replaced_root_fails_without_default_fallback(self):
        checkout = self.registry.register(self.root, repository_url="repo")
        self.registry.set_default("repo", checkout.id)
        self.root.rename(self.home / "old")
        self.repo("first")
        self.assertEqual(self.registry.list(), (checkout,))
        for operation in (lambda: self.registry.get(checkout.id), lambda: self.registry.find(self.root),
                          lambda: self.registry.register(self.root), lambda: self.registry.resolve("repo"),
                          lambda: self.registry.set_default("repo", checkout.id)):
            with self.assertRaisesRegex(StreamError, "replaced"):
                operation()

    def test_replaced_selected_root_never_uses_healthy_default(self):
        selected = self.registry.register(self.root, repository_url="repo")
        fallback = self.registry.register(self.repo("fallback"), repository_url="repo")
        self.registry.set_default("repo", fallback.id)
        self.root.rename(self.home / "old")
        self.repo("first")
        with self.assertRaisesRegex(StreamError, "replaced"):
            self.registry.resolve("repo", selected_checkout_id=selected.id, selected_repository_url="repo")
        self.assertEqual(self.registry.resolve("repo"), fallback)

    def test_moving_registration_is_explicit_preserves_id_bindings_and_default(self):
        checkout = self.registry.register(self.root, repository_url="repo")
        self.registry.set_default("repo", checkout.id)
        destination = self.home / "moved"
        self.root.rename(destination)
        with self.assertRaises(StreamError):
            self.registry.register(destination)
        moved = self.registry.move(checkout.id, destination)
        self.assertEqual(moved.id, checkout.id)
        self.assertEqual(moved.repository_urls, checkout.repository_urls)
        self.assertEqual(self.registry.resolve("repo"), moved)
        self.assertEqual(self.registry.get(checkout.id).directory, str(destination))
        self.assertFalse(self.root.exists())
        other = self.registry.register(self.repo("other"))
        with self.assertRaises(StreamError):
            self.registry.move(checkout.id, Path(other.directory))

    def test_reassociation_to_new_root_is_explicit_and_does_not_move_contents(self):
        checkout = self.registry.register(self.root, repository_url="repo")
        new_root = self.repo("new")
        moved = self.registry.move(checkout.id, new_root)
        self.assertEqual(moved.id, checkout.id)
        self.assertEqual(moved.directory, str(new_root))
        self.assertTrue(self.root.is_dir())
        self.assertTrue(new_root.is_dir())

    def test_ambiguous_bindings_require_explicit_default(self):
        a = self.registry.register(self.root, repository_url="repo")
        b = self.registry.register(self.repo("second"), repository_url="repo")
        with self.assertRaises(StreamError) as raised:
            self.registry.resolve("repo")
        self.assertEqual(raised.exception.kind, "ambiguous")
        self.assertEqual(raised.exception.candidates, [a.id, b.id])
        self.registry.set_default("repo", a.id)
        self.assertEqual(self.registry.resolve("repo"), a)

    def test_registry_is_host_scoped_and_shared_by_instances(self):
        checkout = self.registry.register(self.root)
        same = CheckoutRegistry(self.state_dir, "test-host")
        other = CheckoutRegistry(self.state_dir, "another-host")
        self.assertEqual(same.get(checkout.id), checkout)
        self.assertEqual(other.list(), ())
        self.assertNotEqual(self.registry.path, other.path)

    def test_reads_create_nothing_and_do_not_rewrite_existing_state(self):
        self.assertEqual(self.registry.list(), ())
        self.assertIsNone(self.registry.find(self.root))
        for operation in (lambda: self.registry.get("missing"), lambda: self.registry.resolve("repo"),
                          lambda: self.registry.directory("missing"),
                          lambda: self.registry.document("repo", "file.md")):
            with self.assertRaises(StreamError):
                operation()
        self.assertFalse(self.state_dir.exists())
        checkout = self.registry.register(self.root, repository_url="repo")
        self.registry.set_default("repo", checkout.id)
        (self.root / "file.md").write_text("current")
        before = self.snapshot(self.state_dir)
        self.registry.list()
        self.registry.get(checkout.id)
        self.registry.find(self.root)
        self.registry.resolve("repo")
        self.registry.directory(checkout.id)
        self.registry.document("repo", "file.md")
        self.assertEqual(self.snapshot(self.state_dir), before)

    def test_strict_corrupt_state_is_rejected_not_repaired(self):
        self.registry.register(self.root, repository_url="repo")
        valid = json.loads(self.registry.path.read_text())
        cases = ["not JSON", "[]", '{"version":1,"version":1}', '{"bad":NaN}']
        for field, value in (("version", 2), ("version", True), ("host_id", "other"),
                             ("checkouts", {}), ("defaults", []), ("extra", 1)):
            data = copy.deepcopy(valid)
            data[field] = value
            cases.append(json.dumps(data))
        for field, value in (("id", ""), ("directory", "relative"), ("directory", "/x/../y"),
                             ("directory", "//x/y"),
                             ("device", True), ("inode", -1), ("repository_urls", ["repo", "repo"]),
                             ("repository_urls", [""]), ("repository_urls", "repo"), ("extra", 1)):
            data = copy.deepcopy(valid)
            data["checkouts"][0][field] = value
            cases.append(json.dumps(data))
        duplicate = copy.deepcopy(valid)
        duplicate["checkouts"] *= 2
        cases.append(json.dumps(duplicate))
        duplicate = copy.deepcopy(valid)
        record = dict(duplicate["checkouts"][0], id="other")
        duplicate["checkouts"].append(record)
        cases.append(json.dumps(duplicate))
        for defaults in ({"repo": "missing"}, {"unbound": valid["checkouts"][0]["id"]}, {"repo": []}):
            data = dict(valid, defaults=defaults)
            cases.append(json.dumps(data))
        for contents in cases:
            self.registry.path.write_text(contents)
            with self.subTest(contents=contents):
                for operation in (self.registry.list, lambda: self.registry.register(self.root)):
                    with self.assertRaises(StreamError):
                        operation()
                self.assertEqual(self.registry.path.read_text(), contents)

    def test_duplicate_json_defaults_and_alias_roots_are_corrupt(self):
        first = self.registry.register(self.root, repository_url="repo")
        valid = json.loads(self.registry.path.read_text())
        alias = self.home / "alias"
        alias.symlink_to(self.root)
        record = dict(valid["checkouts"][0], id="other", directory=str(alias), inode=0)
        valid["checkouts"].append(record)
        self.registry.path.write_text(json.dumps(valid))
        with self.assertRaises(StreamError):
            self.registry.list()
        self.registry.path.write_text('{"version":1,"host_id":"test-host","checkouts":[],"defaults":'
                                      '{"repo":"' + first.id + '","repo":"' + first.id + '"}}')
        with self.assertRaises(StreamError):
            self.registry.list()

    def test_symlinked_state_and_lock_fail_closed(self):
        self.registry.register(self.root)
        destination = self.home / "outside.json"
        self.registry.path.rename(destination)
        self.registry.path.symlink_to(destination)
        before = destination.read_bytes()
        for operation in (self.registry.list, lambda: self.registry.register(self.root)):
            with self.assertRaises(StreamError):
                operation()
        self.assertEqual(destination.read_bytes(), before)
        self.registry.path.unlink()
        destination.rename(self.registry.path)
        self.registry._lock_path.unlink()
        self.registry._lock_path.symlink_to(self.home / "missing-lock")
        with self.assertRaises(StreamError):
            self.registry.register(self.root)
        self.assertFalse((self.home / "missing-lock").exists())

    def test_concurrent_process_registration_merges_bindings_and_keeps_id(self):
        source = Path(__file__).resolve().parents[2] / "src"
        code = ("import sys; from pathlib import Path; from _streams.catalog_checkouts import CheckoutRegistry; "
                "r=CheckoutRegistry(Path(sys.argv[1]), 'test-host'); "
                "print(r.register(Path(sys.argv[2]), repository_url=sys.argv[3]).id)")
        environment = dict(os.environ, PYTHONPATH=str(source))
        processes = [subprocess.Popen([sys.executable, "-c", code, str(self.state_dir), str(self.root), f"url-{i}"],
                                      env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                     for i in range(10)]
        ids = []
        for process in processes:
            out, error = process.communicate(timeout=45)
            self.assertEqual(process.returncode, 0, error)
            ids.append(out.strip())
        self.assertEqual(len(set(ids)), 1)
        self.assertEqual(len(self.registry.list()), 1)
        self.assertEqual(set(self.registry.list()[0].repository_urls), {f"url-{i}" for i in range(10)})
        self.assertEqual(list(self.state_dir.glob("*.tmp")), [])

    def test_atomic_failure_leaves_old_state_and_cleans_temporary(self):
        self.registry.register(self.root)
        before = self.registry.path.read_bytes()
        with patch("_streams.catalog_checkouts.os.replace", side_effect=OSError("interrupted")):
            with self.assertRaises(StreamError):
                self.registry.register(self.root, repository_url="new")
        self.assertEqual(self.registry.path.read_bytes(), before)
        self.assertEqual(list(self.state_dir.glob("*.tmp")), [])

    def test_traversal_invalid_paths_and_symlink_escape(self):
        checkout = self.registry.register(self.root, repository_url="repo")
        self.registry.set_default("repo", checkout.id)
        (self.root / "sub").mkdir()
        (self.root / "file.md").write_text("inside")
        outside = self.home / "outside"
        outside.mkdir()
        (outside / "file.md").write_text("outside")
        (self.root / "escape").symlink_to(outside, target_is_directory=True)
        (self.root / "escape.md").symlink_to(outside / "file.md")
        (self.root / "inside.md").symlink_to(self.root / "file.md")
        self.assertEqual(self.registry.document("repo", "inside.md")[1], self.root / "file.md")
        self.assertEqual(self.registry.directory(checkout.id), self.root)
        for path in ("", "..", "../outside", "/absolute", "sub/../file.md", "sub//file.md",
                     "./file.md", "sub/", "C:/file.md", "sub\\file.md", "bad\x00file", "bad\nfile"):
            with self.subTest(path=path):
                with self.assertRaises(StreamError):
                    self.registry.directory(checkout.id, path)
                with self.assertRaises(StreamError):
                    self.registry.document("repo", path)
        for path in ("escape", "file.md", "missing"):
            with self.subTest(path=path), self.assertRaises(StreamError):
                self.registry.directory(checkout.id, path)
        for path in ("escape/file.md", "escape.md", ".", "sub", "missing.md"):
            with self.subTest(path=path), self.assertRaises(StreamError):
                self.registry.document("repo", path)

    def test_root_paths_cannot_persist_invalid_text(self):
        root = self.repo("invalid\nname")
        with self.assertRaises(StreamError):
            self.registry.register(root)
        self.assertEqual(self.registry.list(), ())

    def test_text_validation_does_not_coerce_ids_or_urls(self):
        for value in (None, "", " ", 1, "bad\ntext", "bad\x00text"):
            with self.subTest(value=value):
                with self.assertRaises(StreamError):
                    CheckoutRegistry(self.state_dir, value)
                with self.assertRaises(StreamError):
                    self.registry.get(value)
                with self.assertRaises(StreamError):
                    self.registry.resolve(value)
                if value is not None:
                    with self.assertRaises(StreamError):
                        self.registry.register(self.root, repository_url=value)


if __name__ == "__main__":
    unittest.main()
