import { projectSessionName } from "../../integrations/herdr-work-cards/src/pi/session-naming.ts";
import { StreamsChild } from "./child.ts";
import { StreamsCliError } from "./streams.ts";
import type { ChildRequest, DomainObject } from "./child-protocol.ts";
import type { NativeFactory } from "./native.ts";

/** UI facts never select native work or become authority in a later request. */
export const loadRustNative: NativeFactory = async ({pi, ctx, sessionId, cwd, env, child: shared, assertCurrent, warning}) => {
 const child = shared ?? new StreamsChild({sessionId, cwd, env, warning});
 const unsubscribe = child.subscribe(view => {
  if (view.desired_pi_name === null) return;
  try {projectSessionName(pi, ctx.sessionManager, sessionId, view.desired_pi_name, assertCurrent);}
  catch (error) {try {assertCurrent();warning(String(error));} catch { /* Retired UI lifetime. */ }}
 });
 const call = async (request: ChildRequest, signal: AbortSignal): Promise<DomainObject> => {
  assertCurrent();
  const outcome = await child.run(request, {timeout:120_000, signal});assertCurrent();
  if (outcome.status === "error") throw new StreamsCliError(outcome.error.message, outcome.error.kind);
  if (outcome.status === "saved_with_warnings") for (const message of outcome.warnings) warning(message);
  return outcome.result;
 };
 return {
  start: async signal => {
   const result = await call({kind:"session_started"}, signal);
   if (!(result.native === "active" && ["opened", "existing", "suppressed"].includes(result.display as string)
    || result.native === "inactive" && ["outside_herdr", "not_enrolled"].includes(result.reason as string))) throw Error("Invalid Rust lifecycle acknowledgement");
  },
  open: async signal => {
   const result = await call({kind:"open_card"}, signal);
   if (!["opened", "existing"].includes(result.display as string)) throw Error("Invalid Rust display acknowledgement");
   return result;
  },
  close: async signal => {
   const result = await call({kind:"close_card"}, signal);
   if (result.closed !== true) throw Error("Invalid Rust close acknowledgement");
   return result;
  },
  dispose: async () => {unsubscribe();if (!shared) await child.stop();},
 };
};
