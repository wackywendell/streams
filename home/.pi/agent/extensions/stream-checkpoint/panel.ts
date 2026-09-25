import { Input, matchesKey, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { Component, TUI } from "@earendil-works/pi-tui";
import type { Theme } from "@earendil-works/pi-coding-agent";
import type { Draft, Recommendations } from "./core.ts";
import type { StreamCli } from "./streams.ts";

export function draftDiff(base: string, proposed: string): string[] {
	const before = base.replace(/\n$/, "").split("\n");
	const after = proposed.replace(/\n$/, "").split("\n");
	if (base === proposed) return [];
	// Fall back to a full replacement for unusually large notes rather than
	// spending quadratic time on a terminal render.
	if (before.length * after.length > 2_000_000) return [...before.map((line) => `- ${line}`), ...after.map((line) => `+ ${line}`)];
	const counts = Array.from({ length: before.length + 1 }, () => new Uint32Array(after.length + 1));
	for (let i = before.length - 1; i >= 0; i--) {
		for (let j = after.length - 1; j >= 0; j--) {
			counts[i][j] = before[i] === after[j] ? counts[i + 1][j + 1] + 1 : Math.max(counts[i + 1][j], counts[i][j + 1]);
		}
	}
	const lines: string[] = [];
	let i = 0;
	let j = 0;
	while (i < before.length || j < after.length) {
		if (i < before.length && j < after.length && before[i] === after[j]) {
			lines.push(`  ${before[i++]}`);
			j++;
		} else if (i < before.length && (j === after.length || counts[i + 1][j] >= counts[i][j + 1])) {
			lines.push(`- ${before[i++]}`);
		} else {
			lines.push(`+ ${after[j++]}`);
		}
	}
	return lines;
}

export function annotationText(revision: number, selectedLine: string): string {
	return `Regarding draft ${revision}, ${JSON.stringify(selectedLine)}: `;
}

export interface PanelState {
	slug: string;
	title: string;
	messages: string[];
	draft?: Draft;
	status: string;
}

export interface PanelActions {
	send(text: string): Promise<void>;
	apply(): Promise<void>;
	refresh(): Promise<void>;
	close(clear: boolean): Promise<void>;
	focusMain?(): void;
	inputChanged?(value: string): void;
}

/** Right-anchored overlay: chat and draft scroll independently; input remains visible. */
export class CheckpointPanel implements Component {
	focused = true;
	readonly input = new Input({ placeholder: "Type a message here…" });
	private region: "input" | "diff" | "chat" = "input";
	private chatOffset = 0;
	private diffOffset = 0;
	private selectedDiff = 0;
	private confirmingClear = false;
	private busy = false;
	private closing = false;

	constructor(private tui: TUI, private theme: Theme, public state: PanelState, private actions: PanelActions, private done: () => void) {
		this.input.onSubmit = (text) => {
			if (!text.trim() || this.busy) return;
			this.perform(async () => {
				await this.actions.send(text);
				this.input.setValue("");
				this.actions.inputChanged?.("");
			});
		};
	}

	getInput(): string { return this.input.getValue(); }
	setInput(value: string): void { this.input.setValue(value); this.tui.requestRender(); }
	refresh(): void { this.tui.requestRender(); }

	private perform(work: () => Promise<void>): void {
		if (this.busy) return;
		this.busy = true;
		this.tui.requestRender();
		void work().catch((error) => { this.state.status = String(error); }).finally(() => {
			this.busy = false;
			this.tui.requestRender();
		});
	}

	private close(clear: boolean): void {
		if (this.closing) return;
		this.closing = true;
		void this.actions.close(clear).then(() => this.done()).catch((error) => {
			this.closing = false;
			this.state.status = String(error);
			this.tui.requestRender();
		});
	}

	handleInput(data: string): void {
		if (this.confirmingClear) {
			if (data === "y") this.close(true);
			if (data === "n" || matchesKey(data, "escape")) this.confirmingClear = false;
			this.tui.requestRender();
			return;
		}
		if (matchesKey(data, "tab")) {
			this.region = this.region === "input" ? "diff" : this.region === "diff" ? "chat" : "input";
		} else if (matchesKey(data, "escape")) {
			this.close(false);
		} else if (matchesKey(data, "ctrl+d")) {
			this.confirmingClear = true;
		} else if (matchesKey(data, "ctrl+g")) {
			this.perform(() => this.actions.apply());
		} else if (matchesKey(data, "ctrl+r")) {
			this.perform(() => this.actions.refresh());
		} else if (matchesKey(data, "ctrl+o")) {
			this.actions.focusMain?.();
		} else if (matchesKey(data, "ctrl+k") && this.region === "diff" && this.state.draft) {
			const line = draftDiff(this.state.draft.baseContent, this.state.draft.proposedContent)[this.selectedDiff];
			if (line) {
				this.input.setValue(this.input.getValue() + annotationText(this.state.draft.revision, line));
				this.actions.inputChanged?.(this.input.getValue());
				this.region = "input";
			}
		} else if (this.region === "diff" && (matchesKey(data, "up") || matchesKey(data, "down"))) {
			const last = Math.max(0, draftDiff(this.state.draft?.baseContent ?? "", this.state.draft?.proposedContent ?? "").length - 1);
			this.selectedDiff = Math.max(0, Math.min(last, this.selectedDiff + (matchesKey(data, "down") ? 1 : -1)));
			this.diffOffset = Math.max(0, this.selectedDiff - 3);
		} else if (this.region === "chat" && (matchesKey(data, "up") || matchesKey(data, "down"))) {
			this.chatOffset = Math.max(0, this.chatOffset + (matchesKey(data, "down") ? 1 : -1));
		} else if (this.region === "input" && !this.busy) {
			this.input.handleInput(data);
			this.actions.inputChanged?.(this.input.getValue());
		}
		this.tui.requestRender();
	}

	render(width: number): string[] {
		const inside = Math.max(1, width - 2);
		const height = this.tui.terminal.rows;
		const compact = height < 18;
		const visible = Math.max(1, height - 9);
		const chatHeight = !this.state.draft ? Math.max(1, height - 9) : compact ? (this.region === "diff" ? 0 : visible) : Math.max(2, Math.min(12, Math.floor((height - 10) / 2)));
		const diffHeight = !this.state.draft ? 0 : compact ? (this.region === "diff" ? visible : 0) : Math.max(2, Math.min(12, height - chatHeight - 10));
		const safe = (line: string) => line.replace(/\p{Cc}/gu, "");
		const row = (line: string, trusted = false) => {
			const clipped = truncateToWidth(trusted ? line : safe(line), inside, "…", true);
			return this.theme.fg("border", "│") + clipped + " ".repeat(Math.max(0, inside - visibleWidth(clipped))) + this.theme.fg("border", "│");
		};
		const section = (label: string) => row(this.theme.fg("accent", label), true);
		const chat = this.state.messages.flatMap((message) => message.split("\n"));
		const diff = this.state.draft ? draftDiff(this.state.draft.baseContent, this.state.draft.proposedContent) : [];
		const viewport = (lines: string[], count: number, offset: number, fallback: string) => {
			const start = Math.max(0, Math.min(offset, Math.max(0, lines.length - count)));
			return Array.from({ length: count }, (_, i) => row(lines[start + i] ?? (i === 0 && !lines.length ? fallback : "")));
		};
		const chatStart = Math.max(0, chat.length - chatHeight - this.chatOffset);
		this.input.focused = this.region === "input" && this.focused;
		const inputLine = this.input.render(inside)[0] ?? "";
		return [
			this.theme.fg("border", `╭${"─".repeat(inside)}╮`),
			row(`${this.state.title} · ${this.state.slug}`),
			section(`${this.region === "chat" ? "▶ " : ""}Side conversation`),
			...viewport(chat, chatHeight, chatStart, "No messages yet"),
			section(`${this.region === "diff" ? "▶ " : ""}Draft ${this.state.draft?.revision ?? "—"} · ↑↓ scroll · Ctrl+K comment`),
			...viewport(diff.map((line, i) => i === this.selectedDiff && this.region === "diff" ? `▶ ${line}` : `  ${line}`), diffHeight, this.diffOffset, "No draft yet"),
			row(inputLine, true),
			row(this.confirmingClear ? "Discard pending chat and draft? y/n" : this.busy ? "Working…" : this.state.status),
			row("Tab focus · Enter send · Ctrl+G Apply · Ctrl+R Refresh"),
			row("Ctrl+O Main editor · Esc Close · Ctrl+D Close + clear"),
			this.theme.fg("border", `╰${"─".repeat(inside)}╯`),
		];
	}
}

export type ChosenStream = { stream: Awaited<ReturnType<StreamCli["list"]>>[number]; created: boolean };

export class StreamChooser implements Component {
	focused = true;
	readonly input = new Input({ placeholder: "Search streams or enter a title…" });
	private selected = 0;
	private expanded = false;
	private preview?: ChosenStream["stream"];
	private busy = false;
	private status = "↑↓ choose · Enter select · Ctrl+N preview new stream";

	constructor(
		private tui: TUI,
		private theme: Theme,
		private streams: ChosenStream["stream"][],
		private cli: Pick<StreamCli, "show" | "previewStart" | "start" | "list">,
		private done: (choice: ChosenStream | undefined) => void,
		private recommendations: Recommendations = { existing: [] },
		initialRef = "",
	) {
		this.input.setValue(initialRef);
		this.input.onSubmit = (value) => {
			const choices = this.choices();
			if (!value.trim() && choices[this.selected]) this.done({ stream: choices[this.selected], created: false });
			else if (value.trim()) this.run(async () => { this.done({ stream: await this.cli.show(value.trim()), created: false }); });
		};
	}

	private choices(): ChosenStream["stream"][] {
		const query = this.input.getValue().trim().toLowerCase();
		const matching = this.streams.filter((item) => item.status !== "archived" && (!query || `${item.title} ${item.slug}`.toLowerCase().includes(query)));
		const rank = (slug: string) => this.recommendations.existing.findIndex((item) => item.slug === slug);
		return matching.sort((a, b) => {
			const ar = rank(a.slug);
			const br = rank(b.slug);
			if (ar >= 0 || br >= 0) return ar < 0 ? 1 : br < 0 ? -1 : ar - br;
			return Number(b.status === "active") - Number(a.status === "active") || a.title.localeCompare(b.title);
		}).slice(0, this.expanded ? undefined : 20);
	}

	private run(work: () => Promise<void>): void {
		if (this.busy) return;
		this.busy = true;
		void work().catch((error) => { this.status = String(error); }).finally(() => { this.busy = false; this.tui.requestRender(); });
	}

	handleInput(data: string): void {
		if (this.preview) {
			if (data === "y") this.run(async () => {
				const preview = this.preview!;
				const existing = await this.cli.list();
				if (existing.some((item) => item.slug === preview.slug)) throw new Error(`Stream ${preview.slug} now exists; select it instead`);
				this.done({ stream: await this.cli.start(preview.title), created: true });
			});
			if (data === "n" || matchesKey(data, "escape")) this.preview = undefined;
		} else if (matchesKey(data, "escape")) {
			this.done(undefined);
		} else if (matchesKey(data, "ctrl+m")) {
			this.expanded = true;
		} else if (matchesKey(data, "tab")) {
			const selected = this.choices()[this.selected];
			if (selected) this.done({ stream: selected, created: false });
		} else if (matchesKey(data, "ctrl+n")) {
			const title = this.input.getValue().trim() || this.recommendations.newStream?.title;
			if (title) this.run(async () => { this.preview = await this.cli.previewStart(title); });
			else this.status = "Enter a title before previewing creation";
		} else if (matchesKey(data, "up") || matchesKey(data, "down")) {
			this.selected = Math.max(0, Math.min(this.choices().length - 1, this.selected + (matchesKey(data, "down") ? 1 : -1)));
		} else {
			this.input.handleInput(data);
			this.selected = Math.min(this.selected, Math.max(0, this.choices().length - 1));
		}
		this.tui.requestRender();
	}

	render(width: number): string[] {
		const inside = Math.max(1, width - 2);
		const row = (text: string) => {
			const clipped = truncateToWidth(text.replace(/\p{Cc}/gu, ""), inside, "…", true);
			return this.theme.fg("border", "│") + clipped + " ".repeat(Math.max(0, inside - visibleWidth(clipped))) + this.theme.fg("border", "│");
		};
		const choices = this.choices();
		const selectedReason = this.recommendations.existing.find((item) => item.slug === choices[this.selected]?.slug)?.reason;
		const rows = Math.max(3, Math.min(choices.length, this.tui.terminal.rows - 10));
		const first = Math.max(0, Math.min(this.selected - rows + 1, choices.length - rows));
		this.input.focused = this.focused && !this.preview;
		const inputLine = this.input.render(inside)[0] ?? "";
		const inputRow = this.theme.fg("border", "│") + inputLine + " ".repeat(Math.max(0, inside - visibleWidth(inputLine))) + this.theme.fg("border", "│");
		return [
			this.theme.fg("border", `╭${"─".repeat(inside)}╮`),
			row("Select or create a stream"),
			row(this.recommendations.newStream ? `Suggested new: ${this.recommendations.newStream.title} — ${this.recommendations.newStream.reason} (Ctrl+N to preview)` : "Enter a title and Ctrl+N to preview creation"),
			...Array.from({ length: rows }, (_, index) => {
				const item = choices[first + index];
				const reason = this.recommendations.existing.find((suggestion) => suggestion.slug === item?.slug)?.reason;
				return row(item ? `${first + index === this.selected ? "▶" : " "} ${item.title} · ${item.status} · ${item.state} · ${item.slug}${reason ? ` · Suggested: ${reason}` : ""}` : "");
			}),
			inputRow,
			row(this.preview ? `Create ${this.preview.title} (${this.preview.slug}) in ${String(this.preview.cwd)}? y/n` : this.busy ? "Working…" : this.status !== "↑↓ choose · Enter select · Ctrl+N preview new stream" ? this.status : selectedReason ? `Suggested because: ${selectedReason}` : this.status),
			row("Tab select highlighted · Ctrl+M Show more · Esc Cancel"),
			this.theme.fg("border", `╰${"─".repeat(inside)}╯`),
		];
	}
}
