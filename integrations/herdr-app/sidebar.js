// Sidebar role: segmented header (Session / Stream / All), offline/loading states,
// shared action menus & references section, dispatching to view_all / view_stream / view_session.

import { div, svg } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import {
  panes,
  focused,
  run,
  openOverlay,
  theme,
  settings,
} from "herdr";
import {
  SESSION_STATUSES,
  indexLive,
  resolveSidebarView,
  streamBulkActionState,
} from "./state.js";
import { coordinator, collectSnapshot, scheduleObserve } from "./runtime.js";
import {
  statusStyle,
  referenceIcon,
  spawnSidebarWork,
} from "./ui.js";
import {
  isMenuOpen,
  openMoveMenu,
  startRename,
  renderMenuPopover,
} from "./popovers.js";
import { renderAllView } from "./view_all.js";
import { renderStreamView } from "./view_stream.js";
import { renderSessionView } from "./view_session.js";

export function renderSidebar(view, cx) {
  const t = theme();
  const snap = collectSnapshot();
  const live = indexLive(snap, {
    sessions: coordinator.sessions,
    staleSessionKeys: coordinator.staleSessionKeys,
    lastFocusedStreamBySpace: coordinator.lastFocusedStreamBySpace,
  });
  coordinator.syncContext(live);
  scheduleObserve(view, cx, snap, { live });

  const activeSpace = snap.activeSpace;

  const contextKey = `${activeSpace?.id ?? ""}\0${snap.activeTabId ?? ""}\0${coordinator.preferredView}\0${coordinator.previewStreamKey ?? ""}\0${coordinator.previewSessionKey ?? ""}`;
  if (view._lastContextKey !== null && view._lastContextKey !== contextKey) {
    view.interaction = null;
  }
  view._lastContextKey = contextKey;
  const monoFont = settings()?.font_family ?? "JetBrains Mono";
  const isRefreshing = coordinator.inFlightKey !== null;
  const resolvedView = resolveSidebarView({
    preferredView: coordinator.preferredView,
    sessions: coordinator.sessions,
    streams: coordinator.streams,
    live,
    previewStreamKey: coordinator.previewStreamKey,
    previewSessionKey: coordinator.previewSessionKey,
  });

  const activeSegment = resolvedView.view;
  const sessionDisabled = !resolvedView.hasSessionAccess;
  const streamDisabled = !resolvedView.hasStreamAccess;

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
        view.interaction = null;
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
      (() => {
        const refreshIcon = svg("mods/streams/icons/refresh-cw.svg").size(13);
        return div()
          .id("streams-refresh")
          .flex_none()
          .flex()
          .items_center()
          .justify_center()
          .p(5)
          .rounded(4)
          .cursor_pointer()
          .text_color(isRefreshing ? t.dim : t.muted)
          .hover((s) => s.bg(t.hover).text_color(t.foreground))
          .tooltip(isRefreshing ? "Refreshing…" : "Refresh")
          .on_click((_e, clickCx) => {
            scheduleObserve(view, clickCx, collectSnapshot(), { force: true });
            clickCx.notify();
          })
          .child(isRefreshing ? refreshIcon.spin(900) : refreshIcon);
      })()
    );

  if (!activeSpace || !snap.activeTabId) {
    return v_flex()
      .size_full()
      .bg(t.chrome)
      .child(header)
      .child(div().p(12).text_size(12).text_color(t.dim).child("No active tab."));
  }

  if (activeSpace.machine !== null) {
    const machineInfo =
      (snap.machineList ?? []).find((m) => m.name === activeSpace.machine) ?? null;
    const layoutState = machineInfo?.layout ?? "unavailable";
    if (layoutState === "unavailable") {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(
          div()
            .p(12)
            .text_size(12)
            .text_color(t.dim)
            .child(`Streams sidebar is unavailable — ${activeSpace.machine} is offline.`)
        );
    }
    if (layoutState === "loading") {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(
          div()
            .p(12)
            .text_size(12)
            .text_color(t.dim)
            .child(`Connecting to ${activeSpace.machine}…`)
        );
    }
  }

  const hostBucket = coordinator.getStreamList();
  const bannerError =
    coordinator.readError || (resolvedView.view === "all" ? hostBucket.error : null);
  const activeBanner = coordinator.actionError
    ? {
        kind: "action",
        title: coordinator.actionError.title,
        message: coordinator.actionError.message,
        copyText: coordinator.actionError.copyText ?? null,
        copyLabel: coordinator.actionError.copyLabel ?? "Copy",
        color: coordinator.actionError.level === "warning" ? t.working : t.blocked,
        onDismiss: () => coordinator.clearActionError(),
      }
    : bannerError
      ? {
          kind: "read",
          title: "Refresh failed",
          message: bannerError,
          copyText: null,
          copyLabel: "Copy",
          color: t.blocked,
          onDismiss: () => coordinator.clearReadError(),
        }
      : null;

  const statusLabel = (s) => s.charAt(0).toUpperCase() + s.slice(1);

  const resumeSingleSession = (clickCx, streamKey, sessionKey) => {
    if (!streamKey || !sessionKey) return;
    const snapNow = collectSnapshot();
    spawnSidebarWork(clickCx, async () => {
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
    });
  };

  const buildStatusMenuItems = ({ sessionKey, streamKey, status }) => {
    const statusPending = Boolean(sessionKey && coordinator.isPending(`status:${sessionKey}`));
    return SESSION_STATUSES.map((st) => {
      const style = statusStyle(st, t);
      return {
        id: `status-${st}`,
        label: statusLabel(st),
        icon: { symbol: style.symbol, color: style.color },
        disabled: statusPending || status === st,
        checked: status === st,
        onSelect: (actCx) => {
          if (!sessionKey) return;
          spawnSidebarWork(actCx, async () => {
            await coordinator.setSessionStatus(sessionKey, st, streamKey);
          });
        },
      };
    });
  };

  const buildStreamMenuItems = (stream, { writable = true } = {}) => {
    const streamKey = stream?.key ?? null;
    const bulk = streamBulkActionState(stream);
    const streamBusy = Boolean(streamKey && coordinator.isPending(`stream:${streamKey}`));
    const renameBusy = Boolean(streamKey && coordinator.isPending(`rename:${streamKey}`));
    return [
      {
        id: "rename",
        label: "Rename…",
        disabled: !streamKey || !writable || renameBusy || streamBusy,
        onSelect: (actCx) => {
          if (!streamKey || !writable) return;
          startRename(view, { kind: "stream", key: streamKey, title: stream.title ?? "" }, actCx);
        },
      },
      {
        id: "open-active",
        label: "Open all active sessions",
        disabled: !bulk.canOpenActive || streamBusy,
        onSelect: (actCx) => {
          if (!streamKey) return;
          const snapNow = collectSnapshot();
          spawnSidebarWork(actCx, async () => {
            await coordinator.openWork(
              {
                stream: streamKey,
                sessions: { kind: "only_active" },
                terminalList: snapNow.terminalList,
                panes: snapNow.panes,
                agents: snapNow.agents,
              },
              { run }
            );
          });
        },
      },
      { type: "divider" },
      {
        id: "park-all",
        label: "Park all",
        disabled: !bulk.canParkAll || streamBusy,
        onSelect: (actCx) => {
          if (!streamKey) return;
          spawnSidebarWork(actCx, async () => {
            await coordinator.parkAllSessions(streamKey);
          });
        },
      },
      {
        id: "close-all",
        label: "Close all",
        disabled: !bulk.canCloseAll || streamBusy,
        danger: bulk.canCloseAll && !streamBusy,
        onSelect: (actCx) => {
          if (!streamKey) return;
          spawnSidebarWork(actCx, async () => {
            await coordinator.closeAllSessions(streamKey);
          });
        },
      },
      { type: "divider" },
      {
        id: "copy-id",
        label: "Copy stream ID",
        disabled: !stream?.id,
        onSelect: (actCx) => {
          if (stream?.id) actCx.write_to_clipboard(stream.id);
        },
      },
      { type: "divider" },
      {
        id: "delete",
        label: "Delete empty stream",
        disabled: !bulk.canDelete || streamBusy,
        danger: bulk.canDelete && !streamBusy,
        onSelect: (actCx) => {
          if (!streamKey) return;
          spawnSidebarWork(actCx, async () => {
            await coordinator.deleteStream(streamKey);
          });
        },
      },
    ];
  };

  const buildSessionMenuItems = ({
    sessionKey,
    streamKey,
    title = "",
    status,
    livePane = null,
    writable = true,
  }) => {
    const isOpen = Boolean(livePane);
    const isResuming = coordinator.isPending(`resume:${sessionKey}`);
    const isRenaming = coordinator.isPending(`rename:${sessionKey}`);
    const isMoving = coordinator.isPending(`move:${sessionKey}`);
    const items = isOpen
      ? [
          {
            id: "switch",
            label: "Switch to",
            onSelect: (actCx) => {
              const snapNow = collectSnapshot();
              spawnSidebarWork(actCx, async () => {
                await coordinator.selectSessionFromStream(sessionKey, streamKey, snapNow, {
                  run,
                });
              });
            },
          },
          {
            id: "rename",
            label: "Rename…",
            disabled: !writable || isRenaming,
            onSelect: (actCx) => {
              if (!writable) return;
              startRename(
                view,
                { kind: "session", key: sessionKey, streamKey, title },
                actCx
              );
            },
          },
          {
            id: "move",
            label: "Move to stream…",
            disabled: !writable || isMoving,
            onSelect: (actCx) => {
              if (!writable) return;
              openMoveMenu(
                view,
                { sessionKey, sourceStreamKey: streamKey, sessionTitle: title },
                actCx
              );
            },
          },
          {
            id: "close-term",
            label: "Close terminal",
            onSelect: () => {
              run("pane.close", { pane: livePane });
            },
          },
          { type: "divider" },
        ]
      : [
          {
            id: "resume",
            label: isResuming ? "Resuming…" : "Resume",
            disabled: isResuming,
            onSelect: (actCx) => {
              resumeSingleSession(actCx, streamKey, sessionKey);
            },
          },
          {
            id: "inspect",
            label: "Inspect",
            onSelect: (actCx) => {
              spawnSidebarWork(actCx, async () => {
                await coordinator.previewSession(sessionKey, streamKey);
              });
            },
          },
          {
            id: "rename",
            label: "Rename…",
            disabled: !writable || isRenaming,
            onSelect: (actCx) => {
              if (!writable) return;
              startRename(
                view,
                { kind: "session", key: sessionKey, streamKey, title },
                actCx
              );
            },
          },
          {
            id: "move",
            label: "Move to stream…",
            disabled: !writable || isMoving,
            onSelect: (actCx) => {
              if (!writable) return;
              openMoveMenu(
                view,
                { sessionKey, sourceStreamKey: streamKey, sessionTitle: title },
                actCx
              );
            },
          },
          { type: "divider" },
        ];
    items.push({ type: "label", text: "Set status" });
    items.push(...buildStatusMenuItems({ sessionKey, streamKey, status }));
    return items;
  };

  const buildReferenceMenuItems = (item) => {
    const isDoc = item.kind === "document";
    const isArtifact = item.kind === "artifact";
    const copyLabel = isDoc ? "Copy path" : isArtifact ? "Copy artifact key" : "Copy URL";
    const items = [];
    if (isDoc) {
      items.push({
        id: "open-editor",
        label: "Open in editor",
        disabled: Boolean(item.disabled),
        onSelect: (actCx) => {
          const snapNow = collectSnapshot();
          const focusNow = focused();
          const targetPaneId =
            focusNow?.pane ??
            snapNow.panes.find((p) => p.tab === snapNow.activeTabId && p.focused)?.pane ??
            snapNow.panes.find((p) => p.tab === snapNow.activeTabId)?.pane ??
            null;
          spawnSidebarWork(actCx, async (asyncCx) => {
            await coordinator.openReference(item.reference, {
              activeSpaceId: snapNow.activeSpace?.id ?? null,
              activeTabId: snapNow.activeTabId,
              targetPaneId,
              mode: "editor",
              run,
              openOverlay,
              openUrl: (u) => asyncCx?.open_url?.(u),
            });
          });
        },
      });
    }
    items.push({
      id: "copy",
      label: copyLabel,
      disabled: !item.copyText,
      onSelect: (actCx) => {
        if (item.copyText) actCx.write_to_clipboard(item.copyText);
      },
    });
    if (!isArtifact) {
      const isDropping =
        Boolean(item.sessionKey && item.refKey) &&
        coordinator.isPending(`drop:${item.sessionKey}\0${item.refKey}`);
      items.push({ type: "divider" });
      items.push({
        id: "drop",
        label: isDropping ? "Dropping…" : "Drop",
        danger: !isDropping,
        disabled: !item.sessionKey || !item.refKey || isDropping,
        onSelect: (actCx) => {
          if (!item.sessionKey || !item.refKey) return;
          spawnSidebarWork(actCx, async () => {
            await coordinator.dropReference(
              item.sessionKey,
              item.refKey,
              item.streamKey ?? null
            );
          });
        },
      });
    }
    return items;
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
            view.referencesOpen = !view.referencesOpen;
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
              .child(view.referencesOpen ? "▾" : "▸")
          )
      )
      .when(view.referencesOpen, (el) =>
        el.child(
          v_flex()
            .rounded(6)
            .bg(t.raised)
            .border(1)
            .border_color(t.border)
            .children(
              references.map((item, idx) => {
                const refMenuItems = () => buildReferenceMenuItems(item);
                const refBtnMenuId = `ref-menu-${item.id}`;
                const refCtxMenuId = `ref-ctx-${item.id}`;
                const refBtnOpen = isMenuOpen(view, refBtnMenuId);
                const isRefHovered = view.hoveredRefId === item.id;
                const isRefDropping =
                  Boolean(item.sessionKey && item.refKey) &&
                  coordinator.isPending(`drop:${item.sessionKey}\0${item.refKey}`);
                const card = v_flex()
                  .id(item.id)
                  .relative()
                  .px(9)
                  .py(7)
                  .gap(3)
                  .when(idx > 0, (row) => row.border_t(1).border_color(t.border))
                  .on_hover((hovered, hoverCx) => {
                    const next = hovered
                      ? item.id
                      : view.hoveredRefId === item.id
                        ? null
                        : view.hoveredRefId;
                    if (next !== view.hoveredRefId) {
                      view.hoveredRefId = next;
                      hoverCx.notify();
                    }
                  })
                  .when(!item.disabled && !isRefDropping, (row) =>
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
                      .when(isRefHovered || refBtnOpen, (line) => line.pr(26))
                      .child(referenceIcon(item.kind, t))
                      .child(
                        div()
                          .flex_1()
                          .min_w(0)
                          .font_medium()
                          .text_size(12)
                          .truncate()
                          .text_color(item.disabled || isRefDropping ? t.dim : t.foreground)
                          .when(item.monoTitle, (titleEl) =>
                            titleEl.font_family(monoFont).text_size(11.5)
                          )
                          .child(isRefDropping ? `${item.title} (dropping…)` : item.title)
                      )
                      .when(item.external, (extEl) =>
                        extEl.child(
                          div().flex_none().text_size(11).text_color(t.dim).child("↗")
                        )
                      )
                      .when(Boolean(item.unavailableReason), (warnEl) =>
                        warnEl.child(
                          div()
                            .id(`ref-unavailable-${idx}`)
                            .flex_none()
                            .flex()
                            .items_center()
                            .tooltip(item.unavailableReason)
                            .child(
                              svg("mods/streams/icons/triangle-alert.svg")
                                .size(12)
                                .text_color(t.working)
                            )
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
                      div().text_size(11).text_color(t.muted).child(item.description)
                    )
                  )
                  .when(isRefHovered || refBtnOpen, (row) =>
                    row.child(
                      div()
                        .absolute()
                        .top(5)
                        .right(6)
                        .child(
                          renderMenuPopover(view, {
                            id: refBtnMenuId,
                            width: 185,
                            anchor: "top_right",
                            stopRowPropagation: true,
                            t,
                            items: refMenuItems,
                            trigger: div()
                              .id(`ref-menu-btn-${idx}`)
                              .flex_none()
                              .px(5)
                              .py(1)
                              .rounded(4)
                              .bg(refBtnOpen ? t.hover : t.raised)
                              .border(1)
                              .border_color(refBtnOpen ? t.accent : t.border)
                              .cursor_pointer()
                              .text_color(refBtnOpen ? t.foreground : t.muted)
                              .hover((s) => s.bg(t.hover).text_color(t.foreground))
                              .child("⋮"),
                          })
                        )
                    )
                  );
                return renderMenuPopover(view, {
                  id: refCtxMenuId,
                  width: 185,
                  anchor: "top_left",
                  mouseButton: "right",
                  t,
                  items: refMenuItems,
                  trigger: card,
                });
              })
            )
        )
      );

  if (resolvedView.view === "all") {
    return renderAllView({
      view,
      t,
      header,
      snap,
      live,
      activeSpace,
      hostBucket,
      activeBanner,
      resolvedView,
    });
  }

  if (resolvedView.view === "stream") {
    return renderStreamView({
      view,
      t,
      header,
      snap,
      live,
      activeSpace,
      activeBanner,
      resolvedView,
      buildStatusMenuItems,
      buildStreamMenuItems,
      buildSessionMenuItems,
      resumeSingleSession,
    });
  }

  return renderSessionView({
    view,
    t,
    header,
    snap,
    activeSpace,
    activeBanner,
    isRefreshing,
    resolvedView,
    buildStatusMenuItems,
    resumeSingleSession,
    renderReferencesSection,
  });
}
