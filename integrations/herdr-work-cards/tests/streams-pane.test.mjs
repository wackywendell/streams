import test from 'node:test';
import assert from 'node:assert/strict';
import { promisify } from 'node:util';
import { execFile } from 'node:child_process';
import { chmod, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { streamsPaneSource, runStreamsPane } from '../dist/herdr/streams-pane.js';
const native = '12345678-1234-4234-8234-123456789abc';
const scope = { catalogSource: 'catalog', hostId: 'host', streamId: 'shared', sessionId: 'session', nativeSessionId: native };
const owner = { frontend: 'herdr', pane_id: 'w:p1', tab_id: 'w:t1', workspace_id: 'w' };
const session = { id: 'session', host_id: 'host', stream_id: 'shared', harness: 'pi', harness_session_id: native,
  title: 'Own', title_mode: 'manual', status: 'active', state: 'Own state', state_short: null, remaining: '' };
const wire = { catalog_source: 'catalog', host_id: 'host', authority: null, native: owner, session, sessions: [session],
  stream: { id: 'shared', title: 'Shared', title_mode: 'manual', goal: null, goal_short: null, repositories: {}, documents: [], issues: [], prs: [] }, checkouts: [], document_locations: [] };
const launcher = { STREAMS_CARDS_SCOPE: JSON.stringify(scope), STREAMS_CARDS_OWNER: JSON.stringify(owner),
  STREAMS_CARDS_READ_ENV: '{}', STREAMS_CARDS_OWNER_TERMINAL: 'term1', STREAMS_CARDS_NONCE: 'nonce', STREAMS_CARDS_STATE_DIR: '/native/state' };
test('pane refuses absent/malformed launcher scope, owner, nonce and native UUID before reading anything', () => {
  for (const field of Object.keys(launcher)) assert.throws(() => streamsPaneSource({ ...launcher, [field]: undefined }));
  assert.throws(() => streamsPaneSource({ ...launcher, STREAMS_CARDS_SCOPE: JSON.stringify({ ...scope, nativeSessionId: 'guess' }) }), /UUID/);
  assert.throws(() => streamsPaneSource({ ...launcher, STREAMS_CARDS_NONCE: 'bad nonce' }));
});
test('pane reads the attested exact launcher owner, not its own card pane or cwd, and fences first scope', async t => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'streams-pane-'))); t.after(() => rm(root, { recursive: true, force: true }));
  const executable = join(root, 'streams');
  await writeFile(executable, `#!${process.execPath}\nif (process.env.HERDR_PANE_ID !== 'w:p1' || process.env.HERDR_TAB_ID !== 'w:t1' || process.env.HERDR_WORKSPACE_ID !== 'w' || !process.argv.includes('--attest')) process.exit(10);\nprocess.stdout.write(process.env.RESPONSE);\n`);
  await chmod(executable, 0o755);
  const env = { ...launcher, HERDR_PANE_ID: 'w:card', HERDR_TAB_ID: 'wrong-helper-tab', PATH: root, STREAMS_CARDS_READ_ENV: JSON.stringify({ PATH: root }), RESPONSE: JSON.stringify(wire) };
  assert.deepEqual((await streamsPaneSource(env).read()).scope, scope);
  await assert.rejects(streamsPaneSource({ ...env, RESPONSE: JSON.stringify({ ...wire, catalog_source: 'changed-before-first-read' }) }).read(), /scope changed/);
  await assert.rejects(streamsPaneSource({ ...env, RESPONSE: JSON.stringify({ ...wire, native: { ...owner, pane_id: 'helper' } }) }).read(), /exact requested owner/);
});
for (const captured of [undefined, '{', 'null', '[]', '42', '{"HOME":null}', '{"PATH":3}', '{"SECRET":"bad"}', '{"HOME":"bad\\u0000path"}']) test(`invalid read environment fails before CLI: ${captured}`, () => {
  assert.throws(() => streamsPaneSource({ ...launcher, STREAMS_CARDS_READ_ENV: captured, PATH: '/must-not-run' }), /read environment/);
});
test('pane CLI uses owner selector snapshot and removes stale server selectors', async t => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'streams-pane-env-'))); t.after(() => rm(root, { recursive: true, force: true }));
  const executable = join(root, 'streams');
  const captured = { HOME: '/owner/home', XDG_CONFIG_HOME: '/owner/config', XDG_DATA_HOME: '', PI_CODING_AGENT_DIR: '/owner/pi', HERDR_BIN_PATH: '/owner/herdr', PATH: root };
  await writeFile(executable, `#!${process.execPath}\nimport assert from 'node:assert/strict';\nconst expected = ${JSON.stringify(captured)}; for (const [key, value] of Object.entries(expected)) assert.equal(process.env[key], value);\nassert.equal(process.env.XDG_STATE_HOME, undefined);\nassert.equal(process.env.HERDR_SOCKET_PATH, '/pinned/socket');\nassert.equal(process.env.STREAMS_CARDS_NONCE, 'nonce');\nassert.equal(process.env.HERDR_PANE_ID, 'w:p1');\nprocess.stdout.write(process.env.RESPONSE);\n`);
  await chmod(executable, 0o755);
  const env = { ...launcher, HOME: '/server/home', XDG_CONFIG_HOME: '/server/config', XDG_STATE_HOME: '/server/state', PI_CODING_AGENT_DIR: '/server/pi', PATH: '/server/bin', HERDR_BIN_PATH: '/server/herdr', HERDR_SOCKET_PATH: '/pinned/socket',
    STREAMS_CARDS_READ_ENV: JSON.stringify(captured), RESPONSE: JSON.stringify(wire) };
  assert.deepEqual((await streamsPaneSource(env).read()).scope, scope);
});
for (const [mode, key] of [['unready', 'q'], ['unready', '\x03'], ['mismatch', 'q'], ['close-failure', 'q'], ['self-close', 'q'], ['caption-unchanged', 'q'], ['caption-changed', 'q'], ['caption-manual', 'q']]) test(`pane exit ${mode}/${JSON.stringify(key)} retains proof gates and releases locks before signal cleanup`, async t => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'streams-pane-loop-'))); t.after(() => rm(root, { recursive: true, force: true }));
  const module = name => new URL(`../dist/herdr/${name}.js`, import.meta.url).href;
  const script = `
    import assert from 'node:assert/strict';
    import { EventEmitter } from 'node:events';
    import { readdir } from 'node:fs/promises';
    import { runStreamsPane } from ${JSON.stringify(module('streams-pane'))};
    import { StreamsCardSource } from ${JSON.stringify(module('streams-source'))};
    import { StreamsWorkspaceClient } from ${JSON.stringify(module('streams-workspace-client'))};
    import { withWorkspaceLock } from ${JSON.stringify(module('workspace-lock'))};
    const input = new EventEmitter(), output = new EventEmitter(); let raw = false, closed = 0, projected = 0, restored = false, errors = '';
    Object.assign(input, { isTTY: true, isRaw: false, setRawMode(v) { raw = v; }, resume() {}, pause() {} });
    Object.assign(output, { isTTY: true, columns: 80, rows: 24, write(v) { if (v.includes('\\x1b[?1049l')) restored = true; } });
    Object.defineProperty(process, 'stdin', { value: input }); Object.defineProperty(process, 'stdout', { value: output });
    process.stderr.write = v => { errors += v; };
    const context = { socket: ${JSON.stringify(join(root, 'herdr.sock'))}, paneId: 'w:p2', terminalId: 'term2', workspaceId: 'w', tabId: 'w:t1' };
    StreamsCardSource.prototype.read = async () => ({ scope: ${JSON.stringify(scope)}, stream: ${JSON.stringify(wire.stream)}, session: ${JSON.stringify(session)}, native: ${JSON.stringify(owner)}, checkouts: [], documentLocations: [] });
    StreamsWorkspaceClient.prototype.bootstrap = async () => context;
    StreamsWorkspaceClient.prototype.verifyDisplay = async () => {
      input.emit('data', Buffer.from(${JSON.stringify(key)}));
      if (${JSON.stringify(mode)} === 'unready') throw Error('not ready');
      return { agent: { terminal_id: ${JSON.stringify(mode === 'mismatch' ? 'wrong' : 'term1')} }, card: { terminal_id: 'term2', label: ${JSON.stringify(mode === 'caption-changed' ? 'Old' : 'Shared')}, tokens: { strm_caption: ${JSON.stringify(mode === 'caption-changed' ? 'Old' : 'Shared')}, strm_caption_manual: ${JSON.stringify(mode === 'caption-manual' ? '1' : undefined)} } }, nonce: 'nonce' };
    };
    StreamsWorkspaceClient.prototype.projectCaption = async () => { projected++; };
    StreamsWorkspaceClient.prototype.closeDisplay = async () => {
      closed++; assert.equal(raw, false); assert.equal(restored, true);
      if (${JSON.stringify(mode)} === 'close-failure') throw Error('final close proof failed');
      await withWorkspaceLock(context.socket, context.workspaceId, async () => {
        assert.ok((await readdir(${JSON.stringify(root)})).some(n => n.endsWith('.lock')));
        process.kill(process.pid, 'SIGHUP'); process.kill(process.pid, 'SIGTERM');
        await new Promise(resolve => setTimeout(resolve, 30));
        assert.ok(process.listenerCount('SIGHUP') > 0); assert.ok(process.listenerCount('SIGTERM') > 0);
      });
    };
    await runStreamsPane({ ...${JSON.stringify(launcher)}, STREAMS_CARDS_CONFIG: ${JSON.stringify(join(root, 'missing-config.json'))} });
    assert.equal(closed, ${mode === 'unready' || mode === 'mismatch' ? 0 : 1});
    assert.equal(projected, ${mode === 'caption-changed' ? 1 : 0});
    assert.equal(raw, false); assert.equal(restored, true);
    // Crash-released locks retain their inode after the protected callback exits.
    assert.equal((await readdir(${JSON.stringify(root)})).filter(n => n.endsWith('.lock')).length, ${['unready', 'mismatch', 'close-failure'].includes(mode) ? 0 : 1});
    assert.equal(process.listenerCount('SIGHUP'), 0); assert.equal(process.listenerCount('SIGTERM'), 0);
    if (${JSON.stringify(mode === 'unready' || mode === 'mismatch' || mode === 'close-failure')}) assert.match(errors, /retained.*inspect/);
    else assert.equal(errors, '');
  `;
  await promisify(execFile)(process.execPath, ['--input-type=module', '-e', script], { timeout: 5000 });
});
test('noninteractive pane rejects without acquiring native/catalog resources', async () => {
  if (!process.stdin.isTTY || !process.stdout.isTTY) await assert.rejects(runStreamsPane({}), /interactive Herdr terminal/);
});
