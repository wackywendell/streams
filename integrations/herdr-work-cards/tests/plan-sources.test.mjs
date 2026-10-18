import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtemp, mkdir, writeFile, appendFile, rm, symlink, rename, open, realpath } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { parseWorkspaceV1 } from '../dist/records.js';
import { capturePlanSources, recheckPlanSources, MAX_PLAN_SOURCE_BYTES } from '../dist/pi/plan-sources.js';

async function fixture(t) {
  const base = await realpath(await mkdtemp(join(tmpdir(), 'wc-plan-')));
  t.after(() => rm(base, { recursive: true, force: true }));
  const root = join(base, 'repo'); await mkdir(root);
  const identity = { checkout: 'repo', path: 'plan.md' };
  const workspace = parseWorkspaceV1({ version: 1, id: 'w', creationToken: 'c', mutationToken: 'm', title: 'Workspace', titleMode: 'auto', goal: '', planSources: [], sourceFingerprint: null, checkouts: { repo: { directory: root } }, documents: [{ ...identity, description: 'Plan' }], issues: [], prs: [], exclusions: [] });
  return { base, root, path: join(root, 'plan.md'), identity, workspace };
}

test('capture accepts exact byte bound, freezes provenance and rereads bytes on recheck', async t => {
  const f = await fixture(t); await writeFile(f.path, 'a'.repeat(MAX_PLAN_SOURCE_BYTES));
  const capture = await capturePlanSources(f.workspace, [f.identity]);
  assert.equal(capture.sources[0].content.length, 256 * 1024);
  assert.equal(capture.sources[0].directory, f.root);
  assert.equal(capture.sources[0].target, f.path);
  assert.ok(Object.isFrozen(capture)); assert.ok(Object.isFrozen(capture.sources)); assert.ok(Object.isFrozen(capture.sources[0].identity));
  assert.equal((await recheckPlanSources(f.workspace, [f.identity], capture)).fingerprint, capture.fingerprint);
  await writeFile(f.path, 'b'.repeat(MAX_PLAN_SOURCE_BYTES));
  await assert.rejects(recheckPlanSources(f.workspace, [f.identity], capture), /changed/i);
});

test('selected sources fail visibly on oversize, invalid UTF8, missing and nonregular input', async t => {
  const f = await fixture(t);
  await assert.rejects(capturePlanSources(f.workspace, [f.identity]), /plan.md/);
  await writeFile(f.path, Buffer.alloc(MAX_PLAN_SOURCE_BYTES + 1));
  await assert.rejects(capturePlanSources(f.workspace, [f.identity]), /256|large|limit/i);
  await writeFile(f.path, Buffer.from([0xc3, 0x28]));
  await assert.rejects(capturePlanSources(f.workspace, [f.identity]), /UTF|encoded|encoding/i);
  await rm(f.path); await mkdir(f.path);
  await assert.rejects(capturePlanSources(f.workspace, [f.identity]), /regular/i);
});

test('only selected currently visible Markdown documents are accepted without scanning', async t => {
  const f = await fixture(t); await writeFile(f.path, 'Plan');
  assert.equal((await capturePlanSources(f.workspace, [])).sources.length, 0);
  for (const selected of [[{ ...f.identity, path: 'unknown.md' }], [{ ...f.identity, checkout: 'other' }], [f.identity, f.identity], [{ ...f.identity, path: '../escape.md' }]]) await assert.rejects(capturePlanSources(f.workspace, selected));
  const nonMarkdown = { ...f.identity, path: 'source.ts' };
  await assert.rejects(capturePlanSources({ ...f.workspace, documents: [{ ...nonMarkdown, description: '' }] }, [nonMarkdown]), /Markdown/i);
});

test('bounded read detects growth after size check without truncating', async t => {
  const f = await fixture(t); await writeFile(f.path, 'initial');
  const handle = await open(f.path); const prototype = Object.getPrototypeOf(handle); await handle.close();
  const original = prototype.read; let grew = false;
  t.mock.method(prototype, 'read', async function (...args) {
    if (!grew) { grew = true; await appendFile(f.path, Buffer.alloc(MAX_PLAN_SOURCE_BYTES)); }
    return original.apply(this, args);
  });
  await assert.rejects(capturePlanSources(f.workspace, [f.identity]), /large|limit|256/i);
});

