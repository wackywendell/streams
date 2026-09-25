import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { OverlayHandle } from "@earendil-works/pi-tui";
import { BorderedLoader, getAgentDir, ModelRuntime } from "@earendil-works/pi-coding-agent";
import type { AgentSession, ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { checkpointContext, completedMessages, createCheckpointSession } from "./agent.ts";
import { applyDraft, BINDING_TYPE, clearPending, includeMainSession, latestBinding, loadConfig, parseRecommendations, readPending, savePending, selectStream, stageDraft } from "./core.ts";
import type { Pending, Recommendations } from "./core.ts";
import { CheckpointPanel, StreamChooser } from "./panel.ts";
import type { ChosenStream, PanelState } from "./panel.ts";
import { StreamCli } from "./streams.ts";

function transcript(messages: AgentMessage[]): string[] {
	return messages.filter((message) => message.role === "user" || message.role === "assistant").map((message) => {
		const content = typeof message.content === "string" ? message.content : message.content.filter((part) => part.type === "text").map((part) => part.text).join("\n");
		return `${message.role === "user" ? "You" : "Checkpoint"}: ${content}`;
	}).filter((text) => !text.endsWith(": "));
}

export async function recommendStreams(
	ctx: Pick<ExtensionCommandContext, "model" | "sessionManager" | "modelRegistry">,
	streams: Awaited<ReturnType<StreamCli["list"]>>,
	model = ctx.model,
): Promise<Recommendations> {
	if (!model) return { existing: [] };
	const snapshot = checkpointContext(ctx.sessionManager);
	const metadata = streams.map(({ slug, title, status, state }) => ({ slug, title, status, state }));
	const message: AgentMessage = { role: "user", content: [{ type: "text", text: `Current non-archived streams:\n${JSON.stringify(metadata)}\nRecommend existing streams by their exact slug, or suggest a new title only if none fit.` }], timestamp: Date.now() };
	const answer = await ctx.modelRegistry.complete(model, {
		systemPrompt: "Use the current conversation to recommend up to 3 existing streams and optionally one new stream. Prefer a short, stable human-readable new title: aim for 6 words and around 40 characters. This is guidance, not a hard limit; return the full title if a longer name is needed. Avoid sentences or progress updates; put details in the reason, since streams keeps a fuller description and state separately. Return only JSON: {\"existing\":[{\"slug\":\"...\",\"reason\":\"...\"}],\"newStream\":{\"title\":\"...\",\"reason\":\"...\"}}. Recommendations do not create or bind streams.",
		messages: [...snapshot.messages, message] as any,
	});
	return parseRecommendations(answer.content.filter((part) => part.type === "text").map((part) => part.text).join(""), streams);
}

function configuredModel(ctx: Pick<ExtensionCommandContext, "model" | "modelRegistry">, configured: { provider: string; id: string } | undefined, role: string) {
	if (!configured) return ctx.model;
	const model = ctx.modelRegistry.find(configured.provider, configured.id);
	if (!model) throw new Error(`Checkpoint ${role} model ${configured.provider}/${configured.id} is unavailable`);
	return model;
}

function overlayOptions() {
	return { overlay: true, overlayOptions: { anchor: "right-center" as const, width: "64%" as const, minWidth: 60, margin: { right: 1 }, maxHeight: 36 } };
}

function chooserOverlayOptions() {
	return { overlay: true, overlayOptions: { anchor: "center" as const, width: "78%" as const, minWidth: 64, margin: 2, maxHeight: "80%" as const } };
}

export default function registerCheckpoint(pi: ExtensionAPI, testDeps: { agentDir?: string; cli?: StreamCli; runtime?: ModelRuntime; skill?: string } = {}): void {
	let open = false;
	let activeHandle: OverlayHandle | undefined;
	let activeOwner: string | undefined;
	const statusKey = "checkpoint";
	const focusShortcut = loadConfig(testDeps.agentDir ?? getAgentDir()).focusShortcut;
	pi.registerShortcut(focusShortcut, {
		description: "Focus the open checkpoint panel",
		handler: (ctx) => {
			if (activeHandle && activeOwner === ctx.sessionManager.getSessionId()) {
				activeHandle.focus();
				ctx.ui.setStatus(statusKey, ctx.ui.theme.fg("accent", `Checkpoint open · ${focusShortcut}`));
			} else ctx.ui.notify("No checkpoint panel is open; run /checkpoint", "info");
		},
	});
	pi.registerCommand("checkpoint", {
		description: "Discuss and review an explicit stream checkpoint in a side panel",
		handler: async (ref: string, ctx: ExtensionCommandContext) => {
			if (ctx.mode !== "tui" || !ctx.hasUI) {
				ctx.ui.notify("/checkpoint requires an interactive Pi terminal", "error");
				return;
			}
			if (open) {
				if (activeHandle) {
					activeHandle.focus();
					ctx.ui.setStatus(statusKey, ctx.ui.theme.fg("accent", `Checkpoint open · ${focusShortcut}`));
				}
				else ctx.ui.notify("A checkpoint chooser is already open", "info");
				return;
			}
			open = true;
			let side: AgentSession | undefined;
			let initialization: Promise<void> | undefined;
			let ownerActive = true;
			try {
				const agentDir = testDeps.agentDir ?? getAgentDir();
				const config = loadConfig(agentDir);
				const cli = testDeps.cli ?? new StreamCli(ctx.cwd);
				const ownerId = ctx.sessionManager.getSessionId();
				const binding = latestBinding(ctx.sessionManager.getEntries(), ownerId);
				const streams = await cli.list();
				let stream = ref.trim() ? await cli.show(ref.trim()).catch((error) => {
					ctx.ui.notify(String(error), "warning");
					return undefined;
				}) : await selectStream("", binding, { list: async () => streams, show: (value) => cli.show(value) });
				let created = false;
				let switchConfirmed = false;
				if (!stream) {
					if (binding && readPending(agentDir, ownerId, binding.slug)) {
						switchConfirmed = await ctx.ui.confirm("Switch checkpoint stream?", `Unfinished work for ${binding.slug} will remain saved.`);
						if (!switchConfirmed) return;
					}
					const selectorModel = configuredModel(ctx, config.selectorModel, "selector");
					const recommendations = await ctx.ui.custom<Recommendations>((tui, theme, _keys, done) => {
						const loader = new BorderedLoader(tui, theme, "Finding stream recommendations…", { cancellable: false });
						void recommendStreams(ctx, streams, selectorModel).then(done, () => done({ existing: [] }));
						return loader;
					}, { overlay: true, overlayOptions: { anchor: "center", width: "52%", minWidth: 50, maxHeight: 10 } });
					const choice = await ctx.ui.custom<ChosenStream | undefined>((tui, theme, _keys, done) => new StreamChooser(tui, theme, streams, cli, done, recommendations, ref.trim()), chooserOverlayOptions());
					if (!choice) return;
					stream = choice.stream;
					created = choice.created;
				}
				if (binding?.slug !== stream.slug || binding.path !== stream.path) {
					if (binding && !switchConfirmed && readPending(agentDir, ownerId, binding.slug)) {
						const switchTarget = await ctx.ui.confirm("Switch checkpoint stream?", `Unfinished work for ${binding.slug} will remain saved.`);
						if (!switchTarget) return;
					}
					pi.appendEntry(BINDING_TYPE, { ownerId, slug: stream.slug, path: stream.path });
				}
				let projectionWarning = "";
				if (created && config.cmux) {
					try { await cli.project(stream.slug); }
					catch (error) { projectionWarning = `CMUX update failed: ${String(error)}`; }
				}
				const model = configuredModel(ctx, config.drafterModel, "drafter");
				if (!model) throw new Error("Select a model before starting a checkpoint conversation");
				const selected = stream;
				const runtime = testDeps.runtime ?? await ModelRuntime.create({ authPath: join(agentDir, "auth.json"), modelsPath: join(agentDir, "models.json") });
				const providerConfig = ctx.modelRegistry.getRegisteredProviderConfig(model.provider);
				if (providerConfig) runtime.registerProvider(model.provider, providerConfig);
				const nativeProvider = ctx.modelRegistry.getRegisteredNativeProvider(model.provider);
				if (nativeProvider) runtime.registerNativeProvider(nativeProvider);
				const skill = testDeps.skill ?? readFileSync(join(homedir(), ".agents", "skills", "streams", "SKILL.md"), "utf8");
				const format = await cli.format();
				const systemPrompt = `${ctx.getSystemPrompt()}\n\n<checkpoint_rules>\nYou are a separate checkpoint conversation. Read and discuss stream state; propose only complete scratch Markdown through propose_checkpoint. Do not claim to have written scratch. You have only local read-only tools and the proposal tool. The user alone approves Apply. Every proposal must include pi:${ownerId} in frontmatter sessions (preserve ordering), never the checkpoint session ID.\n\n${skill}\n\n${format}\n</checkpoint_rules>`;
				const pending: Pending = readPending(agentDir, ownerId, selected.slug) ?? { version: 1, ownerId, slug: selected.slug, messages: [], input: "" };
				const fresh = pending.messages.length === 0 && !pending.draft && !pending.input.trim();
				const state: PanelState = { slug: selected.slug, title: selected.title, messages: transcript(pending.messages as AgentMessage[]), draft: pending.draft, status: "Starting checkpoint…" };
				let panel: CheckpointPanel | undefined;
				let inputTimer: ReturnType<typeof setTimeout> | undefined;
				let savedCount = 0;
				const stillOwner = () => ownerActive && ctx.sessionManager.getSessionId() === ownerId;
				const save = () => {
					if (!stillOwner()) return;
					if (side) {
						pending.messages.push(...side.messages.slice(savedCount));
						savedCount = side.messages.length;
					}
					pending.input = panel?.getInput() ?? pending.input;
					savePending(agentDir, pending);
				};
				const startSide = async () => {
					const safeSaved = completedMessages(pending.messages as AgentMessage[]);
					if (safeSaved.length !== pending.messages.length) {
						pending.messages = safeSaved;
						state.messages = transcript(safeSaved);
						state.status = "Interrupted answer omitted; earlier question preserved";
						savePending(agentDir, pending);
					}
					const snapshot = checkpointContext(ctx.sessionManager);
					if (snapshot.omitted) state.status = `${snapshot.omitted} incomplete main message(s) omitted from context`;
					pending.source = { ownerId, leafId: snapshot.leafId };
					const scratch = readFileSync(selected.path, "utf8");
					if (pending.draft && pending.draft.baseContent !== scratch) {
						state.draft = { ...pending.draft, baseContent: scratch };
						state.status = "Scratch changed; review this diff and propose a new draft";
					}
					savePending(agentDir, pending);
					const mainMessages = [...snapshot.messages, { role: "user", content: [{ type: "text", text: `Checkpoint source: main session ${ownerId}, leaf ${snapshot.leafId ?? "root"}. Stream ${selected.slug}, scratch ${selected.path}. ${snapshot.omitted ? `${snapshot.omitted} incomplete or aborted main message(s) omitted for provider-valid context. ` : ""}Current scratch Markdown:\n${scratch}` }], timestamp: Date.now() } as AgentMessage];
					const result = await createCheckpointSession({
						cwd: ctx.cwd, agentDir, modelRuntime: runtime, model, thinkingLevel: ctx.thinkingLevel,
						mainMessages, savedMessages: pending.messages as AgentMessage[], systemPrompt,
						onProposal: async (content, summary) => {
							if (!stillOwner()) throw new Error("Owning Pi session changed; proposal discarded");
							const draft = stageDraft(pending, { ownerId, slug: selected.slug, path: selected.path }, includeMainSession(content, ownerId), summary);
							savePending(agentDir, pending);
							state.draft = draft;
							state.status = `Draft ${draft.revision} staged; review before Apply`;
							panel?.refresh();
							return draft.revision;
						},
					});
					side = result;
					savedCount = result.messages.length;
					result.subscribe((event) => {
						if (!stillOwner()) return;
						if (event.type === "message_update" && event.assistantMessageEvent.type === "text_delta") {
							const partial = event.message.content.filter((part) => part.type === "text").map((part) => part.text).join("\n");
							state.messages = [...transcript(pending.messages as AgentMessage[]), ...transcript(result.messages.slice(savedCount)), `Checkpoint: ${partial}`];
							state.status = "Checkpoint is responding…";
							panel?.refresh();
						} else if (event.type === "tool_execution_start") {
							state.status = `Reading with ${event.toolName}…`;
							panel?.refresh();
						}
					});
				};
				await ctx.ui.custom<void>((tui, theme, _keys, done) => {
					panel = new CheckpointPanel(tui, theme, state, {
						send: async (text) => {
							await initialization;
							if (!stillOwner() || !side || side.isStreaming) throw new Error("Checkpoint agent is busy or the session changed");
							await side.prompt(text, { expandPromptTemplates: false });
							save();
							state.messages = transcript(pending.messages as AgentMessage[]);
							state.status = projectionWarning || "Ready";
							panel?.refresh();
						},
						apply: async () => {
							if (!stillOwner() || !pending.draft || side?.isStreaming) throw new Error("No ready draft or owning session changed");
							const draft = pending.draft;
							const result = await applyDraft(draft, draft.revision, { update: (slug, content, options) => cli.update(slug, content, options), project: config.cmux ? () => cli.project(selected.slug) : undefined });
							if (result.validationError) { state.status = result.validationError; panel?.refresh(); return; }
							pi.sendMessage({ customType: "checkpoint-receipt", content: `Checkpoint applied to ${selected.slug} (${selected.path}): ${draft.summary ?? "scratch updated"}${result.projectionWarning ? `\nCMUX: ${result.projectionWarning}` : ""}`, display: true }, { triggerTurn: false });
							clearPending(agentDir, ownerId, selected.slug);
							ownerActive = false;
							done();
						},
						refresh: async () => {
							await initialization;
							if (!side || side.isStreaming || !stillOwner()) throw new Error("Wait for the current checkpoint response before refreshing");
							save();
							side.dispose();
							await startSide();
							state.status = pending.draft && pending.draft.baseContent !== readFileSync(selected.path, "utf8")
								? "Scratch changed; review this diff and propose a new draft"
								: `Refreshed main context; review draft ${pending.draft?.revision ?? "—"} again`;
						},
						stop: async () => { if (side?.isStreaming) await side.abort(); },
						close: async (clear) => {
							if (inputTimer) clearTimeout(inputTimer);
							if (side?.isStreaming) await side.abort();
							if (clear) clearPending(agentDir, ownerId, selected.slug);
							else save();
							ownerActive = false;
						},
						focusMain: () => {
							activeHandle?.unfocus();
							ctx.ui.setStatus(statusKey, ctx.ui.theme.fg("accent", `Checkpoint open · ${focusShortcut}`));
						},
						inputChanged: (value) => {
							pending.input = value;
							if (inputTimer) clearTimeout(inputTimer);
							inputTimer = setTimeout(() => { if (stillOwner()) savePending(agentDir, pending); }, 250);
						},
					}, done, focusShortcut);
					panel.setInput(pending.input);
					initialization = new Promise<void>((resolve, reject) => setImmediate(() => {
						if (!stillOwner()) { resolve(); return; }
						void startSide().then(resolve, reject);
					})).then(() => {
						if (!stillOwner()) return;
						if (fresh && side && !panel?.getInput().trim()) {
							state.status = projectionWarning || "Drafting handoff…";
							panel?.startMessage(`Using the streams skill, review the current main-session context and scratch note for ${selected.slug}. Draft a handoff with the current state, important decisions, and next action. Ask about material gaps if needed, then stage complete updated scratch Markdown with propose_checkpoint for my review. Do not write scratch or apply the draft.`);
						} else if (state.status === "Starting checkpoint…") state.status = projectionWarning || "Ready";
						panel?.refresh();
					}).catch((error) => { state.status = String(error); panel?.refresh(); });
					return panel;
				}, { ...overlayOptions(), onHandle: (handle: OverlayHandle) => {
					activeHandle = handle;
					activeOwner = ownerId;
					ctx.ui.setStatus(statusKey, ctx.ui.theme.fg("accent", `Checkpoint open · ${focusShortcut}`));
				} });
			} catch (error) {
				ctx.ui.notify(`Checkpoint: ${String(error)}`, "error");
			} finally {
				ownerActive = false;
				await initialization;
				if (side?.isStreaming) await side.abort();
				side?.dispose();
				activeHandle = undefined;
				activeOwner = undefined;
				ctx.ui.setStatus(statusKey, undefined);
				open = false;
			}
		},
	});
}
