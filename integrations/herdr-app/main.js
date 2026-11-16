// Streams mod for herdr-app (roles: sidebar, frame, popup, viewer).
// Preserves the single 30 px pane header, explicit pane controls, and Files/Changes panels.

import { View, div, svg, image } from "gpui-kit";
import { h_flex, v_flex, ScrollHandle, Editor, EditorState, TextView } from "gpui-base";
import {
  spaces,
  panes,
  terminals,
  focused,
  props,
  run,
  openOverlay,
  closeOverlay,
  theme,
  settings,
  home,
  layout,
  exec,
} from "herdr";
import {
  resolveRole,
  observeNoticeToken,
  activeTabSession,
  resolveTabRenameTarget,
  buildReferenceItems,
  StreamsCoordinator,
} from "./state.js";

const subscribers = new Set();
const coordinator = new StreamsCoordinator({
  exec: (cmd, args) => exec(cmd, args, { timeout: 30000 }),
  notify: () => {
    for (const notifyFn of subscribers) {
      try {
        notifyFn();
      } catch {
        // Detached view
      }
    }
  },
});

let lastTabRenameAttempt = null;

function findNode(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = findNode(child, id);
    if (found) return found;
  }
  return null;
}

function paneFrame(node) {
  if (!node || node.hidden) return null;
  if (node.content?.tab && node.content.pane) return node.content.pane;
  let children = node.children ?? [];
  if (node.type === "stack") {
    const visible = children.filter((child) => !child.hidden);
    children = [visible.find((child) => child.id === node.active) ?? visible[0]];
  }
  for (const child of children) {
    const frame = paneFrame(child);
    if (frame) return frame;
  }
  return null;
}

function icon(p, size, color) {
  if (!p?.icon) return null;
  return p.tint
    ? svg(p.icon).size(size).flex_none().text_color(color)
    : image(p.icon).w(size).h(size).flex_none();
}

function folder(cwd) {
  if (!cwd) return "";
  const h = home();
  return cwd === h ? "~" : cwd.startsWith(h + "/") ? "~" + cwd.slice(h.length) : cwd;
}

function statusStyle(status, t) {
  switch (status) {
    case "active":
      return { color: t.ansi2, symbol: "●", label: "active" };
    case "working":
      return { color: t.working, symbol: "●", label: "working" };
    case "waiting":
      return { color: t.working, symbol: "●", label: "waiting" };
    case "blocked":
      return { color: t.blocked, symbol: "●", label: "blocked" };
    case "done":
      return { color: t.done, symbol: "✓", label: "done" };
    case "parked":
      return { color: t.dim, symbol: "○", label: "parked" };
    default:
      return { color: t.dim, symbol: "○", label: status || "unknown" };
  }
}

function statusBadge(status, t) {
  const s = statusStyle(status, t);
  return h_flex()
    .flex_none()
    .gap(4)
    .items_center()
    .text_size(11)
    .font_medium()
    .text_color(s.color)
    .child(div().text_size(9).child(s.symbol))
    .child(s.label);
}

function collectSnapshot() {
  const allSpaces = spaces();
  const activeSpace = allSpaces.find((s) => s.active) ?? null;
  const activeTab = activeSpace?.tabs?.find((t) => t.active) ?? null;
  const allPanes = panes();
  const visibleTerminals = activeTab
    ? allPanes
        .filter((p) => p.tab === activeTab.id && p.terminal)
        .map((p) => p.terminal)
    : [];
  const focus = focused();
  const focusedPane = focus?.pane ? allPanes.find((p) => p.pane === focus.pane) : null;
  const focusedTerminal =
    focusedPane?.program?.agent && focusedPane?.terminal
      ? focusedPane.terminal
      : focus?.agent ?? null;
  return {
    activeSpace,
    activeTab,
    activeTabId: activeTab?.id ?? null,
    visibleTerminals,
    terminalList: terminals(),
    focusedTerminal,
  };
}

function syncActiveTabName(snap) {
  const target = resolveTabRenameTarget({
    activeTab: snap.activeTab,
    visibleTerminals: snap.visibleTerminals,
    frames: coordinator.frames,
    tabNaming: coordinator.tabNaming,
  });
  if (!target) {
    lastTabRenameAttempt = null;
    return;
  }
  const key = `${target.tabId}\0${target.currentName}\0${target.desiredName}`;
  if (lastTabRenameAttempt === key) return;
  lastTabRenameAttempt = key;
  run("tab.rename", { id: target.tabId, name: target.desiredName });
}

