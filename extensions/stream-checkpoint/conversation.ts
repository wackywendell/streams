import { randomUUID } from "node:crypto";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { AgentSession, ExtensionContext, ModelRuntime } from "@earendil-works/pi-coding-agent";
import type { Model } from "@earendil-works/pi-ai/compat";
import { checkpointContext, completedMessages, createCheckpointSession } from "./agent.ts";
import { applyDraft, clearPending, includeMainSession, savePending, stageDraft } from "./core.ts";
import type { Binding, Pending } from "./core.ts";
import { feedbackPrompt } from "./review.ts";
import type { Author, Feedback } from "./review.ts";
import { messageText, tagMessage, transcript } from "./view.ts";
import type { PanelState, UnsentFeedback } from "./view.ts";
import type { RefreshOutcome, StreamCli } from "./streams.ts";

type Options = {
	pending: Pending;
	binding: Binding;
	agentDir: string;
	cwd: string;
	manager: ExtensionContext["sessionManager"];
	runtime: ModelRuntime;
	model: Model<any>;
	thinkingLevel?: Parameters<typeof createCheckpointSession>[0]["thinkingLevel"];
	systemPrompt: string;
	cli: Pick<StreamCli, "read" | "update" | "refresh">;
	changed(): void;
	applied(summary: string, refresh: RefreshOutcome): void;
};

/** Owns model operations and durable state; the terminal edits only unsent feedback, through `editFeedback`. */
export class CheckpointConversation {
	readonly state: PanelState;
	private side?: AgentSession;
	private savedCount = 0;
	private operation?: Promise<void>;
	private closed = false;
	private finished = false;
	private accepting?: Feedback;
	private readonly kickoff: string;

	constructor(private options: Options) {
		const { pending } = options;
		this.kickoff = `Using the streams skill, review the current main-session context and scratch note for ${pending.slug}. Draft a handoff with the current state, important decisions, and next action. Ask about material gaps if needed, then stage complete updated scratch Markdown with propose_checkpoint for my review. Do not write scratch or apply the draft.`;
		this.state = { messages: transcript(pending.messages as AgentMessage[], pending.feedback ?? [], this.kickoff), draft: pending.draft,
			activity: null, input: pending.input, annotations: pending.annotations ?? [], annotationEditor: pending.annotationEditor };
	}

	private isOwner(): boolean { return !this.closed && this.options.manager.getSessionId() === this.options.pending.ownerId; }

	/**
	 * Applies the user's edit to unsent feedback, and saves it. Refused while an operation runs, since sending reads
	 * and then clears that feedback, or once the checkpoint is applied or cleared.
	 */
	editFeedback(change: (feedback: UnsentFeedback) => void): boolean {
		if (this.state.activity || this.finished) return false;
		change(this.state);
		this.persist();
		return true;
	}

	persist(): void {
		if (this.finished) return;
		const { pending, agentDir } = this.options;
		if (this.side) {
			pending.messages.push(...this.side.messages.slice(this.savedCount));
			this.savedCount = this.side.messages.length;
		}
		pending.input = this.state.input;
		pending.annotations = this.state.annotations;
		pending.annotationEditor = this.state.annotationEditor;
		savePending(agentDir, pending);
	}

	private updateTranscript(partial?: AgentMessage): void {
		const { pending } = this.options;
		const messages = [...pending.messages as AgentMessage[], ...(this.side?.messages.slice(this.savedCount) ?? [])];
		if (partial && !messages.includes(partial)) messages.push(partial);
		this.state.messages = transcript(messages, pending.feedback ?? [], this.kickoff);
	}

	private run(activity: NonNullable<PanelState["activity"]>, work: () => Promise<void>): Promise<void> {
		if (this.state.activity || !this.isOwner()) return Promise.resolve();
		this.state.activity = activity;
		this.state.notice = undefined;
		this.options.changed();
		this.operation = (async () => {
			try { await work(); }
			catch (error) { this.state.notice = { level: "error", text: String(error) }; }
			finally {
				this.accepting = undefined;
				this.state.activity = null;
				this.state.tool = undefined;
				this.persist();
				this.updateTranscript();
				if (!this.closed) this.options.changed();
			}
		})();
		return this.operation;
	}