test('resolver rejects escaped and relocated sources but permits contained canonical targets', async t => {
  const f = await fixture(t); const outside = join(f.base, 'outside.md'); await writeFile(outside, 'Outside');
  await symlink(outside, f.path);
  await assert.rejects(capturePlanSources(f.workspace, [f.identity]), /outside/i);
  await rm(f.path); const internal = join(f.root, 'internal.md'); await writeFile(internal, 'Inside'); await symlink(internal, f.path);
  assert.equal((await capturePlanSources(f.workspace, [f.identity])).sources[0].target, internal);
  await rename(f.root, join(f.base, 'moved')); await symlink(join(f.base, 'moved'), f.root);
  await assert.rejects(capturePlanSources(f.workspace, [f.identity]), /canonical/i);
});

test('recheck rejects source removal, replacement, selection changes and registered-root changes', async t => {
  const f = await fixture(t); await writeFile(f.path, 'Plan');
  const capture = await capturePlanSources(f.workspace, [f.identity]);
  await assert.rejects(recheckPlanSources({ ...f.workspace, documents: [] }, [f.identity], capture));
  await assert.rejects(recheckPlanSources(f.workspace, [], capture), /changed/i);
  const newRoot = join(f.base, 'new'); await mkdir(newRoot); await writeFile(join(newRoot, 'plan.md'), 'Plan');
  await assert.rejects(recheckPlanSources({ ...f.workspace, checkouts: { repo: { directory: newRoot } } }, [f.identity], capture), /changed/i);
  await writeFile(join(f.root, 'replacement.md'), 'Different'); await rename(join(f.root, 'replacement.md'), f.path);
  await assert.rejects(recheckPlanSources(f.workspace, [f.identity], capture), /changed/i);
});

test('capture rejects replacement during a read before exposing contents', async t => {
  const f = await fixture(t); await writeFile(f.path, 'Plan');
  const handle = await open(f.path); const prototype = Object.getPrototypeOf(handle); await handle.close();
  const original = prototype.read; let replaced = false;
  t.mock.method(prototype, 'read', async function (...args) {
    const result = await original.apply(this, args);
    if (!replaced) { replaced = true; await writeFile(join(f.root, 'replacement'), 'Plan'); await rename(join(f.root, 'replacement'), f.path); }
    return result;
  });
  await assert.rejects(capturePlanSources(f.workspace, [f.identity]), /changed/i);
});

test('recheck rejects canonical target and root changes during inference, even with identical bytes', async t => {
  const f = await fixture(t); const first = join(f.root, 'first.md'); const second = join(f.root, 'second.md');
  await writeFile(first, 'Same plan'); await writeFile(second, 'Same plan'); await symlink(first, f.path);
  const capture = await capturePlanSources(f.workspace, [f.identity]);
  await rm(f.path); await symlink(second, f.path);
  await assert.rejects(recheckPlanSources(f.workspace, [f.identity], capture), /changed/i);
  await rename(f.root, join(f.base, 'moved')); await symlink(join(f.base, 'moved'), f.root);
  await assert.rejects(recheckPlanSources(f.workspace, [f.identity], capture), /canonical/i);
});

test('capture revalidates canonical root after reading before exposing contents', async t => {
  const f = await fixture(t); await writeFile(f.path, 'Plan');
  const handle = await open(f.path); const prototype = Object.getPrototypeOf(handle); await handle.close();
  const original = prototype.read; let moved = false;
  t.mock.method(prototype, 'read', async function (...args) {
    const result = await original.apply(this, args);
    if (!moved) { moved = true; await rename(f.root, join(f.base, 'moved')); await symlink(join(f.base, 'moved'), f.root); }
    return result;
  });
  await assert.rejects(capturePlanSources(f.workspace, [f.identity]), /canonical/i);
});
