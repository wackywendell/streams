import { Box, Text } from "@earendil-works/pi-tui";
import type { MessageRenderer } from "@earendil-works/pi-coding-agent";
import type { CommandDiagnostic, RefreshOutcome } from "./streams.ts";
import { annotationContext } from "./review.ts";
import type { ChatMessage } from "./view.ts";

type ReceiptDetails = {
	/** Absent on receipts saved before Clear receipts existed, all of which were for Apply. */
	outcome?: "applied";
	slug: string;
	summary: string;
	refresh: RefreshOutcome;
	commands: CommandDiagnostic[];
	discussion?: string;
};
/** Persisted receipts from the CMUX-only integration are decoded only while rendering. */
type LegacyReceiptDetails = Omit<ReceiptDetails, "refresh"> & { cmux: "updated" | "disabled" | "failed"; warning?: string };
type ClearedDetails = { outcome: "cleared"; slug: string; discussion: string };
/** Reports a select_stream outcome; `slug` is the stream bound afterwards, if any. */
type ChoiceDetails = { outcome: "bound" | "choice-cancelled"; slug?: string; created?: boolean; refresh?: RefreshOutcome; commands?: CommandDiagnostic[] };

function refreshStatus(refresh: RefreshOutcome): string {
	switch (refresh.status) {
		case "skipped": return `Workspace refresh skipped (${refresh.reason === "disabled" ? "disabled" : "no caller context"})`;
		case "updated": return `Workspace updated (${refresh.frontend})`;
		case "selection-failed": return "Workspace refresh selection failed";
		case "backend-failed": return `Workspace refresh failed (${refresh.frontend})`;
	}
}

function savedRefresh(details: ReceiptDetails | LegacyReceiptDetails): RefreshOutcome {
	if ("refresh" in details) return details.refresh;
	return details.cmux === "disabled" ? { status: "skipped", reason: "disabled" }
		: details.cmux === "updated" ? { status: "updated", frontend: "cmux" }
		: { status: "backend-failed", frontend: "cmux", warning: details.warning ?? "" };
}

function failed(refresh?: RefreshOutcome): boolean { return refresh?.status === "selection-failed" || refresh?.status === "backend-failed"; }

function addDiagnostics(box: Box, refresh: RefreshOutcome | undefined, commands: CommandDiagnostic[], theme: Parameters<MessageRenderer>[2]) {
	if (refresh && "warning" in refresh && refresh.warning) box.addChild(new Text(theme.fg("warning", refresh.warning), 0, 0));
	for (const command of commands) {
		const args = command.args.map((arg) => /\s/.test(arg) ? JSON.stringify(arg) : arg).join(" ");
		box.addChild(new Text(`\nstreams ${args} · exit ${command.code ?? "unavailable"}`, 0, 0));
		if (command.stdout) box.addChild(new Text(command.stdout, 0, 0));
		if (command.stderr) box.addChild(new Text(theme.fg("warning", command.stderr), 0, 0));
		for (const warning of command.warnings ?? []) box.addChild(new Text(theme.fg("warning", warning), 0, 0));
	}
}

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
		content: `Checkpoint applied to ${details.slug}: ${details.summary}\n${refreshStatus(details.refresh)}.${warnings ? " CLI diagnostics available." : ""}${discussion}`,
		details: { ...details, outcome: "applied" as const, commands: structuredClone(details.commands) },
	};
}

export function clearedReceipt(details: Omit<ClearedDetails, "outcome">) {
	return {
		customType: "checkpoint-receipt", display: true,
		content: `Checkpoint for ${details.slug} was discussed but not applied; the draft was cleared.\n\nCheckpoint discussion:\n${details.discussion}`,
		details: { ...details, outcome: "cleared" as const },
	};
}

export function streamChosenReceipt(details: { slug: string; created: boolean; refresh?: RefreshOutcome; commands?: CommandDiagnostic[] }) {
	return {
		customType: "checkpoint-receipt", display: true,
		content: `The user chose ${details.created ? "a new stream, " : ""}${details.slug}. This session is now bound to it, so checkpoint and stream_read use it.${details.refresh ? ` ${refreshStatus(details.refresh)}.` : ""}`,
		details: { ...details, outcome: "bound" as const, commands: structuredClone(details.commands ?? []) },
	};
}

export function choiceCancelledReceipt(bound?: { slug: string }) {
	return {
		customType: "checkpoint-receipt", display: true,
		content: `The user closed the stream chooser without choosing. ${bound ? `This session is still bound to ${bound.slug}.` : "No stream is bound to this session."}`,
		details: { outcome: "choice-cancelled" as const, slug: bound?.slug },
	};
}

export const renderCheckpointReceipt: MessageRenderer<ReceiptDetails | LegacyReceiptDetails | ClearedDetails | ChoiceDetails> = (message, { expanded, outputPad }, theme) => {
	const details = message.details;
	if (!details) return new Text(typeof message.content === "string" ? message.content : "Checkpoint", outputPad, 1);
	if (details.outcome === "bound" || details.outcome === "choice-cancelled") {
		const label = details.outcome === "bound" ? `Bound${details.created ? " · Created" : ""}` : "Choice cancelled";
		const box = new Box(outputPad, 1, (text) => theme.bg("customMessageBg", text));
		box.addChild(new Text(`${theme.fg("accent", `Stream${details.slug ? ` · ${details.slug}` : ""}`)} · ${theme.fg("muted", label)}${details.refresh ? ` · ${theme.fg(failed(details.refresh) ? "warning" : "muted", refreshStatus(details.refresh))}` : ""}`, 0, 0));
		if (expanded) addDiagnostics(box, details.refresh, details.commands ?? [], theme);
		return box;
	}
	if (details.outcome === "cleared") {
		const box = new Box(outputPad, 1, (text) => theme.bg("customMessageBg", text));
		box.addChild(new Text(`${theme.fg("accent", `Checkpoint · ${details.slug}`)} · ${theme.fg("muted", "Cleared, not applied")}`, 0, 0));
		if (expanded) box.addChild(new Text(details.discussion, 0, 0));
		return box;
	}
	const warnings = details.commands.some((command) => command.stderr.trim() || command.warnings?.length);
	const box = new Box(outputPad, 1, (text) => theme.bg("customMessageBg", text));
	const refresh = savedRefresh(details);
	const status = `Applied · ${refreshStatus(refresh)}${warnings ? " · Diagnostics" : ""}`;
	box.addChild(new Text(`${theme.fg("accent", `Checkpoint · ${details.slug}`)} · ${theme.fg(failed(refresh) || warnings ? "warning" : "success", status)}\n${details.summary}`, 0, 0));
	if (expanded) {
		if ("warning" in details && details.warning && !failed(refresh)) box.addChild(new Text(theme.fg("warning", details.warning), 0, 0));
		if (details.discussion) box.addChild(new Text(`\n${theme.fg("accent", "Discussion")}\n${details.discussion}`, 0, 0));
		addDiagnostics(box, refresh, details.commands, theme);
	}
	return box;
};
