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
const jiti = createJiti(join(agentDir, "npm", "checkpoint-test.mjs"), { alias: { "@earendil-works/pi-coding-agent": join(piDist, "index.js"), "@earendil-works/pi-tui": join(piDist, "../../../@earendil-works/pi-tui/dist/index.js") } });
const { createAgentSession, DefaultResourceLoader, ModelRuntime, SessionManager, SettingsManager } = await import(pathToFileURL(join(piDist, "index.js")).href);
const { createAssistantMessageEventStream } = await import(pathToFileURL(join(piDist, "../../../@earendil-works/pi-ai/dist/index.js")).href);
const { loadConfig, latestBinding, applyDraft, readPending, savePending, clearPending, stageDraft, selectStream, parseRecommendations, includeMainSession } = await jiti.import(new URL("../home/.pi/agent/extensions/stream-checkpoint/core.ts", import.meta.url).pathname);
const { checkpointContext, completedMessages, assertContextFits, createCheckpointSession } = await jiti.import(new URL("../home/.pi/agent/extensions/stream-checkpoint/agent.ts", import.meta.url).pathname);
const { StreamCli } = await jiti.import(new URL("../home/.pi/agent/extensions/stream-checkpoint/streams.ts", import.meta.url).pathname);
const { draftDiff, annotationText, CheckpointPanel, StreamChooser } = await jiti.import(new URL("../home/.pi/agent/extensions/stream-checkpoint/panel.ts", import.meta.url).pathname);
const { default: registerCheckpoint, recommendStreams } = await jiti.import(new URL("../home/.pi/agent/extensions/stream-checkpoint/index.ts", import.meta.url).pathname);

// Feasibility gate: SDK session must run with a separate context and without
// importing main-session extensions, bash, edit, or write.
test("registers the manual checkpoint command without starting work", () => {
	const commands = [];
	registerCheckpoint({ registerCommand: (name, options) => { commands.push({ name, options }); } });
	assert.equal(commands.length, 1);
	assert.equal(commands[0].name, "checkpoint");
	assert.equal(typeof commands[0].options.handler, "function");
});

test("LLM recommendations are advisory and grounded in the full current branch and list", async () => {
	const manager = SessionManager.inMemory(process.cwd());
	manager.appendMessage({ role: "user", content: [{ type: "text", text: "Working on parser" }], timestamp: Date.now() });
	const records = [{ slug: "parser", title: "Parser", status: "active", state: "In progress", path: "/scratch/parser.md" }];
	let seen;
	const suggestions = await recommendStreams({
		model: { id: "fake" }, sessionManager: manager,
		modelRegistry: { complete: async (_model, context) => {
			seen = context;
			return { content: [{ type: "text", text: '{"existing":[{"slug":"parser","reason":"Matches parser work"}]}' }] };
		} },
	}, records);
	assert.deepEqual(suggestions.existing, [{ slug: "parser", reason: "Matches parser work" }]);
	assert.match(JSON.stringify(seen.messages), /Working on parser/);
	assert.match(JSON.stringify(seen.messages), /parser/);
});

