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
const { BorderedLoader, createAgentSession, DefaultResourceLoader, initTheme, ModelRuntime, SessionManager, SettingsManager } = await import(pathToFileURL(join(piDist, "index.js")).href);
initTheme("dark");
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
	registerCheckpoint({ registerCommand: (name, options) => { commands.push({ name, options }); }, registerShortcut() {} });
	assert.equal(commands.length, 1);
	assert.equal(commands[0].name, "checkpoint");
	assert.equal(typeof commands[0].options.handler, "function");
});

test("registers a configurable shortcut for returning to an open checkpoint", () => {
	const dir = mkdtempSync(join(tmpdir(), "checkpoint-shortcut-"));
	try {
		const shortcuts = [];
		registerCheckpoint({ registerCommand() {}, registerShortcut: (key, options) => shortcuts.push({ key, options }) }, { agentDir: dir });
		assert.equal(shortcuts[0].key, "alt+p");
		writeFileSync(join(dir, "checkpoint.json"), '{"focusShortcut":"ctrl+alt+c"}');
		shortcuts.length = 0;
		registerCheckpoint({ registerCommand() {}, registerShortcut: (key, options) => shortcuts.push({ key, options }) }, { agentDir: dir });
		assert.equal(shortcuts[0].key, "ctrl+alt+c");
	} finally { rmSync(dir, { recursive: true, force: true }); }
});

