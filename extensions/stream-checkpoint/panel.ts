import {
  Editor,
  SelectList,
  type SelectItem,
  matchesKey,
  truncateToWidth,
  wrapTextWithAnsi,
} from "@earendil-works/pi-tui";
import { DynamicBorder, getSelectListTheme } from "@earendil-works/pi-coding-agent";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { FieldEdit } from "./core.ts";
import {
  referenceText,
  safeReferenceJson,
  sessionKeyOf,
  type EditResult,
  type LocalContext,
  type ReferenceEntry,
  type ReferenceKind,
  type ReferenceRole,
  type ReferenceStatus,
  type SessionWire,
  type StreamCli,
} from "./streams.ts";
import { normalizeDocumentKey, referenceUrl } from "./wire.ts";

export type DiscussionState = {
  input: string;
  messages: { role: "user" | "assistant"; text: string }[];
  error?: string;
};

export type CardState = {
  discussion: DiscussionState;
  edits: Map<string, FieldEdit>;
};

/** A small composer built around Pi's editor; unsent input survives close/reopen. */
export async function compose(
  ctx: ExtensionContext,
  state: DiscussionState
): Promise<string | undefined> {
  return ctx.ui.custom<string | undefined>((tui, theme, _keys, done) => {
    const editor = new Editor(tui, {
      borderColor: (text) => theme.fg("border", text),
      selectList: {
        selectedPrefix: (text) => theme.fg("accent", text),
        selectedText: (text) => theme.fg("accent", text),
        description: (text) => theme.fg("muted", text),
        scrollInfo: (text) => theme.fg("dim", text),
        noMatch: (text) => theme.fg("warning", text),
      },
    });
    editor.setText(state.input);
    editor.onChange = (text) => {
      state.input = text;
    };
    editor.onSubmit = (text) => {
      if (text.trim()) {
        state.input = text;
        done(text);
      }
    };
    return {
      get focused() {
        return editor.focused;
      },
      set focused(value: boolean) {
        editor.focused = value;
      },
      invalidate() {
        editor.invalidate();
      },
      handleInput(data: string) {
        if (matchesKey(data, "escape")) {
          state.input = editor.getExpandedText();
          done(undefined);
        } else {
          editor.handleInput(data);
        }
      },
      render(width: number) {
        const clean = (text: string) => text.replace(/[\x00-\x08\x0b-\x1f\x7f]/g, "");
        const history = state.messages.flatMap((message) =>
          wrapTextWithAnsi(`${message.role}: ${clean(message.text)}`, width)
        );
        return [
          truncateToWidth("Card discussion · Enter send · Shift+Enter newline · Esc back", width),
          ...history.slice(-Math.max(2, tui.terminal.rows - 12)),
          ...(state.error ? wrapTextWithAnsi(clean(state.error), width) : []),
          ...editor.render(width),
        ];
      },
    };
  });
}

const referenceLabels: Record<string, string> = {
  document: "Document",
  pr: "PR",
  issue: "Issue",
  link: "Link",
  deliverable: "Deliverable",
  dependency: "Dependency",
  brief: "Brief",
  background: "Background",
  active: "Active",
  fulfilled: "Fulfilled",
  dropped: "Dropped",
};

async function selectReference(
  ctx: ExtensionContext,
  items: SelectItem[]
): Promise<string | undefined> {
  if (!ctx.ui.custom) {
    const choices = items.map((item) =>
      item.description ? `${item.label} · ${item.description}` : item.label
    );
    const selected = await ctx.ui.select("References", choices);
    return items[choices.indexOf(selected ?? "")]?.value;
  }
  return ctx.ui.custom<string | undefined>((tui, _theme, _keys, done) => {
    const list = new SelectList(
      items,
      Math.max(4, Math.min(12, tui.terminal.rows - 6)),
      getSelectListTheme()
    );
    list.onSelect = (item) => done(item.value);
    list.onCancel = () => done(undefined);
    return {
      invalidate: () => list.invalidate(),
      handleInput: (data: string) => {
        list.handleInput(data);
        tui.requestRender();
      },
      render: (width: number) => {
        const selected = list.getSelectedItem();
        return [
          truncateToWidth("Session references · Esc back", width),
          ...list.render(width),
          ...(selected?.description
            ? wrapTextWithAnsi(`${selected.label} · ${selected.description}`, width)
            : []),
        ];
      },
    };
  });
}

