import { editRequest, titleModes } from "./core.ts";
import type { Receipt, StreamCli } from "./streams.ts";

type Target = "session" | "stream";
type TitleMode = typeof titleModes[number];
export type NamingEdit = {target: Target; changes: {title: string; title_mode: "manual"} | {title_mode: TitleMode}};

export function parseNamingEdit(command: "name" | "title-mode", args: string): NamingEdit {
 const text = args.trim(), selector = /^(session|stream)(?:\s+|$)/.exec(text);
 const target = selector ? selector[1] as Target : "session";
 const value = (selector ? text.slice(selector[0].length) : text).trim();
 if (command === "name") {
  if (!value || value.includes("\0")) throw Error("Usage: /streams name [session|stream] <name>");
  return {target, changes: {title: value, title_mode: "manual"}};
 }
 const mode = titleModes.find(mode => mode === value);
 if (!mode) throw Error("Usage: /streams title-mode [session|stream] automatic|manual|inherit");
 return {target, changes: {title_mode: mode}};
}

/** Save captured naming intent; ordinary projection belongs to the catalog-change path. */
export async function saveNamingEdit(client: Pick<StreamCli, "context" | "apply">, nativeId: string, edit: NamingEdit, assertCurrent: () => void): Promise<Receipt> {
 assertCurrent();
 const catalog = await client.context(nativeId);
 const request = editRequest(catalog, edit.target, edit.changes, "user");
 assertCurrent();
 return client.apply(request);
}