test("LLM recommendations are advisory and grounded in the full current branch and list", async () => {
	const manager = SessionManager.inMemory(process.cwd());
	manager.appendMessage({ role: "user", content: [{ type: "text", text: "Working on parser" }], timestamp: Date.now() });
	const records = [{ slug: "parser", title: "Parser", status: "active", state: "In progress", path: "/scratch/parser.md" }];
	let seen;
	let chosenModel;
	const selector = { id: "selector" };
	const suggestions = await recommendStreams({
		model: { id: "fake" }, sessionManager: manager,
		modelRegistry: { complete: async (model, context) => {
			chosenModel = model;
			seen = context;
			return { content: [{ type: "text", text: '{"existing":[{"slug":"parser","reason":"Matches parser work"}]}' }] };
		} },
	}, records, selector);
	assert.equal(chosenModel, selector);
	assert.deepEqual(suggestions.existing, [{ slug: "parser", reason: "Matches parser work" }]);
	assert.match(JSON.stringify(seen.messages), /Working on parser/);
	assert.match(JSON.stringify(seen.messages), /parser/);
	assert.match(seen.systemPrompt, /aim for 6 words and around 40 characters/);
	assert.match(seen.systemPrompt, /guidance, not a hard limit/);
	assert.match(seen.systemPrompt, /description and state/);
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
		const initialStatuses = [];
		const panelWidths = [];
		const statuses = [];
		let toggleFocus;
		let activePanel;
		let panelReady;
		const newStream = { ...stream, slug: "new", title: "New", path: join(dir, "new.md") };
		const selectorModel = { ...model, id: "selector" };
		const ctx = {
			mode: "tui", hasUI: true, cwd: dir, model, thinkingLevel: "off", sessionManager: manager,
			modelRegistry: { getRegisteredProviderConfig: () => undefined, getRegisteredNativeProvider: () => undefined,
				find: (_provider, id) => id === "selector" ? selectorModel : undefined,
				complete: async (selected) => {
				events.push(`recommendation-model:${selected.id}`);
				events.push("recommendation-start");
				await new Promise((resolve) => setImmediate(resolve));
				events.push("recommendation-finished");
				return { content: [{ type: "text", text: '{"existing":[]}' }] };
			} },
			getSystemPrompt: () => "Main instructions",
			ui: {
				notify: (message) => notifications.push(message),
				setStatus: (_key, value) => statuses.push(value),
				theme: { fg: (_color, text) => text },
				confirm: async () => { events.push("confirm"); return true; },
				custom: async (factory, options) => new Promise((done) => {
					let panel;
					const finish = (value) => { panel?.dispose?.(); done(value); };
					panel = factory({ terminal: { rows: 30 }, requestRender() {} }, { fg: (_color, text) => text }, {}, finish);
					if (panel instanceof BorderedLoader) {
						events.push("recommendation-loading");
					} else if (panel instanceof StreamChooser) {
						assert.equal(options.overlayOptions.anchor, "center");
						assert.equal(options.overlayOptions.width, "78%");
						events.push("chooser");
						setImmediate(() => { for (const key of "New") panel.handleInput(key); panel.handleInput("\x0e"); setImmediate(() => panel.handleInput("y")); });
					} else {
						activePanel = panel;
						initialStatuses.push(panel.state.status);
						panelWidths.push(options.overlayOptions.width);
						options?.onHandle?.({ focus: () => events.push("focus"), unfocus: () => events.push("unfocus") });
						if (action === "focus") panelReady();
						else setImmediate(() => panel.handleInput(action === "apply" ? "\x07" : "\x1b"));
					}
				}),
			},
		};
		let command;
		registerCheckpoint({ registerCommand: (_name, options) => { command = options.handler; }, registerShortcut: (_key, options) => { toggleFocus = options.handler; }, appendEntry: (type, data) => { events.push("bind"); manager.appendCustomEntry(type, data); }, sendMessage: (message) => receipts.push(message) }, {
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
		assert.match(initialStatuses[0], /Starting checkpoint/);
		assert.equal(panelWidths[0], "64%");
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
		activePanel.handleInput("\x1bp");
		await toggleFocus(ctx);
		assert.deepEqual(events.slice(-2), ["unfocus", "focus"]);
		assert.ok(statuses.some((value) => /Checkpoint open/.test(value)));
		activePanel.handleInput("\x1b");
		await openRun;
		assert.equal(statuses.at(-1), undefined);
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
		writeFileSync(join(dir, "checkpoint.json"), JSON.stringify({ cmux: true, selectorModel: { provider: model.provider, id: "selector" } }));
		action = "create";
		await command("new", ctx);
		assert.ok(events.indexOf("confirm") < events.indexOf("chooser"));
		assert.ok(events.includes("recommendation-model:selector"));
		assert.ok(events.includes("recommendation-loading"));
		assert.ok(events.indexOf("recommendation-loading") < events.indexOf("recommendation-finished"));
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

test("a fresh checkpoint starts a handoff draft conversation with the streams skill, but resume does not restart it", async () => {
	const dir = mkdtempSync(join(tmpdir(), "checkpoint-opening-"));
	try {
		const path = join(dir, "stream.md");
		writeFileSync(path, "scratch unchanged");
		const stream = { slug: "stream", title: "Stream", status: "active", state: "Work", path };
		const manager = SessionManager.inMemory(dir);
		manager.appendCustomEntry("stream-checkpoint-binding", { ownerId: manager.getSessionId(), slug: stream.slug, path });
		const runtime = await ModelRuntime.create({ authPath: join(dir, "auth.json"), modelsPath: null, refreshOnCreate: false });
		const model = runtime.getModels("openai")[0];
		const drafter = runtime.getModels("openai").find((candidate) => candidate.id !== model.id);
		assert.ok(drafter);
		writeFileSync(join(dir, "checkpoint.json"), JSON.stringify({ drafterModel: { provider: drafter.provider, id: drafter.id } }));
		await runtime.setRuntimeApiKey("openai", "test-only");
		const contexts = [];
		const selectedIds = [];
		runtime.streamSimple = (selected, context) => {
			selectedIds.push(selected.id);
			contexts.push(context);
			const events = createAssistantMessageEventStream();
			queueMicrotask(() => {
				events.push({ type: "done", reason: "stop", message: { role: "assistant", content: [{ type: "text", text: "Handoff draft ready for review" }], api: selected.api, provider: selected.provider, model: selected.id, usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } }, stopReason: "stop", timestamp: Date.now() } });
				events.end();
			});
			return events;
		};
		let opens = 0;
		const ctx = { mode: "tui", hasUI: true, cwd: dir, model, thinkingLevel: "off", sessionManager: manager,
			modelRegistry: { getRegisteredProviderConfig: () => undefined, getRegisteredNativeProvider: () => undefined,
				find: (provider, id) => runtime.getModels(provider).find((candidate) => candidate.id === id) }, 
			getSystemPrompt: () => "Main instructions",
			ui: { notify: (message) => { throw new Error(message); }, setStatus() {}, theme: { fg: (_color, text) => text }, custom: (factory) => new Promise((done, reject) => {
				const panel = factory({ terminal: { rows: 30 }, requestRender() {} }, { fg: (_color, text) => text }, {}, done);
				if (!(panel instanceof CheckpointPanel)) return reject(new Error("Expected checkpoint panel"));
				if (opens++ > 0) { setImmediate(() => panel.handleInput("\x1b")); return; }
				void (async () => {
					for (let attempt = 0; attempt < 100 && !panel.state.messages.some((line) => line.includes("Handoff draft ready")); attempt++) await new Promise((resolve) => setImmediate(resolve));
					if (!panel.state.messages.some((line) => line.includes("Handoff draft ready"))) throw new Error(`Opening response missing: ${panel.state.status}`);
					panel.handleInput("\x1b");
				})().catch(reject);
			}) },
		};
		let command;
		registerCheckpoint({ registerCommand: (_name, options) => { command = options.handler; }, registerShortcut() {}, appendEntry() {} }, {
			agentDir: dir, runtime, skill: "STREAMS_SKILL_SENTINEL", cli: { list: async () => [stream], format: async () => "FORMAT_SENTINEL" },
		});
		await command("", ctx);
		assert.equal(contexts.length, 1);
		assert.deepEqual(selectedIds, [drafter.id]);
		assert.match(JSON.stringify(contexts[0]), /STREAMS_SKILL_SENTINEL/);
		assert.match(JSON.stringify(contexts[0]), /FORMAT_SENTINEL/);
		assert.match(contexts[0].messages.at(-1).content[0].text, /draft a handoff/i);
		assert.match(contexts[0].messages.at(-1).content[0].text, /streams skill/i);
		assert.equal(readFileSync(path, "utf8"), "scratch unchanged");
		await command("", ctx);
		assert.equal(contexts.length, 1);
		writeFileSync(join(dir, "checkpoint.json"), '{"drafterModel":{"provider":"openai","id":"missing-model"}}');
		await assert.rejects(command("", ctx), /drafter model openai\/missing-model is unavailable/);
		assert.equal(contexts.length, 1);
	} finally { rmSync(dir, { recursive: true, force: true }); }
});

test("draft diff and annotation quote the reviewed revision", () => {
	const lines = draftDiff("before\nshared\n", "after\nshared\n");
	assert.deepEqual(lines, ["- before", "+ after", "  shared"]);
	assert.equal(annotationText(3, lines[1]), 'Regarding draft 3, "+ after": ');
});

test("chat-first panel wraps the complete streamed response and shows the draft summary", () => {
	const answer = Array.from({ length: 24 }, () => "evidence").join(" ");
	const state = { slug: "stream", title: "Stream", messages: [`Checkpoint: ${answer}`], draft: { revision: 2, summary: "Update progress", path: "/scratch/stream.md", slug: "stream", baseContent: "old", proposedContent: "new" }, status: "Reading source…" };
	const panel = new CheckpointPanel({ terminal: { rows: 40 }, requestRender() {} }, { fg: (_color, text) => text }, state, { send: async () => {}, apply: async () => {}, refresh: async () => {}, close: async () => {} }, () => {});
	const view = panel.render(56).join("\n");
	assert.equal((view.match(/evidence/g) ?? []).length, 24);
	assert.match(view, /Update progress/);
	assert.doesNotMatch(view, /\+ new/);
	assert.match(view, /Reading source/);
	assert.match(view, /Ctrl\+G Apply/);
	assert.match(view, /Alt\+C Clear/);
	assert.ok(panel.render(56).length <= 36);
	panel.handleInput("\x16");
	assert.doesNotMatch(panel.render(56).join("\n"), /\+ new/);
	panel.handleInput("\x1br");
	assert.match(panel.render(56).join("\n"), /\+ new/);
	panel.handleInput("\x1br");
	assert.match(panel.render(56).join("\n"), /evidence/);
});

test("review keeps the composer and quotes the selected draft revision", () => {
	const tui = { terminal: { rows: 32 }, requestRender() {} };
	const theme = { fg: (_color, text) => text };
	const state = { slug: "stream", title: "Stream", messages: ["Earlier question", "Answer"], draft: { revision: 2, path: "/scratch/stream.md", slug: "stream", baseContent: "old\n", proposedContent: "new\n" }, status: "Ready" };
	const panel = new CheckpointPanel(tui, theme, state, { send: async () => {}, apply: async () => {}, refresh: async () => {}, close: async () => {} }, () => {});
	const rendered = panel.render(56).join("\n");
	assert.match(rendered, /Earlier question/);
	panel.handleInput("\x1br");
	assert.match(panel.render(56).join("\n"), /\+ new/);
	panel.handleInput("\x1b[B");
	panel.handleInput("\x1bk");
	assert.match(panel.getInput(), /Regarding draft 2/);
});

test("review keeps a selected long diff line visible and annotates its logical line", () => {
	const base = "old ".repeat(220);
	const proposed = "new ".repeat(220) + "TAIL_MARKER";
	const panel = new CheckpointPanel({ terminal: { rows: 24 }, requestRender() {} }, { fg: (_color, text) => text }, {
		slug: "stream", title: "Stream", messages: [], status: "Ready",
		draft: { revision: 4, path: "/scratch/stream.md", slug: "stream", baseContent: base, proposedContent: proposed },
	}, { send: async () => {}, apply: async () => {}, refresh: async () => {}, close: async () => {} }, () => {});
	panel.handleInput("\x1br");
	panel.handleInput("\x1b[B");
	assert.match(panel.render(48).join("\n"), /▶ .*\+ new/);
	assert.doesNotMatch(panel.render(48).join("\n"), /TAIL_MARKER/);
	panel.handleInput("\x1b[6~");
	panel.handleInput("\x1b[6~");
	assert.match(panel.render(48).join("\n"), /TAIL_MARKER/);
	panel.handleInput("\x1bk");
	assert.match(panel.getInput(), /Regarding draft 4, "\+ new /);
});

test("review styling refreshes when Pi changes theme", () => {
	const panel = new CheckpointPanel({ terminal: { rows: 24 }, requestRender() {} }, { fg: (_color, text) => text }, {
		slug: "stream", title: "Stream", messages: [], status: "Ready",
		draft: { revision: 1, path: "/scratch/stream.md", slug: "stream", baseContent: "old", proposedContent: "new" },
	}, { send: async () => {}, apply: async () => {}, refresh: async () => {}, close: async () => {} }, () => {});
	panel.handleInput("\x1br");
	const dark = panel.render(60).join("\n");
	try {
		initTheme("light");
		panel.invalidate();
		assert.notEqual(panel.render(60).join("\n"), dark);
	} finally { initTheme("dark"); }
});

test("without a draft the chat uses the available conversation space", () => {
	const messages = Array.from({ length: 20 }, (_, index) => `Message ${index}`);
	const panel = new CheckpointPanel({ terminal: { rows: 32 }, requestRender() {} }, { fg: (_color, text) => text }, { slug: "stream", title: "Stream", messages, status: "Ready" }, {
		send: async () => {}, apply: async () => {}, refresh: async () => {}, close: async () => {},
	}, () => {});
	assert.match(panel.render(60).join("\n"), /Message 19/);
	panel.handleInput("\t");
	panel.handleInput("\x1b[5~");
	const before = panel.render(60).join("\n");
	assert.match(before, /Message 1[0-6]/);
	messages.push("Message 20");
	assert.equal(panel.render(60).join("\n"), before);
});

test("panel keeps its input visible on a short terminal", () => {
	const panel = new CheckpointPanel({ terminal: { rows: 12 }, requestRender() {} }, { fg: (_color, text) => text }, {
		slug: "stream", title: "Stream", messages: ["You: hello"], status: "Ready",
		draft: { revision: 1, path: "/scratch/stream.md", slug: "stream", baseContent: "old", proposedContent: "new" },
	}, { send: async () => {}, apply: async () => {}, refresh: async () => {}, close: async () => {} }, () => {});
	panel.setInput("Type a message here");
	const lines = panel.render(60);
	assert.ok(lines.length <= 12);
	assert.match(lines.join("\n"), /Type a message here/);
	panel.setInput("earlier\n".repeat(20) + "latest input");
	const crowded = panel.render(60);
	assert.ok(crowded.length <= 12);
	assert.match(crowded.join("\n"), /latest input/);
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
	panel.handleInput("\r");
	assert.deepEqual(changes.slice(0, 2), ["h", "hi"]);
	assert.ok(changes.slice(2).length > 0 && changes.slice(2).every((value) => value === ""));
});

test("composer remains editable during a response; Escape aborts before closing", async () => {
	let stopCount = 0;
	let closeCount = 0;
	const panel = new CheckpointPanel({ terminal: { rows: 24 }, requestRender() {} }, { fg: (_color, text) => text }, { slug: "stream", title: "Stream", messages: [], status: "Reading…" }, {
		send: async () => new Promise(() => {}), apply: async () => {}, refresh: async () => {}, close: async () => { closeCount++; }, stop: async () => { stopCount++; },
	}, () => {});
	panel.handleInput("question");
	panel.handleInput("\r");
	panel.handleInput("next");
	panel.handleInput("\r");
	assert.equal(panel.getInput(), "next");
	assert.match(panel.render(58).join("\n"), /Reading/);
	panel.handleInput("\x1b");
	await new Promise((resolve) => setImmediate(resolve));
	assert.equal(stopCount, 1);
	assert.equal(closeCount, 0);
});

test("the panel animates its status while starting or running and stops when idle", async () => {
	let renders = 0;
	let finish;
	const state = { slug: "stream", title: "Stream", messages: [], status: "Starting checkpoint…" };
	const panel = new CheckpointPanel({ terminal: { rows: 24 }, requestRender() { renders++; } }, { fg: (_color, text) => text }, state, {
		send: async () => {}, apply: () => new Promise((resolve) => { finish = resolve; }), refresh: async () => {}, close: async () => {},
	}, () => {});
	try {
		const first = panel.render(60).join("\n");
		assert.match(first, /⠋.*Starting checkpoint/);
		await new Promise((resolve) => setTimeout(resolve, 100));
		assert.ok(renders > 0);
		assert.notEqual(panel.render(60).join("\n"), first);
		state.status = "Ready";
		assert.doesNotMatch(panel.render(60).join("\n"), /[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏].*Ready/);
		panel.handleInput("\x07");
		assert.match(panel.render(60).join("\n"), /[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏].*Applying/);
		finish();
		await new Promise((resolve) => setImmediate(resolve));
		assert.doesNotMatch(panel.render(60).join("\n"), /[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏].*Applying/);
	} finally { panel.dispose(); }
});

test("Alt+C asks before clearing pending checkpoint work", () => {
	const panel = new CheckpointPanel({ terminal: { rows: 24 }, requestRender() {} }, { fg: (_color, text) => text }, { slug: "stream", title: "Stream", messages: [], status: "Ready" }, {
		send: async () => {}, apply: async () => {}, refresh: async () => {}, close: async () => {},
	}, () => {});
	panel.handleInput("\x1bc");
	assert.match(panel.render(60).join("\n"), /Discard pending chat and draft/);
});

test("Apply has its own activity state and Escape cannot close during a write", async () => {
	let complete;
	let closed = false;
	let stopped = false;
	const panel = new CheckpointPanel({ terminal: { rows: 24 }, requestRender() {} }, { fg: (_color, text) => text }, {
		slug: "stream", title: "Stream", messages: [], status: "Ready",
	}, { send: async () => {}, apply: () => new Promise((resolve) => { complete = resolve; }), refresh: async () => {}, close: async () => { closed = true; }, stop: async () => { stopped = true; } }, () => {});
	panel.handleInput("\x07");
	assert.match(panel.render(70).join("\n"), /Applying/);
	panel.handleInput("\x1bc");
	assert.doesNotMatch(panel.render(70).join("\n"), /Discard pending/);
	panel.handleInput("\x1b");
	assert.equal(closed, false);
	assert.equal(stopped, false);
	complete();
	await new Promise((resolve) => setImmediate(resolve));
});

test("Escape stops a response, then a second Escape closes without applying", async () => {
	const tui = { terminal: { rows: 24 }, requestRender() {} };
	const theme = { fg: (_color, text) => text };
	const events = [];
	let finish;
	const panel = new CheckpointPanel(tui, theme, { slug: "stream", title: "Stream", messages: [], status: "Ready" }, {
		send: () => new Promise((resolve) => { finish = resolve; }),
		stop: async () => { events.push("stop"); finish(); },
		apply: async () => { throw new Error("should not apply"); },
		refresh: async () => {},
		close: async (clear) => { events.push(clear); },
	}, () => { events.push("done"); });
	panel.handleInput("question");
	panel.handleInput("\r");
	panel.handleInput("\x1b");
	await new Promise((resolve) => setImmediate(resolve));
	assert.deepEqual(events, ["stop"]);
	panel.handleInput("\x1b");
	await new Promise((resolve) => setImmediate(resolve));
	assert.deepEqual(events, ["stop", false, "done"]);
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
	const lines = chooser.render(80);
	const view = lines.join("\n");
	assert.ok(view.indexOf("Suggested") < view.indexOf("Other"));
	const suggested = lines.findIndex((line) => /★ Suggested ·/.test(line));
	assert.match(lines[suggested + 1], /Related work/);
	assert.doesNotMatch(lines[suggested], /Related work/);
	const proposed = lines.findIndex((line) => /▶ ★ Suggested new: New Stream/.test(line));
	assert.match(lines[proposed + 1], /Different scope/);
	chooser.handleInput("\x1b[B");
	assert.match(chooser.render(80).join("\n"), /▶ ★ Suggested ·/);
	assert.equal(selected, undefined);
});

test("Enter selects an existing stream and Alt+M reveals streams beyond the initial 20", () => {
	const records = Array.from({ length: 21 }, (_, index) => ({ slug: `stream-${index}`, title: `Stream ${String(index).padStart(2, "0")}`, status: "active", state: "Work", path: `/scratch/${index}.md` }));
	let chosen;
	const chooser = new StreamChooser({ terminal: { rows: 30 }, requestRender() {} }, { fg: (_color, text) => text }, records, { list: async () => records, show: async () => records[0], start: async () => records[0], previewStart: async () => records[0] }, (choice) => { chosen = choice; });
	chooser.handleInput("\r");
	assert.equal(chosen?.stream.slug, "stream-0");
	chosen = undefined;
	chooser.handleInput("\x1bm");
	for (let index = 0; index < 20; index++) chooser.handleInput("\x1b[B");
	chooser.handleInput("\t");
	assert.equal(chosen?.stream.slug, "stream-20");
});

test("highlighting the suggested new stream previews it before asking to create", async () => {
	const proposed = { slug: "new-stream", title: "New Stream", status: "active", state: "Started", path: "/scratch/new.md", cwd: "/repo" };
	const existing = { slug: "old-stream", title: "Old Stream", status: "active", state: "Work", path: "/scratch/old.md" };
	const calls = [];
	let chosen;
	const chooser = new StreamChooser({ terminal: { rows: 30 }, requestRender() {} }, { fg: (_color, text) => text }, [existing], {
		previewStart: async (title) => { calls.push(`preview:${title}`); return proposed; },
		start: async (title) => { calls.push(`start:${title}`); return proposed; },
		list: async () => [existing], show: async () => existing,
	}, (choice) => { chosen = choice; }, { existing: [], newStream: { title: "New Stream", reason: "Different scope" } });
	const view = chooser.render(80).join("\n");
	assert.match(view, /▶ ★ Suggested new: New Stream/);
	assert.match(view, /Different scope/);
	chooser.handleInput("\r");
	await new Promise((resolve) => setImmediate(resolve));
	assert.deepEqual(calls, ["preview:New Stream"]);
	assert.equal(chosen, undefined);
	assert.match(chooser.render(80).join("\n"), /Create New Stream/);
	chooser.handleInput("n");
	assert.equal(chosen, undefined);
	chooser.handleInput("\r");
	await new Promise((resolve) => setImmediate(resolve));
	chooser.handleInput("y");
	await new Promise((resolve) => setImmediate(resolve));
	assert.deepEqual(calls, ["preview:New Stream", "preview:New Stream", "start:New Stream"]);
	assert.deepEqual(chosen, { stream: proposed, created: true });
});

test("a long suggested title remains visible and selectable in full", async () => {
	const title = "Investigate persistent session checkpoint behavior across multiple independent Pi side conversations";
	const proposed = { slug: "persistent-session", title, status: "active", state: "Started", path: "/scratch/session.md", cwd: "/repo" };
	const calls = [];
	let chosen;
	const chooser = new StreamChooser({ terminal: { rows: 36 }, requestRender() {} }, { fg: (_color, text) => text }, [], {
		previewStart: async (value) => { calls.push(`preview:${value}`); return proposed; },
		start: async (value) => { calls.push(`start:${value}`); return proposed; },
		list: async () => [], show: async () => proposed,
	}, (choice) => { chosen = choice; }, { existing: [], newStream: { title, reason: "No existing stream fits" } });
	const view = chooser.render(60);
	const titleRows = view.slice(2, view.findIndex((line) => line.includes("↳")));
	const displayedTitle = titleRows.map((line) => line.slice(1, -1).trim()).join(" ");
	assert.match(displayedTitle, /▶ ★ Suggested new:/);
	assert.ok(displayedTitle.includes(title));
	assert.match(view.join("\n"), /No existing stream fits/);
	chooser.handleInput("\r");
	await new Promise((resolve) => setImmediate(resolve));
	assert.deepEqual(calls, [`preview:${title}`]);
	const preview = chooser.render(60);
	const promptStart = preview.findIndex((line) => line.includes("Create "));
	const promptEnd = preview.findIndex((line, index) => index >= promptStart && line.includes("? y/n"));
	assert.ok(preview.slice(promptStart, promptEnd + 1).map((line) => line.slice(1, -1).trim()).join(" ").includes(title));
	chooser.handleInput("y");
	await new Promise((resolve) => setImmediate(resolve));
	assert.deepEqual(calls, [`preview:${title}`, `start:${title}`]);
	assert.deepEqual(chosen, { stream: proposed, created: true });
});

test("typed stream references resolve deterministically even when matching a new recommendation", async () => {
	const existing = { slug: "new-stream", title: "New Stream", status: "active", state: "Work", path: "/scratch/new.md" };
	let chosen;
	const calls = [];
	const chooser = new StreamChooser({ terminal: { rows: 30 }, requestRender() {} }, { fg: (_color, text) => text }, [existing], {
		show: async (ref) => { calls.push(`show:${ref}`); return existing; },
		previewStart: async (title) => { calls.push(`preview:${title}`); return existing; },
		list: async () => [existing], start: async () => existing,
	}, (choice) => { chosen = choice; }, { existing: [], newStream: { title: "New Stream", reason: "Suggested" } });
	chooser.input.setValue("New Stream");
	chooser.handleInput("\r");
	await new Promise((resolve) => setImmediate(resolve));
	assert.deepEqual(calls, ["show:New Stream"]);
	assert.deepEqual(chosen, { stream: existing, created: false });
});

test("chooser distinguishes suggested titles, stream status, and details with styling and spacing", () => {
	const existing = { slug: "old-stream", title: "Old Stream", status: "active", state: "Work pending", path: "/scratch/old.md" };
	const ansi = { border: 37, accent: 35, success: 32, warning: 33, muted: 90 };
	const theme = { fg: (color, text) => `\x1b[${ansi[color]}m${text}\x1b[0m`, italic: (text) => `\x1b[3m${text}\x1b[23m` };
	const chooser = new StreamChooser({ terminal: { rows: 30 }, requestRender() {} }, theme, [existing], {
		show: async () => existing, previewStart: async () => existing, start: async () => existing, list: async () => [existing],
	}, () => {}, { existing: [], newStream: { title: "Suggested Stream", reason: "Separate work" } });
	const lines = chooser.render(80);
	const output = lines.join("\n");
	assert.match(output, /\x1b\[35m★ Suggested new: Suggested Stream/);
	assert.match(output, /\x1b\[32mactive/);
	assert.match(output, /\x1b\[3mWork pending/);
	const suggestedReason = lines.findIndex((line) => line.includes("Separate work"));
	const existingLine = lines.findIndex((line) => line.includes("Old Stream"));
	assert.match(lines[existingLine - 1].replace(/\x1b\[[0-9;]*m/g, ""), /^│ *│$/);
	assert.ok(suggestedReason < existingLine - 1);
});

test("chooser wraps a long recommendation reason instead of cutting it off", () => {
	const item = { slug: "stream", title: "Stream", status: "active", state: "Work", path: "/scratch/stream.md" };
	const reason = Array.from({ length: 20 }, () => "evidence").join(" ");
	const chooser = new StreamChooser({ terminal: { rows: 40 }, requestRender() {} }, { fg: (_color, text) => text }, [item], { show: async () => item, previewStart: async () => item, start: async () => item, list: async () => [item] }, () => {}, { existing: [{ slug: "stream", reason }] });
	assert.equal((chooser.render(80).join("\n").match(/evidence/g) ?? []).length, 20);
});

test("chooser keeps the selected recommendation visible in a short viewport", () => {
	const streams = Array.from({ length: 12 }, (_, index) => ({ slug: `stream-${index}`, title: `Stream ${index}`, status: "active", state: "Work", path: `/scratch/${index}.md` }));
	const reasons = streams.slice(0, 3).map((stream) => ({ slug: stream.slug, reason: `Reason for ${stream.title}` }));
	const chooser = new StreamChooser({ terminal: { rows: 16 }, requestRender() {} }, { fg: (_color, text) => text }, streams, { show: async () => streams[0], previewStart: async () => streams[0], start: async () => streams[0], list: async () => streams }, () => {}, { existing: reasons });
	for (let index = 0; index < 2; index++) chooser.handleInput("\x1b[B");
	const view = chooser.render(80).join("\n");
	assert.match(view, /▶ ★ Stream 2/);
	assert.ok(chooser.render(80).length <= Math.floor(16 * 0.8));
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

test("context guard uses Pi's content estimate instead of serialized message metadata", () => {
	const message = { role: "user", content: [{ type: "text", text: "x".repeat(1000) }], timestamp: Date.now() };
	assert.doesNotThrow(() => assertContextFits({ contextWindow: 400 }, "", [message]));
	assert.throws(() => assertContextFits({ contextWindow: 100 }, "instructions".repeat(80), []), /No history was dropped/);
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

test("suggested titles are never shortened by recommendation parsing", () => {
	const title = "Investigate persistent session checkpoint behavior across multiple independent Pi side conversations in deeply nested projects";
	const parsed = parseRecommendations(JSON.stringify({ existing: [], newStream: { title, reason: "Different work" } }), []);
	assert.deepEqual(parsed.newStream, { title, reason: "Different work" });
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
		assert.deepEqual(loadConfig(dir), { cmux: false, focusShortcut: "alt+p" });
		writeFileSync(join(dir, "checkpoint.json"), '{"cmux":true,"focusShortcut":"ctrl+alt+c","selectorModel":{"provider":"openai","id":"selector"},"drafterModel":{"provider":"anthropic","id":"drafter"}}');
		assert.deepEqual(loadConfig(dir), { cmux: true, focusShortcut: "ctrl+alt+c", selectorModel: { provider: "openai", id: "selector" }, drafterModel: { provider: "anthropic", id: "drafter" } });
		writeFileSync(join(dir, "checkpoint.json"), '{"cmux":"true"}');
		assert.throws(() => loadConfig(dir), /cmux must be a boolean/);
		writeFileSync(join(dir, "checkpoint.json"), '{"selectorModel":{"provider":"openai"}}');
		assert.throws(() => loadConfig(dir), /selectorModel.*provider.*id/);
		writeFileSync(join(dir, "checkpoint.json"), '{"focusShortcut":""}');
		assert.throws(() => loadConfig(dir), /focusShortcut/);
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