test("/checkpoint binds, preserves closed work, and applies only its reviewed draft", async () => {
	const dir = mkdtempSync(join(tmpdir(), "checkpoint-command-"));
	try {
		const path = join(dir, "stream.md");
		writeFileSync(path, "scratch unchanged");
		const stream = { slug: "stream", title: "Stream", status: "active", state: "Work", path };
		const manager = SessionManager.inMemory(dir);
		const runtime = await ModelRuntime.create({ authPath: join(dir, "auth.json"), modelsPath: null, refreshOnCreate: false });
		const model = runtime.getModels("openai")[0];
		const notifications = [];
		const receipts = [];
		let action = "close";
		let writes = 0;
		let projections = 0;
		const events = [];
		let activePanel;
		let panelReady;
		const newStream = { ...stream, slug: "new", title: "New", path: join(dir, "new.md") };
		const ctx = {
			mode: "tui", hasUI: true, cwd: dir, model, thinkingLevel: "off", sessionManager: manager,
			modelRegistry: { getRegisteredProviderConfig: () => undefined, getRegisteredNativeProvider: () => undefined, complete: async () => ({ content: [{ type: "text", text: '{"existing":[]}' }] }) },
			getSystemPrompt: () => "Main instructions",
			ui: {
				notify: (message) => notifications.push(message),
				confirm: async () => { events.push("confirm"); return true; },
				custom: async (factory, options) => new Promise((done) => {
					const panel = factory({ terminal: { rows: 30 }, requestRender() {} }, { fg: (_color, text) => text }, {}, done);
					if (panel instanceof StreamChooser) {
						events.push("chooser");
						setImmediate(() => { for (const key of "New") panel.handleInput(key); panel.handleInput("\x0e"); setImmediate(() => panel.handleInput("y")); });
					} else {
						activePanel = panel;
						options?.onHandle?.({ focus: () => events.push("focus"), unfocus: () => events.push("unfocus") });
						if (action === "focus") panelReady();
						else setImmediate(() => panel.handleInput(action === "apply" ? "\x07" : "\x1b"));
					}
				}),
			},
		};
		let command;
		registerCheckpoint({ registerCommand: (_name, options) => { command = options.handler; }, appendEntry: (type, data) => { events.push("bind"); manager.appendCustomEntry(type, data); }, sendMessage: (message) => receipts.push(message) }, {
			agentDir: dir, runtime, skill: "Read-only stream instructions", cli: {
				list: async () => [stream], show: async (ref) => { if (ref === "new") throw new Error("Stream not found"); return stream; }, format: async () => "Stream format",
				previewStart: async () => newStream,
				start: async () => { events.push("create"); writeFileSync(newStream.path, "starter note"); return newStream; },
				project: async () => { projections++; },
				update: async (_slug, content, options) => {
					if (!options.dryRun) { writes++; writeFileSync(path, content); }
					return { path, updated: !options.dryRun };
				},
			},
		});
		await command("Stream", ctx);
		assert.equal(latestBinding(manager.getEntries(), manager.getSessionId()).slug, "stream");
		assert.equal(readPending(dir, manager.getSessionId(), "stream").ownerId, manager.getSessionId());
		assert.equal(readFileSync(path, "utf8"), "scratch unchanged");
		action = "focus";
		const ready = new Promise((resolve) => { panelReady = resolve; });
		const openRun = command("Stream", ctx);
		await ready;
		activePanel.handleInput("\x0f");
		await command("Stream", ctx);
		assert.deepEqual(events.slice(-2), ["unfocus", "focus"]);
		activePanel.handleInput("\x1b");
		await openRun;
		const pending = readPending(dir, manager.getSessionId(), "stream");
		stageDraft(pending, { ownerId: manager.getSessionId(), slug: stream.slug, path }, "scratch updated", "new checkpoint");
		savePending(dir, pending);
		writeFileSync(path, "external edit");
		action = "inspect";
		await command("Stream", ctx);
		assert.equal(activePanel.state.draft.baseContent, "external edit");
		assert.match(activePanel.state.status, /Scratch changed/);
		assert.equal(readPending(dir, manager.getSessionId(), "stream").draft.baseContent, "scratch unchanged");
		writeFileSync(path, "scratch unchanged");
		action = "apply";
		await command("Stream", ctx);
		assert.equal(readFileSync(path, "utf8"), "scratch updated");
		assert.equal(writes, 1);
		assert.equal(readPending(dir, manager.getSessionId(), "stream"), undefined);
		assert.match(receipts[0].content, /stream.*stream\.md.*new checkpoint/);
		assert.deepEqual(notifications, []);
		const unfinished = { version: 1, ownerId: manager.getSessionId(), slug: stream.slug, messages: [], input: "" };
		savePending(dir, unfinished);
		writeFileSync(join(dir, "checkpoint.json"), JSON.stringify({ cmux: true }));
		action = "create";
		await command("new", ctx);
		assert.ok(events.indexOf("confirm") < events.indexOf("chooser"));
		assert.ok(events.indexOf("create") < events.lastIndexOf("bind"));
		assert.equal(projections, 1);
		assert.equal(latestBinding(manager.getEntries(), manager.getSessionId()).slug, "new");
		assert.equal(readFileSync(newStream.path, "utf8"), "starter note");
		assert.ok(readPending(dir, manager.getSessionId(), stream.slug));
		assert.deepEqual(notifications.slice(-1), ["Error: Stream not found"]);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});

test("draft diff and annotation quote the reviewed revision", () => {
	const lines = draftDiff("before\nshared\n", "after\nshared\n");
	assert.deepEqual(lines, ["- before", "+ after", "  shared"]);
	assert.equal(annotationText(3, lines[1]), 'Regarding draft 3, "+ after": ');
});

test("panel keeps chat, diff, and shared input visible while quoting an old revision", () => {
	const tui = { terminal: { rows: 32 }, requestRender() {} };
	const theme = { fg: (_color, text) => text };
	const state = { slug: "stream", title: "Stream", messages: ["Earlier question", "Answer"], draft: { revision: 2, path: "/scratch/stream.md", slug: "stream", baseContent: "old\n", proposedContent: "new\n" }, status: "Ready" };
	const panel = new CheckpointPanel(tui, theme, state, { send: async () => {}, apply: async () => {}, refresh: async () => {}, close: async () => {} }, () => {});
	const rendered = panel.render(56).join("\n");
	assert.match(rendered, /Earlier question/);
	assert.match(rendered, /\+ new/);
	assert.match(rendered, /message here/);
	panel.handleInput("\t");
	panel.handleInput("\x1b[B");
	panel.handleInput("\x0b");
	assert.match(panel.getInput(), /Regarding draft 2/);
});

test("without a draft the chat uses the available conversation space", () => {
	const messages = Array.from({ length: 20 }, (_, index) => `Message ${index}`);
	const panel = new CheckpointPanel({ terminal: { rows: 32 }, requestRender() {} }, { fg: (_color, text) => text }, { slug: "stream", title: "Stream", messages, status: "Ready" }, {
		send: async () => {}, apply: async () => {}, refresh: async () => {}, close: async () => {},
	}, () => {});
	assert.match(panel.render(60).join("\n"), /Message 0/);
});

test("panel keeps its input visible on a short terminal", () => {
	const panel = new CheckpointPanel({ terminal: { rows: 12 }, requestRender() {} }, { fg: (_color, text) => text }, {
		slug: "stream", title: "Stream", messages: ["You: hello"], status: "Ready",
		draft: { revision: 1, path: "/scratch/stream.md", slug: "stream", baseContent: "old", proposedContent: "new" },
	}, { send: async () => {}, apply: async () => {}, refresh: async () => {}, close: async () => {} }, () => {});
	const lines = panel.render(60);
	assert.ok(lines.length <= 12);
	assert.match(lines.join("\n"), /ype a message here/);
});

test("panel can release focus to the main editor without closing", () => {
	let transfers = 0;
	let closed = false;
	const panel = new CheckpointPanel({ terminal: { rows: 24 }, requestRender() {} }, { fg: (_color, text) => text }, { slug: "stream", title: "Stream", messages: [], status: "Ready" }, {
		send: async () => {}, apply: async () => {}, refresh: async () => {}, close: async () => { closed = true; },
		focusMain: () => { transfers++; },
	}, () => { closed = true; });
	panel.handleInput("\x0f");
	assert.equal(transfers, 1);
	assert.equal(closed, false);
});

test("panel reports unsent input changes for durable storage", () => {
	const changes = [];
	const panel = new CheckpointPanel({ terminal: { rows: 24 }, requestRender() {} }, { fg: (_color, text) => text }, { slug: "stream", title: "Stream", messages: [], status: "Ready" }, {
		send: async () => {}, apply: async () => {}, refresh: async () => {}, close: async () => {},
		inputChanged: (value) => changes.push(value),
	}, () => {});
	panel.handleInput("h");
	panel.handleInput("i");
	assert.deepEqual(changes, ["h", "hi"]);
});

test("Close can abort an in-flight side prompt without applying a draft", async () => {
	const tui = { terminal: { rows: 24 }, requestRender() {} };
	const theme = { fg: (_color, text) => text };
	const events = [];
	const panel = new CheckpointPanel(tui, theme, { slug: "stream", title: "Stream", messages: [], status: "Ready" }, {
		send: async () => new Promise(() => {}),
		apply: async () => { throw new Error("should not apply"); },
		refresh: async () => {},
		close: async (clear) => { events.push(clear); },
	}, () => { events.push("done"); });
	panel.handleInput("question");
	panel.handleInput("\r");
	panel.handleInput("\x1b");
	await new Promise((resolve) => setImmediate(resolve));
	assert.deepEqual(events, [false, "done"]);
});

test("chooser never creates a stream before explicit confirmation", async () => {
	const tui = { terminal: { rows: 28 }, requestRender() {} };
	const theme = { fg: (_color, text) => text };
	const calls = [];
	let selected;
	const stream = { slug: "new-stream", title: "New Stream", status: "active", state: "Started", path: "/scratch/new-stream.md", cwd: "/repo" };
	const cli = {
		previewStart: async (title) => { calls.push(`preview:${title}`); return stream; },
		start: async (title) => { calls.push(`start:${title}`); return stream; },
		list: async () => [],
		show: async () => { throw new Error("not found"); },
	};
	const chooser = new StreamChooser(tui, theme, [], cli, (choice) => { selected = choice; });
	chooser.input.setValue("New Stream");
	chooser.handleInput("\x0e"); // Ctrl+N: preview creation
	await new Promise((resolve) => setImmediate(resolve));
	assert.deepEqual(calls, ["preview:New Stream"]);
	assert.match(chooser.render(58).join("\n"), /new-stream/);
	chooser.handleInput("y");
	await new Promise((resolve) => setImmediate(resolve));
	assert.deepEqual(calls, ["preview:New Stream", "start:New Stream"]);
	assert.deepEqual(selected, { stream, created: true });
});

test("explicit unresolved references stay editable and ambiguous matches can be selected", () => {
	const tui = { terminal: { rows: 24 }, requestRender() {} };
	const theme = { fg: (_color, text) => text };
	const matches = [
		{ slug: "parser-one", title: "Parser one", status: "active", state: "One", path: "/scratch/one.md" },
		{ slug: "parser-two", title: "Parser two", status: "active", state: "Two", path: "/scratch/two.md" },
	];
	let choice;
	const chooser = new StreamChooser(tui, theme, matches, { show: async () => { throw new Error("ambiguous"); }, list: async () => matches, start: async () => matches[0], previewStart: async () => matches[0] }, (picked) => { choice = picked; }, { existing: [] }, "parser");
	assert.equal(chooser.input.getValue(), "parser");
	chooser.handleInput("\x1b[B");
	chooser.handleInput("\t");
	assert.equal(choice.stream.slug, "parser-two");
});

test("chooser labels recommendations but never treats them as selection", async () => {
	const tui = { terminal: { rows: 24 }, requestRender() {} };
	const theme = { fg: (_color, text) => text };
	const records = [
		{ slug: "other", title: "Other", status: "active", state: "Work", path: "/scratch/other.md" },
		{ slug: "suggested", title: "Suggested", status: "waiting", state: "Review", path: "/scratch/suggested.md" },
	];
	let selected;
	const chooser = new StreamChooser(tui, theme, records, { previewStart: async () => records[1], start: async () => records[1], list: async () => records, show: async () => records[1] }, (choice) => { selected = choice; }, { existing: [{ slug: "suggested", reason: "Related work" }], newStream: { title: "New Stream", reason: "Different scope" } });
	const view = chooser.render(60).join("\n");
	assert.ok(view.indexOf("Suggested") < view.indexOf("Other"));
	assert.match(view, /Related work/);
	assert.match(view, /New Stream/);
	assert.equal(selected, undefined);
});

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
	assert.equal((await cli.previewStart("New stream")).slug, "new-stream");
	assert.equal((await cli.start("New stream")).slug, "new-stream");
	await cli.update("new-stream", "candidate markdown", { dryRun: true, ifMatch: "a".repeat(64) });
	assert.deepEqual(calls, [
		{ args: ["show", "New stream", "--json"], input: undefined },
		{ args: ["start", "New stream", "--dry-run", "--json"], input: undefined },
		{ args: ["start", "New stream", "--json"], input: undefined },
		{ args: ["update", "new-stream", "--dry-run", "--if-match", "a".repeat(64), "--json"], input: "candidate markdown" },
	]);
});

