"""Read-only native context parity through isolated physical state and fake CLIs."""

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

    def invoke(self, implementation, *arguments):
        command = [str(RUST)] if implementation == "rust" else [sys.executable, "-m", "_streams.cli"]
        result = subprocess.run(command + list(arguments) + ["--json"], cwd=self.cwd,
                                env=self.env, text=True, capture_output=True, timeout=30)
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