async function saveSessionReference(
  ctx: ExtensionContext,
  cli: StreamCli,
  session: SessionWire,
  refKey: string,
  desired: ReferenceEntry,
  before: ReferenceEntry | null,
  onResult: (value: EditResult) => void
): Promise<boolean> {
  try {
    const result = await cli.editSession(sessionKeyOf(session), {
      changes: { references: { [refKey]: { ...desired, manual: true } } },
      before: { references: { [refKey]: before ? structuredClone(before) : null } },
    });
    onResult(result);
    if (
      result.not_written.references &&
      Object.hasOwn(result.not_written.references as Record<string, unknown>, refKey)
    ) {
      ctx.ui.notify(
        `Reference ${referenceText(refKey)} changed concurrently; reload and try again.`,
        "warning"
      );
      return false;
    }
    return true;
  } catch (error) {
    ctx.ui.notify(referenceText(error instanceof Error ? error.message : String(error)), "error");
    return false;
  }
}

async function editReferenceForm(
  ctx: ExtensionContext,
  cli: StreamCli,
  session: SessionWire,
  initialKey: string | null,
  initialEntry: ReferenceEntry | null,
  onResult: (value: EditResult) => void
): Promise<void> {
  let key = initialKey ?? "";
  const draft: ReferenceEntry = initialEntry
    ? structuredClone(initialEntry)
    : {
        kind: "link",
        name: "",
        purpose: "",
        role: "background",
        status: "active",
        manual: true,
      };
  draft.manual = true;

  for (;;) {
    const fields = [
      ...(initialKey === null ? ["Kind…", draft.kind === "document" ? "Path…" : "URL…"] : []),
      "Name…",
      "Purpose…",
      "Role…",
      ...(draft.status === "dropped" ? [] : ["Status…"]),
    ];
    const action = await ctx.ui.select(
      `Reference ${key ? referenceText(key) : "(new)"}\n${safeReferenceJson(draft)}`,
      [...fields, "Save reference", "Cancel"]
    );
    if (!action || action === "Cancel") return;
    if (action === "Save reference") {
      let finalKey = key.trim();
      if (draft.kind === "document") {
        const normalized = normalizeDocumentKey(session.directory?.path, finalKey);
        if (!normalized) {
          ctx.ui.notify(
            "Document path must be inside this Session's directory (no '..'); save external outputs with the artifact tool.",
            "error"
          );
          continue;
        }
        finalKey = normalized;
      } else if (!referenceUrl(finalKey)) {
        ctx.ui.notify("Enter a valid http(s) URL without credentials.", "error");
        continue;
      }
      if (!draft.name.trim() || !draft.purpose.trim()) {
        ctx.ui.notify("Name and purpose are required.", "error");
        continue;
      }
      if (await saveSessionReference(ctx, cli, session, finalKey, draft, initialEntry, onResult)) {
        return;
      }
      continue;
    }
    if (action === "Kind…") {
      const choice = await ctx.ui.select("Kind", ["Document", "PR", "Issue", "Link"]);
      if (choice) {
        draft.kind = (choice === "PR" ? "pr" : choice.toLowerCase()) as ReferenceKind;
      }
    } else if (action === "Role…") {
      const choice = await ctx.ui.select("Role", [
        "Deliverable",
        "Dependency",
        "Brief",
        "Background",
      ]);
      if (choice) draft.role = choice.toLowerCase() as ReferenceRole;
    } else if (action === "Status…") {
      const choice = await ctx.ui.select("Status", ["Active", "Fulfilled"]);
      if (choice) draft.status = choice.toLowerCase() as ReferenceStatus;
    } else if (action === "Path…" || action === "URL…") {
      const val = await ctx.ui.editor(action.slice(0, -1), key);
      if (val !== undefined) key = val.trim();
    } else if (action === "Name…") {
      const val = await ctx.ui.editor("Name", draft.name);
      if (val !== undefined) draft.name = val.trim();
    } else if (action === "Purpose…") {
      const val = await ctx.ui.editor("Purpose", draft.purpose);
      if (val !== undefined) draft.purpose = val.trim();
    }
  }
}

