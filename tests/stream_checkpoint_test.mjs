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
const { CheckpointPanel } = await jiti.import(new URL("../home/.pi/agent/extensions/stream-checkpoint/panel.ts", import.meta.url).pathname);
const { StreamChooser } = await jiti.import(new URL("../home/.pi/agent/extensions/stream-checkpoint/chooser.ts", import.meta.url).pathname);
const { default: registerCheckpoint, recommendStreams } = await jiti.import(new URL("../home/.pi/agent/extensions/stream-checkpoint/index.ts", import.meta.url).pathname);

const turn = () => new Promise((resolve) => setImmediate(resolve));
async function until(check) {
	for (let i = 0; i < 200 && !check(); i++) await turn();
	assert.ok(check(), "Expected condition did not settle");
}

function panelFixture(overrides = {}) {
	const state = { messages: [], activity: null, input: "", annotations: [], ...overrides };
	const events = [];
	const actions = {
		send: () => { events.push("send"); }, apply: () => { events.push("apply"); },
		refresh: () => { events.push("refresh"); }, clear: () => { events.push("clear"); },
		stop: () => { events.push("stop"); }, focusMain: () => { events.push("hide"); },
		changed: () => { events.push("changed"); },
	};
	const panel = new CheckpointPanel({ terminal: { rows: 30 }, requestRender() {} }, { fg: (_color, text) => text }, state, actions);
	return { panel, state, events };
}

const sampleDraft = { path: "/scratch/service.md", slug: "service", revision: 2, baseContent: "# Jobs\nold\n", proposedContent: "# Jobs\nnew\n" };

test("review batches inline comments without sending and retains a separate chat composer", () => {
	const { panel, state, events } = panelFixture({ draft: sampleDraft, input: "Overall feedback" });
	panel.handleInput("\t");
	panel.handleInput("\x1b[B");
	panel.handleInput("\x1bk");
	panel.handleInput("Keep this detail");
	panel.handleInput("\r");
	assert.equal(events.includes("send"), false);
	assert.equal(state.annotations.length, 1);
	assert.equal(state.annotations[0].side, "original");
	assert.equal(state.annotations[0].line, 2);
	assert.equal(state.input, "Overall feedback");
	assert.match(panel.render(90).join("\n"), /Keep this detail/);
	panel.handleInput("\x1b[B");
	panel.handleInput("\x1bk");
	panel.handleInput("Explain the new behavior");
	panel.handleInput("\r");
	assert.equal(state.annotations.length, 2);
	panel.handleInput("\t");
	assert.match(panel.render(90).join("\n"), /2 annotations/);
	panel.handleInput("\r");
	assert.equal(events.filter((event) => event === "send").length, 1);
});

test("drafting disables editing, annotations and sends but keeps reading, minimize and stop", () => {
	const { panel, state, events } = panelFixture({ draft: sampleDraft, activity: "drafting", input: "preserved" });
	panel.handleInput("ignored");
	panel.handleInput("\r");
	panel.handleInput("\t");
	panel.handleInput("\x1bk");
	panel.handleInput("ignored again");
	panel.handleInput("\x07");
	panel.handleInput("\x1bc");
	panel.handleInput("\x1b");
	panel.handleInput("\x1bp");
	panel.handleInput("\x03");
	assert.equal(state.input, "preserved");
	assert.deepEqual(state.annotations, []);
	assert.equal(state.annotationEditor, undefined);
	assert.deepEqual(events, ["hide", "hide", "stop"]);
	const view = panel.render(90).join("\n");
	assert.match(view, /Drafting/);
	assert.doesNotMatch(view, /Ready|Alt\+R|Enter Send|Shift\+Enter/);
	panel.dispose();
});

