import { Input, matchesKey, truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { Component, TUI } from "@earendil-works/pi-tui";
import type { Theme } from "@earendil-works/pi-coding-agent";
import type { Recommendations } from "./core.ts";
import type { StreamCli } from "./streams.ts";

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

	invalidate(): void { this.input.invalidate(); }

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
			? wrapTextWithAnsi(`Create ${this.preview.title} (${this.preview.slug}) in ${String(this.preview.cwd)}? y/n`, inside).map((line) => row(line))
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
