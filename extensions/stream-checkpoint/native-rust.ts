import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";
import { projectSessionName } from "../../integrations/herdr-work-cards/src/pi/session-naming.ts";
import { ProcessExitError } from "../../integrations/herdr-work-cards/src/herdr/subprocess.ts";
import { StreamsChild } from "./child.ts";
import { StreamsCliError } from "./streams.ts";
import type { NativeContext, NativeFactory } from "./native.ts";
import { contextKeys, scopeKeys, type Observation } from "./native-observation.ts";

type Expectation = Pick<Observation, "version" | "scope" | "context">;
const object = (value: unknown): value is Record<string, any> => !!value && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === "string" && !!value.trim() && !/[\x00-\x1f\x7f]/.test(value);

/** Pi keeps its event lane and session-name provenance; the session child owns observations and effects. */
export const loadRustNative: NativeFactory = async ({pi, ctx, sessionId, cwd, env, child: shared, warning}) => {
 const child = shared ?? new StreamsChild({sessionId, cwd, env, warning:warning ?? (() => {})});
 let expected: Expectation | undefined, current: Observation | undefined, revision = -1, waiting = -1, initialized = false;
 let statusCalls = 0, acknowledged: Expectation | undefined;
 const pendingNotifications: (() => void)[] = [];
 const statusOperation = async <T>(operation: () => Promise<T>): Promise<T> => {
  statusCalls++;
  try {return await operation();} finally {
   if (--statusCalls === 0) for (const notify of pendingNotifications.splice(0)) notify();
  }
 };
 const adopt = () => {
  if (!child.scope || revision === child.revision) return;
  const value = child.scope.value ?? undefined;
  revision = child.revision;current = value;
  expected = value ? {version:1, scope:{...value.scope}, context:{...value.context}} : undefined;
 };
 const call = async (command: string[], signal?: AbortSignal): Promise<Record<string, any>> => {
  signal?.throwIfAborted();
  const before = child.revision;
  const output = await child.run(["native", "--session-id", sessionId, "--plugin-root", fileURLToPath(new URL("../../integrations/herdr-work-cards", import.meta.url)),
   ...(expected ? ["--expect", JSON.stringify(expected)] : []), ...command, "--json"], undefined, {signal, timeout:120_000});
  signal?.throwIfAborted();
  const value: unknown = JSON.parse(output.stdout);
  if (output.code !== 0) {
   if (object(value) && object(value.error) && typeof value.error.message === "string") {
    if (value.error.kind === "native-context-changed") waiting = before;
    throw new StreamsCliError(value.error.message, value.error.kind);
   }
   throw new ProcessExitError("streams native", output.code, output.stdout, output.stderr);
  }
  if (!object(value) || value.error) throw Error("Invalid Rust native acknowledgement");
  return value;
 };
 const check = (context: NativeContext, signal?: AbortSignal) => {
  signal?.throwIfAborted();adopt();
  if (!child.scope || !expected || waiting >= child.revision) throw new StreamsCliError("Native owner context changed; waiting for scope", "native-context-changed");
  if (contextKeys.some(key => context[key] !== expected!.context[key])) throw new StreamsCliError("Native owner context changed", "native-context-changed");
 };
 const acceptStatus = (value: Record<string, any>, expectation: Expectation) => {
  const status = value.native;
  if (!object(status) || status.workspace_id !== expectation.context.workspaceId
   || status.owner_pane_id !== expectation.context.paneId || status.owner_terminal_id !== expectation.context.terminalId
   || status.session_id !== expectation.scope.sessionId || status.native_session_id !== sessionId
   || !(status.binding === null || text(status.binding))) throw Error("Invalid Rust owner acknowledgement");
  if (expectation.context.binding !== null && expectation.context.binding !== status.binding) {
   throw new StreamsCliError("Native viewing scope changed; waiting for scope", "native-context-changed");
  }
  acknowledged = {...expectation, context:{...expectation.context, binding:status.binding}};
  if (expected === expectation) expected = acknowledged;
 };
 return {
  get automatic() {return !child.didRestart && (current?.config.automatic ?? false);},
  get sessionNaming() {return current?.config.sessionNaming ?? false;},
  subscribe: listener => child.subscribe(() => {
   const before = current;adopt();const after = current;
   const notify = () => {
    // A status acknowledgement already adopted this first binding. Its echo
    // carries no new work; title, config, membership and actual moves still do.
    const bindingEcho = before && after && before.context.binding === null
     && isDeepStrictEqual(acknowledged, {version:1, scope:after.scope, context:after.context})
     && isDeepStrictEqual({...before, context:{...before.context, binding:after.context.binding}}, after);
    if (initialized && (!child.scope || !bindingEcho && !isDeepStrictEqual(before, after))) listener();
   };
   if (statusCalls) pendingNotifications.push(notify);else notify();
  }),
  dispose: () => shared ? Promise.resolve() : child.stop(),
  read: async signal => {
   try {await child.waitForScope(signal);} finally {initialized = true;}
   adopt();
   if (waiting >= child.revision) throw new StreamsCliError("Native viewing scope changed; waiting for scope", "native-context-changed");
   if (!current) throw Error(`Streams context read failed: ${child.scope?.reason ?? "unbound"}`);
   return current;
  },
  bootstrap: async signal => {
   signal?.throwIfAborted();adopt();
   if (!expected) throw Error("Native bootstrap requires an attested Rust owner observation");
   return {...expected.context};
  },
  open: (context, signal, mode) => statusOperation(async () => {
   check(context, signal);const expectation = expected!;
   const result = await call(["open", ...(mode === "automatic" ? ["--automatic"] : [])], signal);
   if (!["opened", "existing", "suppressed"].includes(result.display)) throw Error("Invalid Rust display acknowledgement");
   acceptStatus(result, expectation);return result;
  }),
  close: async (context, signal) => {
   check(context, signal);const result = await call(["close"], signal);
   if (result.closed !== true) throw Error("Invalid Rust close acknowledgement");
   return result;
  },
  project: (context, signal, intent = "automatic") => statusOperation(async () => {
   check(context, signal);const expectation = expected!;
   const reclaim = {"automatic":undefined, "explicit":"all", "explicit-workspace":"workspace", "explicit-tab":"tab"}[intent];
   const result = await call(["project", ...(reclaim ? ["--reclaim", reclaim] : [])], signal);
   const outcomes = ["disabled", "not-first", "unchanged", "manual", "renamed"];
   if (!object(result.names) || !outcomes.includes(result.names.workspace) || !outcomes.includes(result.names.tab)) throw Error("Invalid Rust projection acknowledgement");
   acceptStatus(result, expectation);return result;
  }),
  capture: async (snapshot, signal) => {
   adopt();
   if (!expected || scopeKeys.some(key => snapshot.scope[key] !== expected!.scope[key])) throw new StreamsCliError("Native capture scope changed", "native-context-changed");
   return call(["capture"], signal);
  },
  sessionName: (snapshot, assertCurrent, intent) => {
   adopt();
   if (!expected || waiting >= child.revision || scopeKeys.some(key => snapshot.scope[key] !== expected!.scope[key])) throw new StreamsCliError("Native Pi naming scope changed", "native-context-changed");
   projectSessionName(pi, ctx.sessionManager, sessionId, snapshot.session.title, assertCurrent, intent);
  },
 };
};
