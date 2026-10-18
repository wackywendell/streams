import test from 'node:test';
import assert from 'node:assert/strict';
import { realpath } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { WorkspaceClient } from '../dist/herdr/workspace-client.js';
import { DEFAULT_CONFIG } from '../dist/config.js';

const env = { HERDR_ENV: '1', HERDR_PANE_ID: 'pane', HERDR_SOCKET_PATH: '/fake.sock' };
const pane = { pane_id: 'pane', terminal_id: 'terminal', workspace_id: 'workspace', tab_id: 'tab', tokens: {} };

test('outside Herdr refuses before invoking any command', async () => {
  const client = new WorkspaceClient({ env: {}, run: async () => { assert.fail('No command permitted'); } });
  await assert.rejects(client.bootstrap(DEFAULT_CONFIG), /Herdr-managed/);
});

test('transport rejects malformed responses, semantic errors and invalid identifiers', async () => {
  for (const response of ['not JSON', '[]', JSON.stringify({ error: 'refused' }), JSON.stringify({ result: { type: 'wrong', pane } }),
    JSON.stringify({ result: { type: 'pane_current', pane: { ...pane, terminal_id: '\n' } } })]) {
    const client = new WorkspaceClient({ env, run: async () => response });
    await assert.rejects(client.bootstrap(DEFAULT_CONFIG));
  }
});

test('transport requires a compatible exact endpoint and never restarts the server', async () => {
  for (const server of [
    { running: false }, { compatible: false }, { endpoint_compatible: false }, { version: '0.9.2' }, { version: 'invalid' }, { socket: '/other.sock' },
  ]) {
    const calls = [];
    const client = new WorkspaceClient({ env, run: async args => {
      calls.push(args);
      if (args[0] === 'pane') return JSON.stringify({ result: { type: 'pane_current', pane } });
      return JSON.stringify({ server: { running: true, compatible: true, endpoint_compatible: true, version: '0.9.3', socket: '/fake.sock', ...server } });
    } });
    await assert.rejects(client.bootstrap(DEFAULT_CONFIG));
    assert.deepEqual(calls, [['pane', 'current', '--current'], ['status', '--json']]);
  }
});

test('conflicting or stale plugin registrations refuse without relinking or replacement', async () => {
  const root = await realpath(fileURLToPath(new URL('../', import.meta.url)));
  const plugin = { plugin_id: 'work-cards', enabled: true, plugin_root: root, manifest_path: join(root, 'herdr-plugin.toml'), source: { kind: 'local' },
    version: '0.1.0', min_herdr_version: '0.9.3', platforms: ['macos', 'linux'], panes: [
      { id: 'card', title: 'Work card', placement: 'split', command: ['node', 'dist/herdr/pane.js'] },
      { id: 'document', title: 'Document', placement: 'tab', command: ['/bin/sh', 'bin/open-document.sh'] },
    ] };
  for (const mutation of [{ enabled: false }, { plugin_root: '/wrong' }, { panes: plugin.panes.slice(0, 1) }, { source: { kind: 'remote' } }]) {
    const calls = [];
    const client = new WorkspaceClient({ env, pluginRoot: root, run: async args => {
      calls.push(args); return JSON.stringify({ result: { type: 'plugin_list', plugins: [{ ...plugin, ...mutation }] } });
    } });
    await assert.rejects(client.ensurePlugin(DEFAULT_CONFIG), /relink|registration/);
    assert.deepEqual(calls, [['plugin', 'list', '--plugin', 'work-cards', '--json']]);
  }
});
