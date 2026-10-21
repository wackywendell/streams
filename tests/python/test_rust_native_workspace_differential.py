"""Real CLI orchestration against stateful fake Herdr/CMUX services and a fake Pi exec."""
import json
import shlex
from pathlib import Path
import sys

from test_rust_native_context_differential import NativeFixture, key

FAKE_WORKSPACE = r'''
import json
from pathlib import Path
import sys
root = Path(__file__).resolve().parent.parent / "native"
program = Path(sys.argv[0]).name
args = [a for a in sys.argv[1:] if a != "--json"]
with (root / "commands.jsonl").open("a") as log:
    log.write(json.dumps([program, args]) + "\n")
def load(name):
    return json.loads((root / (name + ".json")).read_text())
def save(name, value):
    (root / (name + ".json")).write_text(json.dumps(value))
def flag(name):
    return args[args.index(name) + 1]
def fresh(prefix):
    path = root / "counters.json"
    counters = json.loads(path.read_text()) if path.exists() else {}
    counters[prefix] = counters.get(prefix, 0) + 1
    path.write_text(json.dumps(counters))
    return prefix + str(counters[prefix])
def pane(workspace, tab, cwd):
    identifier = fresh("p")
    value = {"workspace_id": workspace, "tab_id": tab, "pane_id": identifier,
             "terminal_id": "term-" + identifier, "cwd": cwd}
    snapshot["panes"].append(value)
    next(layout for layout in snapshot["layouts"] if layout["tab_id"] == tab)["panes"].append({"pane_id": identifier})
    return value
def tab(workspace, cwd, label=None):
    identifier = fresh("t")
    number = sum(t["workspace_id"] == workspace for t in snapshot["tabs"]) + 1
    value = {"workspace_id": workspace, "tab_id": identifier,
             "label": label if label is not None else str(number), "number": number}
    snapshot["tabs"].append(value)
    snapshot["layouts"].append({"workspace_id": workspace, "tab_id": identifier, "panes": []})
    return value, pane(workspace, identifier, cwd)
result = None
if program == "herdr":
    snapshot = load("snapshot")["result"]["snapshot"]
    fault = load("fault") if (root / "fault.json").exists() else {}
    if args == ["status"]:
        result = load("status")
    elif args == ["api", "snapshot"]:
        result = load("snapshot")
    elif args[:2] == ["workspace", "create"]:
        workspace = {"workspace_id": fresh("w"), "label": flag("--label"), "cwd": flag("--cwd")}
        snapshot["workspaces"].append(workspace)
        created_tab, created_pane = tab(workspace["workspace_id"], flag("--cwd"))
        result = {"result": {"type": "workspace_created", "workspace": workspace, "tab": created_tab, "root_pane": created_pane}}
    elif args[:2] == ["tab", "create"]:
        created_tab, created_pane = tab(flag("--workspace"), flag("--cwd"), flag("--label") if "--label" in args else None)
        result = {"result": {"type": "tab_created", "tab": created_tab, "root_pane": created_pane}}
    elif args[:2] == ["tab", "rename"]:
        renamed = next(t for t in snapshot["tabs"] if t["tab_id"] == args[2])
        renamed["label"] = args[3]
        result = {"result": {"type": "tab_info", "tab": renamed}}
    elif args[:2] == ["pane", "split"]:
        original = next(p for p in snapshot["panes"] if p["pane_id"] == flag("--pane"))
        created = pane(original["workspace_id"], original["tab_id"], flag("--cwd"))
        result = {"result": {"type": "pane_split", "pane": created}}
    elif args[:2] == ["agent", "start"]:
        original = next(p for p in snapshot["panes"] if p["pane_id"] == flag("--pane"))
        command = args[args.index("--") + 1:]
        if fault.get("fail_start_at") == len(snapshot["agents"]) + 1:
            sys.exit(92)
        metadata = {"agent": flag("--kind"), "kind": "path", "value": command[1]}
        started = {**original, "name": args[2], "agent": flag("--kind"), "agent_session": metadata, "interactive_ready": True}
        snapshot["agents"].append(started)
        original.update(agent=started["agent"], agent_session=metadata)
        returned = dict(started)
        if fault.get("wrong_pane"):
            returned["pane_id"] = "not-the-acknowledged-pane"
        if fault.get("wrong_identity"):
            metadata["kind"] = "id"
            metadata["value"] = "wrong-live-conversation"
        result = {"result": {"type": "agent_started", "agent": returned}}
    elif args[:2] == ["workspace", "get"]:
        workspace = next((w for w in snapshot["workspaces"] if w["workspace_id"] == args[2]), None)
        if workspace:
            result = {"result": {"type": "workspace_info", "workspace": workspace}}
    elif args[:2] in (["workspace", "focus"], ["agent", "focus"]):
        result = {}
    elif args == ["pane", "current", "--current"]:
        result = load("pane")
    elif args[:2] == ["agent", "get"]:
        result = load("agent")
    save("snapshot", {"result": {"type": "session_snapshot", "snapshot": snapshot}})
    if args[:2] == ["workspace", "create"] and fault.get("revoke_authority"):
        path = Path(fault["revoke_authority"])
        local = json.loads(path.read_text())
        local["creations"].pop(fault["stream_id"])
        path.write_text(json.dumps(local))
else:
    data = load("cmux")
    if args == ["rpc", "workspace.list", "{}"]:
        result = data
    elif "new-workspace" in args:
        identifier = fresh("cmux")
        data["workspaces"].append({"id": identifier, "ref": "workspace:1", "title": flag("--name"), "index": 1, "current_directory": flag("--cwd")})
        save("cmux", data)
        result = {"workspace_id": identifier}
    elif args[:1] == ["select-workspace"]:
        result = {}
if result is None:
    with (root / "forbidden.jsonl").open("a") as log:
        log.write(json.dumps([program, args]) + "\n")
    sys.exit(91)
print(json.dumps(result))
'''


