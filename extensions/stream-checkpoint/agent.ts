import type { AgentMessage } from "@earendil-works/pi-agent-core";
import { convertToLlm, createAgentSession, DefaultResourceLoader, estimateTokens, SessionManager, SettingsManager } from "@earendil-works/pi-coding-agent";
import type { AgentSession, ExtensionContext, ModelRuntime } from "@earendil-works/pi-coding-agent";
import type { Model } from "@earendil-works/pi-ai/compat";
import { Type } from "typebox";

/** A resumed SDK session must never receive an unmatched tool call or aborted answer. */
export function completedMessages(messages: AgentMessage[]): AgentMessage[] {
	const results = new Set(messages.filter((message) => message.role === "toolResult").map((message) => message.toolCallId));
	const omittedCalls = new Set<string>();
	return messages.filter((message) => {
		if (message.role === "assistant") {
			const calls = message.content.filter((item) => item.type === "toolCall").map((item) => item.id);
			if (message.stopReason === "aborted" || calls.some((id) => !results.has(id))) {
				for (const id of calls) omittedCalls.add(id);
				return false;
			}
		}
		return message.role !== "toolResult" || !omittedCalls.has(message.toolCallId);
	});
}

/**
 * Follow Pi's compaction and context edits, then drop incomplete trailing tool batches.
 * A checkpoint started by a tool call drops that call's own batch without reporting it as omitted,
 * but keeps the calling message's text, which often explains what the checkpoint should record.
 * Its thinking goes too: signed reasoning detached from its tool calls may not replay on another model.
 */
export function checkpointContext(manager: ExtensionContext["sessionManager"], callerToolCallId?: string): { messages: AgentMessage[]; leafId: string | null; omitted: number } {
	const projection = manager.buildSessionProjection().messages;
	const caller = projection.find((message) => message.role === "assistant" && message.content.some((item) => item.type === "toolCall" && item.id === callerToolCallId));
	const callerCalls = new Set(caller?.role === "assistant" ? caller.content.flatMap((item) => item.type === "toolCall" ? [item.id] : []) : []);
	const callerText = caller?.role === "assistant" ? caller.content.filter((item) => item.type === "text" && item.text.trim()) : [];
	const messages = caller?.role === "assistant"
		? completedMessages(projection.flatMap((message) => {
			if (message === caller) return callerText.length ? [{ ...caller, content: callerText, stopReason: "stop" as const }] : [];
			return message.role === "toolResult" && callerCalls.has(message.toolCallId) ? [] : [message];
		}))
		: completedMessages(projection);
	const omitted = projection.filter((message) => !messages.includes(message) && message !== caller && !(message.role === "toolResult" && callerCalls.has(message.toolCallId))).length;
	return { messages: structuredClone(messages), leafId: manager.getLeafId(), omitted };
}

export function assertContextFits(model: Pick<Model<any>, "contextWindow">, systemPrompt: string, messages: AgentMessage[]): void {
	const estimatedTokens = Math.ceil(systemPrompt.length / 4) + messages.reduce((total, message) => total + estimateTokens(message), 0);
	if (model.contextWindow && estimatedTokens > model.contextWindow * 0.9) {
		throw new Error(`Checkpoint input is estimated at ${estimatedTokens} tokens for a ${model.contextWindow}-token model. Compact the main session or use a larger model. No history was dropped.`);
	}
}

export async function createCheckpointSession(options: {
	cwd: string;
	agentDir: string;
	modelRuntime: ModelRuntime;
	model: Model<any>;
	thinkingLevel?: "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max";
	mainMessages: AgentMessage[];
	savedMessages: AgentMessage[];
	systemPrompt: string;
	onProposal?: (content: string, summary: string) => Promise<number>;
}): Promise<AgentSession> {
	const messages = convertToLlm([...options.mainMessages, ...completedMessages(options.savedMessages)]);
	assertContextFits(options.model, options.systemPrompt, messages);
	const settings = SettingsManager.create(options.cwd, options.agentDir);
	const loader = new DefaultResourceLoader({
		cwd: options.cwd, agentDir: options.agentDir, settingsManager: settings,
		noExtensions: true, noSkills: true, noContextFiles: true,
		systemPrompt: options.systemPrompt,
	});
	await loader.reload();
	const manager = SessionManager.inMemory(options.cwd);
	for (const message of messages) manager.appendMessage(message);
	const proposalTool = options.onProposal ? {
		name: "propose_checkpoint",
		label: "Propose checkpoint",
		description: "Stage complete scratch Markdown for user review. Does not edit scratch. Include a short change summary.",
		parameters: Type.Object({ content: Type.String(), summary: Type.String() }),
		executionMode: "sequential" as const,
		async execute(_id: string, args: { content: string; summary: string }) {
			const revision = await options.onProposal!(args.content, args.summary);
			return { content: [{ type: "text" as const, text: `Draft ${revision} ready for review.` }], details: { revision } };
		},
	} : undefined;
	const { session } = await createAgentSession({
		cwd: options.cwd, agentDir: options.agentDir, modelRuntime: options.modelRuntime,
		model: options.model, thinkingLevel: options.thinkingLevel,
		sessionManager: manager, settingsManager: settings, resourceLoader: loader,
		tools: ["read", "grep", "find", "ls", ...(proposalTool ? [proposalTool.name] : [])],
		customTools: proposalTool ? [proposalTool] : [],
	});
	return session;
}
