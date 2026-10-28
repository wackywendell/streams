import { Editor, SelectList, type SelectItem, matchesKey, truncateToWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { DynamicBorder, getSelectListTheme } from "@earendil-works/pi-coding-agent";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { LocalContext, Receipt, StreamCli } from "./streams.ts";
import { FieldEdit, fields, statuses, titleModes, validateReferences } from "./core.ts";
export type DiscussionState = {input: string; messages: {role: "user" | "assistant"; text: string}[]; error?: string};
export type CardState = {discussion: DiscussionState; edits: Map<string, FieldEdit>};
/** A small composer built around Pi's editor; unsent input survives close/reopen. */
export async function compose(ctx: ExtensionContext, state: DiscussionState): Promise<string | undefined> {
 return ctx.ui.custom<string | undefined>((tui, theme, _keys, done) => {
  const editor = new Editor(tui, {borderColor: (text) => theme.fg("border", text), selectList: {selectedPrefix: (text) => theme.fg("accent", text), selectedText: (text) => theme.fg("accent", text), description: (text) => theme.fg("muted", text), scrollInfo: (text) => theme.fg("dim", text), noMatch: (text) => theme.fg("warning", text)}});
  editor.setText(state.input);editor.onChange = (text) => {state.input = text;};
  editor.onSubmit = (text) => {if (text.trim()) {state.input = text;done(text);}};
  return {
   get focused() {return editor.focused;}, set focused(value: boolean) {editor.focused = value;},
   invalidate() {editor.invalidate();},
   handleInput(data: string) {if (matchesKey(data, "escape")) {state.input = editor.getExpandedText();done(undefined);} else editor.handleInput(data);},
   render(width: number) {
    const clean = (text: string) => text.replace(/[\x00-\x08\x0b-\x1f\x7f]/g, "");
    const history = state.messages.flatMap((message) => wrapTextWithAnsi(`${message.role}: ${clean(message.text)}`, width));
    return [truncateToWidth("Card discussion · Enter send · Shift+Enter newline · Esc back", width), ...history.slice(-Math.max(2, tui.terminal.rows - 12)), ...(state.error ? wrapTextWithAnsi(clean(state.error), width) : []), ...editor.render(width)];
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
   const ref = await ctx.ui.editor("Recovery branch/bookmark (optional; does not pin opening)", row.ref ?? "");if (ref === undefined) continue;if (ref.trim()) row.ref = ref;else delete row.ref;
   const path = await ctx.ui.editor("Portable relative document path", row.path ?? "");if (path === undefined) continue;row.path = path;
   const commitVal = await ctx.ui.editor("Commit SHA (optional recovery context)", row.commit ?? "");if (commitVal === undefined) continue;
   if (commitVal.trim()) row.commit = commitVal;else delete row.commit;
   if (!row.source_session_id && edit.base.session?.id) row.source_session_id = edit.base.session.id;
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
     const value = await ctx.ui.select(`${kind} ${field} · current: ${edit.value}${edit.error ? `\n${edit.error}` : ""}`, field === "status" ? statuses : field === "title_mode" ? [...titleModes] : ["automatic", "manual"]);if (value === undefined) return;edit.value = value;
    } else if (field === "archived") edit.value = !edit.base.stream!.archived;
    else {
     const value = await ctx.ui.editor(`${kind} ${field}${edit.error ? `\n${edit.error}` : ""}`, String(edit.value ?? ""));
     if (value === undefined) return;edit.value = ["goal", "goal_short", "recap_short", "state_short", "remaining_short"].includes(field) && !value.trim() ? null : value;
    }
    editValue = false;
   }
   const canRebase = Boolean(edit.error && JSON.stringify(edit.base[kind]) !== JSON.stringify(catalog[kind]));
   const action = await ctx.ui.select(`${edit.error ?? ""}\n${kind} ${field}: ${JSON.stringify(edit.value)}\nOnly this field will be submitted.`, [...(canRebase ? ["Rebase on live field & save"] : []), "Save field", "Edit again", "Keep input and close", "Discard input"]);
   if (action === "Discard input") {state.edits.delete(key);return;}
   if (!action || action === "Keep input and close") return;
   if (action === "Edit again") {editValue = true;continue;}
   if (action === "Rebase on live field & save") edit.rebase(catalog);
   try {await edit.save(cli, receipt);state.edits.delete(key);return;} catch(error) {ctx.ui.notify(String(error), "error");}
  }
 } finally {if (!edit.dirty && !edit.error) state.edits.delete(key);}
}
function formatFieldPreview(value: unknown): string {
 if (value === null || value === undefined || value === "") return "—";
 if (Array.isArray(value)) return `${value.length} item${value.length === 1 ? "" : "s"}`;
 return String(value).replace(/\s+/g, " ").trim();
}
export async function openCard(ctx: ExtensionContext, cli: StreamCli, state: CardState, actions: {receipt(value: Receipt): void; discuss(): Promise<void>; settings(): Promise<void>; report(): void; reset?(auto: boolean, target?: "all" | "session" | "stream"): void}): Promise<void> {
 let activeTab: "actions" | "stream" | "session" = "actions";
 const selectedByTab: Record<"actions" | "stream" | "session", number> = {actions: 0, stream: 0, session: 0};
 for (;;) {
  const catalog = await cli.context(ctx.sessionManager.getSessionId());
  if (!catalog.stream) {ctx.ui.notify("No attached work card. Use select_stream or create_stream for initial attachment.", "info");return;}
  const stream = catalog.stream, session = catalog.session;
  const actionItems: SelectItem[] = [
   {value: "report", label: "Report", description: "Summarize recent conversation turns into the work card"},
   {value: "discuss", label: "Discuss…", description: "Chat with the reporting helper to refine fields"},
   {value: "reset:all", label: "Reset Stream and Session…", description: "Choose how to regenerate both records' automatic fields"},
   {value: "settings", label: "Settings…", description: "Configure automatic reporting cadence, model, and naming"},
   {value: "close", label: "Close", description: "Close this dialog (Esc)"},
  ];
  const streamFields = fields.stream.filter((f) => f !== "archived" && !(f === "goal_short" && stream.goal === null));
  const streamItems: SelectItem[] = streamFields.map((field) => ({
   value: `field:stream:${field}`,
   label: `${field}…${state.edits.has(`stream:${field}`) ? " *" : ""}`,
   description: `${formatFieldPreview((stream as any)[field])}${state.edits.has(`stream:${field}`) ? " · retained unsaved edit" : ""}`,
  }));
  streamItems.push(
   {value: "reset:stream", label: "Reset…", description: "Regenerate only the Stream's automatic fields"},
   {value: "finish:stream", label: "Finish…", description: "Mark the Stream as done; Sessions remain unchanged"},
   {value: "archive", label: `${stream.archived ? "Restore…" : "Archive…"}${state.edits.has("stream:archived") ? " *" : ""}`, description: state.edits.has("stream:archived") ? "Retained unsaved archive edit" : stream.archived ? "Unarchive this Stream" : "Hide this Stream from default active lists"},
  );
  const sessionItems: SelectItem[] = session ? fields.session.map((field) => ({
   value: `field:session:${field}`,
   label: `${field}…${state.edits.has(`session:${field}`) ? " *" : ""}`,
   description: `${formatFieldPreview((session as any)[field])}${state.edits.has(`session:${field}`) ? " · retained unsaved edit" : ""}`,
  })) : [{value: "noop", label: "Not yet enrolled", description: "Run /streams attach or /streams report to enroll this session"}];
  if (session) sessionItems.push(
   {value: "reset:session", label: "Reset…", description: "Regenerate only this Session's automatic fields"},
   {value: "finish:session", label: "Finish…", description: "Mark this Session as done; the Stream remains unchanged"},
  );
  const fallbackItems = [...actionItems, ...streamItems.map(item => ({...item, label: `Stream · ${item.label}`})), ...sessionItems.map(item => ({...item, label: `Session · ${item.label}`}))];
  let action: string | undefined;
  if (ctx.ui.custom) action = await ctx.ui.custom<string | undefined>((tui, theme, _keys, done) => {
   const topBorder = new DynamicBorder((text) => theme.fg("border", text));
   const bottomBorder = new DynamicBorder((text) => theme.fg("border", text));
   const tabOrder: ("actions" | "stream" | "session")[] = ["actions", "stream", "session"];
   const itemsForTab = (tab: "actions" | "stream" | "session") => tab === "actions" ? actionItems : tab === "stream" ? streamItems : sessionItems;
   let list: SelectList;
   const buildList = () => {
    const items = itemsForTab(activeTab);
    list = new SelectList(items, Math.max(4, Math.min(12, tui.terminal.rows - 14)), getSelectListTheme());
    list.setSelectedIndex(Math.min(selectedByTab[activeTab], Math.max(0, items.length - 1)));
    list.onSelectionChange = (item) => {selectedByTab[activeTab] = Math.max(0, items.findIndex((i) => i.value === item.value));};
    list.onSelect = (item) => done(item.value);
    list.onCancel = () => done(undefined);
   };
   buildList();
   return {
    invalidate() {topBorder.invalidate();bottomBorder.invalidate();list.invalidate();},
    handleInput(data: string) {
     if (matchesKey(data, "tab") || matchesKey(data, "right") || data === "1" || data === "2" || data === "3" || matchesKey(data, "shift+tab") || matchesKey(data, "left")) {
      if (data === "1") activeTab = "actions";
      else if (data === "2") activeTab = "stream";
      else if (data === "3") activeTab = "session";
      else {
       const delta = matchesKey(data, "shift+tab") || matchesKey(data, "left") ? -1 : 1;
       activeTab = tabOrder[(tabOrder.indexOf(activeTab) + delta + tabOrder.length) % tabOrder.length];
      }
      buildList();tui.requestRender();return;
     }
     list.handleInput(data);tui.requestRender();
    },
    render(width: number) {
     const clean = (text: string) => text.replace(/[\x00-\x1f\x7f]/g, " ");
     const section = (text: string) => theme.fg("accent", theme.bold(text));
     const line = (text: string, limit = 2) => wrapTextWithAnsi(clean(text), Math.max(10, width - 2)).slice(0, limit).map((l) => ` ${l}`);
     const tabLabel = (id: "actions" | "stream" | "session", title: string) =>
      activeTab === id ? theme.fg("accent", theme.bold(`[ ${title} ]`)) : theme.fg("muted", title);
     const tabBar = truncateToWidth(
      ` ${tabLabel("actions", "1: Actions")} ${tabLabel("stream", "2: Stream")} ${tabLabel("session", "3: Session")}   ${theme.fg("dim", "Tab/←→/1-3 switch · Esc close")}`,
      width,
     );
     const summary = [
      ` ${section("Stream")} · ${clean(stream.title)} ${theme.fg("muted", `[${stream.status}${stream.archived ? " · archived" : ""} · title:${stream.title_mode} · goal:${stream.goal_mode}]`)}`,
      ...line(`Goal: ${stream.goal ?? "—"}`, 2),
      ` ${section("Session")} · ${clean(session?.title ?? "Not yet enrolled")} ${session ? theme.fg("muted", `[${session.status} · title:${session.title_mode}]`) : ""}`,
      ...(session ? [
       ...line(`Session: ${session.recap || "—"}`, 2),
       ...line(`Status: ${session.state || "—"}`, 2),
       ...line(`Remaining: ${session.remaining || "—"}`, 1),
      ] : []),
     ].map((l) => truncateToWidth(l, width));
     const controls = list.render(width);
     const budget = Math.max(2, tui.terminal.rows - controls.length - 5);
     return [
      ...topBorder.render(width),
      tabBar,
      "",
      ...summary.slice(0, budget),
      "",
      ...controls,
      ...bottomBorder.render(width),
     ];
    },
   };
  });
  else {
   const selected = await ctx.ui.select("Work card", fallbackItems.map(item => item.label));
   action = fallbackItems.find(item => item.label === selected)?.value;
  }
  if (!action || action === "close") return;
  if (action === "noop") continue;
  if (action.startsWith("field:")) {
   const [, kind, field] = action.split(":") as [string, "stream" | "session", string];
   await editField(ctx, cli, catalog, kind, field, state, actions.receipt);
   continue;
  }
  if (action === "discuss") {await actions.discuss();continue;}
  if (action.startsWith("reset:")) {
   const target = action.split(":")[1] as "all" | "stream" | "session";
   const mode = await ctx.ui.select(`Reset ${target === "all" ? "Stream and Session" : target === "stream" ? "Stream" : "Session"} · regenerate automatic fields`, ["Preserve naming modes", "Restore automatic naming"]);
   if (!mode) continue;
   actions.reset?.(mode === "Restore automatic naming", target);return;
  }
  if (action === "settings") {await actions.settings();continue;}
  if (action === "report") {actions.report();return;}
  if (action.startsWith("finish:")) {
   const kind = action.split(":")[1] as "stream" | "session";
   if (!await ctx.ui.confirm(kind === "session" ? "Finish Session?" : "Finish Stream?", kind === "session" ? "Finish only this Session; the Stream remains unchanged?" : "Mark the Stream done? Other Sessions are not automatically finished.")) continue;
   const edit = new FieldEdit(kind, catalog, "status");edit.value = "done";
   try {await edit.save(cli, actions.receipt);} catch(error) {state.edits.set(`${kind}:status`, edit);ctx.ui.notify(String(error), "error");}
  } else if (action === "archive") {
   if (state.edits.has("stream:archived")) {await editField(ctx, cli, catalog, "stream", "archived", state, actions.receipt);continue;}
   if (!await ctx.ui.confirm(stream.archived ? "Restore Stream?" : "Archive Stream?", stream.archived ? "Show this Stream in default active lists?" : "Hide this Stream from default active lists?")) continue;
   const edit = new FieldEdit("stream", catalog, "archived");edit.value = !stream.archived;
   try {await edit.save(cli, actions.receipt);} catch(error) {state.edits.set("stream:archived", edit);ctx.ui.notify(String(error), "error");}
  }
 }
}
