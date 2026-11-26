// Sidebar "Session" view: Session header, breadcrumb, status dropdown, recap/next,
// and collapsible References section.

import { div } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { effectiveSessionStatus, buildReferenceItems } from "./state.js";
import { coordinator } from "./runtime.js";
import {
  icon,
  statusBadge,
  statusDropdownBadge,
  foreignHostChip,
  renderPinnedErrorBanner,
} from "./ui.js";
import {
  isMenuOpen,
  startRename,
  renderInlineRename,
  renderMenuPopover,
} from "./popovers.js";

export function renderSessionView({
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
}) {
  const session = resolvedView.session ?? null;
  const parentStream = resolvedView.stream ?? null;

  if (!session) {
    return renderPinnedErrorBanner(
      v_flex()
        .size_full()
        .bg(t.chrome)
        .text_size(12)
        .text_color(t.foreground)
        .child(header),
      activeBanner,
      t
    ).child(div().p(12).text_color(t.dim).child("Loading Session…"));
  }

  const tabPane = resolvedView.terminal
    ? snap.panes.find((p) => p.terminal === resolvedView.terminal) ?? null
    : null;
  const termInfo = resolvedView.terminal
    ? snap.terminalList.find((item) => (item.address ?? item.id) === resolvedView.terminal) ?? null
    : null;
  const liveAgent = resolvedView.terminal
    ? snap.agents.find((a) => a.address === resolvedView.terminal) ?? null
    : null;
  const effectiveStatus = resolvedView.isUnopened
    ? session.status
    : effectiveSessionStatus(session, {
        pane: tabPane,
        terminal: termInfo,
        agent: liveAgent,
      });
  const sessionIcon = resolvedView.isUnopened
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
      .child(header),
    activeBanner,
    t
  ).child(
    v_flex()
      .id("streams-body-session")
      .flex_1()
      .p(12)
      .gap(12)
      .overflow_y_scrollbar()
      .track_scroll(view.scrollSession)
      .when(resolvedView.isPreview && resolvedView.isUnopened, (el) =>
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
                  (() => {
                    const isResumingPreview = coordinator.isPending(`resume:${session.key}`);
                    return div()
                      .id("session-preview-resume")
                      .px(8)
                      .py(3)
                      .rounded(4)
                      .bg(isResumingPreview ? t.raised : t.accent)
                      .font_medium()
                      .text_size(11.5)
                      .text_color(isResumingPreview ? t.dim : t.background)
                      .when(!isResumingPreview, (btn) =>
                        btn.cursor_pointer().on_click((_e, clickCx) => {
                          clickCx.stop_propagation();
                          resumeSingleSession(clickCx, streamKey, session.key);
                        })
                      )
                      .child(isResumingPreview ? "Resuming…" : "▶ Resume");
                  })()
                )
                .when(resolvedView.canReturnToCurrent, (bar) =>
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
                        coordinator.returnToCurrent({ isEnrolled: resolvedView.isEnrolled });
                      })
                      .child("Back to current")
                  )
                )
            )
        )
      )
      .when(
        Boolean(streamLabel) || Boolean(coordinator.foreignHostBadge(session, activeSpace?.machine)),
        (el) => {
          const sessionForeignHost = coordinator.foreignHostBadge(session, activeSpace?.machine);
          return el.child(
            h_flex()
              .gap(6)
              .items_center()
              .justify_between()
              .when(Boolean(streamLabel), (row) =>
                row.child(
                  h_flex()
                    .id("session-stream-breadcrumb")
                    .min_w(0)
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
                      if (resolvedView.isPreview) {
                        coordinator.backToStream();
                      } else {
                        coordinator.setView("stream");
                      }
                    })
                    .child(div().flex_none().text_color(t.accent).child("≋"))
                    .child(div().truncate().child(streamLabel))
                )
              )
              .when(!streamLabel, (row) => row.child(div().flex_1()))
              .when(Boolean(sessionForeignHost), (row) =>
                row.child(foreignHostChip(sessionForeignHost, t, "session-header-host"))
              )
          );
        }
      )
      .child(
        v_flex()
          .gap(8)
          .child(
            (() => {
              const sessionForeignHost = coordinator.foreignHostBadge(
                session,
                activeSpace?.machine
              );
              const sessionWritable = !(
                sessionForeignHost && sessionForeignHost.state !== "online"
              );
              const isRenamingSession =
                view.interaction?.kind === "rename" &&
                view.interaction.entityKind === "session" &&
                view.interaction.key === session.key;
              const isSessionRenamePending = coordinator.isPending(`rename:${session.key}`);
              return h_flex()
                .gap(6)
                .items_center()
                .when(Boolean(sessionIcon), (el) => el.child(sessionIcon))
                .child(
                  isRenamingSession
                    ? renderInlineRename(view, t, { id: "session-title-rename" })
                    : div()
                        .id("session-header-title")
                        .flex_1()
                        .min_w(0)
                        .truncate()
                        .font_semibold()
                        .text_size(13.5)
                        .when(isSessionRenamePending, (el) => el.text_color(t.dim))
                        .when(sessionWritable && !isSessionRenamePending, (el) =>
                          el
                            .tooltip("Double-click to rename")
                            .on_mouse_down("left", (ev, downCx) => {
                              if (ev.click_count === 2) {
                                downCx.stop_propagation();
                                startRename(
                                  view,
                                  {
                                    kind: "session",
                                    key: session.key,
                                    streamKey,
                                    title: session.title,
                                  },
                                  downCx
                                );
                              }
                            })
                        )
                        .child(
                          isSessionRenamePending ? `${session.title}…` : session.title
                        )
                )
                .when(!isRenamingSession, (row) =>
                  row.child(
                    (() => {
                      if (!sessionWritable) {
                        return statusBadge(effectiveStatus, t)
                          .id("session-header-status-ro")
                          .tooltip(sessionForeignHost.tooltip);
                      }
                      if (coordinator.isPending(`status:${session.key}`)) {
                        return statusDropdownBadge(effectiveStatus, t, { pending: true })
                          .id("session-header-status-pending")
                          .tooltip("Updating status…");
                      }
                      const statusMenuId = `session-status-${session.key}`;
                      return renderMenuPopover(view, {
                        id: statusMenuId,
                        width: 155,
                        anchor: "top_right",
                        t,
                        items: () =>
                          buildStatusMenuItems({
                            sessionKey: session.key,
                            streamKey,
                            status: session.status,
                          }),
                        trigger: statusDropdownBadge(effectiveStatus, t, {
                          open: isMenuOpen(view, statusMenuId),
                        })
                          .id("session-header-status-btn")
                          .tooltip("Change status"),
                      });
                    })()
                  )
                );
            })()
          )
          .when(!session.detail, (el) =>
            el.child(
              div()
                .text_size(11.5)
                .text_color(t.dim)
                .child("Loading Session details…")
            )
          )
          .when(
            session.detail &&
              !resolvedView.isUnopened &&
              !isRefreshing &&
              session.status !== "done" &&
              session.status !== "closed" &&
              !session.recap &&
              !session.next &&
              references.length === 0,
            (el) =>
              el.child(
                div()
                  .text_size(11.5)
                  .text_color(t.dim)
                  .child("Waiting for first checkpoint…")
              )
          )
          .when(Boolean(session.recap), (el) =>
            el.child(
              v_flex()
                .gap(2)
                .child(div().text_size(10.5).font_medium().text_color(t.dim).child("Recap"))
                .child(div().text_size(12).text_color(t.foreground).child(session.recap))
            )
          )
          .when(Boolean(session.next), (el) =>
            el.child(
              v_flex()
                .gap(2)
                .child(div().text_size(10.5).font_medium().text_color(t.dim).child("Next"))
                .child(div().text_size(11.5).text_color(t.muted).child(session.next))
            )
          )
      )
      .when(references.length > 0, (el) => el.child(renderReferencesSection(references)))
  );
}
