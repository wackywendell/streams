import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { OverlayHandle } from "@earendil-works/pi-tui";
import { convertToLlm, getAgentDir, ModelRuntime } from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ExtensionCommandContext, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { checkpointContext } from "./agent.ts";
import { BINDING_TYPE, exactStream, latestBinding, loadConfig, savePromptingConfig, parseRecommendations, readPending, selectStream } from "./core.ts";
import type { Recommendations } from "./core.ts";
import { CheckpointPanel } from "./panel.ts";
import { StreamChooser } from "./chooser.ts";
import type { ChosenStream } from "./chooser.ts";
import { CheckpointConversation } from "./conversation.ts";
import { checkpointLabel, checkpointStatus, shortcutLabel } from "./view.ts";
import { CheckpointJournal, MARKER_TYPE } from "./journal.ts";
import { StreamCli, type RefreshOutcome } from "./streams.ts";
import { checkpointReceipt, choiceCancelledReceipt, clearedReceipt, discussionText, renderCheckpointReceipt, streamChosenReceipt } from "./receipt.ts";
import { streamReadTool } from "./read.ts";
import type { Author } from "./review.ts";
import { promptingPolicy } from "./policy.ts";
import { CheckpointSettings } from "./settings.ts";

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

/** The overlay a phase shows. `dismiss` exists once the overlay is built, and `handle` once Pi has placed it. */
type Overlay = { handle?: OverlayHandle; dismiss?: () => void };
type SettingsOverlay = Overlay & { ownerId: string; closing: boolean; closed: Promise<void> };
type Phase =
	/** Looking up streams or asking a question; no overlay of the run's own is open. */
	| { kind: "starting" }
	| ({ kind: "choosing"; finding: boolean } & Overlay)
	| ({ kind: "drafting"; slug: string; conversation: CheckpointConversation; panel?: CheckpointPanel } & Overlay);

/**
 * One open checkpoint, or one stream choice for select_stream or create_stream. A selection run binds a stream
 * and ends; it has no journal card, since its receipt is the record.
 */
type Run = {
	kind: "checkpoint" | "selection";
	ctx: ExtensionContext;
	ownerId: string;
	snapshot: ReturnType<typeof checkpointContext>;
	systemPrompt: string;
	cancellation: AbortController;
	phase: Phase;
	/** Automatic reveal is allowed only until the user opens or minimizes this run. */
	visibility: "auto" | "visible" | "minimized";
	marker?: string;
	/** Set once the run has reported its own end, so it is not also marked saved or cancelled. */
	finalized?: boolean;
	task?: Promise<void>;
};

/** A promise that rejects once `signal` aborts. */
function untilAborted(signal: AbortSignal): Promise<never> {
	return new Promise((_resolve, reject) => {
		const stop = () => reject(new Error("Checkpoint cancelled"));
		if (signal.aborted) stop();
		else signal.addEventListener("abort", stop, { once: true });
	});
}

