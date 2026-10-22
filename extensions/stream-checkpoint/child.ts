import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { resolveStreamsBin } from "../../integrations/herdr-work-cards/src/streams-bin.ts";
import type { Runner } from "./streams.ts";
import { observation, type Observation } from "./native-observation.ts";

export type ChildScope = {value: Observation | null; reason?: string};
type Pending = {resolve: (value: {code: number; stdout: string; stderr: string}) => void; reject: (error: Error) => void};
const object = (value: unknown): value is Record<string, any> => !!value && typeof value === "object" && !Array.isArray(value);
const pluginRoot = fileURLToPath(new URL("../../integrations/herdr-work-cards", import.meta.url));

/** One stdio transport per Pi lifetime; request cancellation never kills the session child. */
export class StreamsChild {
 private process?: ReturnType<typeof spawn>;
 private ready?: Promise<void>;
 private stopped = false;
 private restarted = false;
 private nextId = 0;
 private pending = new Map<number, Pending>();
 private listeners = new Set<() => void>();
 private failures = new Set<(error: Error) => void>();
 private closing?: Promise<void>;
 private current?: ChildScope;
 revision = 0;
 constructor(private readonly options: {sessionId: string; cwd: string; env?: NodeJS.ProcessEnv; warning: (message: string) => void; spawn?: typeof spawn}) {}
 get scope(): ChildScope | undefined {return this.current;}
 get didRestart(): boolean {return this.restarted;}
 subscribe(listener: () => void): () => void {this.listeners.add(listener);return () => {this.listeners.delete(listener);};}
 private restart(error: Error): void {
  this.process = undefined;this.ready = undefined;
  if (this.stopped) return;
  if (!this.restarted) {
   this.restarted = true;this.options.warning(`Streams child failed: ${error.message}. Restarting once.`);
   void this.start().catch(() => {});
  } else {
   this.stopped = true;this.options.warning(`Streams child failed again: ${error.message}. Streams stopped for this Pi session; reload required.`);
  }
 }
 start(): Promise<void> {
  if (this.stopped) return Promise.reject(Error("Streams stopped for this Pi session; reload required"));
  if (this.ready) return this.ready;
  let resolve!: () => void, reject!: (error: Error) => void;
  this.ready = new Promise<void>((yes, no) => {resolve = yes;reject = no;});
  // A restart is eager even when no caller is waiting for its handshake.
  void this.ready.catch(() => {});
  const result = this.ready;
  let child: ReturnType<typeof spawn>;
  try {
   const env = this.options.env ?? process.env, binary = resolveStreamsBin(env);
   child = (this.options.spawn ?? spawn)(binary, ["serve", "--session-id", this.options.sessionId, "--plugin-root", pluginRoot],
    {cwd:this.options.cwd, env:{...env, STREAMS_RUST_BIN:binary}, stdio:["pipe", "pipe", "pipe"]});
  } catch (error) {
   reject(error as Error);this.restart(error as Error);return result;
  }
  this.process = child;
  let first = true, buffer = "", failed = false;
  const fail = (error: Error) => {
   if (failed || this.process !== child) return;
   failed = true;clearTimeout(timer);reject(error);
   this.current = undefined;
   for (const request of this.pending.values()) request.reject(error);
   this.pending.clear();
   for (const listener of this.failures) listener(error);
   // Protocol errors must not leave a child accepting further requests.
   child.kill("SIGKILL");this.restart(error);
   // Invalidate cached native work too, including user actions waiting for a scope.
   for (const listener of this.listeners) listener();
  };
  const timer = setTimeout(() => fail(Error("Streams child ready handshake timed out")), 10_000);
  const message = (line: string) => {
   const value: unknown = JSON.parse(line);
   if (!object(value)) throw Error("Invalid Streams child message");
   if (first) {
    if (value.type !== "ready" || value.protocol !== 1) throw Error("Unsupported Streams child handshake (expected protocol 1)");
    first = false;clearTimeout(timer);resolve();return;
   }
   if (value.type === "response") {
    if (!Number.isSafeInteger(value.id) || value.id <= 0 || ![0, 1, 2].includes(value.code) || !("value" in value)
     || !Array.isArray(value.notices) || !value.notices.every((notice: unknown) => typeof notice === "string")) throw Error("Invalid Streams child response");
    const request = this.pending.get(value.id);
    this.pending.delete(value.id);
    request?.resolve({code:value.code, stdout:JSON.stringify(value.value), stderr:value.notices.join("\n")});return;
   }
   if (value.type === "event" && value.event === "scope") {
    if (!(object(value.value) || value.value === null && typeof value.reason === "string")) throw Error("Invalid Streams child scope");
    this.current = {value:value.value === null ? null : observation(value.value, this.options.sessionId), reason:value.reason};this.revision++;
    for (const listener of this.listeners) listener();return;
   }
   if (value.type === "event" && value.event === "warning" && typeof value.message === "string") {this.options.warning(value.message);return;}
   throw Error("Unknown Streams child message");
  };
  child.stdout!.setEncoding("utf8").on("data", (chunk: string) => {
   if (failed || this.process !== child) return;
   buffer += chunk;
   try {
    let end: number;
    while ((end = buffer.indexOf("\n")) >= 0) {const line = buffer.slice(0, end);buffer = buffer.slice(end + 1);message(line);}
   } catch (error) {fail(error as Error);}
  });
  // Diagnostics are not protocol input. Drain them without retaining an unbounded log.
  child.stderr!.resume();
  child.stdin!.on("error", fail);child.on("error", fail);
  child.on("close", (code, signal) => {
   if (this.stopped && this.process === child) {
    clearTimeout(timer);const error = Error("Streams child shut down");reject(error);
    for (const request of this.pending.values()) request.reject(error);
    this.pending.clear();return;
   }
   fail(Error(`Streams child exited (${signal ?? code})`));
  });
  return result;
 }
 private send(value: unknown): void {
  const input = this.process?.stdin;
  if (input && !input.writableEnded) input.write(JSON.stringify(value) + "\n");
 }
 readonly run: Runner = async (argv, input, options) => {
  const signal = options?.signal;
  signal?.throwIfAborted();
  // Include startup in the operation deadline, without cancelling another caller's handshake.
  let id: number | undefined, timer: ReturnType<typeof setTimeout> | undefined;
  let abort!: () => void;
  const cancellation = new Promise<never>((_resolve, reject) => {
   abort = () => {
    if (id !== undefined && this.pending.delete(id)) this.send({type:"cancel", id});
    reject(signal?.reason instanceof Error ? signal.reason : Error("Native catalog request cancelled"));
   };
   signal?.addEventListener("abort", abort, {once:true});
   timer = setTimeout(() => {
    if (id !== undefined && this.pending.delete(id)) this.send({type:"cancel", id});
    reject(Error(argv[0] === "sync"
     ? "Catalog sync timed out; publication may be uncertain. Run streams sync to inspect publication or repair state; do not discard saved changes."
     : "Native catalog request timed out; capture may be incomplete. Retry after checking the workspace."));
   }, options?.timeout ?? 120_000);
  });
  try {
   const ready = this.start(), child = this.process;
   await Promise.race([ready, cancellation]);signal?.throwIfAborted();
   if (this.stopped || child !== this.process || ready !== this.ready) throw Error("Streams child lifetime ended before request dispatch");
   const request = new Promise<{code: number; stdout: string; stderr: string}>((resolve, reject) => {
    id = ++this.nextId;this.pending.set(id, {resolve, reject});
    this.send({type:"request", id, argv, ...(input === undefined ? {} : {input:JSON.parse(input)})});
   });
   return await Promise.race([request, cancellation]);
  } finally {clearTimeout(timer);signal?.removeEventListener("abort", abort);}
 };
 async waitForScope(signal?: AbortSignal): Promise<ChildScope> {
  signal?.throwIfAborted();await this.start();signal?.throwIfAborted();
  if (this.current) return this.current;
  return new Promise((resolve, reject) => {
   const clean = () => {unsubscribe();this.failures.delete(failure);signal?.removeEventListener("abort", abort);};
   const failure = (error: Error) => {clean();reject(error);};
   const abort = () => failure(signal?.reason instanceof Error ? signal.reason : Error("Streams scope cancelled"));
   const unsubscribe = this.subscribe(() => {clean();resolve(this.current!);});
   this.failures.add(failure);signal?.addEventListener("abort", abort, {once:true});
   if (signal?.aborted) abort();
  });
 }
 stop(): Promise<void> {
  if (this.closing) return this.closing;
  this.stopped = true;
  for (const listener of this.failures) listener(Error("Streams child shut down"));
  const child = this.process;
  if (!child) return Promise.resolve();
  this.closing = new Promise(resolve => {child.once("close", () => {this.process = undefined;resolve();});});
  child.stdin!.end();return this.closing;
 }
}
