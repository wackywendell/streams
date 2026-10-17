"""Managed file-backed Git catalog: commit locally, publish by domain replay.

All live file/index/ref changes hold the YAML store lock. Candidate records and
indexes are separate from that checkout. Published history is never rewritten.
"""

from pathlib import Path
import tempfile
import time
from uuid import uuid4

from ..model import Stream, text
from ..operations import Operation, Reconciliation, reconcile
from ..storage import Snapshot, StorageError
from ..yaml_store import YamlStore
from .envelope import BOOTSTRAP, encode_operation
from .git_io import Git, GitError
from .history import History, read_history, records
from .local import RepairRequired, Result, State, authority_for, check_producer
from .policy import Policy, Timing


class GitCatalog:
    def __init__(self, root: Path, *, host_id: str, policy: Policy | None = None, clock=time.time):
        text(host_id, "host_id")
        self.root = Path(root).absolute()
        self.store, self.state = YamlStore(self.root), State(self.root)
        self.host_id, self.policy, self.clock = host_id, policy or Policy(), clock
        self.git = Git(self.root, self.policy.timeout_seconds)

    @classmethod
    def initialize(cls, root: Path, *, remote: str, host_id: str, branch: str = "main", **kwargs):
        """Explicit setup only. Requires normal Git author/signing configuration.

        Existing managed catalogs can be reopened; arbitrary repositories cannot
        be adopted implicitly. A failed setup is preserved for manual inspection.
        """
        catalog = cls(root, host_id=host_id, **kwargs)
        text(remote, "remote")
        text(branch, "branch")
        with catalog.store.transaction():
            config = catalog.state.read("config.json")
            if config is not None:
                if (config.get("remote"), config.get("host"), config.get("branch")) != (remote, host_id, branch):
                    raise RepairRequired("catalog configuration does not match this host, branch or remote")
                catalog._ready()
                return catalog
            catalog.state.check_save()
            allowed = {".catalog.lock", ".catalog-sync"}
            if any(path.name not in allowed for path in catalog.root.iterdir()):
                raise RepairRequired("initialization requires a new empty catalog directory")
            catalog.state.write("save.json", {"setup": True, "remote": remote, "branch": branch})
            catalog.git.run("init", "--initial-branch=" + branch)
            catalog.git.run("check-ref-format", "refs/heads/" + branch)
            catalog.git.run("remote", "add", "origin", remote)
            config = {"remote": remote, "branch": branch, "host": host_id}
            catalog.state.write("config.json", config)
            remote_tip = catalog._fetch()
            if remote_tip is None:
                tree = catalog.git.text("write-tree")
                tip = catalog._commit(tree, BOOTSTRAP)
            else:
                read_history(catalog.git, remote_tip)
                tip = remote_tip
            catalog.git.run("read-tree", "--reset", "-u", tip)
            catalog.git.run("update-ref", "refs/heads/" + branch, tip)
            catalog.state.clear("save.json")
        return catalog

    def _config(self):
        config = self.state.read("config.json")
        if config is None or config.get("host") != self.host_id:
            raise RepairRequired("catalog is not initialized for this host")
        branch, remote = config.get("branch"), config.get("remote")
        if not isinstance(branch, str) or not isinstance(remote, str):
            raise RepairRequired("invalid catalog configuration")
        if self.git.text("symbolic-ref", "--quiet", "HEAD") != "refs/heads/" + branch:
            raise RepairRequired("managed catalog branch was changed")
        if self.git.text("remote", "get-url", "origin") != remote:
            raise RepairRequired("managed catalog remote was changed")
        return config

    def _ready(self):
        if (self.root / ".git").is_symlink() or not (self.root / ".git").is_dir():
            raise RepairRequired("catalog must use its own Git directory")
        self.state.check_save()
        self._config()
        publication = self.state.read("publication.json")
        if publication is not None and publication.get("phase") == "adopting":
            raise RepairRequired("interrupted local adoption; inspect publication.json and retained refs")
        self._clean()

    def _clean(self):
        for name in (".catalog.lock", ".catalog-staging", ".catalog-sync"):
            if (self.root / name).is_symlink():
                raise RepairRequired(f"symlinked catalog machinery: {name}")
        entries = self.git.run("status", "--porcelain=v1", "-z", "--untracked-files=all", "--ignored")
        for entry in entries.split(b"\0"):
            if not entry:
                continue
            status, path = entry[:2], entry[3:].decode()
            machinery = path == ".catalog.lock" or path.startswith((".catalog-staging/", ".catalog-sync/"))
            if status not in (b"??", b"!!") or not machinery:
                raise RepairRequired(f"unexplained catalog index or working-tree change: {path}")

    def _commit(self, tree: str, message: str, parent: str | None = None) -> str:
        return self.git.commit(tree, message, parent)

    def _history(self) -> History:
        history = read_history(self.git, self.git.head())
        # A manually verified/repaired commit may predate its receipt write.
        # Establish retry protection before any history can leave the live tip.
        self.state.validate_operations((op for op, _ in history.operations.values()), remember=True)
        return history

    def snapshot(self) -> Snapshot:
        with self.store.transaction() as transaction:
            self._ready()
            history = self._history()
            snapshot = transaction.read()
            if records(snapshot.catalog) != records(history.catalog):
                raise RepairRequired("working records disagree with supported history")
            return snapshot

    def _authorities(self, local: History):
        remote = read_history(self.git, self._metadata()["confirmed"])
        # A local creation is usable until confirmed history contradicts it.
        return {**local.authorities, **remote.authorities}

    def authority(self, stream_id: str):
        with self.store.transaction():
            self._ready()
            return self._authorities(self._history()).get(stream_id)

    def save(self, operation: Operation, *, origin: dict[str, str] | None = None) -> Result:
        check_producer(operation, self.host_id)
        message = encode_operation(operation, origin={"host": self.host_id, **(origin or {})})
        try:
            with self.store.transaction() as transaction:
                self._ready()
                history = self._history()
                previous = history.operations.get(operation.id)
                remembered = self.state.seen(operation)
                if previous is not None or remembered:
                    if previous is not None and previous[0] != operation:
                        raise RepairRequired(f"operation ID reused with different intent: {operation.id}")
                    return Result("pending", domain=Reconciliation(operation.id, "no-change", reason="operation already saved"),
                                  local_revision=history.tip)
                snapshot = transaction.read()
                result = reconcile(snapshot.catalog, operation, authority=authority_for(operation, self._authorities(history)))
                if result.outcome != "changes":
                    if result.outcome == "no-change":
                        self.state.remember(operation)
                    return Result("pending", domain=result, local_revision=history.tip)
                self.state.write("save.json", {"start": history.tip, "operation": message})
                transaction.save(result.record, expected=snapshot)
                path = self.store.record_path(operation.target).as_posix()
                self.git.run("add", "--", path)
                commit = self._commit(self.git.text("write-tree"), message, history.tip)
                branch = self._config()["branch"]
                self.git.run("update-ref", "refs/heads/" + branch, commit, history.tip)
                self._clean()
                read_history(self.git, commit)
                self.state.remember(operation)
                self.state.clear("save.json")
                return Result("pending", local="committed", domain=result, local_revision=commit)
        except (OSError, ValueError, StorageError) as error:
            return Result("blocked", local="repair-required", detail=str(error))

    def _metadata(self):
        return self.state.read("sync.json") or {"confirmed": None, "last_sync": None, "last_push": None,
                                                "retry_after": 0, "fetched_at": None}

    def _fetch(self) -> str | None:
        config = self._config()
        ref = "refs/heads/" + config["branch"]
        found = self.git.text("ls-remote", "--heads", "origin", ref)
        tip = None
        if found:
            self.git.run("fetch", "--no-tags", "origin", ref)
            tip = self.git.text("rev-parse", "--verify", "FETCH_HEAD")
            history = read_history(self.git, tip)
        metadata = self._metadata()
        previous = metadata.get("confirmed")
        if previous is not None and (tip is None or not self.git.ancestor(previous, tip)):
            raise RepairRequired("confirmed remote history was removed or replaced")
        if tip is not None:
            self.state.validate_operations((op for op, _ in history.operations.values()), remember=True)
        metadata.update(confirmed=tip, fetched_at=self.clock(), refresh_retry_after=0, refresh_error="")
        self.state.write("sync.json", metadata)
        return tip

    def read(self, *, refresh: bool = False):
        """Blocking scheduled fetch and a non-publishing projection of pending intent."""
        from .coordinator import ReadResult
        from .replay import replay
        with self.store.transaction() as transaction:
            self._ready()
            local = self._history()
            snapshot = transaction.read()
            metadata = self._metadata()
            freshness = "cached"
            if self.policy.read_due(fetched_at=metadata["fetched_at"], now=self.clock(),
                                    retry_after=metadata.get("refresh_retry_after", 0), force=refresh):
                try:
                    self._fetch()
                    freshness = "refreshed"
                except GitError as error:
                    metadata = self._metadata()
                    metadata.update(refresh_retry_after=self.clock() + self.policy.cooldown_seconds,
                                    refresh_error=str(error))
                    self.state.write("sync.json", metadata)
                    freshness = "failed"
            metadata = self._metadata()
            remote = read_history(self.git, metadata["confirmed"])
            projected = replay(local, remote)
            catalog = snapshot.catalog if projected.conflicts else projected.catalog
            confirmed = YamlStore.decode_snapshot(remote.files) if metadata["fetched_at"] is not None else None
            return ReadResult(catalog, snapshot, confirmed, freshness, metadata["fetched_at"],
                              projected.conflicts, metadata.get("refresh_error", ""))

    def inspect_remote(self) -> Snapshot:
        """Fetch-only inspection never advances the publication gate or adopts files."""
        with self.store.transaction():
            self._ready()
            tip = self._fetch()
            return YamlStore.decode_snapshot(read_history(self.git, tip).files)

    def confirmed_snapshot(self) -> Snapshot:
        with self.store.transaction():
            tip = self._metadata().get("confirmed")
            return YamlStore.decode_snapshot(read_history(self.git, tip).files)

    def _candidate(self, local: History, remote: History):
        """Return a candidate tip and diagnostics, without touching the live index."""
        from .replay import replay
        base = remote.tip or local.root
        projected = replay(local, remote)
        if projected.conflicts:
            return base, projected.conflicts
        with tempfile.TemporaryDirectory(prefix="streams-candidate-") as directory:
            root = Path(directory) / "records"
            root.mkdir()
            for name, contents in remote.files.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(contents)
            store = YamlStore(root)
            index = Path(directory) / "index"
            self.git.run("read-tree", base, index=index)
            with store.transaction() as transaction:
                for operation, origin, record in projected.changes:
                    transaction.save(record, expected=transaction.read())
                    path = store.record_path(operation.target).as_posix()
                    blob = self.git.text("hash-object", "-w", "--stdin", input=(root / path).read_bytes())
                    self.git.run("update-index", "--add", "--cacheinfo", "100644," + blob + "," + path, index=index)
                    tree = self.git.text("write-tree", index=index)
                    base = self._commit(tree, encode_operation(operation, origin=origin), base)
            read_history(self.git, base)
            return base, ()

    def _retain(self, revision: str) -> str:
        ref = "refs/catalog/retained/" + str(uuid4())
        self.git.run("update-ref", ref, revision)
        return ref

    def _adopt(self, old: str, new: str, marker: dict):
        self._retain(old)
        marker.update(phase="adopting", adoption_from=old, adoption_to=new)
        self.state.write("publication.json", marker)
        if old != new:
            self.git.run("read-tree", "-m", "-u", old, new)
            branch = self._config()["branch"]
            self.git.run("update-ref", "refs/heads/" + branch, new, old)
        self._clean()
        read_history(self.git, new)
        self.state.clear("publication.json")

    def _confirmed_attempt(self, marker: dict, remote: History) -> bool:
        candidate = read_history(self.git, marker["candidate"])
        if remote.tip is not None and self.git.ancestor(candidate.tip, remote.tip):
            return True
        ids = marker["operations"]
        return bool(ids) and all(id in remote.operations and id in candidate.operations
                                 and remote.operations[id][0] == candidate.operations[id][0] for id in ids)

    def _record_confirmation(self, marker: dict):
        if marker.get("confirmed_at") is not None:
            return
        # Write the scheduling baseline first, then the marker. A crash here
        # requires inspection rather than repeatedly moving the baseline.
        metadata = self._metadata()
        key = marker["candidate"]
        if metadata.get("last_confirmed_attempt") != key:
            metadata.update(last_push=self.clock(), last_confirmed_attempt=key)
            self.state.write("sync.json", metadata)
        marker["confirmed_at"] = metadata["last_push"]
        self.state.write("publication.json", marker)

    def _success(self):
        metadata = self._metadata()
        metadata.update(last_sync=self.clock(), retry_after=0)
        self.state.write("sync.json", metadata)

    def _failure(self):
        metadata = self._metadata()
        metadata["retry_after"] = self.clock() + self.policy.cooldown_seconds
        self.state.write("sync.json", metadata)

    def sync(self, *, event: str = "explicit") -> Result:
        # Validate event even if the checkout is broken or the policy skips it.
        self.policy.due(event, Timing(), now=self.clock())
        try:
            with self.store.transaction():
                self._ready()
                local = self._history()
                metadata = self._metadata()
                timing = Timing(metadata["last_sync"], metadata["last_push"], metadata["retry_after"])
                remote_known = read_history(self.git, metadata["confirmed"])
                pending_stream = any(isinstance(op.after, Stream) and id not in remote_known.operations
                                     for id, (op, _) in local.operations.items())
                if not self.policy.due(event, timing, now=self.clock(), pending_stream=pending_stream):
                    return Result("pending", local_revision=local.tip, remote_revision=remote_known.tip)
                had_publication = False
                for attempt in range(self.policy.attempts):
                    remote = read_history(self.git, self._fetch())
                    marker = self.state.read("publication.json")
                    reuse = False
                    if marker is not None:
                        if self._confirmed_attempt(marker, remote):
                            had_publication = True
                            self._record_confirmation(marker)
                        elif marker["base"] == remote.tip and marker["local_tip"] == local.tip:
                            reuse = True
                    if reuse:
                        candidate, conflicts = marker["candidate"], ()
                    else:
                        candidate, conflicts = self._candidate(local, remote)
                    if conflicts:
                        return Result("blocked", local_revision=local.tip, remote_revision=remote.tip,
                                      detail="domain conflict blocks the outgoing batch", conflicts=conflicts)
                    changed = candidate != (remote.tip or local.root)
                    marker = {"phase": "attempting", "base": remote.tip, "candidate": candidate,
                              "local_tip": local.tip, "branch": self._config()["branch"],
                              "remote": self._config()["remote"],
                              "operations": [id for id in read_history(self.git, candidate).operations if id not in remote.operations]}
                    # Even an empty catalog needs its bootstrap published once.
                    if changed or remote.tip is None:
                        self._retain(candidate)
                        self.state.write("publication.json", marker)
                        try:
                            self.git.run("push", "origin", candidate + ":refs/heads/" + marker["branch"])
                        except GitError as push_error:
                            try:
                                observed = read_history(self.git, self._fetch())
                            except GitError:
                                self._failure()
                                return Result("uncertain", local_revision=local.tip, detail=str(push_error))
                            if self._confirmed_attempt(marker, observed):
                                self._record_confirmation(marker)
                                remote, had_publication = observed, True
                                # Rebuild only remaining operations against the
                                # newest accepted state, not the old candidate.
                                candidate, conflicts = self._candidate(local, remote)
                                if conflicts:
                                    return Result("blocked", conflicts=conflicts, detail="conflict after confirming earlier publication")
                            elif observed.tip != remote.tip and attempt + 1 < self.policy.attempts:
                                continue
                            else:
                                self._failure()
                                return Result("failed", local_revision=local.tip, remote_revision=observed.tip, detail=str(push_error))
                        else:
                            had_publication = True
                            metadata = self._metadata()
                            metadata.update(confirmed=candidate, fetched_at=self.clock())
                            self.state.write("sync.json", metadata)
                            self._record_confirmation(marker)
                    # Publication has been established (or no push was needed).
                    try:
                        self._adopt(local.tip, candidate, marker)
                    except (OSError, StorageError) as error:
                        marker.update(phase="adopting", adoption_from=local.tip, adoption_to=candidate)
                        self.state.write("publication.json", marker)
                        return Result("confirmed" if had_publication else "blocked", local="repair-required",
                                      local_revision=local.tip, remote_revision=self._metadata()["confirmed"], detail=str(error))
                    self._success()
                    return Result("confirmed", local_revision=candidate, remote_revision=self._metadata()["confirmed"])
                self._failure()
                return Result("failed", detail="competing-push retry budget exhausted")
        except RepairRequired as error:
            return Result("blocked", local="repair-required", detail=str(error))
        except (ValueError, StorageError, OSError) as error:
            try:
                self._failure()
            except (StorageError, OSError):
                pass
            return Result("failed", detail=str(error))

    def handover(self, *args, **kwargs):
        raise NotImplementedError("handover requires the domain publication coordinator")
