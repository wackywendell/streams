import { SettingsList, matchesKey, truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { Component, SettingItem, TUI } from "@earendil-works/pi-tui";
import type { Theme } from "@earendil-works/pi-coding-agent";
import type { CheckpointConfig } from "./core.ts";
import { CHECKPOINT_PROMPTING, STREAM_PROMPTING } from "./policy.ts";
import type { PromptingConfig } from "./policy.ts";

/** Edits a local copy; only Save writes, and failures leave that copy available for retry. */
export class CheckpointSettings implements Component {
	focused = true;
	private closed = false;
	private error?: string;
	private policy: PromptingConfig;
	private list: SettingsList;

	constructor(private tui: TUI, private theme: Theme, private config: CheckpointConfig, private save: (policy: PromptingConfig) => void, private done: (saved: boolean) => void) {
		this.policy = { streamPrompting: config.streamPrompting, checkpointPrompting: config.checkpointPrompting };
		const items: SettingItem[] = [
			{ id: "streamPrompting", label: "Stream selection / creation", currentValue: config.streamPrompting, values: Object.keys(STREAM_PROMPTING), description: STREAM_PROMPTING[config.streamPrompting] },
			{ id: "checkpointPrompting", label: "Checkpoint frequency", currentValue: config.checkpointPrompting, values: Object.keys(CHECKPOINT_PROMPTING), description: CHECKPOINT_PROMPTING[config.checkpointPrompting] },
		];
		this.list = new SettingsList(items, 2, {
			label: (text, selected) => this.theme.fg(selected ? "accent" : "text", text),
			value: (text) => this.theme.fg("success", text), description: (text) => this.theme.fg("muted", text),
			cursor: this.theme.fg("accent", "▶ "), hint: (text) => this.theme.fg("dim", text),
		}, (id, value) => {
			if (id === "streamPrompting") this.policy.streamPrompting = value as PromptingConfig["streamPrompting"];
			else this.policy.checkpointPrompting = value as PromptingConfig["checkpointPrompting"];
			items[0].description = STREAM_PROMPTING[this.policy.streamPrompting];
			items[1].description = CHECKPOINT_PROMPTING[this.policy.checkpointPrompting];
			this.error = undefined;
		}, () => this.finish(false));
	}

	private finish(saved: boolean): void { if (!this.closed) { this.closed = true; this.done(saved); } }
	dispose(): void { this.closed = true; }
	invalidate(): void { this.list.invalidate(); }
	handleInput(data: string): void {
		if (this.closed) return;
		if (matchesKey(data, "ctrl+s")) {
			try { this.save({ ...this.policy }); this.finish(true); }
			catch (error) { this.error = String(error); }
		} else this.list.handleInput(data);
		this.tui.requestRender();
	}

	render(width: number): string[] {
		const inside = Math.max(1, width - 2);
		const row = (text: string) => {
			const clipped = truncateToWidth(text, inside, "…", true);
			return this.theme.fg("border", "│") + clipped + " ".repeat(Math.max(0, inside - visibleWidth(clipped))) + this.theme.fg("border", "│");
		};
		// Replace SettingsList's generic footer with explicit Save/Cancel controls.
		const content = this.list.render(inside).slice(0, -2);
		const error = this.error ? wrapTextWithAnsi(this.theme.fg("error", this.error.replace(/\p{Cc}/gu, "")), inside) : [];
		const available = Math.max(0, Math.min(36, this.tui.terminal.rows) - 6);
		return [
			this.theme.fg("border", `╭${"─".repeat(inside)}╮`),
			row(this.theme.fg("accent", "Checkpoint settings")),
			row(this.theme.fg("muted", `Across sessions · ${this.config.agentTools ? "Next agent turn; no reload" : "Agent tools disabled"}`)),
			...content.slice(0, Math.max(0, available - error.length)).map(row), ...error.slice(0, available).map(row),
			row("↑↓ Select · Enter/Space Change"), row("Ctrl+S Save · Esc Cancel"),
			this.theme.fg("border", `╰${"─".repeat(inside)}╯`),
		];
	}
}
