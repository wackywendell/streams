import { Input, matchesKey, truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { Component, TUI } from "@earendil-works/pi-tui";
import type { Theme } from "@earendil-works/pi-coding-agent";
import type { Recommendations } from "./core.ts";
import type { StreamCli } from "./streams.ts";
import { shortcutLabel } from "./view.ts";

export type ChosenStream = { stream: Awaited<ReturnType<StreamCli["list"]>>[number]; created: boolean };
type ChooserChoice = { kind: "existing"; stream: ChosenStream["stream"] } | { kind: "new"; title: string; reason: string } | { kind: "manual" };

export class StreamChooser implements Component {
	focused = true;
	readonly input = new Input({ placeholder: "Search streams…" });
	readonly titleInput = new Input({ placeholder: "New stream title…" });
	private selected = 0;
	private expanded = false;
	private creating = false;
	/** Why the title being edited was suggested, if it was. */
	private creatingReason?: string;
	private closed = false;
	private busy = false;
	private moved = false;
	private status = "↑↓ choose · Enter select · Ctrl+N Write your own";

	constructor(
		private tui: TUI,
		private theme: Theme,
		private streams: ChosenStream["stream"][],
		private cli: Pick<StreamCli, "show" | "previewStart" | "start" | "list">,
		private done: (choice: ChosenStream | undefined) => void,
		private recommendations: Recommendations = { existing: [] },
		initialRef = "",
		/**
		 * `minimize` hides the chooser on `shortcut`; `finding` shows that recommendations are still on their way.
		 * `reason` says why the chooser opened. `create` opens it editing the suggested new stream, with Esc back to the list.
		 */
		private options: { shortcut?: string; minimize?: () => void; finding?: boolean; reason?: string; create?: boolean } = {},
	) {
		this.input.setValue(initialRef);
		const suggested = recommendations.newStream;
		if (options.create && suggested) this.editTitle(suggested.title, suggested.reason);
		this.input.onSubmit = (value) => {
			const choice = this.choices()[this.selected];
			if (!value.trim() && choice) this.choose(choice);
			else if (value.trim()) this.run(async () => { this.finish({ stream: await this.cli.show(value.trim()), created: false }); });
		};
		this.titleInput.onSubmit = (value) => {
			const title = value.trim();
			if (!title) { this.status = "Enter a title"; return; }
			this.run(async () => {
				const preview = await this.cli.previewStart(title);
				if (this.closed) return;
				const existing = await this.cli.list();
				if (this.closed) return;
				if (existing.some((item) => item.slug === preview.slug)) throw new Error(`Stream ${preview.slug} now exists; select it instead`);
				this.finish({ stream: await this.cli.start(title), created: true });
			});
		};
	}

	setRecommendations(recommendations: Recommendations): void {
		this.recommendations = recommendations;
		this.options.finding = false;
		if (!this.moved) this.selected = 0;
		if (!this.closed) this.tui.requestRender();
	}

	private choices(): ChooserChoice[] {
		const query = this.input.getValue().trim().toLowerCase();
		const matching = this.streams.filter((item) => !query || `${item.title} ${item.slug}`.toLowerCase().includes(query));
		const rank = (slug: string) => this.recommendations.existing.findIndex((item) => item.slug === slug);
		const existing: ChooserChoice[] = matching.sort((a, b) => {
			const ar = rank(a.slug);
			const br = rank(b.slug);
			if (ar >= 0 || br >= 0) return ar < 0 ? 1 : br < 0 ? -1 : ar - br;
			return Number(b.status === "active") - Number(a.status === "active") || a.title.localeCompare(b.title);
		}).slice(0, this.expanded ? undefined : 20).map((stream) => ({ kind: "existing", stream }));
		const newStream = this.recommendations.newStream;
		return [...(newStream && (!query || newStream.title.toLowerCase().includes(query)) ? [{ kind: "new" as const, ...newStream }] : []), ...existing, { kind: "manual" }];
	}

	private choose(choice: ChooserChoice): void {
		if (choice.kind === "existing") this.finish({ stream: choice.stream, created: false });
		else if (choice.kind === "new") this.editTitle(choice.title, choice.reason);
		else this.editTitle("");
	}

	private editTitle(title: string, reason?: string): void {
		this.creating = true;
		this.creatingReason = reason;
		this.titleInput.setValue(title);
		this.status = "Enter creates the stream; the checkpoint draft still needs Apply";
	}

	private finish(choice: ChosenStream | undefined): void {
		if (this.closed) return;
		this.closed = true;
		this.done(choice);
	}

	private run(work: () => Promise<void>): void {
		if (this.busy || this.closed) return;
		this.busy = true;
		void work().catch((error) => { this.status = String(error); }).finally(() => { this.busy = false; if (!this.closed) this.tui.requestRender(); });
	}

	handleInput(data: string): void {
		if (this.closed) return;
		if (this.options.minimize && this.options.shortcut && matchesKey(data, this.options.shortcut)) { this.options.minimize(); return; }
		if (this.busy) {
			if (!this.creating && matchesKey(data, "escape")) this.finish(undefined);
			return;
		}
		if (this.creating) {
			if (matchesKey(data, "escape")) { this.creating = false; this.status = "↑↓ choose · Enter select · Ctrl+N Write your own"; }
			else this.titleInput.handleInput(data);
		} else if (matchesKey(data, "escape")) {
			this.finish(undefined);
		} else if (matchesKey(data, "alt+m")) {
			this.expanded = true;
		} else if (matchesKey(data, "tab")) {
			const selected = this.choices()[this.selected];
			if (selected) this.choose(selected);
		} else if (matchesKey(data, "ctrl+n")) {
			this.editTitle(this.input.getValue().trim());
		} else if (matchesKey(data, "up") || matchesKey(data, "down")) {
			this.moved = true;
			this.selected = Math.max(0, Math.min(this.choices().length - 1, this.selected + (matchesKey(data, "down") ? 1 : -1)));
		} else {
			this.input.handleInput(data);
			this.selected = Math.min(this.selected, Math.max(0, this.choices().length - 1));
		}
		this.tui.requestRender();
	}

	dispose(): void { this.closed = true; }

	invalidate(): void { this.input.invalidate(); this.titleInput.invalidate(); }

	render(width: number): string[] {
		const inside = Math.max(1, width - 2);
		const row = (text: string, styled = false) => {
			const clipped = truncateToWidth(styled ? text : text.replace(/\p{Cc}/gu, ""), inside, "…", true);
			return this.theme.fg("border", "│") + clipped + " ".repeat(Math.max(0, inside - visibleWidth(clipped))) + this.theme.fg("border", "│");
		};
		const input = this.creating ? this.titleInput : this.input;
		input.focused = this.focused;
		const inputLine = input.render(inside)[0] ?? "";
		const inputRow = this.theme.fg("border", "│") + inputLine + " ".repeat(Math.max(0, inside - visibleWidth(inputLine))) + this.theme.fg("border", "│");
		const reasonRows = (reason: string) => wrapTextWithAnsi(this.theme.fg("muted", `  ↳ ${reason.replace(/\p{Cc}/gu, "")}`), inside).map((line) => row(line, true));
		if (this.creating) return [
			this.theme.fg("border", `╭${"─".repeat(inside)}╮`),
			row("New stream · Edit title"), inputRow,
			...(this.creatingReason ? reasonRows(this.creatingReason) : []),
			row(this.busy ? "Creating stream…" : this.status),
			row(this.busy ? "Please wait for creation to finish" : "Enter Create · Esc Back"),
			this.theme.fg("border", `╰${"─".repeat(inside)}╯`),
		];
		const choices = this.choices();
		const header = this.options.reason ? wrapTextWithAnsi(this.theme.fg("muted", this.options.reason.replace(/\p{Cc}/gu, "")), inside).map((line) => row(line, true)) : [];
		const maxHeight = Math.min(36, Math.floor(this.tui.terminal.rows * 0.8));
		const available = Math.max(1, maxHeight - 7 - header.length);
		const renderChoices = (first: number) => {
			const lines: string[] = [];
			let used = 0;
			let selectedVisible = false;
			for (let index = first; index < choices.length; index++) {
				const item = choices[index];
				if (item.kind === "manual") break;
				const reason = item.kind === "new" ? item.reason : this.recommendations.existing.find((suggestion) => suggestion.slug === item.stream.slug)?.reason;
				const explanation = reason ? reasonRows(reason) : [];
				const choiceRows = item.kind === "new"
					? wrapTextWithAnsi(`${index === this.selected ? "▶" : " "} ${this.theme.fg("accent", `★ Suggested new: ${item.title}`)} ${this.theme.fg("muted", "· Enter edit")}`, Math.max(1, inside - 2)).map((text, line) => row(line ? `  ${text}` : text, true))
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
		if (this.selected < choices.length - 1 && !rendered.selectedVisible) rendered = renderChoices(this.selected);
		return [
			this.theme.fg("border", `╭${"─".repeat(inside)}╮`),
			row("Select or create a stream"),
			...header,
			...(rendered.lines.length ? rendered.lines : [row("No matching streams")]),
			row(`${this.selected === choices.length - 1 ? "▶" : " "} ${this.theme.fg("accent", "Write your own…")}`, true),
			inputRow,
			row(this.busy ? "Working…" : this.options.finding ? "Finding recommendations…" : this.status),
			row(`Tab select highlighted · Alt+M Show more · Esc Cancel${this.options.minimize && this.options.shortcut ? ` · ${shortcutLabel(this.options.shortcut)} Hide` : ""}`),
			this.theme.fg("border", `╰${"─".repeat(inside)}╯`),
		];
	}
}
