// Native reference viewer and Session popup overlay roles.

import { div } from "gpui-kit";
import { h_flex, v_flex, Editor, EditorState, TextView } from "gpui-base";
import {
  panes,
  focused,
  props,
  run,
  openOverlay,
  closeOverlay,
  theme,
  settings,
  layout,
} from "herdr";
import { coordinator, paneFrame, collectSnapshot } from "./runtime.js";
import { statusBadge } from "./ui.js";

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

export function isMarkdownFilename(filename) {
  return typeof filename === "string" && /\.md$/i.test(filename.trim());
}

export function viewerLanguage(filename) {
  if (typeof filename !== "string" || !filename.includes(".")) return "txt";
  const ext = filename.split(".").pop().toLowerCase();
  return VIEWER_LANG[ext] ?? ext;
}

export function syncViewerState(view, rawProps) {
  const filename = rawProps?.filename ?? "";
  const content = rawProps?.content ?? "";
  const key = `${filename}\0${content}`;
  if (view._viewerKey === key) return;
  view._editorState?.release?.();
  view._editorState = null;
  view._viewerKey = key;
  if (!isMarkdownFilename(filename) && typeof content === "string") {
    const t = theme();
    const s = settings?.() ?? {};
    view._editorState = EditorState.new({
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

export function renderPopup() {
  const t = theme();
  const rawProps = props() ?? {};
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

export function renderViewer(view) {
  const t = theme();
  const rawProps = props() ?? {};
  syncViewerState(view, rawProps);
  const title = rawProps.title || rawProps.filename || "Reference";
  const filename = rawProps.filename || "";
  const showFilename = Boolean(filename && filename !== title);
  const monoFont = settings()?.font_family ?? "JetBrains Mono";
  const overlayId = rawProps.overlay ?? null;
  const isMd = isMarkdownFilename(filename);

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
    .child(
      h_flex()
        .h(hasOuterPaneHeader ? 26 : 30)
        .flex_none()
        .px(10)
        .gap(8)
        .items_center()
        .bg(hasOuterPaneHeader ? t.background : t.chrome)
        .border_b(1)
        .border_color(t.border)
        .child(
          div()
            .flex_none()
            .max_w(280)
            .truncate()
            .font_medium()
            .child(title)
        )
        .when(showFilename, (bar) =>
          bar.child(
            div()
              .min_w(0)
              .truncate()
              .font_family(monoFont)
              .text_size(11)
              .text_color(t.dim)
              .child(filename)
          )
        )
        .child(div().flex_1())
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
        : view._editorState
          ? Editor.new(view._editorState).flex_1().min_h(0)
          : div().p(12).text_color(t.dim).child("No content.")
    );
}