	get isFresh(): boolean {
		return !this.options.pending.messages.length && !this.state.draft && !this.state.input.trim() && !this.state.annotations.length && !this.state.annotationEditor;
	}

	/** Whether instructions sent now would reach the side agent rather than be refused. */
	get acceptsInstructions(): boolean {
		return !this.state.activity && !this.state.input.trim() && !this.state.annotations.length && !this.state.annotationEditor;
	}

	start(message = "", snapshot = checkpointContext(this.options.manager), author?: Author): Promise<void> {
		return this.run("starting", async () => {
			const fresh = this.isFresh;
			const accepted = message ? this.acceptCommandMessage(message) : false;
			const rejection = message && !accepted ? this.state.notice : undefined;
			await this.startSide(snapshot);
			if (rejection) this.state.notice = rejection;
			if ((accepted || (!message && fresh)) && this.isOwner()) {
				this.state.activity = "drafting";
				this.options.changed();
				if (accepted) await this.sendFeedback(author);
				else await this.prompt(tagMessage("kickoff", randomUUID().slice(0, 8), this.kickoff));
			}
		});
	}

	private async startSide(snapshot: ReturnType<typeof checkpointContext>): Promise<void> {
		const o = this.options;
		const safe = completedMessages(o.pending.messages as AgentMessage[]);
		if (safe.length !== o.pending.messages.length) this.state.notice = { level: "info", text: "Interrupted answer omitted; earlier question preserved" };
		o.pending.messages = safe;
		o.pending.source = { ownerId: o.pending.ownerId, leafId: snapshot.leafId };
		const scratch = (await o.cli.read(o.binding.slug)).content;
		if (o.pending.draft && o.pending.draft.baseContent !== scratch) this.state.notice = { level: "warning", text: "Scratch changed; request a new draft before Apply" };
		if (snapshot.omitted) this.state.notice = { level: "warning", text: `${snapshot.omitted} incomplete main message(s) omitted from context` };
		const source: AgentMessage = { role: "user", content: [{ type: "text", text: `Checkpoint source: main session ${o.pending.ownerId}, leaf ${snapshot.leafId ?? "root"}. Stream ${o.binding.slug}. Current scratch Markdown:\n${scratch}` }], timestamp: Date.now() };
		const side = await createCheckpointSession({ cwd: o.cwd, agentDir: o.agentDir, modelRuntime: o.runtime, model: o.model,
			thinkingLevel: o.thinkingLevel, systemPrompt: o.systemPrompt, mainMessages: [...snapshot.messages, source], savedMessages: safe,
			onProposal: async (content, summary) => {
				if (!this.isOwner()) throw new Error("Owning Pi session changed; proposal discarded");
				const base = (await o.cli.read(o.binding.slug)).content;
				const draft = stageDraft(o.pending, o.binding, includeMainSession(content, o.pending.ownerId), summary, base);
				o.pending.draft = draft;
				this.state.draft = draft;
				this.persist();
				o.changed();
				return draft.revision;
			},
		});
		if (!this.isOwner()) { side.dispose(); return; }
		this.side = side;
		this.savedCount = side.messages.length;
		side.subscribe((event) => {
			if (!this.isOwner()) return;
			if (event.type === "message_start" && event.message.role === "user") {
				if (this.accepting && messageText(event.message) === this.accepting.prompt) {
					(o.pending.feedback ??= []).push(this.accepting);
					this.accepting = undefined;
					this.state.input = "";
					this.state.annotations = [];
				}
				this.persist();
				this.updateTranscript(event.message);
			} else if (event.type === "message_update") {
				this.updateTranscript(event.message);
			} else if (event.type === "message_end") {
				this.persist();
				this.updateTranscript();
			} else if (event.type === "tool_execution_start") this.state.tool = event.toolName;
			else if (event.type === "tool_execution_end") this.state.tool = undefined;
			o.changed();
		});
		this.updateTranscript();
	}

