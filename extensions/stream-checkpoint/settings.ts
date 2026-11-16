import { DynamicBorder, getSettingsListTheme, getSelectListTheme } from "@earendil-works/pi-coding-agent";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { Container, Text, Input, SettingsList, SelectList, fuzzyFilter, matchesKey } from "@earendil-works/pi-tui";
import type { SettingItem } from "@earendil-works/pi-tui";
import type { ConfigSnapshot } from "./streams.ts";

export type ReportingSettings = {
 reporting: "automatic" | "manual";
 enrollment: "automatic" | "manual";
 frequency: number;
 context: "trimmed" | "full";
 model?: {provider: string; id: string};
 session_logging: boolean;
 debug_commands: boolean;
 pi_name_edits: boolean;
};

export const defaults: ReportingSettings = {
 reporting: "automatic",
 enrollment: "automatic",
 frequency: 1,
 context: "trimmed",
 session_logging: false,
 debug_commands: false,
 pi_name_edits: true,
};

export function defaultConfigSnapshot(): ConfigSnapshot {
 return {
  naming: {stream: "automatic", session: "automatic"},
  herdr: {
   viewer: {mode: "editor", placement: "pane-right"},
   naming: {tab: true, workspace: true},
   cards: {auto_open: "automatic"},
   timeout_ms: 3000,
   pane_height_fraction: 0.2,
  },
  pi: {
   naming: {session: true, import_edits: true},
   reporting: {
    mode: "automatic",
    enrollment: "automatic",
    frequency: 1,
    context: "trimmed",
    model: null,
    session_logging: false,
    debug_commands: false,
   },
  },
 };
}

export function reportingSettingsFromSnapshot(snapshot: ConfigSnapshot): ReportingSettings {
 return {
  reporting: snapshot.pi.reporting.mode,
  enrollment: snapshot.pi.reporting.enrollment,
  frequency: snapshot.pi.reporting.frequency,
  context: snapshot.pi.reporting.context,
  ...(snapshot.pi.reporting.model ? {model: snapshot.pi.reporting.model} : {}),
  session_logging: snapshot.pi.reporting.session_logging,
  debug_commands: snapshot.pi.reporting.debug_commands,
  pi_name_edits: snapshot.pi.naming.import_edits,
 };
}

const plainObject = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);

export function validateSettings(value: ReportingSettings): void {
 if (!plainObject(value)) throw Error("Reporting settings must be a plain object");
 if (!["automatic", "manual"].includes(value.reporting) || !["automatic", "manual"].includes(value.enrollment) || !["trimmed", "full"].includes(value.context) || !Number.isInteger(value.frequency) || value.frequency < 1) throw Error("Reporting settings require automatic/manual, trimmed/full and a positive integer response frequency");
 if (typeof value.session_logging !== "boolean") throw Error("Helper session logging must be a boolean");
 if (typeof value.debug_commands !== "boolean") throw Error("Debug commands must be a boolean");
 if (typeof value.pi_name_edits !== "boolean") throw Error("Pi name edits must be a boolean");
 if (value.model !== undefined && (!plainObject(value.model) || typeof value.model.provider !== "string" || !value.model.provider.trim() || typeof value.model.id !== "string" || !value.model.id.trim())) throw Error("Reporting model must be an object with nonempty string provider and model ID");
}

export type ConfigActions = {
 setConfig: (key: string, value: string) => Promise<ConfigSnapshot>;
 unsetConfig: (key: string) => Promise<ConfigSnapshot>;
};