function snapshotSignature(snap) {
  return JSON.stringify({
    space: snap.activeSpace?.id ?? null,
    machine: snap.activeSpace?.machine ?? null,
    tab: snap.activeTabId,
    tabName: snap.activeTab?.name ?? null,
    terminals: snap.visibleTerminals,
    focused: snap.focusedTerminal,
    notice: observeNoticeToken(
      snap.terminalList,
      coordinator.binding?.notification_key ?? null
    ),
  });
}

const VIEWER_LANG = {
  ts: "js",
  tsx: "js",
  mts: "js",
  cts: "js",
  jsx: "js",
  mjs: "js",
  toml: "yaml",
  jsonc: "json",
  svelte: "html",
  vue: "html",
};

function isMarkdownFilename(filename) {
  return typeof filename === "string" && /\.md$/i.test(filename.trim());
}

function viewerLanguage(filename) {
  if (typeof filename !== "string" || !filename.includes(".")) return "txt";
  const ext = filename.split(".").pop().toLowerCase();
  return VIEWER_LANG[ext] ?? ext;
}

export default class StreamsMod extends View {
  init(initialProps, cx) {
    this.hovered = false;
    this.scroll = ScrollHandle.new();
    this._subscribed = false;
    this._lastSig = null;
    this._observeTask = null;
    this._timerTask = null;
    this._editorState = null;
    this._viewerKey = null;
    const role = resolveRole(initialProps);
    if (role === "viewer") {
      this.syncViewerState(initialProps);
      return;
    }
    this.ensureSubscribed(cx);
    this.scheduleObserve(cx, collectSnapshot());
    this._timerTask = cx?.timer?.every?.(2000, (timerCx) => {
      this.scheduleObserve(timerCx, collectSnapshot());
    });
  }

  async close() {
    this._editorState?.release?.();
    this._editorState = null;
    this._viewerKey = null;
  }

  syncViewerState(rawProps) {
    const filename = rawProps?.filename ?? "";
    const content = rawProps?.content ?? "";
    const key = `${filename}\0${content}`;
    if (this._viewerKey === key) return;
    this._editorState?.release?.();
    this._editorState = null;
    this._viewerKey = key;
    if (!isMarkdownFilename(filename) && typeof content === "string") {
      const t = theme();
      const s = settings?.() ?? {};
      this._editorState = EditorState.new({
        language: viewerLanguage(filename),
        readonly: true,
        value: content,
        font_family: s.font_family,
        font_size: 12,
        syntax: {
          keyword: t.ansi5,
          string: t.ansi2,
          comment: t.dim,
          function: t.ansi4,
          type: t.ansi3,
          number: t.ansi1,
          constant: t.ansi6,
        },
      });
    }
  }

  ensureSubscribed(cx) {
    if (!this._subscribed && cx) {
      this._subscribed = true;
      subscribers.add(() => {
        try {
          cx.notify();
        } catch {
          // Detached
        }
      });
    }
    const configuredBin = props()?.streams_bin;
    if (configuredBin && coordinator.streamsBin !== configuredBin) {
      coordinator.streamsBin = configuredBin;
    }
  }

  scheduleObserve(cx, snap, force = false) {
    const configuredBin = props()?.streams_bin;
    if (configuredBin && coordinator.streamsBin !== configuredBin) {
      coordinator.streamsBin = configuredBin;
    }
    syncActiveTabName(snap);
    const sig = snapshotSignature(snap);
    if (
      !force &&
      sig === this._lastSig &&
      (coordinator.frames.size > 0 ||
        coordinator.describeError ||
        coordinator.lastError ||
        coordinator.lastCompletedKey !== null ||
        coordinator.inFlightKey !== null ||
        coordinator.describePromise !== null)
    ) {
      return;
    }
    this._lastSig = sig;
    if (cx?.spawn) {
      this._observeTask = cx.spawn(async (asyncCx) => {
        try {
          await coordinator.observe({
            ...snap,
            force,
          });
          syncActiveTabName(collectSnapshot());
        } finally {
          asyncCx.notify();
        }
      });
    } else {
      Promise.resolve(coordinator.observe({ ...snap, force })).then(() => {
        syncActiveTabName(collectSnapshot());
        cx?.notify?.();
      });
    }
  }

