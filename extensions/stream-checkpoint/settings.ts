import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
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
export async function settingsDialog(ctx: ExtensionContext, settings: ReportingSettings, save: (value: ReportingSettings) => void): Promise<void> {
 const value = structuredClone(settings);
 for (;;) {
  const action = await ctx.ui.select("Work card reporting settings", [`Reporting: ${value.reporting}`, `Enrollment: ${value.enrollment}`, `Response frequency: ${value.frequency}`, `Context: ${value.context}`, `Model: ${value.model ? `${value.model.provider}/${value.model.id}` : "current Pi model"}`, "Save", "Cancel"]);
  if (!action || action === "Cancel") return;
  if (action === "Save") {try {validateSettings(value);save(value);return;} catch(error) {ctx.ui.notify(String(error), "error");} continue;}
  if (action.startsWith("Reporting:")) value.reporting = value.reporting === "automatic" ? "manual" : "automatic";
  else if (action.startsWith("Enrollment:")) value.enrollment = value.enrollment === "automatic" ? "manual" : "automatic";
  else if (action.startsWith("Context:")) value.context = value.context === "trimmed" ? "full" : "trimmed";
  else if (action.startsWith("Response frequency:")) {
   const input = await ctx.ui.input("Completed responses between automatic reports", String(value.frequency));
   if (input !== undefined) {const frequency = Number(input);if (!Number.isInteger(frequency) || frequency < 1) ctx.ui.notify("Enter a positive integer", "error");else value.frequency = frequency;}
  } else {
   const mode = await ctx.ui.select("Reporting model", ["Current Pi model", "Provider/model ID"]);
   if (mode === "Current Pi model") delete value.model;
   else if (mode) {const provider = await ctx.ui.input("Provider", value.model?.provider);if (!provider?.trim()) continue;const id = await ctx.ui.input("Model ID", value.model?.id);if (id?.trim()) value.model = {provider, id};}
  }
 }
}
