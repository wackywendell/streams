import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { OverlayHandle } from "@earendil-works/pi-tui";
import { BorderedLoader, convertToLlm, getAgentDir, ModelRuntime } from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { checkpointContext } from "./agent.ts";
import { BINDING_TYPE, latestBinding, loadConfig, parseRecommendations, readPending, selectStream } from "./core.ts";
import type { Recommendations } from "./core.ts";
import { CheckpointPanel } from "./panel.ts";
import { StreamChooser } from "./chooser.ts";
import type { ChosenStream } from "./chooser.ts";
import { CheckpointConversation } from "./conversation.ts";
import { checkpointLabel, checkpointStatus } from "./view.ts";
import { CheckpointJournal, MARKER_TYPE } from "./journal.ts";
import { StreamCli } from "./streams.ts";
import { checkpointReceipt, renderCheckpointReceipt } from "./receipt.ts";

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
	marker: string;
	finalized?: boolean;
	cancellation: AbortController;
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
	pi.on("session_start", (_event, ctx) => journal.restore(ctx.sessionManager.getEntries()));
	const show = (run: OpenCheckpoint) => { run.handle?.setHidden(false); run.handle?.focus(); run.updateStatus?.(); run.panel?.refresh(); };
	pi.registerShortcut(focusShortcut, {
		description: "Show the open checkpoint panel",
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

	async function open(run: OpenCheckpoint, ref: string, message: string, ctx: ExtensionCommandContext): Promise<void> {
		const alive = () => !run.cancellation.signal.aborted && ctx.sessionManager.getSessionId() === run.ownerId;
		async function loading<T>(label: string, work: () => Promise<T>): Promise<T | undefined> {
			type Result = { value: T } | { error: unknown } | undefined;
			const result = await ctx.ui.custom<Result>((tui, theme, _keys, done) => {
				const loader = new BorderedLoader(tui, theme, label);
				let finished = false;
				const finish = (value: Result) => { if (!finished) { finished = true; done(value); } };
				run.dismiss = () => finish(undefined);
				loader.onAbort = () => { run.cancellation.abort(); finish(undefined); };
				void work().then((value) => finish({ value }), (error) => finish({ error }));
				return loader;
			}, { overlay: true, overlayOptions: { anchor: "center", width: "52%", minWidth: 50, maxHeight: 10 } });
			if (!alive() || !result) return undefined;
			if ("error" in result) throw result.error;
			return result.value;
		}
		try {
			if (!alive()) return;
			const config = loadConfig(agentDir);
			const cli = testDeps.cli ?? new StreamCli(ctx.cwd, undefined, (text) => { if (alive()) ctx.ui.notify(text, "warning"); });
			const binding = latestBinding(ctx.sessionManager.getEntries(), run.ownerId);
			const lookup = await loading("Looking up checkpoint stream…", async () => {
				const sharedConfig = await cli.config();
				run.cancellation.signal.throwIfAborted();
				const streams = await cli.list();
				run.cancellation.signal.throwIfAborted();
				const stream = ref.trim() ? await cli.show(ref.trim()).catch((error) => { if (alive()) ctx.ui.notify(String(error), "warning"); return undefined; })
					: await selectStream("", binding, { list: async () => streams, show: (value) => cli.show(value) });
				return { sharedConfig, streams, stream };
			});
			if (!lookup) return;
			const { sharedConfig, streams } = lookup;
			let { stream } = lookup;
			let created = false;
			let switchConfirmed = false;
			if (!alive()) return;
			if (!stream) {
				if (binding && readPending(agentDir, run.ownerId, binding.slug)) {
					switchConfirmed = await ctx.ui.confirm("Switch checkpoint stream?", `Unfinished work for ${binding.slug} will remain saved.`, { signal: run.cancellation.signal });
					if (!switchConfirmed || !alive()) return;
				}
				journal.update(run.marker, { label: "Choosing stream", working: true });
				const selectorModel = configuredModel(ctx, config.selectorModel, "selector");
				const recommendations = await loading("Finding stream recommendations…", () =>
					recommendStreams(ctx, streams, selectorModel, run.snapshot, run.cancellation.signal).catch(() => ({ existing: [] })));
				if (!recommendations) return;
				const choice = await ctx.ui.custom<ChosenStream | undefined>((tui, theme, _keys, done) => {
					run.dismiss = () => done(undefined);
					return new StreamChooser(tui, theme, streams, cli, done, recommendations, ref.trim());
				}, { overlay: true, overlayOptions: { anchor: "center", width: "78%", minWidth: 64, margin: 2, maxHeight: "80%" } });
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
			const model = configuredModel(ctx, config.drafterModel, "drafter");
			if (!model) throw new Error("Select a model before starting a checkpoint conversation");
			const runtime = testDeps.runtime ?? await ModelRuntime.create({ authPath: join(agentDir, "auth.json"), modelsPath: join(agentDir, "models.json") });
			const providerConfig = ctx.modelRegistry.getRegisteredProviderConfig(model.provider);
			if (providerConfig) runtime.registerProvider(model.provider, providerConfig);
			const nativeProvider = ctx.modelRegistry.getRegisteredNativeProvider(model.provider);
			if (nativeProvider) runtime.registerNativeProvider(nativeProvider);
			const skill = testDeps.skill ?? readFileSync(join(homedir(), ".agents", "skills", "streams", "SKILL.md"), "utf8");
			const format = await cli.format();
			if (!alive()) return;
			const selected = stream;
			const systemPrompt = `${run.systemPrompt}\n\n<checkpoint_rules>\nYou are a separate checkpoint conversation. Read and discuss stream state; propose only complete scratch Markdown through propose_checkpoint. Do not claim to have written scratch. You have only local read-only tools and the proposal tool. The user alone approves Apply. Every proposal must include pi:${run.ownerId} in frontmatter sessions (preserve ordering), never the checkpoint session ID. After proposing a draft, summarize only substantive changes since the previous scratch note in one or two sentences. Do not announce staging, routine verification, or that scratch was not changed; mention material failures or blockers. Annotations identify an exact draft revision and original/proposed document line, with a quoted excerpt. Treat them as feedback on that snapshot, not as coordinates in a later revision.\n\n${skill}\n\n${format}\n</checkpoint_rules>`;
			const pending = readPending(agentDir, run.ownerId, selected.slug) ?? { version: 1 as const, ownerId: run.ownerId, slug: selected.slug, messages: [], input: "" };
			run.updateStatus = () => {
				const value = run.handle?.isHidden() && run.conversation ? checkpointStatus(run.conversation.state, focusShortcut) : undefined;
				ctx.ui.setStatus("checkpoint", value ? ctx.ui.theme.fg("accent", value) : undefined);
			};
			let previousActivity: CheckpointConversation["state"]["activity"] = null;
			const conversation = new CheckpointConversation({ pending, binding: { ownerId: run.ownerId, slug: selected.slug, path: selected.path },
				agentDir, cwd: ctx.cwd, manager: ctx.sessionManager, runtime, model, thinkingLevel: ctx.thinkingLevel, systemPrompt, cli,
				changed: () => {
					if (!alive() || run.finalized) return;
					const state = conversation.state;
					journal.update(run.marker, { label: state.activity === "drafting" ? "Drafting" : checkpointLabel(state), working: !!state.activity, slug: selected.slug, detail: state.notice?.text });
					const completed = previousActivity === "drafting" && state.activity === null;
					previousActivity = state.activity;
					const last = (pending.messages as AgentMessage[]).at(-1);
					if (completed && state.notice?.level !== "error" && last?.role === "assistant" && last.stopReason === "stop" && ctx.ui.getEditorText() === "") show(run);
					run.panel?.refresh(); run.updateStatus?.();
				},
				applied: (summary, cmux, warning) => {
					if (alive()) {
						run.finalized = true;
						journal.update(run.marker, { label: "Applied", slug: selected.slug, closed: true });
						pi.sendMessage(checkpointReceipt({ slug: selected.slug, path: selected.path, summary, cmux, warning, commands: cli.diagnostics }), { triggerTurn: false });
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
						journal.update(run.marker, { label: "Cleared", slug: selected.slug, closed: true });
						done();
					},
					focusMain: () => { run.panel?.dispose(); run.handle?.setHidden(true); run.updateStatus?.(); },
					changed: () => conversation.persist(),
				}, focusShortcut);
				return run.panel;
			}, { overlay: true, overlayOptions: { anchor: "right-center", width: "64%", minWidth: 60, margin: { right: 1 }, maxHeight: 36 },
				onHandle: (handle) => {
					run.handle = handle;
					if (conversation.isFresh) handle.setHidden(true);
					void conversation.start(message, run.snapshot);
					run.updateStatus?.();
				} });
		} catch (error) {
			if (alive()) {
				run.finalized = true;
				journal.update(run.marker, { label: "Needs attention", detail: String(error), closed: true });
				ctx.ui.notify(`Checkpoint: ${String(error)}`, "error");
			}
		} finally {
			await run.conversation?.dispose();
			if (!run.finalized && ctx.sessionManager.getSessionId() === run.ownerId) journal.update(run.marker, { label: run.conversation ? "Saved for resume" : "Cancelled", closed: true });
			run.panel?.dispose();
			ctx.ui.setStatus("checkpoint", undefined);
			if (active === run) active = undefined;
		}
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
				} else if (!previous.handle) ctx.ui.notify("A checkpoint chooser is already open", "info");
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
