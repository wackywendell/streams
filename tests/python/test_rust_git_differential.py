"""Bounded black-box Git publication parity, using real isolated repositories.

Build with ``cargo build --locked --example git_catalog_trace`` before discovery.
Every logical request reopens both implementations. Only Git OIDs and fixture
remote paths are normalized: OID tokens retain equality across the entire trace,
and each token also carries its full logical ancestry and materialized records.
Managed graph preservation projects reachability from HEAD, retained refs and
remote-tracking refs: random alias names, redundant aliases and protection of an
already reachable tip do not change that graph. Distinct OIDs with equal logical
histories remain distinct equality classes. Error prose is deliberately excluded;
acceptance, repair, conflict, provenance and receipt facts are not.
"""

from __future__ import annotations

from collections.abc import Mapping
from contextlib import contextmanager
from dataclasses import asdict
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "src"))

from _streams.catalog.codec import record_to_data
from _streams.catalog.model import RecordKey, Stream, record_key
from _streams.catalog.operations import Operation, update
from _streams.catalog.storage import StorageError
from _streams.catalog.sync import GitCatalog, decode_operation, encode_operation
from _streams.catalog.sync.git_io import Git, GitError
from _streams.catalog.sync.state import RepairRequired
from _streams.catalog.yaml_store import YamlStore

DRIVER = ROOT / "target/debug/examples/git_catalog_trace"
CONFIG = ("-c", "user.name=Streams differential", "-c",
          "user.email=streams@example.invalid", "-c", "commit.gpgsign=false",
          "-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false",
          "-c", "core.untrackedCache=false")
ORIGIN = {"host": "a", "session": "original/会話\nsource"}


def stream(id="work", host="a"):
    return Stream(id, "Design", "Share safely", host)


def create(id="work", host="a"):
    return Operation("create-" + id, None, stream(id, host), "user")


def wire_records(catalog):
    return [{"kind": record_key(r).kind, "record": record_to_data(r)}
            for r in sorted(catalog.records,
                            key=lambda r: (record_key(r).kind, record_key(r).id,
                                           record_key(r).host_id or ""))]


def outcome(result):
    domain = result.domain
    return {
        "sharing": result.sharing, "local": result.local,
        "domain": None if domain is None else {
            "operation_id": domain.operation_id, "outcome": domain.outcome,
            "record": None if domain.record is None else record_to_data(domain.record),
            "conflicts": list(domain.conflicts), "reason": domain.reason,
        },
        "local_revision": result.local_revision, "remote_revision": result.remote_revision,
        "conflicts": list(result.conflicts), "accepted": result.accepted,
        "requires_repair": result.requires_repair, "needs_attention": result.needs_attention,
    }


