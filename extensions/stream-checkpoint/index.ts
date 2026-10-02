import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { OverlayHandle } from "@earendil-works/pi-tui";
import { convertToLlm, getAgentDir, ModelRuntime } from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ExtensionCommandContext, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { checkpointContext } from "./agent.ts";
import { BINDING_TYPE, latestBinding, loadConfig, parseRecommendations, readPending, selectStream } from "./core.ts";
import type { Recommendations } from "./core.ts";
import { CheckpointPanel } from "./panel.ts";
import { StreamChooser } from "./chooser.ts";
import type { ChosenStream } from "./chooser.ts";
import { CheckpointConversation } from "./conversation.ts";
import { checkpointLabel, checkpointStatus, shortcutLabel } from "./view.ts";
import { CheckpointJournal, MARKER_TYPE } from "./journal.ts";
import { StreamCli } from "./streams.ts";
import { checkpointReceipt, choiceCancelledReceipt, clearedReceipt, discussionText, renderCheckpointReceipt, streamChosenReceipt } from "./receipt.ts";
import { streamReadTool } from "./read.ts";
import type { Author } from "./review.ts";

type Stream = Awaited<ReturnType<StreamCli["show"]>>;
type ToolStatus = "started" | "needs-stream" | "already-open" | "applying" | "awaiting-stream-choice" | "already-bound";
const toolResult = (status: ToolStatus, text: string, details: { slug?: string; instructionsSent?: boolean }) =>
	({ content: [{ type: "text" as const, text }], details: { status, ...details } });

export function parseCheckpointArgs(args: string): { ref: string; message: string } {
	const input = args.trim();
	if (!/^--stream(?:\s|$)/.test(input)) return { ref: "", message: input };
	const match = /^--stream\s+(?:"([^"]*)"|'([^']*)'|([^\s"']+))(?:\s+([\s\S]*))?$/.exec(input);
	const ref = (match?.[1] ?? match?.[2] ?? match?.[3] ?? "").trim();
	if (!match || !ref || ref.startsWith("--")) throw new Error('Usage: /checkpoint [--stream <slug-or-"quoted title">] [instructions]');
	return { ref, message: (match[4] ?? "").trim() };
}

export async function recommendStreams(
	ctx: Pick<ExtensionCommandContext, "model" | "sessionManager" | "modelRegistry">,
	streams: Awaited<ReturnType<StreamCli["list"]>>,
	model = ctx.model,
	snapshot = checkpointContext(ctx.sessionManager),
	signal?: AbortSignal,
): Promise<Recommendations> {
	if (!model) return { existing: [] };
	const metadata = streams.map(({ slug, title, status, state }) => ({ slug, title, status, state }));
	const message: AgentMessage = { role: "user", content: [{ type: "text", text: `Current non-archived streams:\n${JSON.stringify(metadata)}\nRecommend existing streams by their exact slug, or suggest a new title only if none fit.` }], timestamp: Date.now() };
	const answer = await ctx.modelRegistry.complete(model, {
		systemPrompt: "Use the current conversation to recommend up to 3 existing streams and optionally one new stream. Prefer a short, stable human-readable new title: fewer words are better. Usually 2–4 words suffice; use the shortest title that distinguishes the work. This is guidance, not a hard limit; return the full title if a longer name is needed. Avoid sentences or progress updates; put details in the reason, since streams keeps a fuller description and state separately. Return only JSON: {\"existing\":[{\"slug\":\"...\",\"reason\":\"...\"}],\"newStream\":{\"title\":\"...\",\"reason\":\"...\"}}. Recommendations do not create or bind streams.",
		messages: convertToLlm([...snapshot.messages, message]),
	}, { signal });
	return parseRecommendations(answer.content.filter((part) => part.type === "text").map((part) => part.text).join(""), streams);
}

function configuredModel(ctx: Pick<ExtensionCommandContext, "model" | "modelRegistry">, configured: { provider: string; id: string } | undefined, role: string) {
	if (!configured) return ctx.model;
	const model = ctx.modelRegistry.find(configured.provider, configured.id);
	if (!model) throw new Error(`Checkpoint ${role} model ${configured.provider}/${configured.id} is unavailable`);
	return model;
}

