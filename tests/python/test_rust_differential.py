"""Black-box local-catalog parity; Python constructs every canonical envelope.

Build the test adapter explicitly with ``cargo build --example catalog_trace``.
The documented all-suite target owns building; discovery alone never builds Rust.
Only opaque storage UUIDs are normalized, with equality retained across steps.
Creation envelope strings and operation SHA256 receipts are compared verbatim.
"""

from __future__ import annotations

from copy import deepcopy
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from uuid import UUID

import yaml

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "src"))

from _streams.catalog.codec import record_to_data
from _streams.catalog.model import (
    Checkout, Document, Host, Link, Repository, Session, Stream, record_key,
)
from _streams.catalog.operations import Operation, update
from _streams.catalog.storage import StorageError
from _streams.catalog.sync import LocalCatalog, decode_operation, encode_operation
from _streams.catalog.yaml_store import YamlStore

DRIVER = ROOT / "target" / "debug" / "examples" / "catalog_trace"
HOST = "Laptop/é"
ORIGIN = {"host": HOST, "session": "会話\nsource"}


def creation(record, id="create"):
    return Operation(id, None, record, "user")


def stream(id="work", **kwargs):
    return Stream(id, "Design", "Share safely", HOST, **kwargs)


def wire_records(catalog):
    def key(record):
        identity = record_key(record)
        return identity.kind, identity.id, identity.host_id or ""

    return [{"kind": record_key(record).kind, "record": record_to_data(record)}
            for record in sorted(catalog.records, key=key)]


def python_request(root, request):
    # Reopen for every request: authority and retry identity must be durable.
    local = LocalCatalog(YamlStore(root), host_id=HOST)
    try:
        response = {}
        if request["op"] == "save":
            operation, origin = decode_operation(request["message"])
            result = local.save(operation, origin=origin)
            domain = result.domain
            response["result"] = {
                "sharing": result.sharing, "local": result.local,
                "local_revision": result.local_revision,
                "remote_revision": result.remote_revision,
                "detail": result.detail, "conflicts": list(result.conflicts),
                "domain": None if domain is None else {
                    "operation_id": domain.operation_id, "outcome": domain.outcome,
                    "record": None if domain.record is None else record_to_data(domain.record),
                    "conflicts": list(domain.conflicts), "reason": domain.reason,
                },
            }
        response["records"] = wire_records(local.snapshot().catalog)
        return response
    except (ValueError, StorageError, OSError) as error:
        return {"error": str(error)}


def persisted(root, revisions):
    files = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.name == ".catalog.lock":
            continue
        name = path.relative_to(root).as_posix()
        if path.suffix == ".yaml":
            data = yaml.safe_load(path.read_text())
            revision = data["revision"]
            UUID(revision)  # Do not hide missing or malformed storage stamps.
            identities = revisions.setdefault(name, {})
            data["revision"] = identities.setdefault(revision, len(identities))
        elif path.suffix == ".json":
            data = json.loads(path.read_text())
        else:
            raise AssertionError(f"unexpected persisted file: {name}")
        files[name] = data
    return files


