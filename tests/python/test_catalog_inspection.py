from __future__ import annotations

from dataclasses import replace
import fcntl
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'src'))

from _streams import catalog_config as config
from _streams.catalog.model import Stream
from _streams.catalog.operations import Authority, Operation
from _streams.catalog.storage import Snapshot, StorageError
from _streams.catalog.sync import GitCatalog, LocalCatalog, RepairRequired
from _streams.catalog.yaml_store import YamlStore
from _streams.catalog_capture import capture_inputs
from _streams.cli import main
from _streams.errors import StreamError


class CatalogInspectionTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.home = Path(temporary.name)
        gitconfig = self.home / 'gitconfig'
        gitconfig.write_text('[user]\n name = Inspection Test\n email = inspection@example.invalid\n'
                             '[commit]\n gpgsign = false\n')
        self.enterContext(patch.dict(os.environ, {
            'HOME': str(self.home), 'PATH': os.environ.get('PATH', os.defpath),
            'XDG_CONFIG_HOME': str(self.home / 'config'),
            'XDG_DATA_HOME': str(self.home / 'data'),
            'XDG_STATE_HOME': str(self.home / 'state'),
            'GIT_CONFIG_GLOBAL': str(gitconfig), 'GIT_CONFIG_NOSYSTEM': '1',
            'GIT_OPTIONAL_LOCKS': '1',
        }, clear=True))
        self.runtime = Mock()
        self.runtime._transport.side_effect = StreamError('synthetic unavailable topology')

    def initialize(self, strategy):
        backend = config.initialize(strategy)
        stream = Stream('stream', 'Synthetic stream', 'Synthetic goal', backend.host_id)
        operation = Operation('create', None, stream, 'user')
        self.assertTrue(backend.save(operation).accepted)
        return backend, stream

    @staticmethod
    def files(root):
        return {str(path.relative_to(root)): path.read_bytes()
                for path in root.rglob('*') if path.is_file()}

    def test_git_inventory_preserves_receipts_index_head_and_records(self):
        backend, stream = self.initialize('git')
        expected = backend.state.fingerprints()
        backend.state.write('receipts.json', {'operations': {}})
        receipts = backend.state.root / 'receipts.json'
        empty_receipts = receipts.read_bytes()
        # A changed stat cache would normally let Git status refresh the index.
        record = backend.root / backend.store.record_path(Operation('create', None, stream, 'user').target)
        stat = record.stat()
        os.utime(record, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000_000))
        before = self.files(backend.root)
        index_stat = (backend.root / '.git/index').stat()
        head = backend.git.head()
        with patch.object(GitCatalog, 'snapshot', side_effect=AssertionError('snapshot writes receipts')), \
                patch.object(GitCatalog, 'authority', side_effect=AssertionError('authority writes receipts')), \
                patch.object(GitCatalog, 'save', side_effect=AssertionError('save')), \
                patch.object(GitCatalog, 'sync', side_effect=AssertionError('sync')), \
                patch('_streams.cli.runtime', return_value=self.runtime), patch('sys.stdout', io.StringIO()) as output:
            with patch.object(GitCatalog, 'inspect', autospec=True, side_effect=GitCatalog.inspect) as inspected:
                self.assertEqual(main(['agent', 'capture-inputs', '--json']), 0)
            inspected.assert_called_once()
            self.assertEqual(json.loads(output.getvalue())['streams'][0]['id'], stream.id)
            selected = capture_inputs(backend, self.runtime, stream_id=stream.id)
        self.assertEqual(selected['target']['id'], stream.id)
        self.assertEqual(receipts.read_bytes(), empty_receipts)
        self.assertEqual(self.files(backend.root), before)
        self.assertEqual((backend.root / '.git/index').stat().st_mtime_ns, index_stat.st_mtime_ns)
        self.assertEqual(backend.git.head(), head)
        self.assertEqual(backend.snapshot().catalog.stream(stream.id), stream)
        self.assertEqual(backend.state.fingerprints(), expected)
        self.assertNotEqual(receipts.read_bytes(), empty_receipts)

    def test_inspection_returns_snapshot_and_authorities_under_one_read_only_lock(self):
        for strategy in ('none', 'git'):
            with self.subTest(strategy=strategy):
                config.set_config('catalog.root', str(self.home / strategy))
                backend, stream = self.initialize(strategy)
                before = self.files(backend.store.root)
                with patch.object(backend.store, 'transaction', wraps=backend.store.transaction) as transaction:
                    snapshot, authorities = backend.inspect()
                transaction.assert_called_once_with(read_only=True)
                self.assertEqual(snapshot.catalog.stream(stream.id), stream)
                self.assertEqual(authorities, {stream.id: Authority(backend.host_id, 'create')})
                self.assertEqual(self.files(backend.store.root), before)

    def test_fingerprint_mismatch_is_still_rejected_without_repair(self):
        backend, _ = self.initialize('git')
        backend.state.write('receipts.json', {'operations': {'create': '0' * 64}})
        before = self.files(backend.root)
        with self.assertRaisesRegex(RepairRequired, 'different intent'):
            backend.inspect()
        with self.assertRaisesRegex(StreamError, 'different intent'):
            config.open_catalog(read_only=True)
        self.assertEqual(self.files(backend.root), before)

    def test_inspection_checks_working_records_against_history(self):
        backend, stream = self.initialize('git')
        snapshot, _ = backend.inspect()
        changed = Snapshot(snapshot.catalog.replacing(replace(stream, title='Different')), snapshot.revisions)
        from _streams.catalog.yaml_store import _YamlTransaction
        with patch.object(_YamlTransaction, 'read', return_value=changed):
            with self.assertRaisesRegex(RepairRequired, 'working records disagree'):
                backend.inspect()

    def test_unsupported_history_is_still_rejected(self):
        backend, _ = self.initialize('git')
        parent = backend.git.head()
        commit = backend.git.commit(backend.git.text('write-tree'), 'unsupported synthetic commit\n', parent)
        backend.git.run('update-ref', 'refs/heads/main', commit, parent)
        before = self.files(backend.root)
        with self.assertRaisesRegex(RepairRequired, 'unsupported catalog history'):
            backend.inspect()
        self.assertEqual(self.files(backend.root), before)

    def test_read_only_transaction_cannot_create_root_or_lock(self):
        root = self.home / 'missing' / 'catalog'
        store = YamlStore(root)
        with self.assertRaisesRegex(StorageError, 'cannot open catalog lock'):
            with store.transaction(read_only=True):
                self.fail('missing root accepted')
        self.assertFalse(root.parent.exists())
        root.mkdir(parents=True)
        with self.assertRaisesRegex(StorageError, 'cannot open catalog lock'):
            with store.transaction(read_only=True):
                self.fail('missing lock accepted')
        self.assertEqual(list(root.iterdir()), [])

    def test_read_only_transaction_uses_shared_existing_lock_and_refuses_save(self):
        backend, stream = self.initialize('none')
        before = self.files(backend.store.root)
        with patch('os.open', wraps=os.open) as opened, patch('fcntl.flock', wraps=fcntl.flock) as flock:
            with backend.store.transaction(read_only=True) as transaction:
                snapshot = transaction.read()
                with self.assertRaisesRegex(StorageError, 'read-only'):
                    transaction.save(replace(stream, title='Must not save'), expected=snapshot)
        opened.assert_called_once_with(backend.store.root / '.catalog.lock', os.O_RDONLY | os.O_NOFOLLOW, 0o600)
        self.assertEqual(flock.call_args.args[1], fcntl.LOCK_SH)
        self.assertEqual(self.files(backend.store.root), before)

    def test_read_only_config_and_inspection_require_existing_state(self):
        for strategy in ('none', 'git'):
            for missing in ('root', 'state', 'lock'):
                with self.subTest(strategy=strategy, missing=missing):
                    root = self.home / f'{strategy}-{missing}'
                    config.set_config('catalog.root', str(root))
                    if missing != 'root':
                        root.mkdir()
                    if missing == 'state':
                        (root / '.catalog.lock').touch()
                    elif missing == 'lock':
                        state = root / '.catalog-sync'
                        state.mkdir()
                        identity = config.host_id(create=True)
                        (state / 'cli.json').write_text(json.dumps({
                            'version': 1, 'strategy': strategy, 'host': identity,
                        }))
                    before = self.files(self.home)
                    with self.assertRaisesRegex(StreamError, 'initialized|catalog lock'):
                        config.open_catalog(read_only=True)
                    backend = (LocalCatalog(YamlStore(root), host_id='host') if strategy == 'none'
                               else GitCatalog(root, host_id='host'))
                    with self.assertRaisesRegex(StorageError, 'initialized|catalog lock'):
                        backend.inspect()
                    self.assertEqual(self.files(self.home), before)
                    if missing == 'root':
                        self.assertFalse(root.exists())
                    elif missing == 'state':
                        self.assertFalse((root / '.catalog-sync').exists())
                    else:
                        self.assertFalse((root / '.catalog.lock').exists())


if __name__ == '__main__':
    unittest.main()
