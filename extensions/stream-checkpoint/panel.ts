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
   {value: "Report now", label: "Report now", description: "Summarize recent conversation turns into the work card"},
   {value: "Reset generated fields", label: "Reset generated fields", description: "Regenerate both session and effort fields"},
   {value: "Re-enable automatic naming & reset", label: "Re-enable automatic naming & reset", description: "Make title and goal generation automatic, then regenerate"},
   {value: "Discuss live card", label: "Discuss live card", description: "Chat with the reporting helper to refine fields"},
   ...(session ? [
    {value: "Reset this session only", label: "Reset this session only", description: "Regenerate only this tab's title, recap, status, and remaining work"},
    {value: "Re-enable session auto-naming & reset", label: "Re-enable session auto-naming & reset", description: "Make this session's title generation automatic, then regenerate"},
   ] : []),
   {value: "Reset whole effort only", label: "Reset whole effort only", description: "Regenerate only the shared effort title and goal"},
   {value: "Re-enable effort auto-naming & reset", label: "Re-enable effort auto-naming & reset", description: "Make effort title and goal generation automatic, then regenerate"},
   {value: "Whole effort — edit fields", label: "Whole effort — edit fields", description: "Switch to the Stream tab to edit shared effort fields"},
   ...(session ? [
    {value: "This session — edit fields", label: "This session — edit fields", description: "Switch to the Session tab to edit this tab's fields"},
    {value: "Finish this session", label: "Finish this session", description: "Mark this tab's contribution status as done"},
   ] : []),
   {value: "Finish whole effort", label: "Finish whole effort", description: "Mark the shared effort status as done"},
   {value: stream.archived ? "Restore effort" : "Archive effort", label: stream.archived ? "Restore effort" : "Archive effort", description: stream.archived ? "Unarchive this effort" : "Hide this effort from default active lists"},
   {value: "Reporting & Herdr settings", label: "Reporting & Herdr settings", description: "Configure automatic reporting cadence, model, and naming"},
   {value: "Close", label: "Close", description: "Close this dialog (Esc)"},
  ];
  const streamFields = fields.stream.filter((f) => !(f === "goal_short" && stream.goal === null));
  const streamItems: SelectItem[] = streamFields.map((field) => ({
   value: `field:stream:${field}`,
   label: `${field}${state.edits.has(`stream:${field}`) ? " *" : ""}`,
   description: `${formatFieldPreview((stream as any)[field])}${state.edits.has(`stream:${field}`) ? " · retained unsaved edit" : ""}`,
  }));
  const sessionItems: SelectItem[] = session ? fields.session.map((field) => ({
   value: `field:session:${field}`,
   label: `${field}${state.edits.has(`session:${field}`) ? " *" : ""}`,
   description: `${formatFieldPreview((session as any)[field])}${state.edits.has(`session:${field}`) ? " · retained unsaved edit" : ""}`,
  })) : [{value: "noop", label: "Not yet enrolled", description: "Run /streams attach or /streams report to enroll this session"}];
  const rows = actionItems.map((item) => item.value);
  const action = ctx.ui.custom ? await ctx.ui.custom<string | undefined>((tui, theme, _keys, done) => {
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
    list.onSelect = (item) => {
     if (item.value === "Whole effort — edit fields") {activeTab = "stream";buildList();tui.requestRender();return;}
     if (item.value === "This session — edit fields") {activeTab = "session";buildList();tui.requestRender();return;}
     done(item.value);
    };
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
      activeTab === id ? theme.fg("accent", theme.bold(`[ ${title} ]`)) : theme.fg("muted", `  ${title}  `);
     const tabBar = truncateToWidth(
      ` ${tabLabel("actions", "1: Actions")} ${tabLabel("stream", "2: Stream (Whole Effort)")} ${tabLabel("session", "3: Session (This Tab)")}   ${theme.fg("dim", "Tab/←→/1-3 switch · Esc close")}`,
      width,
     );
     const summary = [
      ` ${section("Whole effort")} · ${clean(stream.title)} ${theme.fg("muted", `[${stream.status}${stream.archived ? " · archived" : ""} · title:${stream.title_mode} · goal:${stream.goal_mode}]`)}`,
      ...line(`Goal: ${stream.goal ?? "—"}`, 2),
      ` ${section("This session")} · ${clean(session?.title ?? "Not yet enrolled")} ${session ? theme.fg("muted", `[${session.status} · title:${session.title_mode}]`) : ""}`,
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
  }) : await ctx.ui.select("Work card", rows);
  if (!action || action === "Close") return;
  if (action === "noop") continue;
  if (action.startsWith("field:")) {
   const [, kind, field] = action.split(":") as [string, "stream" | "session", string];
   await editField(ctx, cli, catalog, kind, field, state, actions.receipt);
   continue;
  }
  if (action === "Discuss live card") {await actions.discuss();continue;}
  if (action === "Reset generated fields" || action === "Re-enable automatic naming & reset") {actions.reset?.(action === "Re-enable automatic naming & reset", "all");return;}
  if (action === "Reset this session only" || action === "Re-enable session auto-naming & reset") {actions.reset?.(action === "Re-enable session auto-naming & reset", "session");return;}
  if (action === "Reset whole effort only" || action === "Re-enable effort auto-naming & reset") {actions.reset?.(action === "Re-enable effort auto-naming & reset", "stream");return;}
  if (action === "Reporting & Herdr settings") {await actions.settings();continue;}
  if (action === "Report now") {actions.report();return;}
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
