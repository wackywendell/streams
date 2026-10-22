import { isAbsolute } from "node:path";
import type { NativeContext, NativeSnapshot } from "./native.ts";

export type Observation = NativeSnapshot & {
 version: 1;
 context: NativeContext & {binding: string | null};
 config: {automatic: boolean; sessionNaming: boolean};
};
export const contextKeys = ["socket", "paneId", "terminalId", "workspaceId", "tabId"] as const;
export const scopeKeys = ["catalogSource", "hostId", "streamId", "sessionId", "nativeSessionId"] as const;
const object = (value: unknown): value is Record<string, any> => !!value && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === "string" && !!value.trim() && !/[\x00-\x1f\x7f]/.test(value);

export function observation(value: unknown, sessionId: string): Observation {
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
