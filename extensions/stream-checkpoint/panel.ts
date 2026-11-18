import { Editor, SelectList, type SelectItem, matchesKey, truncateToWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { DynamicBorder, getSelectListTheme } from "@earendil-works/pi-coding-agent";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { referenceText, safeReferenceJson, type LocalContext, type Receipt, type Reference, type ReferenceValue, type ResourceKey, type StreamCli } from "./streams.ts";
import { FieldEdit, ReferenceEdit, fields, isAutomaticReferenceValue, materialiseReference, referenceSelector, statuses, titleModes } from "./core.ts";
import { isReferenceValue, resourceKey } from "../../integrations/herdr-work-cards/src/streams-context.ts";
export type DiscussionState = {input: string; messages: {role: "user" | "assistant"; text: string}[]; error?: string};
export type CardState = {discussion: DiscussionState; edits: Map<string, FieldEdit>; referenceEdits?: Map<string, ReferenceEdit>};
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
const referenceLabels: Record<string, string> = {document:"Document", pr:"PR", issue:"Issue", link:"Link", deliverable:"Deliverable", dependency:"Dependency", brief:"Brief", background:"Background", active:"Active", fulfilled:"Fulfilled", dropped:"Dropped"};
const referenceByKey = (catalog: LocalContext, key: ResourceKey) => catalog.stream?.references.find(row => referenceSelector(resourceKey(row.target)) === referenceSelector(key));
const referenceKeyJson = (key: ResourceKey) => safeReferenceJson(key.kind === "url" ? {kind:key.kind, url:key.url} : {kind:key.kind, repository_url:key.repository_url, path:key.path});
const referenceValue = (reference: Reference): ReferenceValue => structuredClone(reference);

async function selectReference(ctx: ExtensionContext, items: SelectItem[]): Promise<string | undefined> {
 if (!ctx.ui.custom) {
  const choices = items.map(item => item.description ? `${item.label} · ${item.description}` : item.label);
  const selected = await ctx.ui.select("References", choices);
  return items[choices.indexOf(selected ?? "")]?.value;
 }
 return ctx.ui.custom<string | undefined>((tui, theme, _keys, done) => {
  const list = new SelectList(items, Math.max(4, Math.min(12, tui.terminal.rows - 6)), getSelectListTheme());
  list.onSelect = item => done(item.value);list.onCancel = () => done(undefined);
  return {invalidate:() => list.invalidate(), handleInput:(data: string) => {list.handleInput(data);tui.requestRender();}, render:(width: number) => {
   const selected = list.getSelectedItem();
   return [truncateToWidth("References · Esc back", width), ...list.render(width), ...(selected?.description ? wrapTextWithAnsi(`${selected.label} · ${selected.description}`, width) : [])];
  }};
 });
}
function referenceRows(catalog: LocalContext, edits: Map<string, ReferenceEdit>): SelectItem[] {
 const rows: {key: ResourceKey; value: Reference | ReferenceValue; edit?: ReferenceEdit; absent: boolean}[] = [];
 for (const reference of catalog.stream!.references) {
  const key = resourceKey(reference.target), edit = edits.get(referenceSelector(key));
  rows.push({key, value:reference, edit, absent:false});
 }
 for (const [selector, edit] of edits) {
  if (selector === "new" || !edit.key || rows.some(row => referenceSelector(row.key) === selector)) continue;
  const source = referenceByKey(edit.base, edit.key);
  const value = source ?? (edit.desired && typeof edit.desired === "object" ? edit.desired as ReferenceValue : undefined);
  if (value) rows.push({key:edit.key, value, edit, absent:true});
 }
 return rows.map(({key, value, edit, absent}, i) => {
  const retained = edit ? edit.desired === null ? "Retained deletion" : referenceByKey(edit.base, key) ? "Retained edit" : "Retained addition" : undefined;
  return {value:referenceSelector(key), label:`${i + 1}. ${referenceText(value.name)} · ${referenceLabels[value.target.kind]} · ${referenceLabels[value.role]} · ${referenceLabels[value.state]}`, description:[referenceKeyJson(key), retained, absent ? "Source absent" : undefined].filter(Boolean).join(" · ")};
 });
}
async function saveReference(ctx: ExtensionContext, cli: StreamCli, edits: Map<string, ReferenceEdit>, selector: string, edit: ReferenceEdit, receipt: (value: Receipt) => void): Promise<boolean> {
 try {await edit.save(cli, receipt);edits.delete(selector);return true;}
 catch (error) {ctx.ui.notify(referenceText(error instanceof Error ? error.message : String(error)), "error");return false;}
}
async function editReferenceForm(ctx: ExtensionContext, cli: StreamCli, edits: Map<string, ReferenceEdit>, selector: string, edit: ReferenceEdit, receipt: (value: Receipt) => void): Promise<void> {
 if (edit.actor === "automatic" && isAutomaticReferenceValue(edit.desired)) {
  edit.desired = materialiseReference(edit.desired, edit.key ? referenceByKey(edit.base, edit.key) : undefined, edit.base.session);edit.actor = "user";
 }
 for (;;) {
  const value = edit.desired as any;
  const target = value?.target ?? {}, captured = edit.key ? referenceByKey(edit.base, edit.key) : undefined;
  const fields = ["Name…", "Purpose…", "Role…", ...(value?.state === "dropped" || captured?.state === "dropped" ? [] : ["State…"]), "Kind…", ...(target.kind === "document" ? ["Repository URL…", "Path…", "Ref…", "Commit…", "Source session…"] : target.kind ? ["URL…"] : [])];
  const action = await ctx.ui.select(`Reference input${edit.error ? ` · ${referenceText(edit.error)}` : ""}\n${safeReferenceJson(edit.desired)}`, [...fields, "Save reference", "Keep input and close", "Discard input"]);
  if (action === "Discard input") {edits.delete(selector);return;}
  if (!action || action === "Keep input and close") {if (!edit.dirty && !edit.error) edits.delete(selector);return;}
  if (action === "Save reference") {
   if (captured?.state === "dropped" && isReferenceValue(value) && value.state !== "dropped") {
    const choice = await ctx.ui.select("This reference is Dropped. Choose how to save.", ["Keep Dropped", "Restore Active", "Restore Fulfilled", "Cancel"]);
    if (!choice || choice === "Cancel") continue;
    value.state = choice === "Restore Active" ? "active" : choice === "Restore Fulfilled" ? "fulfilled" : "dropped";
   }
   if (await saveReference(ctx, cli, edits, selector, edit, receipt)) return;continue;
  }
  if (action === "Role…") {
   const choice = await ctx.ui.select("Role", ["Deliverable", "Dependency", "Brief", "Background"]);
   if (choice) value.role = choice.toLowerCase();
  } else if (action === "State…") {
   const choice = await ctx.ui.select("State", ["Active", "Fulfilled"]);if (choice) value.state = choice.toLowerCase();
  } else if (action === "Kind…") {
   const choice = await ctx.ui.select("Kind", ["Document", "PR", "Issue", "Link"]);
   if (choice) {
    const kind = choice === "PR" ? "pr" : choice.toLowerCase();
    if (kind === "document") value.target = target.kind === "document" ? target : {kind, repository_url:"", path:""};
    else value.target = {kind, url:target.kind && target.kind !== "document" ? target.url : ""};
   }
  } else {
   const property = ({"Name…":"name", "Purpose…":"purpose", "Repository URL…":"repository_url", "Path…":"path", "URL…":"url", "Ref…":"ref", "Commit…":"commit", "Source session…":"source_session_id"} as Record<string,string>)[action];
   if (!property) continue;
   const owner = property === "name" || property === "purpose" ? value : target;
   const result = await ctx.ui.editor(action.slice(0,-1), owner[property] ?? "");
   if (result !== undefined) {if (["ref", "commit", "source_session_id"].includes(property) && result === "") delete owner[property];else owner[property] = result;}
  }
  edit.error = undefined;
 }
}
async function confirmDeletion(ctx: ExtensionContext, name: string): Promise<boolean> {
 return ctx.ui.confirm(`Forget ${referenceText(name)}?`, `Forget ${referenceText(name)}? Reporting may add it again, including from a report already underway.`);
}
async function manageReferences(ctx: ExtensionContext, cli: StreamCli, state: CardState, receipt: (value: Receipt) => void): Promise<void> {
 const edits = state.referenceEdits ??= new Map();
 for (;;) {
  const catalog = await cli.context(ctx.sessionManager.getSessionId());if (!catalog.stream) return;
  const rows = referenceRows(catalog, edits);
  const items = [...rows, {value:"add", label:"Add reference…", description:edits.has("new") ? "Retained input" : ""}, {value:"close", label:"Close"}];
  const selector = await selectReference(ctx, items);
  if (!selector || selector === "close") return;
  if (selector === "add") {
   let edit = edits.get("new");
   if (!edit) {edit = new ReferenceEdit(catalog, null, "user", {name:"", purpose:"", role:"", state:"active", target:{}});edits.set("new", edit);}
   await editReferenceForm(ctx, cli, edits, "new", edit, receipt);continue;
  }
  const saved = catalog.stream.references.find(row => referenceSelector(resourceKey(row.target)) === selector);
  let edit = edits.get(selector);
  const key = edit?.key ?? (saved ? resourceKey(saved.target) : undefined);if (!key) continue;
  const heading = `Reference ${referenceKeyJson(key)}`;
  if (edit) {
   const actions = ["Inspect retained input", "Inspect current row", ...(edit.desired === null ? [] : ["Edit retained input"]), ...(saved ? ["Reapply against current row"] : []), "Keep input and close", "Discard"];
   const action = await ctx.ui.select(heading, actions);
   if (action === "Discard") {edits.delete(selector);continue;}
   if (!action || action === "Keep input and close") return;
   if (action === "Inspect retained input") {await ctx.ui.select(`${heading}\nRetained input: ${safeReferenceJson(edit.desired)}`, ["Back"]);continue;}
   if (action === "Inspect current row") {await ctx.ui.select(`${heading}\nCurrent: ${saved ? safeReferenceJson(saved) : "Source absent"}`, ["Back"]);continue;}
   if (action === "Edit retained input") {await editReferenceForm(ctx, cli, edits, selector, edit, receipt);continue;}
   if (action === "Reapply against current row" && saved) {
    const fresh = await cli.context(ctx.sessionManager.getSessionId()), current = referenceByKey(fresh, key);
    if (!current) {await ctx.ui.select(`${heading}\nCurrent: Source absent`, ["Back"]);continue;}
    const confirm = await ctx.ui.select(`${heading}\nCurrent: ${safeReferenceJson(current)}\nRetained input: ${safeReferenceJson(edit.desired)}`, ["Reapply", "Cancel"]);
    if (confirm !== "Reapply") continue;
    let desired = edit.desired;
    if (desired === null) {if (!await confirmDeletion(ctx, current.name)) continue;}
    else {
     if (edit.actor === "automatic" && isAutomaticReferenceValue(desired)) desired = materialiseReference(desired, current, fresh.session);
     if (!isReferenceValue(desired)) {edit.error = "Retained input is invalid; edit it before reapplying.";continue;}
     desired = structuredClone(desired);
     if (current.state === "dropped" && desired.state !== "dropped") {
      const choice = await ctx.ui.select("This reference is Dropped. Choose how to reapply.", ["Keep Dropped", "Restore Active", "Restore Fulfilled", "Cancel"]);
      if (!choice || choice === "Cancel") continue;
      desired.state = choice === "Restore Active" ? "active" : choice === "Restore Fulfilled" ? "fulfilled" : "dropped";
     }
    }
    edit.rebase(fresh);edit.actor = "user";edit.desired = desired;
    await saveReference(ctx, cli, edits, selector, edit, receipt);
   }
   continue;
  }
  if (!saved) continue;
  const action = await ctx.ui.select(heading, ["Edit", ...(saved.state === "dropped" ? ["Restore"] : ["Drop"]), "Delete", "Back"]);
  if (!action || action === "Back") continue;
  const desired = referenceValue(saved);
  if (action === "Drop") desired.state = "dropped";
  else if (action === "Restore") {const choice = await ctx.ui.select("Restore reference", ["Active", "Fulfilled"]);if (!choice) continue;desired.state = choice.toLowerCase() as "active" | "fulfilled";}
  else if (action === "Delete" && !await confirmDeletion(ctx, saved.name)) continue;
  edit = new ReferenceEdit(catalog, key, "user", action === "Delete" ? null : desired);edits.set(selector, edit);
  if (action === "Edit") await editReferenceForm(ctx, cli, edits, selector, edit, receipt);
  else await saveReference(ctx, cli, edits, selector, edit, receipt);
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
    if (field === "status" || field.endsWith("_mode")) {
     const value = await ctx.ui.select(`${kind} ${field} · current: ${edit.value}${edit.error ? `\n${edit.error}` : ""}`, field === "status" ? statuses : field === "title_mode" ? [...titleModes] : ["automatic", "manual"]);if (value === undefined) return;edit.value = value;
    } else {
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
  const streamFields = fields.stream.filter((f) => !(f === "goal_short" && stream.goal === null));
  const streamItems: SelectItem[] = streamFields.map((field) => ({
   value: `field:stream:${field}`,
   label: `${field}…${state.edits.has(`stream:${field}`) ? " *" : ""}`,
   description: `${formatFieldPreview((stream as any)[field])}${state.edits.has(`stream:${field}`) ? " · retained unsaved edit" : ""}`,
  }));
  streamItems.push(
   {value: "references", label: "References…", description: "Manage saved references, including Dropped entries"},
   {value: "reset:stream", label: "Reset…", description: "Regenerate only the Stream's automatic fields"},
   {value: "finish:stream", label: "Finish…", description: "Mark the Stream as done; Sessions remain unchanged"},
   {value: "close:stream", label: stream.status === "closed" ? "Restore…" : "Close…", description: stream.status === "closed" ? "Reopen this closed Stream as active" : "Close this Stream as abandoned"},
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
      ` ${section("Stream")} · ${clean(stream.title)} ${theme.fg("muted", `[${stream.status} · title:${stream.title_mode} · goal:${stream.goal_mode}]`)}`,
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
  if (action === "references") {await manageReferences(ctx, cli, state, actions.receipt);continue;}
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
  } else if (action === "close:stream") {
   const restoring = stream.status === "closed";
   if (!await ctx.ui.confirm(restoring ? "Restore Stream?" : "Close Stream?", restoring ? "Reopen this closed Stream as active?" : "Mark this Stream closed (abandoned)?")) continue;
   const edit = new FieldEdit("stream", catalog, "status");edit.value = restoring ? "active" : "closed";
   try {await edit.save(cli, actions.receipt);} catch(error) {state.edits.set("stream:status", edit);ctx.ui.notify(String(error), "error");}
  }
 }
}
