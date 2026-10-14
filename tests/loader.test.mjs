import test from 'node:test';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';

// Pi 1.2.0's public SDK does not export these loader/cache functions.
const loaderUrl = new URL('./core/extensions/loader.js', import.meta.resolve('@earendil-works/pi-coding-agent')).href;
const { loadExtensions, clearExtensionCache } = await import(loaderUrl);
const exec = promisify(execFile);

async function fixture(t) {
  const directory = await mkdtemp(join(tmpdir(), 'work-cards-loader-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  await writeFile(join(directory, 'package.json'), JSON.stringify({ type: 'module' }));
  return directory;
}

async function load(entry, directory) {
  clearExtensionCache();
  const result = await loadExtensions([entry], directory);
  assert.deepEqual(result.errors, []);
  assert.equal(result.extensions.length, 1);
  return result.extensions[0];
}

async function version(entry, directory) {
  const extension = await load(entry, directory);
  return extension.commands.get('loader-probe').handler();
}

for (const extension of ['js', 'ts']) {
  test(`real Pi loader ${extension === 'js' ? 'retains native ESM helpers until cold start' : 'refreshes TypeScript helpers with .js import specifiers'}`, async t => {
    const directory = await fixture(t);
    const entry = join(directory, `index.${extension}`);
    const helper = join(directory, `helper.${extension}`);
    await writeFile(entry, "import { value } from './helper.js';\nexport default function (pi) { pi.registerCommand('loader-probe', { handler: async () => value }); }\n");
    const source = value => `export const value${extension === 'ts' ? ': string' : ''} = '${value}';\n`;
    await writeFile(helper, source('v1'));
    const first = await version(entry, directory);
    await writeFile(helper, source('v2'));
    const reloaded = await version(entry, directory);
    const { stdout } = await exec(process.execPath, ['--input-type=module', '-e', `
      const { loadExtensions, clearExtensionCache } = await import(${JSON.stringify(loaderUrl)});
      clearExtensionCache();
      const result = await loadExtensions([${JSON.stringify(entry)}], ${JSON.stringify(directory)});
      if (result.errors.length) throw new Error(JSON.stringify(result.errors));
      console.log(await result.extensions[0].commands.get('loader-probe').handler());
    `], { env: { ...process.env, PI_CODING_AGENT_DIR: directory } });
    assert.equal(first, 'v1');
    assert.equal(reloaded, extension === 'js' ? 'v1' : 'v2');
    assert.equal(stdout.trim(), 'v2');
    t.diagnostic(`${extension}: first=${first}, same-process reload=${reloaded}, cold-process=${stdout.trim()}`);
  });
}
