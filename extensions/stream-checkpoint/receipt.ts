import { Box, Text } from "@earendil-works/pi-tui";
import type { MessageRenderer } from "@earendil-works/pi-coding-agent";
import type { CommandDiagnostic } from "./streams.ts";
import { annotationContext } from "./review.ts";
import type { ChatMessage } from "./view.ts";

export type CmuxOutcome = "updated" | "disabled" | "failed";
type ReceiptDetails = {
	outcome?: "applied";
	slug: string;
	path: string;
	summary: string;
	cmux: CmuxOutcome;
	warning?: string;
	commands: CommandDiagnostic[];
	discussion?: string;
};
type ClearedDetails = { outcome: "cleared"; slug: string; path: string; discussion: string };

export const DISCUSSION_LIMIT = 16_000;
const label = (message: ChatMessage) => message.author ? "Main agent" : message.role === "user" ? "User" : "Checkpoint agent";

/**
 * Side-chat transcript for the main agent. When over the limit, drop the oldest checkpoint-agent
 * replies first, since the applied note already carries their result, then the oldest user messages.
 */
export function discussionText(messages: ChatMessage[], limit = DISCUSSION_LIMIT): string {
	const entries = messages.map((message) => ({ role: message.role,
		text: [`${label(message)}: ${message.text}`, ...(message.annotations ?? []).map(annotationContext)].join("\n\n") }));
	let omitted = 0;
	const render = () => [...(omitted ? [`[${omitted} earlier message${omitted === 1 ? "" : "s"} omitted]`] : []), ...entries.map((entry) => entry.text)].join("\n\n");
	let text = render();
	while (entries.length > 1 && text.length > limit) {
		const index = entries.findIndex((entry) => entry.role === "assistant");
		entries.splice(index >= 0 ? index : 0, 1);
		omitted++;
		text = render();
	}
	return text.length > limit ? `${text.slice(0, limit)}\n[truncated]` : text;
}

export function checkpointReceipt(details: Omit<ReceiptDetails, "outcome">) {
	const warnings = details.commands.some((command) => command.stderr.trim() || command.warnings?.length);
	const discussion = details.discussion ? `\n\nCheckpoint discussion:\n${details.discussion}` : "";
	return {
		customType: "checkpoint-receipt", display: true,
		content: `Checkpoint applied to ${details.slug} (${details.path}): ${details.summary}\nCMUX ${details.cmux}.${warnings ? " CLI diagnostics available." : ""}${discussion}`,
		details: { ...details, outcome: "applied" as const, commands: structuredClone(details.commands) },
	};
}

export function clearedReceipt(details: Omit<ClearedDetails, "outcome">) {
	return {
		customType: "checkpoint-receipt", display: true,
		content: `Checkpoint for ${details.slug} (${details.path}) was discussed but not applied; the draft was cleared.\n\nCheckpoint discussion:\n${details.discussion}`,
		details: { ...details, outcome: "cleared" as const },
	};
}

export const renderCheckpointReceipt: MessageRenderer<ReceiptDetails | ClearedDetails> = (message, { expanded, outputPad }, theme) => {
	const details = message.details;
	if (!details) return new Text(typeof message.content === "string" ? message.content : "Checkpoint", outputPad, 1);
	if (details.outcome === "cleared") {
		const box = new Box(outputPad, 1, (text) => theme.bg("customMessageBg", text));
		box.addChild(new Text(`${theme.fg("accent", `Checkpoint · ${details.slug}`)} · ${theme.fg("muted", "Cleared, not applied")}`, 0, 0));
		if (expanded) box.addChild(new Text(`${theme.fg("dim", details.path)}\n\n${details.discussion}`, 0, 0));
		return box;
	}
	const warnings = details.commands.some((command) => command.stderr.trim() || command.warnings?.length);
	const box = new Box(outputPad, 1, (text) => theme.bg("customMessageBg", text));
	const status = `Saved · CMUX ${details.cmux}${warnings ? " · Diagnostics" : ""}`;
	box.addChild(new Text(`${theme.fg("accent", `Checkpoint · ${details.slug}`)} · ${theme.fg(details.cmux === "failed" || warnings ? "warning" : "success", status)}\n${details.summary}`, 0, 0));
	if (expanded) {
		box.addChild(new Text(theme.fg("dim", details.path), 0, 0));
		if (details.warning) box.addChild(new Text(theme.fg("warning", details.warning), 0, 0));
		if (details.discussion) box.addChild(new Text(`\n${theme.fg("accent", "Discussion")}\n${details.discussion}`, 0, 0));
		for (const command of details.commands) {
			const args = command.args.map((arg) => /\s/.test(arg) ? JSON.stringify(arg) : arg).join(" ");
			box.addChild(new Text(`\nstreams ${args} · exit ${command.code ?? "unavailable"}`, 0, 0));
			if (command.stdout) box.addChild(new Text(command.stdout, 0, 0));
			if (command.stderr) box.addChild(new Text(theme.fg("warning", command.stderr), 0, 0));
			for (const warning of command.warnings ?? []) box.addChild(new Text(theme.fg("warning", warning), 0, 0));
		}
	}
	return box;
};
