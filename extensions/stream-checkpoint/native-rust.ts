import { isAbsolute } from "node:path";
import { fileURLToPath } from "node:url";
import { resolveStreamsBin } from "../../integrations/herdr-work-cards/src/streams-bin.ts";
import { ProcessExitError, runProcess } from "../../integrations/herdr-work-cards/src/herdr/subprocess.ts";
import { projectSessionName } from "../../integrations/herdr-work-cards/src/pi/session-naming.ts";
import type { NativeContext, NativeFactory, NativeSnapshot } from "./native.ts";

type Observation = NativeSnapshot & {
 version: 1;
 context: NativeContext & {binding: string | null};
 config: {automatic: boolean; sessionNaming: boolean};
};
type Expectation = Pick<Observation, "version" | "scope" | "context">;
const object = (value: unknown): value is Record<string, any> => !!value && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === "string" && !!value.trim() && !/[\x00-\x1f\x7f]/.test(value);
const contextKeys = ["socket", "paneId", "terminalId", "workspaceId", "tabId"] as const;
const scopeKeys = ["catalogSource", "hostId", "streamId", "sessionId", "nativeSessionId"] as const;
function observation(value: unknown, sessionId: string): Observation {
 if (!object(value) || value.version !== 1 || !object(value.scope) || !scopeKeys.every(key => text(value.scope[key]))
  || value.scope.nativeSessionId !== sessionId || !object(value.context) || !contextKeys.every(key => text(value.context[key]))
  || !isAbsolute(value.context.socket) || !(value.context.binding === null || text(value.context.binding))
  || !object(value.native) || value.native.pane_id !== value.context.paneId || value.native.tab_id !== value.context.tabId
  || value.native.workspace_id !== value.context.workspaceId || !object(value.session) || !text(value.session.title)
  || !["automatic", "manual"].includes(value.session.title_mode)
  || !object(value.config) || typeof value.config.automatic !== "boolean" || typeof value.config.sessionNaming !== "boolean") {
  throw Error("Invalid Rust native owner observation");
 }
 return value as Observation;
}

/** Pi keeps its event lane and session-name provenance; Rust owns native effects and evidence. */
export const loadRustNative: NativeFactory = async ({pi, ctx, sessionId, cwd, env}) => {
 const binary = resolveStreamsBin(env);
 const environment = {...env, STREAMS_RUST_BIN:binary};
 const pluginRoot = fileURLToPath(new URL("../../integrations/herdr-work-cards", import.meta.url));
 let expected: Expectation | undefined, current: Observation | undefined, switched = false;
 const call = async (command: string[], signal?: AbortSignal): Promise<Record<string, any>> => {
  signal?.throwIfAborted();
  if (switched) throw Error("Native viewing scope changed; reload required");
  let output: string;
  try {
   output = await runProcess(binary, ["native", "--session-id", sessionId, "--plugin-root", pluginRoot,
    ...(expected && command[0] !== "observe" ? ["--expect", JSON.stringify(expected)] : []), ...command, "--json"],
    {env:environment, cwd, signal, timeoutMs:120_000});
  } catch (error) {
   if (error instanceof ProcessExitError) {
    let data: unknown;
    try {data = JSON.parse(error.stdout);} catch {}
    if (object(data) && object(data.error) && typeof data.error.message === "string") {
     if (data.error.kind === "native-context-changed") {switched = true;throw Error("Native viewing scope changed; reload required");}
     throw Error(data.error.message);
    }
   }
   throw error;
  }
  signal?.throwIfAborted();
  const value: unknown = JSON.parse(output);
  if (!object(value) || value.error) throw Error("Invalid Rust native acknowledgement");
  return value;
 };
 const check = (context: NativeContext, signal?: AbortSignal) => {
  signal?.throwIfAborted();
  if (!expected || switched || contextKeys.some(key => context[key] !== expected!.context[key])) throw Error("Native owner context changed");
 };
 const acceptStatus = (value: Record<string, any>) => {
  const status = value.native;
  if (!expected || !object(status) || status.workspace_id !== expected.context.workspaceId
   || status.owner_pane_id !== expected.context.paneId || status.owner_terminal_id !== expected.context.terminalId
   || status.session_id !== expected.scope.sessionId || status.native_session_id !== sessionId
   || !(status.binding === null || text(status.binding))) throw Error("Invalid Rust owner acknowledgement");
  if (expected.context.binding !== null && expected.context.binding !== status.binding) {
   switched = true;throw Error("Native viewing scope changed; reload required");
  }
  expected = {...expected, context:{...expected.context, binding:status.binding}};
 };
 return {
  get automatic() {return current?.config.automatic ?? false;},
  get sessionNaming() {return current?.config.sessionNaming ?? false;},
  read: async signal => {
   let raw: Record<string, any>;
   try {raw = await call(["observe"], signal);}
   catch (error) {throw Error(`Streams context read failed: ${String(error)}`);}
   const value = observation(raw, sessionId);
   if (expected && (scopeKeys.some(key => expected!.scope[key] !== value.scope[key])
    || contextKeys.some(key => expected!.context[key] !== value.context[key])
    || expected.context.binding !== null && expected.context.binding !== value.context.binding)) {
    switched = true;throw Error("Native viewing scope changed; reload required");
   }
   expected = {version:1, scope:{...value.scope}, context:{...value.context}};
   current = value;return value;
  },
  bootstrap: async signal => {
   signal?.throwIfAborted();
   if (!expected) throw Error("Native bootstrap requires an attested Rust owner observation");
   return {...expected.context};
  },
  open: async (context, signal, mode) => {
   check(context, signal);
   const result = await call(["open", ...(mode === "automatic" ? ["--automatic"] : [])], signal);
   if (!["opened", "existing", "suppressed"].includes(result.display)) throw Error("Invalid Rust display acknowledgement");
   acceptStatus(result);return result;
  },
  close: async (context, signal) => {
   check(context, signal);const result = await call(["close"], signal);
   if (result.closed !== true) throw Error("Invalid Rust close acknowledgement");
   return result;
  },
  project: async (context, signal, intent = "automatic") => {
   check(context, signal);
   const reclaim = {"automatic":undefined, "explicit":"all", "explicit-workspace":"workspace", "explicit-tab":"tab"}[intent];
   const result = await call(["project", ...(reclaim ? ["--reclaim", reclaim] : [])], signal);
   const outcomes = ["disabled", "not-first", "unchanged", "manual", "renamed"];
   if (!object(result.names) || !outcomes.includes(result.names.workspace)
    || !outcomes.includes(result.names.tab)) throw Error("Invalid Rust projection acknowledgement");
   acceptStatus(result);return result;
  },
  capture: async (snapshot, signal) => {
   if (!expected || scopeKeys.some(key => snapshot.scope[key] !== expected!.scope[key])) throw Error("Native capture scope changed");
   return call(["capture"], signal);
  },
  sessionName: (snapshot, assertCurrent, intent) => {
   if (!expected || switched || scopeKeys.some(key => snapshot.scope[key] !== expected!.scope[key])) throw Error("Native Pi naming scope changed");
   projectSessionName(pi, ctx.sessionManager, sessionId, snapshot.session.title, assertCurrent, intent);
  },
 };
};
