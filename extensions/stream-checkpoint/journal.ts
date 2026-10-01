import { randomUUID } from "node:crypto";
import { Box, Text } from "@earendil-works/pi-tui";
import type { EntryRenderer, ExtensionAPI, SessionEntry } from "@earendil-works/pi-coding-agent";
import { shortcutLabel } from "./view.ts";

export const MARKER_TYPE = "checkpoint-start";
const STATUS_TYPE = "checkpoint-status";
type Source = { id: string; ownerId: string; leafId: string | null; omitted: number; timestamp: string; instructions: string };
type Status = { label: string; working?: boolean; slug?: string; detail?: string; closed?: boolean };

/** Append durable lifecycle events, but render their latest state at the source marker. */
export class CheckpointJournal {
	private states = new Map<string, Status>();
	private activeId?: string;

	constructor(private pi: Pick<ExtensionAPI, "appendEntry">, private shortcut = "alt+p") {}

	restore(entries: SessionEntry[]): void {
		this.states.clear();
		this.activeId = undefined;
		for (const entry of entries) {
			if (entry.type !== "custom" || entry.customType !== STATUS_TYPE) continue;
			const { id, ...status } = entry.data as Status & { id: string };
			this.states.set(id, status.working ? { ...status, label: "Interrupted · /checkpoint to resume", working: false } : status);
		}
	}

	begin(ownerId: string, snapshot: { leafId: string | null; omitted: number }, instructions = ""): string {
		const id = randomUUID();
		this.activeId = id;
		this.states.set(id, { label: "Starting", working: true });
		this.pi.appendEntry<Source>(MARKER_TYPE, { id, ownerId, leafId: snapshot.leafId, omitted: snapshot.omitted, timestamp: new Date().toISOString(), instructions });
		return id;
	}

	update(id: string, status: Status): void {
		if (JSON.stringify(this.states.get(id)) === JSON.stringify(status)) return;
		this.states.set(id, status);
		this.pi.appendEntry(STATUS_TYPE, { id, ...status });
	}

	render: EntryRenderer<Source> = (entry, { expanded }, theme) => {
		const source = entry.data;
		if (!source) return;
		return {
			invalidate() {},
			render: (width) => {
				const status = this.states.get(source.id) ?? { label: "Interrupted · /checkpoint to resume" };
				const box = new Box(1, 1, (text) => theme.bg("customMessageBg", text));
				const color = status.label === "Needs attention" ? "warning" : status.label.endsWith("ready") ? "success" : "accent";
				const action = this.activeId === source.id && !status.closed ? ` · ${shortcutLabel(this.shortcut)} Open` : "";
				box.addChild(new Text(`${theme.fg("accent", "Checkpoint")} · ${theme.fg(color, status.label)}${action}`, 0, 0));
				if (expanded) {
					const lines = [status.slug, `Snapshot: ${source.timestamp}`, `Session: ${source.ownerId}`, `Source tree entry: ${source.leafId ?? "root"}`];
					if (source.omitted) lines.push(`${source.omitted} incomplete message(s) omitted`);
					if (source.instructions) lines.push(`Instructions: ${source.instructions}`);
					if (status.detail) lines.push(status.detail);
					box.addChild(new Text(lines.filter(Boolean).join("\n"), 0, 0));
				}
				return box.render(width);
			},
		};
	};
}
