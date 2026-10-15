import assert from 'node:assert/strict';
import test from 'node:test';
import { applyOwnUpdate, applySharedAdditions, validateAgent, validateWorkspace } from '../dist/records.js';
import { captureReferenceBoundary, projectSharedEvidence } from '../dist/pi/shared-evidence.js';
import {
  buildOwnSummaryContext, parseOwnSummaryDecision, requestOwnSummary,
  buildSharedAdditionsContext, parseSharedAdditionsDecision, requestSharedAdditions,
  buildPlanSelectionContext, parsePlanSelectionDecision, requestPlanSelection,
  buildWorkspaceGoalContext, parseWorkspaceGoalDecision, requestWorkspaceGoal,
} from '../dist/pi/workspace-summary.js';

const doc = { checkout: 'repo', path: 'plan.md', description: 'Implementation plan' };
const workspace = validateWorkspace({ version: 1, id: 'w', creationToken: 'c', mutationToken: 'm', title: 'Workspace', titleMode: 'manual', goal: '', planSources: [], sourceFingerprint: null, checkouts: { repo: { directory: '/repo' } }, documents: [doc, { ...doc, path: 'code.ts' }], issues: [], prs: [], exclusions: [{ url: 'https://example.com/removed' }] });
const agent = validateAgent({ version: 1, workspaceId: 'w', sessionId: 's', membershipToken: 'm', boundary: { compactionDigest: 'd', transcriptLength: 0, prefixDigest: 'p' }, evidenceGeneration: 'g', checkout: 'repo', cwd: '/repo', title: 'Manual title', titleMode: 'manual', status: 'active', now: '', remaining: '' });
const options = { model: 'helpers/simple', thinking_level: null, max_output_tokens: 512, request_timeout_ms: 1000 };
const profiles = async () => ({ simple: { thinkingLevel: 'low' } });
const data = context => JSON.parse(context.messages[0].content[0].text);
const own = value => parseOwnSummaryDecision(JSON.stringify(value), agent);
const shared = value => parseSharedAdditionsDecision(JSON.stringify(value), workspace);
const select = value => parsePlanSelectionDecision(JSON.stringify(value), workspace);

test('own protocol retains the entire projection and limits shared context to title and plan-derived goal', () => {
  const evidence = { compactionSummary: 'checkpoint', transcript: [{ role: 'user', text: 'x'.repeat(30000) + 'END' }] };
  const context = buildOwnSummaryContext(agent, evidence, workspace);
  assert.deepEqual(data(context), { ...evidence, agent, workspace: { title: workspace.title, goal: '' } });
  assert.deepEqual(context.tools, []);
  assert.match(context.systemPrompt, /THIS AGENT/);
  assert.match(context.systemPrompt, /15–20/);
  assert.match(context.systemPrompt, /plans.*never.*proof/i);
  assert.match(context.systemPrompt, /done.*affirmative evidence/i);
  assert.match(context.systemPrompt, /not.*activity log/i);
  assert.match(context.systemPrompt, /not.*whole project/i);
  assert.doesNotMatch(context.systemPrompt, /Goal:.*\| Now:/);
});

test('own decisions use exact core title/status validation and preserve manual titles at application', () => {
  const decision = own({ action: 'update', patch: { title: 'Automatic', now: 'Adapter usable', remaining: 'Compatibility checks', status: 'waiting' } });
  assert.equal(applyOwnUpdate(agent, decision.patch).title, 'Manual title');
  assert.equal(applyOwnUpdate({ ...agent, titleMode: 'auto' }, decision.patch).title, 'Automatic');
  assert.deepEqual(own({ action: 'update', patch: {} }), { action: 'no_change' });
  for (const patch of [{ title: '' }, { title: 'bad\nname' }, { status: 'finished' }, { now: 'bad\u200b' }, { goal: 'wrong scope' }, { documents: [] }, { cwd: '/elsewhere' }, { membershipToken: 'x' }, { state: 'pipe carrier' }]) assert.throws(() => own({ action: 'update', patch }));
  for (const value of [null, [], { action: 'no_change', patch: {} }, { action: 'update' }, { action: 'update', patch: {}, extra: true }]) assert.throws(() => own(value));
});

test('shared context has only suffix evidence, checkout table/current references and own checkout', () => {
  const inherited = { compactionSummary: 'SECRET checkpoint', transcript: [{ role: 'user', text: 'SECRET inherited' }] };
  const boundary = captureReferenceBoundary(inherited);
  const projection = { ...inherited, transcript: [...inherited.transcript, { role: 'assistant', text: 'New plan.md' }] };
  const suffix = projectSharedEvidence(projection, boundary, 'continue').evidence;
  const context = buildSharedAdditionsContext(workspace, agent.checkout, suffix);
  assert.deepEqual(data(context), { transcript: suffix.transcript, checkouts: workspace.checkouts, references: { documents: workspace.documents, issues: [], prs: [] }, ownCheckout: 'repo' });
  assert.doesNotMatch(JSON.stringify(context), /SECRET|membershipToken|planSources|compactionSummary/);
  assert.throws(() => buildSharedAdditionsContext(workspace, 'repo', { transcript: [], compactionSummary: null }));
  assert.throws(() => buildSharedAdditionsContext(workspace, 'missing', suffix));
  assert.match(context.systemPrompt, /additions.*not replacements/i);
});