def python_request(root, host, request):
    original = Git.run
    fired = False
    fault = request.get("fault", "none")

    def command(git, *args, **kwargs):
        return original(git, *CONFIG, *args, **kwargs)

    def hooked(git, *args, **kwargs):
        nonlocal fired
        if fired and fault in ("lost-push-reply", "uncertain-unaccepted") and args[0] in ("ls-remote", "fetch"):
            raise GitError("fixture cannot confirm")
        if not fired and args[0] == "push" and fault in ("lost-push-reply", "lost-reply-failed-adoption", "uncertain-unaccepted"):
            fired = True
            if fault in ("lost-push-reply", "lost-reply-failed-adoption"):
                command(git, *args, **kwargs)
            raise GitError("fixture ambiguous push")
        if (fault in ("failed-adoption", "lost-reply-failed-adoption") and kwargs.get("index") is None
                and args[:3] == ("read-tree", "-m", "-u")):
            fired = True
            raise GitError("fixture adoption failure", code=1)
        return command(git, *args, **kwargs)

    with patch.object(Git, "run", hooked):
        catalog = GitCatalog(root, host_id=host, clock=lambda: request.get("now", 1000.0))
        try:
            op = request["op"]
            if op == "init":
                GitCatalog.initialize(root, host_id=host, remote=request.get("remote"), clock=lambda: 1000.0)
                return {"initialized": True}
            if op == "save":
                try:
                    operation, origin = decode_operation(request["message"])
                except ValueError:
                    return {"error": "envelope"}
                return {"result": outcome(catalog.save(operation, origin=origin))}
            if op == "sync":
                return {"result": outcome(catalog.sync(event=request["event"]))}
            if op == "snapshot":
                snapshot = catalog.confirmed_snapshot() if request.get("confirmed") else catalog.snapshot()
                return {"records": wire_records(snapshot.catalog)}
            if op == "authority":
                authority = catalog.authority(request["stream"])
                return {"authority": None if authority is None else asdict(authority)}
            if op == "read":
                read = catalog.read(refresh=request.get("refresh", False))
                return {"records": wire_records(read.catalog), "local": wire_records(read.local.catalog),
                        "confirmed": None if read.confirmed is None else wire_records(read.confirmed.catalog),
                        "freshness": read.freshness, "fetched_at": read.fetched_at,
                        "conflicts": list(read.conflicts)}
            raise AssertionError(op)
        except RepairRequired:
            return {"error": "repair-required"}
        except ValueError:
            return {"error": "validation"}
        except (StorageError, OSError):
            return {"error": "storage"}


def git_bytes(root, *args, allowed=(0,), input=None):
    result = subprocess.run(["git", "-C", str(root), *CONFIG, *args],
                            capture_output=True, input=input, timeout=30)
    if result.returncode not in allowed:
        raise AssertionError(result.stderr.decode())
    return result.stdout


def git(root, *args, allowed=(0,)):
    return git_bytes(root, *args, allowed=allowed).decode().strip()


def state_file(root, name, default):
    path = root / ".catalog-sync" / name
    return json.loads(path.read_text()) if path.exists() else default


