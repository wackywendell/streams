import { editRequest } from "./core.ts";
import type { Receipt, StreamCli } from "./streams.ts";

type Target = "session" | "stream";
/** Completion text only; Rust decides which modes exist. */
const modeDescriptions: Record<string, string> = {automatic:"Allow future title generation", manual:"Protect the saved title from generation", inherit:"Follow the global title default"};
export type NamingEdit = {target: Target; changes: {title: string; title_mode: "manual"} | {title_mode: string}};

export function parseNamingEdit(command: "name" | "title-mode", args: string): NamingEdit {
 const text = args.trim(), selector = /^(session|stream)(?:\s+|$)/.exec(text);
 const target = selector ? selector[1] as Target : "session";
 const value = (selector ? text.slice(selector[0].length) : text).trim();
 if (command === "name") {
  if (!value || value.includes("\0")) throw Error("Usage: /streams name [session|stream] <name>");
  return {target, changes: {title: value, title_mode: "manual"}};
 }
 if (!value || /\s/.test(value)) throw Error("Usage: /streams title-mode [session|stream] automatic|manual|inherit");
 return {target, changes: {title_mode: value}};
}

export function namingCompletions(prefix: string): {value: string; label: string; description: string}[] | null {
 const match = /^(name|title-mode) (.*)$/s.exec(prefix);
 if (!match) return null;
 const [, command, rest] = match, selector = /^(session|stream) +/.exec(rest);
 const partial = selector ? rest.slice(selector[0].length) : rest;
 if (/\s/.test(partial)) return null;
 const targets = [{label: "session", description: "This conversation's saved title"}, {label: "stream", description: "The Stream's saved title"}];
 const modes = Object.entries(modeDescriptions).map(([label, description]) => ({label, description}));
 const choices = [...(!selector ? targets : []), ...(command === "title-mode" ? modes : [])];
 const matches = choices.filter(item => item.label.startsWith(partial)).map(item => ({...item, value: `${command} ${selector?.[0] ?? ""}${item.label}`}));
 return matches.length ? matches : null;
}

/** Save captured naming intent; ordinary projection belongs to the catalog-change path. */
export async function saveNamingEdit(client: Pick<StreamCli, "context" | "apply">, nativeId: string, edit: NamingEdit, assertCurrent: () => void): Promise<Receipt> {
 assertCurrent();
 const catalog = await client.context(nativeId);
 const request = editRequest(catalog, edit.target, edit.changes, "user");
 assertCurrent();
 return client.apply(request);
}

/** Comparing current values also ignores ordinary echoes from Pi name projection. */
export async function savePiNameEdit(client: Pick<StreamCli, "context" | "apply">, nativeId: string, name: string | undefined, assertCurrent: () => void): Promise<Receipt | undefined> {
 if (!name?.trim() || name.includes("\0")) return;
 assertCurrent();
 const catalog = await client.context(nativeId);
 if (!catalog.session || catalog.session.title === name) return;
 const request = editRequest(catalog, "session", {title: name, title_mode: "manual"}, "user");
 assertCurrent();
 return client.apply(request);
}
