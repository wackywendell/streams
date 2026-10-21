"""Native context and enrollment parity with isolated state and observational fake CLIs."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
RUST = ROOT / "target/debug/streams"

# Unknown commands fail and remain in the log, even if a caller swallows failure.
FAKE_NATIVE = r'''
import json
from pathlib import Path
import sys

root = Path(__file__).resolve().parent.parent / "native"
program = Path(sys.argv[0]).name
args = sys.argv[1:]
with (root / "commands.jsonl").open("a") as log:
    log.write(json.dumps([program, args]) + "\n")
args = [arg for arg in args if arg != "--json"]
key = None
if program == "herdr":
    if args == ["status"]:
        key = "status"
    elif args == ["pane", "current", "--current"]:
        key = "pane"
    elif len(args) == 3 and args[:2] == ["agent", "get"]:
        key = "agent"
    elif len(args) == 3 and args[:2] == ["workspace", "get"]:
        key = "workspace"
    elif args == ["api", "snapshot"]:
        key = "snapshot"
elif program == "cmux" and args == ["rpc", "workspace.list", "{}"]:
    key = "cmux"
if key is None:
    with (root / "forbidden.jsonl").open("a") as log:
        log.write(json.dumps([program, args]) + "\n")
    sys.exit(91)
print((root / (key + ".json")).read_text())
'''


def key(*parts):
    return hashlib.sha256(json.dumps(parts, ensure_ascii=True).encode()).hexdigest()


@unittest.skipUnless(RUST.is_file(), "build the Rust streams binary first")
class RustNativeContextDifferentialTests(unittest.TestCase):
    maxDiff = None

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="streams-native-context-")
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name).resolve()
        self.cwd = self.base / "working"
        self.cwd.mkdir()
        self.home = self.base / "home"
        self.home.mkdir()
        self.fixtures = self.base / "native"
        self.fixtures.mkdir()
        tools = self.base / "tools"
        tools.mkdir()
        for name in ("herdr", "cmux"):
            executable = tools / name
            executable.write_text(f"#!{sys.executable}\n" + FAKE_NATIVE, encoding="utf-8")
            executable.chmod(0o700)
        self.env = {
            "HOME": str(self.home),
            "XDG_CONFIG_HOME": str(self.home / "config"),
            "XDG_DATA_HOME": str(self.home / "data"),
            "XDG_STATE_HOME": str(self.home / "state"),
            "PATH": str(tools) + os.pathsep + os.environ.get("PATH", ""),
            "PYTHONPATH": str(ROOT / "src"),
            "PYTHONUTF8": "1",
            "PYTHONDONTWRITEBYTECODE": "1",
            "MISE_AUTO_INSTALL": "false",
            "HERDR_BIN_PATH": str(tools / "herdr"),
        }
        self.socket = str(self.base / "not-a-live-herdr.sock")
        self.put("status", {"server": {"running": True, "compatible": True,
                 "endpoint_compatible": True, "version": "0.9.3", "socket": self.socket,
                 "session": "fake-server"}})
        self.pane = {"pane_id": "pane-local", "terminal_id": "terminal-local",
                     "workspace_id": "workspace-local", "tab_id": "tab-local", "cwd": str(self.cwd)}
        self.agent = {**self.pane, "agent": "pi",
                      "agent_session": {"agent": "pi", "kind": "id", "value": "native-local"}}
        self.put("pane", {"result": {"type": "pane_current", "pane": self.pane}})
        self.put("agent", {"result": {"type": "agent_info", "agent": self.agent}})
        self.put("workspace", {"result": {"type": "workspace_info", "workspace": {
            "workspace_id": "workspace-local", "label": "Fixture"}}})
        self.put("cmux", {"workspaces": [
            {"id": "workspace-local", "ref": "workspace:1", "title": "Caller", "index": 1, "current_directory": str(self.cwd)},
            {"id": "focused-decoy", "ref": "workspace:2", "title": "Decoy", "index": 2, "current_directory": str(self.cwd)}]})
        self.call("python", "init", "--sync", "none")
        self.call("python", "config", "set", "frontend.mode", "none")
        self.catalog = Path(self.call("python", "config", "get", "catalog.root"))
        digest = hashlib.sha256(str(self.catalog.resolve()).encode()).hexdigest()
        self.runtime_root = self.home / "state/streams/catalogs" / digest
        self.addCleanup(self.assert_observational)

    def put(self, name, value):
        (self.fixtures / (name + ".json")).write_text(json.dumps(value), encoding="utf-8")

    def invoke(self, implementation, *arguments, request=None):
        command = [str(RUST)] if implementation == "rust" else [sys.executable, "-m", "_streams.cli"]
        result = subprocess.run(command + list(arguments) + ["--json"], cwd=self.cwd,
                                env=self.env, input=None if request is None else json.dumps(request),
                                text=True, capture_output=True, timeout=30)
        try:
            data = json.loads(result.stdout)
        except json.JSONDecodeError:
            self.fail(f"{implementation} {arguments}: stdout={result.stdout!r}; stderr={result.stderr!r}")
        return result.returncode, data

    def call(self, implementation, *arguments):
        code, data = self.invoke(implementation, *arguments)
        self.assertEqual(code, 0, (implementation, arguments, data))
        return data

    def files(self):
        return {str(path.relative_to(self.runtime_root)): path.read_bytes()
                for path in self.runtime_root.rglob("*") if path.is_file()}

    def compare(self, *arguments, error=None):
        before = self.files()
        python_code, python = self.invoke("python", *arguments)
        rust_code, rust = self.invoke("rust", *arguments)
        if error is None:
            self.assertEqual((python_code, rust_code), (0, 0), (python, rust))
            self.assertEqual(rust, python)
        else:
            self.assertNotEqual(python_code, 0, python)
            self.assertNotEqual(rust_code, 0, rust)
            self.assertEqual(python["error"]["kind"], error, python)
            self.assertEqual(rust["error"]["kind"], error, rust)
        self.assertEqual(self.files(), before, "native reads must not create locks or bindings")
        self.assert_observational()
        return python

    def context(self, *flags, native="native-local", error=None):
        return self.compare("agent", "context", "--harness", "pi", "--session-id", native,
                            *flags, error=error)

    def create_session(self):
        self.stream = self.call("python", "create", "Local work")["stream"]
        self.session = self.call("python", "session", "attach", self.stream["id"],
                                 "--harness", "pi", "--session-id", "native-local",
                                 "--title", "Exact local session")["session"]
        self.state = self.runtime_root / key(self.session["host_id"])

    def herdr(self):
        self.call("python", "config", "set", "frontend.mode", "herdr")
        self.env.update(HERDR_ENV="1", HERDR_PANE_ID="pane-local", HERDR_SOCKET_PATH=self.socket)
        self.scope = key("herdr", self.socket, "fake-server")

    def record(self, collection, value):
        path = self.state / collection / self.scope / (key("workspace-local") + ".json")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({"version": 1, "workspace_id": "workspace-local", **value}), encoding="utf-8")

    def binding(self, ready=True):
        self.record("associations", {"stream_id": self.stream["id"], "session_id": None,
                                     "command": None, "ready": ready})

    def assert_observational(self):
        forbidden = self.fixtures / "forbidden.jsonl"
        self.assertFalse(forbidden.exists(), forbidden.read_text() if forbidden.exists() else "")

    def commands(self):
        path = self.fixtures / "commands.jsonl"
        return path.read_text().splitlines() if path.exists() else []

    def enrollment_request(self, *, actor="automatic", native="native-local"):
        captured = self.call("python", "agent", "context", "--harness", "pi", "--session-id", native)
        return {"operation_id": "enroll-fixture", "catalog_source": captured["catalog_source"],
                "harness": "pi", "harness_session_id": native, "actor": actor,
                "stream": {"title": "Enrollment work", "goal": "Retain original intent"},
                "session": {"title": "Enrollment conversation"}, "cwd": str(self.cwd)}, captured

    def test_rust_enrollment_is_readable_and_retryable_by_python(self):
        request, _ = self.enrollment_request()
        code, created = self.invoke("rust", "agent", "enroll", request=request)
        self.assertEqual(code, 0, created)
        self.assertEqual([saved["operation_id"] for saved in created["saves"]],
                         ["enroll-fixture:stream", "enroll-fixture:session"])
        python_request = {**request, "operation_id": "python-enroll", "harness_session_id": "python-created"}
        python_code, python_created = self.invoke("python", "agent", "enroll", request=python_request)
        self.assertEqual(python_code, 0, python_created)
        for name, saved, python_saved in zip(("stream", "session"), created["saves"], python_created["saves"], strict=True):
            domain, python_domain = saved["save"]["domain"], python_saved["save"]["domain"]
            self.assertEqual(domain.keys(), python_domain.keys())
            self.assertEqual(domain["record"], created[name])
            self.assertEqual(python_domain["record"], python_created[name])
        python_code, python = self.invoke("python", "agent", "enroll", request=request)
        rust_code, rust = self.invoke("rust", "agent", "enroll", request=request)
        self.assertEqual((python_code, rust_code), (0, 0), (python, rust))
        self.assertEqual(python, rust)
        self.assertEqual(rust["saves"], [])
        self.assertEqual(self.context()["stream"], created["stream"])
        request["stream"]["title"] = "Do not overwrite existing work"
        self.assertEqual(self.invoke("rust", "agent", "enroll", request=request)[1]["stream"], created["stream"])

    def test_herdr_enrollment_claim_and_binding_interoperate_with_python(self):
        self.herdr()
        request, captured = self.enrollment_request()
        code, created = self.invoke("rust", "agent", "enroll", request=request)
        self.assertEqual(code, 0, created)
        self.state = self.runtime_root / key(captured["host_id"])
        binding = self.state / "associations" / self.scope / (key("workspace-local") + ".json")
        claim = self.state / "enrollments" / self.scope / (key("workspace-local") + ".json")
        self.assertTrue(json.loads(binding.read_text())["ready"])
        self.assertEqual(json.loads(claim.read_text())["stream_id"], created["stream"]["id"])
        self.context("--attest")
        python_code, python = self.invoke("python", "agent", "enroll", request=request)
        rust_code, rust = self.invoke("rust", "agent", "enroll", request=request)
        self.assertEqual((python_code, rust_code), (0, 0), (python, rust))
        self.assertEqual(python, rust)

    def test_automatic_helper_enrollment_cannot_claim_the_parent_workspace(self):
        self.herdr()
        request, _ = self.enrollment_request(native="background-helper")
        before = self.files()
        for implementation in ("python", "rust"):
            code, data = self.invoke(implementation, "agent", "enroll", request=request)
            self.assertNotEqual(code, 0, data)
            self.assertIn("error", data)
            self.assertEqual(self.files(), before)

    def test_incomplete_enrollment_keeps_receipts_and_claim_until_directory_is_repaired(self):
        from uuid import NAMESPACE_URL, uuid5
        self.herdr()
        request, captured = self.enrollment_request()
        identity = json.dumps([captured["host_id"], "pi", "native-local"])
        stream_id = str(uuid5(NAMESPACE_URL, "streams:effort:" + identity))
        self.state = self.runtime_root / key(captured["host_id"])
        location = self.state / "directories" / (key(stream_id, None) + ".json")
        location.parent.mkdir(parents=True)
        location.write_text("invalid directory intent")
        code, partial = self.invoke("rust", "agent", "enroll", request=request)
        self.assertEqual(code, 2, partial)
        self.assertEqual(len(partial["saves"]), 2)
        self.assertIn("completion_error", partial)
        claim = self.state / "enrollments" / self.scope / (key("workspace-local") + ".json")
        binding = self.state / "associations" / self.scope / (key("workspace-local") + ".json")
        self.assertEqual(json.loads(claim.read_text())["stream_id"], stream_id)
        self.assertFalse(binding.exists())
        for implementation in ("python", "rust"):
            code, retry = self.invoke(implementation, "agent", "enroll", request=request)
            self.assertEqual(code, 2, retry)
            self.assertEqual(retry["saves"], [])
            self.assertIn("completion_error", retry)
            self.assertEqual(retry["session"], partial["session"])
        location.unlink()
        code, completed = self.invoke("rust", "agent", "enroll", request=request)
        self.assertEqual(code, 0, completed)
        self.assertEqual(completed["saves"], [])
        self.assertTrue(json.loads(binding.read_text())["ready"])
        self.context("--attest")

    def test_manual_creation_retries_original_receipt_and_rejects_changed_intent(self):
        request, _ = self.enrollment_request(actor="user")
        code, created = self.invoke("rust", "agent", "enroll", request=request)
        self.assertEqual(code, 0, created)
        self.assertEqual(created["stream"]["title_mode"], "manual")
        retries = []
        for implementation in ("python", "rust"):
            code, retry = self.invoke(implementation, "agent", "enroll", request=request)
            self.assertEqual(code, 0, retry)
            self.assertEqual(len(retry["saves"]), 1)
            retries.append(retry)
        self.assertEqual(retries[0], retries[1])
        request["stream"]["title"] = "Changed intent must not be replayed"
        for implementation in ("python", "rust"):
            code, rejected = self.invoke(implementation, "agent", "enroll", request=request)
            self.assertEqual(code, 2, rejected)
            self.assertIn("completion_error", rejected)
        self.assertEqual(self.context()["stream"], created["stream"])

    def test_enrollment_infers_origin_without_selecting_a_default_or_changing_git(self):
        subprocess.run(["git", "init", "--quiet", "--template=", str(self.cwd)],
                       cwd=self.cwd, env=self.env, check=True, capture_output=True, timeout=30)
        subprocess.run(["git", "config", "--local", "remote.origin.url", "https://example.test/inferred"],
                       cwd=self.cwd, env=self.env, check=True, capture_output=True, timeout=30)
        before = {str(path.relative_to(self.cwd)): path.read_bytes()
                  for path in self.cwd.rglob("*") if path.is_file()}
        request, _ = self.enrollment_request()
        code, created = self.invoke("rust", "agent", "enroll", request=request)
        self.assertEqual(code, 0, created)
        registry = next((self.home / "state/streams/checkouts").glob("checkouts-*.json"))
        self.assertFalse(list(self.runtime_root.rglob("checkouts-*.json")))
        state = json.loads(registry.read_text())
        self.assertEqual(state["checkouts"][0]["repository_urls"], ["https://example.test/inferred"])
        self.assertEqual(state["defaults"], {})
        for implementation in ("python", "rust"):
            listed = self.call(implementation, "checkout", "list")
            self.assertEqual(listed["checkouts"], state["checkouts"])
        after = {str(path.relative_to(self.cwd)): path.read_bytes()
                 for path in self.cwd.rglob("*") if path.is_file()}
        self.assertEqual(before, after)

    def test_malformed_inferred_locator_retains_the_claim_in_both_implementations(self):
        subprocess.run(["git", "init", "--quiet", "--template=", str(self.cwd)],
                       cwd=self.cwd, env=self.env, check=True, capture_output=True, timeout=30)
        subprocess.run(["git", "config", "--local", "remote.origin.url", "https://example.test/a\tb"],
                       cwd=self.cwd, env=self.env, check=True, capture_output=True, timeout=30)
        self.herdr()
        request, captured = self.enrollment_request()
        self.state = self.runtime_root / key(captured["host_id"])
        for implementation in ("rust", "python"):
            code, partial = self.invoke(implementation, "agent", "enroll", request=request)
            self.assertEqual(code, 2, partial)
            self.assertIn("completion_error", partial)
            claim = self.state / "enrollments" / self.scope / (key("workspace-local") + ".json")
            self.assertTrue(claim.exists())
            binding = self.state / "associations" / self.scope / (key("workspace-local") + ".json")
            self.assertFalse(binding.exists())

    def test_unbound_context_without_runtime_state_or_binding_creates_no_locks(self):
        self.assertFalse(self.runtime_root.exists())
        context = self.context(native="unenrolled")
        self.assertIsNone(context["stream"])
        self.assertIsNone(context["session"])
        self.assertFalse(self.runtime_root.exists())
        self.herdr()
        context = self.context(native="unenrolled")
        self.assertIsNone(context["stream"])
        self.assertFalse(self.runtime_root.exists())
        self.compare("show", "--current", error="error")

    def test_claim_only_supplies_reporting_context_but_not_current_association(self):
        self.create_session()
        self.herdr()
        self.record("enrollments", {"stream_id": self.stream["id"]})
        context = self.context(native="unenrolled")
        self.assertEqual(context["stream"]["id"], self.stream["id"])
        self.assertIsNone(context["session"])
        self.compare("show", "--current", error="error")
        self.context("--attest", error="error")

    def test_partial_binding_allows_current_inspection_but_blocks_reporting(self):
        self.create_session()
        self.herdr()
        self.binding(ready=False)
        self.assertEqual(self.compare("show", "--current")["stream"], self.stream)
        self.context(error="error")
        self.context("--attest", error="error")

    def test_ready_context_and_attestation_include_all_native_inventory_tuples(self):
        self.create_session()
        self.herdr()
        self.binding()
        context = self.context()
        self.assertEqual(context["session"], self.session)
        self.assertNotIn("native", context)
        self.assertNotIn("native_inventory", context)
        self.compare("show", "--current")
        native = {"frontend": "herdr", "pane_id": "pane-local", "tab_id": "tab-local",
                  "workspace_id": "workspace-local"}
        attested = self.context("--attest")
        self.assertEqual(attested["native"], native)
        self.assertNotIn("native_inventory", attested)
        self.context("--native-inventory", error="error")
        # Neither ID metadata nor an absent transcript is evidence of death.
        panes = [self.agent,
                 {"pane_id": "pane-foreign", "terminal_id": "terminal-foreign",
                  "workspace_id": "workspace-foreign", "tab_id": "tab-foreign", "agent": "pi",
                  "agent_session": {"agent": "pi", "kind": "id", "value": "foreign-without-transcript"}},
                 {"pane_id": "pane-shell", "terminal_id": "terminal-shell",
                  "workspace_id": "workspace-local", "tab_id": "tab-local", "agent": None}]
        self.put("snapshot", {"result": {"type": "session_snapshot", "snapshot": {"panes": panes}}})
        context = self.context("--attest", "--native-inventory")
        self.assertEqual(context["native"], native)
        expected = [{field: pane[field] for field in ("pane_id", "terminal_id", "workspace_id", "tab_id")}
                    | {"native_session_id": identity}
                    for pane, identity in zip(panes, ("native-local", "foreign-without-transcript", None))]
        self.assertEqual(context["native_inventory"], {"socket": self.socket, "panes": expected})
        self.assertFalse((self.home / ".pi").exists())

    def test_attestation_rejects_mismatched_exact_pane_agent_and_native_identity(self):
        self.create_session()
        self.herdr()
        self.binding()
        for field, value in (("pane_id", "other-pane"), ("workspace_id", "other-workspace"),
                             ("agent", "codex"),
                             ("agent_session", {"agent": "pi", "kind": "id", "value": "other-native"})):
            with self.subTest(field=field):
                self.put("agent", {"result": {"type": "agent_info", "agent": {**self.agent, field: value}}})
                self.context("--attest", error="error")
        self.put("agent", {"result": {"type": "agent_info", "agent": self.agent}})
        self.put("pane", {"result": {"type": "pane_current", "pane": {**self.pane, "pane_id": "other-pane"}}})
        self.context("--attest", error="error")
        self.put("pane", {"result": {"type": "pane_current", "pane": self.pane}})
        self.context("--attest", native="unenrolled-helper", error="error")

    def test_attestation_requires_effective_authority_not_record_owner_fields(self):
        self.create_session()
        self.herdr()
        self.binding()
        path = self.catalog / ".catalog-sync/local.json"
        original = json.loads(path.read_text())
        for owner in (None, "foreign-host"):
            with self.subTest(effective_owner=owner):
                data = json.loads(json.dumps(original))
                if owner is None:
                    data["creations"].pop(self.stream["id"])
                else:
                    prefix, payload = data["creations"][self.stream["id"]].split("\n\n", 1)
                    receipt = json.loads(payload)
                    receipt["after"]["owner"]["host_id"] = owner
                    data["creations"][self.stream["id"]] = prefix + "\n\n" + json.dumps(receipt) + "\n"
                path.write_text(json.dumps(data), encoding="utf-8")
                context = self.context()
                self.assertEqual(context["stream"]["owner"]["host_id"], self.session["host_id"])
                self.assertEqual(context["authority"]["host_id"] if context["authority"] else None, owner)
                self.context("--attest", error="error")

    def test_explicit_identity_conflict_rejects_reporting_and_ready_current(self):
        self.create_session()
        other = self.call("python", "create", "Other work")["stream"]
        self.herdr()
        self.binding()
        self.env["STREAMS_STREAM_ID"] = other["id"]
        self.context(error="error")
        self.compare("show", "--current", error="error")

    def test_auto_ambiguous_current_uses_explicit_identity_without_native_calls(self):
        self.create_session()
        self.call("python", "config", "set", "frontend.mode", "auto")
        self.env.update(HERDR_ENV="1", HERDR_PANE_ID="pane-local",
                        CMUX_WORKSPACE_ID="workspace-local", STREAMS_STREAM_ID=self.stream["id"])
        before = self.commands()
        self.compare("show", "--current")
        self.context(error="ambiguous")
        self.assertEqual(self.commands(), before)

    def test_cmux_current_and_reporting_use_exact_caller_and_custom_socket_scope(self):
        self.create_session()
        self.call("python", "config", "set", "frontend.mode", "cmux")
        socket = str(self.base / "not-a-live-cmux.sock")
        self.env.update(CMUX_WORKSPACE_ID="workspace-local", CMUX_SOCKET_PATH=socket)
        self.scope = key("cmux", socket)
        self.binding()
        self.context()
        self.assertEqual(self.compare("show", "--current")["stream"], self.stream)
        self.env["CMUX_WORKSPACE_ID"] = "not-in-inventory"
        self.context(error="error")
        self.compare("show", "--current", error="error")


if __name__ == "__main__":
    unittest.main()
