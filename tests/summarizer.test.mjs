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
const options = { max_context_chars: 24000, update_guidance: "Keep meaningful state concise." };
const config = { ...options, model: "helpers/simple", thinking_level: null, max_output_tokens: 2048, request_timeout_ms: 30000 };
const text = (context) => context.messages[0].content[0].text;
const size = (context) => context.systemPrompt.length + text(context).length;
const response = (content, stopReason = "stop") => ({ stopReason, content: [{ type: "text", text: content }] });

test("context preserves current card and normalized textual evidence", () => {
  const context = buildSummaryContext(card, [
    { role: "user", text: "User request" },
    { role: "assistant", text: "Visible response" },
    { role: "toolResult", text: "Result evidence" },
  ], options);
  const data = JSON.parse(text(context));
  assert.deepEqual(data.card, card);
  assert.deepEqual(data.evidence, [
    { role: "user", text: "User request" },
    { role: "assistant", text: "Visible response" },
    { role: "toolResult", text: "Result evidence" },
  ]);
  assert.doesNotMatch(JSON.stringify(context), /SECRET|TOOL_SIGNATURE/);
  assert.deepEqual(context.tools, []);
  assert.match(context.systemPrompt, /no_change/);
  assert.match(context.systemPrompt, /affirmative evidence/);
});

test("document descriptions describe the whole purpose briefly, not the latest edit", () => {
  const context = buildSummaryContext(card, [], options);
  assert.match(context.systemPrompt, /Document descriptions must briefly describe the whole document's purpose, never the latest change or edit\./);
  assert.deepEqual(JSON.parse(text(context)).card.documents, card.documents);
});

test("actual serialized character budget favors newest evidence including escape-heavy text", () => {
  const base = buildSummaryContext(card, [], options);
  const budget = size(base) + 160;
  const context = buildSummaryContext(card, [
    { role: "user", text: "OLD_EVIDENCE" },
    { role: "assistant", text: "\n\"\\".repeat(400) + "NEWEST_EVIDENCE" },
  ], { ...options, max_context_chars: budget });
  assert.ok(size(context) <= budget);
  const data = JSON.parse(text(context));
  assert.deepEqual(data.card, card);
  assert.equal(data.evidence.length, 1);
  assert.match(data.evidence[0].text, /NEWEST_EVIDENCE$/);
  assert.match(data.evidence[0].text, /^\[Earlier text omitted\]/);
  assert.doesNotMatch(text(context), /OLD_EVIDENCE/);
});

test("prompt truncation copies evidence and leaves frozen queued input unchanged", () => {
  const messages = Object.freeze([
    Object.freeze({ role: "user", text: "OLD_EVIDENCE" }),
    Object.freeze({ role: "assistant", text: "\n\"\\".repeat(400) + "NEWEST_EVIDENCE" }),
  ]);
  const before = structuredClone(messages);
  const budget = size(buildSummaryContext(card, [], options)) + 160;
  const context = buildSummaryContext(card, messages, { ...options, max_context_chars: budget });
  assert.ok(size(context) <= budget);
  assert.match(JSON.parse(text(context)).evidence[0].text, /NEWEST_EVIDENCE$/);
  assert.deepEqual(messages, before);
});

test("full current card and instructions are never silently truncated", () => {
  const base = buildSummaryContext(card, [], options);
  assert.throws(() => buildSummaryContext(card, [], { ...options, max_context_chars: size(base) - 1 }), /max_context_chars.*full current card/i);
  const exact = buildSummaryContext(card, [{ role: "user", text: "omitted" }], { ...options, max_context_chars: size(base) });
  assert.equal(size(exact), size(base));
  assert.deepEqual(JSON.parse(text(exact)).card, card);
  assert.deepEqual(JSON.parse(text(exact)).evidence, []);
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
  const context = buildSummaryContext(card, [], options);
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
  await requestSummary(registry, buildSummaryContext(card, [], options), { ...config, model: "helpers/moderate" });
  assert.equal(reasoning, "off");
  await writeFile(join(directory, "helper-models.json"), "not JSON");
  await assert.rejects(requestSummary(registry, buildSummaryContext(card, [], options), { ...config, model: "helpers/moderate" }), /helper-models.json/);
});

test("physical model IDs retain slashes and do not read helper profiles", async () => {
  let captured;
  const registry = {
    find: (provider, id) => { assert.equal(provider, "openrouter"); assert.equal(id, "vendor/model"); return { provider, id }; },
    streamSimple: (_model, _context, opts) => { captured = opts; return { result: async () => response('{"action":"no_change"}') }; },
  };
  await requestSummary(registry, buildSummaryContext(card, [], options), { ...config, model: "openrouter/vendor/model" }, undefined,
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
    await requestSummary(registry, buildSummaryContext(card, [], options), { ...config, model, thinking_level }, undefined,
      async () => { throw new Error("explicit overrides must not read helper profiles"); });
    assert.equal(captured, expected);
  }
});

test("missing alias/profile and invalid or unsuccessful model output fail actionably", async () => {
  const context = buildSummaryContext(card, [], options);
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
  const pending = requestSummary(registry, buildSummaryContext(card, [], options), { ...physicalConfig, request_timeout_ms: 10 });
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
  const pending = requestSummary(registry, buildSummaryContext(card, [], options), physicalConfig, controller.signal);
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
    const pending = requestSummary(registry, buildSummaryContext(card, [], options), physicalConfig, controller.signal);
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
    const pending = requestSummary(registry, buildSummaryContext(card, [], options), physicalConfig);
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
  await assert.rejects(requestSummary(registry, buildSummaryContext(card, [], options), config, controller.signal,
    async () => ({ simple: { thinkingLevel: "off" } })), /abort/i);
});
