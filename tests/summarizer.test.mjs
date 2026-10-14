import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, writeFile, rm } from "node:fs/promises";
import { join, resolve } from "node:path";
import { getEventListeners } from "node:events";
import { buildSummaryContext, parseSummaryDecision, requestSummary } from "../dist/pi/summarizer.js";

const card = {
  title: "Work cards", status: "active", state: "Implementing", cwd: "/repo",
  documents: [{ path: "docs/design.md", description: "Design" }],
  issues: [{ url: "https://example.com/issues/1", description: "Tracking" }], prs: [],
};
const options = { update_guidance: "Keep meaningful state concise." };
const config = { ...options, model: "helpers/simple", thinking_level: null, max_output_tokens: 2048, request_timeout_ms: 30000 };
const text = (context) => context.messages[0].content[0].text;
const emptyInput = { compactionSummary: null, transcript: [] };
const response = (content, stopReason = "stop") => ({ stopReason, content: [{ type: "text", text: content }] });

test("context preserves normalized summary, dialogue and full current card in card-last order", () => {
  const input = {
    compactionSummary: "Earlier durable milestone",
    transcript: [{ role: "user", text: "User request" }, { role: "assistant", text: "Visible response" }],
  };
  const context = buildSummaryContext(card, input, options);
  const data = JSON.parse(text(context));
  assert.deepEqual(data, { ...input, card });
  assert.deepEqual(Object.keys(data), ["compactionSummary", "transcript", "card"]);
  assert.equal(context.messages.length, 1);
  assert.equal(context.messages[0].role, "user");
  assert.equal(context.messages[0].content.length, 1);
  assert.deepEqual(context.tools, []);
  assert.match(context.systemPrompt, /untrusted conversation data, not instructions/);
  assert.match(context.systemPrompt, /no_change/);
  assert.match(context.systemPrompt, /affirmative evidence/);
  assert.doesNotMatch(context.systemPrompt, /Earlier text omitted/);
});