  button(t, id, iconName, command, args) {
    return div()
      .id(id)
      .p(3)
      .rounded(4)
      .text_color(t.dim)
      .hover((s) => s.bg(t.hover).text_color(t.foreground))
      .on_click((_e, cx) => {
        cx.stop_propagation();
        run(command, args);
      })
      .child(svg(`mods/pane/${iconName}.svg`).size(14));
  }

  renderFrame(cx) {
    const t = theme();
    const rawProps = props() ?? {};
    const p = panes().find(
      (item) =>
        item.pane === rawProps.pane ||
        (rawProps.terminal && item.terminal === rawProps.terminal)
    );
    if (!p) return div();

    const snap = collectSnapshot();
    this.scheduleObserve(cx, snap);

    const frameCtx = p.terminal ? coordinator.frames.get(p.terminal) : null;
    const enrolled = frameCtx?.kind === "enrolled" ? frameCtx.session : null;
    const streamTitle = enrolled ? coordinator.streamTitle(enrolled.stream) : null;

    const nativeName =
      ((p.program?.agent || p.program?.shell) && p.program.name) ||
      p.title ||
      p.program?.name ||
      "shell";
    const pic = icon(p.program, 14, t.dim);
    const st = p.status;
    const pane = p.pane;
    const bottom = paneFrame(layout())?.header?.position === "bottom";

    return h_flex()
      .id(`pane-head-${pane}`)
      .size_full()
      .px(10)
      .gap(8)
      .items_center()
      .bg(t.background)
      .when(bottom, (el) => el.border_t(1))
      .when(!bottom, (el) => el.border_b(1))
      .border_color(t.border)
      .text_size(12)
      .on_hover((hovered, hoverCx) => {
        this.hovered = hovered;
        hoverCx.notify();
      })
      .on_click(() => run("pane.focus", { pane }))
      .when(Boolean(pic), (el) => el.child(pic))
      .child(
        div()
          .flex_none()
          .truncate()
          .max_w(260)
          .font_medium()
          .text_color(t.foreground)
          .child(nativeName)
      )
      .child(div().flex_1().truncate().text_color(t.dim).child(folder(p.cwd)))
      .when(Boolean(enrolled), (el) =>
        el.child(
          div()
            .id(`ph-stream-${pane}`)
            .flex_none()
            .max_w(280)
            .truncate()
            .px(7)
            .h(20)
            .flex()
            .items_center()
            .rounded(5)
            .bg(t.selected)
            .text_size(11)
            .font_medium()
            .text_color(t.foreground)
            .hover((s) => s.bg(t.hover))
            .on_click((_e, clickCx) => {
              clickCx.stop_propagation();
              const root = layout();
              const panel = findNode(root, "panel");
              const panelBody = findNode(root, "panel-body");
              const panelId = props()?.panel_id ?? "streams";
              const hasPanelChild = Boolean(
                panel && panelBody && findNode(panelBody, panelId)
              );
              if (
                hasPanelChild &&
                (panel.hidden || panelBody.active !== panelId)
              ) {
                run("layout.select", { stack: "panel-body", child: panelId });
                run("layout.show", { id: "panel" });
              } else {
                openOverlay({
                  mod: "streams",
                  kind: "popup",
                  at: "pointer",
                  width: 420,
                  height: 360,
                  props: {
                    kind: "session-popup",
                    session: enrolled,
                    stream_title: streamTitle,
                    space_id: snap.activeSpace?.id ?? null,
                  },
                });
              }
            })
            .child(`≋ ${streamTitle} · ${enrolled.title}`)
        )
      )
      .when(st === "working", (el) =>
        el.child(
          svg("mods/workspace-chrome/spinner.svg")
            .size(12)
            .flex_none()
            .text_color(t.working)
            .spin(900)
        )
      )
      .when(st === "blocked", (el) =>
        el.child(
          div()
            .flex_none()
            .px(6)
            .h(18)
            .flex()
            .items_center()
            .rounded(5)
            .bg(t.blocked + "26")
            .text_size(10.5)
            .font_semibold()
            .text_color(t.blocked)
            .child("Needs you")
        )
      )
      .when(st === "done", (el) =>
        el.child(div().size(7).flex_none().rounded(4).bg(t.done))
      )
      .when(this.hovered, (el) =>
        el
          .child(
            this.button(t, `ph-r-${pane}`, "split-right", "pane.split", {
              direction: "right",
              pane,
            })
          )
          .child(
            this.button(t, `ph-d-${pane}`, "split-down", "pane.split", {
              direction: "down",
              pane,
            })
          )
          .child(
            this.button(
              t,
              `ph-z-${pane}`,
              p.zoomed ? "unzoom" : "zoom",
              "pane.zoom",
              { pane }
            )
          )
          .child(this.button(t, `ph-x-${pane}`, "close", "pane.close", { pane }))
      );
  }

