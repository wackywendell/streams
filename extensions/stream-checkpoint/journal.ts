import { randomUUID } from "node:crypto";
import { Box, Loader, Text, TuiMainScreen } from "@earendil-works/pi-tui";
import type { TUI } from "@earendil-works/pi-tui";
import type { EntryRenderer, ExtensionAPI, SessionEntry } from "@earendil-works/pi-coding-agent";
import { shortcutLabel } from "./view.ts";
import type { Author } from "./review.ts";

export const MARKER_TYPE = "checkpoint-start";
const STATUS_TYPE = "checkpoint-status";
type Source = { id: string; ownerId: string; leafId: string | null; omitted: number; timestamp: string; instructions: string; author?: Author };
type Status = { label: string; working?: boolean; slug?: string; detail?: string; closed?: boolean };

/** Append durable lifecycle events, but render their latest state at the source marker. */
export class CheckpointJournal {
	private states = new Map<string, Status>();
	private activeId?: string;
	private tui?: TUI;
	private spinner?: Loader;

	constructor(private pi: Pick<ExtensionAPI, "appendEntry">, private shortcut = "alt+p") {}

	attach(tui: TUI): void {
		if (this.tui !== tui) this.dispose();
		this.tui = tui;
		this.syncSpinner();
	}

	dispose(): void { this.spinner?.stop(); this.spinner = undefined; this.tui = undefined; }

	private syncSpinner(): void {
		if (this.activeId && this.states.get(this.activeId)?.working && this.tui) {
			this.spinner ??= new Loader(this.tui, (text) => text, (text) => text, "");
		} else {
			this.spinner?.stop();
			this.spinner = undefined;
		}
	}

	restore(entries: SessionEntry[]): void {
		this.dispose();
		this.states.clear();
		this.activeId = undefined;
		for (const entry of entries) {
			if (entry.type !== "custom" || entry.customType !== STATUS_TYPE) continue;
			const { id, ...status } = entry.data as Status & { id: string };
			this.states.set(id, status.working ? { ...status, label: "Interrupted · /checkpoint to resume", working: false } : status);
		}
	}

	begin(ownerId: string, snapshot: { leafId: string | null; omitted: number }, instructions = "", author?: Author): string {
		const id = randomUUID();
		this.activeId = id;
		this.states.set(id, { label: "Starting", working: true });
		this.pi.appendEntry<Source>(MARKER_TYPE, { id, ownerId, leafId: snapshot.leafId, omitted: snapshot.omitted, timestamp: new Date().toISOString(), instructions, ...(author ? { author } : {}) });
		this.syncSpinner();
		return id;
	}

	update(id: string, status: Status): void {
		if (JSON.stringify(this.states.get(id)) === JSON.stringify(status)) return;
		this.states.set(id, status);
		this.pi.appendEntry(STATUS_TYPE, { id, ...status });
		this.syncSpinner();
	}

	render: EntryRenderer<Source> = (entry, { expanded }, theme) => {
		const source = entry.data;
		if (!source) return;
		let indicator = "⠋";
		let indicatorLine: string | undefined;
		return {
			invalidate() {},
			render: (width) => {
				const status = this.states.get(source.id) ?? { label: "Interrupted · /checkpoint to resume" };
				if (status.working && source.id === this.activeId) {
					let visible = true;
					if (indicatorLine && this.tui instanceof TuiMainScreen) {
						const previous = this.tui.captureRenderState();
						const row = previous.previousLines.findIndex((line) => line.includes(indicatorLine!));
						// Changing a line in regular-mode scrollback clears and redraws the entire transcript.
						visible = previous.previousWidth !== width || (row >= previous.previousViewportTop && row < previous.previousViewportTop + previous.previousHeight);
					}
					if (visible) indicator = this.spinner?.render(4)[1]?.trim() || "⠋";
				}
				const box = new Box(1, 1, (text) => theme.bg("customMessageBg", text));
				const color = status.label === "Needs attention" ? "warning" : status.label.endsWith("ready") ? "success" : "accent";
				const action = this.activeId === source.id && !status.closed ? ` · ${shortcutLabel(this.shortcut)} Open` : "";
				box.addChild(new Text(`${theme.fg("accent", "Checkpoint")} · ${theme.fg(color, status.working ? `${indicator} ${status.label}` : status.label)}${action}`, 0, 0));
				if (expanded) {
					const lines = [status.slug, `Snapshot: ${source.timestamp}`, `Session: ${source.ownerId}`, `Source tree entry: ${source.leafId ?? "root"}`];
					if (source.omitted) lines.push(`${source.omitted} incomplete message(s) omitted`);
					if (source.author) lines.push("Started by the main agent");
					if (source.instructions) lines.push(`Instructions: ${source.instructions}`);
					if (status.detail) lines.push(status.detail);
					box.addChild(new Text(lines.filter(Boolean).join("\n"), 0, 0));
				}
				const lines = box.render(width);
				indicatorLine = status.working ? lines.find((line) => line.includes(indicator)) : undefined;
				return lines;
			},
		};
	};
}
