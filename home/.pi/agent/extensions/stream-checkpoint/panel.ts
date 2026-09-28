import { Editor, Markdown, matchesKey, truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { Component, KeyId, TUI } from "@earendil-works/pi-tui";
import { getMarkdownTheme, getSelectListTheme, renderDiff } from "@earendil-works/pi-coding-agent";
import type { Theme } from "@earendil-works/pi-coding-agent";
import { anchorAnnotation, annotationContext, diffText, reviewLines, sameAnchor } from "./review.ts";
import type { ReviewLine } from "./review.ts";
import type { Draft } from "./core.ts";
import { activityLabel, shortcutLabel } from "./view.ts";
import type { PanelState } from "./view.ts";

export interface PanelActions {
	send(): void;
	apply(): void;
	refresh(): void;
	clear(): void;
	stop(): void;
	focusMain(): void;
	changed(): void;
}

const safeText = (text: string) => text.replace(/[\x00-\x09\x0b-\x1f\x7f]/g, "");

/** Two views share durable feedback, but only an idle conversation can edit it. */
export class CheckpointPanel implements Component {
	focused = true;
	private tab: "chat" | "review" = "chat";
	private readonly input: Editor;
	private syncingInput = false;
	private expandedAnnotations = false;
	private confirmingClear = false;
	private selected = 0;
	private reviewTop = 0;
	private chatTop: number | undefined;
	private lastChatTop = 0;
	private contentHeight = 1;
	private reviewStarts: number[] = [];
	private diffCache?: { draft: Draft; lines: ReviewLine[]; colored: string[] };
	private transcriptCache: { text: string; markdown: Markdown }[] = [];
	private spinnerTimer?: ReturnType<typeof setInterval>;
	private spinnerFrame = 0;

	constructor(private tui: TUI, private theme: Theme, public state: PanelState, private actions: PanelActions, private focusShortcut: KeyId = "alt+p") {
		this.input = new Editor(tui, { borderColor: (text) => this.theme.fg("border", text), selectList: getSelectListTheme() });
		this.input.onChange = (text) => {
			if (this.syncingInput) return;
			if (this.tab === "review" && this.state.annotationEditor) this.state.annotationEditor.text = text;
			else this.state.input = text;
			this.actions.changed();
		};
		// The controller clears feedback only after the SDK accepts it, not on Enter.
		this.input.disableSubmit = true;
	}

	refresh(): void { this.tui.requestRender(); }
	dispose(): void { clearInterval(this.spinnerTimer); this.spinnerTimer = undefined; }
	invalidate(): void {
		this.transcriptCache.forEach((entry) => entry.markdown.invalidate());
		this.diffCache = undefined;
		this.input.invalidate();
	}

	private diff(): NonNullable<CheckpointPanel["diffCache"]> | undefined {
		if (!this.state.draft) return undefined;
		if (this.diffCache?.draft !== this.state.draft) {
			const lines = reviewLines(this.state.draft);
			this.diffCache = { draft: this.state.draft, lines, colored: renderDiff(lines.map((line) => safeText(diffText(line))).join("\n")).split("\n") };
			this.selected = Math.min(this.selected, Math.max(0, lines.length - 1));
			this.reviewTop = 0;
		}
		return this.diffCache;
	}

	private editAnnotation(): void {
		const line = this.diff()?.lines[this.selected];
		if (!line || !this.state.draft) return;
		const anchor = anchorAnnotation(this.state.draft, line);
		const existing = this.state.annotations.find((comment) => sameAnchor(comment, anchor));
		this.state.annotationEditor = { anchor, text: existing?.comment ?? "" };
		this.actions.changed();
	}

	private saveAnnotation(): void {
		const editor = this.state.annotationEditor!;
		const index = this.state.annotations.findIndex((annotation) => sameAnchor(annotation, editor.anchor));
		const comment = editor.text.trim();
		if (index >= 0) {
			if (comment) this.state.annotations[index] = { ...editor.anchor, comment };
			else this.state.annotations.splice(index, 1);
		} else if (comment) this.state.annotations.push({ ...editor.anchor, comment });
		this.state.annotationEditor = undefined;
		this.actions.changed();
	}

	private syncInput(): void {
		const text = this.tab === "review" ? this.state.annotationEditor?.text ?? "" : this.state.input;
		if (this.input.getText() === text) return;
		this.syncingInput = true;
		this.input.setText(text);
		this.syncingInput = false;
	}

	handleInput(data: string): void {
		if (matchesKey(data, "escape") || matchesKey(data, this.focusShortcut)) {
			this.actions.focusMain();
			return;
		}
		if (this.confirmingClear) {
			if (data === "y") { this.confirmingClear = false; this.actions.clear(); }
			if (data === "n") this.confirmingClear = false;
		} else if (matchesKey(data, "tab")) {
			this.tab = this.tab === "chat" ? "review" : "chat";
		} else if (matchesKey(data, "ctrl+c")) {
			if (this.state.activity === "drafting") this.actions.stop();
			else if (!this.state.activity && this.state.annotationEditor) { this.state.annotationEditor = undefined; this.actions.changed(); }
		} else if (matchesKey(data, "alt+a") && this.tab === "chat") {
			this.expandedAnnotations = !this.expandedAnnotations;
		} else if (matchesKey(data, "pageUp") || matchesKey(data, "pageDown")) {
			const delta = matchesKey(data, "pageUp") ? -this.contentHeight : this.contentHeight;
			if (this.tab === "chat") this.chatTop = Math.max(0, (this.chatTop ?? this.lastChatTop) + delta);
			else {
				this.reviewTop = Math.max(0, this.reviewTop + delta);
				this.selected = Math.max(0, this.reviewStarts.findLastIndex((start) => start <= this.reviewTop));
			}
		} else if (this.tab === "review" && !this.state.annotationEditor && (matchesKey(data, "up") || matchesKey(data, "down"))) {
			const lines = this.diff()?.lines ?? [];
			this.selected = Math.max(0, Math.min(lines.length - 1, this.selected + (matchesKey(data, "down") ? 1 : -1)));
			const start = this.reviewStarts[this.selected] ?? 0;
			if (start < this.reviewTop || start >= this.reviewTop + this.contentHeight) this.reviewTop = start;
		} else if (!this.state.activity) {
			if (matchesKey(data, "alt+c")) this.confirmingClear = true;
			else if (matchesKey(data, "ctrl+g")) this.actions.apply();
			else if (matchesKey(data, "ctrl+r")) this.actions.refresh();
			else if (matchesKey(data, "alt+k") && this.tab === "review") this.editAnnotation();
			else if (matchesKey(data, "enter")) {
				if (this.tab === "review" && this.state.annotationEditor) this.saveAnnotation();
				else if (this.tab === "chat" && (this.state.input.trim() || this.state.annotations.length)) { this.chatTop = undefined; this.actions.send(); }
			} else if (this.tab === "chat" || this.state.annotationEditor) { this.syncInput(); this.input.handleInput(data); }
		}
		this.refresh();
	}

	private chat(width: number): string[] {
		const lines = this.state.messages.flatMap((message, index) => {
			const text = safeText(message.text);
			if (this.transcriptCache[index]?.text !== text) this.transcriptCache[index] = { text, markdown: new Markdown(text, 0, 0, getMarkdownTheme()) };
			const comments = message.annotations ?? [];
			const label = comments.length ? [`[${comments.length} annotation${comments.length === 1 ? "" : "s"} · Draft ${comments[0].revision}]`] : [];
			const details = this.expandedAnnotations ? comments.flatMap((comment) => wrapTextWithAnsi(safeText(annotationContext(comment)), width)) : [];
			return [this.theme.fg("accent", message.role === "user" ? "You" : "Checkpoint"), ...label, ...details, ...this.transcriptCache[index].markdown.render(width), ""];
		});
		this.transcriptCache.length = this.state.messages.length;
		this.lastChatTop = this.chatTop === undefined ? Math.max(0, lines.length - this.contentHeight) : Math.min(this.chatTop, Math.max(0, lines.length - this.contentHeight));
		return lines.slice(this.lastChatTop, this.lastChatTop + this.contentHeight);
	}

	private review(width: number): string[] {
		const diff = this.diff();
		if (!diff) return [this.theme.fg("muted", "No draft yet")];
		const sent = this.state.messages.flatMap((message) => message.annotations ?? []);
		const comments = [...sent, ...this.state.annotations].filter((comment) => comment.revision === diff.draft.revision && comment.path === diff.draft.path);
		const lines: string[] = [];
		this.reviewStarts = [];
		diff.lines.forEach((line, index) => {
			this.reviewStarts.push(lines.length);
			const matching = comments.map((comment, number) => ({ comment, number: number + 1 })).filter(({ comment }) => comment.side === (line.kind === "removed" ? "original" : "proposed") && comment.line === (line.kind === "removed" ? line.oldLine : line.newLine));
			const number = line.kind === "removed" ? line.oldLine : line.newLine;
			const marker = matching.length ? ` [${matching.map((item) => item.number).join(",")}]` : "";
			lines.push(...wrapTextWithAnsi(`${index === this.selected ? "▶" : " "} ${number} ${diff.colored[index]}${marker}`, width));
			for (const { comment } of matching) lines.push(...wrapTextWithAnsi(this.theme.fg("accent", `    └ You: ${safeText(comment.comment)}`), width));
		});
		this.reviewTop = Math.min(this.reviewTop, Math.max(0, lines.length - this.contentHeight));
		return lines.slice(this.reviewTop, this.reviewTop + this.contentHeight);
	}

	private shortcuts(width: number): string {
		const hide = `${shortcutLabel(this.focusShortcut)} Hide`;
		if (this.state.activity) return ["Tab", ...(this.state.activity === "drafting" ? ["Ctrl+C Stop"] : []), hide].join(" · ");
		if (this.tab === "review" && this.state.annotationEditor) return `Enter Save · Ctrl+C Cancel · ${hide}`;
		const primary = this.tab === "review" ? "Alt+K Comment" : this.state.draft ? "Ctrl+G Apply" : "Ctrl+R Refresh";
		let hints = `Tab · ${primary} · ${hide} · Alt+C Clear`;
		const secondary = this.tab === "review" ? ["Ctrl+G Apply", "Ctrl+R Refresh"] : ["Ctrl+R Refresh", ...(this.state.messages.some((message) => message.annotations?.length) || this.state.annotations.length ? ["Alt+A Details"] : [])];
		for (const hint of secondary) if (!hints.includes(hint) && visibleWidth(`${hints} · ${hint}`) <= width) hints += ` · ${hint}`;
		return hints;
	}

	render(width: number): string[] {
		const inside = Math.max(1, width - 2);
		const height = Math.min(36, this.tui.terminal.rows);
		const row = (text: string) => {
			const clipped = truncateToWidth(text, inside, "…", true);
			return this.theme.fg("border", "│") + clipped + " ".repeat(Math.max(0, inside - visibleWidth(clipped))) + this.theme.fg("border", "│");
		};
		const border = (left: string, right: string) => this.theme.fg("border", `${left}${"─".repeat(inside)}${right}`);
		if (this.state.activity && !this.spinnerTimer) {
			this.spinnerTimer = setInterval(() => { this.spinnerFrame = (this.spinnerFrame + 1) % 10; this.refresh(); }, 80);
			this.spinnerTimer.unref?.();
		} else if (!this.state.activity) this.dispose();
		const spinner = this.state.activity ? `${["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][this.spinnerFrame]} ${activityLabel(this.state)}` : "";
		const tab = (name: string, active: boolean) => this.theme.fg(active ? "accent" : "muted", active ? `[${name}]` : name);
		const heading = this.theme.fg("mdHeading", "Streams Checkpoint") + `  ${tab("Chat", this.tab === "chat")}  ${tab(`Review${this.state.draft ? ` · ${this.state.draft.revision}` : ""}`, this.tab === "review")}`;
		const header = heading + (spinner ? `  ${this.theme.fg("accent", spinner)}` : "");
		this.syncInput();
		this.input.focused = this.focused && !this.state.activity && (this.tab === "chat" || !!this.state.annotationEditor);
		const editor = this.input.render(inside);
		const composer = editor.length > Math.max(3, height - 8) ? [editor[0], ...editor.slice(-(Math.max(3, height - 8) - 1))] : editor;
		const context: string[] = [];
		if (this.confirmingClear) context.push(this.theme.fg("warning", "Discard pending chat and draft? y/n"));
		else if (this.state.notice) context.push(this.theme.fg(this.state.notice.level === "info" ? "muted" : this.state.notice.level, safeText(this.state.notice.text)));
		if (this.tab === "review" && this.state.annotationEditor) {
			const anchor = this.state.annotationEditor.anchor;
			context.push(`Comment · Draft ${anchor.revision} · ${anchor.side} line ${anchor.line}`);
		} else if (this.tab === "chat" && this.state.annotations.length) {
			context.push(`[${this.state.annotations.length} annotations · not sent] · Tab to review`);
			if (this.expandedAnnotations) context.push(...this.state.annotations.flatMap((comment) => wrapTextWithAnsi(safeText(annotationContext(comment)), inside)));
		}
		const contextRows = context.slice(0, Math.max(0, height - composer.length - 6));
		this.contentHeight = Math.max(0, height - composer.length - contextRows.length - 5);
		const content = this.tab === "review" ? this.review(inside) : this.chat(inside);
		const headerDivider = content.length || contextRows.length ? [border("├", "┤")] : [];
		return [border("╭", "╮"), row(header), ...headerDivider, ...content.map(row), ...contextRows.map(row), ...composer.map(row), row(this.shortcuts(inside)), border("╰", "╯")];
	}
}