export async function settingsDialog(
 ctx: ExtensionContext,
 initial: ConfigSnapshot,
 actions: ConfigActions,
): Promise<void> {
 let current = structuredClone(initial);
 const models = (ctx.modelRegistry?.getAvailable?.() ?? ctx.modelRegistry?.getAll?.() ?? []).map((model) => ({value: `${model.provider}/${model.id}`, label: `${model.provider}/${model.id}`})).sort((a, b) => a.value.localeCompare(b.value));
 const modelRows = [{value: "unset", label: "unset (use current Pi model)"}, ...models];
 const valueFor = (id: string, cfg: ConfigSnapshot): string => {
  switch (id) {
   case "pi.reporting.mode": return cfg.pi.reporting.mode;
   case "pi.reporting.enrollment": return cfg.pi.reporting.enrollment;
   case "pi.reporting.frequency": return String(cfg.pi.reporting.frequency);
   case "pi.reporting.context": return cfg.pi.reporting.context;
   case "pi.reporting.model": return cfg.pi.reporting.model ? `${cfg.pi.reporting.model.provider}/${cfg.pi.reporting.model.id}` : "unset";
   case "pi.reporting.debug_commands": return String(cfg.pi.reporting.debug_commands);
   case "pi.reporting.session_logging": return String(cfg.pi.reporting.session_logging);
   case "naming.stream": return cfg.naming.stream;
   case "naming.session": return cfg.naming.session;
   case "pi.naming.import_edits": return String(cfg.pi.naming.import_edits);
   case "pi.naming.session": return String(cfg.pi.naming.session);
   default: return "";
  }
 };
 const debugItems: SettingItem[] = [
  {id: "pi.reporting.debug_commands", label: "pi.reporting.debug_commands", currentValue: valueFor("pi.reporting.debug_commands", current), description: "Enable frozen capture and replay commands for debugging. Does not enable helper session logging.", values: ["true", "false"]},
  {id: "pi.reporting.session_logging", label: "pi.reporting.session_logging", currentValue: valueFor("pi.reporting.session_logging", current), description: "Save complete helper inputs, responses, retries and errors as Pi JSONL sessions in stream-reporting-sessions under the Pi agent directory.", values: ["true", "false"]},
 ];
 const namingItems: SettingItem[] = [
  {id: "naming.stream", label: "naming.stream", currentValue: valueFor("naming.stream", current), description: "Default for Stream titles in inherit mode. Automatic allows future reports to refine titles; manual protects them.", values: ["automatic", "manual"]},
  {id: "naming.session", label: "naming.session", currentValue: valueFor("naming.session", current), description: "Default for Session titles in inherit mode. Automatic allows future reports to refine titles; manual protects them.", values: ["automatic", "manual"]},
  {id: "pi.naming.import_edits", label: "pi.naming.import_edits", currentValue: valueFor("pi.naming.import_edits", current), description: "Save changed Pi /name edits as manual Session titles. Independent of outgoing Pi synchronization.", values: ["true", "false"]},
  {id: "pi.naming.session", label: "pi.naming.session", currentValue: valueFor("pi.naming.session", current), description: "Mirror the saved Session title in Pi. Turning this off stops Streams from changing Pi's displayed name.", values: ["true", "false"]},
 ];
 const items: SettingItem[] = [
  {id: "pi.reporting.mode", label: "pi.reporting.mode", currentValue: valueFor("pi.reporting.mode", current), description: "Automatically summarize your progress into the work card after agent responses, or only when you run /streams report.", values: ["automatic", "manual"]},
  {id: "pi.reporting.enrollment", label: "pi.reporting.enrollment", currentValue: valueFor("pi.reporting.enrollment", current), description: "Automatically attach new Pi chats to a work card once meaningful work starts, or wait until you run /streams attach.", values: ["automatic", "manual"]},
  {id: "pi.reporting.frequency", label: "pi.reporting.frequency", currentValue: valueFor("pi.reporting.frequency", current), description: "How many completed agent responses to wait between automatic work-card updates.", values: ["1", "2", "3", "5", "10"]},
  {id: "pi.reporting.context", label: "pi.reporting.context", currentValue: valueFor("pi.reporting.context", current), description: "What conversation history the summary model reads: 'trimmed' uses dialogue and compaction summaries; 'full' also includes tool calls and outputs.", values: ["trimmed", "full"]},
  {id: "pi.reporting.model", label: "pi.reporting.model", currentValue: valueFor("pi.reporting.model", current), description: "Model used to summarize progress and generate short titles. Press Enter to search and pick from available models."},
  {id: "debug", label: "Debug", currentValue: "settings", description: "Configure debug commands and optional helper session logging. Press Enter to open."},
  {id: "naming", label: "Naming", currentValue: "settings", description: "Configure title generation, incoming Pi name edits and outgoing Pi synchronization. Press Enter to open."},
 ];
 const allItems = [...items, ...debugItems, ...namingItems];
 const savedValues = new Map(allItems.map((item) => [item.id, item.currentValue]));
 async function change(id: string, next: string): Promise<void> {
  const updated = id === "pi.reporting.model" && next === "unset"
   ? await actions.unsetConfig(id)
   : await actions.setConfig(id, next);
  current = updated;
  for (const item of allItems) {
   if (item.id === "debug" || item.id === "naming") continue;
   const nextVal = valueFor(item.id, current);
   item.currentValue = nextVal;
   savedValues.set(item.id, nextVal);
  }
 }
 if (!ctx.ui.custom) {
  let section: "Debug" | "Naming" | undefined;
  for (;;) {
   const rows = section === "Debug" ? debugItems : section === "Naming" ? namingItems : items;
   const selected = await ctx.ui.select(section ?? "Streams Pi & naming settings", [...rows.map((item) => ["debug", "naming"].includes(item.id) ? item.label : `${item.label}: ${item.currentValue}`), section ? "Back" : "Close"]);
   if (!selected || selected === "Back" || selected === "Close" || selected === "Cancel") {
    if (section) {section = undefined;continue;}
    return;
   }
   if (selected === "Debug" || selected === "Naming") {section = selected;continue;}
   const item = rows.find((item) => selected.startsWith(item.label + ":"));if (!item) continue;
   const next = item.id === "pi.reporting.model"
    ? await ctx.ui.select("Reporting model", modelRows.map((row) => row.value))
    : item.values![(item.values!.indexOf(item.currentValue) + 1) % item.values!.length];
   if (next !== undefined) try {await change(item.id, next);} catch(error) {ctx.ui.notify(String(error), "error");}
  }
 }
 await ctx.ui.custom<void>((tui, theme, _keys, done) => {
  items.find((item) => item.id === "pi.reporting.model")!.submenu = (currentModel, close) => {
   const input = new Input();
   input.focused = true;
   const rebuild = () => {
    const list = new SelectList(fuzzyFilter(modelRows, input.getValue(), (row) => row.label), Math.max(1, Math.min(10, tui.terminal.rows - 9)), getSelectListTheme());
    list.onSelect = (item) => close(item.value);list.onCancel = () => close();
    return list;
   };
   let list = rebuild();list.setSelectedIndex(Math.max(0, modelRows.findIndex((row) => row.value === currentModel)));
   return {
    get focused() {return input.focused;}, set focused(focus: boolean) {input.focused = focus;},
    render(width: number) {return [...input.render(width), ...list.render(width)];},
    invalidate() {input.invalidate();list.invalidate();},
    handleInput(data: string) {
     if ((["up", "down", "enter", "escape"] as const).some((key) => matchesKey(data, key))) list.handleInput(data);
     else {input.handleInput(data);list = rebuild();}
     tui.requestRender();
    },
   };
  };
  let saving = false;
  function update(list: SettingsList, id: string, next: string): void {
   const previous = savedValues.get(id)!;
   const failed = (error: unknown) => {list.updateValue(id, previous);ctx.ui.notify(String(error), "error");};
   const saved = () => {
    const val = allItems.find(item => item.id === id)!.currentValue;
    savedValues.set(id, val);list.updateValue(id, val);
   };
   saving = true;list.updateValue(id, "saving…");
   void change(id, next).then(saved, failed).finally(() => {saving = false;tui.requestRender();});
   tui.requestRender();
  }
  for (const [id, label, rows] of [["debug", "Debug", debugItems], ["naming", "Naming", namingItems]] as const) {
   items.find(item => item.id === id)!.submenu = (_current, close) => {
    const sectionList = new SettingsList(rows, Math.max(1, Math.min(rows.length, tui.terminal.rows - 11)), getSettingsListTheme(), (id, next) => update(sectionList, id, next), () => close());
    const section = new Container();
    section.addChild(new Text(theme.fg("accent", theme.bold(label)), 1, 0));
    section.addChild(sectionList);
    return {render: (width: number) => section.render(width), invalidate: () => section.invalidate(), handleInput(data: string) {if (!saving) sectionList.handleInput(data);tui.requestRender();}};
   };
  }
  const list = new SettingsList(items, Math.max(1, Math.min(items.length, tui.terminal.rows - 10)), getSettingsListTheme(), (id, next) => update(list, id, next), () => done());
  const container = new Container();
  container.addChild(new DynamicBorder((text) => theme.fg("border", text)));
  container.addChild(new Text(theme.fg("accent", theme.bold("Streams Pi & naming settings")), 1, 0));
  container.addChild(list);
  container.addChild(new DynamicBorder((text) => theme.fg("border", text)));
  return {render: (width: number) => container.render(width), invalidate: () => container.invalidate(), handleInput(data: string) {if (!saving) list.handleInput(data);tui.requestRender();}};
 });
}
