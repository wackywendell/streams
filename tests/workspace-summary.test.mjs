import assert from 'node:assert/strict';
import test from 'node:test';
import { applyOwnUpdate, applySharedAdditions, validateAgent, parseWorkspaceV1 } from '../dist/records.js';
import {
  buildOwnSummaryContext, parseOwnSummaryDecision, requestOwnSummary,
  buildSharedAdditionsContext, parseSharedAdditionsDecision, requestSharedAdditions,
  buildPlanSelectionContext, parsePlanSelectionDecision, requestPlanSelection,
  buildWorkspaceGoalContext, parseWorkspaceGoalDecision, requestWorkspaceGoal,
  buildConversationGoalContext, requestConversationGoal,
} from '../dist/pi/workspace-summary.js';

const doc = { checkout: 'repo', path: 'plan.md', description: 'Implementation plan' };
const workspace = parseWorkspaceV1({ version: 1, id: 'w', creationToken: 'c', mutationToken: 'm', title: 'Workspace', titleMode: 'manual', goal: '', planSources: [], sourceFingerprint: null, checkouts: { repo: { directory: '/repo' } }, documents: [doc, { ...doc, path: 'code.ts' }], issues: [], prs: [], exclusions: [{ url: 'https://example.com/removed' }] });
const agent = validateAgent({ version: 1, workspaceId: 'w', sessionId: 's', membershipToken: 'm', boundary: { compactionDigest: 'd', transcriptLength: 0, prefixDigest: 'p' }, evidenceGeneration: 'g', checkout: 'repo', cwd: '/repo', title: 'Manual title', titleMode: 'manual', status: 'active', now: '', remaining: '' });
const options = { model: 'helpers/simple', thinking_level: null, max_output_tokens: 512, request_timeout_ms: 1000 };
const profiles = async () => ({ simple: { thinkingLevel: 'low' } });
const data = context => JSON.parse(context.messages[0].content[0].text);
test('reference helper treats inline file activity as candidate evidence, never completion or admission proof', () => {
  const evidence = { compactionSummary: null, transcript: [{ role: 'tool_activity', operation: 'read', path: 'plan.md' }] };
  for (const intent of ['automatic', 'refresh', 'reset']) {
    const input = buildSharedAdditionsContext(workspace, 'repo', evidence, intent);
    assert.deepEqual(data(input).transcript, evidence.transcript);
    assert.match(input.systemPrompt, /tool_activity.*attempted/i);
    assert.match(input.systemPrompt, /not.*success.*completion/i);
    assert.match(input.systemPrompt, /not.*automatically.*references/i);
  }
});

test('reset helper inputs omit prior generated context; refresh keeps it', () => {
  const evidence = { compactionSummary: 'Checkpoint', transcript: [{ role: 'user', text: 'Fresh task' }] };
  const ownReset = buildOwnSummaryContext(agent, evidence, workspace, 'reset');
  assert.deepEqual(data(ownReset), { ...evidence, agent: { checkout: agent.checkout, cwd: agent.cwd } });
  assert.match(ownReset.systemPrompt, /complete.*proposal/i);
  const sharedReset = buildSharedAdditionsContext(workspace, 'repo', evidence, 'reset');
  assert.deepEqual(data(sharedReset), { ...evidence, checkouts: workspace.checkouts, ownCheckout: 'repo' });
  assert.equal(data(buildOwnSummaryContext(agent, evidence, workspace, 'refresh')).agent.title, agent.title);
  assert.ok(data(buildSharedAdditionsContext(workspace, 'repo', evidence, 'refresh')).references);
});

