from __future__ import annotations

import copy
import json
import os
import sys
import tempfile
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor
from dataclasses import replace
from pathlib import Path
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'src'))

from _streams.catalog.model import Session, Stream
from _streams.catalog.operations import Authority
from _streams.catalog_arrangement import inventory, validate_plan
from _streams.catalog_conversations import conversation_file, file_identity, native_identity
from _streams.catalog_runtime import Runtime
from _streams.errors import StreamError
from _streams.frontend import FrontendMode
from _streams.herdr_binding import HerdrEndpoint


class ArrangementTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.cwd = self.root / 'checkout'
        self.cwd.mkdir()
        self.stream = Stream('effort', 'Example', 'Goal', 'host')
        self.authority = Authority('host', 'epoch')
        self.runtime = Runtime(self.root / 'state', 'host', FrontendMode.HERDR)
        self.runtime.remember_directory(self.stream.id, self.cwd)
        env = patch.dict(os.environ, {'PI_CODING_AGENT_DIR': str(self.root / 'pi')}, clear=True)
        env.start()
        self.addCleanup(env.stop)
        transport = patch('_streams.catalog_runtime.HerdrClient')
        self.client = transport.start().return_value
        self.addCleanup(transport.stop)
        self.client.endpoint.return_value = HerdrEndpoint('/test.sock', None)
        self.client.command_result.side_effect = self.command
        self.client.run.side_effect = self.native_run
        self.snapshot = {key: [] for key in ('workspaces', 'tabs', 'panes', 'layouts', 'agents')}
        self.serial = 0
        self.a = self.saved('a')
        self.b = self.saved('b')
        self.c = self.saved('c')
        self.sessions = (self.a, self.b, self.c)

    def saved(self, name, **kwargs):
        path = self.root / 'pi' / 'sessions' / f'{name}.jsonl'
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({'type': 'session', 'id': name}) + '\n')
        return Session(name, 'host', self.stream.id, 'pi', name, name, **kwargs)

    def pane(self, workspace, tab, cwd):
        self.serial += 1
        pane = {'pane_id': f'p{self.serial}', 'terminal_id': f'term{self.serial}',
                'workspace_id': workspace, 'tab_id': tab, 'cwd': str(cwd)}
        self.snapshot['panes'].append(pane)
        next(l for l in self.snapshot['layouts'] if l['tab_id'] == tab)['panes'].append({'pane_id': pane['pane_id']})
        return pane

    def tab(self, workspace, cwd, label=None):
        self.serial += 1
        tab = {'tab_id': f't{self.serial}', 'workspace_id': workspace,
               'number': len(self.snapshot['tabs']) + 1, 'label': label or str(len(self.snapshot['tabs']) + 1)}
        self.snapshot['tabs'].append(tab)
        self.snapshot['layouts'].append({'tab_id': tab['tab_id'], 'workspace_id': workspace, 'panes': []})
        return {'tab': tab, 'root_pane': self.pane(workspace, tab['tab_id'], cwd)}

    def live(self, session, workspace, *, tab=None, locator=True):
        if tab is None:
            created = self.tab(workspace, self.cwd)
            pane = created['root_pane']
        else:
            pane = self.pane(workspace, tab, self.cwd)
        agent = {**pane, 'agent': session.harness, 'name': session.id, 'status': 'done',
                 'agent_session': {'agent': session.harness, 'kind': 'path' if locator else 'id',
                                   'value': str(self.root / 'pi' / 'sessions' / f'{session.harness_session_id}.jsonl') if locator else session.harness_session_id}}
        self.snapshot['agents'].append(agent)
        return agent

    def bind(self, *, claim=False):
        self.snapshot['workspaces'].append({'workspace_id': 'original', 'label': 'Example'})
        _, scope = self.runtime._transport(self.runtime._frontend(None))
        self.runtime._write(self.runtime._association_path(scope, 'original'),
                            {'stream_id': self.stream.id, 'workspace_id': 'original', 'session_id': self.a.id,
                             'command': 'historical launch of A', 'ready': True})
        if claim:
            self.runtime._write(self.runtime._enrollment_path(scope, 'original'),
                                {'stream_id': self.stream.id, 'workspace_id': 'original'})
        return scope

    def open(self, sessions=None, **kwargs):
        return self.runtime.open(self.stream, self.sessions if sessions is None else sessions, authority=self.authority, **kwargs)

    def capture(self, sessions=None):
        return self.runtime.capture_arrangement(self.stream, self.sessions if sessions is None else sessions, authority=self.authority)

    def close(self):
        self.snapshot = {key: [] for key in self.snapshot}

    def native_run(self, args):
        if args[:2] == ['tab', 'rename']:
            next(t for t in self.snapshot['tabs'] if t['tab_id'] == args[2])['label'] = args[3]
        elif args[:2] not in (['workspace', 'focus'], ['agent', 'focus']):
            raise AssertionError(args)

    def command(self, args, expected):
        if args == ['api', 'snapshot']:
            return {'snapshot': copy.deepcopy(self.snapshot)}
        if args[:2] == ['workspace', 'create']:
            self.serial += 1
            workspace = {'workspace_id': f'w{self.serial}', 'label': args[args.index('--label') + 1]}
            self.snapshot['workspaces'].append(workspace)
            return {'workspace': workspace, **self.tab(workspace['workspace_id'], args[args.index('--cwd') + 1])}
        if args[:2] == ['tab', 'create']:
            self.assertEqual(expected, 'tab_created')
            return self.tab(args[args.index('--workspace') + 1], args[args.index('--cwd') + 1],
                            args[args.index('--label') + 1] if '--label' in args else None)
        if args[:2] == ['pane', 'split']:
            self.assertEqual(expected, 'pane_split')
            source = next(p for p in self.snapshot['panes'] if p['pane_id'] == args[args.index('--pane') + 1])
            return {'pane': self.pane(source['workspace_id'], source['tab_id'], args[args.index('--cwd') + 1])}
        if args[:2] == ['agent', 'start']:
            pane = next(p for p in self.snapshot['panes'] if p['pane_id'] == args[args.index('--pane') + 1])
            agent = {**pane, 'name': args[2], 'agent': args[args.index('--kind') + 1], 'interactive_ready': True,
                     'agent_session': {'agent': 'pi', 'kind': 'path', 'value': args[-1]}}
            self.snapshot['agents'].append(agent)
            return {'agent': agent}
        raise AssertionError(args)

    def effects(self, kind):
        return [c.args[0] for c in self.client.command_result.call_args_list if c.args[0][:2] == kind]

    def test_empty_open_and_capture_do_not_poison_later_single_session_restore(self):
        result = self.open(())
        self.assertEqual(result['restored'], [])
        self.assertFalse(list(self.runtime.state_dir.glob('arrangements/*/*.json')))
        self.assertEqual(self.capture()['tabs'], [])
        self.assertFalse(list(self.runtime.state_dir.glob('arrangements/*/*.json')))
        self.close()
        self.assertEqual(self.open((self.a,))['restored'], ['a'])

    def test_legacy_empty_arrangement_uses_initial_candidate_policy(self):
        _, scope = self.runtime._transport(self.runtime._frontend(None))
        path = self.runtime._arrangement_path(scope, self.stream.id)
        self.runtime._write(path, {'stream_id': self.stream.id, 'workspace_id': 'closed', 'tabs': []})
        prepared, skipped, attention = self.runtime._restore_entries(
            self.stream, (self.a,), self.runtime._saved_arrangement(scope, self.stream.id), None, None)
        self.assertEqual([e['session'].id for t in prepared for e in t['entries']], ['a'])
        self.assertEqual((skipped, attention), ([], []))
        prepared, _, attention = self.runtime._restore_entries(
            self.stream, (self.a, self.b), self.runtime._saved_arrangement(scope, self.stream.id), None, None)
        self.assertEqual(prepared, [])
        self.assertTrue(attention)

    def test_empty_capture_preserves_last_nonempty_plan(self):
        scope = self.bind()
        self.live(self.a, 'original')
        plan = self.capture()
        path = self.runtime._arrangement_path(scope, self.stream.id)
        saved = path.read_bytes()
        self.snapshot['agents'] = []
        self.assertEqual(self.capture()['tabs'], [])
        self.assertEqual(path.read_bytes(), saved)
        self.close()
        self.assertEqual(self.open()['restored'], [plan['tabs'][0]['sessions'][0]['session_id']])

    def test_saved_plan_rejects_malformed_workspace_tabs_labels_and_native_identity(self):
        self.bind()
        self.live(self.a, 'original')
        plan = self.capture()
        for field, value in (('workspace_id', None), ('workspace_id', ''), ('workspace_id', 'bad\x7f')):
            invalid = copy.deepcopy(plan)
            invalid[field] = value
            with self.subTest(field=field, value=value), self.assertRaises(StreamError):
                validate_plan(invalid, self.stream.id)
        for field, value in (('tab_id', 'bad\x7f'), ('label', 'bad\nlabel'), ('label', 'bad\x7f')):
            invalid = copy.deepcopy(plan)
            invalid['tabs'][0][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(StreamError):
                validate_plan(invalid, self.stream.id)
        for field in ('session_id', 'native_id', 'harness', 'locator'):
            invalid = copy.deepcopy(plan)
            invalid['tabs'][0]['sessions'][0][field] = 'bad\x7f'
            with self.subTest(field=field), self.assertRaises(StreamError):
                validate_plan(invalid, self.stream.id)

    def test_discovery_ignores_valid_unrelated_filename_hint_but_not_explicit_mismatch(self):
        decoy = self.root / 'pi' / 'sessions' / 'a-decoy.jsonl'
        decoy.write_text('{"type":"session","id":"unrelated"}\n')
        self.assertEqual(conversation_file('pi', 'a'), self.root / 'pi' / 'sessions' / 'a.jsonl')
        with self.assertRaisesRegex(StreamError, 'mismatch'):
            conversation_file('pi', 'a', str(decoy))
        decoy.write_text('{broken')
        with self.assertRaisesRegex(StreamError, 'unavailable'):
            conversation_file('pi', 'a')

    def test_resume_recovers_unique_cross_scope_locator_without_launch_frontend(self):
        scope = self.bind()
        self.live(self.a, 'original')
        plan = self.capture()
        self.runtime._association_path(scope, 'original').unlink()
        saved = self.root / 'pi' / 'sessions' / 'a.jsonl'
        moved = self.root / 'not-discoverable.jsonl'
        saved.rename(moved)
        plan['tabs'][0]['sessions'][0]['locator'] = str(moved)
        self.runtime._write(self.runtime._arrangement_path(scope, self.stream.id), plan)
        runtime = Runtime(self.root / 'state', 'host', FrontendMode.NONE)
        with patch.object(runtime, '_exec_session') as execute:
            runtime.resume(self.stream, (self.a,), authority=self.authority)
            self.assertEqual(execute.call_args.args[-1], str(moved))
            self.runtime._write(self.runtime._arrangement_path('another', self.stream.id), plan)
            execute.reset_mock()
            with self.assertRaisesRegex(StreamError, 'multiple saved arrangements'):
                runtime.resume(self.stream, (self.a,), authority=self.authority)
            execute.assert_not_called()

    def test_default_focus_does_not_select_a_session_or_need_a_directory(self):
        self.bind()
        a = self.live(self.a, 'original')
        self.live(self.b, 'original', tab=a['tab_id'])
        self.runtime.locations.path(self.stream.id, None).unlink()
        result = self.open((replace(self.a, status='done'), self.b, self.c))
        self.assertTrue(result['reused'])
        self.assertFalse(result['restored'])
        self.assertFalse(self.effects(['agent', 'start']))
        self.assertFalse(self.effects(['workspace', 'create']))
        plan = self.capture()
        self.assertEqual([e['session_id'] for e in plan['tabs'][0]['sessions']], ['a', 'b'])
        self.client.run.assert_called_with(['workspace', 'focus', 'original'])

    def test_explicit_live_b_focuses_exact_native_pane_even_when_done(self):
        self.bind()
        self.live(self.a, 'original')
        b = self.live(self.b, 'original', locator=False)
        result = self.open((self.a, replace(self.b, status='done')), session_ref='b')
        self.assertEqual(result['session_id'], 'b')
        self.client.run.assert_called_with(['agent', 'focus', b['pane_id']])
        self.assertFalse(self.effects(['agent', 'start']))

    def test_explicit_absent_b_adds_tab_without_replacing_a(self):
        self.bind()
        self.live(self.a, 'original')
        result = self.open((self.a, replace(self.b, status='parked')), session_ref='b')
        self.assertTrue(result['reused'])
        self.assertEqual(result['restored'], ['b'])
        self.assertEqual(len(self.effects(['tab', 'create'])), 1)
        self.assertEqual(len(self.snapshot['agents']), 2)
        self.assertFalse(self.effects(['workspace', 'create']))
        self.assertEqual([t['sessions'][0]['session_id'] for t in self.capture()['tabs']], ['a', 'b'])

    def test_saved_layout_restores_only_observed_unfinished_not_catalog_helpers(self):
        self.bind()
        a = self.live(self.a, 'original')
        self.live(self.b, 'original', tab=a['tab_id'])
        self.capture()
        self.close()
        result = self.open((self.a, replace(self.b, status='done'), self.c))
        self.assertEqual(result['restored'], ['a'])
        self.assertEqual(result['skipped'][0]['session_id'], 'b')
        self.assertEqual(len(self.snapshot['agents']), 1)
        self.assertFalse(any(c[-1].endswith('c.jsonl') for c in self.effects(['agent', 'start'])))

    def test_closed_ready_binding_and_matching_claim_are_archived(self):
        self.bind(claim=True)
        self.live(self.a, 'original')
        self.capture()
        self.close()
        result = self.open()
        self.assertEqual(result['restored'], ['a'])
        self.assertFalse(result['reused'])
        archived = json.loads(next(self.runtime.state_dir.glob('closed/*/*.json')).read_text())
        self.assertEqual(archived['binding']['workspace_id'], 'original')
        self.assertEqual(archived['claim']['stream_id'], self.stream.id)
        self.assertFalse(list(self.runtime.state_dir.glob('enrollments/*/*.json')))

    def test_timeout_or_incomplete_inventory_never_proves_close(self):
        self.bind()
        self.live(self.a, 'original')
        self.capture()
        self.close()
        saved = next(self.runtime.state_dir.glob('arrangements/*/*.json')).read_bytes()
        for response in (StreamError('snapshot timed out'), {'snapshot': {'workspaces': []}}):
            with self.subTest(response=response):
                self.client.command_result.side_effect = response if isinstance(response, Exception) else lambda *args: response
                with self.assertRaises(StreamError):
                    self.open()
                self.assertFalse(list(self.runtime.state_dir.glob('closed/*/*.json')))
                self.assertTrue(list(self.runtime.state_dir.glob('associations/*/*.json')))
                self.assertEqual(next(self.runtime.state_dir.glob('arrangements/*/*.json')).read_bytes(), saved)

    def test_surviving_unmatched_claim_and_partial_binding_block(self):
        scope = self.bind(claim=True)
        path = self.runtime._association_path(scope, 'original')
        data = self.runtime._association(path)
        self.runtime._write(path, {**{k: v for k, v in data.items() if k != 'version'}, 'ready': False})
        self.close()
        with self.assertRaisesRegex(StreamError, 'claim|partial'):
            self.open()
        path.unlink()
        with self.assertRaisesRegex(StreamError, 'claim'):
            self.open()
        self.assertFalse(self.effects(['workspace', 'create']))

    def test_no_arrangement_multiple_candidates_opens_shell_with_diagnostic(self):
        result = self.open()
        self.assertIsNone(result['session_id'])
        self.assertIn('--session', result['needs_attention'][0])
        self.assertEqual(result['restored'], [])
        self.assertFalse(self.effects(['agent', 'start']))

    def test_no_arrangement_sole_unfinished_candidate_is_initial_conversation(self):
        result = self.open((self.a, replace(self.b, status='done'), replace(self.c, status='parked')))
        self.assertEqual(result['restored'], ['a'])
        self.assertEqual(self.effects(['agent', 'start'])[0][-1], str(self.root / 'pi/sessions/a.jsonl'))

    def test_missing_or_ambiguous_saved_file_never_starts_fresh(self):
        (self.root / 'pi/sessions/a.jsonl').unlink()
        result = self.open((self.a,))
        self.assertFalse(result['restored'])
        self.assertIn('unavailable', result['unavailable'][0]['reason'])
        self.assertFalse(self.effects(['agent', 'start']))
        # A second runtime with two verified candidates must not pick by mtime.
        self.runtime = Runtime(self.root / 'second-state', 'host', FrontendMode.HERDR)
        self.runtime.remember_directory(self.stream.id, self.cwd)
        self.saved('a')
        duplicate = self.root / 'pi/sessions/duplicate-a.jsonl'
        duplicate.write_text('{"type":"session","id":"a"}\n')
        result = self.open((self.a,))
        self.assertIn('ambiguous', result['unavailable'][0]['reason'])
        self.assertFalse(self.effects(['agent', 'start']))

    def test_known_locator_mismatch_skips_instead_of_id_fallback(self):
        self.bind()
        self.live(self.a, 'original')
        self.capture()
        (self.root / 'pi/sessions/a.jsonl').write_text('{"type":"session","id":"b"}\n')
        self.close()
        result = self.open((self.a,))
        self.assertFalse(result['restored'])
        self.assertIn('mismatch', result['unavailable'][0]['reason'])
        self.assertFalse(self.effects(['agent', 'start']))

    def test_capture_failure_does_not_replace_last_good_layout(self):
        self.bind()
        self.live(self.a, 'original')
        self.capture()
        path = next(self.runtime.state_dir.glob('arrangements/*/*.json'))
        original = path.read_bytes()
        agent = self.snapshot['agents'][0]
        for change in ({'agent_session': {'agent': 'pi', 'kind': 'path', 'value': 'relative.jsonl'}},
                       {'terminal_id': 'wrong'}, {'agent_session': {'agent': 'pi', 'kind': 'path', 'value': str(self.root / 'missing')}}):
            before = copy.deepcopy(agent)
            agent.update(change)
            with self.assertRaises(StreamError):
                self.capture()
            self.assertEqual(path.read_bytes(), original)
            agent.clear()
            agent.update(before)
        self.client.command_result.side_effect = StreamError('snapshot unavailable')
        with self.assertRaises(StreamError):
            self.capture()
        self.assertEqual(path.read_bytes(), original)

    def test_capture_matches_real_header_not_filename_and_excludes_helpers(self):
        self.bind()
        agent = self.live(self.a, 'original')
        helper = self.saved('helper')
        actual = self.root / 'pi/sessions/a.jsonl'
        actual.write_text('{"type":"session","id":"b"}\n')
        captured = self.capture((self.a, self.b, helper))
        self.assertEqual(captured['tabs'][0]['sessions'][0]['session_id'], 'b')
        self.assertNotIn('helper', json.dumps(captured))
        self.assertEqual(captured['tabs'][0]['sessions'][0]['locator'], str(actual))
        self.assertEqual(agent['agent_session']['value'], str(actual))

    def test_tab_grouping_order_manual_labels_and_per_session_worktrees(self):
        self.bind()
        a = self.live(self.a, 'original')
        self.live(self.b, 'original', tab=a['tab_id'])
        c = self.live(self.c, 'original')
        self.snapshot['tabs'][0]['label'] = 'Implementation'
        self.snapshot['tabs'][1]['label'] = 'Review notes'
        # Native tab sequence, not catalog order, defines restoration order.
        self.snapshot['tabs'].reverse()
        self.capture()
        for session in self.sessions:
            directory = self.root / f'worktree-{session.id}'
            directory.mkdir()
            self.runtime.remember_directory(self.stream.id, directory, session_id=session.id)
        original_cwd = Path.cwd()
        self.close()
        selected = {'XDG_CONFIG_HOME': '/owner/config', 'XDG_DATA_HOME': '/owner/data',
                    'XDG_STATE_HOME': '/owner/state', 'PI_CODING_AGENT_DIR': '/owner/pi', 'PATH': '/owner/bin'}
        with patch.dict(os.environ, selected):
            result = self.open()
        creates = [*self.effects(['workspace', 'create']), *self.effects(['tab', 'create']), *self.effects(['pane', 'split'])]
        self.assertEqual(len(creates), 3)
        for args in creates:
            values = dict(value.split('=', 1) for index, value in enumerate(args) if args[index - 1] == '--env')
            self.assertEqual({key: values[key] for key in selected}, selected)
            self.assertEqual(values['STREAMS_STREAM_ID'], self.stream.id)
            self.assertNotIn('HERDR_PANE_ID', values)
        self.assertEqual(result['restored'], ['c', 'a', 'b'])
        self.assertEqual(Path.cwd(), original_cwd)
        self.assertEqual([t['label'] for t in self.snapshot['tabs']], ['Review notes', 'Implementation'])
        self.assertEqual(len(self.effects(['tab', 'create'])), 1)
        split = self.effects(['pane', 'split'])[0]
        self.assertEqual(split[split.index('--cwd') + 1], str(self.root / 'worktree-b'))
        self.assertEqual([Path(a['cwd']).name for a in self.snapshot['agents']], ['worktree-c', 'worktree-a', 'worktree-b'])
        plan = self.capture()
        self.assertTrue(all(t['label_mode'] == 'manual' for t in plan['tabs']))

    def test_explicit_cwd_rejects_ambiguous_multisession_restore(self):
        self.bind()
        self.live(self.a, 'original')
        self.live(self.b, 'original')
        self.capture()
        self.close()
        with self.assertRaisesRegex(StreamError, 'ambiguous'):
            self.open(cwd=self.cwd)
        self.assertFalse(self.effects(['workspace', 'create']))

    def test_focus_remembers_explicit_cwd_without_changing_running_terminal(self):
        self.bind()
        self.live(self.a, 'original')
        other = self.root / 'other'
        other.mkdir()
        result = self.open(session_ref='a', cwd=other)
        self.assertTrue(result['reused'])
        self.assertEqual(self.runtime._cwd(self.stream, self.a, None), other)
        self.assertEqual(self.snapshot['agents'][0]['cwd'], str(self.cwd))

    def test_live_native_conversation_refuses_resume_even_status_done(self):
        self.bind()
        self.live(self.a, 'original')
        with patch('_streams.catalog_runtime.os.execvp') as execute:
            with self.assertRaisesRegex(StreamError, 'already running'):
                self.runtime.resume(self.stream, (replace(self.a, status='done'),), authority=self.authority, session_ref='a')
            execute.assert_not_called()

    def test_live_elsewhere_refuses_open_duplicate(self):
        self.bind()
        self.snapshot['workspaces'].append({'workspace_id': 'other', 'label': 'other'})
        self.live(self.b, 'other')
        with self.assertRaisesRegex(StreamError, 'already running'):
            self.open(session_ref='b')
        self.assertFalse(self.effects(['agent', 'start']))

    def test_resume_uses_verified_absolute_saved_path_in_selected_cwd(self):
        original = Path.cwd()
        self.addCleanup(os.chdir, original)
        with patch('_streams.catalog_runtime.os.execvp') as execute:
            self.runtime.resume(self.stream, (self.a,), authority=self.authority, cwd=self.cwd)
        execute.assert_called_once_with('pi', ['pi', '--session', str(self.root / 'pi/sessions/a.jsonl')])
        self.assertEqual(Path.cwd(), self.cwd)

    def test_resume_known_association_inventory_failure_fails_closed(self):
        self.bind()
        self.client.command_result.side_effect = StreamError('snapshot timed out')
        with patch('_streams.catalog_runtime.os.execvp') as execute:
            with self.assertRaisesRegex(StreamError, 'timed out'):
                self.runtime.resume(self.stream, (self.a,), authority=self.authority)
            execute.assert_not_called()

    def test_partial_second_launch_journals_first_and_blocks_retry(self):
        self.bind()
        self.live(self.a, 'original')
        self.live(self.b, 'original')
        self.capture()
        self.close()
        original = self.command
        calls = []
        def fail_second(args, expected):
            if args[:2] == ['agent', 'start']:
                calls.append(args)
                if len(calls) == 2:
                    raise StreamError('uncertain second launch timeout')
            return original(args, expected)
        self.client.command_result.side_effect = fail_second
        with self.assertRaisesRegex(StreamError, "confirmed launches: \\['a'\\].*no rollback"):
            self.open()
        journal = json.loads(next(self.runtime.state_dir.glob('attempts/*/*.json')).read_text())
        self.assertEqual([l['ready'] for l in journal['launches']], [True, False])
        self.assertIsNotNone(journal['workspace_id'])
        self.client.command_result.reset_mock()
        with self.assertRaisesRegex(StreamError, 'pending|partial'):
            self.open()
        self.client.command_result.assert_not_called()

    def test_post_launch_save_failure_reports_confirmed_launch_and_retains_intent(self):
        write = self.runtime._write
        def fail(path, data):
            if 'attempts' in path.parts and any(l['ready'] for l in data.get('launches', [])):
                raise StreamError('disk failed after launch')
            return write(path, data)
        with patch.object(self.runtime, '_write', side_effect=fail):
            with self.assertRaisesRegex(StreamError, "confirmed launches: \\['a'\\].*disk failed"):
                self.open((self.a,))
        self.assertTrue(list(self.runtime.state_dir.glob('attempts/*/*.json')))
        with self.assertRaisesRegex(StreamError, 'pending|partial'):
            self.open((self.a,))
        self.assertEqual(len(self.effects(['agent', 'start'])), 1)

    def test_overlapping_reconstruction_launches_each_conversation_once(self):
        self.bind()
        self.live(self.a, 'original')
        self.live(self.b, 'original')
        self.capture()
        self.close()
        entered, release = threading.Event(), threading.Event()
        command = self.command
        def pause(args, expected):
            if args[:2] == ['workspace', 'create']:
                entered.set()
                self.assertTrue(release.wait(5))
            return command(args, expected)
        self.client.command_result.side_effect = pause
        with ThreadPoolExecutor(max_workers=2) as pool:
            first = pool.submit(self.open)
            self.assertTrue(entered.wait(5))
            fresh = Runtime(self.root / 'state', 'host', FrontendMode.HERDR)
            second = pool.submit(fresh.open, self.stream, self.sessions, authority=self.authority)
            release.set()
            self.assertEqual(first.result(timeout=5)['restored'], ['a', 'b'])
            self.assertTrue(second.result(timeout=5)['reused'])
        self.assertEqual(len(self.effects(['workspace', 'create'])), 1)
        self.assertEqual(len(self.effects(['agent', 'start'])), 2)

    def test_native_identity_and_snapshot_reject_del(self):
        with self.assertRaises(StreamError):
            native_identity({'agent': 'pi', 'kind': 'id', 'value': 'bad\x7f'}, 'pi')
        self.bind()
        self.snapshot['workspaces'][0]['workspace_id'] = 'bad\x7f'
        with self.assertRaises(StreamError):
            inventory(self.client)

    def test_bounded_native_headers_reject_corruption_duplicate_keys_and_ambiguity(self):
        path = self.root / 'header.jsonl'
        invalid = [b'', b'{bad', b'[]\n', b'{"type":"session","id":"a","id":"b"}\n',
                   b'{"type":"session","id":"bad\\u007f"}\n',
                   b'{"type":"session","id":"a","padding":"' + b'x' * 65536 + b'"}\n']
        for value in invalid:
            path.write_bytes(value)
            with self.assertRaises(StreamError):
                file_identity('pi', path)
        path.write_text('{"type":"session_meta","payload":{"id":"codex-id"}}\n')
        self.assertEqual(file_identity('codex', path), 'codex-id')
        path.write_text('{"sessionId":"claude-id"}\n{"sessionId":"claude-id"}\n')
        self.assertEqual(file_identity('claude', path), 'claude-id')
        path.write_text('{"sessionId":"one"}\n{"sessionId":"two"}\n')
        with self.assertRaisesRegex(StreamError, 'ambiguous'):
            file_identity('claude', path)

    def test_codex_claude_validate_saved_locator_but_keep_native_resume_argv(self):
        path = self.root / 'saved.jsonl'
        for harness, content, argv in (
                ('codex', '{"type":"session_meta","payload":{"id":"native"}}\n', ('codex', 'resume', 'native')),
                ('claude', '{"sessionId":"native"}\n', ('claude', '--resume', 'native'))):
            path.write_text(content)
            session = replace(self.a, harness=harness, harness_session_id='native')
            self.assertEqual(self.runtime._command(session, str(path)).argv, argv)
            with self.assertRaisesRegex(StreamError, 'mismatch'):
                self.runtime._command(replace(session, harness_session_id='different'), str(path))

    def test_duplicate_native_snapshot_identity_is_not_a_launch_plan(self):
        self.bind()
        self.live(self.a, 'original')
        self.live(self.a, 'original')
        with self.assertRaisesRegex(StreamError, 'ambiguous'):
            self.capture()
        self.assertFalse(list(self.runtime.state_dir.glob('arrangements/*/*.json')))

    def test_cmux_reports_reconstruction_unavailable_and_keeps_single_open_guard(self):
        runtime = Runtime(self.root / 'cmux-state', 'host', FrontendMode.CMUX)
        with patch('_streams.catalog_runtime.CmuxClient') as transport:
            client = transport.return_value
            client.json_from_command.return_value = {'workspace_id': 'uuid'}
            from _streams.cmux import CmuxWorkspace
            client.workspaces.return_value = [CmuxWorkspace({'id': 'uuid', 'title': 'Example'})]
            result = runtime.open(self.stream, (self.a,), authority=self.authority, cwd=self.cwd)
            self.assertEqual(result['arrangement_support'], 'single-session-only')
            self.assertEqual(result['needs_attention'], [])
            self.assertTrue(runtime.open(self.stream, (self.a,), authority=self.authority)['reused'])
            with self.assertRaisesRegex(StreamError, 'different session'):
                runtime.open(self.stream, (self.b,), authority=self.authority, cwd=self.cwd)
            client.json_from_command.assert_called_once()


if __name__ == '__main__':
    unittest.main()
