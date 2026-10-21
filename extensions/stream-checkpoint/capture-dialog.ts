import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { getSelectListTheme } from "@earendil-works/pi-coding-agent";
import { SelectList, TruncatedText, type SelectItem } from "@earendil-works/pi-tui";
import type { CaptureInventory, InventorySample } from "./capture.ts";

const actions = [
 {value: "capture" as const, label: "Capture private samples"},
 {value: "review" as const, label: "Review conversations and storage"},
 {value: "cancel" as const, label: "Cancel"},
];
export async function confirmCapture(ctx: ExtensionContext, inventory: CaptureInventory, root: string, signal: AbortSignal): Promise<boolean> {
 const samples = inventory.samples ?? [];
 const locallyAvailable = (sample: InventorySample) => !sample.unavailable || sample.host_id === inventory.host_id && sample.native_session_id === ctx.sessionManager.getSessionId();
 const available = samples.filter(locallyAvailable).length;
 const summary = [
  `${samples.length} Pi conversations; ${available} available`,
  `${samples.filter(sample => sample.open === true).length} observed open; ${inventory.unsupported?.length ?? 0} unsupported; ${inventory.unresolved?.length ?? 0} unresolved`,
  "Copies transcripts, cards and helper logs locally.",
  ...(inventory.topology_error || inventory.unresolved?.length ? ["Capture warning: review incomplete visibility before capturing."] : []),
 ];
 async function choose(title: string, body: string[], items: SelectItem[]): Promise<string | undefined> {
  signal.throwIfAborted();
  if (ctx.mode !== "tui" || !ctx.ui.custom) {
   const selected = await ctx.ui.select([title, ...body].join("\n"), items.map(item => item.label), {signal});
   return items.find(item => item.label === selected)?.value;
  }
  let abort: (() => void) | undefined;
  try {
   return await ctx.ui.custom<string | undefined>((tui, theme, _keys, done) => {
    abort = () => done(undefined);signal.addEventListener("abort", abort, {once: true});
    if (signal.aborted) abort();
    let list: SelectList, height = 0;
    const rebuild = () => {
     const next = Math.max(1, Math.min(10, items.length, tui.terminal.rows - body.length - 7));
     if (next === height) return;
     const selected = list?.getSelectedItem()?.value;
     height = next;list = new SelectList(items, height, getSelectListTheme());
     list.setSelectedIndex(Math.max(0, items.findIndex(item => item.value === selected)));
     list.onSelect = item => done(item.value);list.onCancel = () => done(undefined);
    };
    rebuild();
    return {
     render(width: number) {
      rebuild();
      const lines = [new TruncatedText(theme.fg("accent", theme.bold(title)), 0, 0),
       ...body.map(line => new TruncatedText(line.startsWith("Capture warning:") ? theme.fg("warning", line) : line, 0, 0))];
      return [...lines.flatMap(line => line.render(width)), ...list.render(width),
       ...new TruncatedText(theme.fg("dim", "↑↓ choose · Enter select · Esc cancel"), 0, 0).render(width)];
     },
     invalidate() {list.invalidate();},
     handleInput(data: string) {rebuild();list.handleInput(data);tui.requestRender();},
    };
   });
  } finally {if (abort) signal.removeEventListener("abort", abort);}
 }
 for (;;) {
  const choice = await choose("Capture private samples?", summary, actions);
  signal.throwIfAborted();
  if (choice !== "review") return choice === "capture";
  const rows = [
   `Storage: ${root}`,
   "Copies transcripts, cards and available helper logs; no model calls or live card changes.",
   ...(inventory.topology_error ? [`Capture warning: ${inventory.topology_error}`] : []),
   ...(inventory.unresolved ?? []).map(observation => `Unresolved Pi · ${observation.workspace_id}/${observation.pane_id} · ${observation.error}`),
   ...samples.map(sample => `${sample.open === null ? "open unknown" : sample.open ? "open" : "closed"} · ${locallyAvailable(sample) ? "available" : "unavailable"} · ${sample.catalog.session?.title ?? "no card"} · ${sample.native_session_id}${sample.unavailable ? locallyAvailable(sample) ? " · using current Pi session" : ` · ${sample.unavailable}` : ""}`),
  ];
  await choose("Capture inventory — Enter/Esc returns", [], rows.map((label, index) => ({value: String(index), label})));
 }
}