type OpenCheckpoint = {
	ownerId: string;
	snapshot: ReturnType<typeof checkpointContext>;
	systemPrompt: string;
	/** Journal card; select_stream runs have none, since their receipt is the record. */
	marker?: string;
	finalized?: boolean;
	cancellation: AbortController;
	/** Set for agent-started runs: drafting starts hidden, and choosing skips the up-front switch confirmation. */
	author?: Author;
	/** Set for select_stream runs, which always show the chooser and end once the stream is bound. */
	select?: { recommendations?: Recommendations };
	slug?: string;
	choosing?: boolean;
	finding?: boolean;
	handle?: OverlayHandle;
	panel?: CheckpointPanel;
	conversation?: CheckpointConversation;
	dismiss?: () => void;
	updateStatus?: () => void;
	task?: Promise<void>;
};

export default function registerCheckpoint(pi: ExtensionAPI, testDeps: { agentDir?: string; cli?: StreamCli; runtime?: ModelRuntime; skill?: string } = {}): void {
	let active: OpenCheckpoint | undefined;
	pi.registerMessageRenderer("checkpoint-receipt", renderCheckpointReceipt);
	const agentDir = testDeps.agentDir ?? getAgentDir();
	const focusShortcut = loadConfig(agentDir).focusShortcut;
	const journal = new CheckpointJournal(pi, focusShortcut);
	pi.registerEntryRenderer(MARKER_TYPE, journal.render);
	pi.registerTool(streamReadTool(agentDir, (ctx) => testDeps.cli ?? new StreamCli(ctx.cwd)));
	pi.on("session_start", (_event, ctx) => journal.restore(ctx.sessionManager.getEntries()));
	const show = (run: OpenCheckpoint) => { run.handle?.setHidden(false); run.handle?.focus(); run.updateStatus?.(); run.panel?.refresh(); };
	pi.registerShortcut(focusShortcut, {
		description: "Show the open checkpoint panel or stream chooser",
		handler: (ctx) => {
			if (active?.handle && active.ownerId === ctx.sessionManager.getSessionId()) show(active);
			else ctx.ui.notify("No checkpoint panel is open; run /checkpoint", "info");
		},
	});
	pi.on("session_shutdown", async () => {
		const run = active;
		if (!run) return;
		run.cancellation.abort();
		run.dismiss?.();
		await run.task;
	});

	async function open(run: OpenCheckpoint, ref: string, message: string, ctx: ExtensionContext, resolved?: Stream): Promise<void> {
		const alive = () => !run.cancellation.signal.aborted && ctx.sessionManager.getSessionId() === run.ownerId;
		const mark = (status: Parameters<typeof journal.update>[1]) => { if (run.marker) journal.update(run.marker, status); };
		let reported = false;
		run.updateStatus = () => {
			const value = !run.handle?.isHidden() ? undefined : run.conversation ? checkpointStatus(run.conversation.state, focusShortcut)
				: run.choosing ? `Checkpoint · ${run.finding ? "Finding streams" : "Choose stream"} · ${shortcutLabel(focusShortcut)}` : undefined;
			ctx.ui.setStatus("checkpoint", value ? ctx.ui.theme.fg("accent", value) : undefined);
		};
		try {
			if (!alive()) return;
			const config = loadConfig(agentDir);
			const cli = testDeps.cli ?? new StreamCli(ctx.cwd, undefined, (text) => { if (alive()) ctx.ui.notify(text, "warning"); });
			const binding = latestBinding(ctx.sessionManager.getEntries(), run.ownerId);
			// Lookups take no focus. Cancellation abandons them at once, so shutdown never waits on a slow CLI call.
			const stopped = new Promise<never>((_resolve, reject) => {
				const stop = () => reject(new Error("Checkpoint cancelled"));
				if (run.cancellation.signal.aborted) stop();
				else run.cancellation.signal.addEventListener("abort", stop, { once: true });
			});
			stopped.catch(() => undefined);
			const whileOpen = <T>(work: Promise<T>) => Promise.race([work, stopped]);
			const sharedConfig = await whileOpen(cli.config());
			const streams = await whileOpen(cli.list());
			let stream = run.select ? undefined : resolved ?? await whileOpen(ref.trim() ? cli.show(ref.trim()).catch((error) => { if (alive()) ctx.ui.notify(String(error), "warning"); return undefined; })
				: selectStream("", binding, { list: async () => streams, show: (value) => cli.show(value) }));
			let created = false;
			let switchConfirmed = false;
			if (!alive()) return;
			if (!stream) {
				if (!run.author && binding && readPending(agentDir, run.ownerId, binding.slug)) {
					switchConfirmed = await ctx.ui.confirm("Switch checkpoint stream?", `Unfinished work for ${binding.slug} will remain saved.`, { signal: run.cancellation.signal });
					if (!switchConfirmed || !alive()) return;
				}
				mark({ label: "Choosing stream", working: true });
				// The chooser opens hidden and is revealed once recommendations are ready, unless the user is typing in the main editor.
				let recommendations = run.select?.recommendations;
				let chooser: StreamChooser | undefined;
				const stopFinding = new AbortController();
				const reveal = () => {
					if (!run.handle || !recommendations || !alive()) return;
					if (ctx.ui.getEditorText() === "") show(run);
					else run.updateStatus?.();
				};
				run.choosing = true;
				run.finding = !recommendations;
				const choosing = ctx.ui.custom<ChosenStream | undefined>((tui, theme, _keys, done) => {
					run.dismiss = () => done(undefined);
					chooser = new StreamChooser(tui, theme, streams, cli, done, recommendations ?? { existing: [] }, ref.trim(),
						{ shortcut: focusShortcut, minimize: () => { run.handle?.setHidden(true); run.updateStatus?.(); }, finding: !recommendations });
					return chooser;
				}, { overlay: true, overlayOptions: { anchor: "center", width: "78%", minWidth: 64, margin: 2, maxHeight: "80%" },
					onHandle: (handle) => { run.handle = handle; handle.setHidden(true); run.updateStatus?.(); reveal(); } });
				if (!recommendations) {
					const selectorModel = configuredModel(ctx, config.selectorModel, "selector");
					void recommendStreams(ctx, streams, selectorModel, run.snapshot, AbortSignal.any([run.cancellation.signal, stopFinding.signal]))
						.catch(() => ({ existing: [] })).then((found) => {
							// Once the chooser closes, run.handle may be the panel's, and a late result must not reveal it.
							if (stopFinding.signal.aborted) return;
							recommendations = found;
							run.finding = false;
							chooser?.setRecommendations(found);
							reveal();
						});
				}
				const choice = await choosing;
				stopFinding.abort();
				run.choosing = false;
				run.finding = false;
				run.handle = undefined;
				run.updateStatus();
				if (!choice || !alive()) return;
				stream = choice.stream;
				created = choice.created;
			}
			if (binding?.slug !== stream.slug || binding.path !== stream.path) {
				if (binding && !switchConfirmed && readPending(agentDir, run.ownerId, binding.slug)) {
					if (!await ctx.ui.confirm("Switch checkpoint stream?", `Unfinished work for ${binding.slug} will remain saved.`, { signal: run.cancellation.signal }) || !alive()) return;
				}
				pi.appendEntry(BINDING_TYPE, { ownerId: run.ownerId, slug: stream.slug, path: stream.path });
			}
			if (created && sharedConfig.cmux.enabled) {
				try { await cli.project(stream.slug); }
				catch (error) { ctx.ui.notify(`CMUX update failed: ${String(error)}`, "warning"); }
			}
			if (run.select) {
				reported = true;
				pi.sendMessage(streamChosenReceipt({ slug: stream.slug, path: stream.path, created }), { triggerTurn: false });
				return;
			}
			const model = configuredModel(ctx, config.drafterModel, "drafter");
			if (!model) throw new Error("Select a model before starting a checkpoint conversation");
			const runtime = testDeps.runtime ?? await ModelRuntime.create({ authPath: join(agentDir, "auth.json"), modelsPath: join(agentDir, "models.json") });
			const providerConfig = ctx.modelRegistry.getRegisteredProviderConfig(model.provider);
			if (providerConfig) runtime.registerProvider(model.provider, providerConfig);
			const nativeProvider = ctx.modelRegistry.getRegisteredNativeProvider(model.provider);
			if (nativeProvider) runtime.registerNativeProvider(nativeProvider);
			const skill = testDeps.skill ?? readFileSync(fileURLToPath(new URL("../../skills/streams/SKILL.md", import.meta.url)), "utf8");
			const format = await cli.format();
			if (!alive()) return;
			const selected = stream;
			run.slug = selected.slug;
			const systemPrompt = `${run.systemPrompt}\n\n<checkpoint_rules>\nYou are a separate checkpoint conversation. Read and discuss stream state; propose only complete scratch Markdown through propose_checkpoint. Do not claim to have written scratch. You have only local read-only tools and the proposal tool. The user alone approves Apply. Every proposal must include pi:${run.ownerId} in frontmatter sessions (preserve ordering), never the checkpoint session ID. After proposing a draft, summarize only substantive changes since the previous scratch note in one or two sentences. Do not announce staging, routine verification, or that scratch was not changed; mention material failures or blockers. Annotations identify an exact draft revision and original/proposed document line, with a quoted excerpt. Treat them as feedback on that snapshot, not as coordinates in a later revision.\n\n${skill}\n\n${format}\n</checkpoint_rules>`;
			const pending = readPending(agentDir, run.ownerId, selected.slug) ?? { version: 1 as const, ownerId: run.ownerId, slug: selected.slug, messages: [], input: "" };
			let previousActivity: CheckpointConversation["state"]["activity"] = null;
			const conversation = new CheckpointConversation({ pending, binding: { ownerId: run.ownerId, slug: selected.slug, path: selected.path },
				agentDir, cwd: ctx.cwd, manager: ctx.sessionManager, runtime, model, thinkingLevel: ctx.thinkingLevel, systemPrompt, cli,
				changed: () => {
					if (!alive() || run.finalized) return;
					const state = conversation.state;
					mark({ label: state.activity === "drafting" ? "Drafting" : checkpointLabel(state), working: !!state.activity, slug: selected.slug, detail: state.notice?.text });
					const completed = previousActivity === "drafting" && state.activity === null;
					previousActivity = state.activity;
					const last = (pending.messages as AgentMessage[]).at(-1);
					if (completed && state.notice?.level !== "error" && last?.role === "assistant" && last.stopReason === "stop" && ctx.ui.getEditorText() === "") show(run);
					run.panel?.refresh(); run.updateStatus?.();
				},
				applied: (summary, cmux, warning) => {
					if (alive()) {
						run.finalized = true;
						mark({ label: "Applied", slug: selected.slug, closed: true });
						const discussion = discussionText(conversation.state.messages) || undefined;
						pi.sendMessage(checkpointReceipt({ slug: selected.slug, path: selected.path, summary, cmux, warning, commands: cli.diagnostics, discussion }), { triggerTurn: false });
					}
					run.dismiss?.();
				},
			});
			run.conversation = conversation;
			await ctx.ui.custom<void>((tui, theme, _keys, done) => {
				run.dismiss = done;
				run.panel = new CheckpointPanel(tui, theme, conversation.state, {
					send: () => { void conversation.send(); },
					apply: () => { void conversation.apply(); },
					refresh: () => {
						run.snapshot = checkpointContext(ctx.sessionManager);
						run.marker = journal.begin(run.ownerId, run.snapshot);
						void conversation.refreshContext(run.snapshot);
					},
					stop: () => { void conversation.stop(); },
					clear: () => {
						conversation.clear(); run.finalized = true;
						if (conversation.state.messages.some((message) => message.role === "user")) {
							pi.sendMessage(clearedReceipt({ slug: selected.slug, path: selected.path, discussion: discussionText(conversation.state.messages) }), { triggerTurn: false });
						}
						mark({ label: "Cleared", slug: selected.slug, closed: true });
						done();
					},
					focusMain: () => { run.panel?.dispose(); run.handle?.setHidden(true); run.updateStatus?.(); },
					changed: () => conversation.persist(),
				}, focusShortcut);
				return run.panel;
			}, { overlay: true, overlayOptions: { anchor: "right-center", width: "64%", minWidth: 60, margin: { right: 1 }, maxHeight: 36 },
				onHandle: (handle) => {
					run.handle = handle;
					if (conversation.isFresh || run.author) handle.setHidden(true);
					void conversation.start(message, run.snapshot, run.author);
					run.updateStatus?.();
				} });
		} catch (error) {
			if (alive()) {
				run.finalized = true;
				mark({ label: "Needs attention", detail: String(error), closed: true });
				ctx.ui.notify(`Checkpoint: ${String(error)}`, "error");
			}
		} finally {
			await run.conversation?.dispose();
			if (!run.finalized && ctx.sessionManager.getSessionId() === run.ownerId) mark({ label: run.conversation ? "Saved for resume" : "Cancelled", closed: true });
			// The agent was told a receipt follows its select_stream call, so report a choice that ended any other way.
			if (run.select && !reported && alive()) pi.sendMessage(choiceCancelledReceipt(latestBinding(ctx.sessionManager.getEntries(), run.ownerId)), { triggerTurn: false });
			run.panel?.dispose();
			ctx.ui.setStatus("checkpoint", undefined);
			if (active === run) active = undefined;
		}
	}

	if (loadConfig(agentDir).agentTools) {
		pi.registerTool({
			name: "checkpoint",
			label: "Checkpoint",
			description: "Start a checkpoint of this session's bound stream in the background, like the user's /checkpoint command. A separate checkpoint agent drafts an update to the stream's scratch note from this conversation, and the user reviews and applies it. Returns at once with a status: started, needs-stream, already-open, applying or awaiting-stream-choice. When the user applies or clears the checkpoint, a receipt with the discussion arrives in this conversation.",
			promptSnippet: "Start a background checkpoint of the bound stream for the user to review",
			promptGuidelines: [
				"Use checkpoint occasionally, at natural points: after a significant decision, or when the task changes substantially. Do not use it after routine steps.",
				"checkpoint returns at once; do not wait for it or call it again to check progress. Put everything for one checkpoint in a single call's instructions.",
			],
			parameters: Type.Object({
				instructions: Type.Optional(Type.String({ description: "What the checkpoint agent should focus on, such as decisions to record. The user sees these, labelled as coming from the main agent." })),
			}),
			executionMode: "sequential",
			async execute(toolCallId, params, _signal, _onUpdate, ctx) {
				if (ctx.mode !== "tui" || !ctx.hasUI) throw new Error("checkpoint requires an interactive Pi terminal");
				const message = params.instructions?.trim() ?? "";
				const busy = active;
				if (busy) {
					if (busy.choosing) return toolResult("awaiting-stream-choice", "The user is choosing a stream. Instructions were not sent.", { instructionsSent: false });
					const slug = busy.slug;
					if (!slug || !busy.conversation) return toolResult("already-open", "A checkpoint is starting. Instructions were not sent.", { instructionsSent: false });
					if (busy.conversation.state.activity === "applying") return toolResult("applying", `The checkpoint for ${slug} is being applied. Instructions were not sent.`, { slug, instructionsSent: false });
					if (!message) return toolResult("already-open", `A checkpoint for ${slug} is already open.`, { slug, instructionsSent: false });
					if (!busy.conversation.acceptsInstructions) {
						const reason = busy.conversation.state.activity ? "the checkpoint agent is busy" : "the user has unsent feedback";
						return toolResult("already-open", `A checkpoint for ${slug} is already open. Instructions were not sent, because ${reason}.`, { slug, instructionsSent: false });
					}
					void busy.conversation.send(message, "main-agent");
					return toolResult("already-open", `A checkpoint for ${slug} is already open. Instructions were sent to the checkpoint agent.`, { slug, instructionsSent: true });
				}
				const ownerId = ctx.sessionManager.getSessionId();
				const binding = latestBinding(ctx.sessionManager.getEntries(), ownerId);
				const stream = binding ? await selectStream("", binding, testDeps.cli ?? new StreamCli(ctx.cwd)) : undefined;
				if (!stream) {
					return toolResult("needs-stream", `${binding ? `The bound stream ${binding.slug} no longer resolves.` : "No stream is bound to this session."} Call select_stream so the user can choose one, and checkpoint after a receipt confirms the binding.`, { instructionsSent: false });
				}
				const saved = readPending(agentDir, ownerId, stream.slug);
				const blocked = !!message && !!(saved?.input.trim() || saved?.annotations?.length || saved?.annotationEditor);
				const snapshot = checkpointContext(ctx.sessionManager, toolCallId);
				const run: OpenCheckpoint = { ownerId, snapshot, systemPrompt: ctx.getSystemPrompt(), marker: journal.begin(ownerId, snapshot, message, "main-agent"),
					cancellation: new AbortController(), author: "main-agent" };
				active = run;
				run.task = open(run, stream.slug, message, ctx, stream);
				const sent = message ? blocked ? " Instructions were not sent, because the user has unsent feedback." : " Instructions were sent." : "";
				return toolResult("started", `Checkpoint started for ${stream.slug} in the background.${sent} The user reviews and applies it; a receipt arrives here when they apply or clear it. Do not wait for it.`,
					{ slug: stream.slug, instructionsSent: !!message && !blocked });
			},
		});

		pi.registerTool({
			name: "select_stream",
			label: "Select stream",
			description: "Ask the user to choose the stream this session works on, or to switch to another. Opens the stream chooser in the background with your recommendation starred; only the user's choice binds the session. Returns at once with a status: awaiting-stream-choice, already-bound or already-open. A receipt arrives here when the user chooses or cancels.",
			promptSnippet: "Ask the user to choose or switch this session's stream",
			promptGuidelines: [
				"Use select_stream when substantive work has no bound stream, or when the work has clearly moved to a different stream. Do not use it to re-confirm the current stream.",
				"select_stream returns at once and the user may choose later; do not wait for it. Call checkpoint only after a receipt confirms the binding.",
			],
			parameters: Type.Object({
				stream: Type.Optional(Type.String({ description: "Recommended stream: an existing stream's slug, path or title, or a short title for a new stream. Omit to let the chooser suggest streams." })),
				reason: Type.Optional(Type.String({ description: "One sentence on why the recommended stream fits, shown beside it in the chooser." })),
			}),
			executionMode: "sequential",
			async execute(toolCallId, params, _signal, _onUpdate, ctx) {
				if (ctx.mode !== "tui" || !ctx.hasUI) throw new Error("select_stream requires an interactive Pi terminal");
				const busy = active;
				if (busy?.choosing) return toolResult("awaiting-stream-choice", "The user is already choosing a stream.", {});
				if (busy) return toolResult("already-open", `A checkpoint${busy.slug ? ` for ${busy.slug}` : ""} is open. Its stream can change after the user applies or clears it.`, busy.slug ? { slug: busy.slug } : {});
				const ownerId = ctx.sessionManager.getSessionId();
				const binding = latestBinding(ctx.sessionManager.getEntries(), ownerId);
				const ref = params.stream?.trim() ?? "";
				const reason = `Main agent: ${params.reason?.trim() || "recommended for this conversation"}`;
				let recommendations: Recommendations | undefined;
				if (ref) {
					// Anything that doesn't resolve is offered as a new title; the user confirms either way.
					const existing = await (testDeps.cli ?? new StreamCli(ctx.cwd)).show(ref).catch(() => undefined);
					if (existing && binding?.slug === existing.slug && binding.path === existing.path) return toolResult("already-bound", `This session is already bound to ${existing.slug}.`, { slug: existing.slug });
					recommendations = existing ? { existing: [{ slug: existing.slug, reason }] } : { existing: [], newStream: { title: ref, reason } };
				}
				const run: OpenCheckpoint = { ownerId, snapshot: checkpointContext(ctx.sessionManager, toolCallId), systemPrompt: ctx.getSystemPrompt(),
					cancellation: new AbortController(), author: "main-agent", select: { recommendations }, choosing: true, finding: !recommendations };
				active = run;
				run.task = open(run, "", "", ctx);
				return toolResult("awaiting-stream-choice", "The stream chooser is open for the user. It appears once ready, unless they are typing. A receipt arrives here when they choose or cancel. Do not wait for it.", {});
			},
		});
	}

	pi.registerCommand("checkpoint", {
		description: "Review a checkpoint: /checkpoint [--stream <ref>] [instructions]",
		handler: async (args, ctx) => {
			if (ctx.mode !== "tui" || !ctx.hasUI) { ctx.ui.notify("/checkpoint requires an interactive Pi terminal", "error"); return; }
			let input: ReturnType<typeof parseCheckpointArgs>;
			try { input = parseCheckpointArgs(args); }
			catch (error) { ctx.ui.notify(String(error), "error"); return; }
			const { ref, message } = input;
			const previous = active;
			if (previous && (!ref || !previous.handle || previous.conversation?.state.activity === "applying")) {
				if (previous.handle) show(previous);
				if (message) {
					if (!ref && previous.conversation) void previous.conversation.send(message);
					else ctx.ui.notify("Checkpoint is busy; instructions were not sent. Retry after the current operation.", "warning");
				} else if (!previous.handle) ctx.ui.notify("A checkpoint is starting", "info");
				return;
			}
			const ownerId = ctx.sessionManager.getSessionId();
			const snapshot = checkpointContext(ctx.sessionManager);
			const run: OpenCheckpoint = { ownerId, snapshot, systemPrompt: ctx.getSystemPrompt(), marker: journal.begin(ownerId, snapshot, message), cancellation: new AbortController() };
			active = run;
			// Returning releases Pi's serialized prompt queue, even while the overlay is hidden.
			run.task = (async () => {
				if (previous) {
					previous.cancellation.abort();
					previous.dismiss?.();
					await previous.task;
				}
				await open(run, ref, message, ctx);
			})();
		},
	});
}