test('reset own proposals must replace every derived field, never no_change or partial updates', () => {
  const complete = { title: 'Fresh task', status: 'active', statusShort: '', now: '', remaining: '' };
  const parse = value => parseOwnSummaryDecision(JSON.stringify(value), agent, 'reset');
  assert.deepEqual(parse({ action: 'update', patch: complete }), { action: 'update', patch: complete });
  assert.throws(() => parse({ action: 'no_change' }));
  for (const key of Object.keys(complete)) {
    const patch = { ...complete }; delete patch[key];
    assert.throws(() => parse({ action: 'update', patch }));
  }
  assert.deepEqual(parse({ action: 'update', patch: { ...complete, statusShort: 'x'.repeat(24) } }), { action: 'update', patch: complete });
  for (const statusShort of ['unsafe\nshort', 'unsafe\u001bshort', 12]) assert.throws(() => parse({ action: 'update', patch: { ...complete, statusShort } }));
  const applied = applyOwnUpdate({ ...agent, now: 'Stale now', remaining: 'Stale scope' }, parse({ action: 'update', patch: complete }).patch);
  assert.equal(applied.title, agent.title);
  assert.equal(applied.now, ''); assert.equal(applied.remaining, '');
});

test('reset shared proposals cannot relabel links omitted from their input', () => {
  const snapshot = { ...workspace, issues: [{ url: 'https://example.com/issue', description: 'Existing' }] };
  assert.throws(() => parseSharedAdditionsDecision(JSON.stringify({ action: 'update', labels: [{ url: snapshot.issues[0].url, label: 'Unseen' }] }), snapshot, 'reset'));
  assert.equal(parseSharedAdditionsDecision('{"action":"no_change"}', snapshot, 'reset').action, 'no_change');
});

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
  assert.match(context.systemPrompt, /automatic.*Untitled task.*provisional.*replace.*assignment.*dialogue/i);
  assert.doesNotMatch(context.systemPrompt, /Goal:.*\| Now:/);
});

test('own decisions use exact core title/status validation and preserve manual titles at application', () => {
  const decision = own({ action: 'update', patch: { title: 'Automatic', now: 'Adapter usable', remaining: 'Compatibility checks', status: 'waiting' } });
  assert.equal(applyOwnUpdate(agent, decision.patch).title, 'Manual title');
  assert.equal(applyOwnUpdate({ ...agent, titleMode: 'auto' }, decision.patch).title, 'Automatic');
  assert.deepEqual(own({ action: 'update', patch: {} }), { action: 'no_change' });
  for (const patch of [{ title: '' }, { title: 'bad\nname' }, { status: 'finished' }, { now: 'bad\u200b' }, { goal: 'wrong scope', goalShort: 'wrong scope' }, { documents: [] }, { cwd: '/elsewhere' }, { membershipToken: 'x' }, { state: 'pipe carrier' }]) assert.throws(() => own({ action: 'update', patch }));
  for (const value of [null, [], { action: 'no_change', patch: {} }, { action: 'update' }, { action: 'update', patch: {}, extra: true }]) assert.throws(() => own(value));
});

test('shared context includes the full projection and workspace purpose without runtime authority', () => {
  const projection = { compactionSummary: 'Earlier plan.md', transcript: [{ role: 'user', text: 'Retained dialogue' }, { role: 'assistant', text: 'New plan.md' }] };
  const context = buildSharedAdditionsContext(workspace, agent.checkout, projection);
  assert.deepEqual(data(context), { ...projection, workspace: { title: workspace.title, goal: '' }, checkouts: workspace.checkouts, references: { documents: workspace.documents, issues: [], prs: [] }, ownCheckout: 'repo' });
  assert.doesNotMatch(JSON.stringify(data(context)), /membershipToken|planSources|evidenceGeneration|creationToken/);
  assert.throws(() => buildSharedAdditionsContext(workspace, 'missing', projection));
  assert.match(context.systemPrompt, /additions.*not replacements/i);
  assert.match(context.systemPrompt, /latest.*compaction.*all.*dialogue/i);
  assert.match(context.systemPrompt, /workspace.*title.*Goal.*known references/i);
  assert.match(context.systemPrompt, /not.*every.*mention/i);
  assert.match(context.systemPrompt, /purpose.*unknown.*session.*context/i);
  assert.doesNotMatch(context.systemPrompt, /ONLY.*current-membership dialogue suffix|Never copy.*compaction/i);
});

