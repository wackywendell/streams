import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { Draft } from "./core.ts";
import type { Annotation, AnnotationEditor, Feedback } from "./review.ts";

export interface ChatMessage {
	role: "user" | "assistant";
	text: string;
	annotations?: Annotation[];
}

export interface PanelState {
	messages: ChatMessage[];
	draft?: Draft;
	activity: "starting" | "drafting" | "applying" | "refreshing" | null;
	tool?: string;
	notice?: { level: "info" | "warning" | "error"; text: string };
	input: string;
	annotations: Annotation[];
	annotationEditor?: AnnotationEditor;
}

export function messageText(message: Extract<AgentMessage, { role: "user" | "assistant" }>): string {
	return typeof message.content === "string" ? message.content
		: message.content.filter((part) => part.type === "text").map((part) => part.text).join("\n");
}

export function transcript(messages: AgentMessage[], feedback: Feedback[], kickoff: string): ChatMessage[] {
	return messages.flatMap((message): ChatMessage[] => {
		if (message.role !== "user" && message.role !== "assistant") return [];
		const text = messageText(message);
		if (!text || (message.role === "user" && text === kickoff)) return [];
		const sent = message.role === "user" ? feedback.find((entry) => entry.prompt === text) : undefined;
		return [{ role: message.role, text: sent?.text ?? text, ...(sent?.annotations.length ? { annotations: sent.annotations } : {}) }];
	});
}

export function activityLabel(state: PanelState): string {
	if (state.activity === "drafting") return state.tool === "propose_checkpoint" ? "Updating draft" : state.tool ? "Reading" : "Drafting";
	return state.activity ? { starting: "Starting", applying: "Applying", refreshing: "Refreshing" }[state.activity] : "";
}

export function shortcutLabel(shortcut: string): string {
	return shortcut.split("+").map((part) => part.length === 1 ? part.toUpperCase() : part[0].toUpperCase() + part.slice(1)).join("+");
}

export function checkpointStatus(state: PanelState, shortcut: string): string {
	const status = activityLabel(state) || (state.notice?.level === "error" || state.notice?.level === "warning" ? "Needs attention" : state.draft ? `Draft ${state.draft.revision}` : "Open");
	return `Checkpoint · ${status} · ${shortcutLabel(shortcut)}`;
}