	private async prompt(text: string): Promise<void> {
		if (!this.side) throw new Error("Checkpoint agent is unavailable; refresh to retry initialization");
		await this.side.prompt(text, { expandPromptTemplates: false });
		const last = this.side.messages.at(-1);
		if (last?.role === "assistant" && last.stopReason === "error") throw new Error(last.errorMessage ?? "Checkpoint response failed");
		if (last?.role === "assistant" && last.stopReason === "aborted") this.state.notice = { level: "info", text: "Response stopped" };
	}

	private acceptCommandMessage(message: string): boolean {
		if (this.state.input.trim() || this.state.annotations.length || this.state.annotationEditor) {
			this.state.notice = { level: "warning", text: "Checkpoint has unsent feedback; instructions were not sent. Send or remove that feedback first." };
			return false;
		}
		this.state.input = message;
		this.persist();
		return true;
	}

	send(message?: string, author?: Author): Promise<void> {
		if (message !== undefined && this.state.activity) {
			this.state.notice = { level: "warning", text: "Checkpoint is busy; instructions were not sent. Retry after the current operation." };
			this.options.changed();
			return Promise.resolve();
		}
		return this.run("drafting", async () => {
			if (message !== undefined && !this.acceptCommandMessage(message)) return;
			await this.sendFeedback(message !== undefined ? author : undefined);
		});
	}

	private async sendFeedback(author?: Author): Promise<void> {
		if (this.state.annotationEditor) throw new Error("Save or cancel the unfinished annotation in Review first");
		if (!this.state.input.trim() && !this.state.annotations.length) return;
		if (this.state.annotations.some((annotation) => annotation.revision !== this.state.draft?.revision)) {
			throw new Error("Annotations belong to an older draft; remove them before sending");
		}
		const annotations = this.state.annotations.map((annotation) => ({ ...annotation }));
		const text = this.state.input;
		const body = feedbackPrompt(text, annotations);
		const id = randomUUID().slice(0, 8);
		const prompt = tagMessage("feedback", id, author ? `Instructions from the main agent, not the user:\n\n${body}` : body);
		this.accepting = { id, text, annotations, prompt, ...(author ? { author } : {}) };
		await this.prompt(this.accepting.prompt);
	}

	refreshContext(snapshot = checkpointContext(this.options.manager)): Promise<void> {
		return this.run("refreshing", async () => {
			this.persist();
			this.side?.dispose();
			this.side = undefined;
			await this.startSide(snapshot);
			this.state.notice ??= { level: "info", text: this.state.draft ? `Context refreshed; draft ${this.state.draft.revision} unchanged` : "Context refreshed" };
		});
	}

	apply(): Promise<void> {
		return this.run("applying", async () => {
			if (this.state.annotations.length || this.state.annotationEditor || this.state.input.trim()) throw new Error("Send or remove pending feedback before Apply");
			const draft = this.state.draft;
			if (!draft) throw new Error("No draft to apply");
			const o = this.options;
			const result = await applyDraft(draft, draft.revision, { read: (slug) => o.cli.read(slug), update: (...args) => o.cli.update(...args), project: () => o.cli.refresh(draft.slug) });
			if (result.validationError) { this.state.notice = { level: "error", text: result.validationError }; return; }
			this.finished = true;
			clearPending(o.agentDir, o.pending.ownerId, o.pending.slug);
			o.applied(draft.summary ?? "Scratch updated", result.refresh!);
		});
	}

	async stop(): Promise<void> {
		if (this.state.activity !== "drafting") return;
		await this.side?.abort();
		await this.operation;
		this.state.notice = { level: "info", text: "Response stopped" };
		this.options.changed();
	}

	clear(): void {
		if (this.state.activity) return;
		this.finished = true;
		clearPending(this.options.agentDir, this.options.pending.ownerId, this.options.pending.slug);
	}

	async dispose(): Promise<void> {
		if (this.closed) return;
		this.closed = true;
		if (this.side?.isStreaming) await this.side.abort();
		await this.operation;
		this.persist();
		this.side?.dispose();
	}
}
