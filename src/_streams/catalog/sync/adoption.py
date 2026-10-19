"""Explicit, locked import of local-only state into supported local Git history."""

import hashlib
import os
from pathlib import Path
import shutil
import tempfile

from ..model import record_key, text
from ..yaml_store import _sync_directory
from .baseline import Baseline, decode_baseline, encode_baseline
from .git import GitCatalog
from .git_io import Git
from .history import read_history
from .local import LocalCatalog
from .state import GitConfig, RepairRequired


def _sync_tree(root: Path):
    directories = [root]
    for path in root.rglob("*"):
        if path.is_symlink():
            raise RepairRequired("prepared Git machinery contains a symlink")
        if path.is_dir():
            directories.append(path)
        else:
            with path.open("rb") as handle:
                os.fsync(handle.fileno())
    for directory in reversed(directories):
        _sync_directory(directory)


def adopt_local(catalog: GitCatalog, *, branch: str) -> str:
    """Caller holds host configuration lock; retain evidence after fencing begins."""
    text(branch, "branch")
    root, state = catalog.root, catalog.state
    with catalog.store.transaction() as transaction:
        state.check_save()
        marker = state.read("cli.json")
        expected = {"version": 1, "host": catalog.host_id, "strategy": "none"}
        if marker == {**expected, "strategy": "git"}:
            config = catalog._ready()
            if config != GitConfig(remote=None, branch=branch, host=catalog.host_id):
                raise RepairRequired("completed adoption does not match this host, branch or local-only mode")
            history, _ = catalog._saved(transaction)
            if history.baseline is None:
                raise RepairRequired("existing Git catalog is not a completed snapshot adoption")
            return history.root
        if marker != expected:
            raise RepairRequired("adoption requires this host's initialized local-only catalog")
        local = LocalCatalog(catalog.store, host_id=catalog.host_id)
        local._ready()
        snapshot = transaction.read()
        files = {}
        for record in snapshot.catalog.records:
            path = catalog.store.record_path(record_key(record)).as_posix()
            files[path] = (root / path).read_bytes()
        names = ("cli.json", "local.json", "receipts.json")
        before = {name: (state.root / name).read_text() if (state.root / name).exists() else None for name in names}
        creations = (state.read("local.json") or {"creations": {}})["creations"]
        baseline = Baseline(catalog.host_id, creations, state.fingerprints())
        message = encode_baseline(baseline)
        decode_baseline(message, files)
        staging = root / ".catalog-staging"
        if staging.is_symlink():
            raise RepairRequired("symlinked catalog staging directory")
        staging.mkdir(mode=0o700, exist_ok=True)
        candidate = Path(tempfile.mkdtemp(prefix="adoption-", dir=staging))
        fenced, completed = False, False
        try:
            git = Git(candidate, catalog.policy.timeout_seconds)
            git.run("init", "--initial-branch=" + branch)
            git.run("check-ref-format", "refs/heads/" + branch)
            if git.text("remote"):
                raise RepairRequired("local Git preparation has an unexpected remote")
            git.run("read-tree", "--empty")
            entries = []
            for path, contents in files.items():
                blob = git.text("hash-object", "-w", "--stdin", input=contents)
                entries.append(f"100644 {blob}\t{path}\0")
            git.run("update-index", "-z", "--index-info", input="".join(entries).encode())
            profile = git.signing_profile()
            tip = git.commit(git.text("write-tree"), message)
            git.run("update-ref", "refs/heads/" + branch, tip)
            history = read_history(git, tip)
            if history.files != files or git.signing_profile() != profile:
                raise RepairRequired("prepared Git history or signing policy changed")
            _sync_tree(candidate / ".git")
            local._ready()
            if transaction.read().revisions != snapshot.revisions:
                raise RepairRequired("catalog records changed during adoption preparation")
            current = {name: (state.root / name).read_text() if (state.root / name).exists() else None for name in names}
            if current != before:
                raise RepairRequired("catalog bookkeeping changed during adoption preparation")
            # Even a failed marker write can have installed its evidence before
            # directory fsync failed. Retain the candidate from this point on.
            fenced = True
            evidence = {"adoption": {"baseline": tip, "branch": branch, "host": catalog.host_id,
                                     "before": before, "profile": profile,
                                     "files": {path: hashlib.sha256(data).hexdigest() for path, data in files.items()}}}
            state.write("save.json", evidence)
            os.replace(candidate / ".git", root / ".git")
            _sync_directory(root)
            if catalog.git.signing_profile() != profile:
                raise RepairRequired("Git identity or signing policy differs at the final catalog path; inspect save.json")
            catalog._clean()
            installed = read_history(catalog.git, catalog.git.head())
            if installed.tip != tip or installed.files != files:
                raise RepairRequired("installed Git baseline differs from the prepared catalog")
            _sync_tree(root / ".git")
            state.write_config(GitConfig(remote=None, branch=branch, host=catalog.host_id))
            catalog._config()
            state.write("cli.json", {"strategy": "git", "host": catalog.host_id})
            try:
                state.clear("save.json")
            except OSError as error:
                # Clearing can fail after unlink. Fence waiting writers again
                # with the original evidence; this does not roll back adoption.
                state.write("save.json", {**evidence, "completion_error": str(error)})
                raise
            completed = True
            return tip
        finally:
            if not fenced:
                shutil.rmtree(candidate)
            elif completed:
                try:
                    candidate.rmdir()
                except OSError:
                    # An empty staging orphan does not affect catalog reads.
                    pass