test('shared proposals are additive, validate known roots and leave exclusions authoritative', () => {
  const decision = shared({ action: 'add', additions: { documents: [{ ...doc, path: 'new.md' }], issues: [{ url: 'https://example.com/removed', description: 'Old' }] } });
  const applied = applySharedAdditions(workspace, decision.additions);
  assert.equal(applied.documents.length, 3);
  assert.deepEqual(applied.issues, []);
  for (const additions of [{ remove: [doc] }, { documents: [{ ...doc, checkout: 'unknown' }] }, { documents: [{ ...doc, path: '../escape.md' }] }, { issues: [{ url: 'file:///secret', description: '' }] }, { goal: 'invented' }]) assert.throws(() => shared({ action: 'add', additions }));
  assert.throws(() => shared({ action: 'update', additions: {} }));
  assert.deepEqual(shared({ action: 'no_change' }), { action: 'no_change' });
});

test('selection is metadata-only and accepts exact known Markdown identities only', () => {
  assert.deepEqual(data(buildPlanSelectionContext(workspace)), { documents: [doc] });
  assert.deepEqual(select({ sources: [{ checkout: 'repo', path: 'plan.md' }] }), { sources: [{ checkout: 'repo', path: 'plan.md' }] });
  assert.deepEqual(select({ sources: [] }), { sources: [] });
  for (const sources of [[{ checkout: 'repo', path: 'code.ts' }], [{ checkout: 'repo', path: 'missing.md' }], [doc], [{ checkout: 'repo', path: 'plan.md' }, { checkout: 'repo', path: 'plan.md' }]]) assert.throws(() => select({ sources }));
  assert.throws(() => select({ sources: [], goal: 'no' }));
  assert.match(buildPlanSelectionContext(workspace).systemPrompt, /unknown goal/);
});

const capture = { sources: [{ identity: { checkout: 'repo', path: 'plan.md' }, directory: '/repo', target: '/repo/plan.md', content: 'Deliver a library and client', bytesFingerprint: 'abc' }], fingerprint: 'all' };
test('goal protocol sees only fixed selected source identities and contents', () => {
  assert.deepEqual(data(buildWorkspaceGoalContext(capture)), { sources: [{ identity: capture.sources[0].identity, content: capture.sources[0].content }] });
  assert.equal(buildWorkspaceGoalContext({ sources: [], fingerprint: 'empty' }), null);
  assert.deepEqual(parseWorkspaceGoalDecision('{"goal":"Integrate library","title":"Integration"}'), { goal: 'Integrate library', title: 'Integration' });
  for (const value of [{ goal: 'x', sources: [] }, { goal: 'x', status: 'done' }, { goal: 'x', title: '' }, { goal: 'bad\ntext' }, { title: 'Missing goal' }]) assert.throws(() => parseWorkspaceGoalDecision(JSON.stringify(value)));
});

test('all request helpers use the existing virtual route, literal reasoning and no tools', async () => {
  let reply, calls = 0;
  const registry = { find(provider, id) { assert.equal(provider, 'helpers'); assert.equal(id, 'simple'); return { api: 'pi-virtual' }; }, streamSimple(model, context, config) {
    calls++; assert.deepEqual(context.tools, []); assert.equal(config.toolChoice, 'none'); assert.equal(config.reasoning, 'low');
    return { result: async () => ({ stopReason: 'stop', content: [{ type: 'text', text: JSON.stringify(reply) }] }) };
  } };
  reply = { action: 'no_change' };
  assert.deepEqual(await requestOwnSummary(registry, agent, { compactionSummary: null, transcript: [] }, workspace, options, undefined, profiles), reply);
  assert.deepEqual(await requestSharedAdditions(registry, workspace, 'repo', { transcript: [] }, options, undefined, profiles), reply);
  reply = { sources: [] };
  assert.deepEqual(await requestPlanSelection(registry, workspace, options, undefined, profiles), reply);
  reply = { goal: 'Integrate' };
  assert.deepEqual(await requestWorkspaceGoal(registry, capture, options, undefined, profiles), reply);
  assert.equal(await requestWorkspaceGoal(registry, { sources: [], fingerprint: 'empty' }, options, undefined, profiles), null);
  assert.equal(calls, 4);
});

test('new requests retain transport timeout, cancellation and incomplete/tool-call rejection', async () => {
  const request = (registry, config = options, signal) => requestOwnSummary(registry, agent, { compactionSummary: null, transcript: [] }, workspace, config, signal, profiles);
  const pending = { find: () => ({ api: 'pi-virtual' }), streamSimple: () => ({ result: () => new Promise(() => {}) }) };
  const controller = new AbortController();
  const result = request(pending, options, controller.signal); controller.abort(new Error('cancelled'));
  await assert.rejects(result, /cancelled/);
  const keepAlive = setTimeout(() => {}, 1000);
  try { await assert.rejects(request(pending, { ...options, request_timeout_ms: 5 }), /timeout/i); } finally { clearTimeout(keepAlive); }
  for (const response of [{ stopReason: 'length', content: [] }, { stopReason: 'stop', content: [{ type: 'toolCall', name: 'read', arguments: {} }] }, { stopReason: 'stop', content: [{ type: 'text', text: 'not JSON' }] }]) {
    await assert.rejects(request({ find: pending.find, streamSimple: () => ({ result: async () => response }) }));
  }
});