async function manageSessionReferences(
  ctx: ExtensionContext,
  cli: StreamCli,
  onResult: (value: EditResult) => void
): Promise<void> {
  for (;;) {
    const catalog = await cli.context(ctx.sessionManager.getSessionId());
    if (!catalog.session) return;
    const session = catalog.session;
    const entries = Object.entries(session.references).sort(([a], [b]) => a.localeCompare(b));
    const items: SelectItem[] = [
      ...entries.map(([key, entry], i) => ({
        value: `ref:${key}`,
        label: `${i + 1}. ${referenceText(entry.name)} · ${referenceLabels[entry.kind]} · ${referenceLabels[entry.role]} · ${referenceLabels[entry.status]}`,
        description: `${referenceText(key)}${entry.manual ? " · manual" : ""}`,
      })),
      { value: "add", label: "Add reference…", description: "Add a PR, issue, link, or document" },
      { value: "close", label: "Close" },
    ];
    const selected = await selectReference(ctx, items);
    if (!selected || selected === "close") return;
    if (selected === "add") {
      await editReferenceForm(ctx, cli, session, null, null, onResult);
      continue;
    }
    const key = selected.slice(4);
    const saved = session.references[key];
    if (!saved) continue;
    const heading = `Reference ${referenceText(key)}`;
    const action = await ctx.ui.select(heading, [
      "Edit",
      ...(saved.status === "dropped" ? ["Restore"] : ["Drop"]),
      "Back",
    ]);
    if (!action || action === "Back") continue;
    if (action === "Edit") {
      await editReferenceForm(ctx, cli, session, key, saved, onResult);
      continue;
    }
    const next = structuredClone(saved);
    next.manual = true;
    if (action === "Drop") {
      next.status = "dropped";
    } else if (action === "Restore") {
      const choice = await ctx.ui.select("Restore reference", ["Active", "Fulfilled"]);
      if (!choice) continue;
      next.status = choice.toLowerCase() as "active" | "fulfilled";
    }
    await saveSessionReference(ctx, cli, session, key, next, saved, onResult);
  }
}

function formatFieldPreview(value: unknown): string {
  if (value === null || value === undefined || value === "") return "—";
  if (typeof value === "object" && !Array.isArray(value)) {
    const obj = value as Record<string, unknown>;
    if (typeof obj.text === "string") return obj.text.replace(/\s+/g, " ").trim() || "—";
  }
  return String(value).replace(/\s+/g, " ").trim();
}

export async function editField(
  ctx: ExtensionContext,
  cli: StreamCli,
  catalog: LocalContext,
  kind: "stream" | "session",
  field: string,
  state: CardState,
  onResult: (value: EditResult) => void
): Promise<void> {
  const key = `${kind}:${field}`;
  let edit = state.edits.get(key);
  if (!edit) {
    edit = new FieldEdit(kind, catalog, field);
    state.edits.set(key, edit);
  } else {
    edit.refresh(catalog);
  }
  let editing = !edit.dirty && !edit.error;
  try {
    for (;;) {
      if (editing) {
        if (field === "status") {
          const choice = await ctx.ui.select(
            `session status · current: ${edit.value}${edit.error ? `\n${edit.error}` : ""}`,
            ["active", "waiting", "blocked", "parked", "done", "closed"]
          );
          if (choice === undefined) return;
          edit.value = choice;
        } else if (field === "title") {
          const currentText = (edit.value as { text?: string } | null)?.text ?? "";
          const next = await ctx.ui.editor(
            `${kind} title${edit.error ? `\n${edit.error}` : ""}`,
            currentText
          );
          if (next === undefined || !next.trim()) return;
          edit.value = { text: next.trim(), manual: true };
        } else if (field === "goal") {
          const currentText = (edit.value as { text?: string } | null)?.text ?? "";
          const next = await ctx.ui.editor(
            `stream goal (blank to clear)${edit.error ? `\n${edit.error}` : ""}`,
            currentText
          );
          if (next === undefined) return;
          edit.value = next.trim() ? { text: next.trim(), short: null, manual: true } : null;
        } else {
          // TextPair: recap, state, remaining
          const currentText = (edit.value as { text?: string } | null)?.text ?? "";
          const next = await ctx.ui.editor(
            `session ${field} (blank to clear)${edit.error ? `\n${edit.error}` : ""}`,
            currentText
          );
          if (next === undefined) return;
          edit.value = next.trim() ? { text: next.trim(), short: null } : null;
        }
        editing = false;
      }
      const canRebase = Boolean(
        edit.error &&
          JSON.stringify((edit.base[kind] as Record<string, unknown> | null)?.[field]) !==
            JSON.stringify((catalog[kind] as Record<string, unknown> | null)?.[field])
      );
      const action = await ctx.ui.select(
        `${edit.error ? `${edit.error}\n` : ""}${kind} ${field}: ${formatFieldPreview(edit.value)}`,
        [
          ...(canRebase ? ["Rebase on live field & save"] : []),
          "Save field",
          "Edit again",
          "Keep input and close",
          "Discard input",
        ]
      );
      if (action === "Discard input") {
        state.edits.delete(key);
        return;
      }
      if (!action || action === "Keep input and close") return;
      if (action === "Edit again") {
        editing = true;
        continue;
      }
      if (action === "Rebase on live field & save") edit.rebase(catalog);
      try {
        await edit.save(cli, onResult);
        state.edits.delete(key);
        return;
      } catch (error) {
        ctx.ui.notify(String(error), "error");
      }
    }
  } finally {
    if (!edit.dirty && !edit.error) state.edits.delete(key);
  }
}

