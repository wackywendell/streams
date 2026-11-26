// Pane frame role: renders the 30px pane header with program icon, working directory,
// enrolled Session pill (in multi-pane tabs), status indicator, and hover pane controls.

import { div, svg } from "gpui-kit";
import { h_flex } from "gpui-base";
import {
  spaces,
  panes,
  terminals,
  agents,
  props,
  run,
  openOverlay,
  theme,
  layout,
} from "herdr";
import {
  parseTerminalAddress,
  sessionForFramePane,
  effectiveSessionStatus,
} from "./state.js";
import {
  coordinator,
  findNode,
  paneFrame,
  isSidebarDrivingObserve,
  collectSnapshot,
  scheduleObserve,
  folder,
} from "./runtime.js";
import { icon, statusStyle, referenceIcon } from "./ui.js";

function paneButton(t, id, iconName, command, args) {
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

export function renderFrame(view, cx) {
  const t = theme();
  const rawProps = props() ?? {};
  const allPanes = panes();
  const p = allPanes.find(
    (item) =>
      item.pane === rawProps.pane ||
      (rawProps.terminal && item.terminal === rawProps.terminal)
  );
  if (!p) return div();

  if (!isSidebarDrivingObserve()) {
    scheduleObserve(view, cx, collectSnapshot());
  }

  const pane = p.pane;
  const bottom = paneFrame(layout())?.header?.position === "bottom";

  if (p.mod === "streams") {
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
        view.hovered = hovered;
        hoverCx.notify();
      })
      .on_click(() => run("pane.focus", { pane }))
      .child(referenceIcon("document", t))
      .child(
        div()
          .flex_1()
          .min_w(0)
          .truncate()
          .font_medium()
          .text_color(t.foreground)
          .child("Reference")
      )
      .when(view.hovered, (el) =>
        el
          .child(
            paneButton(
              t,
              `ph-z-${pane}`,
              p.zoomed ? "unzoom" : "zoom",
              "pane.zoom",
              { pane }
            )
          )
          .child(paneButton(t, `ph-x-${pane}`, "close", "pane.close", { pane }))
      );
  }

  const frameInfo = sessionForFramePane({
    pane: p,
    terminalList: terminals(),
    agents: agents(),
    staleSessionKeys: coordinator.staleSessionKeys,
    sessions: coordinator.sessions,
  });
  const enrolled = frameInfo?.session ?? null;
  const streamTitle = enrolled ? coordinator.streamTitle(enrolled.stream) : null;
  const effectiveStatus = enrolled
    ? effectiveSessionStatus(enrolled, {
        pane: p,
        terminal: frameInfo.terminal,
        agent: frameInfo.agent,
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
  const tabPaneCount = allPanes.filter((paneItem) => paneItem.tab === p.tab).length;
  const activeSpace = spaces().find((s) => s.active) ?? null;
  const paneMachine =
    parseTerminalAddress(p.terminal)?.machine ?? activeSpace?.machine ?? null;

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
      view.hovered = hovered;
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
    .child(div().min_w(0).truncate().text_color(t.dim).child(folder(p.cwd, paneMachine)))
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
                const liveSession = coordinator.sessions.get(enrolled.key) ?? enrolled;
                const liveStreamTitle = liveSession.stream
                  ? coordinator.streamTitle(liveSession.stream)
                  : streamTitle;
                openOverlay({
                  mod: "streams",
                  kind: "popup",
                  at: "pointer",
                  width: 420,
                  height: 320,
                  props: {
                    kind: "session-popup",
                    session: liveSession,
                    stream_title: liveStreamTitle,
                    space_id: activeSpace?.id ?? null,
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
    .when(view.hovered, (el) =>
      el
        .child(
          paneButton(t, `ph-r-${pane}`, "split-right", "pane.split", {
            direction: "right",
            pane,
          })
        )
        .child(
          paneButton(t, `ph-d-${pane}`, "split-down", "pane.split", {
            direction: "down",
            pane,
          })
        )
        .child(
          paneButton(
            t,
            `ph-z-${pane}`,
            p.zoomed ? "unzoom" : "zoom",
            "pane.zoom",
            { pane }
          )
        )
        .child(paneButton(t, `ph-x-${pane}`, "close", "pane.close", { pane }))
    );
}