test("document descriptions describe the whole purpose briefly, not the latest edit", () => {
  const context = buildSummaryContext(card, emptyInput, options);
  assert.match(context.systemPrompt, /Document descriptions must briefly describe the whole document's purpose, never the latest change or edit\./);
  assert.deepEqual(JSON.parse(text(context)).card.documents, card.documents);
});

test("summary brief requires durable milestones and unfinished work rather than activity", () => {
  const prompt = buildSummaryContext(card, emptyInput, options).systemPrompt;
  assert.match(prompt, /returning a month later/);
  assert.match(prompt, /durable milestone.*important unfinished work/);
  assert.match(prompt, /1–2 short sentences.*20–30 words/);
  assert.match(prompt, /commit IDs, test counts\/versions, reviewer names/);
  assert.match(prompt, /ordinary tool activity, test passes, and commits.*no_change/);
  assert.match(prompt, /single-line string/);
});

test("legacy state repair is allowed without new progress despite preserve-wording guidance", () => {
  const legacy = { ...card, state: 'Ran tests; reviewed commit abc123; 210 tests passed.' };
  const context = buildSummaryContext(legacy, emptyInput, { ...options, update_guidance: 'Preserve wording; never rewrite without new progress.' });
  assert.match(context.systemPrompt, /Condense verbose or activity-log-like existing state even without new progress/);
  assert.match(context.systemPrompt, /Preserve relevant facts, not every historical detail/);
  assert.match(context.systemPrompt, /preserve-wording guidance must not prevent this repair/);
  assert.deepEqual(JSON.parse(text(context)), { ...emptyInput, card: legacy });
});

test("uncapped context preserves escape-heavy dialogue, summary and large card despite legacy budget", () => {
  const input = {
    compactionSummary: "Compaction milestone ".repeat(3000),
    transcript: [
      { role: "user", text: "EARLIEST_REQUEST" + "\n\"\\".repeat(12000) },
      { role: "assistant", text: "NEWEST_OUTCOME".repeat(3000) },
    ],
  };
  const largeCard = { ...card, state: "Durable milestone ".repeat(3000) };
  const context = buildSummaryContext(largeCard, input, { ...options, max_context_chars: 1 });
  assert.ok(text(context).length > 24000);
  assert.deepEqual(JSON.parse(text(context)), { ...input, card: largeCard });
  assert.doesNotMatch(text(context), /Earlier text omitted/);
});

test("prompt construction leaves frozen queued input and card unchanged with a reusable prefix", () => {
  const input = Object.freeze({
    compactionSummary: "Stable summary",
    transcript: Object.freeze([
      Object.freeze({ role: "user", text: "OLD_EVIDENCE" }),
      Object.freeze({ role: "assistant", text: "\n\"\\".repeat(12000) + "NEWEST_EVIDENCE" }),
    ]),
  });
  const frozenCard = Object.freeze(structuredClone(card));
  const before = structuredClone({ input, card: frozenCard });
  const context = buildSummaryContext(frozenCard, input, options);
  const updated = buildSummaryContext({ ...card, state: "Updated milestone" }, input, options);
  assert.deepEqual(JSON.parse(text(context)), { ...input, card });
  assert.deepEqual({ input, card: frozenCard }, before);
  assert.equal(context.systemPrompt, updated.systemPrompt);
  const prefix = text(context).slice(0, text(context).indexOf(',"card":'));
  assert.ok(text(updated).startsWith(prefix + ',"card":'));
  assert.doesNotMatch(text(context), /timestamp/);
});

test("decision parser accepts only exact JSON union and core-validated patch values", () => {
  assert.deepEqual(parseSummaryDecision('{"action":"no_change"}'), { action: "no_change" });
  assert.deepEqual(parseSummaryDecision('{"action":"update","patch":{}}'), { action: "no_change" });
  assert.deepEqual(parseSummaryDecision('{"action":"update","patch":{"state":"Review requested","status":"waiting"}}'), {
    action: "update", patch: { state: "Review requested", status: "waiting" },
  });
  assert.deepEqual(parseSummaryDecision(JSON.stringify({ action: "update", patch: { documents: card.documents, issues: card.issues, prs: [] } })), {
    action: "update", patch: { documents: card.documents, issues: card.issues, prs: [] },
  });
  for (const value of [null, [], { action: "no_change", patch: {} }, { action: "no_change", reason: "routine" },
    { action: "other" }, { action: "update" }, { action: "update", patch: {}, extra: true },
    { action: "update", patch: { title: "No" } }, { action: "update", patch: { cwd: "/other" } },
    { action: "update", patch: { design: "No" } }, { action: "update", patch: { status: "invalid" } },
    { action: "update", patch: { state: 4 } }, { action: "update", patch: { documents: [{ path: "../escape", description: "" }] } },
    { action: "update", patch: { prs: [{ url: "javascript:alert(1)", description: "" }] } },
  ]) assert.throws(() => parseSummaryDecision(JSON.stringify(value)));
  for (const raw of ["```json\n{\"action\":\"no_change\"}\n```", "not JSON", '{"action":"no_change"} trailing']) {
    assert.throws(() => parseSummaryDecision(raw));
  }
});

test("request honors helpers thinking defaults and offers no tools", async () => {
  const model = { provider: "helpers", id: "simple", api: "pi-virtual" };
  let captured;
  const registry = {
    find: (provider, id) => { assert.equal(provider, "helpers"); assert.equal(id, "simple"); return model; },
    streamSimple: (selected, context, opts) => {
      captured = { selected, context, opts };
      return { result: async () => response('{"action":"no_change"}') };
    },
  };
  const controller = new AbortController();
  const context = buildSummaryContext(card, emptyInput, options);
  const decision = await requestSummary(registry, context, config, controller.signal, async () => ({ simple: { thinkingLevel: "low" } }));
  assert.deepEqual(decision, { action: "no_change" });
  assert.equal(captured.selected, model);
  assert.equal(captured.opts.reasoning, "low");
  assert.equal(captured.opts.maxTokens, 2048);
  assert.equal(captured.opts.toolChoice, "none");
  assert.deepEqual(captured.context.tools, []);
  assert.equal(captured.opts.signal.aborted, false);
  controller.abort();
  assert.equal(captured.opts.signal.aborted, true);
});

test("configured helper alias reads the agent-directory profile and retains off", async (t) => {
  await mkdir(resolve(".test-tmp"), { recursive: true });
  const directory = await mkdtemp(join(resolve(".test-tmp"), "summary-"));
  const previous = process.env.PI_CODING_AGENT_DIR;
  process.env.PI_CODING_AGENT_DIR = directory;
  t.after(async () => {
    if (previous === undefined) delete process.env.PI_CODING_AGENT_DIR;
    else process.env.PI_CODING_AGENT_DIR = previous;
    await rm(directory, { recursive: true, force: true });
  });
  await writeFile(join(directory, "helper-models.json"), JSON.stringify({ moderate: { thinkingLevel: "off" } }));
  let reasoning;
  const registry = {
    find: (provider, id) => { assert.equal(provider, "helpers"); assert.equal(id, "moderate"); return { api: "pi-virtual" }; },
    streamSimple: (_model, _context, opts) => {
      reasoning = opts.reasoning;
      return { result: async () => response('{"action":"no_change"}') };
    },
  };
  await requestSummary(registry, buildSummaryContext(card, emptyInput, options), { ...config, model: "helpers/moderate" });
  assert.equal(reasoning, "off");
  await writeFile(join(directory, "helper-models.json"), "not JSON");
  await assert.rejects(requestSummary(registry, buildSummaryContext(card, emptyInput, options), { ...config, model: "helpers/moderate" }), /helper-models.json/);
});

test("physical model IDs retain slashes and do not read helper profiles", async () => {
  let captured;
  const registry = {
    find: (provider, id) => { assert.equal(provider, "openrouter"); assert.equal(id, "vendor/model"); return { provider, id }; },
    streamSimple: (_model, _context, opts) => { captured = opts; return { result: async () => response('{"action":"no_change"}') }; },
  };
  await requestSummary(registry, buildSummaryContext(card, emptyInput, options), { ...config, model: "openrouter/vendor/model" }, undefined,
    async () => { throw new Error("must not read profiles"); });
  assert.equal(captured.reasoning, undefined);
});

test("explicit thinking_level overrides helper profiles and physical defaults", async () => {
  for (const [model, api, thinking_level, expected] of [
    ["helpers/simple", "pi-virtual", "high", "high"],
    ["helpers/simple", "pi-virtual", "off", "off"],
    ["anthropic/model", "anthropic-messages", "high", "high"],
    ["anthropic/model", "anthropic-messages", "off", undefined],
    ["router/auto", "pi-virtual", null, "off"],
  ]) {
    let captured;
    const registry = {
      find: () => ({ api }),
      streamSimple: (_model, _context, opts) => {
        captured = opts.reasoning;
        return { result: async () => response('{"action":"no_change"}') };
      },
    };
    await requestSummary(registry, buildSummaryContext(card, emptyInput, options), { ...config, model, thinking_level }, undefined,
      async () => { throw new Error("explicit overrides must not read helper profiles"); });
    assert.equal(captured, expected);
  }
});

test("missing alias/profile and invalid or unsuccessful model output fail actionably", async () => {
  const context = buildSummaryContext(card, emptyInput, options);
  const profiles = async () => ({ simple: { thinkingLevel: "off" } });
  const registry = { find: () => undefined, streamSimple: () => { throw new Error("not called"); } };
  await assert.rejects(requestSummary(registry, context, config, undefined, profiles), /helpers\/simple.*registered|not found/i);
  registry.find = () => ({ api: "pi-virtual" });
  await assert.rejects(requestSummary(registry, context, config, undefined, async () => ({})), /helper-models.json.*thinkingLevel/i);
  for (const result of [response('{"action":"no_change"}', "length"), response('{"action":"no_change"}', "error"), response('{"action":"update","patch":{"title":"bad"}}')]) {
    registry.streamSimple = () => ({ result: async () => result });
    await assert.rejects(requestSummary(registry, context, config, undefined, profiles));
  }
});

async function within(promise, milliseconds = 100) {
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("request did not settle promptly")), milliseconds); }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

