import { readFileSync } from "node:fs";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { AgentSession, ExtensionContext, ModelRuntime } from "@earendil-works/pi-coding-agent";
import type { Model } from "@earendil-works/pi-ai/compat";
import { checkpointContext, completedMessages, createCheckpointSession } from "./agent.ts";
import { applyDraft, clearPending, includeMainSession, savePending, stageDraft } from "./core.ts";
import type { Binding, Pending } from "./core.ts";
import { feedbackPrompt } from "./review.ts";
import type { Feedback } from "./review.ts";
import { messageText, transcript } from "./view.ts";
import type { PanelState } from "./view.ts";
import type { StreamCli } from "./streams.ts";

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
	cli: Pick<StreamCli, "update" | "project">;
	cmux: boolean;
	changed(): void;
	applied(summary: string, warning?: string): void;
};

/** Owns model operations and durable state; the terminal only edits idle feedback. */
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

	start(message = ""): Promise<void> {
		return this.run("starting", async () => {
			const { pending } = this.options;
			const fresh = !pending.messages.length && !pending.draft && !pending.input.trim() && !this.state.annotations.length && !this.state.annotationEditor;
			const accepted = message ? this.acceptCommandMessage(message) : false;
			const rejection = message && !accepted ? this.state.notice : undefined;
			await this.startSide();
			if (rejection) this.state.notice = rejection;
			if ((accepted || (!message && fresh)) && this.isOwner()) {
				this.state.activity = "drafting";
				this.options.changed();
				if (accepted) await this.sendFeedback();
				else await this.prompt(this.kickoff);
			}
		});
	}

	private async startSide(): Promise<void> {
		const o = this.options;
		const safe = completedMessages(o.pending.messages as AgentMessage[]);
		if (safe.length !== o.pending.messages.length) this.state.notice = { level: "info", text: "Interrupted answer omitted; earlier question preserved" };
		o.pending.messages = safe;
		const snapshot = checkpointContext(o.manager);
		o.pending.source = { ownerId: o.pending.ownerId, leafId: snapshot.leafId };
		const scratch = readFileSync(o.binding.path, "utf8");
		if (o.pending.draft && o.pending.draft.baseContent !== scratch) this.state.notice = { level: "warning", text: "Scratch changed; request a new draft before Apply" };
		if (snapshot.omitted) this.state.notice = { level: "warning", text: `${snapshot.omitted} incomplete main message(s) omitted from context` };
		const source: AgentMessage = { role: "user", content: [{ type: "text", text: `Checkpoint source: main session ${o.pending.ownerId}, leaf ${snapshot.leafId ?? "root"}. Stream ${o.binding.slug}, scratch ${o.binding.path}. Current scratch Markdown:\n${scratch}` }], timestamp: Date.now() };
		const side = await createCheckpointSession({ cwd: o.cwd, agentDir: o.agentDir, modelRuntime: o.runtime, model: o.model,
			thinkingLevel: o.thinkingLevel, systemPrompt: o.systemPrompt, mainMessages: [...snapshot.messages, source], savedMessages: safe,
			onProposal: async (content, summary) => {
				if (!this.isOwner()) throw new Error("Owning Pi session changed; proposal discarded");
				const draft = stageDraft(o.pending, o.binding, includeMainSession(content, o.pending.ownerId), summary);
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

	send(message?: string): Promise<void> {
		if (message !== undefined && this.state.activity) {
			this.state.notice = { level: "warning", text: "Checkpoint is busy; instructions were not sent. Retry after the current operation." };
			this.options.changed();
			return Promise.resolve();
		}
		return this.run("drafting", async () => {
			if (message !== undefined && !this.acceptCommandMessage(message)) return;
			await this.sendFeedback();
		});
	}

	private async sendFeedback(): Promise<void> {
		if (this.state.annotationEditor) throw new Error("Save or cancel the unfinished annotation in Review first");
		if (!this.state.input.trim() && !this.state.annotations.length) return;
		if (this.state.annotations.some((annotation) => annotation.revision !== this.state.draft?.revision || annotation.path !== this.state.draft?.path)) {
			throw new Error("Annotations belong to an older draft; remove them before sending");
		}
		const annotations = this.state.annotations.map((annotation) => ({ ...annotation }));
		const text = this.state.input;
		this.accepting = { text, annotations, prompt: feedbackPrompt(text, annotations) };
		await this.prompt(this.accepting.prompt);
	}

	refreshContext(): Promise<void> {
		return this.run("refreshing", async () => {
			this.persist();
			this.side?.dispose();
			this.side = undefined;
			await this.startSide();
			this.state.notice ??= { level: "info", text: this.state.draft ? `Context refreshed; draft ${this.state.draft.revision} unchanged` : "Context refreshed" };
		});
	}

	apply(): Promise<void> {
		return this.run("applying", async () => {
			if (this.state.annotations.length || this.state.annotationEditor || this.state.input.trim()) throw new Error("Send or remove pending feedback before Apply");
			const draft = this.state.draft;
			if (!draft) throw new Error("No draft to apply");
			const o = this.options;
			const result = await applyDraft(draft, draft.revision, { update: (...args) => o.cli.update(...args), project: o.cmux ? () => o.cli.project(draft.slug) : undefined });
			if (result.validationError) { this.state.notice = { level: "error", text: result.validationError }; return; }
			this.finished = true;
			clearPending(o.agentDir, o.pending.ownerId, o.pending.slug);
			o.applied(draft.summary ?? "Scratch updated", result.projectionWarning);
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
