import {
  sessionKeyOf,
  streamKeyOf,
  type EditResult,
  type Mode,
  type StreamCli,
} from "./streams.ts";

type Target = "session" | "stream";

const modeDescriptions: Record<Mode, string> = {
  automatic: "Allow future title generation",
  manual: "Protect the saved title from generation",
};

export type NamingEdit =
  | { target: Target; kind: "name"; text: string }
  | { target: Target; kind: "mode"; mode: Mode };

export function parseNamingEdit(command: "name" | "title-mode", args: string): NamingEdit {
  const text = args.trim();
  const selector = /^(session|stream)(?:\s+|$)/.exec(text);
  const target = (selector ? selector[1] : "session") as Target;
  const value = (selector ? text.slice(selector[0].length) : text).trim();
  if (command === "name") {
    if (!value || value.includes("\0")) {
      throw new Error("Usage: /streams name [session|stream] <name>");
    }
    return { target, kind: "name", text: value };
  }
  if (value !== "automatic" && value !== "manual") {
    throw new Error("Usage: /streams title-mode [session|stream] automatic|manual");
  }
  return { target, kind: "mode", mode: value };
}

export function namingCompletions(
  prefix: string
): { value: string; label: string; description: string }[] | null {
  const match = /^(name|title-mode) (.*)$/s.exec(prefix);
  if (!match) return null;
  const [, command, rest] = match;
  const selector = /^(session|stream) +/.exec(rest);
  const partial = selector ? rest.slice(selector[0].length) : rest;
  if (/\s/.test(partial)) return null;
  const targets = [
    { label: "session", description: "This conversation's saved title" },
    { label: "stream", description: "The Stream's saved title" },
  ];
  const modes = Object.entries(modeDescriptions).map(([label, description]) => ({
    label,
    description,
  }));
  const choices = [...(!selector ? targets : []), ...(command === "title-mode" ? modes : [])];
  const matches = choices
    .filter((item) => item.label.startsWith(partial))
    .map((item) => ({ ...item, value: `${command} ${selector?.[0] ?? ""}${item.label}` }));
  return matches.length ? matches : null;
}

/** Save captured naming intent using format-5 `{ title: { text, manual } }`. */
export async function saveNamingEdit(
  client: Pick<StreamCli, "context" | "editSession" | "editStream">,
  nativeId: string,
  edit: NamingEdit,
  assertCurrent: () => void
): Promise<EditResult> {
  assertCurrent();
  const catalog = await client.context(nativeId);
  assertCurrent();
  const record = catalog[edit.target];
  if (!record) throw new Error(`No attached ${edit.target}`);
  const nextTitle =
    edit.kind === "name"
      ? { text: edit.text, manual: true }
      : { text: record.title.text, manual: edit.mode === "manual" };
  const body = {
    changes: { title: nextTitle },
    before: { title: structuredClone(record.title) },
  };
  return edit.target === "session"
    ? client.editSession(sessionKeyOf(catalog.session!), body)
    : client.editStream(streamKeyOf(catalog.stream!), body);
}

/** Comparing current values ignores ordinary echoes from Pi name projection. */
export async function savePiNameEdit(
  client: Pick<StreamCli, "context" | "editSession">,
  nativeId: string,
  name: string | undefined,
  assertCurrent: () => void
): Promise<EditResult | undefined> {
  if (!name?.trim() || name.includes("\0")) return;
  assertCurrent();
  const catalog = await client.context(nativeId);
  if (!catalog.session || catalog.session.title.text === name) return;
  assertCurrent();
  return client.editSession(sessionKeyOf(catalog.session), {
    changes: { title: { text: name, manual: true } },
    before: { title: structuredClone(catalog.session.title) },
  });
}