  renderPopup() {
    const t = theme();
    const rawProps = props() ?? {};
    if (rawProps.kind === "stream-menu") {
      const streamId = rawProps.stream_id;
      return v_flex()
        .w_full()
        .p(5)
        .bg(t.popover)
        .text_size(12.5)
        .text_color(t.foreground)
        .child(
          div()
            .id("stream-menu-open-all")
            .h(30)
            .px(10)
            .flex()
            .items_center()
            .rounded(6)
            .hover((s) => s.bg(t.hover))
            .on_click((_e, clickCx) => {
              if (rawProps.overlay) closeOverlay(rawProps.overlay);
              if (streamId) {
                coordinator.openWork(
                  { stream: streamId },
                  {
                    run,
                    openUrl: (u) => clickCx.open_url(u),
                  }
                );
              }
            })
            .child("Open all Stream sessions")
        );
    }

    const session = rawProps.session;
    const streamTitle = rawProps.stream_title ?? null;
    if (!session) return div().p(12).text_color(t.dim).child("No Session selected.");

    return v_flex()
      .size_full()
      .p(12)
      .gap(8)
      .bg(t.raised)
      .text_color(t.foreground)
      .text_size(12)
      .child(
        h_flex()
          .justify_between()
          .items_center()
          .child(div().font_semibold().text_size(13).child(session.title))
          .child(
            div()
              .id("popup-close")
              .px(6)
              .py(2)
              .rounded(4)
              .text_color(t.dim)
              .hover((s) => s.bg(t.hover).text_color(t.foreground))
              .on_click(() => {
                if (rawProps.overlay) closeOverlay(rawProps.overlay);
              })
              .child("Close")
          )
      )
      .when(Boolean(streamTitle), (el) =>
        el.child(div().text_color(t.dim).child(`≋ ${streamTitle}`))
      )
      .child(
        h_flex()
          .gap(8)
          .items_center()
          .child(statusBadge(session.status, t))
          .when(Boolean(session.state), (el) =>
            el.child(div().text_color(t.muted).truncate().child(session.state))
          )
      )
      .when(Boolean(session.recap), (el) =>
        el.child(div().text_color(t.foreground).child(session.recap))
      )
      .when(Boolean(session.remaining), (el) =>
        el.child(
          v_flex()
            .gap(4)
            .child(div().font_medium().text_color(t.muted).child("Remaining"))
            .child(div().text_color(t.foreground).child(session.remaining))
        )
      )
      .child(
        h_flex()
          .pt(4)
          .gap(6)
          .child(
            div()
              .id("popup-open-session")
              .px(8)
              .py(4)
              .rounded(5)
              .bg(t.selected)
              .text_color(t.foreground)
              .hover((s) => s.bg(t.hover))
              .on_click((_e, clickCx) => {
                coordinator.openWork(
                  {
                    stream: session.stream,
                    sessions: { kind: "only", value: [session.id] },
                    cwd: null,
                  },
                  {
                    run,
                    openUrl: (u) => clickCx.open_url(u),
                  }
                );
              })
              .child("Open Session")
          )
      );
  }

