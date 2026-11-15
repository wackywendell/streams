import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { resolveStreamsBin } from "../../integrations/herdr-work-cards/src/streams-bin.ts";
import { decodeServerFrame, hcLifecycle, type ChildOutcome, type ChildView, type PiRequestContext, type Runner } from "./child-protocol.ts";

type Pending = {resolve: (value: ChildOutcome) => void; reject: (error: Error) => void};
const pluginRoot = fileURLToPath(new URL("../../integrations/herdr-work-cards", import.meta.url));

/** One transport per Pi session. Restart reattaches enabled native work, never requests. */
export class StreamsChild {
 private process?: ReturnType<typeof spawn>;
 private ready?: Promise<void>;
 private stopped = false;
 private restarted = false;
 private nativeEnabled = false;
 private nextId = 0;
 private pending = new Map<number, Pending>();
 private listeners = new Set<(view: ChildView) => void>();
 private closing?: Promise<void>;
 private closed?: Promise<void>;
 constructor(private readonly options: {sessionId: string; cwd: string; env?: NodeJS.ProcessEnv; piContext?: () => PiRequestContext; warning: (message: string) => void; spawn?: typeof spawn}) {}
 subscribe(listener: (view: ChildView) => void): () => void {this.listeners.add(listener);return () => {this.listeners.delete(listener);};}
 private restart(error: Error): void {
  this.ready = undefined;
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
  const result = this.ready, attaching = this.restarted && this.nativeEnabled;
  let child: ReturnType<typeof spawn>;
  try {
   const env = this.options.env ?? process.env, binary = resolveStreamsBin(env);
   child = (this.options.spawn ?? spawn)(binary, ["serve", "--session-id", this.options.sessionId, "--sdk-pid", String(process.pid), "--plugin-root", pluginRoot],
    {cwd:this.options.cwd, env:{...env, STREAMS_BIN_PATH:binary}, stdio:["pipe", "pipe", "pipe"]});
  } catch (error) {
   reject(error as Error);this.restart(error as Error);return result;
  }
  this.process = child;
  let resolveClose!: () => void;
  this.closed = new Promise(resolve => {resolveClose = resolve;});
  let first = true, buffer = "", failed = false, failure: Error | undefined;
  const fail = (error: Error) => {
   if (failed || this.process !== child) return;
   failed = true;failure = error;clearTimeout(timer);reject(error);
   for (const request of this.pending.values()) request.reject(error);
   this.pending.clear();
   child.kill("SIGKILL");
  };
  const timer = setTimeout(() => fail(Error("Streams child ready handshake timed out")), 10_000);
  const message = (line: string) => {
   const value = decodeServerFrame(line);
   if (first) {
    if (value.type !== "ready") throw Error("Unsupported Streams child handshake (expected protocol 2)");
    first = false;clearTimeout(timer);
    if (attaching) {
     // Dispatch attachment first; its native outcome must not gate catalog work.
     const id = ++this.nextId;
     const warn = (message: string) => {if (!this.stopped) this.options.warning(message);};
     const deadline = setTimeout(() => {
      this.pending.delete(id);this.send({type:"cancel", id});
      warn("Streams native attach timed out; catalog requests remain available.");
     }, 120_000);
     this.pending.set(id, {reject:() => {clearTimeout(deadline);}, resolve:outcome => {
      clearTimeout(deadline);
      if (outcome.status === "error") {warn(`Streams native attach: ${outcome.error.message}`);return;}
      const native = outcome.result, hc = hcLifecycle(native);
      if (hc) for (const message of hc.warnings) warn(message);
      else if (!(native.native === "active" && !Object.hasOwn(native, "display") || native.native === "inactive" && ["outside_herdr", "not_enrolled"].includes(native.reason as string))) warn("Invalid Streams child attach acknowledgement");
      if (outcome.status === "saved_with_warnings") for (const warning of outcome.warnings) warn(warning);
     }});
     const pi_context = this.options.piContext?.();
     this.send({type:"request", id, request:{kind:"attach"}, ...(pi_context ? {pi_context} : {})});
    }
    resolve();return;
   }
   if (value.type === "response") {
    const request = this.pending.get(value.id);this.pending.delete(value.id);request?.resolve(value.outcome);return;
   }
   if (value.type === "view") {
    for (const listener of this.listeners) listener(value.view);return;
   }
   if (value.type === "warning") {if (!this.stopped) this.options.warning(value.message);return;}
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
   resolveClose();
   if (this.process !== child) return;
   clearTimeout(timer);this.process = undefined;this.closed = undefined;
   const error = failure ?? Error(this.stopped ? "Streams child shut down" : `Streams child exited (${signal ?? code})`);
   if (this.stopped && (signal || code !== 0)) this.options.warning("Streams child shutdown failed; native source cleanup may be incomplete.");
   reject(error);
   for (const request of this.pending.values()) request.reject(error);
   this.pending.clear();
   this.restart(error);
  });
  return result;
 }
 private send(value: unknown): void {
  const input = this.process?.stdin;
  if (input && !input.writableEnded) input.write(JSON.stringify(value) + "\n");
 }
 readonly run: Runner = async (request, options) => {
  const signal = options?.signal;signal?.throwIfAborted();
  const intent = structuredClone(request), pi_context = this.options.piContext?.();
  if (intent.kind === "session_started" || intent.kind === "attach") this.nativeEnabled = true;
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
    this.send({type:"request", id, request:intent, ...(pi_context ? {pi_context} : {})});
   });
   return await Promise.race([response, cancellation]);
  } finally {clearTimeout(timer);signal?.removeEventListener("abort", abort);}
 };
 stop(): Promise<void> {
  if (this.closing) return this.closing;
  this.stopped = true;this.listeners.clear();
  const child = this.process;
  if (!child) return Promise.resolve();
  this.closing = this.closed!;
  child.stdin!.end();return this.closing;
 }
}