class Observations:
    """Project public Git history and small bookkeeping facts, not coordinator state."""

    def __init__(self):
        self.identities = {}
        self.histories = {}
        self.parents = {}
        self.blobs = {}

    def files(self, root, revision):
        # Git object IDs are immutable; do not re-read unchanged blobs per commit.
        entries = {}
        for row in git_bytes(root, "ls-tree", "-r", "-z", revision).split(b"\0"):
            if not row:
                continue
            header, name = row.split(b"\t", 1)
            _, kind, oid = header.split()
            if kind != b"blob":
                raise AssertionError("fixture catalog unexpectedly contains a non-blob entry")
            entries[name.decode()] = oid
        missing = sorted(set(entries.values()) - self.blobs.keys())
        if missing:
            output = git_bytes(root, "cat-file", "--batch", input=b"".join(oid + b"\n" for oid in missing))
            cursor = 0
            for expected in missing:
                end = output.index(b"\n", cursor)
                oid, kind, size = output[cursor:end].split()
                size = int(size)
                cursor = end + 1
                if oid != expected or kind != b"blob" or size < 0 or output[cursor + size:cursor + size + 1] != b"\n":
                    raise AssertionError("invalid Git batch response")
                self.blobs[oid] = output[cursor:cursor + size]
                cursor += size + 1
            if cursor != len(output):
                raise AssertionError("unexpected trailing Git batch output")
        return {name: self.blobs[oid] for name, oid in entries.items()}

    def history(self, root, oid):
        if oid not in self.histories:
            history = []
            for commit in git(root, "rev-list", "--reverse", oid).splitlines():
                if commit in self.histories:
                    history = list(self.histories[commit])
                    continue
                parents, _, message = git(root, "show", "-s", "--format=%P%x00%B", commit).partition("\0")
                parents = parents.split()
                if len(parents) > 1:
                    raise AssertionError("fixture catalog unexpectedly contains a merge commit")
                self.parents[commit] = parents[0] if parents else None
                files = self.files(root, commit)
                records = wire_records(YamlStore.decode_snapshot(files).catalog)
                history.append({"message": message, "records": records})
                self.histories[commit] = list(history)
            self.histories[oid] = history
        return self.histories[oid]

    def revision(self, root, oid):
        if oid is None:
            return None
        identity = self.identities.setdefault(oid, len(self.identities))
        history = self.history(root, oid)
        parent = self.parents[oid]
        parent_identity = None if parent is None else self.identities.setdefault(parent, len(self.identities))
        return {"identity": identity, "parent": parent_identity, "history": history}

    def response(self, root, response):
        if "result" in response:
            for field in ("local_revision", "remote_revision"):
                response["result"][field] = self.revision(root, response["result"][field])
        return response

    def disk(self, root, remote):
        head = git(root, "rev-parse", "HEAD")
        remote_tip = git(remote, "rev-parse", "--verify", "refs/heads/main", allowed=(0, 128))
        metadata = state_file(root, "sync.json", {})
        confirmed = metadata.get("confirmed")
        # Decode envelopes rather than infer pending identity from filenames or tips.
        def operation_ids(oid):
            result = []
            if oid:
                for item in self.history(root, oid):
                    try:
                        operation, _ = decode_operation(item["message"] + "\n")
                    except ValueError:
                        continue  # The only non-operation in these fixtures is bootstrap.
                    result.append(operation.id)
            return result
        facts = {"head": self.revision(root, head),
                 "remote": self.revision(remote, remote_tip or None),
                 "confirmed": self.revision(root, confirmed),
                 "pending": [id for id in operation_ids(head) if id not in operation_ids(confirmed)],
                 "receipts": state_file(root, "receipts.json", {"version": 1, "operations": {}}),
                 "save_fenced": (root / ".catalog-sync/save.json").exists(),
                 "last_push": metadata.get("last_push")}
        tips = {head, *git(root, "for-each-ref", "--format=%(objectname)",
                          "refs/catalog/retained", "refs/remotes").splitlines()}
        # Compare reachable actual nodes, not protected alias names or collapsed
        # operation IDs. A conservative alias of HEAD adds no preservation fact.
        reachable = set(git(root, "rev-list", *sorted(tips)).splitlines())
        reachable = sorted(reachable, key=lambda oid: (
            json.dumps(self.history(root, oid), sort_keys=True), self.identities.get(oid, -1)))
        facts["reachable"] = sorted((self.revision(root, oid) for oid in reachable),
                                    key=lambda revision: revision["identity"])
        marker = state_file(root, "publication.json", None)
        if marker is not None:
            # Exact field projection; only this fixture's remote path is unstable.
            assert marker["remote"] == str(remote)
            marker["remote"] = "<fixture remote>"
            for field in ("base", "candidate", "local_tip", "adoption_from", "adoption_to"):
                if field in marker:
                    marker[field] = self.revision(root, marker[field])
        facts["publication"] = marker
        return facts