@unittest.skipUnless(
    DRIVER.is_file(),
    "Rust differential driver not built; run cargo build --example catalog_trace first",
)
class LocalCatalogDifferentialTest(unittest.TestCase):
    maxDiff = None

    def run_trace(self, steps):
        """Each step is (operation, expected local, expected domain or error)."""
        with tempfile.TemporaryDirectory() as directory:
            roots = [Path(directory) / name for name in ("python", "rust")]
            revisions = [{}, {}]
            receipts, creations, results = {}, {}, []
            for index, (operation, expected_local, expected_domain) in enumerate(steps):
                message = encode_operation(operation, origin=ORIGIN)
                request = {"op": "save", "message": message}
                requests = [request, {"op": "snapshot"}]
                python = [python_request(roots[0], item) for item in requests]
                process = subprocess.run(
                    [str(DRIVER), str(roots[1]), HOST],
                    input="".join(json.dumps(item, ensure_ascii=False) + "\n" for item in requests),
                    capture_output=True, text=True, encoding="utf-8", timeout=30,
                )
                self.assertEqual(process.returncode, 0, process.stderr)
                lines = process.stdout.splitlines()
                self.assertEqual(len(lines), len(requests), process.stdout)
                rust = [json.loads(line) for line in lines]
                with self.subTest(step=index, operation=operation.id, check="save outcome and wire records"):
                    self.assertEqual(rust[0], python[0])
                with self.subTest(step=index, operation=operation.id, check="reopened snapshot"):
                    self.assertEqual(rust[1], python[1])
                result = python[0].get("result")
                if expected_local == "error":
                    self.assertIn(expected_domain, python[0]["error"])
                else:
                    self.assertEqual(result["local"], expected_local)
                    self.assertEqual(result["sharing"], "local-only")
                    self.assertEqual(None if result["domain"] is None else result["domain"]["outcome"],
                                     expected_domain)
                if result is not None and result["domain"] is not None:
                    if result["domain"]["outcome"] in ("changes", "no-change"):
                        canonical = encode_operation(operation).encode("utf-8")
                        receipts[operation.id] = hashlib.sha256(canonical).hexdigest()
                        if operation.before is None and isinstance(operation.after, Stream):
                            creations.setdefault(operation.after.id, message)
                disk = [persisted(root, stamps) for root, stamps in zip(roots, revisions)]
                with self.subTest(step=index, operation=operation.id, check="persisted wire catalog and bookkeeping"):
                    self.assertEqual(disk[1], disk[0])
                # Independent expectations ensure neither implementation can silently
                # drop envelopes/receipts or hash a normalized-but-different intent.
                for implementation, files in zip(("python", "rust"), disk):
                    with self.subTest(step=index, implementation=implementation, check="canonical receipts"):
                        actual = files.get(".catalog-sync/receipts.json", {"version": 1, "operations": {}})
                        self.assertEqual(actual, {"version": 1, "operations": receipts})
                    with self.subTest(step=index, implementation=implementation, check="creation envelopes"):
                        actual = files.get(".catalog-sync/local.json", {"version": 1, "creations": {}})
                        self.assertEqual(actual, {"version": 1, "creations": creations})
                self.assertNotIn(".catalog-sync/save.json", disk[0])
                self.assertNotIn(".catalog-sync/save.json", disk[1])
                self.assertFalse(any((root / ".git").exists() for root in roots))
                results.append(python[0])
            return results

    def test_retry_restore_and_changed_operation_id_rejection(self):
        initial = stream()
        create = creation(initial)
        park = update("park", initial, {"status": "parked"})
        restore = update("restore", park.after, {"status": "active"})
        changed_id = update("park", initial, {"status": "done"})
        results = self.run_trace([
            (create, "persisted", "changes"), (create, "unchanged", "no-change"),
            (park, "persisted", "changes"), (restore, "persisted", "changes"),
            (park, "unchanged", "no-change"), (changed_id, "repair-required", None),
            (creation(initial, "equivalent-create"), "unchanged", "no-change"),
        ])
        self.assertEqual(results[-1]["records"][0]["record"]["status"], "active")

    def test_user_title_normalization_and_automatic_protection(self):
        initial = stream()
        manual = update("manual", initial, {"title": initial.title})
        automatic = update("infer", manual.after, {"title": "Generated"},
                           actor="automatic", ownership_token="create")
        rename = update("rename", manual.after, {"title": "User choice"})
        results = self.run_trace([
            (creation(initial), "persisted", "changes"),
            (manual, "persisted", "changes"), (automatic, "unchanged", "superseded"),
            (rename, "persisted", "changes"), (rename, "unchanged", "no-change"),
        ])
        final = results[-1]["records"][0]["record"]
        self.assertEqual((final["title"], final["title_mode"]), ("User choice", "manual"))

    def test_captured_input_superseded_without_target_change(self):
        initial, evidence = stream(), stream("input")
        inferred = update("infer", initial, {"goal": "Derived goal"}, actor="automatic",
                          ownership_token="create", inputs=(evidence,))
        self.run_trace([
            (creation(initial), "persisted", "changes"),
            (creation(evidence, "create-input"), "persisted", "changes"),
            (update("edit-input", evidence, {"title": "New evidence"}), "persisted", "changes"),
            (inferred, "unchanged", "superseded"), (inferred, "unchanged", "superseded"),
        ])

    def test_session_authority_and_producer_fences(self):
        initial = stream()
        session = Session("Session/会話", HOST, initial.id, "pi", "Native/é", "Report")
        join = Operation("join", None, session, "automatic", "create")
        report = update("report", session, {"state": "Done\n下一步", "remaining": "Ship"},
                        actor="automatic", ownership_token="create")
        stale = update("stale", report.after, {"state": "Stale"}, actor="automatic",
                       ownership_token="expired")
        user_stale = update("user-stale", report.after, {"state": "User"},
                            ownership_token="expired")
        foreign = Session("foreign", "server", initial.id, "pi", "other", "Foreign")
        self.run_trace([
            (creation(initial), "persisted", "changes"),
            (Operation("expired-join", None, session, "automatic", "expired"), "unchanged", "superseded"),
            (join, "persisted", "changes"), (join, "unchanged", "no-change"),
            (report, "persisted", "changes"), (stale, "unchanged", "superseded"),
            (user_stale, "unchanged", "conflict"),
            (Operation("foreign", None, foreign, "user", "create"), "error", "another host"),
        ])

    def test_unicode_newlines_and_nested_wire_records(self):
        initial = Stream(
            "../É/设计\nwork", "Résumé\n设计", "First line\n第二行", HOST,
            repositories={"源码": Repository("ssh://example.org/é")},
            documents=(Document("源码", "分支", "docs/设计.md", "Design\nnotes"),),
            issues=(Link("https://example.org/é", "问题\ntracking"),),
        )
        session = Session("../会話", HOST, initial.id, "pi", "native\n会話", "Session\n标题",
                          state="First\nsecond", remaining="继续\n", checkout=Checkout("源码", "分支"))
        create = creation(initial, "创建\nstream")
        self.run_trace([
            (create, "persisted", "changes"),
            (Operation("加入\nsession", None, session, "automatic", create.id), "persisted", "changes"),
            (update("目标", initial, {"goal": "Changed\n目标"}), "persisted", "changes"),
            (create, "unchanged", "no-change"),
        ])

    def test_yaml_ambiguous_strings_remain_strings(self):
        initial = Stream(
            "yes", "on", "OFF", HOST, goal_short="null",
            repositories={"on": Repository("no")},
            documents=(Document("on", "yes", "on", "NO"),),
            issues=(Link("on", "yes"),),
        )
        session = Session("on", HOST, initial.id, "yes", "off", "no",
                          state="ON", remaining="NO", checkout=Checkout("on", "yes"))
        self.run_trace([
            (creation(initial), "persisted", "changes"),
            (Operation("join", None, session, "automatic", "create"), "persisted", "changes"),
            (creation(Host(HOST, "yes", datetime(2026, 10, 16, tzinfo=timezone.utc)), "host"),
             "persisted", "changes"),
        ])

    def test_yaml_tag_acceptance_matches_python(self):
        # Seed identical physical YAML, independently of either writer. Parser
        # diagnostics differ, so compare acceptance and decoded wire data here.
        for tag, accepted in (("!!str", True), ("!unknown", False),
                              ("!<tag:example.org,2026:text>", False)):
            with self.subTest(tag=tag), tempfile.TemporaryDirectory() as directory:
                roots = [Path(directory) / name for name in ("python", "rust")]
                envelope = {"version": 1, "revision": "00000000-0000-4000-8000-000000000001",
                            "kind": "stream", "record": record_to_data(stream())}
                contents = yaml.safe_dump(envelope, sort_keys=False, allow_unicode=True)
                self.assertIn("title: Design", contents)
                contents = contents.replace("title: Design", f"title: {tag} Design")
                for root in roots:
                    path = root / "streams" / "work.yaml"
                    path.parent.mkdir(parents=True)
                    path.write_text(contents, encoding="utf-8")
                python = python_request(roots[0], {"op": "snapshot"})
                process = subprocess.run(
                    [str(DRIVER), str(roots[1]), HOST], input='{"op":"snapshot"}\n',
                    capture_output=True, text=True, encoding="utf-8", timeout=30,
                )
                self.assertEqual(process.returncode, 0, process.stderr)
                rust = json.loads(process.stdout)
                self.assertEqual("error" not in python, accepted, python)
                self.assertEqual("error" not in rust, accepted, rust)
                if accepted:
                    self.assertEqual(rust, python)

    def test_positional_record_arrays_are_rejected_at_yaml_ingress(self):
        initial = stream(
            repositories={"repo": Repository("url")},
            documents=(Document("repo", "main", "doc", ""),),
            issues=(Link("url", ""),), prs=(Link("url", ""),),
        )
        session = Session("session", HOST, initial.id, "pi", "native", "Session",
                          checkout=Checkout("repo", "main"))
        host = Host(HOST, "Laptop", datetime(2026, 1, 1, tzinfo=timezone.utc))
        cases = [
            (initial, "owner", [HOST]),
            (initial, "repositories", {"repo": ["url"]}),
            (initial, "documents", [["repo", "main", "doc", "", None, None]]),
            (initial, "issues", [["url", ""]]),
            (initial, "prs", [["url", ""]]),
            (session, "checkout", ["repo", "main", "."]),
            (host, None, [HOST, "Laptop", "2026-01-01T00:00:00+00:00"]),
        ]
        for record, field, positional in cases:
            with self.subTest(kind=type(record).__name__, field=field), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                store = YamlStore(root)
                with store.transaction() as transaction:
                    for item in (initial, session, host):
                        transaction.save(item, expected=transaction.read())

                def rust_snapshot():
                    process = subprocess.run(
                        [str(DRIVER), str(root), HOST], input='{"op":"snapshot"}\n',
                        capture_output=True, text=True, encoding="utf-8", timeout=30,
                    )
                    self.assertEqual(process.returncode, 0, process.stderr)
                    return json.loads(process.stdout)

                expected = python_request(root, {"op": "snapshot"})
                self.assertNotIn("error", expected)
                self.assertEqual(rust_snapshot(), expected)
                path = root / store.record_path(record_key(record))
                envelope = yaml.safe_load(path.read_text())
                if field is None:
                    envelope["record"] = positional
                else:
                    envelope["record"][field] = positional
                path.write_text(yaml.safe_dump(envelope), encoding="utf-8")
                self.assertIn("error", python_request(root, {"op": "snapshot"}))
                actual = rust_snapshot()
                self.assertIn("error", actual)
                self.assertIn("must be an object", actual["error"])

    def test_positional_operation_envelopes_and_captures_are_rejected(self):
        initial = stream()
        supporting = Host(HOST, "Captured", datetime(2026, 1, 1, tzinfo=timezone.utc))
        operation = Operation("create", None, initial, "user", inputs=(supporting,))
        canonical = encode_operation(operation, origin=ORIGIN)
        prefix = "Streams operation v1\n\n"
        wire = json.loads(canonical[len(prefix):])
        cases = [[wire[field] for field in (
            "version", "id", "kind", "before", "after", "actor", "ownership_token", "origin", "inputs",
        )]]
        captured = deepcopy(wire)
        captured["inputs"] = [["host", record_to_data(supporting)]]
        cases.append(captured)
        captured = deepcopy(wire)
        captured["inputs"][0]["record"] = [HOST, "Captured", "2026-01-01T00:00:00+00:00"]
        cases.append(captured)
        for index, malformed in enumerate(cases):
            with self.subTest(case=index), tempfile.TemporaryDirectory() as directory:
                roots = [Path(directory) / name for name in ("python", "rust")]
                accepted = python_request(roots[0], {"op": "save", "message": canonical})
                self.assertNotIn("error", accepted)
                process = subprocess.run(
                    [str(DRIVER), str(roots[1]), HOST],
                    input=json.dumps({"op": "save", "message": canonical}) + "\n",
                    capture_output=True, text=True, encoding="utf-8", timeout=30,
                )
                self.assertEqual(process.returncode, 0, process.stderr)
                self.assertNotIn("error", json.loads(process.stdout))
                request = {"op": "save", "message": prefix + json.dumps(malformed) + "\n"}
                self.assertIn("error", python_request(roots[0], request))
                process = subprocess.run(
                    [str(DRIVER), str(roots[1]), HOST], input=json.dumps(request) + "\n",
                    capture_output=True, text=True, encoding="utf-8", timeout=30,
                )
                self.assertEqual(process.returncode, 0, process.stderr)
                self.assertIn("error", json.loads(process.stdout))

    def test_operation_enum_maps_are_not_wire_strings(self):
        prefix = "Streams operation v1\n\n"
        canonical = encode_operation(Operation("enum-shape", None, stream(), "user"), origin=ORIGIN)
        wire = json.loads(canonical[len(prefix):])
        for field in ("status", "title_mode", "goal_mode", "actor", "kind"):
            malformed = deepcopy(wire)
            container = malformed["after"] if field in ("status", "title_mode", "goal_mode") else malformed
            container[field] = {container[field]: None}
            request = {"op": "save", "message": prefix + json.dumps(malformed) + "\n"}
            with self.subTest(field=field), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                self.assertIn("error", python_request(root / "python", request))
                process = subprocess.run([str(DRIVER), str(root / "rust"), HOST],
                                         input=json.dumps(request) + "\n", capture_output=True, text=True, timeout=30)
                self.assertEqual(process.returncode, 0, process.stderr)
                self.assertIn("error", json.loads(process.stdout))

    def test_rust_opens_common_python_yaml_scalars(self):
        for title in ("0o10", "yes", "2026-01-01"):
            with self.subTest(title=title), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                initial = Stream("work", title, "Share safely", HOST)
                saved = python_request(root, {"op": "save",
                    "message": encode_operation(creation(initial))})
                self.assertNotIn("error", saved)
                expected = python_request(root, {"op": "snapshot"})
                process = subprocess.run([str(DRIVER), str(root), HOST],
                    input='{"op":"snapshot"}\n', capture_output=True, text=True,
                    encoding="utf-8", timeout=30)
                self.assertEqual(process.returncode, 0, process.stderr)
                self.assertEqual(json.loads(process.stdout), expected)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            envelope = {"version": 1, "revision": "revision", "kind": "stream",
                        "record": record_to_data(stream())}
            contents = yaml.safe_dump(envelope).replace("archived: false", "archived: yes")
            path = root / "streams" / "work.yaml"
            path.parent.mkdir(parents=True)
            path.write_text(contents)
            process = subprocess.run([str(DRIVER), str(root), HOST],
                input='{"op":"snapshot"}\n', capture_output=True, text=True, timeout=30)
            self.assertEqual(json.loads(process.stdout), python_request(root, {"op": "snapshot"}))

    def test_legacy_ambiguous_scalar_can_be_manually_quoted(self):
        # Exact legacy YAML 1.1 resolution is not a migration requirement.
        # Document the known boundary and the supported manual repair instead.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            initial = Stream("work", "1e3", "Share safely", HOST)
            python_request(root, {"op": "save", "message": encode_operation(creation(initial))})
            path = root / "streams" / "work.yaml"
            contents = path.read_text()
            self.assertIn("title: 1e3", contents)
            def rust_snapshot():
                process = subprocess.run([str(DRIVER), str(root), HOST],
                    input='{"op":"snapshot"}\n', capture_output=True, text=True, timeout=30)
                self.assertEqual(process.returncode, 0, process.stderr)
                return json.loads(process.stdout)
            self.assertIn("stream.title", rust_snapshot()["error"])
            path.write_text(contents.replace("title: 1e3", 'title: "1e3"'))
            self.assertEqual(rust_snapshot(), python_request(root, {"op": "snapshot"}))

    def test_host_fractional_microseconds_and_unicode_receipts(self):
        for microsecond in (123000, 123):
            with self.subTest(microsecond=microsecond):
                host = Host(HOST, "机器\nCafé", datetime(2026, 10, 16, 12, 34, 56,
                                                     microsecond, tzinfo=timezone.utc))
                create = creation(host, "主机\ncreate")
                edit = update("主机\nedit", host, {"label": "Révisé\n机器"})
                self.run_trace([
                    (create, "persisted", "changes"), (create, "unchanged", "no-change"),
                    (edit, "persisted", "changes"), (edit, "unchanged", "no-change"),
                ])

    def test_driver_serializes_decode_errors_and_continues(self):
        with tempfile.TemporaryDirectory() as directory:
            process = subprocess.run(
                [str(DRIVER), directory, HOST],
                input='{"op":"save","message":"not an envelope"}\n{"op":"snapshot"}\n',
                capture_output=True, text=True, encoding="utf-8", timeout=30,
            )
            self.assertEqual(process.returncode, 0, process.stderr)
            responses = [json.loads(line) for line in process.stdout.splitlines()]
            self.assertEqual(responses, [
                {"error": "unsupported operation commit message"}, {"records": []},
            ])


if __name__ == "__main__":
    unittest.main()
