import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { mkdtempSync, readFileSync, rmSync, realpathSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { tmpdir, homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";

const agentDir = process.env.PI_CODING_AGENT_DIR ?? join(homedir(), ".pi", "agent");
const require = createRequire(join(agentDir, "npm", "package.json"));
const { createJiti } = require("jiti");
const piExecutable = realpathSync(execFileSync("which", ["pi"], { encoding: "utf8" }).trim());
const shimTarget = readFileSync(piExecutable, "utf8").match(/target=([^\s]+)/)?.[1];
assert.ok(shimTarget, "Expected pi package shim");
const piDist = dirname(dirname(resolve(dirname(piExecutable), shimTarget)));
const jiti = createJiti(join(agentDir, "npm", "checkpoint-test.mjs"), { alias: { "@earendil-works/pi-coding-agent": join(piDist, "index.js") } });
const { createAgentSession, DefaultResourceLoader, ModelRuntime, SessionManager, SettingsManager } = await import(pathToFileURL(join(piDist, "index.js")).href);
const { createAssistantMessageEventStream } = await import(pathToFileURL(join(piDist, "../../../@earendil-works/pi-ai/dist/index.js")).href);
const { loadConfig, latestBinding, applyDraft, readPending, savePending, clearPending } = await jiti.import(new URL("../home/.pi/agent/extensions/stream-checkpoint/core.ts", import.meta.url).pathname);
const { checkpointContext, createCheckpointSession } = await jiti.import(new URL("../home/.pi/agent/extensions/stream-checkpoint/agent.ts", import.meta.url).pathname);
const { StreamCli } = await jiti.import(new URL("../home/.pi/agent/extensions/stream-checkpoint/streams.ts", import.meta.url).pathname);

// Feasibility gate: SDK session must run with a separate context and without
// importing main-session extensions, bash, edit, or write.
test("CLI integration uses show and update without shell interpolation", async () => {
	const calls = [];
	const stream = { slug: "new-stream", path: "/scratch/new-stream.md", title: "New stream", status: "active", state: "Started" };
	const cli = new StreamCli("/repo", async (args, input) => {
		calls.push({ args, input });
		if (args[0] === "show" || args[0] === "start") return JSON.stringify(stream);
		if (args[0] === "update") return JSON.stringify({ slug: stream.slug, path: stream.path, updated: !args.includes("--dry-run"), findings: [] });
		return "";
	});
	assert.equal((await cli.show("New stream")).slug, "new-stream");
	assert.equal((await cli.start("New stream")).slug, "new-stream");
	await cli.update("new-stream", "candidate markdown", { dryRun: true, ifMatch: "a".repeat(64) });
	assert.deepEqual(calls, [
		{ args: ["show", "New stream", "--json"], input: undefined },
		{ args: ["start", "New stream", "--json"], input: undefined },
		{ args: ["update", "new-stream", "--dry-run", "--if-match", "a".repeat(64), "--json"], input: "candidate markdown" },
	]);
});

test("CLI integration fails closed on malformed stream lists and update responses", async () => {
	const cli = new StreamCli("/repo", async () => "{} ");
	await assert.rejects(cli.list(), /Invalid streams list/);
	await assert.rejects(cli.update("stream", "candidate", { dryRun: true }), /Invalid streams update response/);
});

test("side session has only the explicit read-only tools", async () => {
	const dir = mkdtempSync(join(tmpdir(), "checkpoint-sdk-"));
	let session;
	try {
		const settings = SettingsManager.create(dir, dir);
		const loader = new DefaultResourceLoader({ cwd: dir, agentDir: dir, settingsManager: settings, noExtensions: true, noSkills: true, noContextFiles: true });
		await loader.reload();
		const runtime = await ModelRuntime.create({ authPath: join(dir, "auth.json"), modelsPath: null, refreshOnCreate: false });
		({ session } = await createAgentSession({
			cwd: dir, agentDir: dir, modelRuntime: runtime, model: runtime.getModels()[0],
			settingsManager: settings, resourceLoader: loader, sessionManager: SessionManager.inMemory(dir),
			tools: ["read", "grep", "find", "ls"],
		}));
		assert.deepEqual(session.getActiveToolNames().sort(), ["find", "grep", "ls", "read"]);
		assert.equal(loader.getExtensions().extensions.length, 0);
	} finally {
		session?.dispose();
		rmSync(dir, { recursive: true, force: true });
	}
});

test("resuming seeds current main context plus saved side history exactly once", async () => {
	const dir = mkdtempSync(join(tmpdir(), "checkpoint-resume-"));
	let session;
	try {
		const runtime = await ModelRuntime.create({ authPath: join(dir, "auth.json"), modelsPath: null, refreshOnCreate: false });
		const message = (text) => ({ role: "user", content: [{ type: "text", text }], timestamp: Date.now() });
		const main = [message("new main snapshot")];
		const saved = [message("earlier checkpoint question")];
		session = await createCheckpointSession({ cwd: dir, agentDir: dir, modelRuntime: runtime, model: runtime.getModels()[0], mainMessages: main, savedMessages: saved, systemPrompt: "checkpoint instructions" });
		assert.deepEqual(session.messages.filter((item) => item.role === "user").map((item) => item.content[0].text), ["new main snapshot", "earlier checkpoint question"]);
		assert.deepEqual(session.getActiveToolNames().sort(), ["find", "grep", "ls", "read"]);
	} finally {
		session?.dispose();
		rmSync(dir, { recursive: true, force: true });
	}
});

test("side session streams through the supplied runtime without steering the main session", async () => {
	const dir = mkdtempSync(join(tmpdir(), "checkpoint-provider-"));
	let session;
	try {
		const runtime = await ModelRuntime.create({ authPath: join(dir, "auth.json"), modelsPath: null, refreshOnCreate: false });
		const model = runtime.getModels("openai")[0];
		assert.ok(model);
		await runtime.setRuntimeApiKey("openai", "test-only");
		let seen;
		runtime.streamSimple = (selected, context) => {
			seen = { selected, context };
			const stream = createAssistantMessageEventStream();
			queueMicrotask(() => {
				const message = { role: "assistant", content: [{ type: "text", text: "side answer" }], api: selected.api, provider: selected.provider, model: selected.id, usage: { input: 1, output: 2, cacheRead: 0, cacheWrite: 0, totalTokens: 3, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } }, stopReason: "stop", timestamp: Date.now() };
				stream.push({ type: "done", reason: "stop", message });
				stream.end();
			});
			return stream;
		};
		session = await createCheckpointSession({ cwd: dir, agentDir: dir, modelRuntime: runtime, model, mainMessages: [], savedMessages: [], systemPrompt: "checkpoint instructions" });
		await session.prompt("A checkpoint question", { expandPromptTemplates: false });
		assert.equal(seen.selected.id, model.id);
		assert.equal(seen.context.messages.at(-1).content[0].text, "A checkpoint question");
		assert.equal(session.messages.at(-1).content[0].text, "side answer");
	} finally {
		session?.dispose();
		rmSync(dir, { recursive: true, force: true });
	}
});

test("checkpoint context uses projected active branch, not abandoned branches", () => {
	const manager = SessionManager.inMemory(process.cwd());
	manager.appendMessage({ role: "user", content: [{ type: "text", text: "first" }], timestamp: Date.now() });
	const fork = manager.getLeafId();
	manager.appendMessage({ role: "user", content: [{ type: "text", text: "abandoned" }], timestamp: Date.now() });
	manager.branch(fork);
	manager.appendMessage({ role: "user", content: [{ type: "text", text: "current" }], timestamp: Date.now() });
	const messages = checkpointContext(manager).messages;
	assert.deepEqual(messages.map((message) => message.content[0].text), ["first", "current"]);
});

test("checkpoint context excludes an unfinished tool-call batch", () => {
	const manager = SessionManager.inMemory(process.cwd());
	manager.appendMessage({ role: "user", content: [{ type: "text", text: "explain" }], timestamp: Date.now() });
	manager.appendMessage({ role: "assistant", content: [{ type: "toolCall", id: "call-1", name: "read", arguments: { path: "file" } }], provider: "test", model: "test", api: "test", stopReason: "toolUse", usage: { input: 0, output: 0 }, timestamp: Date.now() });
	assert.equal(checkpointContext(manager).messages.length, 1);
});

test("configuration enables CMUX only with an explicit boolean true", () => {
	const dir = mkdtempSync(join(tmpdir(), "checkpoint-config-"));
	try {
		assert.deepEqual(loadConfig(dir), { cmux: false });
		writeFileSync(join(dir, "checkpoint.json"), '{"cmux":true}');
		assert.deepEqual(loadConfig(dir), { cmux: true });
		writeFileSync(join(dir, "checkpoint.json"), '{"cmux":"true"}');
		assert.throws(() => loadConfig(dir), /cmux must be a boolean/);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});

test("binding is latest by append order across branches, but cannot cross session IDs", () => {
	const entry = (ownerId, slug) => ({ type: "custom", customType: "stream-checkpoint-binding", data: { ownerId, slug, path: `/scratch/${slug}.md` } });
	const entries = [entry("one", "first"), entry("two", "foreign"), entry("one", "second")];
	assert.equal(latestBinding(entries, "one").slug, "second");
	assert.equal(latestBinding(entries, "two").slug, "foreign");
	assert.equal(latestBinding(entries, "three"), undefined);
});

test("pending chat and draft persist per owner and stream, including unsent input", () => {
	const dir = mkdtempSync(join(tmpdir(), "checkpoint-state-"));
	try {
		const pending = { version: 1, ownerId: "session-one", slug: "stream", messages: [{ role: "user", content: "Why?" }], draft: { revision: 1, path: "/scratch/stream.md", slug: "stream", baseContent: "a", proposedContent: "b" }, input: "unfinished" };
		savePending(dir, pending);
		assert.deepEqual(readPending(dir, "session-one", "stream"), pending);
		assert.equal(readPending(dir, "session-two", "stream"), undefined);
		assert.equal(readPending(dir, "session-one", "another"), undefined);
		clearPending(dir, "session-one", "stream");
		assert.equal(readPending(dir, "session-one", "stream"), undefined);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});

test("Apply asks CLI to preview and write exactly the approved revision with its base hash", async () => {
	const dir = mkdtempSync(join(tmpdir(), "checkpoint-apply-"));
	try {
		const path = join(dir, "stream.md");
		writeFileSync(path, "base");
		const draft = { path, slug: "stream", revision: 3, baseContent: "base", proposedContent: "candidate" };
		const calls = [];
		const deps = {
			update: async (slug, content, options) => {
				calls.push({ slug, content, options });
				if (!options.dryRun) writeFileSync(path, content);
				return { slug, path, updated: !options.dryRun, findings: [] };
			},
			project: async () => {},
		};
		await assert.rejects(applyDraft(draft, 2, deps), /revision/);
		assert.equal(calls.length, 0);
		const result = await applyDraft(draft, 3, deps);
		assert.equal(result.written, true);
		const hash = createHash("sha256").update("base").digest("hex");
		assert.deepEqual(calls, [
			{ slug: "stream", content: "candidate", options: { dryRun: true, ifMatch: hash } },
			{ slug: "stream", content: "candidate", options: { ifMatch: hash } },
		]);
		await assert.rejects(applyDraft(draft, 3, deps), /already applied/);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});

test("Apply refuses changed scratch before invoking update", async () => {
	const dir = mkdtempSync(join(tmpdir(), "checkpoint-stale-"));
	try {
		const path = join(dir, "stream.md");
		writeFileSync(path, "changed elsewhere");
		await assert.rejects(applyDraft({ path, slug: "stream", revision: 1, baseContent: "base", proposedContent: "candidate" }, 1, {
			update: async () => { throw new Error("should not run"); },
		}), /changed since/);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});

test("Apply treats a failed CLI confirmation after a write as partial success", async () => {
	const dir = mkdtempSync(join(tmpdir(), "checkpoint-check-"));
	try {
		const path = join(dir, "stream.md");
		writeFileSync(path, "base");
		const result = await applyDraft({ path, slug: "stream", revision: 1, baseContent: "base", proposedContent: "candidate" }, 1, {
			update: async (_slug, content, options) => {
				if (!options.dryRun) { writeFileSync(path, content); throw new Error("confirmation failed"); }
				return { slug: "stream", path, updated: false, findings: [] };
			},
		});
		assert.equal(result.written, true);
		assert.match(result.validationError, /confirmation failed/);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});

test("Apply records projection failure without repeating the CLI write", async () => {
	const dir = mkdtempSync(join(tmpdir(), "checkpoint-project-"));
	try {
		const path = join(dir, "stream.md");
		writeFileSync(path, "base");
		let writes = 0;
		const draft = { path, slug: "stream", revision: 1, baseContent: "base", proposedContent: "candidate" };
		const deps = {
			update: async (slug, content, options) => {
				if (!options.dryRun) { writes++; writeFileSync(path, content); }
				return { slug, path, updated: !options.dryRun, findings: [] };
			},
			project: async () => { throw new Error("CMUX unavailable"); },
		};
		const result = await applyDraft(draft, 1, deps);
		assert.match(result.projectionWarning, /CMUX unavailable/);
		await assert.rejects(applyDraft(draft, 1, deps), /already applied/);
		assert.equal(writes, 1);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});