test("header shares the empty composer's divider and keeps its own above content or notices", () => {
	const { panel, state } = panelFixture({ draft: sampleDraft });
	try {
		for (const width of [48, 60, 100]) {
			for (const activity of [null, "starting", "drafting"]) {
				state.activity = activity;
				const lines = panel.render(width);
				assert.match(lines[1], /Streams Checkpoint.*Chat.*Review/);
				assert.match(lines[2], /^[│├]─+[│┤]$/);
				assert.doesNotMatch(lines[3], /^[│├]─+[│┤]$/, "Empty chat must not stack two dividers");
				assert.equal(lines.filter((line) => /Alt\+P/.test(line)).length, 1);
				assert.doesNotMatch(lines.join("\n"), /Ready|Alt\+R|Enter Send/);
			}
		}
		state.activity = null;
		state.messages.push({ role: "assistant", text: "A wrapped answer ".repeat(12) });
		assert.match(panel.render(60)[2], /├─+┤/);
		panel.handleInput("\t");
		assert.match(panel.render(60)[2], /├─+┤/);
		panel.handleInput("\t");
		assert.match(panel.render(60).join("\n"), /wrapped answer/);
		state.messages = [];
		state.notice = { level: "info", text: "Context refreshed" };
		const lines = panel.render(60);
		assert.match(lines[2], /├─+┤/);
		assert.match(lines[3], /Context refreshed/);
		assert.match(lines[4], /^│─+│$/);
	} finally { panel.dispose(); }
});

async function conversationFixture() {
	const dir = mkdtempSync(join(tmpdir(), "checkpoint-conversation-"));
	const path = join(dir, "service.md");
	writeFileSync(path, "scratch unchanged");
	const manager = SessionManager.inMemory(dir);
	const runtime = await ModelRuntime.create({ authPath: join(dir, "auth.json"), modelsPath: null, refreshOnCreate: false });
	const model = runtime.getModels("openai")[0];
	await runtime.setRuntimeApiKey("openai", "test-only");
	const contexts = [];
	let finish;
	runtime.streamSimple = (selected, context, options) => {
		contexts.push(context);
		const events = createAssistantMessageEventStream();
		finish = (text = "Response ready", aborted = false) => {
			const message = { role: "assistant", content: [{ type: "text", text }], api: selected.api, provider: selected.provider, model: selected.id, usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } }, stopReason: aborted ? "aborted" : "stop", timestamp: Date.now() };
			events.push(aborted ? { type: "error", reason: "aborted", error: message } : { type: "done", reason: "stop", message });
			events.end();
		};
		options?.signal?.addEventListener("abort", () => finish("", true), { once: true });
		return events;
	};
	const pending = { version: 1, ownerId: manager.getSessionId(), slug: "service", messages: [], input: "unsent" };
	const { CheckpointConversation } = await jiti.import(new URL("../home/.pi/agent/extensions/stream-checkpoint/conversation.ts", import.meta.url).pathname);
	const conversation = new CheckpointConversation({ pending, binding: { ownerId: pending.ownerId, slug: pending.slug, path }, agentDir: dir, cwd: dir, manager, runtime, model, thinkingLevel: "off", systemPrompt: "Test checkpoint instructions", cli: { update: async (_slug, content, options) => { if (!options.dryRun) writeFileSync(path, content); return { path, updated: !options.dryRun }; } }, cmux: false, changed() {}, applied() {} });
	return { conversation, pending, contexts, path, dir, manager, finish: (...args) => finish(...args), async dispose() { await conversation.dispose(); rmSync(dir, { recursive: true, force: true }); } };
}

test("conversation sends full annotation evidence but displays feedback immediately and collapsed", async () => {
	const f = await conversationFixture();
	try {
		await f.conversation.start();
		const annotation = { path: f.path, revision: 2, side: "proposed", line: 4, section: "# Jobs", excerpt: "4: same text", comment: "Explain the retry limit" };
		f.conversation.state.draft = { ...sampleDraft, path: f.path };
		f.conversation.state.annotations = [annotation];
		f.conversation.state.input = "Overall feedback";
		const sending = f.conversation.send();
		await until(() => f.contexts.length === 1);
		const state = f.conversation.state;
		assert.equal(state.activity, "drafting");
		assert.equal(state.messages.at(-1).text, "Overall feedback");
		assert.deepEqual(state.messages.at(-1).annotations, [annotation]);
		assert.deepEqual(state.annotations, []);
		assert.equal(state.input, "");
		const stored = readPending(f.dir, f.pending.ownerId, "service");
		assert.equal(stored.feedback.length, 1);
		assert.match(JSON.stringify(f.contexts[0]), /proposed line 4/);
		assert.match(JSON.stringify(f.contexts[0]), /Explain the retry limit/);
		await f.conversation.send();
		assert.equal(f.contexts.length, 1);
		f.finish();
		await sending;
		assert.equal(state.activity, null);
		assert.equal(state.messages.at(-1).text, "Response ready");
	} finally { await f.dispose(); }
});

