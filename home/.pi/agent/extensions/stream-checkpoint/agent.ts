import type { AgentMessage } from "@earendil-works/pi-agent-core";
import { createAgentSession, DefaultResourceLoader, SessionManager, SettingsManager } from "@earendil-works/pi-coding-agent";
import type { AgentSession, ModelRuntime, ReadonlySessionManager } from "@earendil-works/pi-coding-agent";
import type { Model } from "@earendil-works/pi-ai/compat";

/** Follow Pi's compaction and context edits, then drop incomplete trailing tool batches. */
export function checkpointContext(manager: ReadonlySessionManager): { messages: AgentMessage[]; leafId: string | null } {
	const messages = manager.buildSessionProjection().messages;
	let safeCount = messages.length;
	for (let index = 0; index < messages.length; index++) {
		const message = messages[index];
		if (message.role !== "assistant") continue;
		const calls = message.content.filter((item) => item.type === "toolCall").map((item) => item.id);
		if (!calls.length) continue;
		const results = new Set(messages.slice(index + 1).filter((item) => item.role === "toolResult").map((item) => item.toolCallId));
		if (calls.some((id) => !results.has(id))) { safeCount = index; break; }
	}
	return { messages: messages.slice(0, safeCount), leafId: manager.getLeafId() };
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
}): Promise<AgentSession> {
	const settings = SettingsManager.create(options.cwd, options.agentDir);
	const loader = new DefaultResourceLoader({
		cwd: options.cwd, agentDir: options.agentDir, settingsManager: settings,
		noExtensions: true, noSkills: true, noContextFiles: true,
		systemPrompt: options.systemPrompt,
	});
	await loader.reload();
	const manager = SessionManager.inMemory(options.cwd);
	for (const message of [...options.mainMessages, ...options.savedMessages]) manager.appendMessage(message);
	const { session } = await createAgentSession({
		cwd: options.cwd, agentDir: options.agentDir, modelRuntime: options.modelRuntime,
		model: options.model, thinkingLevel: options.thinkingLevel,
		sessionManager: manager, settingsManager: settings, resourceLoader: loader,
		tools: ["read", "grep", "find", "ls"],
	});
	return session;
}
