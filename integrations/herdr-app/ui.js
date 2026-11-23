// Shared visual primitives for Streams views: icons, status badges, host chips,
// reference icons, error banners, and unreadable-record notices.

import { div, svg, image } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";

export function icon(p, size, color) {
  if (!p?.icon) return null;
  return p.tint
    ? svg(p.icon).size(size).flex_none().text_color(color)
    : image(p.icon).w(size).h(size).flex_none();
}

export function statusStyle(status, t) {
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

export function statusBadge(status, t) {
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

export function statusDropdownBadge(status, t, { open = false, pending = false } = {}) {
  const s = statusStyle(status, t);
  return h_flex()
    .flex_none()
    .gap(4)
    .px(7)
    .py(2)
    .items_center()
    .rounded(5)
    .bg(open ? t.hover : t.raised)
    .border(1)
    .border_color(open ? t.accent : t.border)
    .text_size(11)
    .font_medium()
    .text_color(pending ? t.dim : s.color)
    .when(!pending, (el) => el.cursor_pointer().hover((st) => st.bg(t.hover)))
    .child(div().text_size(9).child(s.symbol))
    .child(pending ? `${s.label}…` : s.label)
    .child(div().text_size(9).text_color(t.dim).child("▾"));
}

export function foreignHostChip(badge, t, id) {
  if (!badge) return null;
  const isOnline = badge.state === "online";
  const isLoading = badge.state === "loading";
  const bg = isOnline
    ? t.ansi6 + "1a"
    : isLoading
      ? t.working + "16"
      : t.chrome;
  const border = isOnline
    ? t.ansi6 + "40"
    : isLoading
      ? t.working + "38"
      : t.border;
  const fg = isOnline
    ? t.ansi6
    : isLoading
      ? t.working
      : t.dim;
  return div()
    .id(id)
    .flex_none()
    .px(5)
    .py(1)
    .rounded(4)
    .border(1)
    .border_color(border)
    .bg(bg)
    .text_size(10.5)
    .when(isOnline || isLoading, (pill) => pill.font_medium())
    .text_color(fg)
    .tooltip(badge.tooltip)
    .child(badge.label);
}

export function referenceStyle(kind, t) {
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

export function referenceIcon(kind, t) {
  const r = referenceStyle(kind, t);
  return svg(r.icon).size(14).flex_none().text_color(r.color);
}

export function renderUnreadableNotice(notice, t) {
  return v_flex()
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
}

export function renderPinnedErrorBanner(parent, activeBanner, t) {
  return parent.when(Boolean(activeBanner), (el) => {
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
                  activeBanner.onDismiss?.();
                })
                .child("×")
            )
        )
        .child(div().text_size(11).text_color(t.foreground).child(activeBanner.message))
    );
  });
}

export function spawnSidebarWork(actCx, work) {
  if (!actCx?.spawn) return;
  actCx.spawn(async (asyncCx) => {
    try {
      await work(asyncCx);
    } finally {
      asyncCx.notify();
    }
  });
}
