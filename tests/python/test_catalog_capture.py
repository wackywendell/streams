from __future__ import annotations

import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'src'))

from _streams.catalog.model import Catalog, Session, Stream
from _streams.catalog.operations import Authority
from _streams.catalog.storage import Snapshot
from _streams.catalog.sync import LocalCatalog
from _streams.catalog.yaml_store import YamlStore
from _streams.catalog_capture import capture_inputs
from _streams.catalog_conversations import native_identity
from _streams.catalog_runtime import Runtime
from _streams.cli import build_parser, command_capture_inputs, main
from _streams.errors import StreamError
from _streams.frontend import FrontendMode


class CaptureInputsTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.enterContext(patch.dict(os.environ, {'PI_CODING_AGENT_DIR': str(self.root / 'pi'),
                                                'STREAMS_STREAM_ID': 'pollution'}, clear=True))
        self.stream = Stream('stream', 'First', 'Goal', 'host')
        self.other = Stream('other', 'Second', 'Goal', 'host')
        self.session = Session('session', 'host', 'stream', 'pi', 'native', 'Session', status='done')
        self.foreign = Session('foreign', 'remote', 'stream', 'pi', 'native', 'Foreign')
        self.backend = Mock(host_id='host')
        self.backend.store.root = self.root
        self.backend.snapshot.side_effect = AssertionError('snapshot may write receipts')
        self.backend.authority.side_effect = AssertionError('authority may write receipts')
        self.backend.save.side_effect = AssertionError('save')
        self.backend.sync.side_effect = AssertionError('sync')
        self.backend.read.side_effect = AssertionError('projected read')
        self.backend.context_records.side_effect = AssertionError('caller-dependent context')
        self.catalog(self.session, self.foreign)
        self.runtime = Runtime(self.root / 'runtime', 'host', FrontendMode.NONE)
        self.client = Mock()
        self.enterContext(patch.object(self.runtime, '_transport', return_value=(self.client, 'scope')))
        self.enterContext(patch.object(self.runtime, '_write', side_effect=AssertionError('runtime write')))
        self.bindings = self.enterContext(patch.object(self.runtime, 'association_inventory'))
        self.bindings.return_value = [{'ready': True, 'workspace_id': 'w1', 'stream_id': 'stream'}]
        self.topology = {key: [] for key in ('workspaces', 'tabs', 'panes', 'layouts', 'agents')}
        self.workspace('w1')
        self.client.command_result.side_effect = lambda *args: {'snapshot': self.topology}

    def catalog(self, *sessions):
        self.backend.inspect.return_value = (
            Snapshot(Catalog(streams=(self.stream, self.other), sessions=sessions), {}),
            {s.id: Authority('host', 'secret-live-token') for s in (self.stream, self.other)},
        )

    def workspace(self, identity):
        self.topology['workspaces'].append({'workspace_id': identity, 'label': identity})

    def live(self, native, workspace='w1'):
        index = str(len(self.topology['agents']))
        pane = {'workspace_id': workspace, 'tab_id': 't' + index, 'pane_id': 'p' + index,
                'terminal_id': 'term' + index}
        self.topology['tabs'].append({'workspace_id': workspace, 'tab_id': pane['tab_id'], 'label': 'Tab'})
        self.topology['panes'].append(pane)
        self.topology['layouts'].append({'workspace_id': workspace, 'tab_id': pane['tab_id'],
                                         'panes': [{'pane_id': pane['pane_id']}]})
        self.topology['agents'].append({**pane, 'agent': 'pi',
                                       'agent_session': {'agent': 'pi', 'kind': 'id', 'value': native}})

    def saved(self, native, filename=None):
        path = self.root / 'pi' / 'sessions' / (filename or (native + '.jsonl'))
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({'type': 'session', 'id': native}) + '\n')
        return str(path.resolve())

    def capture(self, **kwargs):
        return capture_inputs(self.backend, self.runtime, **kwargs)

    def test_listing_is_exact_and_has_no_samples(self):
        result = self.capture()
        self.assertEqual(result['version'], 1)
        self.assertEqual(result['workspaces'], [{'id': 'w1', 'label': 'w1', 'stream_id': 'stream'}])
        self.assertNotIn('samples', result)
        self.assertIsNone(result['topology_error'])
        with self.assertRaises(StreamError):
            self.capture(stream_id='First')

    def test_history_foreign_host_redaction_missing_file_and_no_writer_calls(self):
        result = self.capture(stream_id='stream')
        self.assertEqual(len(result['samples']), 2)
        local, foreign = sorted(result['samples'], key=lambda s: s['host_id'])
        self.assertFalse(local['open'])
        self.assertIn('unavailable or ambiguous', local['unavailable'])
        self.assertEqual(foreign['unavailable'], 'foreign host')
        self.assertIsNone(foreign['transcript_file'])
        self.assertEqual(local['catalog']['authority'], {'host_id': 'host', 'token': '[redacted]'})
        self.assertNotIn('secret-live-token', json.dumps(result))
        self.backend.inspect.assert_called_once_with()
        self.backend.snapshot.assert_not_called()
        self.backend.authority.assert_not_called()
        self.backend.save.assert_not_called()
        self.backend.sync.assert_not_called()
        self.backend.context_records.assert_not_called()
        self.runtime._write.assert_not_called()
        self.client.command_result.assert_called_once_with(['api', 'snapshot'], 'session_snapshot')

    def test_foreign_host_is_never_resolved_locally(self):
        self.catalog(self.foreign)
        with patch('_streams.catalog_capture.conversation_file', side_effect=AssertionError('local resolution')):
            self.assertEqual(self.capture(stream_id='stream')['samples'][0]['unavailable'], 'foreign host')

    def test_open_uncarded_enrollment_context_is_not_polluted_by_environment(self):
        self.live('uncarded')
        saved = self.saved('uncarded')
        sample = next(s for s in self.capture(stream_id='stream')['samples'] if s['native_session_id'] == 'uncarded')
        self.assertTrue(sample['open'])
        self.assertIsNone(sample['catalog']['session'])
        self.assertEqual(sample['catalog']['stream']['id'], 'stream')
        self.assertEqual(sample['transcript_file'], saved)
        self.assertEqual(sample['locations'][0]['workspace_id'], 'w1')

    def test_other_stream_card_keeps_actual_membership(self):
        card = Session('other-card', 'host', 'other', 'pi', 'other-native', 'Other')
        self.catalog(self.session, card)
        self.live('other-native')
        sample = next(s for s in self.capture(workspace_id='w1')['samples'] if s['native_session_id'] == 'other-native')
        self.assertEqual(sample['catalog']['stream']['id'], 'other')
        self.assertEqual(sample['catalog']['session']['id'], card.id)

    def test_history_and_live_native_id_deduplicate_but_hosts_do_not(self):
        self.live('native')
        self.saved('native')
        result = self.capture(stream_id='stream')
        self.assertEqual(len(result['samples']), 2)
        self.assertTrue(next(s for s in result['samples'] if s['host_id'] == 'host')['open'])

    def test_multiple_bound_workspaces_and_locations(self):
        self.workspace('w2')
        self.bindings.return_value.append({'ready': True, 'workspace_id': 'w2', 'stream_id': 'stream'})
        self.live('native')
        self.live('native', 'w2')
        local = next(s for s in self.capture(stream_id='stream')['samples'] if s['host_id'] == 'host')
        self.assertEqual({loc['workspace_id'] for loc in local['locations']}, {'w1', 'w2'})

    def test_unavailable_topology_retains_history_unknown_not_closed(self):
        self.client.command_result.side_effect = StreamError('Herdr unavailable')
        result = self.capture(stream_id='stream')
        self.assertIn('Herdr unavailable', result['topology_error'])
        self.assertTrue(all(s['open'] is None for s in result['samples']))
        with self.assertRaises(StreamError):
            self.capture(workspace_id='w1')

    def test_empty_topology_is_closed(self):
        self.topology['workspaces'].clear()
        sample = next(s for s in self.capture(stream_id='stream')['samples'] if s['host_id'] == 'host')
        self.assertFalse(sample['open'])

    def test_malformed_and_duplicate_identity_fail_closed(self):
        self.live('native')
        self.live('native')
        result = self.capture(stream_id='stream')
        self.assertIn('ambiguous live', result['topology_error'])
        self.assertTrue(all(s['open'] is None for s in result['samples']))
        self.topology['workspaces'].append({'workspace_id': 'w1', 'label': 'duplicate'})
        self.assertIn('ambiguous Herdr', self.capture(stream_id='stream')['topology_error'])

    def test_unrelated_workspace_is_not_verified_and_history_open_is_unknown(self):
        self.workspace('unrelated')
        self.live('native', 'unrelated')
        self.topology['agents'][0]['agent_session'] = {'agent': 'pi', 'kind': 'path',
                                                      'value': '/must-not-open.jsonl'}
        with patch('_streams.catalog_capture.native_identity', side_effect=AssertionError('unrelated identity')):
            result = self.capture(stream_id='stream')
        local = next(s for s in result['samples'] if s['host_id'] == 'host')
        self.assertIsNone(local['open'])
        self.assertIn('outside selected workspaces', result['topology_error'])

    def test_transcript_filename_is_not_identity_proof(self):
        self.saved('wrong-id', 'native.jsonl')
        local = next(s for s in self.capture(stream_id='stream')['samples'] if s['host_id'] == 'host')
        self.assertIsNone(local['transcript_file'])
        self.assertIn('0 verified files', local['unavailable'])

    def test_ambiguous_transcript_is_reported_per_sample(self):
        self.saved('native')
        self.saved('native', 'duplicate-native.jsonl')
        result = self.capture(stream_id='stream')
        local = next(s for s in result['samples'] if s['host_id'] == 'host')
        self.assertIn('ambiguous (2 verified files)', local['unavailable'])

    def test_unsupported_catalog_harness_is_explicit(self):
        self.catalog(self.session, Session('codex', 'host', 'stream', 'codex', 'codex-id', 'Codex'))
        result = self.capture(stream_id='stream')
        self.assertEqual(len(result['samples']), 1)
        self.assertEqual(result['unsupported'][0]['harness'], 'codex')

    def test_unsupported_live_harnesses_cannot_block_pi_discovery_or_open_transcripts(self):
        self.live('uncarded')
        self.saved('uncarded')
        for harness in ('codex', 'codex', 'claude', 'unknown-harness'):
            self.live('unsupported')
            agent = self.topology['agents'][-1]
            agent.update(agent=harness, agent_session={'agent': harness, 'kind': 'path',
                                                       'value': '/must-not-open.jsonl'})
        with patch('_streams.catalog_capture.native_identity', wraps=native_identity) as verified:
            result = self.capture(workspace_id='w1')
        verified.assert_called_once_with(self.topology['agents'][0]['agent_session'], 'pi')
        self.assertIsNone(result['topology_error'])
        self.assertEqual(len(result['unsupported']), 4)
        self.assertEqual({a['harness'] for a in result['unsupported']}, {'codex', 'claude', 'unknown-harness'})
        sample = next(s for s in result['samples'] if s['native_session_id'] == 'uncarded')
        self.assertTrue(sample['open'])
        self.assertEqual(len(sample['locations']), 1)

    def test_unidentified_pi_pane_cannot_hide_verified_carded_or_uncarded_conversations(self):
        from _streams.catalog_arrangement import workspace_inventory
        self.live('native')
        self.live('uncarded')
        self.live('unidentified')
        self.saved('native')
        self.saved('uncarded')
        for metadata in (None, {'agent': 'other'}, {'agent': 'pi', 'kind': 'invalid', 'value': 'unknown'}):
            with self.subTest(metadata=metadata):
                self.topology['agents'][-1]['agent_session'] = metadata
                with self.assertRaises(StreamError):
                    workspace_inventory(self.topology, 'w1')
                result = self.capture(workspace_id='w1')
                local = {s['native_session_id']: s for s in result['samples'] if s['host_id'] == 'host'}
                self.assertEqual(set(local), {'native', 'uncarded'})
                for sample in local.values():
                    self.assertTrue(sample['open'])
                    self.assertEqual(len(sample['locations']), 1)
                    self.assertIsNone(sample['unavailable'])
                self.assertIsNone(local['uncarded']['catalog']['session'])
                self.assertEqual(result['unsupported'], [])
                self.assertEqual(len(result['unresolved']), 1)
                observation = result['unresolved'][0]
                self.assertEqual(observation['pane_id'], self.topology['agents'][-1]['pane_id'])
                self.assertEqual(observation['workspace_id'], 'w1')
                self.assertEqual(observation['harness'], 'pi')
                self.assertIn('native agent_session', observation['error'])
                self.assertNotIn('native_session_id', observation)
                self.assertIn(observation['pane_id'], result['topology_error'])
                self.assertIsNone(next(s for s in result['samples'] if s['host_id'] == 'remote')['open'])

    def test_duplicate_pi_identity_only_blocks_the_conflicting_panes(self):
        self.live('native')
        self.live('native')
        self.live('uncarded')
        self.saved('uncarded')
        result = self.capture(stream_id='stream')
        self.assertEqual(len(result['unresolved']), 2)
        self.assertTrue(all('ambiguous live' in a['error'] for a in result['unresolved']))
        local = {s['native_session_id']: s for s in result['samples'] if s['host_id'] == 'host'}
        self.assertIsNone(local['native']['open'])
        self.assertEqual(local['native']['locations'], [])
        self.assertTrue(local['uncarded']['open'])
        self.assertEqual(len(local['uncarded']['locations']), 1)

    def test_unidentified_pi_pane_keeps_unmatched_history_unknown_not_closed(self):
        self.live('unknown')
        self.topology['agents'][-1]['agent_session'] = None
        result = self.capture(stream_id='stream')
        self.assertTrue(all(s['open'] is None for s in result['samples']))
        self.assertEqual(len(result['unresolved']), 1)

    def test_one_unidentified_pane_preserves_verified_conversations_across_bound_workspaces(self):
        self.catalog()
        self.workspace('w2')
        self.bindings.return_value.append({'ready': True, 'workspace_id': 'w2', 'stream_id': 'stream'})
        self.live('first')
        self.live('unknown', 'w2')
        self.topology['agents'][-1]['agent_session'] = None
        self.live('second', 'w2')
        self.saved('first')
        self.saved('second')
        result = self.capture(stream_id='stream')
        self.assertEqual({s['native_session_id'] for s in result['samples']}, {'first', 'second'})
        self.assertTrue(all(s['open'] is True for s in result['samples']))
        self.assertEqual(result['unresolved'][0]['workspace_id'], 'w2')

    def test_unbound_workspace_has_null_context(self):
        self.bindings.return_value = []
        self.live('uncarded')
        result = self.capture(workspace_id='w1')
        self.assertEqual(len(result['samples']), 1)
        self.assertIsNone(result['samples'][0]['catalog']['stream'])

    def test_parser_handler_never_falls_through_to_enroll(self):
        args = build_parser().parse_args(['agent', 'capture-inputs', '--stream-id', 'stream', '--json'])
        self.assertIs(args.func, command_capture_inputs)
        out = io.StringIO()
        with patch('_streams.cli.config.open_catalog', return_value=self.backend) as opened, \
                patch('_streams.cli.runtime', return_value=self.runtime), patch('sys.stdout', out), \
                patch('_streams.catalog_agent.enroll', side_effect=AssertionError('enroll')):
            self.assertEqual(main(['agent', 'capture-inputs', '--stream-id', 'stream', '--json']), 0)
        opened.assert_called_once_with(validate_records=False, read_only=True)
        self.backend.inspect.assert_called_once_with()
        self.assertEqual(json.loads(out.getvalue())['target']['id'], 'stream')

    def test_real_fixture_catalog_snapshot_without_saves_or_sync(self):
        root = self.root / 'catalog'
        root.mkdir()
        store = YamlStore(root)
        with store.transaction() as transaction:
            transaction.save(self.stream, expected=transaction.read())
            transaction.save(self.session, expected=transaction.read())
        backend = LocalCatalog(store, host_id='host')
        backend.state.root.mkdir()
        with patch.object(backend, 'save', side_effect=AssertionError('save')), \
                patch.object(backend, 'sync', side_effect=AssertionError('sync')):
            result = capture_inputs(backend, self.runtime, stream_id='stream')
        self.assertEqual(result['samples'][0]['catalog']['session']['id'], 'session')
        self.assertIsNone(result['samples'][0]['catalog']['authority'])


if __name__ == '__main__':
    unittest.main()
