import { Editor, matchesKey, truncateToWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { LocalContext, Receipt, StreamCli } from "./streams.ts";
import { FieldEdit, fields, statuses, validateReferences } from "./core.ts";
export type DiscussionState = {input: string; messages: {role: "user" | "assistant"; text: string}[]; error?: string};
export type CardState = {discussion: DiscussionState; edits: Map<string, FieldEdit>};
export function cardSummary(catalog: LocalContext): string {
 const stream = catalog.stream, session = catalog.session;
 if (!stream) return "No work card attached";
 return [`Whole effort · ${stream.title} [${stream.title_mode}]`, `ID: ${stream.id} · owner: ${stream.owner.host_id}`, `${stream.status}${stream.archived ? " · archived" : ""}`, `Goal [${stream.goal_mode}]: ${stream.goal ?? "—"}`, ...stream.documents.map((row) => `Document: ${row.repository}:${row.path} — ${row.description}${row.added_at ? ` · added at ${row.added_at}` : ""} · recovery ref ${row.ref}`), ...[...stream.issues, ...stream.prs].map((row) => `${row.url} — ${row.description}`), "", "This session", session ? `${session.title} [${session.title_mode}] · ${session.status}\nID: ${session.id} · host: ${session.host_id} · membership: ${session.stream_id}\n${session.state}\nRemaining: ${session.remaining}` : "Not yet enrolled", "", "Other recorded contributions", ...catalog.sessions.filter((row) => row.id !== session?.id || row.host_id !== session?.host_id).map((row) => `${row.title} · ${row.host_id} · ${row.status}: ${row.state_short ?? row.state}`)].join("\n");
}
/** A small composer built around Pi's editor; unsent input survives close/reopen. */
export async function compose(ctx: ExtensionContext, state: DiscussionState): Promise<string | undefined> {
 return ctx.ui.custom<string | undefined>((tui, theme, _keys, done) => {
  const editor = new Editor(tui, {borderColor: (text) => theme.fg("border", text), selectList: {selectedPrefix: (text) => theme.fg("accent", text), selectedText: (text) => theme.fg("accent", text), description: (text) => theme.fg("muted", text), scrollInfo: (text) => theme.fg("dim", text), noMatch: (text) => theme.fg("warning", text)}});
  editor.setText(state.input);editor.onChange = (text) => {state.input = text;};
  editor.onSubmit = (text) => {if (text.trim()) {state.input = text;done(text);}};
  return {
   get focused() {return editor.focused;}, set focused(value: boolean) {editor.focused = value;},
   invalidate() {editor.invalidate();},
   handleInput(data: string) {if (matchesKey(data, "escape") || matchesKey(data, "alt+p")) {state.input = editor.getExpandedText();done(undefined);} else editor.handleInput(data);},
   render(width: number) {
    const clean = (text: string) => text.replace(/[\x00-\x08\x0b-\x1f\x7f]/g, "");
    const history = state.messages.flatMap((message) => wrapTextWithAnsi(`${message.role}: ${clean(message.text)}`, width));
    return [truncateToWidth("Card discussion · Enter send · Shift+Enter newline · Esc/Alt+P back", width), ...history.slice(-Math.max(2, tui.terminal.rows - 12)), ...(state.error ? wrapTextWithAnsi(clean(state.error), width) : []), ...editor.render(width)];
   },
  };
 });
}
async function referenceRows(ctx: ExtensionContext, field: string, edit: FieldEdit): Promise<void> {
 const stream = edit.base.stream!;
 const rows = structuredClone(edit.value as any[]);
 for (;;) {
  const action = await ctx.ui.select(`${field}: choose a row${edit.error ? `\n${edit.error}` : ""}`,  [...rows.map((row, i) => `${i + 1}. ${row.url ?? `${row.repository}:${row.path}`} — ${row.description}`), "Add row", "Done"]);
  if (!action || action === "Done") {edit.value = rows;return;}
  if (action === "Add row" && field === "documents" && !Object.keys(stream.repositories).length) {ctx.ui.notify("No repository aliases are configured. Add a repository through the Streams CLI before adding a document.", "warning");continue;}
  const index = action === "Add row" ? rows.length : Number(action.split(".")[0]) - 1;
  const existing = rows[index];
  if (existing) {const choice = await ctx.ui.select("Reference row", ["Edit", "Remove", "Back"]);if (choice === "Remove") {rows.splice(index, 1);edit.value = structuredClone(rows);try {validateReferences(field, rows, stream);edit.error = undefined;} catch(error) {edit.error = String(error);}continue;}if (choice !== "Edit") continue;}
  const row: Record<string, string> = {...existing};
  if (field === "documents") {
   const alias = await ctx.ui.select("Existing repository alias", Object.keys(stream.repositories));if (!alias) continue;row.repository = alias;
   const ref = await ctx.ui.editor("Recovery branch/bookmark (does not pin opening)", row.ref ?? "");if (ref === undefined) continue;row.ref = ref;
   const path = await ctx.ui.editor("Portable relative document path", row.path ?? "");if (path === undefined) continue;row.path = path;
   const added = await ctx.ui.editor("Added-at commit (optional recovery context)", row.added_at ?? "");if (added === undefined) continue;
   if (added.trim()) row.added_at = added;else delete row.added_at;
  } else {const url = await ctx.ui.editor("HTTP(S) URL", row.url ?? "");if (url === undefined) continue;row.url = url;}
  const description = await ctx.ui.editor("Description", row.description ?? "");if (description === undefined) continue;row.description = description;
  rows[index] = row;edit.value = structuredClone(rows);
  try {validateReferences(field, rows, stream);edit.error = undefined;} catch(error) {edit.error = String(error);ctx.ui.notify(edit.error, "error");}
 }
}
export async function editField(ctx: ExtensionContext, cli: StreamCli, catalog: LocalContext, kind: "stream" | "session", field: string, state: CardState, receipt: (value: Receipt) => void): Promise<void> {
 const key = `${kind}:${field}`;
 let edit = state.edits.get(key);
 if (!edit) {edit = new FieldEdit(kind, catalog, field);state.edits.set(key, edit);} else edit.refresh(catalog);
 let editValue = !edit.dirty && !edit.error;
 try {
  for (;;) {
   if (editValue) {
    if (["documents", "issues", "prs"].includes(field)) await referenceRows(ctx, field, edit);
    else if (field === "status" || field.endsWith("_mode")) {
     const value = await ctx.ui.select(`${kind} ${field} · current: ${edit.value}${edit.error ? `\n${edit.error}` : ""}`, field === "status" ? statuses : ["automatic", "manual"]);if (value === undefined) return;edit.value = value;
    } else if (field === "archived") edit.value = !edit.base.stream!.archived;
    else {
     const value = await ctx.ui.editor(`${kind} ${field}${edit.error ? `\n${edit.error}` : ""}`, String(edit.value ?? ""));
     if (value === undefined) return;edit.value = ["goal", "goal_short", "state_short"].includes(field) && !value.trim() ? null : value;
    }
    editValue = false;
   }
   const action = await ctx.ui.select(`${edit.error ?? ""}\n${kind} ${field}: ${JSON.stringify(edit.value)}\nOnly this field will be submitted.`, ["Save field", "Edit again", "Keep input and close", "Discard input"]);
   if (action === "Discard input") {state.edits.delete(key);return;}
   if (!action || action === "Keep input and close") return;
   if (action === "Edit again") {editValue = true;continue;}
   try {await edit.save(cli, receipt);state.edits.delete(key);return;} catch(error) {ctx.ui.notify(String(error), "error");}
  }
 } finally {if (!edit.dirty && !edit.error) state.edits.delete(key);}
}
export async function openCard(ctx: ExtensionContext, cli: StreamCli, state: CardState, actions: {receipt(value: Receipt): void; discuss(): Promise<void>; settings(): Promise<void>; report(): void}): Promise<void> {
 for (;;) {
  const catalog = await cli.context(ctx.sessionManager.getSessionId(), process.env.STREAMS_STREAM_ID);
  if (!catalog.stream) {ctx.ui.notify("No attached work card. Use select_stream or create_stream for initial attachment.", "info");return;}
  const action = await ctx.ui.select(cardSummary(catalog), ["Whole effort — edit fields", ...(catalog.session ? ["This session — edit fields", "Finish this session"] : []), "Finish whole effort", catalog.stream.archived ? "Restore effort" : "Archive effort", "Discuss live card", "Report now", "Reporting settings", "Close"]);
  if (!action || action === "Close") return;
  if (action === "Discuss live card") {await actions.discuss();continue;}
  if (action === "Reporting settings") {await actions.settings();continue;}
  if (action === "Report now") {actions.report();continue;}
  let kind: "stream" | "session" = action.startsWith("This session") || action === "Finish this session" ? "session" : "stream";
  if (action.startsWith("Finish")) {
   if (!await ctx.ui.confirm(action, kind === "session" ? "Finish only this contribution; the effort remains unchanged?" : "Mark the entire effort done? Other contributions are not automatically finished.")) continue;
   const edit = new FieldEdit(kind, catalog, "status");edit.value = "done";
   try {await edit.save(cli, actions.receipt);} catch(error) {state.edits.set(`${kind}:status`, edit);ctx.ui.notify(String(error), "error");}
  } else if (action === "Archive effort" || action === "Restore effort") {
   if (!await ctx.ui.confirm(action, "Change the whole effort archive flag?")) continue;
   const edit = new FieldEdit("stream", catalog, "archived");edit.value = !catalog.stream.archived;
   try {await edit.save(cli, actions.receipt);} catch(error) {state.edits.set("stream:archived", edit);ctx.ui.notify(String(error), "error");}
  } else {
   const options = fields[kind].filter((field) => field !== "archived" && !(kind === "stream" && field === "goal_short" && catalog.stream!.goal === null));
   const field = await ctx.ui.select(`${kind === "stream" ? "Whole effort" : "This session"} · IDs, owner and membership are read-only`, options.map((field) => `${field}: ${JSON.stringify((catalog[kind] as any)[field])}${state.edits.has(`${kind}:${field}`) ? " · retained input" : ""}`));
   if (field) await editField(ctx, cli, catalog, kind, field.split(":")[0], state, actions.receipt);
  }
 }
}