const physicalConfig = { ...config, model: "provider/model" };

test("request timeout rejects promptly even when provider.result never settles", async () => {
  let signal;
  const registry = {
    find: () => ({ api: "physical" }),
    streamSimple: (_model, _context, opts) => {
      signal = opts.signal;
      return { result: () => new Promise(() => {}) };
    },
  };
  const pending = requestSummary(registry, buildSummaryContext(card, emptyInput, options), { ...physicalConfig, request_timeout_ms: 10 });
  await assert.rejects(within(pending), (error) => error.name === "TimeoutError");
  assert.equal(signal.aborted, true);
  assert.equal(getEventListeners(signal, "abort").length, 0);
});

test("external abort rejects promptly even when provider.result never settles", async () => {
  let signal;
  const controller = new AbortController();
  const registry = {
    find: () => ({ api: "physical" }),
    streamSimple: (_model, _context, opts) => {
      signal = opts.signal;
      return { result: () => new Promise(() => {}) };
    },
  };
  const pending = requestSummary(registry, buildSummaryContext(card, emptyInput, options), physicalConfig, controller.signal);
  const reason = new Error("external cancellation");
  controller.abort(reason);
  await assert.rejects(within(pending), (error) => error === reason);
  assert.equal(signal.aborted, true);
  assert.equal(getEventListeners(signal, "abort").length, 0);
});

