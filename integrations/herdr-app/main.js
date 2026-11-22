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
  setShared,
  notify as herdrNotify,
} from "herdr";
import {
  LABEL_SAVED,
  STALE_FALLBACK_MS,
  SESSION_STATUSES,
  resolveRole,
  observeNoticeToken,
  resolveSpaceStreamKey,
  resolveSidebarView,
  effectiveSessionStatus,
  resolveTabRenameTarget,
  buildReferenceItems,
  groupHostStreams,
  unreadableNotice,
  hostStreamsEmptyMessage,
  formatRelativeAge,
  countLiveTerminalsByStream,
  findLivePaneForSession,
  runningSessionTerminals,
  streamBulkActionState,
  groupStreamSessions,
  StreamsCoordinator,
} from "./state.js";

const pendingSidebarActions = [];
let sidebarActionSeq = 0;
let viewRev = 0;
let lastWindowActive = false;

function bumpViewRev() {
  if (checking()) return;
  try {
    setShared("streams.viewRev", ++viewRev);
  } catch {
    // Non-fatal in check/test mode
  }
}

function dispatchSidebarAction(fn) {
  pendingSidebarActions.push(fn);
  if (checking()) return;
  try {
    setShared("streams.actionSeq", ++sidebarActionSeq);
  } catch {
    // Non-fatal in check/test mode
  }
}