test("checkpoint command releases main input, preserves work on stream switches and cleans up on shutdown", async () => {
	const f = await conversationFixture();
	let shutdown;
	try {
		savePending(f.dir, f.pending);
		const otherPath = join(f.dir, "other.md");
		writeFileSync(otherPath, "other scratch");
		savePending(f.dir, { ...f.pending, slug: "other", input: "other feedback" });
		const streams = [{ slug: "service", path: f.path }, { slug: "other", path: otherPath }];
		let command, shortcut, panel, dismissed = 0, confirmations = 0, allowSwitch = true, holdSwitch = false;
		const statuses = [];
		const confirmationSignals = [];
		const ctx = { mode: "tui", hasUI: true, cwd: f.dir, model: f.conversation.options.model, thinkingLevel: "off", sessionManager: f.manager,
			modelRegistry: { getRegisteredProviderConfig() {}, getRegisteredNativeProvider() {} }, getSystemPrompt: () => "Test instructions",
			ui: { notify: (text) => assert.fail(text), theme: { fg: (_color, text) => text }, setStatus: (_key, value) => statuses.push(value),
				confirm: async (title, _message, options) => {
					assert.match(title, /Switch checkpoint/);
					confirmations++;
					confirmationSignals.push(options?.signal);
					return holdSwitch ? new Promise((resolve) => options.signal.addEventListener("abort", () => resolve(false), { once: true })) : allowSwitch;
				},
				custom: (factory, options) => new Promise((resolve) => {
					panel = factory({ terminal: { rows: 30 }, requestRender() {} }, { fg: (_color, text) => text }, {}, () => { dismissed++; resolve(); });
					const handle = { hidden: false, focus() {}, setHidden(value) { this.hidden = value; }, isHidden() { return this.hidden; } };
					options.onHandle(handle);
				}),
			},
		};
		registerCheckpoint({ registerCommand: (_name, options) => { command = options.handler; }, registerShortcut: (_key, options) => { shortcut = options.handler; }, on: (_event, handler) => { shutdown = handler; }, appendEntry: (type, data) => f.manager.appendCustomEntry(type, data) }, {
			agentDir: f.dir, runtime: f.conversation.options.runtime, skill: "Test skill", cli: { list: async () => streams, show: async (ref) => streams.find((stream) => stream.slug === ref), format: async () => "format" },
		});
		let returned = false;
		Promise.resolve(command("service", ctx)).then(() => { returned = true; });
		await until(() => !!panel);
		assert.equal(returned, true, "The command must not occupy Pi's prompt queue for the overlay lifetime");
		assert.equal(dismissed, 0);
		await until(() => panel.state.activity === null);
		panel.handleInput("\x1b");
		assert.match(statuses.at(-1), /Checkpoint.*Alt\+P/);
		assert.doesNotMatch(statuses.at(-1), /composer|return|alt\+p/);
		await shortcut(ctx);
		assert.equal(statuses.at(-1), undefined);
		const firstPanel = panel;
		panel.state.activity = "applying";
		await command("other", ctx);
		assert.equal(panel, firstPanel);
		assert.equal(confirmations, 0, "A stream switch cannot interrupt Apply");
		panel.state.activity = null;
		await command("other", ctx);
		await until(() => panel !== firstPanel && panel.state.activity === null);
		assert.equal(confirmations, 1);
		assert.ok(confirmationSignals[0] instanceof AbortSignal, "Shutdown must be able to cancel a pending switch confirmation");
		assert.equal(dismissed, 1);
		assert.equal(panel.state.input, "other feedback");
		assert.equal(readPending(f.dir, f.pending.ownerId, "service").input, "unsent");
		assert.equal(latestBinding(f.manager.getEntries(), f.pending.ownerId).slug, "other");
		allowSwitch = false;
		await command("service", ctx);
		await until(() => confirmations === 2);
		await shutdown();
		assert.equal(dismissed, 2);
		assert.equal(statuses.at(-1), undefined);
		assert.equal(latestBinding(f.manager.getEntries(), f.pending.ownerId).slug, "other");
		assert.equal(readPending(f.dir, f.pending.ownerId, "other").input, "other feedback");
		const secondPanel = panel;
		await command("", ctx);
		await until(() => panel !== secondPanel && panel.state.activity === null);
		holdSwitch = true;
		await command("service", ctx);
		await until(() => confirmations === 3);
		await shutdown();
		assert.equal(confirmationSignals.at(-1).aborted, true);
		assert.equal(latestBinding(f.manager.getEntries(), f.pending.ownerId).slug, "other");
	} finally { await shutdown?.(); await f.dispose(); }
});