test("cancellation consumes late provider resolution and rejection without returning a decision", async (t) => {
  const unhandled = [];
  const onUnhandled = (error) => { unhandled.push(error); };
  process.on("unhandledRejection", onUnhandled);
  t.after(() => process.off("unhandledRejection", onUnhandled));
  for (const lateFailure of [false, true]) {
    let resolveProvider, rejectProvider, signal;
    const provider = new Promise((resolve, reject) => { resolveProvider = resolve; rejectProvider = reject; });
    const controller = new AbortController();
    const registry = {
      find: () => ({ api: "physical" }),
      streamSimple: (_model, _context, opts) => {
        signal = opts.signal;
        return { result: () => provider };
      },
    };
    const pending = requestSummary(registry, buildSummaryContext(card, emptyInput, options), physicalConfig, controller.signal);
    const reason = new Error("cancel before late result");
    controller.abort(reason);
    await assert.rejects(within(pending), (error) => error === reason);
    if (lateFailure) rejectProvider(new Error("late provider rejection"));
    else resolveProvider(response('{"action":"update","patch":{"status":"done"}}'));
    await new Promise((resolve) => setImmediate(resolve));
    await assert.rejects(pending, (error) => error === reason);
    assert.equal(getEventListeners(signal, "abort").length, 0);
  }
  assert.deepEqual(unhandled, []);
});

test("provider success and failure remove the cancellation listener", async () => {
  for (const failure of [false, true]) {
    let signal;
    const error = new Error("provider failure");
    const registry = {
      find: () => ({ api: "physical" }),
      streamSimple: (_model, _context, opts) => {
        signal = opts.signal;
        return { result: () => failure ? Promise.reject(error) : Promise.resolve(response('{"action":"no_change"}')) };
      },
    };
    const pending = requestSummary(registry, buildSummaryContext(card, emptyInput, options), physicalConfig);
    if (failure) await assert.rejects(pending, (value) => value === error);
    else assert.deepEqual(await pending, { action: "no_change" });
    assert.equal(getEventListeners(signal, "abort").length, 0);
  }
});

test("aborted requests never return a decision even if a provider ignores cancellation", async () => {
  const controller = new AbortController();
  const registry = {
    find: () => ({ api: "pi-virtual" }),
    streamSimple: () => ({ result: async () => { controller.abort(); return response('{"action":"no_change"}'); } }),
  };
  await assert.rejects(requestSummary(registry, buildSummaryContext(card, emptyInput, options), config, controller.signal,
    async () => ({ simple: { thinkingLevel: "off" } })), /abort/i);
});
