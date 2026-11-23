// Sidebar "All" view: searchable, sortable, host-scoped Stream list grouped by openness.

import { div } from "gpui-kit";
import { h_flex, v_flex, Input } from "gpui-base";
import { run } from "herdr";
import {
  groupHostStreams,
  formatHostStreamMeta,
  unreadableNotice,
  hostStreamsEmptyMessage,
  formatRelativeAge,
} from "./state.js";
import { coordinator } from "./runtime.js";
import {
  statusBadge,
  foreignHostChip,
  renderUnreadableNotice,
  renderPinnedErrorBanner,
} from "./ui.js";

export function renderAllView({
  view,
  t,
  header,
  snap,
  live,
  activeSpace,
  hostBucket,
  activeBanner,
  resolvedView,
}) {
  const streamList = hostBucket.items;
  const unreadable = unreadableNotice(hostBucket.unreadable);
  const openCounts = live.openCountByStream;
  const streamGroups = groupHostStreams(streamList, {
    openCounts,
    sort: view.streamSort,
    query: view.streamQuery,
  });
  const nonEmptyStreamGroups = streamGroups.filter((g) => g.streams.length > 0);
  const shownStreams = nonEmptyStreamGroups.flatMap((g) => g.streams);
  const hasQuery = Boolean(view.streamQuery.trim());
  const onlyFinishedStreams =
    nonEmptyStreamGroups.length === 1 && nonEmptyStreamGroups[0].id === "done_closed";
  const hostScope = coordinator.hostScope;
  const localHostLabel = activeSpace.machine ?? "This host";
  const emptyMessage = hostStreamsEmptyMessage({
    error: activeBanner?.kind === "read" ? null : hostBucket.error,
    streams: streamList,
    shown: shownStreams,
    unreadable: hostBucket.unreadable,
    scope: hostScope,
    loaded: hostBucket.loaded,
  });

  const toolbar = v_flex()
    .px(12)
    .py(8)
    .gap(6)
    .border_b(1)
    .border_color(t.border)
    .when(Boolean(view.searchInput), (el) =>
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
            Input.new(view.searchInput)
              .flex_1()
              .h_full()
              .text_size(12)
              .text_color(t.foreground)
          )
          .when(Boolean(view.streamQuery), (bar) =>
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
                  view.streamQuery = "";
                  view.searchInput?.set_value?.("");
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
        .gap(6)
        .text_size(11)
        .child(
          div()
            .text_color(t.dim)
            .child(`${shownStreams.length} stream${shownStreams.length === 1 ? "" : "s"}`)
        )
        .child(
          h_flex()
            .flex_none()
            .gap(4)
            .items_center()
            .child(
              div()
                .id("streams-scope-toggle")
                .px(6)
                .py(2)
                .rounded(4)
                .text_color(hostScope === "all" ? t.foreground : t.muted)
                .when(hostScope === "all", (el) => el.bg(t.raised))
                .hover((s) => s.bg(t.hover).text_color(t.foreground))
                .tooltip(
                  hostScope === "all"
                    ? `Showing all synced hosts — click for ${localHostLabel} only`
                    : `Showing Streams on ${localHostLabel} — click for all synced hosts`
                )
                .on_click((_e, clickCx) => {
                  clickCx.stop_propagation();
                  const nextScope = coordinator.hostScope === "all" ? "local" : "all";
                  clickCx.spawn(async () => {
                    await coordinator.setHostScope(nextScope);
                  });
                })
                .child(hostScope === "all" ? "All hosts" : localHostLabel)
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
                  view.streamSort === "recent"
                    ? "Sorted by most recent — click for A–Z"
                    : "Sorted A–Z — click for most recent"
                )
                .on_click((_e, clickCx) => {
                  clickCx.stop_propagation();
                  view.streamSort = view.streamSort === "recent" ? "alpha" : "recent";
                  clickCx.notify();
                })
                .child(view.streamSort === "recent" ? "↕ Recent" : "↕ A–Z")
            )
        )
    );

  const currentAllStreamKey = resolvedView.currentStreamKey ?? null;

  const renderStreamRow = (summary, idx, groupId) => {
    const openCount = openCounts.get(summary.key) ?? 0;
    const isCurrentStream = Boolean(currentAllStreamKey && summary.key === currentAllStreamKey);
    const age = formatRelativeAge(summary.last_activity_ms);
    const hostBadge =
      hostScope === "all"
        ? coordinator.foreignHostBadge(summary, activeSpace.machine ?? null)
        : null;
    const meta = formatHostStreamMeta({
      groupId,
      openCount,
      activeSessions: summary.active_sessions,
      goalShort: summary.goal_short,
      hostBadge,
    });
    return v_flex()
      .id(`host-stream-${summary.key}`)
      .px(9)
      .py(8)
      .gap(3)
      .when(idx > 0, (row) => row.border_t(1).border_color(t.border))
      .when(isCurrentStream, (row) => row.bg(t.selected))
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
          .child(div().w(12).flex_none().text_size(12).text_color(t.accent).child("≋"))
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
      .when(meta.hasSecondLine, (row) =>
        row.child(
          h_flex()
            .pl(18)
            .gap(6)
            .items_center()
            .child(
              div()
                .flex_1()
                .min_w(0)
                .truncate()
                .text_size(11)
                .text_color(t.muted)
                .child(meta.goalText)
            )
            .when(Boolean(meta.hostBadge || meta.countText), (line) =>
              line.child(
                h_flex()
                  .flex_none()
                  .gap(6)
                  .items_center()
                  .when(Boolean(meta.hostBadge), (right) =>
                    right.child(
                      foreignHostChip(meta.hostBadge, t, `host-stream-pill-${summary.key}`)
                    )
                  )
                  .when(Boolean(meta.countText), (right) =>
                    right.child(
                      div()
                        .flex_none()
                        .text_size(10.5)
                        .text_color(t.dim)
                        .child(meta.countText)
                    )
                  )
              )
            )
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
      .child(toolbar),
    activeBanner,
    t
  ).child(
    v_flex()
      .id("streams-body-all")
      .flex_1()
      .p(12)
      .gap(10)
      .overflow_y_scrollbar()
      .track_scroll(view.scrollAll)
      .when(Boolean(unreadable), (el) => el.child(renderUnreadableNotice(unreadable, t)))
      .when(Boolean(emptyMessage), (el) =>
        el.child(div().text_size(12).text_color(t.dim).child(emptyMessage))
      )
      .when(nonEmptyStreamGroups.length > 0, (el) =>
        el.child(
          v_flex()
            .gap(8)
            .children(
              nonEmptyStreamGroups.map((group) => {
                const savedState = view.allGroupsOpen?.[group.id];
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
                        view.allGroupsOpen[group.id] = !isExpanded;
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
                        .children(
                          group.streams.map((summary, idx) =>
                            renderStreamRow(summary, idx, group.id)
                          )
                        )
                    )
                  );
              })
            )
        )
      )
  );
}