test('documents are a small current reading list without a hard cap or cleanup authority', () => {
  const prompt = buildSharedAdditionsContext(workspace, 'repo', { compactionSummary: null, transcript: [] }).systemPrompt;
  assert.match(prompt, /small current reading list, not.*history/i);
  assert.match(prompt, /explains the workspace purpose, governs current work, or is an active plan/i);
  assert.match(prompt, /Usually 3–5.*soft guide, not a quota or hard cap/i);
  assert.match(prompt, /complementary documents.*overlapping/i);
  assert.match(prompt, /read or edited.*does not qualify/i);
  assert.match(prompt, /another agent may still need/i);
  assert.match(prompt, /These are additions, not replacements or removals/i);
  const many = Array.from({ length: 6 }, (_, i) => ({ ...doc, path: `reference-${i}.md` }));
  const decision = shared({ action: 'update', additions: { documents: many } });
  assert.equal(decision.additions.documents.length, 6);
});

test('shared issue admission means actual published tickets, not CI or informal problems', () => {
  const prompt = buildSharedAdditionsContext(workspace, 'repo', { compactionSummary: null, transcript: [] }).systemPrompt;
  assert.match(prompt, /Issues are actual published issues or tickets/i);
  assert.match(prompt, /GitHub Issues.*Jira tickets/i);
  assert.match(prompt, /Never classify CI job\/run links, change-request links or log links as issues/i);
  assert.match(prompt, /informal problems without a published ticket are not issues/i);
  assert.match(prompt, /semantic admission rules, not an exhaustive provider allowlist/i);
});

test('shared PR admission requires this session assignment focus rather than mere mention', () => {
  const prompt = buildSharedAdditionsContext(workspace, 'repo', { compactionSummary: null, transcript: [] }).systemPrompt;
  assert.match(prompt, /PRs must be actively worked on or focused on in THIS session/i);
  assert.match(prompt, /created, authored, reviewed, debugged or explicitly tracked for this assignment/i);
  assert.match(prompt, /Merely mentioned PRs, interesting examples and background links do not qualify/i);
});

test('semantic admission adds no other-links authority and never silently removes stored references', () => {
  const existing = { ...workspace, issues: [{ url: 'https://example.com/ci/run/1', description: 'Previously stored run' }] };
  const context = buildSharedAdditionsContext(existing, 'repo', { compactionSummary: null, transcript: [] });
  assert.deepEqual(data(context).references.issues, existing.issues);
  assert.match(context.systemPrompt, /Do not silently delete or reclassify existing stored references/i);
  assert.match(context.systemPrompt, /Do not introduce an other-links bucket/i);
  assert.throws(() => parseSharedAdditionsDecision(JSON.stringify({ action: 'update', additions: { links: [{ url: 'https://example.com/log', description: 'Log' }] } }), existing));
  assert.deepEqual(parseSharedAdditionsDecision('{"action":"no_change"}', existing), { action: 'no_change' });
});

test('shared proposals are additive, validate known roots and leave exclusions authoritative', () => {
  const decision = shared({ action: 'update', additions: { documents: [{ ...doc, path: 'new.md' }], issues: [{ url: 'https://example.com/removed', description: 'Old' }] } });
  const applied = applySharedAdditions(workspace, decision.additions);
  assert.equal(applied.documents.length, 3);
  assert.deepEqual(applied.issues, []);
  for (const additions of [{ remove: [doc] }, { documents: [{ ...doc, checkout: 'unknown' }] }, { documents: [{ ...doc, path: '../escape.md' }] }, { issues: [{ url: 'file:///secret', description: '' }] }, { goal: 'invented', goalShort: 'invented' }]) assert.throws(() => shared({ action: 'update', additions }));
  assert.throws(() => shared({ action: 'add', additions: {} }));
  assert.deepEqual(shared({ action: 'no_change' }), { action: 'no_change' });
});