  renderSidebar(cx) {
    const t = theme();
    const snap = collectSnapshot();
    this.scheduleObserve(cx, snap);

    const activeSpace = snap.activeSpace;
    const header = h_flex()
      .h(34)
      .px(12)
      .justify_between()
      .items_center()
      .border_b(1)
      .border_color(t.border)
      .child(div().font_semibold().text_size(12.5).text_color(t.foreground).child("Streams"))
      .child(
        div()
          .id("streams-refresh")
          .px(6)
          .py(2)
          .rounded(4)
          .text_size(11.5)
          .text_color(t.muted)
          .hover((s) => s.bg(t.hover).text_color(t.foreground))
          .on_click((_e, clickCx) => {
            this.scheduleObserve(clickCx, collectSnapshot(), true);
          })
          .child("Refresh")
      );

    if (coordinator.describeError) {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(div().p(12).text_size(12).text_color(t.dim).child(coordinator.describeError));
    }

    if (coordinator.lastError && coordinator.frames.size === 0) {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(
          div()
            .p(12)
            .text_size(12)
            .text_color(t.dim)
            .child(`Unavailable: ${coordinator.lastError}`)
        );
    }

    if (!activeSpace || !snap.activeTabId) {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(
          div()
            .p(12)
            .text_size(12)
            .text_color(t.dim)
            .child("No active tab.")
        );
    }

    if (activeSpace.machine !== null) {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(
          div()
            .p(12)
            .text_size(12)
            .text_color(t.dim)
            .child("Streams sidebar is available on local tabs.")
        );
    }

    if (coordinator.lastCompletedKey === null && coordinator.inFlightKey !== null) {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(div().p(12).text_size(12).text_color(t.dim).child("Loading Streams…"));
    }

    const tabSession = activeTabSession({
      visibleTerminals: snap.visibleTerminals,
      focusedTerminal: snap.focusedTerminal,
      frames: coordinator.frames,
    });
    if (!tabSession) {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(
          div()
            .p(12)
            .text_size(12)
            .text_color(t.dim)
            .child(
              coordinator.lastError
                ? `Unavailable: ${coordinator.lastError}`
                : "This tab is not enrolled in a Stream."
            )
        );
    }

    const enrolled = tabSession.session;
    const stream = coordinator.streams.get(enrolled.stream) ?? null;
    if (!stream) {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(div().p(12).text_size(12).text_color(t.dim).child("Loading Stream…"));
    }

    const references = buildReferenceItems(stream, {
      viewingSessionId: enrolled.id,
    });

    return v_flex()
      .size_full()
      .bg(t.chrome)
      .text_size(12)
      .text_color(t.foreground)
      .child(header)
      .child(
        v_flex()
          .id("streams-sidebar-body")
          .flex_1()
          .p(12)
          .gap(12)
          .overflow_y_scrollbar()
          .track_scroll(this.scroll)
          .when(Boolean(coordinator.lastError), (el) =>
            el.child(
              div()
                .p(8)
                .rounded(5)
                .bg(t.raised)
                .text_size(11.5)
                .text_color(t.blocked)
                .child(coordinator.lastError)
            )
          )
          .child(
            v_flex()
              .gap(6)
              .child(
                h_flex()
                  .gap(8)
                  .items_center()
                  .child(
                    div()
                      .flex_1()
                      .min_w(0)
                      .truncate()
                      .font_semibold()
                      .text_size(13.5)
                      .child(stream.title)
                  )
                  .child(statusBadge(stream.status, t))
                  .child(
                    div()
                      .id("streams-menu-btn")
                      .px(5)
                      .py(2)
                      .rounded(4)
                      .text_color(t.muted)
                      .hover((s) => s.bg(t.hover).text_color(t.foreground))
                      .on_click((_e, clickCx) => {
                        clickCx.stop_propagation();
                        openOverlay({
                          mod: "streams",
                          kind: "popup",
                          at: "pointer",
                          width: 210,
                          height: 44,
                          props: {
                            kind: "stream-menu",
                            stream_id: stream.id,
                          },
                        });
                      })
                      .child("⋮")
                  )
              )
              .when(Boolean(stream.goal), (el) =>
                el.child(div().text_color(t.muted).child(stream.goal))
              )
          )
          .child(
            v_flex()
              .gap(5)
              .child(div().font_medium().text_size(11.5).text_color(t.muted).child("Sessions"))
              .child(
                v_flex()
                  .p(9)
                  .gap(4)
                  .rounded(6)
                  .bg(t.raised)
                  .border(1)
                  .border_color(t.border)
                  .child(
                    h_flex()
                      .gap(8)
                      .items_center()
                      .child(
                        div()
                          .flex_1()
                          .min_w(0)
                          .truncate()
                          .font_medium()
                          .child(enrolled.title)
                      )
                      .child(statusBadge(enrolled.status, t))
                  )
                  .when(Boolean(enrolled.recap || enrolled.state), (el) =>
                    el.child(
                      div()
                        .text_size(11.5)
                        .text_color(t.muted)
                        .child(enrolled.recap || enrolled.state)
                    )
                  )
              )
          )
          .when(references.length > 0, (el) =>
            el.child(
              v_flex()
                .gap(5)
                .child(
                  div()
                    .font_medium()
                    .text_size(11.5)
                    .text_color(t.muted)
                    .child("References")
                )
                .child(
                  v_flex()
                    .rounded(6)
                    .bg(t.raised)
                    .border(1)
                    .border_color(t.border)
                    .children(
                      references.map((item, idx) =>
                        v_flex()
                          .id(item.id)
                          .px(9)
                          .py(7)
                          .gap(2)
                          .when(idx > 0, (row) =>
                            row.border_t(1).border_color(t.border)
                          )
                          .when(!item.disabled, (row) =>
                            row
                              .hover((s) => s.bg(t.hover))
                              .on_click((_e, clickCx) => {
                                const clickedSpaceId = activeSpace?.id ?? null;
                                const clickedTabId = snap.activeTabId;
                                const focusNow = focused();
                                const allPanesNow = panes();
                                const targetPaneId =
                                  focusNow?.pane ??
                                  allPanesNow.find((p) => p.tab === clickedTabId && p.focused)
                                    ?.pane ??
                                  allPanesNow.find((p) => p.tab === clickedTabId)?.pane ??
                                  null;
                                clickCx.spawn(async () => {
                                  await coordinator.openReference(item.reference, {
                                    activeSpaceId: clickedSpaceId,
                                    targetPaneId,
                                    run,
                                    openOverlay,
                                    openUrl: (u) => clickCx.open_url(u),
                                  });
                                });
                              })
                          )
                          .child(
                            div()
                              .font_medium()
                              .text_size(12)
                              .truncate()
                              .text_color(t.foreground)
                              .child(item.title)
                          )
                          .when(Boolean(item.subtitle), (row) =>
                            row.child(
                              div()
                                .text_size(11)
                                .text_color(t.muted)
                                .truncate()
                                .child(item.subtitle)
                            )
                          )
                      )
                    )
                )
            )
          )
      );
  }