class RustNativeWorkspaceDifferentialTests(NativeFixture):
    def setUp(self):
        super().setUp()
        for name in ("herdr", "cmux"):
            (self.base / "tools" / name).write_text(f"#!{sys.executable}\n" + FAKE_WORKSPACE)
        self.put("snapshot", {"result": {"type": "session_snapshot", "snapshot": {
            "workspaces": [], "tabs": [], "panes": [], "layouts": [], "agents": []}}})
        executable = self.base / "tools/pi"
        executable.write_text(f"#!{sys.executable}\nimport json, os, sys\nprint(json.dumps({{'argv':sys.argv[1:],'cwd':os.getcwd(),'stream_id':os.environ.get('STREAMS_STREAM_ID')}}))\n")
        executable.chmod(0o700)

    def transcript(self, native="native-local"):
        path = Path(self.env.get("PI_CODING_AGENT_DIR", self.home / ".pi/agent")) / "sessions/fixture" / (native + ".jsonl")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({"type": "session", "id": native}) + "\n")
        return path

    def prepare(self):
        self.create_session()
        self.transcript()
        self.herdr()

    def effects(self, prefix):
        return [json.loads(line) for line in self.commands() if json.loads(line)[1][:len(prefix)] == prefix]

    def open(self, implementation="rust", *flags):
        return self.invoke(implementation, "open", self.stream["id"], *flags)

    def saved_plan(self):
        path = self.state / "arrangements" / self.scope / (key(self.stream["id"]) + ".json")
        return path, json.loads(path.read_text())

    def save_layout(self, groups, label="Saved layout"):
        path = self.state / "arrangements" / self.scope / (key(self.stream["id"]) + ".json")
        path.parent.mkdir(parents=True, exist_ok=True)
        tabs = [{"tab_id": f"saved-{i}", "label": label, "label_mode": "manual", "sessions": [{"session_id": s["id"], "harness": s["harness"], "native_id": s["harness_session_id"], "locator": str(self.transcript(s["harness_session_id"]))} for s in group]} for i, group in enumerate(groups)]
        path.write_text(json.dumps({"version": 1, "stream_id": self.stream["id"], "workspace_id": "saved-workspace", "tabs": tabs}))

    def attach(self, native):
        return self.call("python", "session", "attach", self.stream["id"], "--harness", "pi", "--session-id", native, "--title", native)["session"]

    def test_rust_restores_exact_conversation_and_python_can_focus_and_capture(self):
        self.prepare()
        code, opened = self.open()
        self.assertEqual(code, 0, opened)
        self.assertEqual(opened["runtime"]["restored"], [self.session["id"]])
        self.assertFalse(opened["runtime"]["reused"])
        self.assertEqual(len(self.effects(["agent", "start"])), 1)
        python = self.call("python", "open", self.stream["id"])
        rust = self.call("rust", "open", self.stream["id"])
        self.assertEqual(rust, python)
        self.assertTrue(rust["runtime"]["reused"])
        self.assertEqual(len(self.effects(["agent", "start"])), 1)
        python = self.call("python", "workspace", "capture", self.stream["id"])
        rust = self.call("rust", "workspace", "capture", self.stream["id"])
        self.assertEqual(rust, python)
        _, plan = self.saved_plan()
        self.assertEqual(plan["version"], 1)
        self.assertEqual(plan["tabs"][0]["sessions"][0]["locator"], str(self.transcript()))

    def test_python_open_state_is_reused_without_rust_relaunch(self):
        self.prepare()
        code, python = self.open("python")
        self.assertEqual(code, 0, python)
        code, rust = self.open()
        self.assertEqual(code, 0, rust)
        self.assertTrue(rust["runtime"]["reused"])
        self.assertEqual(len(self.effects(["workspace", "create"])), 1)
        self.assertEqual(len(self.effects(["agent", "start"])), 1)

    def test_lost_launch_acknowledgement_fences_retries_and_preserves_resources(self):
        self.prepare()
        self.put("fault", {"wrong_pane": True})
        code, failed = self.open()
        self.assertEqual(code, 2, failed)
        self.assertIn("may be partial", failed["error"]["message"])
        attempts = list((self.state / "attempts" / self.scope).glob("*.json"))
        self.assertEqual(len(attempts), 1)
        attempt = json.loads(attempts[0].read_text())
        self.assertFalse(attempt["launches"][0]["ready"])
        self.assertTrue(any(c["outcome"] == "uncertain" for c in attempt["effects"][-1]["error"]["commands"]))
        self.assertEqual(len(self.effects(["workspace", "create"])), 1)
        for implementation in ("rust", "python"):
            code, failed = self.open(implementation)
            self.assertEqual(code, 2, failed)
        self.assertEqual(len(self.effects(["agent", "start"])), 1)
        self.assertEqual(len(self.effects(["workspace", "create"])), 1)

    def test_ready_receipt_does_not_substitute_for_live_conversation_identity(self):
        self.prepare()
        self.put("fault", {"wrong_identity": True})
        code, failed = self.open()
        self.assertEqual(code, 2, failed)
        self.assertIn("not live", failed["error"]["message"])
        attempt = json.loads(next((self.state / "attempts" / self.scope).glob("*.json")).read_text())
        self.assertFalse(attempt["launches"][0]["ready"])
        associations = list((self.state / "associations" / self.scope).glob("*.json"))
        self.assertFalse(json.loads(associations[0].read_text())["ready"])
        self.assertEqual(self.open()[0], 2)
        self.assertEqual(len(self.effects(["agent", "start"])), 1)

    def test_closed_workspace_restores_manual_tab_and_split_order(self):
        self.prepare()
        second_native = "second-native"
        self.transcript(second_native)
        second = self.call("python", "session", "attach", self.stream["id"], "--harness", "pi", "--session-id", second_native, "--title", "Second")["session"]
        # First open is deliberately ambiguous: no arbitrary session launches.
        code, initial = self.open()
        self.assertEqual(code, 2, initial)
        self.assertEqual(initial["runtime"]["restored"], [])
        self.assertFalse(self.effects(["agent", "start"]))
        workspace = initial["runtime"]["workspace_id"]
        path = self.state / "arrangements" / self.scope / (key(self.stream["id"]) + ".json")
        path.parent.mkdir(parents=True, exist_ok=True)
        entries = [{"session_id": s["id"], "harness": "pi", "native_id": s["harness_session_id"], "locator": str(self.transcript(s["harness_session_id"]))} for s in (self.session, second)]
        path.write_text(json.dumps({"version": 1, "stream_id": self.stream["id"], "workspace_id": workspace, "tabs": [{"tab_id": "old-tab", "label": "Manual layout", "label_mode": "manual", "sessions": entries}]}))
        self.put("snapshot", {"result": {"type": "session_snapshot", "snapshot": {"workspaces": [], "tabs": [], "panes": [], "layouts": [], "agents": []}}})
        code, restored = self.open()
        self.assertEqual(code, 0, restored)
        self.assertEqual(restored["runtime"]["restored"], [self.session["id"], second["id"]])
        self.assertNotEqual(restored["runtime"]["workspace_id"], workspace)
        self.assertEqual(len(self.effects(["pane", "split"])), 1)
        self.assertEqual(len(self.effects(["tab", "rename"])), 1)
        self.assertEqual(len(list((self.state / "closed" / self.scope).glob("*.json"))), 1)
        python = self.call("python", "workspace", "capture", self.stream["id"])
        rust = self.call("rust", "workspace", "capture", self.stream["id"])
        self.assertEqual(rust, python)
        self.assertEqual(rust["arrangement"]["tabs"][0]["label"], "Manual layout")

    def test_partial_second_launch_retains_verified_first_and_fences_both_implementations(self):
        self.prepare()
        second = self.attach("second")
        self.save_layout([[self.session, second]])
        self.put("fault", {"fail_start_at": 2})
        code, failed = self.open()
        self.assertEqual(code, 2, failed)
        attempt = json.loads(next((self.state / "attempts" / self.scope).glob("*.json")).read_text())
        self.assertEqual([launch["ready"] for launch in attempt["launches"]], [True, False])
        self.assertEqual([launch["session_id"] for launch in attempt["launches"]], [self.session["id"], second["id"]])
        self.assertEqual(len(self.effects(["agent", "start"])), 2)
        for implementation in ("rust", "python"):
            self.assertEqual(self.open(implementation)[0], 2)
        self.assertEqual(len(self.effects(["agent", "start"])), 2)

    def test_wrong_first_live_identity_stops_before_second_pane_or_launch(self):
        self.prepare()
        second = self.attach("second")
        self.save_layout([[self.session, second]])
        self.put("fault", {"wrong_identity": True})
        code, failed = self.open()
        self.assertEqual(code, 2, failed)
        self.assertIn("not live", failed["error"]["message"])
        self.assertEqual(len(self.effects(["agent", "start"])), 1)
        self.assertFalse(self.effects(["pane", "split"]))
        self.assertFalse(self.effects(["tab", "create"]))
        attempt = json.loads(next((self.state / "attempts" / self.scope).glob("*.json")).read_text())
        self.assertEqual([launch["ready"] for launch in attempt["launches"]], [False])

    def test_workspace_tab_and_split_pin_only_bounded_caller_environment(self):
        self.prepare()
        self.env.update(PI_CODING_AGENT_DIR=str(self.home / "caller-pi"), CODEX_HOME=str(self.home / "caller-codex"), CLAUDE_CONFIG_DIR=str(self.home / "caller-claude"), PI_OFFLINE="1", SHOULD_NOT_FORWARD="not-a-selector")
        second, third = self.attach("second"), self.attach("third")
        self.save_layout([[self.session, second], [third]])
        code, opened = self.open()
        self.assertEqual(code, 0, opened)
        expected = {key: self.env[key] for key in ("HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "PI_CODING_AGENT_DIR", "CODEX_HOME", "CLAUDE_CONFIG_DIR", "PATH", "HERDR_BIN_PATH", "HERDR_SOCKET_PATH", "PI_OFFLINE")}
        expected["STREAMS_STREAM_ID"] = self.stream["id"]
        for prefix in (["workspace", "create"], ["tab", "create"], ["pane", "split"]):
            effects = self.effects(prefix)
            self.assertEqual(len(effects), 1)
            args = effects[0][1]
            actual = dict(args[i + 1].split("=", 1) for i, value in enumerate(args) if value == "--env")
            self.assertEqual(actual, expected)

    def assert_blank_label_uses_automatic_naming(self, label):
        self.prepare()
        self.save_layout([[self.session]], label)
        code, opened = self.open()
        self.assertEqual(code, 0, opened)
        self.assertFalse(self.effects(["tab", "rename"]))
        _, saved = self.saved_plan()
        self.assertIsNone(saved["tabs"][0]["label"])
        self.assertEqual(saved["tabs"][0]["label_mode"], "automatic")

    def test_whitespace_only_tab_label_does_not_fence_a_created_workspace(self):
        self.assert_blank_label_uses_automatic_naming("  ")

    def test_empty_tab_label_restores_with_a_consistent_automatic_record(self):
        self.assert_blank_label_uses_automatic_naming("")

    def test_start_reports_success_or_retains_accepted_save_on_native_failure(self):
        self.herdr()
        started = self.call("rust", "start", "Empty native workspace")
        self.assertTrue(started["save"]["domain"])
        self.assertFalse(started["runtime"]["reused"])
        code, failed = self.invoke("rust", "start", "Saved despite native selection", "--frontend", "third-party")
        self.assertEqual(code, 2, failed)
        self.assertIn("error", failed["runtime"])
        self.assertEqual(self.call("python", "show", failed["stream"]["id"])["stream"], failed["stream"])

    def test_cmux_single_session_launch_and_reuse_match_python(self):
        self.create_session()
        self.env["PI_CODING_AGENT_DIR"] = str(self.home / "agent's files with spaces")
        self.transcript()
        self.call("python", "config", "set", "frontend.mode", "cmux")
        self.env["CMUX_SOCKET_PATH"] = str(self.base / "not-a-live-cmux.sock")
        self.put("cmux", {"workspaces": []})
        code, created = self.open()
        self.assertEqual(code, 0, created)
        self.assertEqual(created["runtime"]["arrangement_support"], "single-session-only")
        commands = [json.loads(line)[1] for line in self.commands() if "new-workspace" in json.loads(line)[1]]
        self.assertEqual(len(commands), 1)
        self.assertIn("--command", commands[0])
        self.assertEqual(shlex.split(commands[0][commands[0].index("--command") + 1]), ["pi", "--session", str(self.transcript())])
        args = commands[0]
        environment = dict(args[i + 1].split("=", 1) for i, value in enumerate(args) if value == "--env")
        self.assertEqual(environment["PI_CODING_AGENT_DIR"], self.env["PI_CODING_AGENT_DIR"])
        self.assertEqual(environment["XDG_CONFIG_HOME"], self.env["XDG_CONFIG_HOME"])
        self.assertNotIn("CMUX_WORKSPACE_ID", environment)
        python = self.call("python", "open", self.stream["id"])
        rust = self.call("rust", "open", self.stream["id"])
        self.assertEqual(rust, python)

    def test_inactive_session_requires_explicit_selection_and_reuses_existing_workspace(self):
        self.prepare()
        self.call("python", "session", "update", self.session["id"], "--status", "done")
        code, empty = self.open()
        self.assertEqual(code, 0, empty)
        self.assertEqual(empty["runtime"]["restored"], [])
        self.assertFalse(self.effects(["agent", "start"]))
        code, selected = self.open("rust", "--session", self.session["id"])
        self.assertEqual(code, 0, selected)
        self.assertTrue(selected["runtime"]["reused"])
        self.assertEqual(selected["runtime"]["restored"], [self.session["id"]])
        self.assertEqual(len(self.effects(["tab", "create"])), 1)
        self.assertEqual(self.call("python", "open", self.stream["id"]), self.call("rust", "open", self.stream["id"]))

    def test_revoked_authority_between_effects_prevents_any_agent_start(self):
        self.prepare()
        self.put("fault", {"revoke_authority": str(self.catalog / ".catalog-sync/local.json"), "stream_id": self.stream["id"]})
        code, failed = self.open()
        self.assertEqual(code, 2, failed)
        self.assertIn("authority", failed["error"]["message"])
        self.assertEqual(len(self.effects(["workspace", "create"])), 1)
        self.assertFalse(self.effects(["agent", "start"]))
        self.assertEqual(self.open()[0], 2)
        self.assertEqual(len(self.effects(["workspace", "create"])), 1)

    def test_catalog_owner_fields_without_effective_authority_never_grant_effects(self):
        self.prepare()
        path = self.catalog / ".catalog-sync/local.json"
        local = json.loads(path.read_text())
        local["creations"].pop(self.stream["id"])
        path.write_text(json.dumps(local))
        for implementation in ("rust", "python"):
            code, failed = self.open(implementation)
            self.assertEqual(code, 2, failed)
        self.assertFalse(self.effects(["workspace", "create"]))
        self.assertFalse(self.effects(["agent", "start"]))

    def test_closed_binding_is_not_retired_on_unverifiable_host_inventory(self):
        self.prepare()
        self.assertEqual(self.open()[0], 0)
        before = self.files()
        member = {"workspace_id": "other", "tab_id": "other-tab", "pane_id": "other-pane", "terminal_id": "other-terminal", "cwd": str(self.cwd)}
        agent = {**member, "agent": "pi", "agent_session": {"agent": "pi", "kind": "path", "value": str(self.base / "missing.jsonl")}}
        snapshot = {"workspaces": [{"workspace_id": "other", "label": "Other"}], "tabs": [{"workspace_id": "other", "tab_id": "other-tab", "label": "Other"}], "panes": [member], "layouts": [{"workspace_id": "other", "tab_id": "other-tab", "panes": [{"pane_id": "other-pane"}]}], "agents": [agent]}
        self.put("snapshot", {"result": {"type": "session_snapshot", "snapshot": snapshot}})
        for implementation in ("rust", "python"):
            code, failed = self.open(implementation)
            self.assertEqual(code, 2, failed)
            self.assertEqual(self.files(), before)
        self.assertEqual(len(self.effects(["workspace", "create"])), 1)

    def test_unavailable_saved_file_is_reported_without_an_arbitrary_launch(self):
        self.prepare()
        self.transcript().unlink()
        code, opened = self.open()
        self.assertEqual(code, 2, opened)
        self.assertEqual(opened["runtime"]["restored"], [])
        self.assertEqual(opened["runtime"]["skipped"], opened["runtime"]["unavailable"])
        self.assertEqual(opened["runtime"]["skipped"][0]["session_id"], self.session["id"])
        self.assertFalse(self.effects(["agent", "start"]))
        code, existing = self.open("rust", "--session", self.session["id"])
        self.assertEqual(code, 2, existing)
        self.assertTrue(existing["runtime"]["reused"])
        self.assertEqual(len(self.effects(["workspace", "create"])), 1)

    def test_resume_exec_is_exact_and_does_not_need_a_native_frontend(self):
        self.create_session()
        transcript = self.transcript()
        python = self.call("python", "resume", self.stream["id"])
        rust = self.call("rust", "resume", self.stream["id"])
        self.assertEqual(rust, python)
        self.assertEqual(rust, {"argv": ["--session", str(transcript)], "cwd": str(self.cwd), "stream_id": self.stream["id"]})

    def test_resume_rejects_a_conversation_already_live_in_herdr(self):
        self.prepare()
        self.assertEqual(self.open()[0], 0)
        for implementation in ("python", "rust"):
            code, failed = self.invoke(implementation, "resume", self.stream["id"])
            self.assertEqual(code, 2, failed)
            self.assertIn("already running", failed["error"]["message"])