@unittest.skipUnless(DRIVER.is_file(), "build --example git_catalog_trace before differential discovery")
class GitCatalogDifferentialTest(unittest.TestCase):
    maxDiff = None

    @contextmanager
    def trace(self):
        with tempfile.TemporaryDirectory(prefix="streams-git-differential-") as directory:
            self.roots = [Path(directory) / name for name in ("python", "rust")]
            self.observers = [Observations(), Observations()]
            self.receipts = {"a": {}, "b": {}, "reader": {}}
            self.intents = {}
            self.steps = []
            for root in self.roots:
                remote = root / "remote.git"
                remote.mkdir(parents=True)
                git(remote, "init", "--bare", "--initial-branch=main")
            yield

    def step(self, host, op, operation=None, **kwargs):
        request = {"op": op, **kwargs}
        if operation is not None:
            request["message"] = encode_operation(operation, origin=ORIGIN)
        self.steps.append((host, request.copy()))
        outputs, disks = [], []
        for index, base in enumerate(self.roots):
            root, remote = base / host, base / "remote.git"
            actual = dict(request)
            if op == "init":
                actual["remote"] = str(remote)
            if index == 0:
                # Both adapters cross the same JSON-lines wire boundary; Python
                # reconciliation tuples become JSON arrays, just like Rust Vecs.
                def wire_default(value):
                    if isinstance(value, RecordKey):
                        return asdict(value)
                    if isinstance(value, Mapping):
                        return dict(value)
                    raise TypeError(f"unsupported fixture wire value: {type(value)}")
                response = json.loads(json.dumps(python_request(root, host, actual),
                                                 ensure_ascii=False, default=wire_default))
            else:
                process = subprocess.run([str(DRIVER), str(root), host],
                    input=json.dumps(actual, ensure_ascii=False) + "\n", capture_output=True,
                    text=True, encoding="utf-8", timeout=60)
                self.assertEqual(process.returncode, 0, process.stderr)
                self.assertEqual(len(process.stdout.splitlines()), 1, process.stdout)
                response = json.loads(process.stdout)
            outputs.append(self.observers[index].response(root, response))
            disks.append(self.observers[index].disk(root, remote))
        context = f"logical trace prefix: {self.steps!r}"
        self.assertEqual(outputs[1], outputs[0], context)
        self.assertEqual(disks[1], disks[0], context)
        if operation is not None and outputs[0].get("result", {}).get("accepted"):
            digest = hashlib.sha256(encode_operation(operation).encode()).hexdigest()
            self.intents.setdefault(operation.id, digest)
            self.receipts[host][operation.id] = digest
        # Fetching supported history also establishes receipts. Expectations come
        # from the original fixture intents, not from either receipt writer.
        for tip in (disks[0]["head"], disks[0]["confirmed"]):
            if tip is not None:
                for item in tip["history"][1:]:
                    seen, _ = decode_operation(item["message"] + "\n")
                    self.receipts[host][seen.id] = self.intents[seen.id]
        self.assertEqual(disks[0]["receipts"], {"version": 1, "operations": self.receipts[host]}, context)
        self.last_disk = disks[0]
        return outputs[0]

    def save(self, host, operation):
        return self.step(host, "save", operation)["result"]

    def sync(self, host, **kwargs):
        return self.step(host, "sync", event="explicit", **kwargs)["result"]

    def initial(self):
        self.step("a", "init")
        self.assertTrue(self.save("a", create())["accepted"])
        self.assertEqual(self.sync("a")["sharing"], "confirmed")

    def current(self, host="a", **kwargs):
        return self.step(host, "snapshot", **kwargs)["records"]

    def test_two_clones_nonoverlapping_merge_preserves_original_origin(self):
        with self.trace():
            self.initial()
            self.step("b", "init")
            park = update("park", stream(), {"status": "parked"})
            self.save("a", park)
            self.save("b", update("rename", stream(), {"title": "Remote title"}))
            self.assertEqual(self.sync("b")["sharing"], "confirmed")
            projected = self.step("a", "read", refresh=True)
            self.assertEqual(projected["records"][0]["record"]["title"], "Remote title")
            self.assertEqual(projected["local"][0]["record"]["title"], "Design")
            self.assertEqual(self.sync("a")["sharing"], "confirmed")
            self.assertEqual(self.last_disk["pending"], [])
            self.assertIn(self.last_disk["head"], self.last_disk["reachable"])
            envelope = self.last_disk["head"]["history"][-1]["message"] + "\n"
            self.assertEqual(decode_operation(envelope), (park, ORIGIN))
            self.assertEqual(self.step("a", "authority", stream="work")["authority"],
                             {"host_id": "a", "token": "create-work"})
            self.assertEqual(self.sync("b")["sharing"], "confirmed")
            self.assertEqual(self.current("b"), self.current())

    def test_lost_reply_restart_and_intervening_save_publish_both(self):
        with self.trace():
            self.step("a", "init")
            self.save("a", create())
            self.assertEqual(self.sync("a", fault="lost-push-reply")["sharing"], "uncertain")
            self.assertEqual(self.last_disk["publication"]["phase"], "attempting")
            self.save("a", create("later"))
            self.assertEqual(self.sync("a")["sharing"], "confirmed")
            self.step("reader", "init")
            self.assertEqual({r["record"]["id"] for r in self.current("reader")}, {"work", "later"})
            messages = self.last_disk["remote"]["history"]
            ids = [decode_operation(item["message"] + "\n")[0].id for item in messages[1:]]
            self.assertEqual(ids, ["create-work", "create-later"])

    def test_lost_confirmation_then_conflict_remains_blocked(self):
        with self.trace():
            self.step("a", "init")
            self.save("a", create())
            self.assertEqual(self.sync("a", fault="lost-push-reply")["sharing"], "uncertain")
            self.step("b", "init")
            self.save("b", update("remote-title", stream(), {"title": "Remote"}))
            self.sync("b")
            self.save("a", update("local-title", stream(), {"title": "Local"}))
            result = self.sync("a", now=1100.0)
            self.assertEqual(result["sharing"], "blocked")
            self.assertTrue(result["needs_attention"])
            self.assertFalse(result["requires_repair"])
            self.assertEqual(result["conflicts"][0]["operation"], "local-title")
            self.assertEqual(self.last_disk["last_push"], 1100.0)
            again = self.sync("a", now=1200.0)
            self.assertEqual(again["sharing"], "blocked")
            self.assertTrue(again["needs_attention"])
            self.assertEqual(self.last_disk["last_push"], 1100.0)
            self.assertEqual(self.current()[0]["record"]["title"], "Local")
            self.assertEqual(self.current(confirmed=True)[0]["record"]["title"], "Remote")

    def test_unaccepted_uncertain_restart_replays_onto_remote_change(self):
        with self.trace():
            self.initial()
            self.step("b", "init")
            self.save("a", update("park", stream(), {"status": "parked"}))
            self.assertEqual(self.sync("a", fault="uncertain-unaccepted")["sharing"], "uncertain")
            self.save("a", create("later"))
            self.save("b", update("rename", stream(), {"title": "Concurrent"}))
            self.sync("b")
            self.assertEqual(self.sync("a")["sharing"], "confirmed")
            self.step("reader", "init")
            records = {r["record"]["id"]: r["record"] for r in self.current("reader")}
            self.assertEqual((records["work"]["status"], records["work"]["title"]), ("parked", "Concurrent"))
            self.assertIn("later", records)

    def test_failed_adoption_stays_fenced_with_confirmed_publication(self):
        with self.trace():
            self.initial()
            self.step("b", "init")
            self.save("b", update("rename", stream(), {"title": "Remote"}))
            self.sync("b")
            self.save("a", update("park", stream(), {"status": "parked"}))
            old = self.last_disk["head"]
            result = self.sync("a", fault="failed-adoption")
            self.assertEqual((result["sharing"], result["local"]), ("confirmed", "repair-required"))
            self.assertTrue(result["requires_repair"])
            self.assertEqual(self.last_disk["head"], old)
            self.assertIn(old, self.last_disk["reachable"])
            self.assertIn(self.last_disk["confirmed"], self.last_disk["reachable"])
            self.assertEqual(self.last_disk["publication"]["phase"], "adopting")
            self.assertEqual(self.step("a", "snapshot"), {"error": "repair-required"})
            self.assertEqual(self.step("a", "read", refresh=True), {"error": "repair-required"})
            self.assertEqual(self.save("a", create("later"))["local"], "repair-required")
            self.assertEqual(self.sync("a")["local"], "repair-required")
            confirmed = self.current(confirmed=True)[0]["record"]
            self.assertEqual((confirmed["title"], confirmed["status"]), ("Remote", "parked"))
            self.step("reader", "init")
            self.assertEqual(self.current("reader")[0]["record"], confirmed)

    def test_recovered_confirmation_receipt_survives_adoption_failure(self):
        for restart in (False, True):
            with self.subTest(restart=restart), self.trace():
                self.step("a", "init")
                self.save("a", create())
                if restart:
                    self.assertEqual(self.sync("a", fault="lost-push-reply")["sharing"], "uncertain")
                    original = self.last_disk["publication"].copy()
                    self.step("b", "init")
                    self.save("b", create("newer", "b"))
                    self.sync("b")
                    result = self.sync("a", now=1100.0, fault="failed-adoption")
                else:
                    original = None
                    result = self.sync("a", now=1100.0, fault="lost-reply-failed-adoption")
                self.assertEqual((result["sharing"], result["local"]), ("confirmed", "repair-required"))
                marker = self.last_disk["publication"]
                self.assertEqual(marker["phase"], "adopting")
                self.assertEqual(marker["confirmed_at"], 1100.0)
                self.assertEqual(marker["operations"], ["create-work"])
                self.assertIsNone(marker["base"])
                if original is not None:
                    for field in ("base", "candidate", "local_tip", "operations", "branch", "remote"):
                        self.assertEqual(marker[field], original[field])
                    self.assertNotEqual(marker["candidate"], marker["adoption_to"])
                self.assertEqual(self.last_disk["last_push"], 1100.0)
                self.assertEqual(self.step("a", "snapshot"), {"error": "repair-required"})
                self.assertTrue(self.sync("a", now=1200.0)["requires_repair"])
                self.assertEqual(self.last_disk["last_push"], 1100.0)

    def test_stale_intent_conflict_read_projection_and_later_saves(self):
        with self.trace():
            self.initial()
            self.step("b", "init")
            self.save("b", update("done", stream(), {"status": "done"}))
            self.sync("b")
            self.save("a", update("park", stream(), {"status": "parked"}))
            before = self.last_disk["remote"]
            read = self.step("a", "read", refresh=True)
            self.assertEqual(read["records"], read["local"])
            self.assertEqual(read["confirmed"][0]["record"]["status"], "done")
            self.assertEqual(read["records"][0]["record"]["status"], "parked")
            result = self.sync("a")
            self.assertEqual((result["sharing"], result["local"]), ("blocked", "unchanged"))
            self.assertEqual(result["conflicts"][0]["origin"], ORIGIN)
            self.assertEqual(result["conflicts"][0]["operation"], "park")
            self.assertEqual(self.last_disk["remote"], before)
            self.assertEqual(self.save("a", create("later"))["local"], "committed")

    def test_retired_operation_retry_idempotent_and_changed_intent_rejected(self):
        with self.trace():
            self.initial()
            self.step("b", "init")
            park = update("park-a", stream(), {"status": "parked"})
            self.save("a", park)
            self.save("b", update("park-b", stream(), {"status": "parked"}))
            self.sync("b")
            self.assertEqual(self.sync("a")["sharing"], "confirmed")
            adopted_ids = [decode_operation(item["message"] + "\n")[0].id
                           for item in self.last_disk["head"]["history"][1:]]
            self.assertNotIn("park-a", adopted_ids)
            self.save("a", update("restore", park.after, {"status": "active"}))
            self.sync("a")
            result = self.save("a", park)
            self.assertEqual((result["local"], result["domain"]["outcome"]), ("unchanged", "no-change"))
            changed = self.save("a", update("park-a", stream(), {"status": "done"}))
            self.assertEqual(changed["local"], "repair-required")
            self.assertFalse(changed["accepted"])
            self.assertEqual(self.current()[0]["record"]["status"], "active")


if __name__ == "__main__":
    unittest.main()