export default function registerCheckpoint(pi: ExtensionAPI, testDeps: { agentDir?: string; cli?: StreamCli; runtime?: ModelRuntime; skill?: string } = {}): void {
	let active: Run | undefined;
	let settings: SettingsOverlay | undefined;
	pi.registerMessageRenderer("checkpoint-receipt", renderCheckpointReceipt);
	const agentDir = testDeps.agentDir ?? getAgentDir();
	const config = loadConfig(agentDir);
	const focusShortcut = config.focusShortcut;
	if (config.agentTools) pi.on("before_agent_start", (event) => {
		event.systemPromptOptions.sections.stream_prompting_policy = promptingPolicy(loadConfig(agentDir));
	});
	const journal = new CheckpointJournal(pi, focusShortcut);
	pi.registerEntryRenderer(MARKER_TYPE, journal.render);
	pi.registerTool(streamReadTool(agentDir, (ctx) => testDeps.cli ?? new StreamCli(ctx.cwd)));
	pi.on("session_start", (_event, ctx) => journal.restore(ctx.sessionManager.getEntries()));

	const newRun = (kind: Run["kind"], ctx: ExtensionContext, snapshot: Run["snapshot"], marker?: string): Run =>
		({ kind, ctx, ownerId: ctx.sessionManager.getSessionId(), snapshot, systemPrompt: ctx.getSystemPrompt(), cancellation: new AbortController(), phase: { kind: "starting" }, visibility: "auto", marker });
	const alive = (run: Run) => !run.cancellation.signal.aborted && run.ctx.sessionManager.getSessionId() === run.ownerId;
	const mark = (run: Run, status: Parameters<typeof journal.update>[1]) => { if (run.marker) journal.update(run.marker, status); };
	/** Lookups take no focus, and cancellation abandons them at once, so shutdown never waits on a slow CLI call. */
	const whileOpen = <T>(run: Run, work: Promise<T>) => Promise.race([work, untilAborted(run.cancellation.signal)]);
	const closeSettings = (editor: SettingsOverlay) => {
		editor.closing = true;
		// Pi's custom-UI completion pops the stack, so wait until this overlay has actually been placed.
		if (editor.handle) editor.dismiss?.();
		return editor.closed;
	};
	const finishOverlay = (run: Run, done: () => void) => {
		if (settings?.ownerId === run.ownerId) void closeSettings(settings).then(done);
		else done();
	};
	const waitForSettings = async (run: Run) => {
		if (settings?.ownerId === run.ownerId) await whileOpen(run, settings.closed);
	};
	const updateStatus = (run: Run) => {
		const phase = run.phase;
		const value = phase.kind === "starting" || !phase.handle?.isHidden() ? undefined
			: phase.kind === "drafting" ? checkpointStatus(phase.conversation.state, focusShortcut)
			: `Checkpoint · ${phase.finding ? "Finding streams" : "Choose stream"} · ${shortcutLabel(focusShortcut)}`;
		run.ctx.ui.setStatus("checkpoint", value ? run.ctx.ui.theme.fg("accent", value) : undefined);
	};
	const minimize = (run: Run) => {
		run.visibility = "minimized";
		const phase = run.phase;
		if (phase.kind === "starting") return;
		phase.handle?.setHidden(true);
		if (phase.kind === "drafting") phase.panel?.dispose();
		updateStatus(run);
	};
	const show = (run: Run) => {
		if (settings?.ownerId === run.ownerId) { settings.handle?.focus(); return; }
		const phase = run.phase;
		if (phase.kind === "starting") return;
		run.visibility = "visible";
		phase.handle?.setHidden(false);
		phase.handle?.focus();
		updateStatus(run);
		if (phase.kind === "drafting") phase.panel?.refresh();
	};
	async function showSettings(ctx: ExtensionContext): Promise<void> {
		if (!ctx.hasUI) { ctx.ui.notify("Checkpoint settings require an interactive UI", "error"); return; }
		if (settings) { settings.handle?.focus(); return; }
		const parent = active?.ownerId === ctx.sessionManager.getSessionId() ? active : undefined;
		const phase = parent?.phase;
		const wasVisible = phase && phase.kind !== "starting" && phase.handle && !phase.handle.isHidden();
		let resolveClosed!: () => void;
		const editor: SettingsOverlay = { ownerId: ctx.sessionManager.getSessionId(), closing: false, closed: new Promise((resolve) => { resolveClosed = resolve; }) };
		try {
			const current = loadConfig(agentDir);
			settings = editor;
			if (wasVisible && phase && phase.kind !== "starting") phase.handle?.setHidden(true);
			await ctx.ui.custom<boolean>((tui, theme, _keys, done) => {
				editor.dismiss = () => done(false);
				return new CheckpointSettings(tui, theme, current, (policy) => {
					if (settings !== editor || editor.closing || ctx.sessionManager.getSessionId() !== editor.ownerId) throw new Error("Settings session has closed");
					savePromptingConfig(agentDir, policy);
				}, done);
			}, { overlay: true, overlayOptions: { anchor: "center", width: "78%", minWidth: 60, margin: { left: 2, right: 2 }, maxHeight: 36 }, onHandle: (handle) => { editor.handle = handle; if (editor.closing) editor.dismiss?.(); } });
		} catch (error) { ctx.ui.notify(`Checkpoint settings: ${String(error)}`, "error"); }
		finally {
			if (settings === editor) settings = undefined;
			if (!editor.closing && wasVisible && parent && parent === active && parent.phase === phase && alive(parent)) show(parent);
			resolveClosed();
		}
	}

	pi.registerCommand("checkpoint:settings", { description: "Configure agent stream selection and checkpoint prompting", handler: (_args, ctx) => { void showSettings(ctx); } });

	const confirmSwitch = (run: Run, slug: string) =>
		run.ctx.ui.confirm("Switch checkpoint stream?", `Unfinished work for ${slug} will remain saved.`, { signal: run.cancellation.signal });

	pi.registerShortcut(focusShortcut, {
		description: "Show the open checkpoint panel or stream chooser",
		handler: (ctx) => {
			if (active && active.phase.kind !== "starting" && active.phase.handle && active.ownerId === ctx.sessionManager.getSessionId()) show(active);
			else ctx.ui.notify("No checkpoint panel is open; run /checkpoint", "info");
		},
	});
	pi.on("session_shutdown", async () => {
		journal.dispose();
		const run = active;
		run?.cancellation.abort();
		if (settings) await closeSettings(settings);
		if (!run) return;
		if (run.phase.kind !== "starting") run.phase.dismiss?.();
		await run.task;
	});

	/** Runs `work` to the run's end: reports a failure, closes the journal card, and releases the run. */
	async function guarded(run: Run, work: (cli: StreamCli) => Promise<void>): Promise<void> {
		try {
			if (!alive(run)) return;
			await work(testDeps.cli ?? new StreamCli(run.ctx.cwd, undefined, (text) => { if (alive(run)) run.ctx.ui.notify(text, "warning"); }));
		} catch (error) {
			if (alive(run)) {
				run.finalized = true;
				mark(run, { label: "Needs attention", detail: String(error), closed: true });
				run.ctx.ui.notify(`Checkpoint: ${String(error)}`, "error");
			}
		} finally {
			if (!run.finalized && run.ctx.sessionManager.getSessionId() === run.ownerId) mark(run, { label: run.phase.kind === "drafting" ? "Saved for resume" : "Cancelled", closed: true });
			run.ctx.ui.setStatus("checkpoint", undefined);
			if (active === run) active = undefined;
		}
	}

	/**
	 * Shows the chooser and waits for the user's choice. It opens hidden, and is revealed once recommendations are ready,
	 * unless the user is typing in the main editor. Without `recommendations`, the selector model finds them.
	 */
	async function choose(run: Run, cli: StreamCli, streams: Stream[], options: { ref?: string; recommendations?: Recommendations; reason?: string; create?: boolean }): Promise<ChosenStream | undefined> {
		await waitForSettings(run);
		if (!alive(run)) return undefined;
		const { ctx } = run;
		mark(run, { label: "Choosing stream", working: true });
		let recommendations = options.recommendations;
		const selectorModel = recommendations ? undefined : configuredModel(ctx, loadConfig(agentDir).selectorModel, "selector");
		const phase: Extract<Phase, { kind: "choosing" }> = { kind: "choosing", finding: !recommendations };
		run.phase = phase;
		let chooser: StreamChooser | undefined;
		const reveal = () => {
			if (run.phase !== phase || !phase.handle || !recommendations || !alive(run) || settings?.ownerId === run.ownerId) return;
			if (run.visibility === "auto" && ctx.ui.getEditorText() === "") show(run);
			else updateStatus(run);
		};
		const choosing = ctx.ui.custom<ChosenStream | undefined>((tui, theme, _keys, done) => {
			journal.attach(tui);
			const finish = (choice: ChosenStream | undefined) => finishOverlay(run, () => done(choice));
			phase.dismiss = () => finish(undefined);
			chooser = new StreamChooser(tui, theme, streams, cli, finish, recommendations ?? { existing: [] }, options.ref ?? "",
				{ shortcut: focusShortcut, minimize: () => minimize(run), settings: () => { void showSettings(ctx); }, finding: phase.finding, busyChanged: (working) => { if (alive(run) && run.phase === phase) mark(run, { label: "Choosing stream", working }); }, reason: options.reason, create: options.create, drafting: run.kind === "checkpoint" });
			return chooser;
		}, { overlay: true, overlayOptions: { anchor: "center", width: "78%", minWidth: 64, margin: 2, maxHeight: "80%" },
			onHandle: (handle) => { phase.handle = handle; handle.setHidden(run.visibility !== "visible"); updateStatus(run); reveal(); } });
		const stopFinding = new AbortController();
		if (!recommendations) {
			void recommendStreams(ctx, streams, selectorModel, run.snapshot, AbortSignal.any([run.cancellation.signal, stopFinding.signal]))
				.catch(() => ({ existing: [] })).then((found) => {
					if (stopFinding.signal.aborted) return;
					recommendations = found;
					phase.finding = false;
					chooser?.setRecommendations(found);
					reveal();
				});
		}
		try { return await choosing; }
		finally {
			stopFinding.abort();
			run.phase = { kind: "starting" };
			updateStatus(run);
		}
	}

	/**
	 * Binds the session to `stream`, asking first if that leaves unfinished work for the bound stream, unless the user
	 * already agreed to switch. Returns false if the user declines or the run ended.
	 */
	async function bind(run: Run, cli: StreamCli, stream: Stream, created: boolean, switchConfirmed: boolean): Promise<{ refresh?: RefreshOutcome } | false> {
		const binding = latestBinding(run.ctx.sessionManager.getEntries(), run.ownerId);
		if (binding?.slug !== stream.slug || binding.path !== stream.path) {
			if (binding && !switchConfirmed && readPending(agentDir, run.ownerId, binding.slug)) {
				if (!await confirmSwitch(run, binding.slug) || !alive(run)) return false;
			}
			pi.appendEntry(BINDING_TYPE, { ownerId: run.ownerId, slug: stream.slug, path: stream.path });
		}
		if (!created) return {};
		const refresh = await cli.refresh(stream.slug);
		if (refresh.status === "selection-failed" || refresh.status === "backend-failed") {
			run.ctx.ui.notify("Stream bound; optional workspace refresh failed.", "warning");
		}
		return { refresh };
	}

	/**
	 * Opens the checkpoint conversation for the bound `stream` and waits until its panel closes. Inherit any explicit
	 * chooser visibility; otherwise start hidden when `startHidden` is set or until a fresh conversation's first reply.
	 * `author` marks `message` as not the user's.
	 */
	async function draft(run: Run, cli: StreamCli, stream: Stream, message: string, options: { author?: Author; startHidden: boolean }): Promise<void> {
		const { ctx } = run;
		const model = configuredModel(ctx, loadConfig(agentDir).drafterModel, "drafter");
		if (!model) throw new Error("Select a model before starting a checkpoint conversation");
		const runtime = testDeps.runtime ?? await ModelRuntime.create({ authPath: join(agentDir, "auth.json"), modelsPath: join(agentDir, "models.json") });
		const providerConfig = ctx.modelRegistry.getRegisteredProviderConfig(model.provider);
		if (providerConfig) runtime.registerProvider(model.provider, providerConfig);
		const nativeProvider = ctx.modelRegistry.getRegisteredNativeProvider(model.provider);
		if (nativeProvider) runtime.registerNativeProvider(nativeProvider);
		const skill = testDeps.skill ?? readFileSync(fileURLToPath(new URL("../../skills/streams/SKILL.md", import.meta.url)), "utf8");
		const format = await cli.format();
		await waitForSettings(run);
		if (!alive(run)) return;
		const { slug } = stream;
		const systemPrompt = `${run.systemPrompt}\n\n<checkpoint_rules>\nYou are a separate checkpoint conversation. Read and discuss stream state; propose only complete scratch Markdown through propose_checkpoint. Do not claim to have written scratch. You have only local read-only tools and the proposal tool. The user alone approves Apply. Every proposal must include pi:${run.ownerId} in frontmatter sessions (preserve ordering), never the checkpoint session ID. After proposing a draft, summarize only substantive changes since the previous scratch note in one or two sentences. Do not announce staging, routine verification, or that scratch was not changed; mention material failures or blockers. Annotations identify an exact draft revision and original/proposed document line, with a quoted excerpt. Treat them as feedback on that snapshot, not as coordinates in a later revision.\n\n${skill}\n\n${format}\n</checkpoint_rules>`;
		const pending = readPending(agentDir, run.ownerId, slug) ?? { version: 1 as const, ownerId: run.ownerId, slug, messages: [], input: "" };
		let previousActivity: CheckpointConversation["state"]["activity"] = null;
		const conversation: CheckpointConversation = new CheckpointConversation({ pending, binding: { ownerId: run.ownerId, slug, path: stream.path },
			agentDir, cwd: ctx.cwd, manager: ctx.sessionManager, runtime, model, thinkingLevel: ctx.thinkingLevel, systemPrompt, cli,
			changed: () => {
				if (!alive(run) || run.finalized) return;
				const state = conversation.state;
				mark(run, { label: state.activity === "drafting" ? "Drafting" : checkpointLabel(state), working: !!state.activity, slug, detail: state.notice?.text });
				const completed = previousActivity === "drafting" && state.activity === null;
				previousActivity = state.activity;
				const last = (pending.messages as AgentMessage[]).at(-1);
				if (completed && run.visibility === "auto" && state.notice?.level !== "error" && last?.role === "assistant" && last.stopReason === "stop" && ctx.ui.getEditorText() === "") show(run);
				phase.panel?.refresh(); updateStatus(run);
			},
			applied: (summary, refresh) => {
				if (alive(run)) {
					run.finalized = true;
					mark(run, { label: "Applied", slug, closed: true });
					const discussion = discussionText(conversation.state.messages) || undefined;
					pi.sendMessage(checkpointReceipt({ slug, summary, refresh, commands: cli.diagnostics, discussion }), { triggerTurn: false });
				}
				phase.dismiss?.();
			},
		});
		const phase: Extract<Phase, { kind: "drafting" }> = { kind: "drafting", slug, conversation };
		run.phase = phase;
		try {
			await ctx.ui.custom<void>((tui, theme, _keys, done) => {
				journal.attach(tui);
				phase.dismiss = () => finishOverlay(run, done);
				phase.panel = new CheckpointPanel(tui, theme, conversation.state, {
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
							pi.sendMessage(clearedReceipt({ slug, discussion: discussionText(conversation.state.messages) }), { triggerTurn: false });
						}
						mark(run, { label: "Cleared", slug, closed: true });
						phase.dismiss?.();
					},
					focusMain: () => minimize(run),
					settings: () => { void showSettings(ctx); },
					editFeedback: (change) => { conversation.editFeedback(change); },
				}, focusShortcut);
				return phase.panel;
			}, { overlay: true, overlayOptions: { anchor: "right-center", width: "64%", minWidth: 60, margin: { right: 1 }, maxHeight: 36 },
				onHandle: (handle) => {
					phase.handle = handle;
					if (run.visibility === "visible" || (run.visibility === "auto" && !conversation.isFresh && !options.startHidden)) show(run);
					else handle.setHidden(true);
					void conversation.start(message, run.snapshot, options.author);
					updateStatus(run);
				} });
		} finally {
			await conversation.dispose();
			phase.panel?.dispose();
		}
	}

	/** /checkpoint: drafts for `ref`, or the bound stream, letting the user choose when neither resolves. */
	function checkpointCommand(run: Run, ref: string, message: string): Promise<void> {
		return guarded(run, async (cli) => {
			const streams = await whileOpen(run, cli.list());
			const binding = latestBinding(run.ctx.sessionManager.getEntries(), run.ownerId);
			let stream = await whileOpen(run, ref ? cli.show(ref).catch((error) => { if (alive(run)) run.ctx.ui.notify(String(error), "warning"); return undefined; })
				: selectStream("", binding, { list: async () => streams, show: (value) => cli.show(value) }));
			if (!alive(run)) return;
			let created = false;
			let switchConfirmed = false;
			if (!stream) {
				if (binding && readPending(agentDir, run.ownerId, binding.slug)) {
					switchConfirmed = await confirmSwitch(run, binding.slug);
					if (!switchConfirmed || !alive(run)) return;
				}
				const choice = await choose(run, cli, streams, { ref });
				if (!choice || !alive(run)) return;
				({ stream, created } = choice);
				mark(run, { label: "Starting", working: true, slug: stream.slug });
			}
			if (!await bind(run, cli, stream, created, switchConfirmed)) return;
			await draft(run, cli, stream, message, { startHidden: false });
		});
	}

	/**
	 * select_stream and create_stream: the user chooses a stream, and the run ends once it is bound. The chooser never
	 * asks up front, since the agent opened it; choosing a stream that leaves unfinished work still asks.
	 */
	async function streamSelection(run: Run, options: Parameters<typeof choose>[3]): Promise<void> {
		let reported = false;
		await guarded(run, async (cli) => {
			const streams = await whileOpen(run, cli.list());
			if (!alive(run)) return;
			const choice = await choose(run, cli, streams, options);
			if (!choice || !alive(run)) return;
			const bound = await bind(run, cli, choice.stream, choice.created, false);
			if (!bound) return;
			reported = true;
			pi.sendMessage(streamChosenReceipt({ slug: choice.stream.slug, created: choice.created, refresh: bound.refresh, commands: cli.diagnostics }), { triggerTurn: false });
		});
		// The agent was told a receipt follows its call, so report a choice that ended any other way.
		if (!reported && alive(run)) pi.sendMessage(choiceCancelledReceipt(latestBinding(run.ctx.sessionManager.getEntries(), run.ownerId)), { triggerTurn: false });
	}

	if (config.agentTools) {
		pi.registerTool({
			name: "checkpoint",
			label: "Checkpoint",
			description: "Start a checkpoint of this session's bound stream in the background, like the user's /checkpoint command. A separate checkpoint agent drafts an update to the stream's scratch note from this conversation, and the user reviews and applies it. Returns at once with a status: started, needs-stream, already-open, applying or awaiting-stream-choice. When the user applies or clears the checkpoint, a receipt with the discussion arrives in this conversation.",
			promptSnippet: "Start a background checkpoint of the bound stream for the user to review",
			promptGuidelines: [
				"Follow stream_prompting_policy for when to initiate checkpoint. Only the user can apply the draft.",
				"checkpoint returns at once; do not wait for it or call it again to check progress. Call it once per checkpoint.",
				"The checkpoint agent sees this whole conversation. Usually call checkpoint with no instructions. Add a sentence only for something it couldn't infer, such as which decision matters most or what to leave out.",
			],
			parameters: Type.Object({
				instructions: Type.Optional(Type.String({ description: "Optional, usually omitted. One or two sentences on focus. Don't summarise the conversation; the checkpoint agent already has it. The user sees these, labelled as coming from the main agent." })),
			}),
			executionMode: "sequential",
			async execute(toolCallId, params, _signal, _onUpdate, ctx) {
				if (ctx.mode !== "tui" || !ctx.hasUI) throw new Error("checkpoint requires an interactive Pi terminal");
				const message = params.instructions?.trim() ?? "";
				const busy = active;
				if (busy) {
					if (busy.kind === "selection" || busy.phase.kind === "choosing") return toolResult("awaiting-stream-choice", "The user is choosing a stream. Instructions were not sent.", { instructionsSent: false });
					if (busy.phase.kind !== "drafting") return toolResult("already-open", "A checkpoint is starting. Instructions were not sent.", { instructionsSent: false });
					const { slug, conversation } = busy.phase;
					if (conversation.state.activity === "applying") return toolResult("applying", `The checkpoint for ${slug} is being applied. Instructions were not sent.`, { slug, instructionsSent: false });
					if (!message) return toolResult("already-open", `A checkpoint for ${slug} is already open.`, { slug, instructionsSent: false });
					if (!conversation.acceptsInstructions) {
						const reason = conversation.state.activity ? "the checkpoint agent is busy" : "the user has unsent feedback";
						return toolResult("already-open", `A checkpoint for ${slug} is already open. Instructions were not sent, because ${reason}.`, { slug, instructionsSent: false });
					}
					void conversation.send(message, "main-agent");
					return toolResult("already-open", `A checkpoint for ${slug} is already open. Instructions were sent to the checkpoint agent.`, { slug, instructionsSent: true });
				}
				const ownerId = ctx.sessionManager.getSessionId();
				const binding = latestBinding(ctx.sessionManager.getEntries(), ownerId);
				const stream = binding ? await selectStream("", binding, testDeps.cli ?? new StreamCli(ctx.cwd)) : undefined;
				if (!stream) {
					return toolResult("needs-stream", `${binding ? `The bound stream ${binding.slug} no longer resolves.` : "No stream is bound to this session."} Follow stream_prompting_policy: do not initiate stream selection merely to enable an automatic checkpoint. If the user requests selection or creation, use select_stream or create_stream and checkpoint only after a receipt confirms the binding.`, { instructionsSent: false });
				}
				const saved = readPending(agentDir, ownerId, stream.slug);
				const blocked = !!message && !!(saved?.input.trim() || saved?.annotations?.length || saved?.annotationEditor);
				const snapshot = checkpointContext(ctx.sessionManager, toolCallId);
				const run = newRun("checkpoint", ctx, snapshot, journal.begin(ownerId, snapshot, message, "main-agent"));
				active = run;
				run.task = guarded(run, (cli) => draft(run, cli, stream, message, { author: "main-agent", startHidden: true }));
				const sent = message ? blocked ? " Instructions were not sent, because the user has unsent feedback." : " Instructions were sent." : "";
				return toolResult("started", `Checkpoint started for ${stream.slug} in the background.${sent} The user reviews and applies it; a receipt arrives here when they apply or clear it. Do not wait for it.`,
					{ slug: stream.slug, instructionsSent: !!message && !blocked });
			},
		});

		/** The result for a select_stream or create_stream call made while a checkpoint or chooser is open. */
		const busyResult = () => {
			const busy = active;
			if (!busy) return undefined;
			if (busy.kind === "selection" || busy.phase.kind === "choosing") return toolResult("awaiting-stream-choice", "The user is already choosing a stream.", {});
			const slug = busy.phase.kind === "drafting" ? busy.phase.slug : undefined;
			return toolResult("already-open", `A checkpoint${slug ? ` for ${slug}` : ""} is open. Its stream can change after the user applies or clears it.`, slug ? { slug } : {});
		};
		const openChooser = (toolCallId: string, ctx: ExtensionContext, options: Parameters<typeof choose>[3]) => {
			const busy = busyResult();
			if (busy) return busy;
			const run = newRun("selection", ctx, checkpointContext(ctx.sessionManager, toolCallId));
			active = run;
			run.task = streamSelection(run, options);
			return toolResult("awaiting-stream-choice", "The stream chooser is open for the user. It appears once ready, unless they are typing. A receipt arrives here when they choose or cancel. Do not wait for it.", {});
		};
		const interactive = (ctx: ExtensionContext, name: string) => { if (ctx.mode !== "tui" || !ctx.hasUI) throw new Error(`${name} requires an interactive Pi terminal`); };
		const reasonParameter = Type.String({ minLength: 1, description: "One sentence on why, shown to the user in the chooser." });

		pi.registerTool({
			name: "select_stream",
			label: "Select stream",
			description: "Ask the user to choose the existing stream this session works on, or to switch to another. Opens the stream chooser in the background; with a slug, that stream is starred with your reason, and without one the chooser suggests streams. Only the user's choice binds the session. Returns at once with a status: awaiting-stream-choice, already-bound or already-open. A receipt arrives here when the user chooses or cancels.",
			promptSnippet: "Ask the user to choose or switch this session's stream",
			promptGuidelines: [
				"Follow stream_prompting_policy for when to initiate select_stream. Do not use it to re-confirm the current stream.",
				"select_stream takes an exact slug from `streams list`, or no slug to let the chooser suggest streams. Use create_stream to propose a new stream.",
				"select_stream returns at once and the user may choose later; do not wait for it. Call checkpoint only after a receipt confirms the binding.",
			],
			parameters: Type.Object({
				slug: Type.Optional(Type.String({ description: "Exact slug of the existing stream you recommend. Omit to let the chooser suggest streams." })),
				reason: reasonParameter,
			}),
			executionMode: "sequential",
			async execute(toolCallId, params, _signal, _onUpdate, ctx) {
				interactive(ctx, "select_stream");
				const busy = busyResult();
				if (busy) return busy;
				const reason = `Main agent: ${params.reason.trim()}`;
				const slug = params.slug?.trim();
				if (!slug) return openChooser(toolCallId, ctx, { reason });
				const stream = await exactStream(slug, testDeps.cli ?? new StreamCli(ctx.cwd));
				const binding = latestBinding(ctx.sessionManager.getEntries(), ctx.sessionManager.getSessionId());
				if (binding?.slug === stream.slug && binding.path === stream.path) return toolResult("already-bound", `This session is already bound to ${stream.slug}.`, { slug: stream.slug });
				return openChooser(toolCallId, ctx, { recommendations: { existing: [{ slug: stream.slug, reason }] } });
			},
		});

		pi.registerTool({
			name: "create_stream",
			label: "Create stream",
			description: "Ask the user to start a new stream for this session's work. Opens the stream chooser in the background, editing your title for the new stream; the user can change it and create the stream, or go back and choose an existing one. Only the user's choice creates a stream or binds the session. Returns at once with a status: awaiting-stream-choice or already-open. A receipt arrives here when the user chooses or cancels.",
			promptSnippet: "Ask the user to start a new stream for this session",
			promptGuidelines: [
				"Follow stream_prompting_policy for when to initiate create_stream. Once authorised, check `streams list` first and use select_stream if an existing stream fits.",
			],
			parameters: Type.Object({
				title: Type.String({ minLength: 1, description: "Short title for the new stream, usually 2–4 words. The user can edit it." }),
				reason: reasonParameter,
			}),
			executionMode: "sequential",
			async execute(toolCallId, params, _signal, _onUpdate, ctx) {
				interactive(ctx, "create_stream");
				const title = params.title.trim();
				if (!title) throw new Error("create_stream needs a title");
				const newStream = { title, reason: `Main agent: ${params.reason.trim()}` };
				return openChooser(toolCallId, ctx, { recommendations: { existing: [], newStream }, create: true });
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
			const shown = previous && previous.phase.kind !== "starting" && !!previous.phase.handle;
			const drafting = previous?.phase.kind === "drafting" ? previous.phase.conversation : undefined;
			if (previous && (!ref || !shown || drafting?.state.activity === "applying")) {
				if (shown) show(previous);
				if (message) {
					if (!ref && drafting) void drafting.send(message);
					else ctx.ui.notify("Checkpoint is busy; instructions were not sent. Retry after the current operation.", "warning");
				} else if (!shown) ctx.ui.notify("A checkpoint is starting", "info");
				return;
			}
			const snapshot = checkpointContext(ctx.sessionManager);
			const run = newRun("checkpoint", ctx, snapshot, journal.begin(ctx.sessionManager.getSessionId(), snapshot, message));
			active = run;
			// Returning releases Pi's serialized prompt queue, even while the overlay is hidden.
			run.task = (async () => {
				if (previous) {
					previous.cancellation.abort();
					if (previous.phase.kind !== "starting") previous.phase.dismiss?.();
					await previous.task;
				}
				await checkpointCommand(run, ref, message);
			})();
		},
	});
}
