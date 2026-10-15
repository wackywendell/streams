import test from 'node:test';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { cp, mkdir, mkdtemp, readFile, readdir, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { promisify } from 'node:util';

const root = fileURLToPath(new URL('../', import.meta.url));
const exec = promisify(execFile);
// The public SDK does not export clearExtensionCache; use the real installed loader.
const loaderUrl = new URL('./core/extensions/loader.js', import.meta.resolve('@earendil-works/pi-coding-agent'));
const { discoverAndLoadExtensions, loadExtensions, clearExtensionCache } = await import(loaderUrl);

async function fixture(t) {
  const directory = await realpath(await mkdtemp(join(tmpdir(), 'work-cards-package-')));
  t.after(() => rm(directory, { recursive: true, force: true }));
  return directory;
}

async function loadPackage(directory) {
  clearExtensionCache();
  const result = await discoverAndLoadExtensions([directory], directory, join(directory, 'empty-agent'));
  assert.deepEqual(result.errors, []);
  assert.equal(result.extensions.length, 1);
  const extension = result.extensions[0];
  assert.ok(extension.commands.has('work-card'));
  assert.ok(extension.tools.has('work_card'));
  return extension;
}

test('declared Pi package entry refreshes its real private Herdr helper graph on reload', async t => {
  const directory = await fixture(t);
  for (const name of ['src', 'dist', 'package.json']) {
    await cp(join(root, name), join(directory, name), { recursive: true });
  }
  // Supplied read-only host dependencies, not a clean dependency installation.
  await symlink(join(root, 'node_modules'), join(directory, 'node_modules'), 'dir');
  const originals = new Map();
  for (const [tree, extension] of [['src', 'ts'], ['dist', 'js']]) {
    const entry = join(directory, tree, 'pi', `index.${extension}`);
    const source = await readFile(entry, 'utf8');
    assert.ok(source.includes('registerWorkCards(pi);'));
    await writeFile(entry, "import { HerdrClient, loaderVersion } from '../herdr/client.js';\n" + source.replace('registerWorkCards(pi);', `registerWorkCards(pi);
      pi.registerCommand('loader-probe', { handler: async () => ({
        version: loaderVersion, pluginRoot: new HerdrClient({ env: {} }).pluginRoot,
      }) });`));
    const helper = join(directory, tree, 'herdr', `client.${extension}`);
    originals.set(helper, await readFile(helper, 'utf8'));
  }
  const writeVersion = async value => {
    for (const [helper, source] of originals) {
      await writeFile(helper, `${source}\nexport const loaderVersion = '${value}';\n`);
    }
  };
  await writeVersion('v1');
  const first = await (await loadPackage(directory)).commands.get('loader-probe').handler();
  assert.equal(first.version, 'v1');
  assert.equal(resolve(first.pluginRoot), directory);
  await writeVersion('v2');
  const reloaded = await (await loadPackage(directory)).commands.get('loader-probe').handler();
  assert.equal(reloaded.version, 'v2');
  assert.equal(resolve(reloaded.pluginRoot), directory);
  // Compiled Herdr consumers still calculate the same package root.
  const compiled = await loadExtensions([join(directory, 'dist/pi/index.js')], directory);
  assert.deepEqual(compiled.errors, []);
  const probe = await compiled.extensions[0].commands.get('loader-probe').handler();
  assert.equal(resolve(probe.pluginRoot), directory);
  t.diagnostic(`declared package: first=${first.version}, same-process reload=${reloaded.version}; source/compiled pluginRoot=${directory}`);
});

test('actual npm tarball includes the source graph and compiled Herdr, and loads in real Pi', async t => {
  const directory = await fixture(t);
  const { stdout } = await exec('npm', ['pack', '--ignore-scripts', '--json', '--pack-destination', directory, '--cache', join(directory, 'npm-cache')], { cwd: root });
  const [packed] = Object.values(JSON.parse(stdout));
  const files = new Set(packed.files.map(file => file.path));
  const sourceFiles = (await readdir(join(root, 'src'), { recursive: true })).filter(path => path.endsWith('.ts'));
  for (const path of sourceFiles) assert.ok(files.has(`src/${path}`), `Missing packed source: ${path}`);
  for (const path of ['package.json', 'dist/pi/index.js', 'dist/herdr/client.js', 'dist/herdr/pane.js', 'dist/card.js', 'dist/config.js', 'dist/state.js', 'src/state.ts', 'bin/open-document.sh', 'herdr-plugin.toml']) {
    assert.ok(files.has(path), `Missing packed runtime: ${path}`);
  }
  assert.ok([...files].every(path => !path.startsWith('tests/') && !path.startsWith('node_modules/')));
  const extracted = join(directory, 'extracted');
  await mkdir(extracted);
  await exec('tar', ['-xzf', join(directory, packed.filename), '-C', extracted]);
  const packageRoot = join(extracted, 'package');
  await symlink(join(root, 'node_modules'), join(packageRoot, 'node_modules'), 'dir');
  const manifest = JSON.parse(await readFile(join(packageRoot, 'package.json'), 'utf8'));
  const extension = await loadPackage(packageRoot);
  assert.equal(extension.resolvedPath, join(packageRoot, manifest.pi.extensions[0]));
  assert.deepEqual(manifest.exports, { './card': './dist/card.js', './config': './dist/config.js' });
  const { parseStateSections } = await import(pathToFileURL(join(packageRoot, 'dist/state.js')));
  assert.deepEqual(parseStateSections('Goal: Purpose | Now: Ready | Remaining: Scope'), { goal: 'Purpose', now: 'Ready', remaining: 'Scope' });
  const { HerdrClient } = await import(pathToFileURL(join(packageRoot, 'dist/herdr/client.js')));
  assert.equal(resolve(new HerdrClient({ env: {} }).pluginRoot), packageRoot);
  t.diagnostic(`packed ${files.size} files (${sourceFiles.length} TypeScript sources); extracted ${manifest.pi.extensions[0]} registered work-card/work_card with supplied host dependencies`);
});