const coordinator = new StreamsCoordinator({
  exec: (cmd, args) => exec(cmd, args, { timeout: 30000 }),
  publishShared: (key, value) => setShared(key, value),
  notifyToast: (n) => herdrNotify(n),
  homeDir: (() => {
    try {
      return home();
    } catch {
      return null;
    }
  })(),
  notify: () => {
    bumpViewRev();
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
  const allAgents = agents();
  return {
    allSpaces,
    activeSpace,
    activeTab,
    activeTabId: activeTab?.id ?? null,
    tabIds: allSpaces.flatMap((space) => (space.tabs ?? []).map((tab) => tab.id)),
    visibleTerminals,
    liveTerminals,
    panes: allPanes,
    agents: allAgents,
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
  const running = [...runningSessionTerminals(snap).entries()]
    .map(([t, s]) => `${t}=${s}`)
    .sort();
  return JSON.stringify({
    space: snap.activeSpace?.id ?? null,
    machine: snap.activeSpace?.machine ?? null,
    tab: snap.activeTabId,
    tabName: snap.activeTab?.name ?? null,
    terminals: snap.visibleTerminals,
    liveTerminals: snap.liveTerminals,
    running,
    focused: snap.focusedTerminal,
    notice: observeNoticeToken(snap.terminalList, LABEL_SAVED),
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
    this.referencesOpen = true;
    this.sessionsOpen = true;
    this.sessionGroupsOpen = {
      open: true,
      not_open: true,
      done_closed: null,
    };
    this.allGroupsOpen = {
      open: true,
      not_open: true,
      done_closed: null,
    };
    this.hoveredSessionKey = null;
    this.sessionSort = "recent";
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
    const rawProps = props() ?? initialProps ?? {};
    const role = resolveRole(rawProps);
    this._role = role;
    if (role === "sidebar") {
      this.searchInput = InputState.new({ placeholder: "Filter streams…" });
      this.searchInput.on("change", (_e, changeCx) => {
        this.streamQuery = this.searchInput.value();
        changeCx.notify();
      });
    }
    if (role === "viewer") {
      this.syncViewerState(rawProps);
      return;
    }
    if (role !== "sidebar" && role !== "frame") {
      return;
    }
    if (checking()) {
      return;
    }
    this.syncConfiguredBin();
    this.scheduleObserve(cx, collectSnapshot());
    if (role === "sidebar" && cx?.timer?.every) {
      this._timerTask = cx.timer.every(STALE_FALLBACK_MS, (timerCx) => {
        this.scheduleObserve(timerCx, collectSnapshot(), true);
      });
    }
  }

  async close() {
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

  syncConfiguredBin() {
    const configuredBin = props()?.streams_bin;
    if (configuredBin && coordinator.streamsBin !== configuredBin) {
      coordinator.streamsBin = configuredBin;
    }
  }

  scheduleObserve(cx, snap, force = false) {
    if (checking()) return;
    this.syncConfiguredBin();
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
        coordinator.lastError ||
        coordinator.lastCompletedKey !== null ||
        coordinator.lastFailedKey !== null ||
        coordinator.inFlightKey !== null)
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
    const tabPaneCount = snap.panes.filter((paneItem) => paneItem.tab === p.tab).length;

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
      .when(Boolean(enrolled) && tabPaneCount > 1, (el) =>
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
    const closeSelf = () => {
      if (rawProps.overlay) closeOverlay(rawProps.overlay);
    };
    const menuRow = ({ id, label, disabled = false, danger = false, checked = false, onClick }) =>
      h_flex()
        .id(id)
        .h(28)
        .px(10)
        .gap(8)
        .items_center()
        .justify_between()
        .rounded(5)
        .text_size(12)
        .when(disabled, (el) => el.text_color(t.dim))
        .when(!disabled, (el) =>
          el
            .text_color(danger ? t.blocked : t.foreground)
            .hover((s) => s.bg(t.hover))
            .on_click((e, clickCx) => {
              clickCx.stop_propagation();
              try {
                onClick?.(e, clickCx);
              } finally {
                closeSelf();
              }
            })
        )
        .child(div().flex_1().min_w(0).truncate().child(label))
        .when(checked, (el) => el.child(div().flex_none().text_size(11).text_color(t.dim).child("✓")));

    const menuDivider = () => div().my(3).h(1).w_full().bg(t.border);
    const menuLabel = (text) =>
      div().px(10).py(3).text_size(10.5).font_medium().text_color(t.dim).child(text);

    if (rawProps.kind === "stream-menu") {
      const streamKey = rawProps.stream_id;
      const stream = streamKey ? coordinator.streams.get(streamKey) ?? null : null;
      const bulk = streamBulkActionState(stream);
      return v_flex()
        .w_full()
        .p(5)
        .bg(t.popover)
        .text_color(t.foreground)
        .child(
          menuRow({
            id: "stream-menu-open-active",
            label: "Open all active sessions",
            disabled: !bulk.canOpenActive,
            onClick: () => {
              if (!streamKey) return;
              const snap = collectSnapshot();
              dispatchSidebarAction(async () => {
                await coordinator.openWork(
                  {
                    stream: streamKey,
                    sessions: { kind: "only_active" },
                    terminalList: snap.terminalList,
                    panes: snap.panes,
                    agents: snap.agents,
                  },
                  { run }
                );
              });
            },
          })
        )
        .child(menuDivider())
        .child(
          menuRow({
            id: "stream-menu-park-all",
            label: "Park all",
            disabled: !bulk.canParkAll,
            onClick: () => {
              if (!streamKey) return;
              dispatchSidebarAction(async () => {
                await coordinator.parkAllSessions(streamKey);
              });
            },
          })
        )
        .child(
          menuRow({
            id: "stream-menu-close-all",
            label: "Close all",
            disabled: !bulk.canCloseAll,
            danger: bulk.canCloseAll,
            onClick: () => {
              if (!streamKey) return;
              dispatchSidebarAction(async () => {
                await coordinator.closeAllSessions(streamKey);
              });
            },
          })
        )
        .child(menuDivider())
        .child(
          menuRow({
            id: "stream-menu-delete",
            label: "Delete empty stream",
            disabled: !bulk.canDelete,
            danger: bulk.canDelete,
            onClick: () => {
              if (!streamKey) return;
              dispatchSidebarAction(async () => {
                await coordinator.deleteStream(streamKey);
              });
            },
          })
        );
    }

    if (rawProps.kind === "session-menu") {
      const sessionKey = rawProps.session_key;
      const streamKey = rawProps.stream_key ?? null;
      const currentStatus = rawProps.status ?? null;
      const livePane = rawProps.live_pane ?? null;
      const isOpen = Boolean(livePane);
      const fromHeader = Boolean(rawProps.from_header);
      const statusLabel = (s) => s.charAt(0).toUpperCase() + s.slice(1);
      return v_flex()
        .w_full()
        .p(5)
        .bg(t.popover)
        .text_color(t.foreground)
        .when(isOpen && !fromHeader, (el) =>
          el
            .child(
              menuRow({
                id: "session-menu-switch",
                label: "Switch to",
                onClick: () => {
                  const snap = collectSnapshot();
                  dispatchSidebarAction(async () => {
                    await coordinator.selectSessionFromStream(sessionKey, streamKey, snap, { run });
                  });
                },
              })
            )
            .child(
              menuRow({
                id: "session-menu-close-term",
                label: "Close terminal",
                onClick: () => {
                  run("pane.close", { pane: livePane });
                },
              })
            )
            .child(menuDivider())
        )
        .when(!isOpen, (el) =>
          el
            .child(
              menuRow({
                id: "session-menu-resume",
                label: "Resume",
                onClick: () => {
                  if (!streamKey || !sessionKey) return;
                  const snap = collectSnapshot();
                  dispatchSidebarAction(async () => {
                    await coordinator.openWork(
                      {
                        stream: streamKey,
                        sessions: { kind: "selected", session_ids: [sessionKey] },
                        terminalList: snap.terminalList,
                        panes: snap.panes,
                        agents: snap.agents,
                      },
                      { run }
                    );
                  });
                },
              })
            )
            .when(!fromHeader, (m) =>
              m.child(
                menuRow({
                  id: "session-menu-inspect",
                  label: "Inspect",
                  onClick: () => {
                    dispatchSidebarAction(async () => {
                      await coordinator.previewSession(sessionKey, streamKey);
                    });
                  },
                })
              )
            )
            .child(menuDivider())
        )
        .child(menuLabel("Set status"))
        .children(
          SESSION_STATUSES.map((st) =>
            menuRow({
              id: `session-menu-status-${st}`,
              label: statusLabel(st),
              disabled: currentStatus === st,
              checked: currentStatus === st,
              onClick: () => {
                if (!sessionKey) return;
                dispatchSidebarAction(async () => {
                  await coordinator.setSessionStatus(sessionKey, st, streamKey);
                });
              },
            })
          )
        );
    }

    if (rawProps.kind === "reference-menu") {
      const item = rawProps.item;
      if (!item) return div().p(10).text_color(t.dim).child("No reference.");
      const isDoc = item.kind === "document";
      const isArtifact = item.kind === "artifact";
      const copyLabel = isDoc ? "Copy path" : isArtifact ? "Copy artifact key" : "Copy URL";
      return v_flex()
        .w_full()
        .p(5)
        .bg(t.popover)
        .text_color(t.foreground)
        .when(isDoc, (el) =>
          el.child(
            menuRow({
              id: "ref-menu-open-editor",
              label: "Open in editor",
              disabled: Boolean(item.disabled),
              onClick: () => {
                const snap = collectSnapshot();
                const focusNow = focused();
                const targetPaneId =
                  focusNow?.pane ??
                  snap.panes.find((p) => p.tab === snap.activeTabId && p.focused)?.pane ??
                  snap.panes.find((p) => p.tab === snap.activeTabId)?.pane ??
                  null;
                dispatchSidebarAction(async (asyncCx) => {
                  await coordinator.openReference(item.reference, {
                    activeSpaceId: snap.activeSpace?.id ?? null,
                    activeTabId: snap.activeTabId,
                    targetPaneId,
                    mode: "editor",
                    run,
                    openOverlay,
                    openUrl: (u) => asyncCx?.open_url?.(u),
                  });
                });
              },
            })
          )
        )
        .child(
          menuRow({
            id: "ref-menu-copy",
            label: copyLabel,
            disabled: !item.copyText,
            onClick: (_e, clickCx) => {
              if (item.copyText) {
                clickCx.write_to_clipboard(item.copyText);
              }
            },
          })
        )
        .when(!isArtifact, (el) =>
          el
            .child(menuDivider())
            .child(
              menuRow({
                id: "ref-menu-drop",
                label: "Drop",
                danger: true,
                disabled: !item.sessionKey || !item.refKey,
                onClick: () => {
                  if (!item.sessionKey || !item.refKey) return;
                  dispatchSidebarAction(async () => {
                    await coordinator.dropReference(item.sessionKey, item.refKey, item.streamKey ?? null);
                  });
                },
              })
            )
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
    if (pendingSidebarActions.length > 0 && cx?.spawn) {
      const batch = pendingSidebarActions.splice(0);
      cx.spawn(async (asyncCx) => {
        for (const fn of batch) {
          try {
            await fn(asyncCx);
          } finally {
            asyncCx.notify();
          }
        }
      });
    }
    const t = theme();
    const snap = collectSnapshot();
    this.scheduleObserve(cx, snap);

    const activeSpace = snap.activeSpace;
    const monoFont = settings()?.font_family ?? "JetBrains Mono";
    const spaceStreamKey = resolveSpaceStreamKey({
      space: activeSpace,
      panes: snap.panes,
      terminalList: snap.terminalList,
      agents: snap.agents,
      frames: coordinator.frames,
      lastFocusedStreamBySpace: coordinator.lastFocusedStreamBySpace,
    });
    const view = resolveSidebarView({
      preferredView: coordinator.preferredView,
      tabId: snap.activeTabId,
      tabStreams: coordinator.tabStreams,
      streams: coordinator.streams,
      previewSessions: coordinator.previewSessions,
      frames: coordinator.frames,
      visibleTerminals: snap.visibleTerminals,
      focusedTerminal: snap.focusedTerminal,
      spaceStreamKey,
      previewStreamKey: coordinator.previewStreamKey,
      previewSessionKey: coordinator.previewSessionKey,
      inFlight: coordinator.inFlightKey !== null,
    });

    const activeSegment =
      view.view === "session" || view.view === "stream" || view.view === "all"
        ? view.view
        : view.hasSessionAccess
          ? "session"
          : view.hasStreamAccess
            ? "stream"
            : "all";
    const sessionDisabled = !view.hasSessionAccess;
    const streamDisabled = !view.hasStreamAccess;

    const navButton = (id, label, targetView, disabled = false, disabledTooltip = null) => {
      const selected = activeSegment === targetView;
      const btn = div()
        .id(id)
        .px(7)
        .py(3)
        .rounded(4)
        .text_size(11.5)
        .when(selected, (el) => el.bg(t.raised).font_medium().text_color(t.foreground))
        .when(!selected && disabled, (el) => el.text_color(t.dim))
        .when(!selected && !disabled, (el) =>
          el.text_color(t.muted).hover((s) => s.bg(t.hover).text_color(t.foreground))
        );
      if (disabled) {
        if (disabledTooltip) btn.tooltip(disabledTooltip);
        return btn.child(label);
      }
      return btn
        .on_click((_e, clickCx) => {
          clickCx.stop_propagation();
          clickCx.spawn(async () => {
            await coordinator.setView(targetView);
          });
        })
        .child(label);
    };

    const header = h_flex()
      .h(34)
      .px(10)
      .gap(6)
      .justify_between()
      .items_center()
      .border_b(1)
      .border_color(t.border)
      .child(
        h_flex()
          .gap(2)
          .items_center()
          .child(
            navButton(
              "streams-nav-session",
              "Session",
              "session",
              sessionDisabled,
              "Tab not enrolled"
            )
          )
          .child(
            navButton(
              "streams-nav-stream",
              "Stream",
              "stream",
              streamDisabled,
              "No stream in space"
            )
          )
          .child(navButton("streams-nav-all", "All", "all", false))
      )
      .child(
        div()
          .id("streams-refresh")
          .flex_none()
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

    if (coordinator.readError && !coordinator.actionError && coordinator.frames.size === 0 && coordinator.hostStreams.length === 0) {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(
          div()
            .p(12)
            .text_size(12)
            .text_color(t.dim)
            .child(`Unavailable: ${coordinator.readError}`)
        );
    }

    if (!activeSpace || !snap.activeTabId) {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(div().p(12).text_size(12).text_color(t.dim).child("No active tab."));
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

    if (view.view === "loading" || (coordinator.lastCompletedKey === null && coordinator.inFlightKey !== null)) {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(div().p(12).text_size(12).text_color(t.dim).child("Loading Streams…"));
    }

    const activeBanner = coordinator.actionError
      ? {
          kind: "action",
          title: coordinator.actionError.title,
          message: coordinator.actionError.message,
          copyText: coordinator.actionError.copyText ?? null,
          copyLabel: coordinator.actionError.copyLabel ?? "Copy",
          color: coordinator.actionError.level === "warning" ? t.working : t.blocked,
        }
      : coordinator.readError
        ? {
            kind: "read",
            title: "Refresh failed",
            message: coordinator.readError,
            copyText: null,
            copyLabel: "Copy",
            color: t.blocked,
          }
        : null;

    const renderPinnedErrorBanner = (parent) =>
      parent.when(Boolean(activeBanner), (el) => {
        const copyPayload =
          activeBanner.copyText ??
          (activeBanner.title
            ? `${activeBanner.title}: ${activeBanner.message}`
            : activeBanner.message);
        return el.child(
          v_flex()
            .flex_none()
            .mx(10)
            .mt(8)
            .p(8)
            .gap(4)
            .rounded(5)
            .bg(t.raised)
            .border(1)
            .border_color(activeBanner.color)
            .text_size(11.5)
            .child(
              h_flex()
                .gap(6)
                .items_center()
                .justify_between()
                .child(
                  div()
                    .flex_1()
                    .min_w(0)
                    .font_semibold()
                    .text_color(activeBanner.color)
                    .child(activeBanner.title)
                )
                .child(
                  div()
                    .id("streams-error-copy")
                    .flex_none()
                    .px(5)
                    .py(1)
                    .rounded(4)
                    .text_size(10.5)
                    .text_color(t.muted)
                    .hover((s) => s.bg(t.hover).text_color(t.foreground))
                    .on_click((_e, clickCx) => {
                      clickCx.stop_propagation();
                      clickCx.write_to_clipboard(copyPayload);
                    })
                    .child(activeBanner.copyLabel)
                )
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
                      if (activeBanner.kind === "action") {
                        coordinator.clearActionError();
                      } else {
                        coordinator.clearReadError();
                      }
                    })
                    .child("×")
                )
            )
            .child(div().text_size(11).text_color(t.foreground).child(activeBanner.message))
        );
      });

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

    const openReferenceMenu = (item) => {
      const height = item.kind === "document" ? 104 : item.kind === "artifact" ? 40 : 76;
      openOverlay({
        mod: "streams",
        kind: "popup",
        at: "pointer",
        width: 185,
        height,
        props: {
          kind: "reference-menu",
          item,
        },
      });
    };

    const openSessionMenu = ({ sessionKey, streamKey, status, livePane = null, fromHeader = false }) => {
      const topRows = livePane ? (fromHeader ? 0 : 2) : fromHeader ? 1 : 2;
      const height = 10 + (topRows > 0 ? topRows * 28 + 7 : 0) + 22 + SESSION_STATUSES.length * 28;
      openOverlay({
        mod: "streams",
        kind: "popup",
        at: "pointer",
        width: 190,
        height,
        props: {
          kind: "session-menu",
          session_key: sessionKey,
          stream_key: streamKey,
          status,
          live_pane: livePane,
          from_header: fromHeader,
        },
      });
    };

    const openStreamMenu = (streamKey) => {
      openOverlay({
        mod: "streams",
        kind: "popup",
        at: "pointer",
        width: 215,
        height: 138,
        props: {
          kind: "stream-menu",
          stream_id: streamKey,
        },
      });
    };

    const resumeSingleSession = (clickCx, streamKey, sessionKey) => {
      if (!streamKey || !sessionKey) return;
      const snapNow = collectSnapshot();
      const work = async () => {
        await coordinator.openWork(
          {
            stream: streamKey,
            sessions: { kind: "selected", session_ids: [sessionKey] },
            terminalList: snapNow.terminalList,
            panes: snapNow.panes,
            agents: snapNow.agents,
          },
          { run }
        );
      };
      if (clickCx?.spawn) {
        clickCx.spawn(async (asyncCx) => {
          try {
            await work();
          } finally {
            asyncCx.notify();
          }
        });
      } else {
        dispatchSidebarAction(work);
      }
    };

    const renderReferencesSection = (references) =>
      v_flex()
        .pt(4)
        .gap(6)
        .border_t(1)
        .border_color(t.border)
        .child(
          h_flex()
            .id("streams-references-toggle")
            .justify_between()
            .items_center()
            .hover((s) => s.text_color(t.foreground))
            .on_click((_e, clickCx) => {
              clickCx.stop_propagation();
              this.referencesOpen = !this.referencesOpen;
              clickCx.notify();
            })
            .child(
              div()
                .font_medium()
                .text_size(11.5)
                .text_color(t.muted)
                .child(`References · ${references.length}`)
            )
            .child(
              div()
                .text_size(11)
                .text_color(t.dim)
                .child(this.referencesOpen ? "▾" : "▸")
            )
        )
        .when(this.referencesOpen, (el) =>
          el.child(
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
                    .on_mouse_down("right", (_e, downCx) => {
                      downCx.stop_propagation();
                      openReferenceMenu(item);
                    })
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
                            allPanesNow.find((p) => p.tab === clickedTabId && p.focused)?.pane ??
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
                        .gap(6)
                        .items_center()
                        .child(referenceIcon(item.kind, t))
                        .child(
                          div()
                            .flex_1()
                            .min_w(0)
                            .font_medium()
                            .text_size(12)
                            .truncate()
                            .text_color(item.disabled ? t.dim : t.foreground)
                            .when(item.monoTitle, (titleEl) =>
                              titleEl.font_family(monoFont).text_size(11.5)
                            )
                            .child(item.title)
                        )
                        .when(item.external, (extEl) =>
                          extEl.child(
                            div().flex_none().text_size(11).text_color(t.dim).child("↗")
                          )
                        )
                        .child(
                          div()
                            .id(`ref-menu-btn-${idx}`)
                            .flex_none()
                            .px(4)
                            .py(1)
                            .rounded(4)
                            .text_color(t.dim)
                            .hover((s) => s.bg(t.hover).text_color(t.foreground))
                            .on_mouse_down("left", (_e, downCx) => downCx.stop_propagation())
                            .on_click((_e, clickCx) => {
                              clickCx.stop_propagation();
                              openReferenceMenu(item);
                            })
                            .child("⋮")
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
                    .when(Boolean(item.unavailableReason), (row) =>
                      row.child(
                        div().text_size(11).text_color(t.dim).child(item.unavailableReason)
                      )
                    )
                    .when(Boolean(item.description), (row) =>
                      row.child(
                        div().text_size(11).text_color(t.muted).child(item.description)
                      )
                    )
                )
              )
          )
        );

    // -------------------------------------------------------------------------
    // ALL VIEW
    // -------------------------------------------------------------------------
    if (view.view === "all") {
      const streamList = coordinator.hostStreams ?? [];
      const unreadable = unreadableNotice(coordinator.hostUnreadable);
      const openCounts = countLiveTerminalsByStream({
        panes: snap.panes,
        terminalList: snap.terminalList,
        agents: snap.agents,
        frames: coordinator.frames,
      });
      const streamGroups = groupHostStreams(streamList, {
        openCounts,
        sort: this.streamSort,
        query: this.streamQuery,
      });
      const nonEmptyStreamGroups = streamGroups.filter((g) => g.streams.length > 0);
      const shownStreams = nonEmptyStreamGroups.flatMap((g) => g.streams);
      const hasQuery = Boolean(this.streamQuery.trim());
      const onlyFinishedStreams =
        nonEmptyStreamGroups.length === 1 && nonEmptyStreamGroups[0].id === "done_closed";
      const emptyMessage = hostStreamsEmptyMessage({
        error: coordinator.hostStreamsError,
        streams: streamList,
        shown: shownStreams,
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
            .justify_between()
            .items_center()
            .text_size(11)
            .child(
              div()
                .text_color(t.dim)
                .child(`${shownStreams.length} stream${shownStreams.length === 1 ? "" : "s"}`)
            )
            .child(
              div()
                .id("streams-sort-toggle")
                .px(6)
                .py(2)
                .rounded(4)
                .text_color(t.muted)
                .hover((s) => s.bg(t.hover).text_color(t.foreground))
                .tooltip(
                  this.streamSort === "recent"
                    ? "Sorted by most recent — click for A–Z"
                    : "Sorted A–Z — click for most recent"
                )
                .on_click((_e, clickCx) => {
                  clickCx.stop_propagation();
                  this.streamSort = this.streamSort === "recent" ? "alpha" : "recent";
                  clickCx.notify();
                })
                .child(this.streamSort === "recent" ? "↕ Recent" : "↕ A–Z")
            )
        );

      const renderStreamRow = (summary, idx) => {
        const openCount = openCounts.get(summary.key) ?? 0;
        const age = formatRelativeAge(summary.last_activity_ms);
        const openText = openCount > 0 ? `${openCount} open` : "none open";
        const metaLine = `${summary.active_sessions} active · ${openText}`;
        return v_flex()
          .id(`host-stream-${summary.id}`)
          .px(9)
          .py(8)
          .gap(3)
          .when(idx > 0, (row) => row.border_t(1).border_color(t.border))
          .hover((s) => s.bg(t.hover))
          .on_click((_e, clickCx) => {
            clickCx.spawn(async () => {
              await coordinator.selectStreamFromAll(
                summary.key,
                {
                  spaces: snap.allSpaces,
                  panes: snap.panes,
                  terminalList: snap.terminalList,
                  agents: snap.agents,
                },
                { run }
              );
            });
          })
          .child(
            h_flex()
              .gap(6)
              .items_center()
              .child(div().flex_none().text_size(12).text_color(t.accent).child("≋"))
              .child(
                div()
                  .flex_1()
                  .min_w(0)
                  .truncate()
                  .font_medium()
                  .text_size(12)
                  .text_color(openCount > 0 ? t.foreground : t.muted)
                  .child(summary.title)
              )
              .child(statusBadge(summary.status, t))
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
              .child(metaLine)
              .when(Boolean(summary.goal_short), (row) =>
                row
                  .child("·")
                  .child(div().flex_1().min_w(0).truncate().child(summary.goal_short))
              )
          );
      };

      return renderPinnedErrorBanner(
        v_flex()
          .size_full()
          .bg(t.chrome)
          .text_size(12)
          .text_color(t.foreground)
          .child(header)
          .child(toolbar)
      )
        .child(
          v_flex()
            .id("streams-sidebar-body")
            .flex_1()
            .p(12)
            .gap(10)
            .overflow_y_scrollbar()
            .track_scroll(this.scroll)
            .when(Boolean(unreadable), (el) => el.child(renderUnreadableNotice(unreadable)))
            .when(Boolean(emptyMessage), (el) =>
              el.child(div().text_size(12).text_color(t.dim).child(emptyMessage))
            )
            .when(nonEmptyStreamGroups.length > 0, (el) =>
              el.child(
                v_flex()
                  .gap(8)
                  .children(
                    nonEmptyStreamGroups.map((group) => {
                      const savedState = this.allGroupsOpen?.[group.id];
                      const isExpanded =
                        typeof savedState === "boolean"
                          ? savedState
                          : group.defaultOpen || onlyFinishedStreams || hasQuery;
                      return v_flex()
                        .gap(4)
                        .child(
                          h_flex()
                            .id(`streams-all-group-${group.id}`)
                            .px(2)
                            .py(1)
                            .gap(5)
                            .items_center()
                            .justify_between()
                            .rounded(4)
                            .text_size(11)
                            .text_color(t.dim)
                            .hover((s) => s.bg(t.hover).text_color(t.foreground))
                            .on_click((_e, clickCx) => {
                              clickCx.stop_propagation();
                              this.allGroupsOpen[group.id] = !isExpanded;
                              clickCx.notify();
                            })
                            .child(
                              h_flex()
                                .gap(5)
                                .items_center()
                                .child(div().text_size(10).child(isExpanded ? "▾" : "▸"))
                                .child(div().font_medium().child(group.label))
                            )
                            .child(div().text_size(10.5).child(`${group.streams.length}`))
                        )
                        .when(isExpanded, (sec) =>
                          sec.child(
                            v_flex()
                              .rounded(6)
                              .bg(t.raised)
                              .border(1)
                              .border_color(t.border)
                              .children(group.streams.map(renderStreamRow))
                          )
                        );
                    })
                  )
              )
            )
        );
    }

    // -------------------------------------------------------------------------
    // STREAM VIEW
    // -------------------------------------------------------------------------
    if (view.view === "stream") {
      const stream = view.stream ?? null;
      if (!stream) {
        return v_flex()
          .size_full()
          .bg(t.chrome)
          .text_size(12)
          .text_color(t.foreground)
          .child(header)
          .child(
            div()
              .p(12)
              .text_color(t.dim)
              .child(
                coordinator.previewStreamKey
                  ? "Loading Stream…"
                  : "This tab has no enrolled Stream. Switch to All to browse Streams."
              )
          );
      }

      const streamSessions = stream.sessions ?? [];
      const streamUnreadable = unreadableNotice(stream.unreadable);
      const currentSessKey = view.currentSession?.key ?? null;

      return renderPinnedErrorBanner(
        v_flex()
          .size_full()
          .bg(t.chrome)
          .text_size(12)
          .text_color(t.foreground)
          .child(header)
      )
        .child(
          v_flex()
            .id("streams-sidebar-body")
            .flex_1()
            .p(12)
            .gap(12)
            .overflow_y_scrollbar()
            .track_scroll(this.scroll)
            .when(view.isPreview, (el) =>
              el.child(
                h_flex()
                  .px(8)
                  .py(6)
                  .gap(8)
                  .items_center()
                  .justify_between()
                  .rounded(5)
                  .bg(t.raised)
                  .border(1)
                  .border_color(t.border)
                  .text_size(11.5)
                  .child(
                    div()
                      .id("streams-preview-back-all")
                      .px(5)
                      .py(2)
                      .rounded(4)
                      .text_color(t.muted)
                      .hover((s) => s.bg(t.hover).text_color(t.foreground))
                      .on_click((_e, clickCx) => {
                        clickCx.stop_propagation();
                        clickCx.spawn(async () => {
                          await coordinator.setView("all");
                        });
                      })
                      .child("← All")
                  )
                  .when(view.canReturnToCurrent, (bar) =>
                    bar.child(
                      div()
                        .id("streams-preview-back-current")
                        .px(6)
                        .py(2)
                        .rounded(4)
                        .text_color(t.accent)
                        .hover((s) => s.bg(t.hover))
                        .on_click((_e, clickCx) => {
                          clickCx.stop_propagation();
                          coordinator.returnToCurrent({ isEnrolled: view.isEnrolled });
                        })
                        .child("Back to current")
                    )
                  )
              )
            )
            .when(Boolean(streamUnreadable), (el) =>
              el.child(renderUnreadableNotice(streamUnreadable))
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
                        .child(stream.title)
                    )
                    .child(statusBadge(stream.status, t))
                    .child(
                      div()
                        .id("streams-menu-btn")
                        .flex_none()
                        .px(5)
                        .py(2)
                        .rounded(4)
                        .text_color(t.muted)
                        .hover((s) => s.bg(t.hover).text_color(t.foreground))
                        .on_click((_e, clickCx) => {
                          clickCx.stop_propagation();
                          openStreamMenu(stream.key);
                        })
                        .child("⋮")
                    )
                )
                .when(Boolean(stream.id), (el) =>
                  el.child(
                    div()
                      .font_family(monoFont)
                      .text_size(10.5)
                      .text_color(t.dim)
                      .truncate()
                      .child(stream.id)
                  )
                )
                .when(Boolean(stream.goal), (el) =>
                  el.child(div().text_size(11.5).text_color(t.muted).child(stream.goal))
                )
            )
            .child(
              v_flex()
                .pt(4)
                .gap(6)
                .border_t(1)
                .border_color(t.border)
                .child(
                  h_flex()
                    .justify_between()
                    .items_center()
                    .gap(6)
                    .child(
                      h_flex()
                        .id("streams-sessions-toggle")
                        .flex_1()
                        .gap(5)
                        .items_center()
                        .hover((s) => s.text_color(t.foreground))
                        .on_click((_e, clickCx) => {
                          clickCx.stop_propagation();
                          this.sessionsOpen = !this.sessionsOpen;
                          clickCx.notify();
                        })
                        .child(
                          div()
                            .text_size(11)
                            .text_color(t.dim)
                            .child(this.sessionsOpen ? "▾" : "▸")
                        )
                        .child(
                          div()
                            .font_medium()
                            .text_size(11.5)
                            .text_color(t.muted)
                            .child(`Sessions · ${streamSessions.length}`)
                        )
                    )
                    .when(streamSessions.length > 1, (row) =>
                      row.child(
                        div()
                          .id("streams-sessions-sort-toggle")
                          .flex_none()
                          .px(5)
                          .py(1)
                          .rounded(4)
                          .text_size(11)
                          .text_color(t.dim)
                          .hover((s) => s.bg(t.hover).text_color(t.foreground))
                          .tooltip(
                            this.sessionSort === "recent"
                              ? "Sorted by most recent — click for A–Z"
                              : "Sorted A–Z — click for most recent"
                          )
                          .on_click((_e, clickCx) => {
                            clickCx.stop_propagation();
                            this.sessionSort = this.sessionSort === "recent" ? "alpha" : "recent";
                            clickCx.notify();
                          })
                          .child(this.sessionSort === "recent" ? "↕ Recent" : "↕ A–Z")
                      )
                    )
                )
                .when(this.sessionsOpen && streamSessions.length === 0, (el) =>
                  el.child(div().text_size(11.5).text_color(t.dim).child("No Sessions yet."))
                )
                .when(this.sessionsOpen && streamSessions.length > 0, (el) => {
                  const liveSessionKeys = new Set(
                    runningSessionTerminals({
                      terminalList: snap.terminalList,
                      panes: snap.panes,
                      agents: snap.agents,
                    }).values()
                  );
                  const sessionGroups = groupStreamSessions(streamSessions, {
                    liveSessionKeys,
                    currentSessionKey: currentSessKey,
                    sort: this.sessionSort,
                  });
                  const nonEmptyGroups = sessionGroups.filter((g) => g.sessions.length > 0);
                  const onlyFinished =
                    nonEmptyGroups.length === 1 && nonEmptyGroups[0].id === "done_closed";
                  const renderSessionRow = (sess, idx) => {
                    const liveMatch = findLivePaneForSession({
                      sessionKey: sess.key,
                      panes: snap.panes,
                      terminalList: snap.terminalList,
                      agents: snap.agents,
                    });
                    const isOpen = Boolean(liveMatch);
                    const livePaneObj = liveMatch
                      ? snap.panes.find((p) => p.pane === liveMatch.pane) ?? null
                      : null;
                    const isCurrent = Boolean(currentSessKey && sess.key === currentSessKey);
                    const isRowHovered = this.hoveredSessionKey === sess.key;
                    const openIcon = isOpen
                      ? icon(livePaneObj?.program, 13, t.foreground) ??
                        div().flex_none().text_size(10).text_color(t.foreground).child("●")
                      : div().flex_none().text_size(10).text_color(t.dim).child("○");
                    return v_flex()
                      .id(`stream-session-${sess.id}`)
                      .px(9)
                      .py(7)
                      .gap(3)
                      .when(idx > 0, (row) => row.border_t(1).border_color(t.border))
                      .when(isCurrent, (row) => row.bg(t.selected))
                      .hover((s) => s.bg(t.hover))
                      .on_hover((hovered, hoverCx) => {
                        const next = hovered
                          ? sess.key
                          : this.hoveredSessionKey === sess.key
                            ? null
                            : this.hoveredSessionKey;
                        if (next !== this.hoveredSessionKey) {
                          this.hoveredSessionKey = next;
                          hoverCx.notify();
                        }
                      })
                      .on_mouse_down("right", (_e, downCx) => {
                        downCx.stop_propagation();
                        openSessionMenu({
                          sessionKey: sess.key,
                          streamKey: stream.key,
                          status: sess.status,
                          livePane: liveMatch?.pane ?? null,
                          fromHeader: false,
                        });
                      })
                      .on_click((_e, clickCx) => {
                        clickCx.spawn(async () => {
                          await coordinator.selectSessionFromStream(
                            sess.key,
                            stream.key,
                            {
                              panes: snap.panes,
                              terminalList: snap.terminalList,
                              agents: snap.agents,
                            },
                            { run }
                          );
                        });
                      })
                      .child(
                        h_flex()
                          .gap(6)
                          .items_center()
                          .child(openIcon)
                          .child(
                            div()
                              .flex_1()
                              .min_w(0)
                              .truncate()
                              .font_medium()
                              .text_size(12)
                              .text_color(isOpen ? t.foreground : t.muted)
                              .child(sess.title)
                          )
                          .child(statusBadge(sess.status, t))
                          .when(isOpen && isRowHovered && Boolean(liveMatch?.pane), (row) =>
                            row.child(
                              div()
                                .id(`stream-session-close-${sess.id}`)
                                .flex_none()
                                .px(5)
                                .py(1)
                                .rounded(4)
                                .text_size(11)
                                .text_color(t.dim)
                                .hover((s) => s.bg(t.raised).text_color(t.blocked))
                                .tooltip("Close terminal")
                                .on_mouse_down("left", (_e, downCx) => downCx.stop_propagation())
                                .on_click((_e, clickCx) => {
                                  clickCx.stop_propagation();
                                  run("pane.close", { pane: liveMatch.pane });
                                })
                                .child("×")
                            )
                          )
                          .when(!isOpen && isRowHovered, (row) =>
                            row.child(
                              div()
                                .id(`stream-session-resume-${sess.id}`)
                                .flex_none()
                                .px(5)
                                .py(1)
                                .rounded(4)
                                .text_size(10.5)
                                .text_color(t.accent)
                                .hover((s) => s.bg(t.raised))
                                .tooltip("Resume session")
                                .on_mouse_down("left", (_e, downCx) => downCx.stop_propagation())
                                .on_click((_e, clickCx) => {
                                  clickCx.stop_propagation();
                                  resumeSingleSession(clickCx, stream.key, sess.key);
                                })
                                .child("▶")
                            )
                          )
                          .child(
                            div()
                              .id(`stream-session-menu-${sess.id}`)
                              .flex_none()
                              .px(4)
                              .py(1)
                              .rounded(4)
                              .text_color(t.dim)
                              .hover((s) => s.bg(t.hover).text_color(t.foreground))
                              .on_mouse_down("left", (_e, downCx) => downCx.stop_propagation())
                              .on_click((_e, clickCx) => {
                                clickCx.stop_propagation();
                                openSessionMenu({
                                  sessionKey: sess.key,
                                  streamKey: stream.key,
                                  status: sess.status,
                                  livePane: liveMatch?.pane ?? null,
                                  fromHeader: false,
                                });
                              })
                              .child("⋮")
                          )
                      )
                      .when(Boolean(sess.state_short), (row) =>
                        row.child(
                          div()
                            .pl(18)
                            .text_size(11)
                            .text_color(t.dim)
                            .truncate()
                            .child(sess.state_short)
                        )
                      );
                  };

                  return el.child(
                    v_flex()
                      .gap(8)
                      .children(
                        nonEmptyGroups.map((group) => {
                          const savedState = this.sessionGroupsOpen?.[group.id];
                          const hasCurrent =
                            Boolean(currentSessKey) &&
                            group.sessions.some((s) => s.key === currentSessKey);
                          const isExpanded =
                            typeof savedState === "boolean"
                              ? savedState
                              : group.defaultOpen || onlyFinished || hasCurrent;
                          return v_flex()
                            .gap(4)
                            .child(
                              h_flex()
                                .id(`streams-session-group-${group.id}`)
                                .px(2)
                                .py(1)
                                .gap(5)
                                .items_center()
                                .justify_between()
                                .rounded(4)
                                .text_size(11)
                                .text_color(t.dim)
                                .hover((s) => s.bg(t.hover).text_color(t.foreground))
                                .on_click((_e, clickCx) => {
                                  clickCx.stop_propagation();
                                  this.sessionGroupsOpen[group.id] = !isExpanded;
                                  clickCx.notify();
                                })
                                .child(
                                  h_flex()
                                    .gap(5)
                                    .items_center()
                                    .child(div().text_size(10).child(isExpanded ? "▾" : "▸"))
                                    .child(div().font_medium().child(group.label))
                                )
                                .child(div().text_size(10.5).child(`${group.sessions.length}`))
                            )
                            .when(isExpanded, (sec) =>
                              sec.child(
                                v_flex()
                                  .rounded(6)
                                  .bg(t.raised)
                                  .border(1)
                                  .border_color(t.border)
                                  .children(group.sessions.map(renderSessionRow))
                              )
                            );
                        })
                      )
                  );
                })
            )
        );
    }

    // -------------------------------------------------------------------------
    // SESSION VIEW
    // -------------------------------------------------------------------------
    const session = view.session ?? null;
    const parentStream = view.stream ?? null;

    if (!session) {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .text_size(12)
        .text_color(t.foreground)
        .child(header)
        .child(
          div()
            .p(12)
            .text_color(t.dim)
            .child(
              coordinator.previewSessionKey
                ? "Loading Session…"
                : "This tab is not enrolled in a Session. Switch to All to browse Streams."
            )
        );
    }

    const tabPane = view.terminal
      ? snap.panes.find((p) => p.terminal === view.terminal) ?? null
      : null;
    const termInfo = view.terminal
      ? snap.terminalList.find((item) => (item.address ?? item.id) === view.terminal) ?? null
      : null;
    const liveAgent = view.terminal
      ? snap.agents.find((a) => a.address === view.terminal) ?? null
      : null;
    const effectiveStatus = view.isUnopened
      ? session.status
      : effectiveSessionStatus(session, {
          pane: tabPane,
          terminal: termInfo,
          agent: liveAgent,
        });
    const sessionIcon = view.isUnopened
      ? div().flex_none().text_size(11).text_color(t.dim).child("○")
      : icon(tabPane?.program, 13, t.muted);
    const references = buildReferenceItems(session);
    const streamKey = parentStream?.key ?? session.stream;
    const streamLabel = parentStream?.title || coordinator.streamTitle(streamKey);

    return renderPinnedErrorBanner(
      v_flex()
        .size_full()
        .bg(t.chrome)
        .text_size(12)
        .text_color(t.foreground)
        .child(header)
    )
      .child(
        v_flex()
          .id("streams-sidebar-body")
          .flex_1()
          .p(12)
          .gap(12)
          .overflow_y_scrollbar()
          .track_scroll(this.scroll)
          .when(view.isPreview && view.isUnopened, (el) =>
            el.child(
              v_flex()
                .px(8)
                .py(7)
                .gap(6)
                .rounded(5)
                .bg(t.raised)
                .border(1)
                .border_color(t.border)
                .text_size(11.5)
                .child(
                  h_flex()
                    .gap(6)
                    .items_center()
                    .justify_between()
                    .child(
                      div()
                        .id("session-preview-back-stream")
                        .px(5)
                        .py(2)
                        .rounded(4)
                        .text_color(t.muted)
                        .hover((s) => s.bg(t.hover).text_color(t.foreground))
                        .on_click((_e, clickCx) => {
                          clickCx.stop_propagation();
                          coordinator.backToStream();
                        })
                        .child("← Stream")
                    )
                    .child(
                      div().text_size(10.5).text_color(t.dim).child("Unopened · read-only")
                    )
                )
                .child(
                  h_flex()
                    .gap(6)
                    .items_center()
                    .justify_between()
                    .child(
                      div()
                        .id("session-preview-resume")
                        .px(8)
                        .py(3)
                        .rounded(4)
                        .bg(t.accent)
                        .font_medium()
                        .text_size(11.5)
                        .text_color(t.background)
                        .on_click((_e, clickCx) => {
                          clickCx.stop_propagation();
                          resumeSingleSession(clickCx, streamKey, session.key);
                        })
                        .child("▶ Resume")
                    )
                    .when(view.canReturnToCurrent, (bar) =>
                      bar.child(
                        div()
                          .id("session-preview-back-current")
                          .px(6)
                          .py(2)
                          .rounded(4)
                          .text_color(t.accent)
                          .hover((s) => s.bg(t.hover))
                          .on_click((_e, clickCx) => {
                            clickCx.stop_propagation();
                            coordinator.returnToCurrent({ isEnrolled: view.isEnrolled });
                          })
                          .child("Back to current")
                      )
                    )
                )
            )
          )
          .when(Boolean(streamLabel), (el) =>
            el.child(
              h_flex()
                .id("session-stream-breadcrumb")
                .self_start()
                .px(5)
                .py(2)
                .gap(5)
                .items_center()
                .rounded(4)
                .text_size(11.5)
                .text_color(t.muted)
                .hover((s) => s.bg(t.hover).text_color(t.foreground))
                .on_click((_e, clickCx) => {
                  clickCx.stop_propagation();
                  if (view.isPreview) {
                    coordinator.backToStream();
                  } else {
                    coordinator.setView("stream");
                  }
                })
                .child(div().flex_none().text_color(t.accent).child("≋"))
                .child(div().truncate().child(streamLabel))
            )
          )
          .child(
            v_flex()
              .gap(8)
              .child(
                h_flex()
                  .gap(6)
                  .items_center()
                  .when(Boolean(sessionIcon), (el) => el.child(sessionIcon))
                  .child(
                    div()
                      .flex_1()
                      .min_w(0)
                      .truncate()
                      .font_semibold()
                      .text_size(13.5)
                      .child(session.title)
                  )
                  .child(
                    div()
                      .id("session-header-status-btn")
                      .flex_none()
                      .rounded(10)
                      .hover((s) => s.bg(t.hover))
                      .tooltip("Change Session status")
                      .on_click((_e, clickCx) => {
                        clickCx.stop_propagation();
                        openSessionMenu({
                          sessionKey: session.key,
                          streamKey: sessionStreamKey,
                          status: effectiveStatus,
                          livePane: sessionLivePane,
                          fromHeader: true,
                        });
                      })
                      .child(statusBadge(effectiveStatus, t))
                  )
                  .child(
                    div()
                      .id("session-header-menu-btn")
                      .flex_none()
                      .px(5)
                      .py(2)
                      .rounded(4)
                      .text_color(t.muted)
                      .hover((s) => s.bg(t.hover).text_color(t.foreground))
                      .on_click((_e, clickCx) => {
                        clickCx.stop_propagation();
                        openSessionMenu({
                          sessionKey: session.key,
                          streamKey,
                          status: session.status,
                          livePane: tabPane?.pane ?? null,
                          fromHeader: true,
                        });
                      })
                      .child("⋮")
                  )
              )
              .when(Boolean(session.state), (el) =>
                el.child(
                  v_flex()
                    .gap(2)
                    .child(div().text_size(10.5).font_medium().text_color(t.dim).child("State"))
                    .child(div().text_size(12).text_color(t.foreground).child(session.state))
                )
              )
              .when(Boolean(session.recap), (el) =>
                el.child(
                  v_flex()
                    .gap(2)
                    .child(div().text_size(10.5).font_medium().text_color(t.dim).child("Recap"))
                    .child(div().text_size(11.5).text_color(t.muted).child(session.recap))
                )
              )
              .when(Boolean(session.remaining), (el) =>
                el.child(
                  v_flex()
                    .gap(2)
                    .child(div().text_size(10.5).font_medium().text_color(t.dim).child("Remaining"))
                    .child(div().text_size(11.5).text_color(t.muted).child(session.remaining))
                )
              )
          )
          .when(references.length > 0, (el) => el.child(renderReferencesSection(references)))
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
    this.syncConfiguredBin();
    if (role === "frame") return this.renderFrame(cx);
    return this.renderSidebar(cx);
  }
}
