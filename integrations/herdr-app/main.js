// Streams mod for herdr-app (roles: sidebar, frame, popup, viewer).
// Preserves the single 30 px pane header, explicit pane controls, and Files/Changes panels.

import { View, div, svg, image } from "gpui-kit";
import { h_flex, v_flex, ScrollHandle, Editor, EditorState, TextView, Input, InputState } from "gpui-base";
import {
  spaces,
  panes,
  terminals,
  agents,
  machines,
  focused,
  props,
  run,
  openOverlay,
  closeOverlay,
  theme,
  settings,
  home,
  layout,
  window as herdrWindow,
  checking,
  exec,
} from "herdr";
import {
  STALE_FALLBACK_MS,
  HOST_STREAM_FILTERS,
  HOST_STREAM_SORTS,
  resolveRole,
  observeNoticeToken,
  activeTabSession,
  effectiveSessionStatus,
  resolveTabRenameTarget,
  buildReferenceItems,
  filterHostStreams,
  unreadableNotice,
  hostStreamsEmptyMessage,
  formatRelativeAge,
  liveStreamIds,
  StreamsCoordinator,
} from "./state.js";

const subscribers = new Set();
let fallbackTimerOwner = null;
let lastWindowActive = false;
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
    case "closed":
      return { color: t.dim, symbol: "×", label: "closed" };
    case "offline":
      return { color: t.dim, symbol: "○", label: "offline" };
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

function referenceStyle(kind, t) {
  switch (kind) {
    case "pr":
      return { icon: "mods/streams/icons/git-pull-request.svg", color: t.ansi2 };
    case "issue":
      return { icon: "mods/streams/icons/circle-dot.svg", color: t.ansi3 };
    case "document":
      return { icon: "mods/streams/icons/file-code.svg", color: t.ansi6 };
    case "artifact":
      return { icon: "mods/streams/icons/scroll-text.svg", color: t.ansi5 };
    default:
      return { icon: "mods/streams/icons/link-2.svg", color: t.ansi4 };
  }
}

