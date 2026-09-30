import { Box, Text } from "@earendil-works/pi-tui";
import type { MessageRenderer } from "@earendil-works/pi-coding-agent";
import type { CommandDiagnostic } from "./streams.ts";

export type CmuxOutcome = "updated" | "disabled" | "failed";
type ReceiptDetails = {
	slug: string;
	path: string;
	summary: string;
	cmux: CmuxOutcome;
	warning?: string;
	commands: CommandDiagnostic[];
};

export function checkpointReceipt(details: ReceiptDetails) {
	const warnings = details.commands.some((command) => command.stderr.trim() || command.warnings?.length);
	return {
		customType: "checkpoint-receipt", display: true,
		content: `Checkpoint applied to ${details.slug} (${details.path}): ${details.summary}\nCMUX ${details.cmux}.${warnings ? " CLI diagnostics available." : ""}`,
		details: { ...details, commands: structuredClone(details.commands) },
	};
}

export const renderCheckpointReceipt: MessageRenderer<ReceiptDetails> = (message, { expanded, outputPad }, theme) => {
	const details = message.details;
	if (!details) return new Text(typeof message.content === "string" ? message.content : "Checkpoint", outputPad, 1);
	const warnings = details.commands.some((command) => command.stderr.trim() || command.warnings?.length);
	const box = new Box(outputPad, 1, (text) => theme.bg("customMessageBg", text));
	const status = `Saved · CMUX ${details.cmux}${warnings ? " · Diagnostics" : ""}`;
	box.addChild(new Text(`${theme.fg("accent", `Checkpoint · ${details.slug}`)} · ${theme.fg(details.cmux === "failed" || warnings ? "warning" : "success", status)}\n${details.summary}`, 0, 0));
	if (expanded) {
		box.addChild(new Text(theme.fg("dim", details.path), 0, 0));
		if (details.warning) box.addChild(new Text(theme.fg("warning", details.warning), 0, 0));
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
