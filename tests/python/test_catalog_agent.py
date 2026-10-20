from __future__ import annotations

import io
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams import catalog_config
from _streams.cli import main
from _streams.catalog.sync import LocalCatalog


class CatalogAgentTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.enterContext(patch.dict(os.environ, {
            "HOME": str(self.root), "XDG_CONFIG_HOME": str(self.root / "config"),
            "XDG_DATA_HOME": str(self.root / "data"), "XDG_STATE_HOME": str(self.root / "state"),
        }, clear=True))
        self.assertEqual(self.call("init", "--sync", "none")[0], 0)
        self.assertEqual(self.call("config", "set", "frontend.mode", "none")[0], 0)

    def call(self, *args, request=None, raw=None):
        out, err = io.StringIO(), io.StringIO()
        with patch("sys.stdout", out), patch("sys.stderr", err), patch("sys.stdin", io.StringIO(raw if raw is not None else json.dumps(request))):
            try:
                code = main([*args, "--json"])
            except SystemExit as error:
                code = error.code
        return code, json.loads(out.getvalue()) if out.getvalue() else {}, err.getvalue()

    def enroll(self, native="native", **changes):
        request = {"operation_id": "enroll-" + native, "harness": "pi", "harness_session_id": native,
                   "stream": {"title": "Catalog integration", "goal": "Safe automatic progress"},
                   "session": {"title": "Port the extension"}, "actor": "automatic", "cwd": str(self.root),
                   "catalog_source": self.context(native)["catalog_source"], **changes}
        code, result, error = self.call("agent", "enroll", request=request)
        self.assertEqual((code, error), (0, ""), result)
        return result

    def context(self, native="native", **kwargs):
        args = ["agent", "context", "--harness", "pi", "--session-id", native]
        if "stream_id" in kwargs:
            args.extend(["--stream-id", kwargs["stream_id"]])
        code, result, error = self.call(*args)
        self.assertEqual((code, error), (0, ""), result)
        return result

    def apply(self, context, changes, *, kind="session", actor="automatic", operation_id="report", inputs=None):
        return self.call("agent", "apply", request={
            "operation_id": operation_id, "catalog_source": context["catalog_source"],
            "kind": kind, "actor": actor, "before": context[kind], "changes": changes,
            "ownership_token": context["authority"]["token"] if kind == "session" or actor == "automatic" else None,
            "inputs": inputs if inputs is not None else ([{"kind": "stream", "record": context["stream"]}] if kind == "session" else []),
        })

    def test_unattached_local_context_neither_fetches_nor_creates_records(self):
        with patch.object(LocalCatalog, "read", side_effect=AssertionError("projected read")), patch.object(LocalCatalog, "sync", side_effect=AssertionError("publication")):
            result = self.context()
        self.assertIsNone(result["stream"])
        self.assertIsNone(result["session"])
        self.assertIsNone(result["authority"])
        self.assertEqual(result["sessions"], [])
        self.assertEqual(self.call("list")[1]["streams"], [])

    def test_automatic_enrollment_and_repeated_attach_use_the_same_durable_identity(self):
        first = self.enroll()
        second = self.enroll(operation_id="second-attempt", stream={"title": "Must not overwrite"})
        current = self.context()
        self.assertEqual((second["stream"], second["session"]), (first["stream"], first["session"]))
        self.assertEqual(current["stream"], first["stream"])
        self.assertEqual(current["session"], first["session"])
        self.assertEqual(current["stream"]["title_mode"], "automatic")
        self.assertEqual(len(current["sessions"]), 1)
        self.assertEqual(current["authority"]["host_id"], current["host_id"])
        self.assertEqual(len(self.call("list")[1]["streams"]), 1)

    def test_herdr_helper_rejected_before_any_catalog_or_runtime_mutation_but_user_can_enroll(self):
        from _streams.cli import runtime
        from _streams.herdr import HerdrPane, HerdrWorkspace
        from _streams.herdr import HerdrEndpoint
        source = self.context('helper')['catalog_source']
        self.assertEqual(self.call('config', 'set', 'frontend.mode', 'herdr')[0], 0)
        # A persisted helper session is real, but is not the session in the parent pane.
        helper_path = self.root / 'helper.jsonl'
        helper_path.write_text(json.dumps({'type': 'session', 'id': 'helper'}) + '\n')
        parent_path = self.root / 'parent.jsonl'
        parent_path.write_text(json.dumps({'type': 'session', 'id': 'parent'}) + '\n')
        request = {'operation_id': 'helper-enroll', 'catalog_source': source, 'harness': 'pi',
                   'harness_session_id': 'helper', 'stream': {'title': 'Helper investigation'},
                   'session': {'title': 'Helper'}, 'actor': 'automatic', 'cwd': str(self.root)}
        with patch('_streams.catalog_runtime.HerdrClient') as transport, patch.dict(os.environ, {
            'HERDR_ENV': '1', 'HERDR_PANE_ID': 'wH:p1A',
        }):
            client = transport.return_value
            client.endpoint.return_value = HerdrEndpoint('/test.sock', None)
            client.current_pane.return_value = HerdrPane('wH:p1A', 'wH:t1', 'wH', '')
            client.resolve_workspace.return_value = HerdrWorkspace('wH', 'Parent')
            agent = {'pane_id': 'wH:p1A', 'workspace_id': 'wH', 'agent': 'pi',
                     'agent_session': {'agent': 'pi', 'kind': 'path', 'value': str(parent_path)}}
            client.command_result.return_value = {'agent': agent}
            before = {path.relative_to(self.root): path.read_bytes()
                      for path in self.root.rglob('*') if path.is_file()}
            code, result, _ = self.call('agent', 'enroll', request=request)
            self.assertEqual(code, 2, result)
            self.assertIn('background helper cannot automatically enroll', result['error']['message'])
            self.assertFalse(runtime().state_dir.exists())
            self.assertEqual(before, {path.relative_to(self.root): path.read_bytes()
                                      for path in self.root.rglob('*') if path.is_file()})
            self.assertEqual(self.call('list')[1]['streams'], [])
            self.assertIsNone(self.context('helper')['session'])
            client.command_result.assert_called_once_with(['agent', 'get', 'wH:p1A'], 'agent_info')
            client.command_result.reset_mock()
            code, result, _ = self.call('agent', 'enroll', request={**request, 'actor': 'user'})
            self.assertEqual(code, 0, result)
            self.assertEqual(result['session']['harness_session_id'], 'helper')
            client.command_result.assert_not_called()
            client.run.assert_not_called()

    def test_automatic_herdr_fork_and_resume_enroll_the_actual_pane_session(self):
        from _streams.herdr import HerdrPane, HerdrWorkspace
        from _streams.herdr import HerdrEndpoint
        self.assertEqual(self.call('config', 'set', 'frontend.mode', 'herdr')[0], 0)
        path = self.root / 'original-filename.jsonl'
        with patch('_streams.catalog_runtime.HerdrClient') as transport, patch.dict(os.environ, {
            'HERDR_ENV': '1', 'HERDR_PANE_ID': 'wH:p1A',
        }):
            client = transport.return_value
            client.endpoint.return_value = HerdrEndpoint('/test.sock', None)
            client.current_pane.return_value = HerdrPane('wH:p1A', 'wH:t1', 'wH', '')
            client.resolve_workspace.return_value = HerdrWorkspace('wH', 'Parent')
            for native_id, kind in (('fork-uuid', 'path'), ('resumed-uuid', 'path'), ('explicit-uuid', 'id')):
                with self.subTest(native_id=native_id):
                    path.write_text(json.dumps({'type': 'session', 'id': native_id}) + '\n')
                    client.command_result.return_value = {'agent': {
                        'pane_id': 'wH:p1A', 'workspace_id': 'wH', 'agent': 'pi',
                        'agent_session': {'agent': 'pi', 'kind': kind,
                                          'value': str(path) if kind == 'path' else native_id},
                    }}
                    result = self.enroll(native_id)
                    self.assertEqual(result['session']['harness_session_id'], native_id)
            self.assertEqual(len(self.call('list')[1]['streams']), 1)
            client.run.assert_not_called()

    def test_initial_explicit_attachment_keeps_manual_stream_title_but_accepts_generated_goal(self):
        _, created, _ = self.call("create", "Human title")
        enrolled = self.enroll(stream_id=created["stream"]["id"], stream=None)
        self.assertEqual(enrolled["stream"]["title_mode"], "manual")
        code, result, _ = self.apply(enrolled, {"title": "Ignore generated title", "goal": "An inferred goal", "goal_short": "Inference"}, kind="stream")
        self.assertEqual(code, 0, result)
        self.assertEqual(result["stream"]["title"], "Human title")
        self.assertEqual(result["stream"]["goal"], "An inferred goal")
        self.assertEqual(result["stream"]["goal_mode"], "automatic")

    def test_captured_target_is_not_refreshed_after_inference(self):
        before = self.enroll()
        self.assertEqual(self.apply(before, {"state": "Human update"}, actor="user", operation_id="human")[0], 0)
        code, result, _ = self.apply(before, {"state": "Stale generated update"})
        self.assertEqual(code, 2)
        self.assertEqual(result["save"]["domain"]["outcome"], "superseded")
        self.assertEqual(self.context()["session"]["state"], "Human update")

    def test_captured_other_record_is_checked_before_report_application(self):
        before = self.enroll()
        self.assertEqual(self.apply(before, {"goal": "Changed scope"}, kind="stream", actor="user", operation_id="goal")[0], 0)
        code, result, _ = self.apply(before, {"state": "Based on old goal"})
        self.assertEqual((code, result["save"]["domain"]["outcome"]), (2, "superseded"))
        self.assertEqual(self.context()["session"]["state"], "")

    def test_user_edit_uses_dirty_fields_and_retains_independent_automatic_changes(self):
        before = self.enroll()
        self.assertEqual(self.apply(before, {"state": "Report", "state_short": "Ready"})[0], 0)
        code, result, _ = self.apply(before, {"title": "Human contribution"}, actor="user", operation_id="title", inputs=[])
        self.assertEqual(code, 0, result)
        self.assertEqual(result["session"]["state"], "Report")
        self.assertEqual(result["session"]["state_short"], "Ready")
        self.assertEqual(result["session"]["title_mode"], "manual")
        code, result, _ = self.apply(self.context(), {"title_mode": "automatic"}, actor="user", operation_id="unprotect", inputs=[])
        self.assertEqual(code, 0, result)
        self.assertEqual(result["session"]["title_mode"], "automatic")

    def test_compact_descriptions_are_accepted_and_cleared_with_their_sources(self):
        before = self.enroll()
        self.assertEqual(self.apply(before, {"state": "Detailed", "state_short": "Compact"})[0], 0)
        code, result, _ = self.apply(self.context(), {"remaining": "New task"}, actor="user", operation_id="remaining", inputs=[])
        self.assertEqual(code, 0, result)
        self.assertIsNone(result["session"]["state_short"])

    def test_operation_identity_and_captured_inputs_are_stable_across_retries(self):
        before = self.enroll()
        first = self.apply(before, {"state": "Ready"})
        second = self.apply(before, {"state": "Ready"})
        self.assertEqual((first[0], second[0]), (0, 0))
        self.assertEqual(second[1]["operation_id"], "report")
        self.assertEqual(self.apply(before, {"state": "Different intent"})[0], 2)
        self.assertEqual(self.context()["session"]["state"], "Ready")

    def test_bound_native_conversation_cannot_be_switched_by_explicit_stream(self):
        before = self.enroll()
        _, other, _ = self.call("create", "Other")
        code, result, _ = self.call("agent", "context", "--harness", "pi", "--session-id", "native", "--stream-id", other["stream"]["id"])
        self.assertEqual(code, 2)
        self.assertIn("move", result["error"]["message"])
        request = {"operation_id": "switch", "harness": "pi", "harness_session_id": "native", "stream_id": other["stream"]["id"],
                   "session": {"title": "Other"}, "actor": "user", "cwd": str(self.root)}
        self.assertEqual(self.call("agent", "enroll", request=request)[0], 2)
        self.assertEqual(self.context()["stream"], before["stream"])

    def test_invalid_generated_or_manual_changes_never_reach_persistence(self):
        before = self.enroll()
        for changes, kind, actor in (({"status": "done"}, "stream", "automatic"), ({"archived": True}, "stream", "automatic"),
                                      ({"title": ""}, "session", "user"), ({"status": "unknown"}, "session", "user"),
                                      ({"stream_id": "different"}, "session", "user"), ({"repositories": {}}, "stream", "user")):
            with self.subTest(changes=changes):
                self.assertEqual(self.apply(before, changes, kind=kind, actor=actor)[0], 2)
        self.assertEqual(self.context()["session"], before["session"])

    def test_sharing_attention_does_not_fail_apply_or_enrollment_but_fails_explicit_sync(self):
        from _streams.catalog.sync import Result
        self.enroll()
        for sharing in ("pending", "blocked", "failed", "uncertain"):
            with self.subTest(sharing=sharing):
                before = self.context()
                with patch("_streams.cli.maybe_sync", return_value=Result(sharing)):
                    code, result, _ = self.apply(before, {"state": sharing}, operation_id=sharing)
                    self.assertEqual(code, 0, result)
                    self.assertEqual(result["publication"]["sharing"], sharing)
                    enrolled = self.enroll(native=sharing)
                    self.assertEqual(enrolled["publication"]["sharing"], sharing)
                    self.assertEqual(self.call("sync")[0], 2 if sharing != "pending" else 0)

    def test_enrollment_publication_exception_preserves_all_accepted_saves(self):
        request = {"operation_id": "interrupted-enroll", "catalog_source": self.context()["catalog_source"],
                   "harness": "pi", "harness_session_id": "native", "actor": "automatic", "cwd": str(self.root),
                   "stream": {"title": "Local effort"}, "session": {"title": "Local contribution"}}
        with patch("_streams.cli.maybe_sync", side_effect=OSError("publication interrupted")):
            code, result, _ = self.call("agent", "enroll", request=request)
        self.assertEqual(code, 2)
        self.assertEqual(len(result["saves"]), 2)
        self.assertTrue(all(item["save"]["local"] == "persisted" for item in result["saves"]))
        self.assertIn("local save succeeded", result["completion_error"]["detail"])
        self.assertEqual(self.context()["session"], result["session"])

    def test_publication_failure_preserves_a_successful_local_save(self):
        before = self.enroll()
        with patch("_streams.cli.maybe_sync", side_effect=OSError("publication interrupted")):
            code, result, _ = self.apply(before, {"state": "Saved locally"})
        self.assertEqual(code, 2)
        self.assertEqual(result["save"]["domain"]["outcome"], "changes")
        self.assertIn("local save succeeded", result["completion_error"]["detail"])
        self.assertEqual(self.context()["session"]["state"], "Saved locally")

    def test_context_includes_readonly_calling_workspace_identity_and_rejects_mismatch(self):
        first = self.enroll()
        _, other, _ = self.call("create", "Other")
        from _streams.catalog_runtime import Runtime
        with patch.object(Runtime, "context_stream_id", return_value=other["stream"]["id"]):
            code, result, _ = self.call("agent", "context", "--harness", "pi", "--session-id", "native")
            self.assertEqual(code, 2)
            self.assertIn("move", result["error"]["message"])
            unbound = self.context("second-native")
            self.assertEqual(unbound["stream"], other["stream"])
            self.assertIsNone(unbound["session"])
        self.assertEqual(self.context()["stream"], first["stream"])

    def test_user_enrollment_recovers_its_own_partial_save_without_creating_another_effort(self):
        from _streams.catalog_runtime import Enrollment
        request = {"operation_id": "manual-create", "catalog_source": self.context()["catalog_source"],
                   "harness": "pi", "harness_session_id": "native", "stream": {"title": "Manual effort"},
                   "session": {"title": "Manual contribution"}, "actor": "user", "cwd": str(self.root)}
        with patch.object(Enrollment, "bind", side_effect=OSError("binding unavailable")):
            code, first, _ = self.call("agent", "enroll", request=request)
            self.assertEqual(code, 2)
            self.assertEqual(len(first["saves"]), 2)
            code, retry, _ = self.call("agent", "enroll", request=request)
            self.assertEqual(code, 2)
            self.assertEqual(retry["stream"], first["stream"])
            self.assertEqual(retry["session"], first["session"])
            self.assertIn("completion_error", retry)
        code, recovered, _ = self.call("agent", "enroll", request=request)
        self.assertEqual(code, 0, recovered)
        self.assertEqual(recovered["session"], first["session"])
        self.assertEqual(len(self.call("list")[1]["streams"]), 1)
        changed = {**request, "stream": {"title": "Reinterpreted intent"}}
        self.assertEqual(self.call("agent", "enroll", request=changed)[0], 2)
        self.assertEqual(self.context()["stream"]["title"], "Manual effort")

    def test_user_enrollment_recovers_after_only_its_stream_save_succeeded(self):
        from _streams.catalog.model import Session
        from _streams.catalog.sync import LocalCatalog
        original = LocalCatalog.save
        def fail_session(backend, operation, **kwargs):
            if isinstance(operation.after, Session):
                raise OSError("session save unavailable")
            return original(backend, operation, **kwargs)
        request = {"operation_id": "partial-create", "catalog_source": self.context()["catalog_source"],
                   "harness": "pi", "harness_session_id": "native", "stream": {"title": "Partial effort"},
                   "session": {"title": "Contribution"}, "actor": "user", "cwd": str(self.root)}
        with patch.object(LocalCatalog, "save", autospec=True, side_effect=fail_session):
            code, first, _ = self.call("agent", "enroll", request=request)
        self.assertEqual(code, 2)
        self.assertEqual(len(first["saves"]), 1)
        changed = {**request, "stream": {"title": "Different intent"}}
        self.assertEqual(self.call("agent", "enroll", request=changed)[0], 2)
        code, recovered, _ = self.call("agent", "enroll", request=request)
        self.assertEqual(code, 0, recovered)
        self.assertEqual(recovered["stream"], first["stream"])
        self.assertIsNotNone(recovered["session"])
        self.assertEqual(len(self.call("list")[1]["streams"]), 1)

    def test_failed_attachment_without_any_save_does_not_claim_an_enrollment_receipt(self):
        from _streams.catalog.sync import LocalCatalog
        _, created, _ = self.call("create", "Existing effort")
        request = {"operation_id": "failed-attach", "catalog_source": self.context()["catalog_source"],
                   "harness": "pi", "harness_session_id": "native", "stream_id": created["stream"]["id"],
                   "session": {"title": "Contribution"}, "actor": "user", "cwd": str(self.root)}
        with patch.object(LocalCatalog, "save", side_effect=OSError("session save unavailable")):
            code, result, _ = self.call("agent", "enroll", request=request)
        self.assertEqual(code, 2)
        self.assertEqual(result["error"]["message"], "session save unavailable")
        self.assertIsNone(self.context()["session"])

    def test_clear_goal_clears_its_compact_derivative_at_the_parse_boundary(self):
        before = self.enroll()
        self.assertEqual(self.apply(before, {"goal": "Detailed goal", "goal_short": "Compact"}, kind="stream")[0], 0)
        code, result, _ = self.apply(self.context(), {"goal": None}, kind="stream", actor="user", operation_id="clear")
        self.assertEqual(code, 0, result)
        self.assertIsNone(result["stream"]["goal"])
        self.assertIsNone(result["stream"]["goal_short"])

    def test_enrollment_checks_publication_once_after_related_saves(self):
        from _streams.cli import maybe_sync
        with patch("_streams.cli.maybe_sync", wraps=maybe_sync) as sync:
            enrolled = self.enroll()
        self.assertEqual(len(enrolled["saves"]), 2)
        self.assertEqual(sync.call_count, 1)
        self.assertEqual(sync.call_args.args[1], "attach")

    def test_repair_required_publication_retains_enrollment_saves_but_fails_completion(self):
        from _streams.catalog.sync import Result
        request = {"operation_id": "repair-enroll", "catalog_source": self.context()["catalog_source"],
                   "harness": "pi", "harness_session_id": "native", "actor": "automatic", "cwd": str(self.root),
                   "stream": {"title": "Saved before publication"}, "session": {"title": "Retained"}}
        with patch("_streams.cli.maybe_sync", return_value=Result("blocked", local="repair-required", detail="inspect adoption")):
            code, result, _ = self.call("agent", "enroll", request=request)
        self.assertEqual(code, 2)
        self.assertEqual(len(result["saves"]), 2)
        self.assertTrue(all(item["save"]["local"] == "persisted" for item in result["saves"]))
        self.assertEqual(result["publication"]["local"], "repair-required")
        self.assertEqual(self.context()["session"]["id"], result["session"]["id"])

    def test_repair_required_publication_stops_apply_completion_without_losing_save(self):
        from _streams.catalog.sync import Result
        before = self.enroll()
        with patch("_streams.cli.maybe_sync", return_value=Result("confirmed", local="repair-required", detail="inspect adoption")):
            code, result, _ = self.apply(before, {"state": "Saved before repair"})
        self.assertEqual(code, 2)
        self.assertEqual(result["save"]["local"], "persisted")
        self.assertEqual(result["publication"]["sharing"], "confirmed")
        self.assertEqual(result["publication"]["local"], "repair-required")
        self.assertEqual(self.context()["session"]["state"], "Saved before repair")

    def test_enrollment_preserves_a_stream_default_directory(self):
        from _streams.cli import runtime
        enrolled = self.enroll()
        local = runtime()
        # Runtime lookup with no eligible session still has a useful cwd.
        self.assertEqual(str(local.locations.read(enrolled["stream"]["id"], None).cwd), str(self.root.resolve()))

    def test_user_supplied_title_selects_manual_mode_even_when_unchanged(self):
        before = self.enroll()
        code, result, _ = self.apply(before, {"title": before["session"]["title"]}, actor="user", inputs=[])
        self.assertEqual(code, 0, result)
        self.assertEqual(result["session"]["title_mode"], "manual")

    def test_captured_source_fences_a_configuration_switch_to_an_identical_catalog(self):
        before = self.enroll()
        other = self.root / "other-catalog"
        shutil.copytree(catalog_config.catalog_root(), other)
        self.assertEqual(self.call("config", "set", "catalog.root", str(other))[0], 0)
        current = self.context()
        self.assertNotEqual(current["catalog_source"], before["catalog_source"])
        self.assertEqual(current["session"], before["session"])
        code, result, _ = self.apply(before, {"state": "Wrong catalog"})
        self.assertEqual(code, 2)
        self.assertIn("source", result["error"]["message"])
        self.assertEqual(self.context()["session"]["state"], "")

    def test_malformed_input_or_duplicate_keys_are_rejected(self):
        before = self.enroll()
        for raw in ('{}', 'null', '{"operation_id":"one","operation_id":"two"}', '[]'):
            with self.subTest(raw=raw):
                self.assertEqual(self.call("agent", "apply", raw=raw)[0], 2)
        self.assertEqual(self.context()["session"], before["session"])


if __name__ == "__main__":
    unittest.main()