function referenceIcon(kind, t) {
  const r = referenceStyle(kind, t);
  return svg(r.icon).size(14).flex_none().text_color(r.color);
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
  const liveTerminals = allPanes
    .filter((p) => p.terminal && !p.terminal.includes("/") && p.program?.agent)
    .map((p) => p.terminal);
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
    liveTerminals,
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
    liveTerminals: snap.liveTerminals,
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
    this.otherSessionsOpen = true;
    this.streamFilter = "in_progress";
    this.streamSort = "recent";
    this.streamQuery = "";
    this.scroll = ScrollHandle.new();
    this._sub = null;
    this._lastSig = null;
    this._observeTask = null;
    this._timerTask = null;
    this._editorState = null;
    this._viewerKey = null;
    this.searchInput = null;
    const role = resolveRole(initialProps);
    this._role = role;
    if (role === "sidebar") {
      this.searchInput = InputState.new({ placeholder: "Filter streams…" });
      this.searchInput.on("change", (_e, changeCx) => {
        this.streamQuery = this.searchInput.value();
        changeCx.notify();
      });
    }
    if (role === "viewer") {
      this.syncViewerState(initialProps);
      return;
    }
    if (role !== "sidebar" && role !== "frame") {
      return;
    }
    if (checking()) {
      return;
    }
    this.ensureSubscribed(cx);
    this.scheduleObserve(cx, collectSnapshot());
    if (!fallbackTimerOwner && cx?.timer?.every) {
      fallbackTimerOwner = this;
      this._timerTask = cx.timer.every(STALE_FALLBACK_MS, (timerCx) => {
        if (fallbackTimerOwner !== this || subscribers.size === 0) return;
        this.scheduleObserve(timerCx, collectSnapshot(), true);
      });
    }
  }

  async close() {
    if (this._sub) {
      subscribers.delete(this._sub);
      this._sub = null;
    }
    if (fallbackTimerOwner === this) {
      fallbackTimerOwner = null;
    }
    this._timerTask = null;
    this.searchInput?.release?.();
    this.searchInput = null;
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
    if (!this._sub && cx) {
      this._sub = () => {
        try {
          cx.notify();
        } catch {
          // Detached
        }
      };
      subscribers.add(this._sub);
    }
    const configuredBin = props()?.streams_bin;
    if (configuredBin && coordinator.streamsBin !== configuredBin) {
      coordinator.streamsBin = configuredBin;
    }
  }

  scheduleObserve(cx, snap, force = false) {
    if (checking()) return;
    const configuredBin = props()?.streams_bin;
    if (configuredBin && coordinator.streamsBin !== configuredBin) {
      coordinator.streamsBin = configuredBin;
    }
    if (!fallbackTimerOwner && cx?.timer?.every) {
      fallbackTimerOwner = this;
      this._timerTask = cx.timer.every(STALE_FALLBACK_MS, (timerCx) => {
        if (fallbackTimerOwner !== this || subscribers.size === 0) return;
        this.scheduleObserve(timerCx, collectSnapshot(), true);
      });
    }
    const isWinActive = Boolean(herdrWindow()?.active);
    const windowActivated = isWinActive && !lastWindowActive;
    lastWindowActive = isWinActive;
    syncActiveTabName(snap);
    const sig = snapshotSignature(snap);
    const stale =
      coordinator.staleFallbackMs > 0 &&
      coordinator.lastCompletedAt > 0 &&
      Date.now() - coordinator.lastCompletedAt >= coordinator.staleFallbackMs;
    if (
      !force &&
      !(windowActivated && stale) &&
      sig === this._lastSig &&
      (coordinator.frames.size > 0 ||
        coordinator.describeError ||
        coordinator.lastError ||
        coordinator.lastCompletedKey !== null ||
        coordinator.lastFailedKey !== null ||
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
    const allPanes = panes();
    const p = allPanes.find(
      (item) =>
        item.pane === rawProps.pane ||
        (rawProps.terminal && item.terminal === rawProps.terminal)
    );
    if (!p) return div();

    const snap = collectSnapshot();
    this.scheduleObserve(cx, snap);

    const pane = p.pane;
    const bottom = paneFrame(layout())?.header?.position === "bottom";

    if (p.mod === "streams") {
      const meta = coordinator.resolveViewerPaneMeta(pane, p.tab, allPanes);
      const monoFont = settings()?.font_family ?? "JetBrains Mono";
      const viewerTitle = meta?.title || meta?.filename || "Reference";
      const showFilename = Boolean(meta?.filename && meta.filename !== viewerTitle);
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
        .child(referenceIcon("document", t))
        .child(
          div()
            .flex_none()
            .truncate()
            .max_w(280)
            .font_medium()
            .text_color(t.foreground)
            .child(viewerTitle)
        )
        .when(showFilename, (el) =>
          el.child(
            div()
              .min_w(0)
              .truncate()
              .font_family(monoFont)
              .text_size(11)
              .text_color(t.dim)
              .child(meta.filename)
          )
        )
        .child(div().flex_1())
        .when(Boolean(meta?.truncated), (el) =>
          el
            .child(
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
            .when(Boolean(meta?.reference), (btn) =>
              btn.child(
                div()
                  .id(`ph-open-editor-${pane}`)
                  .flex_none()
                  .px(6)
                  .py(1)
                  .rounded(4)
                  .text_size(10.5)
                  .text_color(t.accent)
                  .hover((s) => s.bg(t.hover))
                  .on_click((_e, clickCx) => {
                    clickCx.stop_propagation();
                    clickCx.spawn(async (asyncCx) => {
                      await coordinator.openReference(meta.reference, {
                        activeSpaceId: meta.spaceId ?? snap.activeSpace?.id ?? null,
                        activeTabId: p.tab,
                        targetPaneId: pane,
                        mode: "editor",
                        run,
                        openOverlay,
                        openUrl: (u) => asyncCx.open_url(u),
                      });
                    });
                  })
                  .child("Open in editor")
              )
            )
        )
        .when(this.hovered, (el) =>
          el
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

    const frameCtx = p.terminal ? coordinator.frames.get(p.terminal) : null;
    const enrolled = frameCtx?.kind === "enrolled" ? frameCtx.session : null;
    const streamTitle = enrolled ? coordinator.streamTitle(enrolled.stream) : null;
    const termInfo = p.terminal
      ? snap.terminalList.find((item) => (item.address ?? item.id) === p.terminal) ?? null
      : null;
    const liveAgent = p.terminal
      ? agents().find((a) => a.address === p.terminal) ?? null
      : null;
    const effectiveStatus = enrolled
      ? effectiveSessionStatus(enrolled, {
          pane: p,
          terminal: termInfo,
          agent: liveAgent,
        })
      : null;
    const enrolledStyle = enrolled ? statusStyle(effectiveStatus, t) : null;

    const nativeName =
      ((p.program?.agent || p.program?.shell) && p.program.name) ||
      p.title ||
      p.program?.name ||
      "shell";
    const pic = icon(p.program, 14, t.dim);
    const st = p.status;

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
          .max_w(220)
          .font_medium()
          .text_color(t.foreground)
          .child(nativeName)
      )
      .child(div().min_w(0).truncate().text_color(t.dim).child(folder(p.cwd)))
      .when(Boolean(enrolled), (el) =>
        el
          .child(div().flex_none().text_color(t.dim).child("·"))
          .child(
            h_flex()
              .id(`ph-stream-${pane}`)
              .flex_initial()
              .min_w(0)
              .max_w(260)
              .px(5)
              .h(20)
              .gap(5)
              .items_center()
              .rounded(4)
              .text_size(11.5)
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
                if (hasPanelChild) {
                  if (panel.hidden || panelBody.active !== panelId) {
                    run("layout.select", { stack: "panel-body", child: panelId });
                    run("layout.show", { id: "panel" });
                  } else {
                    run("layout.hide", { id: "panel" });
                  }
                } else {
                  openOverlay({
                    mod: "streams",
                    kind: "popup",
                    at: "pointer",
                    width: 420,
                    height: 320,
                    props: {
                      kind: "session-popup",
                      session: enrolled,
                      stream_title: streamTitle,
                      space_id: snap.activeSpace?.id ?? null,
                    },
                  });
                }
              })
              .child(
                div()
                  .flex_none()
                  .text_size(10)
                  .text_color(enrolledStyle.color)
                  .child(enrolledStyle.symbol)
              )
              .child(div().flex_1().min_w(0).truncate().child(enrolled.title))
          )
      )
      .child(div().flex_1())
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
      .when(!enrolled && st === "done", (el) =>
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
                clickCx.spawn(async (asyncCx) => {
                  await coordinator.openWork(
                    { stream: streamId },
                    {
                      run,
                      openUrl: (u) => asyncCx.open_url(u),
                    }
                  );
                });
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
      const machineInfo = machines().find((m) => m.name === activeSpace.machine) ?? null;
      const offline = machineInfo?.layout === "unavailable";
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
              offline
                ? `Streams sidebar is unavailable — ${activeSpace.machine} is offline.`
                : "Streams sidebar is available on local tabs."
            )
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
    const renderErrorBanner = (parent) =>
      parent.when(Boolean(coordinator.lastError), (el) =>
        el.child(
          h_flex()
            .p(8)
            .gap(8)
            .items_start()
            .rounded(5)
            .bg(t.raised)
            .border(1)
            .border_color(t.border)
            .text_size(11.5)
            .text_color(t.blocked)
            .child(div().flex_1().min_w(0).child(coordinator.lastError))
            .child(
              div()
                .id("streams-error-dismiss")
                .flex_none()
                .px(4)
                .py(1)
                .rounded(4)
                .text_color(t.muted)
                .hover((s) => s.bg(t.hover).text_color(t.foreground))
                .on_click((_e, clickCx) => {
                  clickCx.stop_propagation();
                  coordinator.lastError = null;
                  coordinator.notifyViews();
                })
                .child("×")
            )
        )
      );
    // One notice for every view that skips unreadable records: the host list and Stream views.
    const renderUnreadableNotice = (notice) =>
      v_flex()
        .id("streams-unreadable-notice")
        .p(8)
        .gap(4)
        .rounded(5)
        .bg(t.raised)
        .border(1)
        .border_color(t.border)
        .text_size(11.5)
        .child(div().text_color(t.blocked).child(notice.title))
        .children(
          notice.records.map((record) =>
            v_flex()
              .gap(1)
              .child(div().text_size(11).text_color(t.muted).child(record.label))
              .child(div().text_size(11).text_color(t.dim).child(record.repair))
          )
        );
    const renderReferencesSection = (references, monoFont) =>
      v_flex()
        .pt(4)
        .gap(6)
        .border_t(1)
        .border_color(t.border)
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
                  .gap(3)
                  .when(idx > 0, (row) => row.border_t(1).border_color(t.border))
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
                        clickCx.spawn(async (asyncCx) => {
                          await coordinator.openReference(item.reference, {
                            activeSpaceId: clickedSpaceId,
                            activeTabId: clickedTabId,
                            targetPaneId,
                            run,
                            openOverlay,
                            openUrl: (u) => asyncCx.open_url(u),
                          });
                        });
                      })
                  )
                  .child(
                    h_flex()
                      .gap(7)
                      .items_center()
                      .child(referenceIcon(item.kind, t))
                      .child(
                        div()
                          .flex_1()
                          .min_w(0)
                          .font_medium()
                          .text_size(12)
                          .truncate()
                          .text_color(t.foreground)
                          .when(item.monoTitle, (el) =>
                            el.font_family(monoFont).text_size(11.5)
                          )
                          .child(item.title)
                      )
                      .when(item.external, (el) =>
                        el.child(
                          div()
                            .flex_none()
                            .text_size(11)
                            .text_color(t.dim)
                            .child("↗")
                        )
                      )
                  )
                  .when(Boolean(item.location), (row) =>
                    row.child(
                      div()
                        .font_family(monoFont)
                        .text_size(11)
                        .text_color(t.dim)
                        .truncate()
                        .child(item.location)
                    )
                  )
                  .when(Boolean(item.description), (row) =>
                    row.child(
                      div()
                        .text_size(11)
                        .text_color(t.muted)
                        .child(item.description)
                    )
                  )
              )
            )
        );
    const renderSessionListSection = (streamId, sessionList, titleText, collapsible) =>
      v_flex()
        .pt(4)
        .gap(6)
        .border_t(1)
        .border_color(t.border)
        .child(
          h_flex()
            .id("streams-other-sessions-toggle")
            .justify_between()
            .items_center()
            .when(collapsible, (el) =>
              el
                .hover((s) => s.text_color(t.foreground))
                .on_click((_e, clickCx) => {
                  clickCx.stop_propagation();
                  this.otherSessionsOpen = !this.otherSessionsOpen;
                  clickCx.notify();
                })
            )
            .child(
              div()
                .font_medium()
                .text_size(11.5)
                .text_color(t.muted)
                .child(titleText)
            )
            .when(collapsible, (el) =>
              el.child(
                div()
                  .text_size(11)
                  .text_color(t.dim)
                  .child(this.otherSessionsOpen ? "▾" : "▸")
              )
            )
        )
        .when(!collapsible || this.otherSessionsOpen, (el) =>
          el.child(
            v_flex()
              .rounded(6)
              .bg(t.raised)
              .border(1)
              .border_color(t.border)
              .children(
                sessionList.map((sess, idx) =>
                  v_flex()
                    .id(`stream-session-${sess.id}`)
                    .px(9)
                    .py(7)
                    .gap(3)
                    .when(idx > 0, (row) => row.border_t(1).border_color(t.border))
                    .hover((s) => s.bg(t.hover))
                    .on_click((_e, clickCx) => {
                      clickCx.spawn(async (asyncCx) => {
                        await coordinator.openWork(
                          {
                            stream: streamId,
                            sessions: { kind: "only", value: [sess.id] },
                            cwd: null,
                          },
                          {
                            run,
                            openUrl: (u) => asyncCx.open_url(u),
                          }
                        );
                      });
                    })
                    .child(
                      h_flex()
                        .gap(6)
                        .items_center()
                        .child(
                          div()
                            .flex_1()
                            .min_w(0)
                            .truncate()
                            .font_medium()
                            .text_size(12)
                            .child(sess.title)
                        )
                        .child(statusBadge(sess.status, t))
                    )
                    .when(Boolean(sess.state_short), (row) =>
                      row.child(
                        div()
                          .text_size(11)
                          .text_color(t.muted)
                          .truncate()
                          .child(sess.state_short)
                      )
                    )
                )
              )
          )
        );

    if (!coordinator.tabStreams.has(snap.activeTabId) && coordinator.inFlightKey !== null) {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(div().p(12).text_size(12).text_color(t.dim).child("Loading Streams…"));
    }
    const tabStreamId = coordinator.tabStreams.get(snap.activeTabId) ?? null;
    const tabStream = tabStreamId ? coordinator.streams.get(tabStreamId) ?? null : null;
    if (tabStream) coordinator.inspectedStreamId = null;
    // The enrolled Session leads the view only when it belongs to the tab's Stream.
    const viewing = tabStream && tabSession?.session?.stream === tabStream.id ? tabSession : null;

    if (!viewing) {
      const inspectedStream =
        tabStream ??
        (coordinator.inspectedStreamId
          ? coordinator.streams.get(coordinator.inspectedStreamId) ?? null
          : null);
      if (inspectedStream) {
        const monoFont = settings()?.font_family ?? "JetBrains Mono";
        const inspectedRefs = buildReferenceItems(inspectedStream, {
          viewingSessionId: inspectedStream.sessions?.[0]?.id ?? null,
        });
        const inspectedSessions = inspectedStream.sessions ?? [];
        const inspectedUnreadable = unreadableNotice(inspectedStream.unreadable);
        return v_flex()
          .size_full()
          .bg(t.chrome)
          .text_size(12)
          .text_color(t.foreground)
          .child(header)
          .child(
            renderErrorBanner(
              v_flex()
                .id("streams-sidebar-body")
                .flex_1()
                .p(12)
                .gap(12)
                .overflow_y_scrollbar()
                .track_scroll(this.scroll)
                .when(!tabStream, (el) =>
                  el.child(
                    h_flex()
                      .id("streams-back-list")
                      .self_start()
                      .px(6)
                      .py(3)
                      .rounded(4)
                      .text_size(11.5)
                      .text_color(t.muted)
                      .hover((s) => s.bg(t.hover).text_color(t.foreground))
                      .on_click((_e, clickCx) => {
                        clickCx.stop_propagation();
                        coordinator.inspectStream(null);
                      })
                      .child("← All Streams")
                  )
                )
                .when(Boolean(inspectedUnreadable), (el) =>
                  el.child(renderUnreadableNotice(inspectedUnreadable))
                )
                .child(
                  v_flex()
                    .gap(5)
                    .child(
                      h_flex()
                        .gap(6)
                        .items_center()
                        .child(div().flex_none().text_size(13).text_color(t.accent).child("≋"))
                        .child(
                          div()
                            .flex_1()
                            .min_w(0)
                            .truncate()
                            .font_semibold()
                            .text_size(13.5)
                            .child(inspectedStream.title)
                        )
                        .child(statusBadge(inspectedStream.status, t))
                    )
                    .when(Boolean(inspectedStream.goal), (el) =>
                      el.child(
                        div().text_size(11.5).text_color(t.muted).child(inspectedStream.goal)
                      )
                    )
                )
                .when(inspectedSessions.length > 0, (el) =>
                  el.child(
                    renderSessionListSection(
                      inspectedStream.id,
                      inspectedSessions,
                      `Sessions · ${inspectedSessions.length}`,
                      false
                    )
                  )
                )
                .when(inspectedRefs.length > 0, (el) =>
                  el.child(renderReferencesSection(inspectedRefs, monoFont))
                )
            )
          );
      }

      const streamList = coordinator.hostStreams ?? [];
      const unreadable = unreadableNotice(coordinator.hostUnreadable);
      const liveSet = liveStreamIds({
        panes: panes(),
        terminalList: snap.terminalList,
        frames: coordinator.frames,
      });
      const filteredStreams = filterHostStreams(streamList, {
        filter: this.streamFilter,
        sort: this.streamSort,
        query: this.streamQuery,
      });
      const emptyMessage = hostStreamsEmptyMessage({
        error: coordinator.hostStreamsError,
        streams: streamList,
        shown: filteredStreams,
        unreadable: coordinator.hostUnreadable,
      });
      const toolbar = v_flex()
        .px(12)
        .py(8)
        .gap(6)
        .border_b(1)
        .border_color(t.border)
        .when(Boolean(this.searchInput), (el) =>
          el.child(
            h_flex()
              .h(28)
              .px(8)
              .gap(4)
              .items_center()
              .rounded(5)
              .border(1)
              .border_color(t.border)
              .bg(t.background)
              .child(
                Input.new(this.searchInput)
                  .flex_1()
                  .h_full()
                  .text_size(12)
                  .text_color(t.foreground)
              )
              .when(Boolean(this.streamQuery), (bar) =>
                bar.child(
                  div()
                    .id("streams-search-clear")
                    .flex_none()
                    .px(4)
                    .py(1)
                    .rounded(3)
                    .text_size(11)
                    .text_color(t.muted)
                    .hover((s) => s.bg(t.hover).text_color(t.foreground))
                    .on_click((_e, clickCx) => {
                      clickCx.stop_propagation();
                      this.streamQuery = "";
                      this.searchInput?.set_value?.("");
                      clickCx.notify();
                    })
                    .child("×")
                )
              )
          )
        )
        .child(
          h_flex()
            .gap(4)
            .items_center()
            .flex_wrap()
            .children(
              HOST_STREAM_FILTERS.map((opt) => {
                const selected = this.streamFilter === opt.id;
                return div()
                  .id(`streams-filter-${opt.id}`)
                  .px(6)
                  .py(2)
                  .rounded(4)
                  .text_size(11)
                  .when(selected, (el) =>
                    el.bg(t.raised).font_medium().text_color(t.foreground)
                  )
                  .when(!selected, (el) =>
                    el.text_color(t.muted).hover((s) => s.bg(t.hover).text_color(t.foreground))
                  )
                  .on_click((_e, clickCx) => {
                    clickCx.stop_propagation();
                    this.streamFilter = opt.id;
                    clickCx.notify();
                  })
                  .child(opt.label);
              })
            )
        )
        .child(
          h_flex()
            .justify_between()
            .items_center()
            .text_size(11)
            .child(
              div()
                .text_color(t.dim)
                .child(
                  `${filteredStreams.length} stream${filteredStreams.length === 1 ? "" : "s"}`
                )
            )
            .child(
              h_flex()
                .gap(4)
                .items_center()
                .children(
                  HOST_STREAM_SORTS.map((opt) => {
                    const selected = this.streamSort === opt.id;
                    return div()
                      .id(`streams-sort-${opt.id}`)
                      .px(5)
                      .py(1)
                      .rounded(4)
                      .when(selected, (el) =>
                        el.bg(t.raised).font_medium().text_color(t.foreground)
                      )
                      .when(!selected, (el) =>
                        el.text_color(t.dim).hover((s) => s.bg(t.hover).text_color(t.foreground))
                      )
                      .on_click((_e, clickCx) => {
                        clickCx.stop_propagation();
                        this.streamSort = opt.id;
                        clickCx.notify();
                      })
                      .child(opt.label);
                  })
                )
            )
        );
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .text_size(12)
        .text_color(t.foreground)
        .child(header)
        .child(toolbar)
        .child(
          renderErrorBanner(
            v_flex()
              .id("streams-sidebar-body")
              .flex_1()
              .p(12)
              .gap(10)
              .overflow_y_scrollbar()
              .track_scroll(this.scroll)
          )
            .when(Boolean(unreadable), (el) => el.child(renderUnreadableNotice(unreadable)))
            .when(Boolean(emptyMessage), (el) =>
              el.child(div().text_size(12).text_color(t.dim).child(emptyMessage))
            )
            .when(filteredStreams.length > 0, (el) =>
              el.child(
                v_flex()
                  .rounded(6)
                  .bg(t.raised)
                  .border(1)
                  .border_color(t.border)
                  .children(
                    filteredStreams.map((summary, idx) => {
                      const isLive = liveSet.has(summary.id);
                      const age = formatRelativeAge(summary.last_updated_ms);
                      const sessionCountText =
                        summary.active_sessions > 0
                          ? `${summary.active_sessions} active`
                          : `${summary.total_sessions} session${summary.total_sessions === 1 ? "" : "s"}`;
                      return v_flex()
                        .id(`host-stream-${summary.id}`)
                        .px(9)
                        .py(8)
                        .gap(3)
                        .when(idx > 0, (row) => row.border_t(1).border_color(t.border))
                        .hover((s) => s.bg(t.hover))
                        .on_click((_e, clickCx) => {
                          clickCx.spawn(async (asyncCx) => {
                            if (isLive || summary.active_sessions > 0) {
                              await coordinator.openWork(
                                {
                                  stream: summary.id,
                                  sessions: { kind: "all_eligible" },
                                  cwd: null,
                                },
                                {
                                  run,
                                  openUrl: (u) => asyncCx.open_url(u),
                                }
                              );
                            } else {
                              await coordinator.inspectStream(summary.id);
                            }
                          });
                        })
                        .child(
                          h_flex()
                            .gap(6)
                            .items_center()
                            .child(
                              div()
                                .flex_none()
                                .text_size(12)
                                .text_color(t.accent)
                                .child("≋")
                            )
                            .child(
                              div()
                                .flex_1()
                                .min_w(0)
                                .truncate()
                                .font_medium()
                                .text_size(12)
                                .child(summary.title)
                            )
                            .when(isLive, (row) =>
                              row.child(
                                h_flex()
                                  .flex_none()
                                  .gap(3)
                                  .items_center()
                                  .text_size(10.5)
                                  .font_medium()
                                  .text_color(t.ansi2)
                                  .child(div().text_size(8).child("●"))
                                  .child("live")
                              )
                            )
                            .when(summary.status !== "active", (row) =>
                              row.child(statusBadge(summary.status, t))
                            )
                            .when(Boolean(age), (row) =>
                              row.child(
                                div()
                                  .flex_none()
                                  .text_size(10.5)
                                  .text_color(t.dim)
                                  .child(age)
                              )
                            )
                        )
                        .child(
                          h_flex()
                            .gap(6)
                            .items_center()
                            .text_size(11)
                            .text_color(t.dim)
                            .child(sessionCountText)
                            .when(Boolean(summary.goal_short), (row) =>
                              row
                                .child("·")
                                .child(div().flex_1().min_w(0).truncate().child(summary.goal_short))
                            )
                        );
                    })
                  )
              )
            )
        );
    }

    const enrolled = viewing.session;
    const stream = tabStream;
    const tabPane = panes().find((p) => p.terminal === viewing.terminal) ?? null;
    const termInfo =
      snap.terminalList.find((item) => (item.address ?? item.id) === viewing.terminal) ?? null;
    const liveAgent = agents().find((a) => a.address === viewing.terminal) ?? null;
    const effectiveStatus = effectiveSessionStatus(enrolled, {
      pane: tabPane,
      terminal: termInfo,
      agent: liveAgent,
    });
    const tabIcon = icon(tabPane?.program, 13, t.muted);
    const monoFont = settings()?.font_family ?? "JetBrains Mono";
    const sessionPrimary =
      effectiveStatus === "done"
        ? enrolled.recap || enrolled.state
        : enrolled.state || enrolled.recap;
    const references = buildReferenceItems(stream, {
      viewingSessionId: enrolled.id,
    });
    const otherSessions = (stream.sessions ?? []).filter((s) => s.id !== enrolled.id);
    const streamUnreadable = unreadableNotice(stream.unreadable);

    return v_flex()
      .size_full()
      .bg(t.chrome)
      .text_size(12)
      .text_color(t.foreground)
      .child(header)
      .child(
        renderErrorBanner(
          v_flex()
            .id("streams-sidebar-body")
            .flex_1()
            .p(12)
            .gap(12)
            .overflow_y_scrollbar()
            .track_scroll(this.scroll)
        )
          .when(Boolean(streamUnreadable), (el) => el.child(renderUnreadableNotice(streamUnreadable)))
          .child(
            v_flex()
              .gap(8)
              .child(
                v_flex()
                  .gap(5)
                  .child(
                    h_flex()
                      .gap(6)
                      .items_center()
                      .child(div().flex_none().text_size(13).text_color(t.accent).child("≋"))
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
                    el.child(div().text_size(11.5).text_color(t.muted).child(stream.goal))
                  )
              )
              .child(
                h_flex()
                  .pl(6)
                  .gap(8)
                  .items_stretch()
                  .child(div().w(2).flex_none().rounded(1).bg(t.border))
                  .child(
                    v_flex()
                      .flex_1()
                      .min_w(0)
                      .py(2)
                      .gap(3)
                      .child(
                        h_flex()
                          .gap(6)
                          .items_center()
                          .when(Boolean(tabIcon), (el) => el.child(tabIcon))
                          .child(
                            div()
                              .flex_1()
                              .min_w(0)
                              .truncate()
                              .font_medium()
                              .child(enrolled.title)
                          )
                          .child(statusBadge(effectiveStatus, t))
                      )
                      .when(Boolean(sessionPrimary), (el) =>
                        el.child(
                          div()
                            .text_size(11.5)
                            .text_color(t.muted)
                            .child(sessionPrimary)
                        )
                      )
                      .when(Boolean(enrolled.remaining), (el) =>
                        el.child(
                          div()
                            .text_size(11)
                            .text_color(t.dim)
                            .child(`→ ${enrolled.remaining}`)
                        )
                      )
                  )
              )
          )
          .when(references.length > 0, (el) =>
            el.child(renderReferencesSection(references, monoFont))
          )
          .when(otherSessions.length > 0, (el) =>
            el.child(
              renderSessionListSection(
                stream.id,
                otherSessions,
                `Other sessions · ${otherSessions.length}`,
                true
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
    const frameHeader = paneFrame(layout())?.header;
    const hasOuterPaneHeader =
      !overlayId && Boolean(frameHeader && frameHeader.reveal !== "none");
    const openInEditorButton = (id) =>
      div()
        .id(id)
        .flex_none()
        .px(6)
        .py(1)
        .rounded(4)
        .text_size(10.5)
        .text_color(t.accent)
        .hover((s) => s.bg(t.hover))
        .on_mouse_down((_e, downCx) => downCx.stop_propagation())
        .on_click((_e, clickCx) => {
          clickCx.stop_propagation();
          const snap = collectSnapshot();
          const focusNow = focused();
          const targetPaneId =
            rawProps.targetPaneId ??
            focusNow?.pane ??
            panes().find((p) => p.tab === snap.activeTabId)?.pane ??
            null;
          if (overlayId) closeOverlay(overlayId);
          clickCx.spawn(async (asyncCx) => {
            await coordinator.openReference(rawProps.reference, {
              activeSpaceId: rawProps.spaceId ?? snap.activeSpace?.id ?? null,
              activeTabId: rawProps.tabId ?? snap.activeTabId ?? null,
              targetPaneId,
              mode: "editor",
              ...(overlayId ? { placement: "pane-right" } : {}),
              run,
              openOverlay,
              openUrl: (u) => asyncCx.open_url(u),
            });
          });
        })
        .child("Open in editor");

    return v_flex()
      .size_full()
      .bg(t.background)
      .text_size(12)
      .text_color(t.foreground)
      .when(!hasOuterPaneHeader, (el) =>
        el.child(
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
            .when(Boolean(rawProps.truncated), (bar) =>
              bar
                .child(
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
                .when(Boolean(rawProps.reference), (b) =>
                  b.child(openInEditorButton("streams-viewer-open-editor"))
                )
            )
            .when(Boolean(overlayId), (bar) =>
              bar.child(
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
      )
      .when(hasOuterPaneHeader && Boolean(rawProps.truncated), (el) =>
        el.child(
          h_flex()
            .px(12)
            .py(5)
            .gap(8)
            .items_center()
            .bg(t.chrome)
            .border_b(1)
            .border_color(t.border)
            .text_size(11)
            .text_color(t.muted)
            .child(div().flex_1().child("Showing first 400 KiB (truncated)."))
            .when(Boolean(rawProps.reference), (b) =>
              b.child(openInEditorButton("streams-viewer-banner-open-editor"))
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
    if (role === "popup") return this.renderPopup(cx);
    this.ensureSubscribed(cx);
    if (role === "frame") return this.renderFrame(cx);
    return this.renderSidebar(cx);
  }
}