export async function openCard(
  ctx: ExtensionContext,
  cli: StreamCli,
  state: CardState,
  actions: {
    onResult(value: EditResult): void;
    discuss(): Promise<void>;
    settings(): Promise<void>;
    report(): void;
    reset?(auto: boolean, target?: "all" | "session" | "stream"): void;
  }
): Promise<void> {
  let activeTab: "actions" | "stream" | "session" = "actions";
  const selectedByTab: Record<"actions" | "stream" | "session", number> = {
    actions: 0,
    stream: 0,
    session: 0,
  };
  for (;;) {
    const catalog = await cli.context(ctx.sessionManager.getSessionId());
    if (!catalog.stream) {
      ctx.ui.notify(
        "No attached work card. Use select_stream or create_stream for initial attachment.",
        "info"
      );
      return;
    }
    const stream = catalog.stream;
    const session = catalog.session;
    const actionItems: SelectItem[] = [
      {
        value: "report",
        label: "Report",
        description: "Summarize recent conversation turns into the work card",
      },
      {
        value: "discuss",
        label: "Discuss…",
        description: "Chat with the reporting helper to refine fields",
      },
      {
        value: "reset:all",
        label: "Reset Stream and Session…",
        description: "Choose how to regenerate both records' automatic fields",
      },
      {
        value: "settings",
        label: "Settings…",
        description: "Configure automatic reporting cadence, model, and naming",
      },
      { value: "close", label: "Close", description: "Close this dialog (Esc)" },
    ];
    const streamItems: SelectItem[] = (["title", "goal"] as const).map((field) => ({
      value: `field:stream:${field}`,
      label: `${field}…${state.edits.has(`stream:${field}`) ? " *" : ""}`,
      description: `${formatFieldPreview(stream[field])}${state.edits.has(`stream:${field}`) ? " · retained unsaved edit" : ""}`,
    }));
    streamItems.push({
      value: "reset:stream",
      label: "Reset…",
      description: "Regenerate only the Stream's automatic fields",
    });

    const sessionItems: SelectItem[] = session
      ? (["title", "status", "recap", "state", "remaining"] as const).map((field) => ({
          value: `field:session:${field}`,
          label: `${field}…${state.edits.has(`session:${field}`) ? " *" : ""}`,
          description: `${formatFieldPreview(session[field])}${state.edits.has(`session:${field}`) ? " · retained unsaved edit" : ""}`,
        }))
      : [
          {
            value: "noop",
            label: "Not yet enrolled",
            description: "Run /streams attach or /streams report to enroll this session",
          },
        ];
    if (session) {
      sessionItems.push(
        {
          value: "references",
          label: "References…",
          description: "Manage this Session's references, including Dropped entries",
        },
        {
          value: "reset:session",
          label: "Reset…",
          description: "Regenerate only this Session's automatic fields",
        },
        { value: "finish:session", label: "Finish…", description: "Mark this Session as done" }
      );
    }

    const fallbackItems = [
      ...actionItems,
      ...streamItems.map((item) => ({ ...item, label: `Stream · ${item.label}` })),
      ...sessionItems.map((item) => ({ ...item, label: `Session · ${item.label}` })),
    ];

    let action: string | undefined;
    if (ctx.ui.custom) {
      action = await ctx.ui.custom<string | undefined>((tui, theme, _keys, done) => {
        const topBorder = new DynamicBorder((text) => theme.fg("border", text));
        const bottomBorder = new DynamicBorder((text) => theme.fg("border", text));
        const tabOrder: ("actions" | "stream" | "session")[] = ["actions", "stream", "session"];
        const itemsForTab = (tab: "actions" | "stream" | "session") =>
          tab === "actions" ? actionItems : tab === "stream" ? streamItems : sessionItems;
        let list: SelectList;
        const buildList = () => {
          const items = itemsForTab(activeTab);
          list = new SelectList(
            items,
            Math.max(4, Math.min(12, tui.terminal.rows - 14)),
            getSelectListTheme()
          );
          list.setSelectedIndex(Math.min(selectedByTab[activeTab], Math.max(0, items.length - 1)));
          list.onSelectionChange = (item) => {
            selectedByTab[activeTab] = Math.max(
              0,
              items.findIndex((i) => i.value === item.value)
            );
          };
          list.onSelect = (item) => done(item.value);
          list.onCancel = () => done(undefined);
        };
        buildList();
        return {
          invalidate() {
            topBorder.invalidate();
            bottomBorder.invalidate();
            list.invalidate();
          },
          handleInput(data: string) {
            if (
              matchesKey(data, "tab") ||
              matchesKey(data, "right") ||
              data === "1" ||
              data === "2" ||
              data === "3" ||
              matchesKey(data, "shift+tab") ||
              matchesKey(data, "left")
            ) {
              if (data === "1") activeTab = "actions";
              else if (data === "2") activeTab = "stream";
              else if (data === "3") activeTab = "session";
              else {
                const delta = matchesKey(data, "shift+tab") || matchesKey(data, "left") ? -1 : 1;
                activeTab = tabOrder[(tabOrder.indexOf(activeTab) + delta + tabOrder.length) % tabOrder.length];
              }
              buildList();
              tui.requestRender();
              return;
            }
            list.handleInput(data);
            tui.requestRender();
          },
          render(width: number) {
            const clean = (text: string) => text.replace(/[\x00-\x1f\x7f]/g, " ");
            const section = (text: string) => theme.fg("accent", theme.bold(text));
            const line = (text: string, limit = 2) =>
              wrapTextWithAnsi(clean(text), Math.max(10, width - 2))
                .slice(0, limit)
                .map((l) => ` ${l}`);
            const tabLabel = (id: "actions" | "stream" | "session", title: string) =>
              activeTab === id ? theme.fg("accent", theme.bold(`[ ${title} ]`)) : theme.fg("muted", title);
            const tabBar = truncateToWidth(
              ` ${tabLabel("actions", "1: Actions")} ${tabLabel("stream", "2: Stream")} ${tabLabel("session", "3: Session")}   ${theme.fg("dim", "Tab/←→/1-3 switch · Esc close")}`,
              width
            );
            const streamMeta = [
              stream.status,
              `title:${stream.title.manual ? "manual" : "auto"}`,
              stream.goal ? `goal:${stream.goal.manual ? "manual" : "auto"}` : null,
            ]
              .filter(Boolean)
              .join(" · ");
            const summary = [
              ` ${section("Stream")} · ${clean(stream.title.text)} ${theme.fg("muted", `[${streamMeta}]`)}`,
              ...line(`Goal: ${stream.goal?.text ?? "—"}`, 2),
              ` ${section("Session")} · ${clean(session?.title.text ?? "Not yet enrolled")} ${session ? theme.fg("muted", `[${session.status} · title:${session.title.manual ? "manual" : "auto"}]`) : ""}`,
              ...(session
                ? [
                    ...line(`Session: ${session.recap?.text || "—"}`, 2),
                    ...line(`Status: ${session.state?.text || "—"}`, 2),
                    ...line(`Remaining: ${session.remaining?.text || "—"}`, 1),
                  ]
                : []),
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
    } else {
      const selected = await ctx.ui.select(
        "Work card",
        fallbackItems.map((item) => item.label)
      );
      action = fallbackItems.find((item) => item.label === selected)?.value;
    }

    if (!action || action === "close") return;
    if (action === "noop") continue;
    if (action === "references") {
      await manageSessionReferences(ctx, cli, actions.onResult);
      continue;
    }
    if (action.startsWith("field:")) {
      const [, kind, field] = action.split(":") as [string, "stream" | "session", string];
      await editField(ctx, cli, catalog, kind, field, state, actions.onResult);
      continue;
    }
    if (action === "discuss") {
      await actions.discuss();
      continue;
    }
    if (action.startsWith("reset:")) {
      const target = action.split(":")[1] as "all" | "stream" | "session";
      const mode = await ctx.ui.select(
        `Reset ${target === "all" ? "Stream and Session" : target === "stream" ? "Stream" : "Session"} · regenerate automatic fields`,
        ["Preserve naming modes", "Restore automatic naming"]
      );
      if (!mode) continue;
      actions.reset?.(mode === "Restore automatic naming", target);
      return;
    }
    if (action === "settings") {
      await actions.settings();
      continue;
    }
    if (action === "report") {
      actions.report();
      return;
    }
    if (action === "finish:session") {
      if (!(await ctx.ui.confirm("Finish Session?", "Mark this Session as done?"))) continue;
      const edit = new FieldEdit("session", catalog, "status");
      edit.value = "done";
      try {
        await edit.save(cli, actions.onResult);
      } catch (error) {
        state.edits.set("session:status", edit);
        ctx.ui.notify(String(error), "error");
      }
    }
  }
}
