import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { parseConfig } from "../../integrations/herdr-work-cards/src/config.ts";
import { DynamicBorder, getSettingsListTheme, getSelectListTheme } from "@earendil-works/pi-coding-agent";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { Container, Text, Input, SettingsList, SelectList, fuzzyFilter, matchesKey } from "@earendil-works/pi-tui";
import type { SettingItem } from "@earendil-works/pi-tui";
import type { LocalContext, Mode, StreamCli, CardMode, CardSettings } from "./streams.ts";
export type ReportingSettings = {reporting: "automatic" | "manual"; enrollment: "automatic" | "manual"; frequency: number; context: "trimmed" | "full"; model?: {provider: string; id: string}; session_logging: boolean; debug_commands: boolean; pi_name_edits: boolean};
export const defaults: ReportingSettings = {reporting: "automatic", enrollment: "automatic", frequency: 1, context: "trimmed", session_logging: false, debug_commands: false, pi_name_edits: true};
const plainObject = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);
function storedSettings(path: string): Record<string, unknown> {
 if (!existsSync(path)) return {};
 let value: unknown;
 try {value = JSON.parse(readFileSync(path, "utf8"));} catch(error) {throw Error(`Cannot read reporting settings ${path}: ${String(error)}. Fix the JSON before saving.`);}
 if (!plainObject(value)) throw Error(`Reporting settings ${path} must contain a JSON object`);
 return value;
}
export function validateSettings(value: ReportingSettings): void {
 if (!plainObject(value)) throw Error("Reporting settings must be a plain object");
 if (!["automatic", "manual"].includes(value.reporting) || !["automatic", "manual"].includes(value.enrollment) || !["trimmed", "full"].includes(value.context) || !Number.isInteger(value.frequency) || value.frequency < 1) throw Error("Reporting settings require automatic/manual, trimmed/full and a positive integer response frequency");
 if (typeof value.session_logging !== "boolean") throw Error("Helper session logging must be a boolean");
 if (typeof value.debug_commands !== "boolean") throw Error("Debug commands must be a boolean");
 if (typeof value.pi_name_edits !== "boolean") throw Error("Pi name edits must be a boolean");
 if (value.model !== undefined && (!plainObject(value.model) || typeof value.model.provider !== "string" || !value.model.provider.trim() || typeof value.model.id !== "string" || !value.model.id.trim())) throw Error("Reporting model must be an object with nonempty string provider and model ID");
}
export function loadSettings(agentDir: string): ReportingSettings {
 const path = join(agentDir, "stream-reporting.json");
 if (!existsSync(path)) return {...defaults};
 const value = {...defaults, ...storedSettings(path)};validateSettings(value);return value;
}
export function saveSettings(agentDir: string, value: ReportingSettings): void {
 validateSettings(value);mkdirSync(agentDir, {recursive: true});
 const path = join(agentDir, "stream-reporting.json"), temp = `${path}.${randomUUID()}.tmp`;
 const saved: Record<string, unknown> = {...value, ...storedSettings(path)};
 for (const key of ["reporting", "enrollment", "frequency", "context", "model", "session_logging", "debug_commands", "pi_name_edits"] as const) {
  if (value[key] === undefined) delete saved[key];else saved[key] = value[key];
 }
 writeFileSync(temp, JSON.stringify(saved, null, 2) + "\n", {mode: 0o600});renameSync(temp, path);
}
export async function settingsDialog(ctx: ExtensionContext, settings: ReportingSettings, save: (value: ReportingSettings) => void, naming: {defaults: LocalContext["title_defaults"]; save: StreamCli["setTitleDefault"]}, agentDir?: string, cards?: {settings: CardSettings; save: StreamCli["setCardMode"]}): Promise<void> {
 const value = structuredClone(settings);
 const nativePath = agentDir ? join(agentDir, "work-cards.json") : undefined;
 const native: Record<string, unknown> = nativePath ? {...parseConfig(storedSettings(nativePath))} : {};
 const models = (ctx.modelRegistry?.getAvailable?.() ?? ctx.modelRegistry?.getAll?.() ?? []).map((model) => ({value: `${model.provider}/${model.id}`, label: `${model.provider}/${model.id}`})).sort((a, b) => a.value.localeCompare(b.value));
 const modelRows = [{value: "current Pi model", label: "current Pi model"}, ...models];
 const debugItems: SettingItem[] = [
  {id: "debug_commands", label: "Debug commands", currentValue: value.debug_commands ? "enabled" : "disabled", description: "Enable frozen capture and replay commands for debugging. Does not enable helper session logging.", values: ["enabled", "disabled"]},
  {id: "session_logging", label: "Helper session logging", currentValue: value.session_logging ? "enabled" : "disabled", description: "Save complete helper inputs, responses, retries and errors as Pi JSONL sessions in stream-reporting-sessions under the Pi agent directory. Copies conversation content; logs can be large. Local logging only, not another agent or capture extensions.", values: ["enabled", "disabled"]},
 ];
 const namingItems: SettingItem[] = [
  ...(["stream", "session"] as const).map(target => ({
   id: `${target}_title_default`, label: `${target === "stream" ? "Stream" : "Session"} title generation`, currentValue: naming.defaults[target], values: ["automatic", "manual"],
   description: `Default for ${target === "stream" ? "Stream" : "Session"} titles in inherit mode. Automatic allows future reports to refine titles; manual protects them. Does not change explicit record modes or frontend synchronization. Enrollment may still create an initial title.`,
  })),
  {id: "pi_name_edits", label: "Pi name edits", currentValue: value.pi_name_edits ? "enabled" : "disabled", description: "Save changed Pi /name edits as manual Session titles. Independent of outgoing Pi synchronization. Unbound chats stay native-only; equal names do not pin.", values: ["enabled", "disabled"]},
  ...(nativePath ? Object.entries({
   workspace_naming: ["Herdr workspace naming", "Mirror the saved effort title in Herdr. While enabled, replaces direct workspace renames; disable for an independent frontend name."],
   tab_naming: ["Herdr tab naming", "Mirror saved Session titles in agent tabs and document identities in editor tabs. While enabled, replaces direct tab renames."],
   session_naming: ["Pi session naming", "Mirror the saved Session title in Pi. Turning this off stops Streams from changing Pi's displayed name, but does not disable incoming Pi name edits."],
  }).map(([id, [label, description]]) => ({id, description, label, currentValue: native[id] === false ? "disabled" : "enabled", values: ["enabled", "disabled"]})) : []),
 ];
 const items: SettingItem[] = [
  {id: "reporting", label: "Reporting", currentValue: value.reporting, description: "Automatically summarize your progress into the work card after agent responses, or only when you run /streams report.", values: ["automatic", "manual"]},
  {id: "enrollment", label: "Enrollment", currentValue: value.enrollment, description: "Automatically attach new Pi chats to a work card once meaningful work starts, or wait until you run /streams attach.", values: ["automatic", "manual"]},
  {id: "frequency", label: "Response frequency", currentValue: String(value.frequency), description: "How many completed agent responses to wait between automatic work-card updates.", values: ["1", "2", "3", "5", "10"]},
  {id: "context", label: "Context", currentValue: value.context, description: "What conversation history the summary model reads: 'trimmed' uses dialogue and compaction summaries; 'full' also includes tool calls and outputs.", values: ["trimmed", "full"]},
  {id: "model", label: "Model", currentValue: value.model ? `${value.model.provider}/${value.model.id}` : "current Pi model", description: "Model used to summarize progress and generate short titles. Press Enter to search and pick from available models."},
  {id: "debug", label: "Debug", currentValue: "settings", description: "Configure debug commands and optional helper session logging. Press Enter to open."},
  ...(cards ? [
   {id: "cards_global", label: "Default card opening", currentValue: cards.settings.global, description: "Open cards automatically at Pi startup or attachment, or only by explicit command. Stored by Streams, not Pi.", values: ["automatic", "manual"]},
   {id: "cards_session", label: "This conversation's card opening", currentValue: cards.settings.session, description: "Inherit the Streams default or override it for this conversation. Opening a pane saves automatic; closing it with q or the Pi close command saves manual.", values: ["inherit", "automatic", "manual"]},
  ] : []),
  {id: "naming", label: "Naming", currentValue: "settings", description: "Configure title generation, incoming Pi name edits and outgoing frontend synchronization independently. Press Enter to open."},
 ];
 const allItems = [...items, ...debugItems, ...namingItems];
 function change(id: string, next: string): void | Promise<void> {
  if ((id === "cards_global" || id === "cards_session") && cards) {
   return cards.save(id === "cards_global" ? "global" : "session", next as CardMode).then(settings => {
    for (const target of ["global", "session"] as const) {
     const item = items.find(item => item.id === `cards_${target}`)!;
     item.currentValue = settings[target];savedValues.set(item.id, item.currentValue);
    }
   });
  }
  if (id === "stream_title_default" || id === "session_title_default") {
   return naming.save(id === "stream_title_default" ? "stream" : "session", next as Mode).then(defaults => {
    for (const target of ["stream", "session"] as const) {
     const item = namingItems.find(item => item.id === `${target}_title_default`)!;
     item.currentValue = defaults[target];savedValues.set(item.id, item.currentValue);
    }
   });
  }
  if (id.endsWith("_naming")) {
   if (!nativePath || !agentDir) return;
   const updated = {...storedSettings(nativePath), [id]: next === "enabled"};
   parseConfig(updated);
   mkdirSync(agentDir, {recursive: true});
   const temp = `${nativePath}.${randomUUID()}.tmp`;
   writeFileSync(temp, JSON.stringify(updated, null, 2) + "\n", {mode: 0o600});renameSync(temp, nativePath);
  } else {
   const updated = structuredClone(value);
   if (id === "model") {
    const model = models.find((row) => row.value === next);
    if (model) {const slash = next.indexOf("/");updated.model = {provider: next.slice(0, slash), id: next.slice(slash + 1)};}
    else delete updated.model;
   } else if (id === "frequency") updated.frequency = Number(next);
   else if (id === "session_logging" || id === "debug_commands" || id === "pi_name_edits") updated[id] = next === "enabled";
   else (updated as any)[id] = next;
   validateSettings(updated);save(updated);
   delete value.model;Object.assign(value, updated);
  }
  allItems.find((item) => item.id === id)!.currentValue = next;
 }
 const savedValues = new Map(allItems.map((item) => [item.id, item.currentValue]));
 if (!ctx.ui.custom) {
  let section: "Debug" | "Naming" | undefined;
  for (;;) {
   const rows = section === "Debug" ? debugItems : section === "Naming" ? namingItems : items;
   const selected = await ctx.ui.select(section ?? "Work card & Herdr settings", [...rows.map((item) => ["debug", "naming"].includes(item.id) ? item.label : `${item.label}: ${item.currentValue}`), section ? "Back" : "Close"]);
   if (!selected || selected === "Back" || selected === "Close" || selected === "Cancel") {
    if (section) {section = undefined;continue;}
    return;
   }
   if (selected === "Debug" || selected === "Naming") {section = selected;continue;}
   const item = rows.find((item) => selected.startsWith(item.label + ":"));if (!item) continue;
   const next = item.id === "model" ? await ctx.ui.select("Reporting model", modelRows.map((row) => row.value)) : item.values![(item.values!.indexOf(item.currentValue) + 1) % item.values!.length];
   if (next !== undefined) try {await change(item.id, next);} catch(error) {ctx.ui.notify(String(error), "error");}
  }
 }
 await ctx.ui.custom<void>((tui, theme, _keys, done) => {
  items.find((item) => item.id === "model")!.submenu = (current, close) => {
   const input = new Input();
   input.focused = true;
   const rebuild = () => {
    const list = new SelectList(fuzzyFilter(modelRows, input.getValue(), (row) => row.label), Math.max(1, Math.min(10, tui.terminal.rows - 9)), getSelectListTheme());
    list.onSelect = (item) => close(item.value);list.onCancel = () => close();
    return list;
   };
   let list = rebuild();list.setSelectedIndex(Math.max(0, modelRows.findIndex((row) => row.value === current)));
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
    const current = allItems.find(item => item.id === id)!.currentValue;
    savedValues.set(id, current);list.updateValue(id, current);
   };
   try {
    const pending = change(id, next);
    if (pending) {
     saving = true;list.updateValue(id, "saving…");
     void pending.then(saved, failed).finally(() => {saving = false;tui.requestRender();});
    } else saved();
   } catch(error) {failed(error);}
   tui.requestRender();
  }
  for (const [id, label, rows] of [["debug", "Debug", debugItems], ["naming", "Naming", namingItems]] as const) {
   items.find(item => item.id === id)!.submenu = (_current, close) => {
    const sectionList = new SettingsList(rows, Math.max(1, Math.min(rows.length, tui.terminal.rows - 11)), getSettingsListTheme(), (id, next) => update(sectionList, id, next), () => close());
    const section = new Container();
    section.addChild(new Text(theme.fg("accent", theme.bold(label)), 1, 0));
    section.addChild(sectionList);
    return {render: (width: number) => section.render(width), invalidate: () => section.invalidate(), handleInput(data: string) {sectionList.handleInput(data);tui.requestRender();}};
   };
  }
  const list = new SettingsList(items, Math.max(1, Math.min(items.length, tui.terminal.rows - 10)), getSettingsListTheme(), (id, next) => update(list, id, next), () => done());
  const container = new Container();
  container.addChild(new DynamicBorder((text) => theme.fg("border", text)));
  container.addChild(new Text(theme.fg("accent", theme.bold("Work card & Herdr settings")), 1, 0));
  container.addChild(list);
  container.addChild(new DynamicBorder((text) => theme.fg("border", text)));
  return {render: (width: number) => container.render(width), invalidate: () => container.invalidate(), handleInput(data: string) {if (!saving) list.handleInput(data);tui.requestRender();}};
 });
}