test('selection is metadata-only and accepts exact known Markdown identities only', () => {
  assert.deepEqual(data(buildPlanSelectionContext(workspace)), { documents: [doc] });
  assert.deepEqual(select({ sources: [{ checkout: 'repo', path: 'plan.md' }] }), { sources: [{ checkout: 'repo', path: 'plan.md' }] });
  assert.deepEqual(select({ sources: [] }), { sources: [] });
  for (const sources of [[{ checkout: 'repo', path: 'code.ts' }], [{ checkout: 'repo', path: 'missing.md' }], [doc], [{ checkout: 'repo', path: 'plan.md' }, { checkout: 'repo', path: 'plan.md' }]]) assert.throws(() => select({ sources }));
  assert.throws(() => select({ sources: [], goal: 'no' }));
  assert.match(buildPlanSelectionContext(workspace).systemPrompt, /Conversation purpose.*separately.*empty selection/);
});

const capture = { sources: [{ identity: { checkout: 'repo', path: 'plan.md' }, directory: '/repo', target: '/repo/plan.md', content: 'Deliver a library and client', bytesFingerprint: 'abc' }], fingerprint: 'all' };
test('goal protocol sees only fixed selected source identities and contents', () => {
  assert.deepEqual(data(buildWorkspaceGoalContext(capture)), { sources: [{ identity: capture.sources[0].identity, content: capture.sources[0].content }] });
  assert.equal(buildWorkspaceGoalContext({ sources: [], fingerprint: 'empty' }), null);
  assert.deepEqual(parseWorkspaceGoalDecision('{"goal":"Integrate library","goalShort":"Library integration","title":"Integration"}'), { goal: 'Integrate library', goalShort: 'Library integration', title: 'Integration' });
  for (const value of [{ goal: 'x', goalShort: 'x', sources: [] }, { goal: 'x', goalShort: 'x', status: 'done' }, { goal: 'x', goalShort: 'x', title: '' }, { goal: 'bad\ntext', goalShort: 'bad\ntext' }, { title: 'Missing goal' }]) assert.throws(() => parseWorkspaceGoalDecision(JSON.stringify(value)));
});

test('conversation purpose sees projected dialogue only, reconciles shared purpose and omits generated context on reset', () => {
  const evidence = { compactionSummary: 'Checkpoint', transcript: [{ role: 'user', text: 'Build a reusable parser' }] };
  assert.equal(buildConversationGoalContext(workspace, { compactionSummary: null, transcript: [] }), null);
  assert.deepEqual(data(buildConversationGoalContext(workspace, evidence)), { ...evidence, workspace: { title: workspace.title, goal: '' } });
  assert.deepEqual(data(buildConversationGoalContext(workspace, evidence, 'reset')), evidence);
  const prompt = buildConversationGoalContext(workspace, evidence).systemPrompt;
  assert.match(prompt, /no word or message-count threshold/i); assert.match(prompt, /Nonempty chatter.*insufficient/i);
  assert.match(prompt, /not.*aggregate progress.*completion/i); assert.match(prompt, /No tool paths, tool results, source bodies or repository crawling/);
  assert.match(prompt, /joining agent.*not a replacement workspace purpose/i);
});

test('conversation helper reuses strict purpose validation and ignores names on insufficient proposals', async () => {
  let reply;
  const registry = { find: () => ({ api: 'pi-virtual' }), streamSimple(_model, context) {
    assert.deepEqual(context.tools, []);
    return { result: async () => ({ stopReason: 'stop', content: [{ type: 'text', text: JSON.stringify(reply) }] }) };
  } };
  const evidence = { compactionSummary: null, transcript: [{ role: 'user', text: 'Hello' }] };
  const request = intent => requestConversationGoal(registry, workspace, evidence, { ...options, intent }, undefined, profiles);
  reply = { goal: '', goalShort: '', title: 'Invented' };
  assert.deepEqual(await request(), { goal: '', goalShort: '' });
  reply = { goal: 'Reusable parser', goalShort: '界'.repeat(13), title: 'Parser' };
  assert.deepEqual(await request(), { goal: 'Reusable parser', title: 'Parser' });
  assert.deepEqual(await request('reset'), { goal: 'Reusable parser', title: 'Parser' });
  for (const invalid of [
    null, {}, { goal: 'Purpose' }, { goal: 'Purpose', goalShort: '' },
    { goal: '', goalShort: 'Nonempty' }, { goal: 'bad\ntext', goalShort: 'Short' },
    { goal: 'Purpose', goalShort: 'x'.repeat(30) + '\u200b' },
    { goal: '', goalShort: '', title: 'bad\nname' },
    { goal: 'Purpose', goalShort: 'Short', status: 'done' },
    { goal: 'Purpose', goalShort: 'Short', sources: [] },
  ]) { reply = invalid; await assert.rejects(request()); }
});

