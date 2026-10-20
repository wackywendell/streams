import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { parseConfig } from "../../integrations/herdr-work-cards/src/config.ts";
import { DynamicBorder, getSettingsListTheme, getSelectListTheme } from "@earendil-works/pi-coding-agent";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { Container, Text, Input, SettingsList, SelectList, fuzzyFilter, matchesKey } from "@earendil-works/pi-tui";
import type { SettingItem } from "@earendil-works/pi-tui";
export type ReportingSettings = {reporting: "automatic" | "manual"; enrollment: "automatic" | "manual"; frequency: number; context: "trimmed" | "full"; model?: {provider: string; id: string}};
export const defaults: ReportingSettings = {reporting: "automatic", enrollment: "automatic", frequency: 1, context: "trimmed"};
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
 for (const key of ["reporting", "enrollment", "frequency", "context", "model"] as const) {
  if (value[key] === undefined) delete saved[key];else saved[key] = value[key];
 }
 writeFileSync(temp, JSON.stringify(saved, null, 2) + "\n", {mode: 0o600});renameSync(temp, path);
}
export async function settingsDialog(ctx: ExtensionContext, settings: ReportingSettings, save: (value: ReportingSettings) => void, agentDir?: string): Promise<void> {
 const value = structuredClone(settings);
 const nativePath = agentDir ? join(agentDir, "work-cards.json") : undefined;
 const native: Record<string, unknown> = nativePath ? {...parseConfig(storedSettings(nativePath))} : {};
 const models = (ctx.modelRegistry?.getAvailable?.() ?? ctx.modelRegistry?.getAll?.() ?? []).map((model) => ({value: `${model.provider}/${model.id}`, label: `${model.provider}/${model.id}`})).sort((a, b) => a.value.localeCompare(b.value));
 const modelRows = [{value: "current Pi model", label: "current Pi model"}, ...models];
 const items: SettingItem[] = [
  {id: "reporting", label: "Reporting", currentValue: value.reporting, description: "Automatically summarize your progress into the work card after agent responses, or only when you run /streams report.", values: ["automatic", "manual"]},
  {id: "enrollment", label: "Enrollment", currentValue: value.enrollment, description: "Automatically attach new Pi chats to a work card once meaningful work starts, or wait until you run /streams attach.", values: ["automatic", "manual"]},
  {id: "frequency", label: "Response frequency", currentValue: String(value.frequency), description: "How many completed agent responses to wait between automatic work-card updates.", values: ["1", "2", "3", "5", "10"]},
  {id: "context", label: "Context", currentValue: value.context, description: "What conversation history the summary model reads: 'trimmed' uses dialogue and compaction summaries; 'full' also includes tool calls and outputs.", values: ["trimmed", "full"]},
  {id: "model", label: "Model", currentValue: value.model ? `${value.model.provider}/${value.model.id}` : "current Pi model", description: "Model used to summarize progress and generate short titles. Press Enter to search and pick from available models."},
  ...(nativePath ? [
   {id: "workspace_startup", label: "Herdr workspace startup", currentValue: String(native.workspace_startup), description: "Automatically open the side-by-side Herdr work-card pane when a chat is attached, or only when you run /streams pane.", values: ["automatic", "manual"]},
   ...Object.entries({
    workspace_naming: ["Herdr workspace naming", "Sync the Herdr workspace name with the effort's short title (manual renames in Herdr stay protected)."],
    tab_naming: ["Herdr tab naming", "Sync this Herdr tab's name with this session's short title (manual renames stay protected)."],
    session_naming: ["Pi session naming", "Sync the Pi session name (/name) with this session's short title."],
   }).map(([id, [label, description]]) => ({id, description, label, currentValue: native[id] === false ? "disabled" : "enabled", values: ["enabled", "disabled"]})),
  ] : []),
 ];
 function change(id: string, next: string): void {
  if (id === "workspace_startup" || id.endsWith("_naming")) {
   if (!nativePath || !agentDir) return;
   const updated = {...storedSettings(nativePath), [id]: id === "workspace_startup" ? next : next === "enabled"};
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
   else (updated as any)[id] = next;
   validateSettings(updated);save(updated);
   delete value.model;Object.assign(value, updated);
  }
  items.find((item) => item.id === id)!.currentValue = next;
 }
 const savedValues = new Map(items.map((item) => [item.id, item.currentValue]));
 if (!ctx.ui.custom) {
  for (;;) {
   const selected = await ctx.ui.select("Work card & Herdr settings", [...items.map((item) => `${item.label}: ${item.currentValue}`), "Close"]);
   if (!selected || selected === "Close" || selected === "Cancel") return;
   const item = items.find((item) => selected.startsWith(item.label + ":"));if (!item) continue;
   const next = item.id === "model" ? await ctx.ui.select("Reporting model", modelRows.map((row) => row.value)) : item.values![(item.values!.indexOf(item.currentValue) + 1) % item.values!.length];
   if (next !== undefined) try {change(item.id, next);} catch(error) {ctx.ui.notify(String(error), "error");}
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
  const list = new SettingsList(items, Math.max(1, Math.min(items.length, tui.terminal.rows - 10)), getSettingsListTheme(), (id, next) => {
   const previous = savedValues.get(id)!;
   try {change(id, next);savedValues.set(id, next);list.updateValue(id, next);} catch(error) {list.updateValue(id, previous);ctx.ui.notify(String(error), "error");}
   tui.requestRender();
  }, () => done());
  const container = new Container();
  container.addChild(new DynamicBorder((text) => theme.fg("border", text)));
  container.addChild(new Text(theme.fg("accent", theme.bold("Work card & Herdr settings")), 1, 0));
  container.addChild(list);
  container.addChild(new DynamicBorder((text) => theme.fg("border", text)));
  return {render: (width: number) => container.render(width), invalidate: () => container.invalidate(), handleInput(data: string) {list.handleInput(data);tui.requestRender();}};
 });
}
