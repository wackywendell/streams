import { Editor, Input, Markdown, matchesKey, truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { getMarkdownTheme, getSelectListTheme, renderDiff } from "@earendil-works/pi-coding-agent";
import type { Component, KeyId, TUI } from "@earendil-works/pi-tui";
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
	stop?(): Promise<void>;
	focusMain?(): void;
	inputChanged?(value: string): void;
}

/** Chat-first overlay: the review expands in place and the composer stays available. */
export class CheckpointPanel implements Component {
	focused = true;
	readonly input: Editor;
	private region: "input" | "review" | "chat" = "input";
	private chatTop: number | undefined;
	private lastChatTop = 0;
	private reviewTop: number | undefined;
	private lastReviewTop = 0;
	private transcriptCache: { text: string; markdown: Markdown }[] = [];
	private diffCache?: { draft: Draft; lines: string[]; colored: string[] };
	private selectedDiff = 0;
	private confirmingClear = false;
	private busy: "send" | "apply" | "refresh" | undefined;
	private closing = false;
	private spinnerFrame = 0;
	private spinnerTimer?: ReturnType<typeof setInterval>;

	constructor(private tui: TUI, private theme: Theme, public state: PanelState, private actions: PanelActions, private done: () => void, private focusShortcut: KeyId = "alt+p") {
		this.input = new Editor(tui, { borderColor: (text) => theme.fg("border", text), selectList: getSelectListTheme() });
		this.input.onChange = (text) => this.actions.inputChanged?.(text);
		this.input.onSubmit = (text) => {
			if (!text.trim() || this.busy) return;
			this.input.setText("");
			this.perform("send", () => this.actions.send(text));
		};
	}

	getInput(): string { return this.input.getText(); }
	setInput(value: string): void { this.input.setText(value); this.tui.requestRender(); }
	startMessage(text: string): void { this.perform("send", () => this.actions.send(text)); }
	private diffLines(): string[] {
		if (!this.state.draft) return [];
		if (this.diffCache?.draft !== this.state.draft) {
			this.reviewTop = undefined;
			const lines = draftDiff(this.state.draft.baseContent, this.state.draft.proposedContent);
			this.diffCache = { draft: this.state.draft, lines, colored: renderDiff(lines.map((line) => line.replace(/[\x00-\x09\x0b-\x1f\x7f]/g, "")).join("\n")).split("\n") };
			this.selectedDiff = Math.min(this.selectedDiff, Math.max(0, lines.length - 1));
		}
		return this.diffCache.lines;
	}
	refresh(): void { this.tui.requestRender(); }
	dispose(): void {
		if (this.spinnerTimer) clearInterval(this.spinnerTimer);
		this.spinnerTimer = undefined;
	}
	invalidate(): void {
		this.transcriptCache.forEach((entry) => entry.markdown.invalidate());
		this.diffCache = undefined;
		this.input.invalidate();
	}

