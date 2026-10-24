import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { resolveStreamsBin } from "../../integrations/herdr-work-cards/src/streams-bin.ts";
import { decodeServerFrame, type ChildOutcome, type ChildView, type Runner } from "./child-protocol.ts";

type Pending = {resolve: (value: ChildOutcome) => void; reject: (error: Error) => void};
const pluginRoot = fileURLToPath(new URL("../../integrations/herdr-work-cards", import.meta.url));

/** One transport per Pi session. Restart attaches; it never replays a request. */
export class StreamsChild {
 private process?: ReturnType<typeof spawn>;
 private ready?: Promise<void>;
 private stopped = false;
 private restarted = false;
 private nextId = 0;
 private pending = new Map<number, Pending>();
 private listeners = new Set<(view: ChildView) => void>();
 private closing?: Promise<void>;
 constructor(private readonly options: {sessionId: string; cwd: string; env?: NodeJS.ProcessEnv; warning: (message: string) => void; spawn?: typeof spawn}) {}
 subscribe(listener: (view: ChildView) => void): () => void {this.listeners.add(listener);return () => {this.listeners.delete(listener);};}
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
  void this.ready.catch(() => {});
  const result = this.ready, attaching = this.restarted;
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
   for (const request of this.pending.values()) request.reject(error);
   this.pending.clear();
   child.kill("SIGKILL");this.restart(error);
  };
  const timer = setTimeout(() => fail(Error("Streams child ready handshake timed out")), 10_000);
  const message = (line: string) => {
   const value = decodeServerFrame(line);
   if (first) {
    if (value.type !== "ready") throw Error("Unsupported Streams child handshake (expected protocol 2)");
    first = false;
    if (!attaching) {clearTimeout(timer);resolve();return;}
    // Readiness after a restart includes attachment, so a new catalog caller
    // cannot overtake it. The failed request stays rejected on the old lifetime.
    const id = ++this.nextId;
    this.pending.set(id, {reject:fail, resolve:outcome => {
     if (outcome.status === "error") {fail(Error(outcome.error.message));return;}
     const native = outcome.result;
     if (!(native.native === "active" && !Object.hasOwn(native, "display") || native.native === "inactive" && ["outside_herdr", "not_enrolled"].includes(native.reason as string))) {
      fail(Error("Invalid Streams child attach acknowledgement"));return;
     }
     if (outcome.status === "saved_with_warnings") for (const warning of outcome.warnings) this.options.warning(warning);
     clearTimeout(timer);resolve();
    }});
    this.send({type:"request", id, request:{kind:"attach"}});return;
   }
   if (value.type === "response") {
    const request = this.pending.get(value.id);this.pending.delete(value.id);request?.resolve(value.outcome);return;
   }
   if (value.type === "view") {
    for (const listener of this.listeners) listener(value.view);return;
   }
   if (value.type === "warning") {this.options.warning(value.message);return;}
   throw Error("Unexpected Streams child ready frame");
  };
  child.stdout!.setEncoding("utf8").on("data", (chunk: string) => {
   if (failed || this.process !== child) return;
   buffer += chunk;
   try {
    let end: number;
    while ((end = buffer.indexOf("\n")) >= 0) {
     const line = buffer.slice(0, end);buffer = buffer.slice(end + 1);message(line);
     if (failed) break;
    }
   } catch (error) {fail(first ? Error(`Unsupported Streams child handshake (expected protocol 2): ${String(error)}`) : error as Error);}
  });
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
 readonly run: Runner = async (request, options) => {
  const signal = options?.signal;signal?.throwIfAborted();
  const intent = structuredClone(request);
  let id: number | undefined, timer: ReturnType<typeof setTimeout> | undefined;
  let abort!: () => void;
  const cancellation = new Promise<never>((_resolve, reject) => {
   const cancel = (error: Error) => {
    if (id !== undefined && this.pending.delete(id)) this.send({type:"cancel", id});
    reject(error);
   };
   abort = () => cancel(signal?.reason instanceof Error ? signal.reason : Error("Streams request cancelled"));
   signal?.addEventListener("abort", abort, {once:true});
   timer = setTimeout(() => cancel(Error(request.kind === "sync"
    ? "Catalog sync timed out; publication may be uncertain. Run streams sync to inspect publication or repair state; do not discard saved changes."
    : "Streams request timed out; an effect may already have happened. Inspect saved state before retrying.")), options?.timeout ?? 120_000);
  });
  try {
   const ready = this.start(), child = this.process;
   await Promise.race([ready, cancellation]);signal?.throwIfAborted();
   if (this.stopped || child !== this.process || ready !== this.ready) throw Error("Streams child lifetime ended before request dispatch");
   const response = new Promise<ChildOutcome>((resolve, reject) => {
    id = ++this.nextId;this.pending.set(id, {resolve, reject});
    this.send({type:"request", id, request:intent});
   });
   return await Promise.race([response, cancellation]);
  } finally {clearTimeout(timer);signal?.removeEventListener("abort", abort);}
 };
 stop(): Promise<void> {
  if (this.closing) return this.closing;
  this.stopped = true;this.listeners.clear();
  const child = this.process;
  if (!child) return Promise.resolve();
  this.closing = new Promise(resolve => {child.once("close", () => {this.process = undefined;resolve();});});
  child.stdin!.end();return this.closing;
 }
}