test("CLI integration fails closed on malformed stream lists and update responses", async () => {
	const cli = new StreamCli("/repo", async () => "{} ");
	await assert.rejects(cli.list(), /Invalid streams list/);
	await assert.rejects(new StreamCli("/repo", async () => '[{"slug":"stream","title":"Stream","status":"active","path":"/scratch/stream.md"}]').list(), /Invalid streams list/);
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

test("drafts include the owning Pi session once without changing the body", () => {
	const candidate = "---\ntitle: Stream\nsessions:\n  - pi:earlier\n---\n\n# Body\n";
	const prepared = includeMainSession(candidate, "owner");
	assert.match(prepared, /- pi:earlier\n  - pi:owner/);
	assert.ok(prepared.endsWith("---\n\n# Body\n"));
	assert.equal(includeMainSession(prepared, "owner"), prepared);
	assert.match(includeMainSession("---\ntitle: Stream\nsessions: []\n---\n", "owner"), /sessions: \[ pi:owner \]/);
	assert.throws(() => includeMainSession("---\ntitle: Stream\n---\n", "owner"), /sessions list/);
});

test("proposal staging changes only pending state, never scratch", () => {
	const dir = mkdtempSync(join(tmpdir(), "checkpoint-propose-"));
	try {
		const path = join(dir, "stream.md");
		writeFileSync(path, "original");
		const pending = { version: 1, ownerId: "main", slug: "stream", messages: [], input: "" };
		const draft = stageDraft(pending, { ownerId: "main", slug: "stream", path }, "proposal", "Clarify state");
		assert.deepEqual(draft, { path, slug: "stream", revision: 1, baseContent: "original", proposedContent: "proposal", summary: "Clarify state" });
		assert.equal(readFileSync(path, "utf8"), "original");
		assert.equal(stageDraft(pending, { ownerId: "main", slug: "stream", path }, "revision", "Second").revision, 2);
	} finally {
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
		const proposals = [];
		session = await createCheckpointSession({ cwd: dir, agentDir: dir, modelRuntime: runtime, model: runtime.getModels()[0], mainMessages: main, savedMessages: saved, systemPrompt: "checkpoint instructions", onProposal: async (content, summary) => { proposals.push({ content, summary }); return 1; } });
		assert.deepEqual(session.getActiveToolNames().sort(), ["find", "grep", "ls", "propose_checkpoint", "read"]);
		const tool = session.getToolDefinition("propose_checkpoint");
		await tool.execute("proposal-1", { content: "proposed", summary: "New state" }, undefined, undefined, {});
		assert.deepEqual(proposals, [{ content: "proposed", summary: "New state" }]);
		assert.deepEqual(session.messages.filter((item) => item.role === "user").map((item) => item.content[0].text), ["new main snapshot", "earlier checkpoint question"]);
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

test("oversized context fails visibly instead of silently dropping history", () => {
	assert.throws(() => assertContextFits({ contextWindow: 100 }, "instructions".repeat(80), []), /without dropping context/);
	assert.doesNotThrow(() => assertContextFits({ contextWindow: 100_000 }, "instructions", []));
});

test("interrupted side turns do not seed an unmatched tool call on resume", () => {
	const question = { role: "user", content: [{ type: "text", text: "question" }], timestamp: 1 };
	const interrupted = { role: "assistant", content: [{ type: "toolCall", id: "read-1", name: "read", arguments: {} }], timestamp: 2, stopReason: "aborted" };
	assert.deepEqual(completedMessages([question, interrupted]), [question]);
	assert.deepEqual(completedMessages([question, { ...interrupted, content: [{ type: "text", text: "partial" }] }]), [question]);
	const later = { role: "user", content: [{ type: "text", text: "continue" }], timestamp: 3 };
	assert.deepEqual(completedMessages([question, interrupted, later]), [question, later]);
	assert.deepEqual(completedMessages([question, { ...interrupted, stopReason: "toolUse" }, later]), [question, later]);
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
	assert.equal(checkpointContext(manager).omitted, 1);
});

test("explicit stream choice resolves through show; bound choice stays exact and session-wide", async () => {
	const streams = [
		{ slug: "alpha", title: "Alpha", status: "active", state: "Working", path: "/scratch/alpha.md" },
		{ slug: "beta", title: "Beta", status: "waiting", state: "Review", path: "/scratch/beta.md" },
	];
	const seen = [];
	const cli = { list: async () => streams, show: async (ref) => { seen.push(ref); return streams[1]; } };
	assert.equal((await selectStream("Beta", undefined, cli)).slug, "beta");
	assert.deepEqual(seen, ["Beta"]);
	assert.equal((await selectStream("", { ownerId: "session", slug: "alpha", path: "/scratch/alpha.md" }, cli)).slug, "alpha");
	assert.deepEqual(seen, ["Beta"]);
	assert.equal(await selectStream("", { ownerId: "session", slug: "gone", path: "/scratch/gone.md" }, cli), undefined);
});

test("failed list does not imply absence and cannot trigger creation", async () => {
	await assert.rejects(selectStream("new", undefined, { list: async () => { throw new Error("network failure"); }, show: async () => { throw new Error("not called"); } }), /network failure/);
});

test("recommendations cannot invent existing streams or create anything", () => {
	const streams = [{ slug: "known", title: "Known", status: "active", state: "Work", path: "/scratch/known.md" }];
	assert.deepEqual(parseRecommendations('{"existing":[{"slug":"unknown","reason":"guess"},{"slug":"known","reason":"same work"}],"newStream":{"title":"New idea","reason":"different scope"}}', streams), {
		existing: [{ slug: "known", reason: "same work" }], newStream: { title: "New idea", reason: "different scope" },
	});
	assert.deepEqual(parseRecommendations("not json", streams), { existing: [] });
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