	private perform(kind: "send" | "apply" | "refresh", work: () => Promise<void>): void {
		if (this.busy) return;
		this.busy = kind;
		this.input.disableSubmit = true;
		this.tui.requestRender();
		void work().catch((error) => { this.state.status = String(error); }).finally(() => {
			this.busy = undefined;
			this.input.disableSubmit = false;
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
		if (matchesKey(data, "alt+r")) {
			this.region = this.region === "review" ? "input" : this.state.draft ? "review" : "input";
		} else if (matchesKey(data, "tab")) {
			this.region = this.region === "input" ? (this.state.draft ? "review" : "chat") : this.region === "review" ? "chat" : "input";
		} else if (matchesKey(data, "escape")) {
			if (this.busy === "send" && this.actions.stop) void this.actions.stop().catch((error) => { this.state.status = String(error); this.refresh(); });
			else if (!this.busy) this.close(false);
		} else if (matchesKey(data, "alt+c")) {
			if (!this.busy) this.confirmingClear = true;
		} else if (matchesKey(data, "ctrl+g")) {
			this.perform("apply", () => this.actions.apply());
		} else if (matchesKey(data, "ctrl+r")) {
			this.perform("refresh", () => this.actions.refresh());
		} else if (matchesKey(data, this.focusShortcut) || matchesKey(data, "ctrl+o")) {
			this.actions.focusMain?.();
		} else if (matchesKey(data, "alt+k") && this.region === "review" && this.state.draft) {
			const line = this.diffLines()[this.selectedDiff];
			if (line) {
				this.input.insertTextAtCursor(annotationText(this.state.draft.revision, line));
				this.region = "input";
			}
		} else if (this.region === "review" && (matchesKey(data, "up") || matchesKey(data, "down"))) {
			const last = Math.max(0, this.diffLines().length - 1);
			this.selectedDiff = Math.max(0, Math.min(last, this.selectedDiff + (matchesKey(data, "down") ? 1 : -1)));
			this.reviewTop = undefined;
		} else if (this.region === "review" && (matchesKey(data, "pageUp") || matchesKey(data, "pageDown"))) {
			this.reviewTop = Math.max(0, this.lastReviewTop + (matchesKey(data, "pageUp") ? -10 : 10));
		} else if (this.region === "chat" && (matchesKey(data, "up") || matchesKey(data, "down") || matchesKey(data, "pageUp") || matchesKey(data, "pageDown"))) {
			const delta = matchesKey(data, "up") ? -1 : matchesKey(data, "down") ? 1 : matchesKey(data, "pageUp") ? -10 : 10;
			this.chatTop = Math.max(0, (this.chatTop ?? this.lastChatTop) + delta);
		} else if (this.region === "input") {
			this.input.handleInput(data);
		}
		this.tui.requestRender();
	}

	render(width: number): string[] {
		const inside = Math.max(1, width - 2);
		const maxHeight = Math.min(36, this.tui.terminal.rows);
		const compact = maxHeight < 16;
		const running = !!this.busy || /^(Starting checkpoint|Drafting handoff|Checkpoint is responding|Reading with )/.test(this.state.status);
		if (running && !this.spinnerTimer) {
			this.spinnerTimer = setInterval(() => { this.spinnerFrame = (this.spinnerFrame + 1) % 10; this.tui.requestRender(); }, 80);
			this.spinnerTimer.unref?.();
		} else if (!running && this.spinnerTimer) this.dispose();
		const spinner = running ? this.theme.fg("accent", `${["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][this.spinnerFrame]} `) : "";
		const safe = (text: string) => text.replace(/[\x00-\x09\x0b-\x1f\x7f]/g, "");
		const row = (line: string, trusted = false) => {
			const clipped = truncateToWidth(trusted ? line : safe(line), inside, "…", true);
			return this.theme.fg("border", "│") + clipped + " ".repeat(Math.max(0, inside - visibleWidth(clipped))) + this.theme.fg("border", "│");
		};
		this.input.focused = this.region === "input" && this.focused;
		const composer = this.input.render(inside).map((line) => row(line, true));
		const available = Math.max(0, maxHeight - composer.length - (compact ? 5 : 8));
		let content: string[];
		if (this.region === "review" && this.state.draft) {
			this.diffLines();
			const starts: number[] = [];
			const lines: string[] = [];
			this.diffCache!.colored.forEach((line, index) => {
				starts.push(lines.length);
				lines.push(...wrapTextWithAnsi(`${index === this.selectedDiff ? "▶ " : "  "}${line}`, inside));
			});
			this.lastReviewTop = Math.max(0, Math.min(this.reviewTop ?? starts[this.selectedDiff] ?? 0, Math.max(0, lines.length - available)));
			content = lines.slice(this.lastReviewTop, this.lastReviewTop + available);
		} else {
			const chat = this.state.messages.flatMap((message, index) => {
				const label = message.startsWith("You: ") ? "You" : "Checkpoint";
				const text = safe(message.replace(/^(?:You|Checkpoint): /, ""));
				if (this.transcriptCache[index]?.text !== text) this.transcriptCache[index] = { text, markdown: new Markdown(text, 0, 0, getMarkdownTheme()) };
				return [this.theme.fg("accent", label), ...this.transcriptCache[index].markdown.render(inside), ""];
			});
			this.transcriptCache.length = this.state.messages.length;
			const maxTop = Math.max(0, chat.length - available);
			this.lastChatTop = this.chatTop === undefined ? maxTop : Math.min(this.chatTop, maxTop);
			content = chat.slice(this.lastChatTop, this.lastChatTop + available);
		}
		while (content.length < available) content.push("");
		return [
			this.theme.fg("border", `╭${"─".repeat(inside)}╮`),
			row(`${this.state.title} · ${this.state.slug}${compact && this.state.draft ? ` · Draft ${this.state.draft.revision}` : ""}`),
			...(!compact ? [row(this.state.draft ? `Draft ${this.state.draft.revision}: ${this.state.draft.summary ?? "Scratch update"} · Alt+R ${this.region === "review" ? "Chat" : "Review"}` : "Side conversation")] : []),
			...content.map((line) => row(line, true)),
			...composer,
			row(this.confirmingClear ? "Discard pending chat and draft? y/n" : `${spinner}${this.busy === "send" ? "Responding · Esc stop · " : this.busy === "apply" ? "Applying · " : this.busy === "refresh" ? "Refreshing · " : ""}${this.state.status}`, true),
			row(compact ? "Enter Send · Tab Focus · Alt+R Review · Esc Close" : this.region === "review" ? "↑↓ Select · PgUp/PgDn Scroll · Alt+K Comment" : "Enter Send · Shift+Enter Newline · Tab Focus"),
			...(!compact ? [row("Alt+R Review · Ctrl+G Apply · Ctrl+R Refresh"), row(`Esc Stop/Close · ${this.focusShortcut} Main · Alt+C Clear`)] : []),
			this.theme.fg("border", `╰${"─".repeat(inside)}╯`),
		];
	}
}

export type ChosenStream = { stream: Awaited<ReturnType<StreamCli["list"]>>[number]; created: boolean };
type ChooserChoice = { kind: "existing"; stream: ChosenStream["stream"] } | { kind: "new"; title: string; reason: string };

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
			const choice = this.choices()[this.selected];
			if (!value.trim() && choice) this.choose(choice);
			else if (value.trim()) this.run(async () => { this.done({ stream: await this.cli.show(value.trim()), created: false }); });
		};
	}

	private choices(): ChooserChoice[] {
		const query = this.input.getValue().trim().toLowerCase();
		const matching = this.streams.filter((item) => item.status !== "archived" && (!query || `${item.title} ${item.slug}`.toLowerCase().includes(query)));
		const rank = (slug: string) => this.recommendations.existing.findIndex((item) => item.slug === slug);
		const existing: ChooserChoice[] = matching.sort((a, b) => {
			const ar = rank(a.slug);
			const br = rank(b.slug);
			if (ar >= 0 || br >= 0) return ar < 0 ? 1 : br < 0 ? -1 : ar - br;
			return Number(b.status === "active") - Number(a.status === "active") || a.title.localeCompare(b.title);
		}).slice(0, this.expanded ? undefined : 20).map((stream) => ({ kind: "existing", stream }));
		const newStream = this.recommendations.newStream;
		return newStream && (!query || newStream.title.toLowerCase().includes(query)) ? [{ kind: "new", ...newStream }, ...existing] : existing;
	}

	private choose(choice: ChooserChoice): void {
		if (choice.kind === "new") this.run(async () => { this.preview = await this.cli.previewStart(choice.title); });
		else this.done({ stream: choice.stream, created: false });
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
		} else if (matchesKey(data, "alt+m")) {
			this.expanded = true;
		} else if (matchesKey(data, "tab")) {
			const selected = this.choices()[this.selected];
			if (selected) this.choose(selected);
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
		const row = (text: string, styled = false) => {
			const clipped = truncateToWidth(styled ? text : text.replace(/\p{Cc}/gu, ""), inside, "…", true);
			return this.theme.fg("border", "│") + clipped + " ".repeat(Math.max(0, inside - visibleWidth(clipped))) + this.theme.fg("border", "│");
		};
		const choices = this.choices();
		const reasonRows = (reason: string) => wrapTextWithAnsi(this.theme.fg("muted", `  ↳ ${reason.replace(/\p{Cc}/gu, "")}`), inside).map((line) => row(line, true));
		const hint = this.recommendations.newStream ? [] : [row("Enter a title and Ctrl+N to preview creation")];
		const previewRows = this.preview
			? wrapTextWithAnsi(`Create ${this.preview.title} (${this.preview.slug}) in ${String(this.preview.cwd)}? y/n`, inside).map(row)
			: [row(this.busy ? "Working…" : this.status)];
		const maxHeight = Math.min(36, Math.floor(this.tui.terminal.rows * 0.8));
		const available = Math.max(1, maxHeight - 5 - hint.length - previewRows.length);
		const renderChoices = (first: number) => {
			const lines: string[] = [];
			let used = 0;
			let selectedVisible = false;
			for (let index = first; index < choices.length; index++) {
				const item = choices[index];
				const reason = item.kind === "new" ? item.reason : this.recommendations.existing.find((suggestion) => suggestion.slug === item.stream.slug)?.reason;
				const explanation = reason ? reasonRows(reason) : [];
				const choiceRows = item.kind === "new"
					? wrapTextWithAnsi(`${index === this.selected ? "▶" : " "} ${this.theme.fg("accent", `★ Suggested new: ${item.title}`)} ${this.theme.fg("muted", "· Enter preview")}`, Math.max(1, inside - 2)).map((text, line) => row(line ? `  ${text}` : text, true))
					: [row(`${index === this.selected ? "▶" : " "} ${reason ? this.theme.fg("accent", "★ ") : ""}${this.theme.fg("accent", item.stream.title)} · ${this.theme.fg(item.stream.status === "active" ? "success" : item.stream.status === "blocked" ? "error" : item.stream.status === "waiting" ? "warning" : "muted", item.stream.status)} · ${this.theme.italic?.(item.stream.state) ?? item.stream.state} · ${this.theme.fg("muted", item.stream.slug)}`, true)];
				const spacer = item.kind === "new" && index < choices.length - 1 ? 1 : 0;
				const cost = choiceRows.length + explanation.length + spacer;
				if (used + cost > available && lines.length) break;
				lines.push(...choiceRows, ...explanation);
				if (spacer) lines.push(row(""));
				selectedVisible ||= index === this.selected;
				used += cost;
			}
			return { lines, selectedVisible };
		};
		let rendered = renderChoices(Math.max(0, this.selected - Math.floor(available / 2)));
		if (choices.length && !rendered.selectedVisible) rendered = renderChoices(this.selected);
		this.input.focused = this.focused && !this.preview;
		const inputLine = this.input.render(inside)[0] ?? "";
		const inputRow = this.theme.fg("border", "│") + inputLine + " ".repeat(Math.max(0, inside - visibleWidth(inputLine))) + this.theme.fg("border", "│");
		return [
			this.theme.fg("border", `╭${"─".repeat(inside)}╮`),
			row("Select or create a stream"),
			...hint,
			...(rendered.lines.length ? rendered.lines : [row("No matching streams")]),
			inputRow,
			...previewRows,
			row("Tab select highlighted · Alt+M Show more · Esc Cancel"),
			this.theme.fg("border", `╰${"─".repeat(inside)}╯`),
		];
	}
}
