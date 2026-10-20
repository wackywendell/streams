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
from _streams.catalog_runtime import Runtime, _native_environment
from _streams.cmux import CmuxWorkspace
from _streams.errors import StreamError
from _streams.frontend import FrontendMode
from _streams.herdr import HerdrPane, HerdrWorkspace
from _streams.herdr import HerdrEndpoint


class NativeEnvironmentTest(unittest.TestCase):
    @staticmethod
    def values():
        args = _native_environment('stream')
        return dict(args[index + 1].split('=', 1) for index in range(0, len(args), 2))

    def test_missing_selectors_pin_caller_defaults_not_server_environment(self):
        with patch.dict(os.environ, {'HOME': '/owner-home'}, clear=True):
            self.assertEqual(self.values(), {
                'STREAMS_STREAM_ID': 'stream', 'HOME': '/owner-home',
                'XDG_CONFIG_HOME': '/owner-home/.config', 'XDG_DATA_HOME': '/owner-home/.local/share',
                'XDG_STATE_HOME': '/owner-home/.local/state', 'PI_CODING_AGENT_DIR': '/owner-home/.pi/agent',
                'CODEX_HOME': '/owner-home/.codex', 'CLAUDE_CONFIG_DIR': '/owner-home/.claude',
                'PATH': os.defpath, 'HERDR_BIN_PATH': 'herdr',
            })

    def test_custom_selectors_are_bounded_without_credentials_or_caller_panes(self):
        selected = {'HOME': '/owner', 'XDG_CONFIG_HOME': '/config', 'XDG_DATA_HOME': '/data',
                    'XDG_STATE_HOME': '/state', 'PI_CODING_AGENT_DIR': '/pi', 'CODEX_HOME': '/codex',
                    'CLAUDE_CONFIG_DIR': '/claude', 'PATH': '/tools', 'HERDR_BIN_PATH': '/tools/herdr',
                    'HERDR_SOCKET_PATH': '/socket', 'PI_OFFLINE': '1'}
        with patch.dict(os.environ, {**selected, 'API_KEY': 'secret', 'HERDR_PANE_ID': 'old-pane',
                                     'CMUX_WORKSPACE_ID': 'old-workspace'}, clear=True):
            self.assertEqual(self.values(), {'STREAMS_STREAM_ID': 'stream', **selected})


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
        self.saved = self.root.resolve() / 'pi' / 'sessions' / 'native id.jsonl'
        self.saved.parent.mkdir(parents=True)
        self.saved.write_text(json.dumps({'type': 'session', 'id': 'native id'}) + '\n')
        self.running = False
        env = patch.dict(os.environ, {'PI_CODING_AGENT_DIR': str(self.root / 'pi')}, clear=True)
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
        if args == ['api', 'snapshot']:
            pane = {'pane_id': 'p1', 'terminal_id': 'term1', 'tab_id': 't1', 'workspace_id': 'w1'}
            agent = {**pane, 'agent': 'pi', 'agent_session': {'agent': 'pi', 'kind': 'path', 'value': str(self.saved)}}
            return {'snapshot': {'workspaces': [{'workspace_id': 'w1', 'label': 'Example'}],
                                 'tabs': [{'tab_id': 't1', 'workspace_id': 'w1', 'label': '1', 'number': 1}],
                                 'panes': [pane], 'layouts': [{'tab_id': 't1', 'workspace_id': 'w1', 'panes': [{'pane_id': 'p1'}]}],
                                 'agents': [agent] if self.running else []}}
        if args[:2] == ['workspace', 'create']:
            return {'workspace': {'workspace_id': 'w1', 'label': 'Example'},
                    'tab': {'tab_id': 't1', 'workspace_id': 'w1'},
                    'root_pane': {'pane_id': 'p1', 'tab_id': 't1', 'workspace_id': 'w1', 'cwd': str(self.cwd)}}
        if args[:2] == ['agent', 'start']:
            self.running = True
            return {'agent': {'pane_id': 'p1', 'tab_id': 't1', 'workspace_id': 'w1',
                              'name': args[2], 'agent': 'pi', 'interactive_ready': True}}
        raise AssertionError(args)

    def open(self, sessions=(), **kwargs):
        return self.runtime.open(self.stream, sessions, authority=self.authority, **kwargs)

    def test_automatic_pi_enrollment_matches_only_native_header_or_explicit_id(self):
        path = self.root / 'filename-is-not-the-session-id.jsonl'
        for native_id, kind in (('fork-uuid', 'path'), ('resumed-uuid', 'path'), ('explicit-uuid', 'id')):
            with self.subTest(native_id=native_id, kind=kind):
                path.write_text(json.dumps({'type': 'session', 'id': native_id}) + '\nnot a header\n')
                self.herdr.command_result.side_effect = None
                self.herdr.command_result.return_value = {'agent': {
                    'pane_id': 'p1', 'workspace_id': 'w1', 'agent': 'pi',
                    'agent_session': {'agent': 'pi', 'kind': kind,
                                      'value': str(path) if kind == 'path' else native_id},
                }}
                with patch.dict(os.environ, {'HERDR_PANE_ID': 'p1'}):
                    self.runtime.verify_automatic_enrollment('pi', native_id)
                self.herdr.command_result.assert_called_with(['agent', 'get', 'p1'], 'agent_info')
                self.assertFalse(self.runtime.state_dir.exists())

    def test_automatic_pi_enrollment_fails_closed_on_native_metadata(self):
        agent = {'pane_id': 'p1', 'workspace_id': 'w1', 'agent': 'pi',
                 'agent_session': {'agent': 'pi', 'kind': 'id', 'value': 'parent'}}
        invalid = [None, {}, {**agent, 'pane_id': 'other'}, {**agent, 'workspace_id': 'other'},
                   {**agent, 'agent': 'codex'}, {**agent, 'agent_session': None}]
        for session in ({}, {'agent': 'codex', 'kind': 'id', 'value': 'parent'},
                        {'agent': 'pi', 'kind': 'title', 'value': 'parent'},
                        {'agent': 'pi', 'kind': 'id', 'value': ''},
                        {'agent': 'pi', 'kind': 'path', 'value': 'relative.jsonl'},
                        {'agent': 'pi', 'kind': 'path', 'value': str(self.root / 'missing.jsonl')}):
            invalid.append({**agent, 'agent_session': session})
        invalid.append(agent)  # The helper's requested ID is not the parent's explicit ID.
        self.herdr.command_result.side_effect = None
        for metadata in invalid:
            with self.subTest(metadata=metadata), patch.dict(os.environ, {'HERDR_PANE_ID': 'p1'}):
                self.herdr.command_result.return_value = {'agent': metadata}
                with self.assertRaisesRegex(StreamError, 'background helper cannot automatically enroll'):
                    self.runtime.verify_automatic_enrollment('pi', 'helper')
                self.assertFalse(self.runtime.state_dir.exists())
        with patch.dict(os.environ, {'HERDR_PANE_ID': 'other'}):
            with self.assertRaisesRegex(StreamError, 'exact Herdr caller pane'):
                self.runtime.verify_automatic_enrollment('pi', 'parent')
        self.herdr.current_pane.side_effect = StreamError('missing current pane')
        with self.assertRaisesRegex(StreamError, 'background helper'):
            self.runtime.verify_automatic_enrollment('pi', 'parent')

    def test_automatic_pi_enrollment_reads_a_bounded_valid_first_session_header(self):
        path = self.root / 'helper.jsonl'
        self.herdr.command_result.side_effect = None
        self.herdr.command_result.return_value = {'agent': {
            'pane_id': 'p1', 'workspace_id': 'w1', 'agent': 'pi',
            'agent_session': {'agent': 'pi', 'kind': 'path', 'value': str(path)},
        }}
        for content in (b'', b'{broken', b'[]', b'null', b'\xff',
                        b'{"type":"message","id":"helper"}\n',
                        b'{"type":"session"}\n', b'{"type":"session","id":null}\n',
                        b'{"type":"session","id":"helper","id":"parent"}\n',
                        b'{"type":"session","id":"parent"}\n',
                        b'\n{"type":"session","id":"helper"}\n',
                        b'{"type":"session","id":"helper","padding":"' + b'x' * 65536 + b'"}\n'):
            with self.subTest(content=content[:80]), patch.dict(os.environ, {'HERDR_PANE_ID': 'p1'}):
                path.write_bytes(content)
                with self.assertRaisesRegex(StreamError, 'background helper'):
                    self.runtime.verify_automatic_enrollment('pi', 'helper')
                self.assertFalse(self.runtime.state_dir.exists())

    def test_automatic_enrollment_guard_leaves_other_frontends_and_harnesses_alone(self):
        for mode in (FrontendMode.NONE, FrontendMode.AUTO, FrontendMode.CMUX):
            with self.subTest(mode=mode):
                Runtime(self.root / 'state', 'local', mode).verify_automatic_enrollment('pi', 'helper')
        self.runtime.verify_automatic_enrollment('codex', 'helper')
        self.herdr.endpoint.assert_not_called()
        self.herdr.current_pane.assert_not_called()
        self.herdr.command_result.assert_not_called()
        self.assertFalse(self.runtime.state_dir.exists())

    def test_reporting_context_reads_unbound_claimed_and_bound_workspaces_without_writes(self):
        self.assertIsNone(self.runtime.context_stream_id())
        self.assertFalse(self.runtime.state_dir.exists())
        with self.runtime.enrollment() as scope:
            scope.claim(self.stream.id)
        before = sorted(str(path) for path in self.runtime.state_dir.rglob('*'))
        with patch.object(self.runtime, '_write', side_effect=AssertionError('context wrote state')):
            self.assertEqual(self.runtime.context_stream_id(), self.stream.id)
        self.assertEqual(before, sorted(str(path) for path in self.runtime.state_dir.rglob('*')))
        with self.runtime.enrollment() as scope:
            scope.bind(self.stream, authority=self.authority)
        with patch.object(self.runtime, '_write', side_effect=AssertionError('context wrote state')):
            self.assertEqual(self.runtime.context_stream_id(), self.stream.id)
        self.herdr.command_result.assert_not_called()
        self.herdr.run.assert_not_called()

    def test_reporting_context_rejects_conflicting_explicit_identity_and_partial_binding(self):
        with self.runtime.enrollment() as scope:
            scope.claim(self.stream.id)
            scope.bind(self.stream, authority=self.authority)
        with patch.dict(os.environ, {'STREAMS_STREAM_ID': 'different'}):
            with self.assertRaisesRegex(StreamError, 'disagree'):
                self.runtime.context_stream_id()
        path = next(self.runtime.state_dir.glob('associations/*/*.json'))
        binding = self.runtime._association(path)
        self.runtime._write(path, {key: value for key, value in {**binding, 'ready': False}.items() if key != 'version'})
        with self.assertRaisesRegex(StreamError, 'partial'):
            self.runtime.context_stream_id()

    def test_remember_initial_directory_does_not_replace_an_explicit_mapping(self):
        self.runtime.remember_directory(self.stream.id, self.cwd)
        other = self.root / 'other'
        other.mkdir()
        self.runtime.remember_directory(self.stream.id, other, if_missing=True)
        self.assertEqual(self.runtime._directory_record(self.stream.id, None)['cwd'], str(self.cwd.resolve()))

    def test_pending_enrollment_claim_blocks_explicit_open_after_interruption(self):
        with self.runtime.enrollment() as scope:
            scope.claim(self.stream.id)
        with self.assertRaisesRegex(StreamError, 'claim|enrollment'):
            self.open(cwd=self.cwd)
        self.herdr.command_result.assert_not_called()
        self.herdr.run.assert_not_called()

    def test_claim_persistence_and_open_are_serialized_by_stream_lock(self):
        entered = threading.Event()
        release = threading.Event()
        native_creation = threading.Event()
        write = self.runtime._write
        def paused_claim(path, data):
            if 'enrollments' in path.parts:
                entered.set()
                self.assertTrue(release.wait(5))
            return write(path, data)
        def create(args, expected):
            native_creation.set()
            return self.herdr_result(args, expected)
        def claim():
            with self.runtime.enrollment() as scope:
                scope.claim(self.stream.id)
        self.herdr.command_result.side_effect = create
        fresh = Runtime(self.root / 'state', 'local', FrontendMode.HERDR)
        with patch.object(self.runtime, '_write', side_effect=paused_claim), ThreadPoolExecutor(max_workers=2) as pool:
            one = pool.submit(claim)
            self.assertTrue(entered.wait(5))
            two = pool.submit(fresh.open, self.stream, (), authority=self.authority, cwd=self.cwd)
            try:
                self.assertFalse(native_creation.wait(0.2))
            finally:
                release.set()
            one.result(timeout=5)
            with self.assertRaisesRegex(StreamError, 'claim|enrollment'):
                two.result(timeout=5)
        self.herdr.command_result.assert_not_called()

    def test_claim_and_bind_reject_same_stream_bound_to_another_workspace(self):
        self.open(cwd=self.cwd)
        self.herdr.current_pane.return_value = HerdrPane('p2', 't2', 'w2', '/irrelevant')
        self.herdr.resolve_workspace.return_value = HerdrWorkspace('w2', 'Second')
        with self.runtime.enrollment() as scope:
            for action in (lambda: scope.claim(self.stream.id), lambda: scope.bind(self.stream, authority=self.authority)):
                with self.assertRaisesRegex(StreamError, 'different|another|workspace'):
                    action()
        self.assertFalse(list(self.runtime.state_dir.rglob('enrollments/**/*.json')))

    def test_claim_and_bind_reject_same_stream_claimed_by_another_workspace(self):
        with self.runtime.enrollment() as scope:
            scope.claim(self.stream.id)
        self.herdr.current_pane.return_value = HerdrPane('p2', 't2', 'w2', '/irrelevant')
        self.herdr.resolve_workspace.return_value = HerdrWorkspace('w2', 'Second')
        with self.runtime.enrollment() as scope:
            for action in (lambda: scope.claim(self.stream.id), lambda: scope.bind(self.stream, authority=self.authority)):
                with self.assertRaisesRegex(StreamError, 'different|another|workspace'):
                    action()
        self.assertEqual(len(list(self.runtime.state_dir.rglob('enrollments/**/*.json'))), 1)

    def test_claim_and_bind_reject_pending_native_open(self):
        _, frontend_scope = self.runtime._transport(FrontendMode.HERDR)
        with self.runtime._open_lock(frontend_scope, self.stream.id) as attempt:
            self.runtime._write(attempt, {'stream_id': self.stream.id, 'session_id': None,
                                         'command': None, 'cwd': str(self.cwd), 'workspace_id': None})
        with self.runtime.enrollment() as scope:
            for action in (lambda: scope.claim(self.stream.id), lambda: scope.bind(self.stream, authority=self.authority)):
                with self.assertRaisesRegex(StreamError, 'pending|partial'):
                    action()
        self.assertFalse(list(self.runtime.state_dir.rglob('enrollments/**/*.json')))

    def test_enrollment_does_not_retroactively_claim_open_created_workspace(self):
        self.open(cwd=self.cwd)
        with self.runtime.enrollment() as scope:
            scope.claim(self.stream.id)
            scope.bind(self.stream, authority=self.authority)
        self.assertFalse(list(self.runtime.state_dir.rglob('enrollments/**/*.json')))
        self.assertTrue(self.open((self.session,))['reused'])
        effects = [call for call in self.herdr.command_result.call_args_list if call.args[0] != ['api', 'snapshot']]
        self.assertEqual(len(effects), 1)

    def test_open_and_claim_fail_closed_on_all_claim_files(self):
        with self.runtime.enrollment() as scope:
            scope.claim('other-stream')
        claim = next(self.runtime.state_dir.rglob('enrollments/**/*.json'))
        original = claim.read_text()
        self.herdr.current_pane.return_value = HerdrPane('p2', 't2', 'w2', '/irrelevant')
        self.herdr.resolve_workspace.return_value = HerdrWorkspace('w2', 'Second')
        for failure in ('corrupt', 'symlink', 'wrong-name'):
            claim.write_text(original)
            target = None
            if failure == 'corrupt':
                claim.write_text('{broken')
            elif failure == 'symlink':
                target = self.root / 'external.json'
                claim.rename(target)
                claim.symlink_to(target)
            else:
                target = claim
                claim = claim.with_name('wrong-hash.json')
                target.rename(claim)
            try:
                with self.subTest(failure=failure):
                    with self.assertRaises(StreamError):
                        self.open(cwd=self.cwd)
                    with self.runtime.enrollment() as scope:
                        with self.assertRaises(StreamError):
                            scope.claim(self.stream.id)
            finally:
                if failure == 'symlink':
                    claim.unlink()
                    target.rename(claim)
                elif failure == 'wrong-name':
                    claim.rename(target)
                    claim = target
        self.herdr.command_result.assert_not_called()

    def test_enrollment_binds_without_native_mutations_and_reuses_identity(self):
        with self.runtime.enrollment() as scope:
            self.assertIsNone(scope.stream_id)
            scope.claim(self.stream.id)
            self.assertEqual(scope.stream_id, self.stream.id)
            scope.bind(self.stream, authority=self.authority)
        fresh = Runtime(self.root / 'state', 'local', FrontendMode.HERDR)
        with fresh.enrollment() as scope:
            self.assertEqual(scope.stream_id, self.stream.id)
            scope.bind(self.stream, authority=self.authority)
        self.assertEqual(fresh.current_stream_id(), self.stream.id)
        binding = next(fresh.state_dir.rglob('associations/**/*.json'))
        self.assertEqual(json.loads(binding.read_text()), {
            'version': 1, 'stream_id': self.stream.id, 'workspace_id': 'w1',
            'session_id': None, 'command': None, 'ready': True,
        })
        self.herdr.command_result.assert_not_called()
        self.herdr.run.assert_not_called()

    def test_enrollment_requires_effective_authority_even_without_native_context(self):
        for mode in (FrontendMode.HERDR, FrontendMode.NONE, FrontendMode.AUTO):
            runtime = Runtime(self.root / str(mode), 'local', mode)
            with runtime.enrollment() as scope:
                for authority in (None, Authority('remote', 'epoch')):
                    with self.assertRaisesRegex(StreamError, 'authority'):
                        scope.bind(self.stream, authority=authority)
                scope.bind(replace(self.stream, owner_host_id='remote'), authority=self.authority)

    def test_enrollment_no_frontend_has_only_a_stream_lock(self):
        for mode in (FrontendMode.NONE, FrontendMode.AUTO):
            runtime = Runtime(self.root / str(mode), 'local', mode)
            with runtime.enrollment() as scope:
                self.assertIsNone(scope.stream_id)
                scope.claim(self.stream.id)
                scope.bind(self.stream, authority=self.authority)
            self.assertEqual(len(list(runtime.state_dir.glob('stream-locks/*.lock'))), 1)
            self.assertFalse(list(runtime.state_dir.rglob('*.json')))
        self.herdr.endpoint.assert_not_called()
        self.herdr.current_pane.assert_not_called()
        self.herdr.run.assert_not_called()

    def test_enrollment_explicit_identity_without_native_context_cannot_be_replaced(self):
        runtime = Runtime(self.root / 'state', 'local', FrontendMode.NONE)
        with patch.dict(os.environ, {'STREAMS_STREAM_ID': self.stream.id}):
            with runtime.enrollment() as scope:
                self.assertEqual(scope.stream_id, self.stream.id)
                with self.assertRaisesRegex(StreamError, 'different|replace'):
                    scope.claim('another')
                scope.bind(self.stream, authority=self.authority)
        self.assertEqual(len(list(runtime.state_dir.glob('stream-locks/*.lock'))), 1)
        self.assertFalse(list(runtime.state_dir.rglob('*.json')))

    def test_enrollment_claim_requires_a_chosen_nonempty_identity(self):
        with self.runtime.enrollment() as scope:
            for invalid in (None, '', ' ', '\x00'):
                with self.assertRaisesRegex(StreamError, 'nonempty'):
                    scope.claim(invalid)
            self.assertIsNone(scope.stream_id)

    def test_enrollment_cross_process_lock_covers_claim_and_bind(self):
        probe = '''import fcntl, sys
with open(sys.argv[1]) as handle:
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        sys.exit(0)
sys.exit(1)
'''
        with self.runtime.enrollment() as scope:
            lock = next(self.runtime.state_dir.rglob('enrollments/**/*.lock'))
            for action in (lambda: None, lambda: scope.claim(self.stream.id),
                           lambda: scope.bind(self.stream, authority=self.authority)):
                action()
                result = subprocess.run([sys.executable, '-c', probe, str(lock)], capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 0, result.stderr)
        released = subprocess.run([sys.executable, '-c', probe, str(lock)], capture_output=True, text=True, timeout=5)
        self.assertEqual(released.returncode, 1, released.stderr)

    def test_enrollment_claim_survives_catalog_failure_and_rejects_replacement(self):
        with self.assertRaisesRegex(ValueError, 'catalog failed'):
            with self.runtime.enrollment() as scope:
                scope.claim(self.stream.id)
                raise ValueError('catalog failed')
        fresh = Runtime(self.root / 'state', 'local', FrontendMode.HERDR)
        with fresh.enrollment() as scope:
            self.assertEqual(scope.stream_id, self.stream.id)
            with self.assertRaisesRegex(StreamError, 'different|mismatch|replace'):
                scope.claim('another')
            with self.assertRaisesRegex(StreamError, 'different|mismatch|replace'):
                scope.bind(replace(self.stream, id='another'), authority=self.authority)
            scope.bind(self.stream, authority=self.authority)

    def test_enrollment_unconfirmed_claim_durability_keeps_retry_identity(self):
        with self.runtime.enrollment() as scope:
            with patch.object(self.runtime, '_sync_directory', side_effect=OSError('sync failed')):
                with self.assertRaisesRegex(StreamError, 'sync failed'):
                    scope.claim(self.stream.id)
        fresh = Runtime(self.root / 'state', 'local', FrontendMode.HERDR)
        with fresh.enrollment() as scope:
            self.assertEqual(scope.stream_id, self.stream.id)
            scope.bind(self.stream, authority=self.authority)

    def test_enrollment_claim_schema_and_symlinks_fail_closed(self):
        with self.runtime.enrollment() as scope:
            scope.claim(self.stream.id)
        path = next(self.runtime.state_dir.rglob('enrollments/**/*.json'))
        original = path.read_text()
        data = json.loads(original)
        for change in ({'stream_id': None}, {'workspace_id': 'wrong'}, {'extra': True}, {'version': True}):
            path.write_text(json.dumps({**data, **change}))
            with self.subTest(change=change), self.assertRaises(StreamError):
                with self.runtime.enrollment():
                    pass
        path.write_text(original.replace('"version": 1', '"version": 1, "version": 1'))
        with self.assertRaisesRegex(StreamError, 'duplicate JSON'):
            with self.runtime.enrollment():
                pass
        target = self.root / 'external-claim.json'
        path.rename(target)
        path.symlink_to(target)
        with self.assertRaisesRegex(StreamError, 'symlink'):
            with self.runtime.enrollment():
                pass

    def test_enrollment_scope_is_exact_workspace_and_frontend_endpoint(self):
        with self.runtime.enrollment() as first:
            first.claim(self.stream.id)
            self.herdr.endpoint.return_value = HerdrEndpoint('/another.sock', None)
            with self.runtime.enrollment() as endpoint:
                self.assertIsNone(endpoint.stream_id)
                endpoint.claim('endpoint-stream')
            self.herdr.endpoint.return_value = HerdrEndpoint('/herdr.sock', None)
            self.herdr.current_pane.return_value = HerdrPane('p2', 't2', 'w2', '/irrelevant')
            self.herdr.resolve_workspace.return_value = HerdrWorkspace('w2', 'Second')
            with self.runtime.enrollment() as workspace:
                self.assertIsNone(workspace.stream_id)
                workspace.claim('workspace-stream')
        self.herdr.current_pane.return_value = HerdrPane('p1', 't1', 'w1', '/irrelevant')
        self.herdr.resolve_workspace.return_value = HerdrWorkspace('w1', 'Example')
        with self.runtime.enrollment() as original:
            self.assertEqual(original.stream_id, self.stream.id)

    def test_enrollment_failed_bind_preserves_claim_for_explicit_retry(self):
        write = self.runtime._write
        def fail_binding(path, data):
            if 'associations' in path.parts:
                raise StreamError('disk failed')
            return write(path, data)
        with self.runtime.enrollment() as scope:
            scope.claim(self.stream.id)
            with patch.object(self.runtime, '_write', side_effect=fail_binding):
                with self.assertRaisesRegex(StreamError, 'claim|retry|inspect'):
                    scope.bind(self.stream, authority=self.authority)
        fresh = Runtime(self.root / 'state', 'local', FrontendMode.HERDR)
        with fresh.enrollment() as scope:
            self.assertEqual(scope.stream_id, self.stream.id)
            scope.bind(self.stream, authority=self.authority)

    def test_enrollment_explicit_identity_detects_native_mismatch(self):
        with self.runtime.enrollment() as scope:
            scope.bind(self.stream, authority=self.authority)
        with patch.dict(os.environ, {'STREAMS_STREAM_ID': 'another'}):
            with self.assertRaisesRegex(StreamError, 'mismatch|different'):
                with self.runtime.enrollment():
                    self.fail('mismatched explicit identity accepted')
        with patch.dict(os.environ, {'STREAMS_STREAM_ID': ''}):
            with self.assertRaisesRegex(StreamError, 'STREAMS_STREAM_ID'):
                with self.runtime.enrollment():
                    pass

    def test_enrollment_explicit_identity_is_available_without_binding(self):
        with patch.dict(os.environ, {'STREAMS_STREAM_ID': self.stream.id}):
            with self.runtime.enrollment() as scope:
                self.assertEqual(scope.stream_id, self.stream.id)
                with self.assertRaisesRegex(StreamError, 'different|mismatch|replace'):
                    scope.bind(replace(self.stream, id='another'), authority=self.authority)
                scope.bind(self.stream, authority=self.authority)

    def test_enrollment_rejects_nonready_and_corrupt_associations(self):
        with self.runtime.enrollment() as scope:
            scope.bind(self.stream, authority=self.authority)
        path = next(self.runtime.state_dir.rglob('associations/**/*.json'))
        data = json.loads(path.read_text())
        data['ready'] = False
        path.write_text(json.dumps(data))
        with self.assertRaisesRegex(StreamError, 'partial|ready'):
            with self.runtime.enrollment():
                pass
        path.write_text('{broken')
        with self.assertRaisesRegex(StreamError, 'runtime state'):
            with self.runtime.enrollment():
                pass

    def test_enrollment_rejects_corrupt_or_conflicting_claim(self):
        with self.runtime.enrollment() as scope:
            scope.bind(self.stream, authority=self.authority)
        path = next(self.runtime.state_dir.rglob('enrollments/**/*.json'))
        data = json.loads(path.read_text())
        data['stream_id'] = 'another'
        path.write_text(json.dumps(data))
        with self.assertRaisesRegex(StreamError, 'mismatch|different'):
            with self.runtime.enrollment():
                pass
        data['workspace_id'] = 'another-workspace'
        path.write_text(json.dumps(data))
        with self.assertRaisesRegex(StreamError, 'identity'):
            with self.runtime.enrollment():
                pass
        path.write_text('{broken')
        with self.assertRaisesRegex(StreamError, 'runtime state'):
            with self.runtime.enrollment():
                pass

    def test_enrollment_exact_herdr_confirmation_and_native_errors_fail_closed(self):
        self.herdr.resolve_workspace.return_value = HerdrWorkspace('other', 'Other')
        with self.assertRaisesRegex(StreamError, 'exact|mismatch'):
            with self.runtime.enrollment():
                pass
        self.herdr.resolve_workspace.side_effect = StreamError('native unavailable')
        with self.assertRaisesRegex(StreamError, 'native unavailable'):
            with self.runtime.enrollment():
                pass

    def test_enrollment_auto_does_not_swallow_incomplete_or_ambiguous_context(self):
        runtime = Runtime(self.root / 'state', 'local', FrontendMode.AUTO)
        for env in ({'HERDR_ENV': '1'}, {'HERDR_ENV': '1', 'HERDR_PANE_ID': 'p', 'CMUX_WORKSPACE_ID': 'w'}):
            with patch.dict(os.environ, env), self.assertRaises(StreamError):
                with runtime.enrollment():
                    pass
        self.herdr.endpoint.assert_not_called()

    def test_enrollment_explicit_identity_does_not_hide_native_errors(self):
        self.herdr.current_pane.side_effect = StreamError('caller unavailable')
        with patch.dict(os.environ, {'STREAMS_STREAM_ID': self.stream.id}):
            with self.assertRaisesRegex(StreamError, 'caller unavailable'):
                with self.runtime.enrollment():
                    pass

    def test_enrollment_cmux_requires_exact_caller_uuid(self):
        runtime = Runtime(self.root / 'state', 'local', FrontendMode.CMUX)
        with patch('_streams.catalog_runtime.CmuxClient') as transport:
            client = transport.return_value
            client.workspaces.return_value = [CmuxWorkspace({'id': 'uuid-1', 'ref': 'workspace:1', 'title': 'Example'})]
            for workspace in (None, 'workspace:1', 'unknown'):
                env = {} if workspace is None else {'CMUX_WORKSPACE_ID': workspace}
                with patch.dict(os.environ, env), self.assertRaisesRegex(StreamError, 'exact|caller'):
                    with runtime.enrollment():
                        pass
            with patch.dict(os.environ, {'CMUX_WORKSPACE_ID': 'uuid-1'}):
                with runtime.enrollment() as scope:
                    scope.bind(self.stream, authority=self.authority)
            client.run.assert_not_called()
            client.json_from_command.assert_not_called()

    def test_enrollment_scopes_converge_while_lock_covers_catalog_callback(self):
        entered = threading.Event()
        release = threading.Event()
        second_entered = threading.Event()
        def first():
            with self.runtime.enrollment() as scope:
                scope.claim(self.stream.id)
                entered.set()
                self.assertTrue(release.wait(5))
                scope.bind(self.stream, authority=self.authority)
        def second():
            fresh = Runtime(self.root / 'state', 'local', FrontendMode.HERDR)
            with fresh.enrollment() as scope:
                second_entered.set()
                return scope.stream_id
        with ThreadPoolExecutor(max_workers=2) as pool:
            one = pool.submit(first)
            self.assertTrue(entered.wait(5))
            two = pool.submit(second)
            try:
                self.assertFalse(second_entered.wait(0.2))
            finally:
                release.set()
            one.result(timeout=5)
            self.assertEqual(two.result(timeout=5), self.stream.id)

    def test_enrollment_scope_cannot_bind_after_exit(self):
        with self.runtime.enrollment() as scope:
            scope.claim(self.stream.id)
        with self.assertRaisesRegex(StreamError, 'scope|closed'):
            scope.bind(self.stream, authority=self.authority)

    def test_enrolled_sessionless_workspace_open_only_focuses(self):
        with self.runtime.enrollment() as scope:
            scope.bind(self.stream, authority=self.authority)
        result = self.open((self.session,), cwd=self.cwd)
        self.assertTrue(result['reused'])
        self.assertIsNone(result['session_id'])
        self.herdr.command_result.assert_called_once_with(['api', 'snapshot'], 'session_snapshot')
        self.herdr.run.assert_called_once_with(['workspace', 'focus', 'w1'])

    def test_enrolled_sessionless_cmux_open_never_starts_selected_session(self):
        runtime = Runtime(self.root / 'state', 'local', FrontendMode.CMUX)
        with patch('_streams.catalog_runtime.CmuxClient') as transport:
            client = transport.return_value
            client.workspaces.return_value = [CmuxWorkspace({'id': 'uuid-1', 'title': 'Example'})]
            with patch.dict(os.environ, {'CMUX_WORKSPACE_ID': 'uuid-1'}):
                with runtime.enrollment() as scope:
                    scope.bind(self.stream, authority=self.authority)
            result = runtime.open(self.stream, (self.session,), authority=self.authority, cwd=self.cwd)
            self.assertTrue(result['reused'])
            self.assertIsNone(result['session_id'])
            client.run.assert_called_once_with(['cmux', 'select-workspace', '--workspace', 'uuid-1'])
            client.json_from_command.assert_not_called()

    def test_missing_mapping_requires_explicit_cwd(self):
        with self.assertRaisesRegex(StreamError, '--cwd'):
            self.open()
        self.herdr.command_result.assert_not_called()

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
        self.assertEqual(args[-3:], ['--', '--session', str(self.saved)])
        create = next(call for call in self.herdr.command_result.call_args_list if call.args[0][:2] == ['workspace', 'create'])
        self.assertEqual(create.args[0][3], str(session_cwd.resolve()))

    def test_ambiguity_and_reference_precedence(self):
        second = replace(self.session, id='second', title=self.session.id)
        ambiguous = self.open((self.session, second), cwd=self.cwd)
        self.assertIsNone(ambiguous['session_id'])
        self.assertIn('--session', ambiguous['needs_attention'][0])
        self.herdr.command_result.assert_called_once()
        self.assertEqual(self.runtime._select_session(self.stream, (self.session, second), self.session.id), self.session)

    def test_title_reference_and_case_insensitive_substring(self):
        two = replace(self.session, id='second', title='Implementation followup')
        for index, reference in enumerate(('Implementation', 'PLEMENTATION')):
            self.running = False
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
        self.herdr.command_result.assert_called_once_with(['api', 'snapshot'], 'session_snapshot')
        self.herdr.run.assert_called_with(['workspace', 'focus', 'w1'])
        self.assertEqual(fresh.current_stream_id(), self.stream.id)

    def test_reused_workspace_does_not_launch_different_session(self):
        self.open(cwd=self.cwd)
        self.assertTrue(self.open((self.session,))['reused'])
        effects = [call for call in self.herdr.command_result.call_args_list if call.args[0] != ['api', 'snapshot']]
        self.assertEqual(len(effects), 1)

    def test_stale_workspace_never_relaunched(self):
        self.open(cwd=self.cwd)
        self.herdr.command_result.side_effect = StreamError('snapshot timed out')
        with self.assertRaisesRegex(StreamError, 'timed out'):
            self.open()
        self.assertEqual(self.herdr.command_result.call_count, 2)

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

    def test_other_scope_ready_cmux_binding_blocks_herdr_open_resume_and_claim(self):
        with patch('_streams.catalog_runtime.CmuxClient'):
            from _streams.frontend import Frontend
            _, scope = self.runtime._transport(Frontend.CMUX)
        path = self.runtime._association_path(scope, 'cmux-workspace')
        self.runtime._write(path, {'stream_id': self.stream.id, 'workspace_id': 'cmux-workspace',
                                   'session_id': self.session.id, 'command': 'pi --resume saved', 'ready': True})
        with patch.object(self.runtime, '_exec_session') as execute:
            with self.assertRaisesRegex(StreamError, 'another scope'):
                self.open((self.session,), cwd=self.cwd)
            with self.assertRaisesRegex(StreamError, 'another scope'):
                self.runtime.resume(self.stream, (self.session,), authority=self.authority, cwd=self.cwd)
            with self.runtime.enrollment() as enrollment:
                with self.assertRaisesRegex(StreamError, 'another scope'):
                    enrollment.claim(self.stream.id)
            execute.assert_not_called()
        self.assertTrue(path.exists())
        self.herdr.command_result.assert_not_called()
        self.herdr.run.assert_not_called()

    def test_other_scope_attempt_and_claim_block_open_resume_and_enrollment(self):
        from _streams.catalog_runtime import _key
        for collection in ('attempts', 'enrollments'):
            with self.subTest(collection=collection):
                if collection == 'attempts':
                    path = self.runtime.state_dir / collection / 'other-endpoint' / (_key(self.stream.id) + '.json')
                    record = {'stream_id': self.stream.id, 'session_id': None, 'command': None,
                              'cwd': str(self.cwd), 'workspace_id': None}
                else:
                    path = self.runtime._enrollment_path('other-endpoint', 'w1')
                    record = {'stream_id': self.stream.id, 'workspace_id': 'w1'}
                self.runtime._write(path, record)
                with patch.object(self.runtime, '_exec_session') as execute:
                    for mode in (FrontendMode.HERDR, FrontendMode.NONE):
                        runtime = Runtime(self.root / 'state', 'local', mode)
                        with self.assertRaises(StreamError):
                            runtime.resume(self.stream, (self.session,), authority=self.authority, cwd=self.cwd)
                    with self.assertRaises(StreamError):
                        self.open(cwd=self.cwd)
                    with self.runtime.enrollment() as enrollment:
                        with self.assertRaises(StreamError):
                            enrollment.claim(self.stream.id)
                    execute.assert_not_called()
                self.assertTrue(path.exists())
                path.unlink()
        self.herdr.command_result.assert_not_called()
        self.herdr.run.assert_not_called()

    def test_other_scope_malformed_attempt_is_not_silently_skipped(self):
        from _streams.catalog_runtime import _key
        path = self.runtime.state_dir / 'attempts' / 'other' / (_key(self.stream.id) + '.json')
        self.runtime._write(path, {'stream_id': 'wrong-identity', 'session_id': None, 'command': None,
                                  'cwd': str(self.cwd), 'workspace_id': None})
        with self.assertRaisesRegex(StreamError, 'invalid open attempt identity'):
            self.open(cwd=self.cwd)
        path.write_text('{broken')
        with self.assertRaisesRegex(StreamError, 'invalid open attempt'):
            self.open(cwd=self.cwd)
        self.herdr.command_result.assert_not_called()

    def test_cmux_open_serializes_with_herdr_open_on_global_stream_lock(self):
        entered, release, attempted = threading.Event(), threading.Event(), threading.Event()
        cmux = Runtime(self.root / 'state', 'local', FrontendMode.CMUX)
        def create(args):
            entered.set()
            self.assertTrue(release.wait(5))
            return {'workspace_id': 'c1'}
        def open_herdr():
            attempted.set()
            return self.open(cwd=self.cwd)
        with patch('_streams.catalog_runtime.CmuxClient') as transport:
            transport.return_value.json_from_command.side_effect = create
            with ThreadPoolExecutor(max_workers=2) as pool:
                first = pool.submit(cmux.open, self.stream, (), authority=self.authority, cwd=self.cwd)
                self.assertTrue(entered.wait(5))
                second = pool.submit(open_herdr)
                self.assertTrue(attempted.wait(5))
                try:
                    self.assertFalse(second.done())
                    self.herdr.command_result.assert_not_called()
                finally:
                    release.set()
                self.assertFalse(first.result(timeout=5)['reused'])
                with self.assertRaisesRegex(StreamError, 'another scope'):
                    second.result(timeout=5)
            transport.return_value.json_from_command.assert_called_once()
        self.assertEqual(len(list(self.runtime.state_dir.glob('stream-locks/*.lock'))), 1)
        self.herdr.command_result.assert_not_called()

    def test_cmux_claim_serializes_with_herdr_open_on_global_stream_lock(self):
        entered, release, attempted = threading.Event(), threading.Event(), threading.Event()
        write = self.runtime._write
        def pause(path, data):
            if 'enrollments' in path.parts:
                entered.set()
                self.assertTrue(release.wait(5))
            return write(path, data)
        cmux = Runtime(self.root / 'state', 'local', FrontendMode.CMUX)
        def claim():
            with cmux.enrollment() as enrollment:
                enrollment.claim(self.stream.id)
        def open_herdr():
            attempted.set()
            return self.open(cwd=self.cwd)
        with patch('_streams.catalog_runtime.CmuxClient') as transport, patch.dict(os.environ, {'CMUX_WORKSPACE_ID': 'c1'}):
            transport.return_value.workspaces.return_value = [CmuxWorkspace({'id': 'c1', 'title': 'Example'})]
            with patch.object(cmux, '_write', side_effect=pause), ThreadPoolExecutor(max_workers=2) as pool:
                first = pool.submit(claim)
                self.assertTrue(entered.wait(5))
                second = pool.submit(open_herdr)
                self.assertTrue(attempted.wait(5))
                try:
                    self.assertFalse(second.done())
                    self.herdr.command_result.assert_not_called()
                finally:
                    release.set()
                first.result(timeout=5)
                with self.assertRaisesRegex(StreamError, 'another scope'):
                    second.result(timeout=5)
        self.assertEqual(len(list(self.runtime.state_dir.glob('stream-locks/*.lock'))), 1)
        self.herdr.command_result.assert_not_called()

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
                locks.extend((self.runtime.state_dir / 'stream-locks').glob('*.lock'))
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
        execute.assert_called_once_with('pi', ['pi', '--session', str(self.saved)])
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
        version = data['version']
        path.write_text(original.replace(f'"version": {version}', f'"version": {version}, "version": {version}'))
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
        self.herdr.resolve_workspace.return_value = HerdrWorkspace('other', 'Other')
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
        with self.assertRaisesRegex(StreamError, 'pending|partial open'):
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
            import shlex
            self.assertEqual(args[args.index('--command') + 1], shlex.join(['pi', '--session', str(self.saved)]))
            self.assertIn('STREAMS_STREAM_ID=' + self.stream.id, args)
            self.assertIn('HOME=' + str(Path.home()), args)
            self.assertIn('PI_CODING_AGENT_DIR=' + (os.environ.get('PI_CODING_AGENT_DIR') or str(Path.home() / '.pi/agent')), args)
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
        args = next(call.args[0] for call in self.herdr.command_result.call_args_list if call.args[0][:2] == ['workspace', 'create'])
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