test('all request helpers use the existing virtual route, literal reasoning and no tools', async () => {
  let reply, calls = 0;
  const registry = { find(provider, id) { assert.equal(provider, 'helpers'); assert.equal(id, 'simple'); return { api: 'pi-virtual' }; }, streamSimple(model, context, config) {
    calls++; assert.deepEqual(context.tools, []); assert.equal(config.toolChoice, 'none'); assert.equal(config.reasoning, 'low');
    return { result: async () => ({ stopReason: 'stop', content: [{ type: 'text', text: JSON.stringify(reply) }] }) };
  } };
  reply = { action: 'no_change' };
  assert.deepEqual(await requestOwnSummary(registry, agent, { compactionSummary: null, transcript: [] }, workspace, options, undefined, profiles), reply);
  assert.deepEqual(await requestSharedAdditions(registry, workspace, 'repo', { compactionSummary: null, transcript: [] }, options, undefined, profiles), reply);
  reply = { sources: [] };
  assert.deepEqual(await requestPlanSelection(registry, workspace, options, undefined, profiles), reply);
  reply = { goal: 'Integrate', goalShort: 'Integrate' };
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


test('oversized helper shorts are omitted without truncating valid full text', () => {
  const now = 'The full assignment summary remains intact even when the sidebar summary is too wide';
  assert.deepEqual(own({ action: 'update', patch: { statusShort: 'Adapter usable' } }).patch, { statusShort: 'Adapter usable' });
  assert.deepEqual(own({ action: 'update', patch: { now, statusShort: 'x'.repeat(24) } }), { action: 'update', patch: { now } });
  assert.deepEqual(own({ action: 'update', patch: { statusShort: 'x'.repeat(24) } }), { action: 'no_change' });
  assert.deepEqual(own({ action: 'update', patch: { now } }), { action: 'update', patch: { now } });
  assert.deepEqual(parseWorkspaceGoalDecision(JSON.stringify({ goal: now, goalShort: '界'.repeat(13), title: 'Workspace subject' })), { goal: now, title: 'Workspace subject' });
  assert.deepEqual(parseWorkspaceGoalDecision('{"goal":"","goalShort":""}'), { goal: '', goalShort: '' });
  assert.deepEqual(parseWorkspaceGoalDecision('{"goal":""}'), { goal: '' });
  assert.match(buildOwnSummaryContext(agent, { compactionSummary: null, transcript: [] }, workspace).systemPrompt, /statusShort is absent or empty/);
  assert.match(buildPlanSelectionContext(workspace).systemPrompt, /design.*ADR.*README/i);
  assert.match(buildSharedAdditionsContext(workspace, 'repo', { compactionSummary: null, transcript: [] }).systemPrompt, /durable.*orientation/i);
});

for (const [name, statusShort, goalShort] of [
  ['ASCII', 'x'.repeat(23), 'x'.repeat(25)],
  ['CJK', '界'.repeat(11) + 'x', '界'.repeat(12) + 'x'],
  ['combining', 'e\u0301'.repeat(23), 'e\u0301'.repeat(25)],
  ['emoji', '😀'.repeat(11) + 'x', '😀'.repeat(12) + 'x'],
]) test(`${name} short boundaries use display cells and never truncate raw text`, () => {
  assert.deepEqual(own({ action: 'update', patch: { statusShort } }), { action: 'update', patch: { statusShort } });
  assert.deepEqual(own({ action: 'update', patch: { statusShort: statusShort + 'x' } }), { action: 'no_change' });
  assert.deepEqual(parseWorkspaceGoalDecision(JSON.stringify({ goal: 'Purpose', goalShort })), { goal: 'Purpose', goalShort });
  assert.deepEqual(parseWorkspaceGoalDecision(JSON.stringify({ goal: 'Purpose', goalShort: goalShort + 'x' })), { goal: 'Purpose' });
});

test('omitting oversized own shorts leaves core source-change invalidation authoritative', () => {
  const previous = { ...agent, statusShort: 'Old stage' };
  for (const patch of [{ status: 'waiting' }, { now: 'New useful stage' }, { remaining: 'New scope' }]) {
    const decision = parseOwnSummaryDecision(JSON.stringify({ action: 'update', patch: { ...patch, statusShort: 'x'.repeat(24) } }), previous);
    assert.equal(applyOwnUpdate(previous, decision.patch).statusShort, undefined);
  }
  const unchanged = parseOwnSummaryDecision(JSON.stringify({ action: 'update', patch: { now: previous.now, statusShort: 'x'.repeat(24) } }), previous);
  assert.equal(applyOwnUpdate(previous, unchanged.patch).statusShort, 'Old stage');
});

test('short overflow does not excuse unsafe text, malformed required fields or unknown fields', () => {
  for (const short of [null, 42, {}, [], 'x'.repeat(30) + '\n', 'x'.repeat(30) + '\u200b', 'x'.repeat(30) + '\ud800', '\udc00']) {
    assert.throws(() => own({ action: 'update', patch: { now: 'Valid full text', statusShort: short } }));
    assert.throws(() => parseWorkspaceGoalDecision(JSON.stringify({ goal: 'Purpose', goalShort: short })));
  }
  for (const value of [
    { goal: 'Purpose' }, { goal: 'Purpose', goalShort: '' }, { goal: 'Purpose', goalShort: '   ' },
    { goal: '', goalShort: 'x'.repeat(30) }, { goal: 'bad\ntext', goalShort: 'x'.repeat(30) },
    { goal: 42, goalShort: 'x'.repeat(30) }, { goalShort: 'x'.repeat(30) },
    { goal: 'Purpose', goalShort: 'x'.repeat(30), extra: true },
    { goal: 'Purpose', goalShort: 'x'.repeat(30), title: '' },
  ]) assert.throws(() => parseWorkspaceGoalDecision(JSON.stringify(value)));
  for (const patch of [{ now: 'bad\ntext' }, { status: 'finished' }, { extra: true }]) {
    assert.throws(() => own({ action: 'update', patch: { ...patch, statusShort: 'x'.repeat(30) } }));
  }
});

test('shared labels target only exact snapshot links and cannot carry replacement authority', () => {
  const snapshot = { ...workspace, issues: [{ url: 'https://example.com/issue', description: 'Full text' }] };
  const parse = value => parseSharedAdditionsDecision(JSON.stringify(value), snapshot);
  assert.deepEqual(parse({ action: 'update', labels: [{ url: snapshot.issues[0].url, label: 'Issue purpose' }] }), { action: 'update', labels: [{ url: snapshot.issues[0].url, label: 'Issue purpose' }] });
  for (const labels of [
    [{ url: 'https://example.com/unknown', label: 'Unknown' }],
    [{ url: snapshot.issues[0].url + '/', label: 'Not exact' }],
    [{ url: snapshot.issues[0].url, label: 'New', description: 'Replaced' }],
    [{ url: snapshot.issues[0].url, label: '' }],
  ]) assert.throws(() => parse({ action: 'update', labels }));
  assert.throws(() => parse({ action: 'update', issues: [] }));
  assert.throws(() => parse({ action: 'update', labels: [{ checkout: 'repo', path: 'plan.md', label: 'Doc' }] }));
});
