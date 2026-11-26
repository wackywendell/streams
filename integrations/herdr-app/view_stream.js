// Sidebar "Stream" view: Stream header, goal, bulk actions menu, and grouped Sessions list.

import { div, svg } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { run } from "herdr";
import {
  unreadableNotice,
  formatRelativeAge,
  groupStreamSessions,
  sessionRowSubtitle,
} from "./state.js";
import { coordinator } from "./runtime.js";
import {
  icon,
  statusStyle,
  statusBadge,
  foreignHostChip,
  renderUnreadableNotice,
  renderPinnedErrorBanner,
} from "./ui.js";
import {
  isMenuOpen,
  startRename,
  renderInlineRename,
  renderMenuPopover,
  renderMovePopover,
} from "./popovers.js";

export function renderStreamView({
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
}) {
  const stream = resolvedView.stream ?? null;
  if (!stream) {
    return renderPinnedErrorBanner(
      v_flex()
        .size_full()
        .bg(t.chrome)
        .text_size(12)
        .text_color(t.foreground)
        .child(header),
      activeBanner,
      t
    ).child(div().p(12).text_color(t.dim).child("Loading Stream…"));
  }

  const streamSessions = coordinator.getStreamSessions(stream.key);
  const streamUnreadable = unreadableNotice(stream.unreadable);
  const currentSessKey = resolvedView.currentSession?.key ?? null;
  const streamForeignHost = coordinator.foreignHostBadge(stream, activeSpace?.machine);

  return renderPinnedErrorBanner(
    v_flex()
      .size_full()
      .bg(t.chrome)
      .text_size(12)
      .text_color(t.foreground)
      .child(header),
    activeBanner,
    t
  ).child(
    v_flex()
      .id("streams-body-stream")
      .flex_1()
      .p(12)
      .gap(12)
      .overflow_y_scrollbar()
      .track_scroll(view.scrollStream)
      .when(resolvedView.isPreview, (el) =>
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
              h_flex()
                .gap(6)
                .items_center()
                .min_w(0)
                .child(
                  div()
                    .id("streams-preview-back-all")
                    .flex_none()
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
                .when(Boolean(streamForeignHost), (left) =>
                  left.child(foreignHostChip(streamForeignHost, t, "stream-preview-host"))
                )
            )
            .when(resolvedView.canReturnToCurrent, (bar) =>
              bar.child(
                div()
                  .id("streams-preview-back-current")
                  .flex_none()
                  .px(6)
                  .py(2)
                  .rounded(4)
                  .text_color(t.accent)
                  .hover((s) => s.bg(t.hover))
                  .on_click((_e, clickCx) => {
                    clickCx.stop_propagation();
                    coordinator.returnToCurrent({ isEnrolled: resolvedView.isEnrolled });
                  })
                  .child("Back to current")
              )
            )
        )
      )
      .when(Boolean(streamUnreadable), (el) =>
        el.child(renderUnreadableNotice(streamUnreadable, t))
      )
      .child(
        v_flex()
          .gap(5)
          .child(
            (() => {
              const streamWritable = !(streamForeignHost && streamForeignHost.state !== "online");
              const isRenamingStream =
                view.interaction?.kind === "rename" &&
                view.interaction.entityKind === "stream" &&
                view.interaction.key === stream.key;
              const isStreamRenamePending = coordinator.isPending(`rename:${stream.key}`);
              return h_flex()
                .gap(6)
                .items_center()
                .child(div().flex_none().text_size(13).text_color(t.accent).child("≋"))
                .child(
                  isRenamingStream
                    ? renderInlineRename(view, t, { id: "stream-title-rename" })
                    : div()
                        .id("stream-header-title")
                        .flex_1()
                        .min_w(0)
                        .truncate()
                        .font_semibold()
                        .text_size(13.5)
                        .when(isStreamRenamePending, (el) => el.text_color(t.dim))
                        .when(streamWritable && !isStreamRenamePending, (el) =>
                          el
                            .tooltip("Double-click to rename")
                            .on_mouse_down("left", (ev, downCx) => {
                              if (ev.click_count === 2) {
                                downCx.stop_propagation();
                                startRename(
                                  view,
                                  { kind: "stream", key: stream.key, title: stream.title },
                                  downCx
                                );
                              }
                            })
                        )
                        .child(
                          isStreamRenamePending ? `${stream.title}…` : stream.title
                        )
                )
                .when(!isRenamingStream && !resolvedView.isPreview && Boolean(streamForeignHost), (row) =>
                  row.child(foreignHostChip(streamForeignHost, t, "stream-header-host"))
                )
                .when(!isRenamingStream && coordinator.isPending(`stream:${stream.key}`), (row) =>
                  row.child(
                    div()
                      .flex_none()
                      .text_size(10.5)
                      .text_color(t.dim)
                      .child(coordinator.pendingValue(`stream:${stream.key}`))
                  )
                )
                .when(!isRenamingStream, (row) =>
                  row
                    .child(
                      statusBadge(stream.status, t)
                        .id("streams-header-status")
                        .tooltip("Derived from sessions")
                    )
                    .child(
                      (() => {
                        const streamMenuId = `stream-actions-${stream.key}`;
                        const streamMenuOpen = isMenuOpen(view, streamMenuId);
                        return renderMenuPopover(view, {
                          id: streamMenuId,
                          width: 215,
                          anchor: "top_right",
                          t,
                          items: () =>
                            buildStreamMenuItems(stream, { writable: streamWritable }),
                          trigger: div()
                            .id("streams-menu-btn")
                            .flex_none()
                            .px(5)
                            .py(2)
                            .rounded(4)
                            .cursor_pointer()
                            .text_color(streamMenuOpen ? t.foreground : t.muted)
                            .when(streamMenuOpen, (el) => el.bg(t.hover))
                            .hover((s) => s.bg(t.hover).text_color(t.foreground))
                            .child("⋮"),
                        });
                      })()
                    )
                );
            })()
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
                    view.sessionsOpen = !view.sessionsOpen;
                    clickCx.notify();
                  })
                  .child(
                    div()
                      .text_size(11)
                      .text_color(t.dim)
                      .child(view.sessionsOpen ? "▾" : "▸")
                  )
                  .child(
                    div()
                      .font_medium()
                      .text_size(11.5)
                      .text_color(t.muted)
                      .child(
                        `Sessions · ${stream.detail ? streamSessions.length : stream.total_sessions}`
                      )
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
                      view.sessionSort === "recent"
                        ? "Sorted by most recent — click for A–Z"
                        : "Sorted A–Z — click for most recent"
                    )
                    .on_click((_e, clickCx) => {
                      clickCx.stop_propagation();
                      view.sessionSort = view.sessionSort === "recent" ? "alpha" : "recent";
                      clickCx.notify();
                    })
                    .child(view.sessionSort === "recent" ? "↕ Recent" : "↕ A–Z")
                )
              )
          )
          .when(view.sessionsOpen && streamSessions.length === 0, (el) =>
            el.child(
              div()
                .text_size(11.5)
                .text_color(t.dim)
                .child(stream.detail ? "No Sessions yet." : "Loading Sessions…")
            )
          )
          .when(view.sessionsOpen && streamSessions.length > 0, (el) => {
            const liveSessionKeys = live.liveSessionKeys;
            const sessionGroups = groupStreamSessions(streamSessions, {
              liveSessionKeys,
              currentSessionKey: currentSessKey,
              sort: view.sessionSort,
            });
            const nonEmptyGroups = sessionGroups.filter((g) => g.sessions.length > 0);
            const onlyFinished =
              nonEmptyGroups.length === 1 && nonEmptyGroups[0].id === "done_closed";
            const renderSessionRow = (sess, idx) => {
              const liveMatch = live.paneBySession.get(sess.key) ?? null;
              const isOpen = Boolean(liveMatch);
              const livePaneObj = liveMatch
                ? snap.panes.find((p) => p.pane === liveMatch.pane) ?? null
                : null;
              const isCurrent = Boolean(currentSessKey && sess.key === currentSessKey);
              const isRowHovered = view.hoveredSessionKey === sess.key;
              const openIcon = isOpen
                ? icon(livePaneObj?.program, 13, t.foreground) ??
                  div().flex_none().text_size(10).text_color(t.foreground).child("●")
                : div().flex_none().text_size(10).text_color(t.dim).child("○");
              const rowStatusMenuId = `row-status-${sess.key}`;
              const rowActionMenuId = `row-menu-${sess.key}`;
              const rowCtxMenuId = `row-ctx-${sess.key}`;
              const rowStatusOpen = isMenuOpen(view, rowStatusMenuId);
              const rowActionOpen = isMenuOpen(view, rowActionMenuId);
              const isRowStatusPending = coordinator.isPending(`status:${sess.key}`);
              const isRowResuming = coordinator.isPending(`resume:${sess.key}`);
              const isRowMoving = coordinator.isPending(`move:${sess.key}`);
              const isRowMovingMenu =
                view.interaction?.kind === "move" &&
                view.interaction.sessionKey === sess.key;
              const isRowRenaming =
                view.interaction?.kind === "rename" &&
                view.interaction.entityKind === "session" &&
                view.interaction.key === sess.key;
              const isRowRenamePending = coordinator.isPending(`rename:${sess.key}`);
              const rowWritable = !(streamForeignHost && streamForeignHost.state !== "online");
              const showRowActions = isRowHovered || rowActionOpen || isRowMovingMenu;
              const rowMenuItems = () =>
                buildSessionMenuItems({
                  sessionKey: sess.key,
                  streamKey: stream.key,
                  title: sess.title,
                  status: sess.status,
                  livePane: liveMatch?.pane ?? null,
                  writable: rowWritable,
                });
              const rowEl = v_flex()
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
                    : view.hoveredSessionKey === sess.key
                      ? null
                      : view.hoveredSessionKey;
                  if (next !== view.hoveredSessionKey) {
                    view.hoveredSessionKey = next;
                    hoverCx.notify();
                  }
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
                      isRowRenaming
                        ? renderInlineRename(view, t, {
                            id: `stream-session-rename-${sess.id}`,
                            compact: true,
                          })
                        : div()
                            .flex_1()
                            .min_w(0)
                            .truncate()
                            .font_medium()
                            .text_size(12)
                            .text_color(
                              isRowRenamePending || isRowMoving
                                ? t.dim
                                : isOpen
                                  ? t.foreground
                                  : t.muted
                            )
                            .child(
                              isRowRenamePending || isRowMoving
                                ? `${sess.title}…`
                                : sess.title
                            )
                    )
                    .when(!isRowRenaming, (line) =>
                      line.child(
                        isRowStatusPending
                          ? h_flex()
                              .id(`stream-session-status-pending-${sess.id}`)
                              .flex_none()
                              .px(5)
                              .py(1)
                              .gap(4)
                              .items_center()
                              .text_size(11)
                              .text_color(t.dim)
                              .tooltip("Updating status…")
                              .child(`${statusStyle(sess.status, t).label}…`)
                          : streamForeignHost && streamForeignHost.state !== "online"
                            ? statusBadge(sess.status, t)
                                .id(`stream-session-status-ro-${sess.id}`)
                                .tooltip(streamForeignHost.tooltip)
                            : renderMenuPopover(view, {
                                id: rowStatusMenuId,
                                width: 155,
                                anchor: "top_right",
                                stopRowPropagation: true,
                                t,
                                items: () =>
                                  buildStatusMenuItems({
                                    sessionKey: sess.key,
                                    streamKey: stream.key,
                                    status: sess.status,
                                  }),
                                trigger: div()
                                  .id(`stream-session-status-${sess.id}`)
                                  .flex_none()
                                  .px(5)
                                  .py(1)
                                  .rounded(4)
                                  .cursor_pointer()
                                  .when(rowStatusOpen, (elBtn) => elBtn.bg(t.raised))
                                  .hover((s) => s.bg(t.raised))
                                  .tooltip("Change status")
                                  .child(statusBadge(sess.status, t)),
                              })
                      )
                    )
                )
                .child(
                  (() => {
                    const rowAge = formatRelativeAge(sess.updated_at_ms);
                    return h_flex()
                      .pl(18)
                      .h(18)
                      .gap(6)
                      .items_center()
                      .child(
                        div()
                          .flex_1()
                          .min_w(0)
                          .text_size(11)
                          .text_color(t.dim)
                          .truncate()
                          .child(sessionRowSubtitle(sess))
                      )
                      .child(
                        isRowMoving
                          ? div()
                              .id(`stream-session-moving-${sess.id}`)
                              .flex_none()
                              .text_size(10.5)
                              .text_color(t.dim)
                              .child("Moving…")
                          : isRowResuming
                            ? div()
                                .id(`stream-session-resuming-${sess.id}`)
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .tooltip("Resuming…")
                                .child(
                                  svg("mods/workspace-chrome/spinner.svg")
                                    .size(11)
                                    .text_color(t.accent)
                                    .spin(900)
                                )
                            : showRowActions
                              ? h_flex()
                                  .flex_none()
                                  .gap(2)
                                  .items_center()
                                  .when(isOpen && Boolean(liveMatch?.pane), (actions) =>
                                    actions.child(
                                      div()
                                        .id(`stream-session-close-${sess.id}`)
                                        .flex_none()
                                        .px(5)
                                        .py(1)
                                        .rounded(4)
                                        .cursor_pointer()
                                        .text_size(11)
                                        .text_color(t.dim)
                                        .hover((s) => s.bg(t.raised).text_color(t.blocked))
                                        .tooltip("Close terminal")
                                        .on_mouse_down("left", (_e, downCx) =>
                                          downCx.stop_propagation()
                                        )
                                        .on_click((_e, clickCx) => {
                                          clickCx.stop_propagation();
                                          run("pane.close", { pane: liveMatch.pane });
                                        })
                                        .child("×")
                                    )
                                  )
                                  .when(!isOpen, (actions) =>
                                    actions.child(
                                      div()
                                        .id(`stream-session-resume-${sess.id}`)
                                        .flex_none()
                                        .px(5)
                                        .py(1)
                                        .rounded(4)
                                        .cursor_pointer()
                                        .text_size(10.5)
                                        .text_color(t.accent)
                                        .hover((s) => s.bg(t.raised))
                                        .tooltip("Resume session")
                                        .on_mouse_down("left", (_e, downCx) =>
                                          downCx.stop_propagation()
                                        )
                                        .on_click((_e, clickCx) => {
                                          clickCx.stop_propagation();
                                          resumeSingleSession(clickCx, stream.key, sess.key);
                                        })
                                        .child("▶")
                                    )
                                  )
                                  .child(
                                    (() => {
                                      const menuTrigger = div()
                                        .id(`stream-session-menu-${sess.id}`)
                                        .flex_none()
                                        .px(4)
                                        .py(1)
                                        .rounded(4)
                                        .cursor_pointer()
                                        .text_color(
                                          rowActionOpen || isRowMovingMenu
                                            ? t.foreground
                                            : t.dim
                                        )
                                        .when(rowActionOpen || isRowMovingMenu, (elBtn) =>
                                          elBtn.bg(t.hover)
                                        )
                                        .hover((s) =>
                                          s.bg(t.hover).text_color(t.foreground)
                                        )
                                        .child("⋮");
                                      if (isRowMovingMenu) {
                                        return renderMovePopover(view, {
                                          id: `stream-session-move-pop-${sess.id}`,
                                          trigger: menuTrigger,
                                          snap,
                                          t,
                                        });
                                      }
                                      return renderMenuPopover(view, {
                                        id: rowActionMenuId,
                                        width: 190,
                                        anchor: "top_right",
                                        stopRowPropagation: true,
                                        t,
                                        items: rowMenuItems,
                                        trigger: menuTrigger,
                                      });
                                    })()
                                  )
                              : rowAge
                                ? div()
                                    .flex_none()
                                    .text_size(10.5)
                                    .text_color(t.dim)
                                    .child(rowAge)
                                : div()
                      );
                  })()
                );
              return renderMenuPopover(view, {
                id: rowCtxMenuId,
                width: 190,
                anchor: "top_left",
                mouseButton: "right",
                t,
                items: rowMenuItems,
                trigger: rowEl,
              });
            };

            return el.child(
              v_flex()
                .gap(8)
                .children(
                  nonEmptyGroups.map((group) => {
                    const savedState = view.sessionGroupsOpen?.[group.id];
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
                            view.sessionGroupsOpen[group.id] = !isExpanded;
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
