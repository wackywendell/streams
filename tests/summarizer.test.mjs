import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, writeFile, rm } from "node:fs/promises";
import { join, resolve } from "node:path";
import { getEventListeners } from "node:events";
import { requestHelperDecision } from "../dist/pi/summarizer.js";

const context = { systemPrompt: 'Return JSON.', messages: [{ role: 'user', content: [{ type: 'text', text: 'Evidence' }], timestamp: 0 }], tools: [] };
const config = { model: "helpers/simple", thinking_level: null, max_output_tokens: 2048, request_timeout_ms: 30000 };
const response = (content, stopReason = "stop") => ({ stopReason, content: [{ type: "text", text: content }] });

test('request activity starts only after helper-profile preparation, around the real request', async () => {
  let release, started = 0, stopped = 0;
  const profiles = new Promise(resolve => { release = resolve; });
  const registry = {
    find: () => ({ api: 'pi-virtual' }),
    streamSimple: () => {
      assert.equal(started, 1); assert.equal(stopped, 0);
      return { result: async () => response('{"action":"no_change"}') };
    },
  };
  const requested = requestHelperDecision(registry, context, config, JSON.parse, undefined, () => profiles, () => { started++; return () => { stopped++; }; });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(started, 0);
  release({ simple: { thinkingLevel: 'low' } });
  assert.deepEqual(await requested, { action: 'no_change' });
  assert.equal(stopped, 1);
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
  const decision = await requestHelperDecision(registry, context, config, JSON.parse, controller.signal, async () => ({ simple: { thinkingLevel: "low" } }));
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
  await requestHelperDecision(registry, context, { ...config, model: "helpers/moderate" }, JSON.parse);
  assert.equal(reasoning, "off");
  await writeFile(join(directory, "helper-models.json"), "not JSON");
  await assert.rejects(requestHelperDecision(registry, context, { ...config, model: "helpers/moderate" }, JSON.parse), /helper-models.json/);
});

test("physical model IDs retain slashes and do not read helper profiles", async () => {
  let captured;
  const registry = {
    find: (provider, id) => { assert.equal(provider, "openrouter"); assert.equal(id, "vendor/model"); return { provider, id }; },
    streamSimple: (_model, _context, opts) => { captured = opts; return { result: async () => response('{"action":"no_change"}') }; },
  };
  await requestHelperDecision(registry, context, { ...config, model: "openrouter/vendor/model" }, JSON.parse, undefined,
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
    await requestHelperDecision(registry, context, { ...config, model, thinking_level }, JSON.parse, undefined,
      async () => { throw new Error("explicit overrides must not read helper profiles"); });
    assert.equal(captured, expected);
  }
});

test("missing alias/profile and invalid or unsuccessful model output fail actionably", async () => {
  const profiles = async () => ({ simple: { thinkingLevel: "off" } });
  const registry = { find: () => undefined, streamSimple: () => { throw new Error("not called"); } };
  await assert.rejects(requestHelperDecision(registry, context, config, JSON.parse, undefined, profiles), /helpers\/simple.*registered|not found/i);
  registry.find = () => ({ api: "pi-virtual" });
  await assert.rejects(requestHelperDecision(registry, context, config, JSON.parse, undefined, async () => ({})), /helper-models.json.*thinkingLevel/i);
  for (const result of [response('{"action":"no_change"}', "length"), response('{"action":"no_change"}', "error"), response('not JSON')]) {
    registry.streamSimple = () => ({ result: async () => result });
    await assert.rejects(requestHelperDecision(registry, context, config, JSON.parse, undefined, profiles));
  }
});

test('provider error reports the configured helper and diagnostic without parsing or leaking content', async () => {
  let parsed = false, stopped = 0;
  const registry = {
    find: () => ({ api: 'pi-virtual' }),
    streamSimple: () => ({ result: async () => ({ ...response('PRIVATE RESPONSE CONTENT', 'error'), errorMessage: '401 Unauthorized: sign in to the provider again' }) }),
  };
  await assert.rejects(requestHelperDecision(registry, context, config, () => { parsed = true; }, undefined,
    async () => ({ simple: { thinkingLevel: 'off' } }), () => () => { stopped++; }), error => {
    assert.match(error.message, /complete text decision \(error\)/);
    assert.match(error.message, /helpers\/simple/);
    assert.match(error.message, /401 Unauthorized: sign in to the provider again/);
    assert.doesNotMatch(error.message, /PRIVATE RESPONSE CONTENT/);
    return true;
  });
  assert.equal(parsed, false);
  assert.equal(stopped, 1);
});

test('provider errors without usable diagnostics offer provider configuration guidance without guessing auth', async () => {
  for (const errorMessage of [undefined, '', ' \r\n\u0000 ']) {
    const registry = {
      find: () => ({ api: 'physical' }),
      streamSimple: () => ({ result: async () => ({ ...response('PRIVATE CONTENT', 'error'), errorMessage }) }),
    };
    await assert.rejects(requestHelperDecision(registry, context, physicalConfig, JSON.parse), error => {
      assert.match(error.message, /provider\/model/);
      assert.match(error.message, /provider.*configuration/i);
      assert.doesNotMatch(error.message, /401|unauthorized|PRIVATE CONTENT/i);
      return true;
    });
  }
});

test('provider diagnostic is bounded, terminal-safe and redacts bearer credentials', async () => {
  const registry = {
    find: () => ({ api: 'physical' }),
    streamSimple: () => ({ result: async () => ({ ...response('PRIVATE CONTENT', 'error'),
      errorMessage: '401\r\nAuthorization: Bearer fake-provider-secret\u001b[31m\u0000\u0008\u009b\u202e' + 'x'.repeat(2000) }) }),
  };
  await assert.rejects(requestHelperDecision(registry, context, physicalConfig, JSON.parse), error => {
    assert.match(error.message, /401/);
    assert.doesNotMatch(error.message, /fake-provider-secret|PRIVATE CONTENT|\[31m/);
    assert.doesNotMatch(error.message, /[\u0000-\u001f\u007f-\u009f\u202e]/);
    assert.ok(error.message.length < 700);
    return true;
  });
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
  const pending = requestHelperDecision(registry, context, { ...physicalConfig, request_timeout_ms: 10 }, JSON.parse);
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
  const pending = requestHelperDecision(registry, context, physicalConfig, JSON.parse, controller.signal);
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
  for (const lateFailure of [false, true, 'error-result']) {
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
    const pending = requestHelperDecision(registry, context, physicalConfig, JSON.parse, controller.signal);
    const reason = new Error("cancel before late result");
    controller.abort(reason);
    await assert.rejects(within(pending), (error) => error === reason);
    if (lateFailure === true) rejectProvider(new Error("late provider rejection"));
    else if (lateFailure === 'error-result') resolveProvider({ ...response('PRIVATE CONTENT', 'error'), errorMessage: 'late provider error' });
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
    const pending = requestHelperDecision(registry, context, physicalConfig, JSON.parse);
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
  await assert.rejects(requestHelperDecision(registry, context, config, JSON.parse, controller.signal,
    async () => ({ simple: { thinkingLevel: "off" } })), /abort/i);
});
