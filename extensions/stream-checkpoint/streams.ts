import { spawn } from "node:child_process";
import { resolveStreamsBin } from "../../integrations/herdr-work-cards/src/streams-bin.ts";

import { parseLocalContext, isStream as stream, isSession as session, type StreamWire, type SessionWire, type LocalContext } from "../../integrations/herdr-work-cards/src/streams-context.ts";
export type { Status, Mode, Link, Document, StreamWire, SessionWire, LocalContext } from "../../integrations/herdr-work-cards/src/streams-context.ts";
export type LocalStatus = "unchanged" | "persisted" | "committed" | "repair-required";
export type SharingStatus = "local-only" | "pending" | "confirmed" | "blocked" | "failed" | "uncertain";
export type Result = { local: LocalStatus; sharing: SharingStatus; domain: { outcome: "changes" | "no-change" | "superseded" | "conflict"; [key: string]: unknown } | null; detail?: string; [key: string]: unknown };
export type PublicationResult = Result & {domain: null};
export type Receipt = {simulated?: true; stream?: StreamWire; session?: SessionWire; authority?: LocalContext["authority"]; publication?: PublicationResult; completion_error?: unknown} & ({operation_id: string; save: Result; saves?: never} | {operation_id?: never; save?: never; saves: {operation_id: string; save: Result}[]});
export type ApplyRequest = {catalog_source: string; operation_id: string; kind: "stream" | "session"; actor: "automatic" | "user"; before: StreamWire | SessionWire; changes: Record<string, unknown>; ownership_token: string | null; inputs: {kind: "stream" | "session"; record: StreamWire | SessionWire}[]};
type CommandResult = {code: number | null; stdout: string; stderr: string};
export type Runner = (args: string[], input?: string, options?: {timeout: number; signal?: AbortSignal}) => Promise<CommandResult>;
export class StreamsCliError extends Error {
 constructor(message: string, readonly kind?: string, readonly receipt?: Receipt) { super(message); }
}
const object = (value: unknown): value is Record<string, any> => !!value && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === "string" && !value.includes("\0");
const result = (v: unknown): v is Result => object(v) && ["unchanged", "persisted", "committed", "repair-required"].includes(v.local) && ["local-only", "pending", "confirmed", "blocked", "failed", "uncertain"].includes(v.sharing) && (v.domain === null || object(v.domain) && ["changes", "no-change", "superseded", "conflict"].includes(v.domain.outcome));
function receipt(value: unknown): value is Receipt {
 if (!object(value) || value.publication !== undefined && (!result(value.publication) || value.publication.domain !== null)) return false;
 if (value.save !== undefined) return value.saves === undefined && text(value.operation_id) && !!value.operation_id && result(value.save);
 if (value.operation_id !== undefined || !Array.isArray(value.saves) || !value.saves.every((s: unknown) => object(s) && text(s.operation_id) && !!s.operation_id && result(s.save))) return false;
 return value.saves.length > 0 || stream(value.stream) && session(value.session) && "authority" in value && (value.publication !== undefined || value.completion_error !== undefined);
}
export function resultNeedsAttention(value: Result): boolean {
 return value.local === "repair-required" || ["failed", "blocked", "uncertain"].includes(value.sharing);
}
export function receiptCanContinue(value: Receipt): boolean {
 const saves = value.saves?.map(item => item.save) ?? (value.save ? [value.save] : []);
 return !value.completion_error && value.publication?.local !== "repair-required" && saves.every(save => save.local !== "repair-required" && !!save.domain && ["changes", "no-change"].includes(save.domain.outcome));
}
export function receiptHasWarning(value: Receipt): boolean {
 const saves = value.saves?.map(item => item.save) ?? (value.save ? [value.save] : []);
 return !receiptCanContinue(value) || saves.some(resultNeedsAttention) || !!value.publication && resultNeedsAttention(value.publication);
}
export class StreamCli {
 private runner: Runner;
 constructor(readonly cwd: string, runner?: Runner) {
  this.runner = runner ?? ((args, input, options) => new Promise((resolve, reject) => {
   const signal = options?.signal;
   signal?.throwIfAborted();
   const detached = !!options?.timeout || !!signal;
   const child = spawn(resolveStreamsBin(), args, {cwd, detached, stdio: ["pipe", "pipe", "pipe"]});
   let stdout = "", stderr = "", failure: Error | undefined;
   // Wait for the process group and stdio to close before releasing lifecycle work.
   const terminate = (error: Error) => {
    failure ??= error;
    if (child.pid) try {process.kill(detached ? -child.pid : child.pid, "SIGKILL");} catch (cause) {if ((cause as NodeJS.ErrnoException).code !== "ESRCH") failure = cause as Error;}
   };
   const abort = () => terminate(signal?.reason instanceof Error ? signal.reason : new StreamsCliError("Native catalog request cancelled"));
   signal?.addEventListener("abort", abort, {once: true});
   const timer = options?.timeout ? setTimeout(() => terminate(new StreamsCliError(args[0] === "sync"
    ? "Catalog sync timed out; publication may be uncertain. Run streams sync to inspect publication or repair state; do not discard saved changes."
    : "Native catalog request timed out; capture may be incomplete. Retry after checking the workspace.")), options.timeout) : undefined;
   child.stdout.setEncoding("utf8").on("data", (chunk) => {stdout += chunk;});
   child.stderr.setEncoding("utf8").on("data", (chunk) => {stderr += chunk;});
   child.on("error", (error) => {failure ??= error;});
   child.on("close", (code) => {
    clearTimeout(timer); signal?.removeEventListener("abort", abort);
    if (failure) reject(failure); else resolve({code, stdout, stderr});
   });
   child.stdin.on("error", (error: NodeJS.ErrnoException) => {if (error.code !== "EPIPE") terminate(error);});
   child.stdin.end(input);
   if (signal?.aborted) abort();
  }));
 }
 private async run(args: string[], input?: unknown, purpose: "read" | "save" | "publication" = "read", options?: {timeout: number; signal?: AbortSignal}): Promise<any> {
  options?.signal?.throwIfAborted();
  const output = await this.runner(args, input === undefined ? undefined : JSON.stringify(input), options ?? (purpose === "publication" ? {timeout: 10000} : undefined));
  options?.signal?.throwIfAborted();
  let value: any;
  try {value = JSON.parse(output.stdout);} catch {throw new StreamsCliError(`streams ${args.join(" ")}: invalid JSON response (${output.code}); ${output.stderr}`);}
  // Accepted local saves are receipts even when publication or later enrollment steps failed.
  if ((purpose === "save" && receipt(value) || purpose === "publication" && result(value) && value.domain === null) && (output.code === 0 || output.code === 2)) return value;
  if (output.code !== 0 || purpose !== "read") throw new StreamsCliError(value?.error?.message ?? output.stderr ?? "Invalid catalog receipt", value?.error?.kind);
  return value;
 }
 async context(sessionId: string, streamId?: string): Promise<LocalContext> {
  const value = await this.run(["agent", "context", "--session-id", sessionId, "--harness", "pi", ...(streamId ? ["--stream-id", streamId] : []), "--json"]);
  try {return parseLocalContext(value);} catch {throw new StreamsCliError("Invalid local catalog context");}
 }
 async list(): Promise<StreamWire[]> {
  const value = await this.run(["list", "--json"]);
  const rows = value?.streams;
  if (!Array.isArray(rows) || !rows.every(stream)) throw new StreamsCliError("Invalid catalog list");
  return rows;
 }
 async captureInputs(target?: {kind: "stream" | "workspace"; id: string}, signal?: AbortSignal): Promise<unknown> {
  return this.run(["agent", "capture-inputs", ...(target ? [target.kind === "stream" ? "--stream-id" : "--workspace-id", target.id] : []), "--json"], undefined, "read", {timeout: 10000, signal});
 }
 async preview(input: {catalog: LocalContext; action: "apply" | "enroll"; request: unknown}, signal?: AbortSignal): Promise<{catalog: LocalContext; receipt: Receipt}> {
  const value = await this.run(["agent", "preview", "--json"], input, "read", {timeout: 10000, signal});
  if (!object(value) || !receipt(value.receipt) || value.receipt.simulated !== true) throw new StreamsCliError("Invalid simulated catalog receipt");
  return {catalog: parseLocalContext(value.catalog), receipt: value.receipt};
 }
 async captureArrangement(streamId: string, signal?: AbortSignal): Promise<unknown> {return this.run(["workspace", "capture", streamId, "--json"], undefined, "read", {timeout: 10000, ...(signal ? {signal} : {})});}
 async sync(event: "start" | "shutdown"): Promise<PublicationResult> {return this.run(["sync", "--event", event, "--json"], undefined, "publication");}
 async enroll(request: unknown): Promise<Receipt> {return this.run(["agent", "enroll", "--json"], request, "save");}
 async apply(request: ApplyRequest): Promise<Receipt> {return this.run(["agent", "apply", "--json"], request, "save");}
}
export function receiptText(value: Receipt): string {
 const saves = value.saves ?? (value.save ? [{operation_id: value.operation_id ?? "", save: value.save}] : []);
 if (!saves.length) return "Already enrolled; no new local saves";
 if (receiptHasWarning(value)) {
  return saves.map(({operation_id, save}) => `${operation_id}: ${save.domain?.outcome ?? "not reconciled; save outcome uncertain — inspect catalog repair state before another write"}; local=${save.local}; sharing=${save.sharing}${save.detail ? `; ${save.detail}` : ""}`).join("\n") + (value.publication ? `\nPublication: ${JSON.stringify(value.publication)}` : "") + (value.completion_error ? `\nSaved receipt retained; completion problem: ${JSON.stringify(value.completion_error)}. Do not blindly retry.` : "");
 }
 if (value.saves) {
  const label = [value.stream?.title, value.session?.title].filter(Boolean).join(" / ");
  return label ? `Work card enrolled: ${label}` : "Work card enrolled";
 }
 if (value.save?.domain?.outcome === "no-change") return "Work card up to date (no changes)";
 if (value.session) {
  const summary = value.session.state_short || value.session.state;
  return `Work card updated (${value.session.title})${summary ? `: ${summary}` : ""}`;
 }
 if (value.stream) {
  const summary = value.stream.goal_short || value.stream.goal;
  return `Work card effort updated (${value.stream.title})${summary ? `: ${summary}` : ""}`;
 }
 return "Work card updated";
}
