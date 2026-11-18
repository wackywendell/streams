import { randomUUID } from "node:crypto";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { receiptCanContinue, unreadableNotice, type Receipt, type StreamCli } from "./streams.ts";

/** Only initial enrollment offers membership selection. No native focus or workspace calls. */
export async function chooseInitial(ctx: ExtensionContext, cli: StreamCli, options: {streamId?: string; title?: string; reason?: string}, receipt: (value: Receipt) => void, signal?: AbortSignal): Promise<{status: string; stream_id?: string}> {
 const native = ctx.sessionManager.getSessionId();
 const before = await cli.context(native);
 if (before.session) return {status: "already-bound; membership moves/forks are not implemented", stream_id: before.session.stream_id};
 if (before.stream && (options.title || options.streamId && options.streamId !== before.stream.id)) throw Error("Caller workspace is already associated with a Stream. Membership moves/forks are not implemented; initial enrollment cannot switch it.");
 if (!ctx.hasUI) throw Error("Initial enrollment needs an interactive UI");
 let streamId = before.stream?.id ?? options.streamId;
 let title = options.title;
 if (!before.stream) {
  const listing = await cli.list();
  const streams = listing.streams.filter(stream => stream.owner.host_id === before.host_id);
  // Like the Streams it offers, only this host's records are named: /streams reset cannot repair a replica.
  const unreadable = unreadableNotice(listing.unreadable.filter(record => record.host === before.host_id));
  if (streamId && !streams.some((stream) => stream.id === streamId)) throw Error(`Pass an exact catalog stream ID from streams list --json${unreadable ? `\n${unreadable}` : ""}`);
  const rows = streams.map((stream) => `${stream.id} · ${stream.title}`);
  const chosen = title ? "Create…" : await ctx.ui.select(`Initial work card attachment${options.reason ? `\n${options.reason}` : ""}${unreadable ? `\n${unreadable}` : ""}`, [...(streamId ? rows.filter((row) => row.startsWith(`${streamId} ·`)) : rows), "Create…", "Cancel"]);
  if (!chosen || chosen === "Cancel" || signal?.aborted) return {status: "cancelled"};
  if (chosen === "Create…") {
   title = await ctx.ui.editor("Stream title", title ?? "");if (!title?.trim()) return {status: "cancelled"};streamId = undefined;
  } else streamId = streams.find((stream) => chosen === `${stream.id} · ${stream.title}`)!.id;
 }
 const sessionTitle = await ctx.ui.editor("This session title", "Pi contribution");if (!sessionTitle?.trim() || signal?.aborted) return {status: "cancelled"};
 if (!await ctx.ui.confirm("Enroll this conversation?", `Stream: ${title ?? streamId}\nSession: ${sessionTitle}\nMembership cannot be switched through this card.`)) return {status: "cancelled"};
 if (ctx.sessionManager.getSessionId() !== native || signal?.aborted) return {status: "cancelled"};
 const value = await cli.enroll({version: 2, catalog_source: before.catalog_source, operation_id: randomUUID(), harness: "pi", harness_session_id: native, ...(streamId ? {stream_id: streamId} : {stream: {title}}), session: {title: sessionTitle}, actor: "user", cwd: ctx.cwd});
 receipt(value);
 return {status: receiptCanContinue(value) ? "enrolled" : "receipts retained; inspect save outcomes before further writes", stream_id: value.stream?.id};
}
