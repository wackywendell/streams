import type { Usage } from "@earendil-works/pi-ai";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { labParameters } from "./lab.ts";
import type { LabOperation } from "./lab.ts";

export function registerLabTools(pi: ExtensionAPI, options: {enabled: () => boolean; execute: (operation: LabOperation, signal: AbortSignal, ctx: ExtensionContext) => Promise<unknown>}): void {
 if (!options.enabled()) return;
 const descriptions = {
  capture: "Freeze the explicitly named stream or workspace into a private capture.",
  prepare: "Prepare a saved request from an explicit capture sample and task, or fork a saved request. Does not run a model.",
  run: "Run a saved request with the explicitly specified provider and model. No fallback or live catalog writes.",
  replay: "Replay a saved capture with the explicitly specified provider and model in a sandbox. No live catalog writes.",
  inspect: "Inspect a saved artifact summary, or explicitly request paged input, output or errors.",
  list: "List saved artifact IDs and directories for the specified kind. Does not read raw content.",
 };
 for (const operation of Object.keys(labParameters) as (keyof typeof labParameters)[]) {
  pi.registerTool({
   name: `stream_debug_${operation}`, label: `Streams debug ${operation}`, description: descriptions[operation], parameters: labParameters[operation],
   async execute(_id, args, signal, _update, ctx) {
    if (!options.enabled()) throw Error("Debug tools are disabled. Enable Debug commands in /streams settings and /reload to discover them.");
    if (!signal) throw Error("Tool abort signal is unavailable");
    const details = await options.execute({...args, operation} as LabOperation, signal, ctx);
    const summary = details && typeof details === "object" ? details as {status?: unknown; failed?: unknown; unrun?: unknown; usage?: Usage} : undefined;
    const isError = operation === "run" && (summary?.status === "failed" || summary?.status === "cancelled")
     || operation === "replay" && (typeof summary?.failed === "number" && summary.failed > 0 || typeof summary?.unrun === "number" && summary.unrun > 0);
    return {content: [{type: "text" as const, text: JSON.stringify(details)}], details, ...(summary?.usage ? {usage: summary.usage} : {}), ...(isError ? {isError: true} : {})};
   },
  });
 }
}