  renderViewer() {
    const t = theme();
    const rawProps = props() ?? {};
    this.syncViewerState(rawProps);
    const title = rawProps.title || rawProps.filename || "Reference";
    const overlayId = rawProps.overlay ?? null;
    const isMd = isMarkdownFilename(rawProps.filename);

    return v_flex()
      .size_full()
      .bg(t.background)
      .text_size(12)
      .text_color(t.foreground)
      .child(
        h_flex()
          .h(30)
          .flex_none()
          .px(10)
          .gap(8)
          .items_center()
          .bg(t.chrome)
          .border_b(1)
          .border_color(t.border)
          .child(div().flex_1().min_w(0).truncate().font_medium().child(title))
          .when(Boolean(rawProps.truncated), (el) =>
            el.child(
              div()
                .flex_none()
                .px(6)
                .py(1)
                .rounded(4)
                .bg(t.raised)
                .text_size(10.5)
                .text_color(t.muted)
                .child("truncated")
            )
          )
          .when(Boolean(overlayId), (el) =>
            el.child(
              div()
                .id("streams-viewer-close")
                .flex_none()
                .px(6)
                .py(2)
                .rounded(4)
                .text_size(11.5)
                .text_color(t.muted)
                .hover((s) => s.bg(t.hover).text_color(t.foreground))
                .on_mouse_down((_e, downCx) => downCx.stop_propagation())
                .on_click((_e, clickCx) => {
                  clickCx.stop_propagation();
                  closeOverlay(overlayId);
                })
                .child("Close")
            )
          )
      )
      .child(
        isMd
          ? v_flex()
              .flex_1()
              .min_h(0)
              .p(12)
              .child(
                TextView.markdown("streams-md-viewer", rawProps.content ?? "")
                  .scrollable(true)
                  .selectable(true)
              )
          : this._editorState
            ? Editor.new(this._editorState).flex_1().min_h(0)
            : div().p(12).text_color(t.dim).child("No content.")
      );
  }

  render(cx) {
    const role = resolveRole(props());
    if (role === "viewer") return this.renderViewer();
    this.ensureSubscribed(cx);
    if (role === "popup") return this.renderPopup(cx);
    if (role === "frame") return this.renderFrame(cx);
    return this.renderSidebar(cx);
  }
}
