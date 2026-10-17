from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'src'))

from _streams.catalog.model import Session, Stream
from _streams.catalog.operations import Authority
from _streams.catalog_runtime import Runtime
from _streams.cmux import CmuxWorkspace
from _streams.errors import StreamError
from _streams.frontend import FrontendMode
from _streams.herdr import HerdrPane, HerdrWorkspace
from _streams.herdr_binding import HerdrEndpoint


class RuntimeTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.cwd = self.root / 'checkout'
        self.cwd.mkdir()
        self.runtime = Runtime(self.root / 'state', 'local', FrontendMode.HERDR)
        self.stream = Stream('../arbitrary/id', 'Example', 'Goal', 'local')
        self.authority = Authority('local', 'epoch')
        self.session = Session('../session/id', 'local', self.stream.id, 'pi', 'native id', 'Implementation')
        env = patch.dict(os.environ, {}, clear=True)
        env.start()
        self.addCleanup(env.stop)
        transport = patch('_streams.catalog_runtime.HerdrClient')
        self.herdr = transport.start().return_value
        self.addCleanup(transport.stop)
        self.herdr.endpoint.return_value = HerdrEndpoint('/herdr.sock', None)
        self.herdr.resolve_workspace.return_value = HerdrWorkspace('w1', 'Example')
        self.herdr.current_pane.return_value = HerdrPane('p1', 't1', 'w1', '/irrelevant')
        self.herdr.command_result.side_effect = self.herdr_result

    def herdr_result(self, args, expected):
        if args[:2] == ['workspace', 'create']:
            return {'workspace': {'workspace_id': 'w1', 'label': 'Example'},
                    'tab': {'tab_id': 't1', 'workspace_id': 'w1'},
                    'root_pane': {'pane_id': 'p1', 'tab_id': 't1', 'workspace_id': 'w1', 'cwd': str(self.cwd)}}
        if args[:2] == ['agent', 'start']:
            return {'agent': {'pane_id': 'p1', 'tab_id': 't1', 'workspace_id': 'w1',
                              'name': args[2], 'agent': 'pi', 'interactive_ready': True}}
        raise AssertionError(args)

    def open(self, sessions=(), **kwargs):
        return self.runtime.open(self.stream, sessions, authority=self.authority, **kwargs)

    def test_missing_mapping_requires_explicit_cwd(self):
        with self.assertRaisesRegex(StreamError, '--cwd'):
            self.open()
        self.herdr.command_result.assert_not_called()
        self.herdr.endpoint.assert_not_called()

    def test_directory_persistence_arbitrary_ids_and_symlinks(self):
        link = self.root / 'link'
        link.symlink_to(self.cwd, target_is_directory=True)
        self.runtime.remember_directory(self.stream.id, link)
        fresh = Runtime(self.root / 'state', 'local', FrontendMode.HERDR)
        result = fresh.open(self.stream, (), authority=self.authority)
        self.assertEqual(result['stream_id'], self.stream.id)
        self.assertEqual(result['workspace_id'], 'w1')
        self.assertEqual(self.herdr.command_result.call_args_list[0].args[0][3], str(self.cwd.resolve()))
        self.assertFalse((self.root / 'arbitrary').exists())

    def test_directory_must_exist_and_be_directory(self):
        file = self.root / 'file'
        file.touch()
        for cwd in (file, self.root / 'missing'):
            with self.subTest(cwd=cwd), self.assertRaisesRegex(StreamError, 'directory|exist'):
                self.open(cwd=cwd)
        self.herdr.command_result.assert_not_called()

    def test_effective_authority_not_pending_owner(self):
        for authority in (None, Authority('remote', 'epoch')):
            with self.subTest(authority=authority), self.assertRaisesRegex(StreamError, 'authority|owner'):
                self.runtime.open(self.stream, (), authority=authority, cwd=self.cwd)
        pending = replace(self.stream, owner_host_id='remote')
        self.runtime.open(pending, (), authority=self.authority, cwd=self.cwd)

    def test_unique_local_session_and_session_directory(self):
        session_cwd = self.root / 'session-checkout'
        session_cwd.mkdir()
        self.runtime.remember_directory(self.stream.id, session_cwd, session_id=self.session.id)
        others = (replace(self.session, host_id='remote'), replace(self.session, stream_id='other'))
        result = self.open(others + (self.session,))
        self.assertEqual(result['session_id'], self.session.id)
        args = self.herdr.command_result.call_args_list[-1].args[0]
        self.assertEqual(args[-3:], ['--', '--session', 'native id'])
        self.assertEqual(self.herdr.command_result.call_args_list[0].args[0][3], str(session_cwd.resolve()))

    def test_ambiguity_and_reference_precedence(self):
        second = replace(self.session, id='second', title=self.session.id)
        with self.assertRaisesRegex(StreamError, '--session'):
            self.open((self.session, second), cwd=self.cwd)
        selected = self.open((self.session, second), session_ref=self.session.id, cwd=self.cwd)
        self.assertEqual(selected['session_id'], self.session.id)

    def test_title_reference_and_case_insensitive_substring(self):
        two = replace(self.session, id='second', title='Implementation followup')
        for index, reference in enumerate(('Implementation', 'PLEMENTATION')):
            runtime = Runtime(self.root / f'state-{index}', 'local', FrontendMode.HERDR)
            with self.subTest(reference=reference):
                result = runtime.open(self.stream, (self.session,), authority=self.authority,
                                      session_ref=reference, cwd=self.cwd)
                self.assertEqual(result['session_id'], self.session.id)
        with self.assertRaisesRegex(StreamError, 'ambiguous'):
            self.open((self.session, two), session_ref='implement', cwd=self.cwd)
        with self.assertRaisesRegex(StreamError, 'eligible|local'):
            self.open((replace(self.session, host_id='remote'),), session_ref=self.session.id, cwd=self.cwd)

    def test_unsupported_harness_rejected_before_creation(self):
        with self.assertRaisesRegex(StreamError, 'unsupported'):
            self.open((replace(self.session, harness='unknown'),), cwd=self.cwd)
        self.herdr.command_result.assert_not_called()

    def test_reuse_and_current_persist_across_instances(self):
        self.open((self.session,), cwd=self.cwd)
        self.herdr.command_result.reset_mock()
        fresh = Runtime(self.root / 'state', 'local', FrontendMode.HERDR)
        result = fresh.open(self.stream, (self.session,), authority=self.authority)
        self.assertTrue(result['reused'])
        self.herdr.command_result.assert_not_called()
        self.herdr.run.assert_called_with(['workspace', 'focus', 'w1'])
        self.assertEqual(fresh.current_stream_id(), self.stream.id)

    def test_reused_workspace_does_not_launch_different_session(self):
        self.open(cwd=self.cwd)
        with self.assertRaisesRegex(StreamError, 'different|session'):
            self.open((self.session,))
        self.assertEqual(self.herdr.command_result.call_count, 1)

    def test_stale_workspace_never_relaunched(self):
        self.open(cwd=self.cwd)
        self.herdr.resolve_workspace.side_effect = StreamError('missing workspace')
        with self.assertRaisesRegex(StreamError, 'stale|Inspect'):
            self.open()
        self.assertEqual(self.herdr.command_result.call_count, 1)

    def test_current_explicit_environment_and_no_heuristic(self):
        with patch.dict(os.environ, {'STREAMS_STREAM_ID': 'exact id'}):
            self.assertEqual(self.runtime.current_stream_id(), 'exact id')
        with self.assertRaisesRegex(StreamError, 'association'):
            self.runtime.current_stream_id()

    def test_malformed_local_state_is_not_silently_overwritten(self):
        self.runtime.remember_directory(self.stream.id, self.cwd)
        path = next((self.root / 'state').rglob('*.json'))
        path.write_text('{invalid')
        with self.assertRaisesRegex(StreamError, 'state|JSON'):
            self.open()
        with self.assertRaisesRegex(StreamError, 'state|JSON'):
            self.runtime.remember_directory(self.stream.id, self.cwd)

    def test_partial_open_keeps_association_and_reports_created_ids(self):
        def fail(args, expected):
            if args[:2] == ['agent', 'start']:
                raise StreamError('failed to start')
            return self.herdr_result(args, expected)
        self.herdr.command_result.side_effect = fail
        with self.assertRaisesRegex(StreamError, 'w1.*t1.*p1.*no rollback'):
            self.open((self.session,), cwd=self.cwd)
        self.assertEqual(self.runtime.current_stream_id(), self.stream.id)

    def test_uncertain_creation_never_relaunches_after_process_restart(self):
        for frontend in (FrontendMode.HERDR, FrontendMode.CMUX):
            for failure in (StreamError('creation response timed out'), {}):
                with self.subTest(frontend=frontend, failure=failure), patch('_streams.catalog_runtime.CmuxClient') as cmux:
                    state = self.root / f'uncertain-{frontend}-{isinstance(failure, dict)}'
                    runtime = Runtime(state, 'local', frontend)
                    command = self.herdr.command_result if frontend == FrontendMode.HERDR else cmux.return_value.json_from_command
                    command.reset_mock()
                    command.side_effect = failure if isinstance(failure, Exception) else None
                    command.return_value = failure
                    with self.assertRaises(StreamError):
                        runtime.open(self.stream, (), authority=self.authority, cwd=self.cwd)
                    command.reset_mock()
                    fresh = Runtime(state, 'local', frontend)
                    with self.assertRaisesRegex(StreamError, 'pending|partial|inspect'):
                        fresh.open(self.stream, (), authority=self.authority, cwd=self.cwd)
                    command.assert_not_called()
        self.herdr.command_result.side_effect = self.herdr_result

    def test_failed_association_write_retains_creation_attempt(self):
        write = self.runtime._write
        def fail_association(path, data):
            if 'associations' in path.parts:
                raise StreamError('association write failed')
            return write(path, data)
        with patch.object(self.runtime, '_write', side_effect=fail_association), self.assertRaisesRegex(StreamError, 'w1.*association write failed'):
            self.open(cwd=self.cwd)
        fresh = Runtime(self.root / 'state', 'local', FrontendMode.HERDR)
        with self.assertRaisesRegex(StreamError, 'pending|partial|inspect'):
            fresh.open(self.stream, (), authority=self.authority, cwd=self.cwd)
        self.assertEqual(self.herdr.command_result.call_count, 1)
        attempt = next((self.root / 'state').rglob('attempts/**/*.json'))
        self.assertEqual(json.loads(attempt.read_text())['workspace_id'], 'w1')

    def test_failure_to_persist_creation_intent_prevents_native_calls(self):
        write = self.runtime._write
        def fail_attempt(path, data):
            if 'attempts' in path.parts:
                raise StreamError('creation intent write failed')
            return write(path, data)
        with patch.object(self.runtime, '_write', side_effect=fail_attempt), self.assertRaisesRegex(StreamError, 'intent write failed'):
            self.open(cwd=self.cwd)
        self.herdr.command_result.assert_not_called()

    def test_overlapping_opens_create_only_one_workspace(self):
        entered = threading.Event()
        released = threading.Event()
        duplicate = threading.Event()
        native_calls = []
        def create(args, expected):
            if args[:2] == ['workspace', 'create']:
                native_calls.append(args)
                if len(native_calls) == 1:
                    entered.set()
                    if not released.wait(5):
                        raise AssertionError('test did not release first creation')
                else:
                    duplicate.set()
            return self.herdr_result(args, expected)
        self.herdr.command_result.side_effect = create
        with ThreadPoolExecutor(max_workers=2) as pool:
            first = pool.submit(self.open, cwd=self.cwd)
            self.assertTrue(entered.wait(5))
            fresh = Runtime(self.root / 'state', 'local', FrontendMode.HERDR)
            second = pool.submit(fresh.open, self.stream, (), authority=self.authority, cwd=self.cwd)
            try:
                self.assertFalse(duplicate.wait(0.25), 'second open entered native creation')
            finally:
                released.set()
            self.assertFalse(first.result(timeout=5)['reused'])
            self.assertTrue(second.result(timeout=5)['reused'])
        self.assertEqual(len(native_calls), 1)

    def test_active_creation_holds_a_cross_process_lock(self):
        probe = '''import fcntl, sys
with open(sys.argv[1]) as handle:
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        sys.exit(0)
sys.exit(1)
'''
        locks = []
        def create(args, expected):
            if args[:2] == ['workspace', 'create']:
                locks.extend(self.runtime.state_dir.rglob('*.lock'))
                self.assertEqual(len(locks), 1)
                result = subprocess.run([sys.executable, '-c', probe, str(locks[0])], capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 0, result.stderr)
            return self.herdr_result(args, expected)
        self.herdr.command_result.side_effect = create
        self.open(cwd=self.cwd)
        released = subprocess.run([sys.executable, '-c', probe, str(locks[0])], capture_output=True, text=True, timeout=5)
        self.assertEqual(released.returncode, 1, released.stderr)

    def test_unconfirmed_creation_intent_durability_prevents_native_calls(self):
        sync = self.runtime._sync_directory
        def fail_attempt(path):
            if 'attempts' in path.parts:
                raise OSError('directory sync failed')
            return sync(path)
        with patch.object(self.runtime, '_sync_directory', side_effect=fail_attempt), self.assertRaisesRegex(StreamError, 'directory sync failed'):
            self.open(cwd=self.cwd)
        self.herdr.command_result.assert_not_called()
        fresh = Runtime(self.root / 'state', 'local', FrontendMode.HERDR)
        with self.assertRaisesRegex(StreamError, 'pending|partial'):
            fresh.open(self.stream, (), authority=self.authority, cwd=self.cwd)

    def test_resume_explicit_directory_is_remembered_for_a_fresh_runtime(self):
        original = Path.cwd()
        self.addCleanup(os.chdir, original)
        other = self.root / 'other-checkout'
        other.mkdir()
        self.runtime.remember_directory(self.stream.id, self.cwd, session_id=self.session.id)
        with patch('_streams.catalog_runtime.os.execvp'):
            self.runtime.resume(self.stream, (self.session,), authority=self.authority, cwd=other)
        fresh = Runtime(self.root / 'state', 'local', FrontendMode.HERDR)
        with patch('_streams.catalog_runtime.os.execvp'):
            fresh.resume(self.stream, (self.session,), authority=self.authority)
        self.assertEqual(Path.cwd(), other.resolve())

    def test_reused_workspace_remembers_explicit_session_directory(self):
        self.open((self.session,), cwd=self.cwd)
        other = self.root / 'other-checkout'
        other.mkdir()
        self.assertTrue(self.open((self.session,), cwd=other)['reused'])
        fresh = Runtime(self.root / 'state', 'local', FrontendMode.HERDR)
        self.assertEqual(fresh._cwd(self.stream, self.session, None), other.resolve())

    def test_session_exact_title_precedes_substring_case_insensitively(self):
        second = replace(self.session, id='second', title='Implementation followup')
        result = self.open((self.session, second), session_ref='IMPLEMENTATION', cwd=self.cwd)
        self.assertEqual(result['session_id'], self.session.id)

    def test_resume_no_session_and_effective_authority(self):
        with self.assertRaisesRegex(StreamError, 'session'):
            self.runtime.resume(self.stream, (), authority=self.authority, cwd=self.cwd)
        with self.assertRaisesRegex(StreamError, 'authority|owner'):
            self.runtime.resume(self.stream, (self.session,), authority=None, cwd=self.cwd)

    def test_resume_executes_native_argv_in_explicit_directory(self):
        original = Path.cwd()
        self.addCleanup(os.chdir, original)
        with patch('_streams.catalog_runtime.os.execvp') as execute:
            self.runtime.resume(self.stream, (self.session,), authority=self.authority, cwd=self.cwd)
        execute.assert_called_once_with('pi', ['pi', '--session', 'native id'])
        self.assertEqual(Path.cwd(), self.cwd.resolve())
        self.assertEqual(os.environ['STREAMS_STREAM_ID'], self.stream.id)

    def test_cmux_exact_caller_and_durable_association(self):
        runtime = Runtime(self.root / 'state', 'local', FrontendMode.CMUX)
        workspace = CmuxWorkspace({'id': 'uuid-1', 'ref': 'workspace:1', 'title': 'Unrelated'})
        with patch('_streams.catalog_runtime.CmuxClient') as transport:
            client = transport.return_value
            client.json_from_command.return_value = {'workspace_id': 'uuid-1'}
            client.workspaces.return_value = [workspace]
            runtime.open(self.stream, (), authority=self.authority, cwd=self.cwd)
            with patch.dict(os.environ, {'CMUX_WORKSPACE_ID': 'uuid-1'}):
                self.assertEqual(runtime.current_stream_id(), self.stream.id)
            with patch.dict(os.environ, {'CMUX_WORKSPACE_ID': 'workspace:1'}):
                with self.assertRaisesRegex(StreamError, 'exact|caller'):
                    runtime.current_stream_id()
            with self.assertRaisesRegex(StreamError, 'caller'):
                runtime.current_stream_id()

    def test_explicit_invalid_cwd_rejected_even_when_workspace_reusable(self):
        self.open(cwd=self.cwd)
        with self.assertRaisesRegex(StreamError, 'directory'):
            self.open(cwd=self.root / 'missing')

    def test_all_created_ids_reported_on_native_workspace_collision(self):
        self.open(cwd=self.cwd)
        path = next((self.root / 'state').rglob('associations/**/*.json'))
        # A different stream is allowed to attempt creation but cannot overwrite w1.
        second = replace(self.stream, id='second')
        with self.assertRaisesRegex(StreamError, 'w1.*t1.*p1.*no rollback'):
            self.runtime.open(second, (), authority=self.authority, cwd=self.cwd)
        self.assertEqual(json.loads(path.read_text())['stream_id'], self.stream.id)

    def test_directory_identity_collision_and_duplicate_json_fail_closed(self):
        self.runtime.remember_directory(self.stream.id, self.cwd)
        path = next((self.root / 'state').rglob('directories/*.json'))
        original = path.read_text()
        data = json.loads(original)
        data['stream_id'] = 'another stream'
        path.write_text(json.dumps(data))
        with self.assertRaisesRegex(StreamError, 'identity'):
            self.open()
        path.write_text(original.replace('"version": 1', '"version": 1, "version": 1'))
        with self.assertRaisesRegex(StreamError, 'duplicate JSON'):
            self.open()

    def test_directory_state_symlink_is_rejected_not_followed(self):
        self.runtime.remember_directory(self.stream.id, self.cwd)
        path = next((self.root / 'state').rglob('directories/*.json'))
        target = self.root / 'external.json'
        path.rename(target)
        path.symlink_to(target)
        with self.assertRaisesRegex(StreamError, 'symlink'):
            self.open()
        with self.assertRaisesRegex(StreamError, 'symlink'):
            self.runtime.remember_directory(self.stream.id, self.cwd)

    def test_current_herdr_uses_live_pane_workspace_not_environment_hint(self):
        self.open(cwd=self.cwd)
        self.herdr.current_pane.return_value = HerdrPane('p2', 't2', 'other', str(self.cwd))
        with patch.dict(os.environ, {'HERDR_WORKSPACE_ID': 'w1'}):
            with self.assertRaisesRegex(StreamError, 'association'):
                self.runtime.current_stream_id()
        self.herdr.resolve_workspace.assert_called_with('other')

    def test_partial_session_start_is_not_retried(self):
        def fail(args, expected):
            if args[:2] == ['agent', 'start']:
                raise StreamError('failed')
            return self.herdr_result(args, expected)
        self.herdr.command_result.side_effect = fail
        with self.assertRaises(StreamError):
            self.open((self.session,), cwd=self.cwd)
        self.herdr.command_result.reset_mock()
        with self.assertRaisesRegex(StreamError, 'partial open'):
            self.open((self.session,))
        self.herdr.command_result.assert_not_called()

    def test_cmux_selected_conversation_is_shell_quoted_and_not_duplicated(self):
        runtime = Runtime(self.root / 'state', 'local', FrontendMode.CMUX)
        with patch('_streams.catalog_runtime.CmuxClient') as transport:
            client = transport.return_value
            client.json_from_command.return_value = {'workspace_id': 'uuid-1'}
            client.workspaces.return_value = [CmuxWorkspace({'id': 'uuid-1', 'title': 'anything'})]
            runtime.open(self.stream, (self.session,), authority=self.authority, cwd=self.cwd)
            args = client.json_from_command.call_args.args[0]
            self.assertEqual(args[args.index('--command') + 1], "pi --session 'native id'")
            self.assertIn('STREAMS_STREAM_ID=' + self.stream.id, args)
            runtime.open(self.stream, (self.session,), authority=self.authority)
            client.json_from_command.assert_called_once()
            client.run.assert_called_once_with(['cmux', 'select-workspace', '--workspace', 'uuid-1'])

    def test_cmux_partial_failure_reports_returned_resource_ids(self):
        runtime = Runtime(self.root / 'state', 'local', FrontendMode.CMUX)
        with patch('_streams.catalog_runtime.CmuxClient') as transport:
            client = transport.return_value
            client.json_from_command.return_value = {'workspace_id': 'uuid-1', 'surface_id': 'surface-1', 'pane_id': 'pane-1'}
            runtime.open(self.stream, (), authority=self.authority, cwd=self.cwd)
            with self.assertRaisesRegex(StreamError, 'uuid-1.*surface-1.*pane-1.*no rollback'):
                runtime.open(replace(self.stream, id='second'), (), authority=self.authority, cwd=self.cwd)

    def test_auto_frontend_never_uses_global_focus(self):
        runtime = Runtime(self.root / 'state', 'local', FrontendMode.AUTO)
        with self.assertRaisesRegex(StreamError, 'frontend'):
            runtime.open(self.stream, (), authority=self.authority, cwd=self.cwd)
        with patch.dict(os.environ, {'CMUX_WORKSPACE_ID': 'uuid', 'HERDR_ENV': '1', 'HERDR_PANE_ID': 'pane'}):
            with self.assertRaisesRegex(StreamError, 'both'):
                runtime.current_stream_id()

    def test_distinct_ids_do_not_share_directory_mappings(self):
        second_cwd = self.root / 'second-checkout'
        second_cwd.mkdir()
        self.runtime.remember_directory('a/b', self.cwd, session_id='x/y')
        self.runtime.remember_directory('a_b', second_cwd, session_id='x_y')
        self.assertEqual(len(list((self.root / 'state').rglob('directories/*.json'))), 2)
        second = Stream('a_b', 'Second', 'Goal', 'local')
        session = replace(self.session, id='x_y', stream_id='a_b')
        self.runtime.open(second, (session,), authority=self.authority)
        args = self.herdr.command_result.call_args_list[0].args[0]
        self.assertEqual(args[3], str(second_cwd.resolve()))

    def test_open_and_current_need_no_scratch_catalog_or_lookup(self):
        with patch.dict(os.environ, {'HOME': str(self.root / 'empty-home'),
                                     'XDG_CONFIG_HOME': str(self.root / 'empty-config')}):
            result = self.open(cwd=self.cwd)
            self.assertEqual(result['stream_id'], self.stream.id)
            self.assertEqual(self.runtime.current_stream_id(), self.stream.id)
        self.assertFalse((self.root / 'empty-home').exists())
        self.assertFalse((self.root / 'empty-config').exists())

    def test_none_frontend_is_explicit_error(self):
        runtime = Runtime(self.root / 'state', 'local', FrontendMode.NONE)
        with self.assertRaisesRegex(StreamError, 'frontend'):
            runtime.open(self.stream, (), authority=self.authority, cwd=self.cwd)


if __name__ == '__main__':
    unittest.main()