test("refresh without a draft succeeds without a review warning and preserves unfinished annotations", async () => {
	const f = await conversationFixture();
	try {
		await f.conversation.start();
		f.conversation.state.annotationEditor = { anchor: { path: f.path, revision: 1, side: "original", line: 1, section: "# Jobs", excerpt: "1: old" }, text: "Unfinished comment" };
		f.conversation.persist();
		await f.conversation.refreshContext();
		assert.equal(f.conversation.state.activity, null);
		assert.equal(f.conversation.state.notice.level, "info");
		assert.doesNotMatch(f.conversation.state.notice.text, /draft|—/i);
		assert.equal(readPending(f.dir, f.pending.ownerId, "service").annotationEditor.text, "Unfinished comment");
	} finally { await f.dispose(); }
});

// Feasibility gate: SDK session must run with a separate context and without
// importing main-session extensions, bash, edit, or write.
test("registers the manual checkpoint command without starting work", () => {
	const commands = [];
	registerCheckpoint({ on() {}, registerCommand: (name, options) => { commands.push({ name, options }); }, registerShortcut() {} });
	assert.equal(commands.length, 1);
	assert.equal(commands[0].name, "checkpoint");
	assert.equal(typeof commands[0].options.handler, "function");
});

test("registers a configurable shortcut for returning to an open checkpoint", () => {
	const dir = mkdtempSync(join(tmpdir(), "checkpoint-shortcut-"));
	try {
		const shortcuts = [];
		registerCheckpoint({ on() {}, registerCommand() {}, registerShortcut: (key, options) => shortcuts.push({ key, options }) }, { agentDir: dir });
		assert.equal(shortcuts[0].key, "alt+p");
		writeFileSync(join(dir, "checkpoint.json"), '{"focusShortcut":"ctrl+alt+c"}');
		shortcuts.length = 0;
		registerCheckpoint({ on() {}, registerCommand() {}, registerShortcut: (key, options) => shortcuts.push({ key, options }) }, { agentDir: dir });
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

test("fresh kickoff is model context, never visible chat, and resume never starts another draft", async () => {
	const f = await conversationFixture();
	try {
		f.pending.input = "";
		f.conversation.state.input = "";
		const opening = f.conversation.start();
		await until(() => f.contexts.length === 1);
		assert.match(JSON.stringify(f.contexts[0]), /Draft a handoff/);
		assert.deepEqual(f.conversation.state.messages, []);
		f.finish();
		await opening;
		assert.deepEqual(f.conversation.state.messages.map((message) => message.text), ["Response ready"]);
		await f.conversation.refreshContext();
		assert.equal(f.contexts.length, 1);
		assert.equal(readFileSync(f.path, "utf8"), "scratch unchanged");
	} finally { await f.dispose(); }
});

test("Apply refuses unsent feedback, then writes the reviewed draft and clears pending state", async () => {
	const f = await conversationFixture();
	try {
		await f.conversation.start();
		const draft = stageDraft(f.pending, f.conversation.options.binding, "approved content", "Updated handoff");
		f.conversation.state.draft = draft;
		await f.conversation.apply();
		assert.match(f.conversation.state.notice.text, /pending feedback/);
		assert.equal(readFileSync(f.path, "utf8"), "scratch unchanged");
		f.conversation.state.input = "";
		let summary;
		f.conversation.options.applied = (value) => { summary = value; };
		await f.conversation.apply();
		assert.equal(readFileSync(f.path, "utf8"), draft.proposedContent);
		assert.equal(summary, "Updated handoff");
		assert.equal(readPending(f.dir, f.pending.ownerId, "service"), undefined);
	} finally { await f.dispose(); }
});

test("refresh keeps the original review snapshot when scratch changes externally", async () => {
	const f = await conversationFixture();
	try {
		await f.conversation.start();
		const draft = stageDraft(f.pending, f.conversation.options.binding, "proposed", "Updated");
		f.conversation.state.draft = draft;
		writeFileSync(f.path, "external change");
		await f.conversation.refreshContext();
		assert.equal(f.conversation.state.draft.baseContent, "scratch unchanged");
		assert.match(f.conversation.state.notice.text, /Scratch changed/);
		f.conversation.state.input = "";
		await f.conversation.apply();
		assert.match(f.conversation.state.notice.text, /Scratch changed/);
		assert.equal(readFileSync(f.path, "utf8"), "external change");
	} finally { await f.dispose(); }
});

test("stopping and shutting down a running conversation preserves the accepted question", async () => {
	const f = await conversationFixture();
	try {
		await f.conversation.start();
		const sending = f.conversation.send();
		await until(() => f.contexts.length === 1);
		await f.conversation.stop();
		await sending;
		assert.equal(f.conversation.state.activity, null);
		assert.match(f.conversation.state.notice.text, /stopped/);
		await f.conversation.dispose();
		assert.match(JSON.stringify(readPending(f.dir, f.pending.ownerId, "service").messages), /unsent/);
	} finally { await f.dispose(); }
});

test("annotations distinguish repeated lines, deletions, and immutable draft revisions", async () => {
	const { reviewLines, anchorAnnotation, feedbackPrompt } = await jiti.import(new URL("../home/.pi/agent/extensions/stream-checkpoint/review.ts", import.meta.url).pathname);
	const draft = { path: "/scratch/service.md", revision: 2, baseContent: "# First\nsame\n# Second\nsame\nremoved\n", proposedContent: "# First\nsame\n# Second\nsame\nadded\n" };
	const lines = reviewLines(draft);
	const first = anchorAnnotation(draft, lines[1]);
	const second = anchorAnnotation(draft, lines[3]);
	assert.equal(first.line, 2);
	assert.equal(second.line, 4);
	assert.equal(second.section, "# Second");
	const deleted = anchorAnnotation(draft, lines.find((line) => line.kind === "removed"));
	assert.equal(deleted.side, "original");
	assert.equal(deleted.line, 5);
	assert.match(deleted.excerpt, /4: same\n5: removed/);
	const comments = [{ ...second, comment: "Clarify this occurrence" }, { ...deleted, comment: "Keep the exception" }];
	const prompt = feedbackPrompt("Please revise", comments);
	assert.match(prompt, /Draft 2.*\/scratch\/service.md/);
	assert.match(prompt, /proposed line 4/);
	assert.match(prompt, /original line 5/);
	assert.match(prompt, /Clarify this occurrence/);
	assert.match(prompt, /Keep the exception/);
	draft.proposedContent = "replacement";
	assert.match(comments[0].excerpt, /same/);
	assert.equal(comments[0].revision, 2);
});

test("review diff preserves source line numbers through replacements and blank lines", async () => {
	const { reviewLines } = await jiti.import(new URL("../home/.pi/agent/extensions/stream-checkpoint/review.ts", import.meta.url).pathname);
	assert.deepEqual(reviewLines({ baseContent: "a\n\nb\n", proposedContent: "a\n\nc\n" }), [
		{ kind: "context", text: "a", oldLine: 1, newLine: 1 },
		{ kind: "context", text: "", oldLine: 2, newLine: 2 },
		{ kind: "removed", text: "b", oldLine: 3 },
		{ kind: "added", text: "c", newLine: 3 },
	]);
});

test("panel starts compact, grows with wrapped chat and does not repeat the draft summary", () => {
	const { panel, state } = panelFixture();
	const initial = panel.render(60);
	assert.ok(initial.length < 12);
	state.messages = [{ role: "assistant", text: "evidence ".repeat(24) }];
	state.draft = { ...sampleDraft, summary: "Long summary should not appear in chrome" };
	const lines = panel.render(60);
	assert.ok(lines.length > initial.length);
	assert.equal((lines.join("\n").match(/evidence/g) ?? []).length, 24);
	assert.doesNotMatch(lines.join("\n"), /Long summary/);
});

test("review paging selects the new page and the next arrow stays on that page", () => {
	const { panel } = panelFixture({ draft: { ...sampleDraft, baseContent: Array.from({ length: 60 }, (_, i) => `old ${i}`).join("\n"), proposedContent: "new" } });
	const view = () => panel.render(56).join("\n").replace(/\x1b\[[0-9;]*m/g, "");
	panel.handleInput("\t");
	assert.match(view(), /▶ 1 - old 0/);
	panel.handleInput("\x1b[6~");
	const selected = Number(view().match(/▶ \d+ - old (\d+)/)?.[1]);
	assert.ok(selected >= 8);
	panel.handleInput("\x1b[B");
	assert.match(view(), new RegExp(`▶ \\d+ - old ${selected + 1}\\b`));
	panel.handleInput("\x1b[5~");
	assert.match(view(), /▶ \d+ - old [01]\b/);
});

test("wrapped review lines retain their document anchors and comments can be edited or removed", () => {
	const { panel, state } = panelFixture({ draft: { ...sampleDraft, baseContent: "old", proposedContent: "new ".repeat(220) + "TAIL_MARKER" } });
	panel.handleInput("\t"); panel.render(48);
	panel.handleInput("\x1b[B"); panel.render(48);
	panel.handleInput("\x1b[6~");
	assert.match(panel.render(48).join("\n"), /TAIL_MARKER/);
	panel.handleInput("\x1bk");
	assert.equal(state.annotationEditor.anchor.line, 1);
	assert.equal(state.annotationEditor.anchor.side, "proposed");
	assert.match(state.annotationEditor.anchor.excerpt, /TAIL_MARKER/);
	panel.handleInput("Comment"); panel.handleInput("\r");
	panel.handleInput("\x1bk");
	assert.equal(state.annotationEditor.text, "Comment");
	panel.handleInput("\x15"); panel.handleInput("\r");
	assert.deepEqual(state.annotations, []);
});

test("sent annotations collapse in chat, expand on demand and never attach to a later draft", () => {
	const annotation = { path: sampleDraft.path, revision: 2, side: "proposed", line: 2, section: "# Jobs", excerpt: "2: new", comment: "Unique annotation detail" };
	const { panel, state } = panelFixture({ draft: sampleDraft, messages: [{ role: "user", text: "Please revise", annotations: [annotation] }] });
	let view = panel.render(100).join("\n");
	assert.match(view, /1 annotation · Draft 2/);
	assert.doesNotMatch(view, /Unique annotation detail/);
	panel.handleInput("\x1ba");
	assert.match(panel.render(100).join("\n"), /Unique annotation detail/);
	panel.handleInput("\t");
	assert.match(panel.render(100).join("\n"), /Unique annotation detail/);
	state.draft = { ...sampleDraft, revision: 3 };
	assert.doesNotMatch(panel.render(100).join("\n"), /Unique annotation detail/);
});

test("chat scrolling stays put while new content arrives", () => {
	const { panel, state } = panelFixture({ messages: Array.from({ length: 30 }, (_, i) => ({ role: "assistant", text: `Message ${i}` })) });
	assert.match(panel.render(60).join("\n"), /Message 29/);
	panel.handleInput("\x1b[5~");
	const before = panel.render(60).join("\n");
	state.messages.push({ role: "assistant", text: "Message 30" });
	assert.equal(panel.render(60).join("\n"), before);
});

test("short terminals keep the latest input and never overflow", () => {
	const { state } = panelFixture({ input: "earlier\n".repeat(20) + "latest input", draft: sampleDraft });
	const panel = new CheckpointPanel({ terminal: { rows: 12 }, requestRender() {} }, { fg: (_color, text) => text }, state, {});
	const lines = panel.render(60);
	assert.ok(lines.length <= 12);
	assert.match(lines[2], /↑/, "The shared editor border must retain its scroll indicator");
	assert.match(lines.join("\n"), /latest input/);
});

test("theme invalidation recolors review and the heading is distinct from participants", () => {
	const { panel } = panelFixture({ draft: sampleDraft });
	panel.handleInput("\t");
	const dark = panel.render(60).join("\n");
	try { initTheme("light"); panel.invalidate(); assert.notEqual(panel.render(60).join("\n"), dark); }
	finally { initTheme("dark"); }
});

test("Esc minimizes, Clear confirms, and busy Apply cannot clear or stop", () => {
	const { panel, state, events } = panelFixture();
	panel.handleInput("\x1b");
	assert.deepEqual(events, ["hide"]);
	panel.handleInput("\x1bc");
	assert.match(panel.render(70).join("\n"), /Discard pending chat and draft/);
	panel.handleInput("n");
	assert.equal(events.includes("clear"), false);
	panel.handleInput("\x1bc"); panel.handleInput("y");
	assert.equal(events.at(-1), "clear");
	state.activity = "applying";
	panel.handleInput("\x1bc"); panel.handleInput("\x03"); panel.handleInput("\x1b");
	assert.deepEqual(events, ["hide", "clear", "hide"]);
});

test("input stays durable until accepted and hiding an annotation preserves its unfinished text", () => {
	const { panel, state, events } = panelFixture({ draft: sampleDraft });
	panel.handleInput("question"); panel.handleInput("\r");
	assert.equal(state.input, "question");
	assert.equal(events.at(-1), "send");
	panel.handleInput("\t"); panel.handleInput("\x1bk"); panel.handleInput("unfinished"); panel.handleInput("\x1b");
	assert.equal(state.annotationEditor.text, "unfinished");
	assert.equal(events.at(-1), "hide");
});

test("spinner animates during operations and stops when idle", async () => {
	const { panel, state } = panelFixture({ activity: "starting" });
	try {
		const first = panel.render(80).join("\n");
		assert.match(first, /⠋ Starting/);
		await new Promise((resolve) => setTimeout(resolve, 110));
		assert.notEqual(panel.render(80).join("\n"), first);
		state.activity = null;
		assert.doesNotMatch(panel.render(80).join("\n"), /[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]/);
	} finally { panel.dispose(); }
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
	chooser.invalidate();
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
		const result = await tool.execute("proposal-1", { content: "proposed", summary: "New state" }, undefined, undefined, {});
		assert.equal(result.content[0].text, "Draft 1 ready for review.");
		assert.deepEqual(proposals, [{ content: "proposed", summary: "New state" }]);
		assert.deepEqual(session.messages.filter((item) => item.role === "user").map((item) => item.content[0].text), ["new main snapshot", "earlier checkpoint question"]);
	} finally {
		session?.dispose();
		rmSync(dir, { recursive: true, force: true });
	}
});

test("side context preserves native branch and compaction summaries through Pi's model projection", async () => {
	const dir = mkdtempSync(join(tmpdir(), "checkpoint-projection-"));
	let session;
	try {
		const runtime = await ModelRuntime.create({ authPath: join(dir, "auth.json"), modelsPath: null, refreshOnCreate: false });
		const mainMessages = [
			{ role: "branchSummary", summary: "Earlier branch decisions", fromId: "earlier", timestamp: 1 },
			{ role: "compactionSummary", summary: "Compacted context", tokensBefore: 100, timestamp: 2 },
			{ role: "bashExecution", command: "echo private", output: "excluded context", exitCode: 0, cancelled: false, truncated: false, excludeFromContext: true, timestamp: 3 },
		];
		session = await createCheckpointSession({ cwd: dir, agentDir: dir, modelRuntime: runtime, model: runtime.getModels()[0], mainMessages, savedMessages: [], systemPrompt: "checkpoint instructions" });
		assert.equal(session.messages.length, 2);
		assert.ok(session.messages.every((message) => message.role === "user"));
		assert.match(session.messages[0].content[0].text, /Earlier branch decisions/);
		assert.match(session.messages[1].content[0].text, /Compacted context/);
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
