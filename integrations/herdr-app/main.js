// Streams mod for herdr-app (roles: sidebar, frame, popup).
// Preserves the single 30 px pane header, explicit pane controls, and Files/Changes panels.

import { View, div, svg, image } from "gpui-kit";
import { h_flex, v_flex, ScrollHandle } from "gpui-base";
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
  home,
  layout,
  exec,
} from "herdr";
import { resolveRole, StreamsCoordinator } from "./state.js";

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
    focusedPane?.program?.agent && focusedPane?.terminal ? focusedPane.terminal : null;
  return {
    activeSpace,
    visibleTerminals,
    terminalList: terminals(),
    focusedTerminal,
  };
}

function snapshotSignature(snap) {
  return JSON.stringify({
    space: snap.activeSpace?.id ?? null,
    machine: snap.activeSpace?.machine ?? null,
    terminals: snap.visibleTerminals,
    focused: snap.focusedTerminal,
    labels: snap.terminalList
      .filter((t) => t.machine === null)
      .map((t) => [t.id, t.labels]),
  });
}

export default class StreamsMod extends View {
  init(_props, cx) {
    this.hovered = false;
    this.scroll = ScrollHandle.new();
    this._subscribed = false;
    this._lastSig = null;
    this.ensureSubscribed(cx);
    this.scheduleObserve(cx, collectSnapshot());
    cx?.timer?.every?.(2000, (timerCx) => {
      this.scheduleObserve(timerCx, collectSnapshot());
    });
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
    const sig = snapshotSignature(snap);
    if (
      !force &&
      sig === this._lastSig &&
      (coordinator.sidebar ||
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
      cx.spawn(async (asyncCx) => {
        try {
          await coordinator.observe({
            ...snap,
            force,
          });
        } finally {
          asyncCx.notify();
        }
      });
    } else {
      coordinator.observe({ ...snap, force });
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
    const session = rawProps.session;
    const streamTitle = rawProps.stream_title ?? null;
    const spaceId = rawProps.space_id ?? null;
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
        div()
          .text_color(t.muted)
          .child(`Status: ${session.status}${session.state ? ` · ${session.state}` : ""}`)
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
                    activeSpaceId: spaceId,
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
    const sb = coordinator.sidebar;

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

    if (coordinator.lastError && !sb) {
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

    if (!activeSpace) {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(
          div()
            .p(12)
            .text_size(12)
            .text_color(t.dim)
            .child("No active space.")
        );
    }

    if (!sb) {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(div().p(12).text_size(12).text_color(t.dim).child("Loading Streams…"));
    }

    if (sb.kind === "not_applicable") {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(
          div()
            .p(12)
            .text_size(12)
            .text_color(t.dim)
            .child("Streams sidebar is available on local spaces.")
        );
    }

    if (sb.kind === "unassociated") {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(
          div()
            .p(12)
            .text_size(12)
            .text_color(t.dim)
            .child("This space is not associated with a Stream.")
        );
    }

    if (sb.kind === "unavailable") {
      return v_flex()
        .size_full()
        .bg(t.chrome)
        .child(header)
        .child(
          div()
            .p(12)
            .text_size(12)
            .text_color(t.dim)
            .child(`Unavailable: ${sb.reason}`)
        );
    }

    const stream = sb.stream;
    const viewing = sb.viewing_session;
    const selectedSessionId = viewing?.kind === "selected" ? viewing.session : null;
    const selectedSession =
      stream.sessions.find((s) => s.id === selectedSessionId) ?? null;

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
          .gap(10)
          .overflow_y_scrollbar()
          .track_scroll(this.scroll)
          .child(
            v_flex()
              .gap(4)
              .child(div().font_semibold().text_size(13).child(stream.title))
              .child(div().text_color(t.muted).child(stream.status))
              .when(Boolean(stream.goal), (el) =>
                el.child(div().text_color(t.foreground).child(stream.goal))
              )
              .child(
                h_flex()
                  .pt(4)
                  .gap(6)
                  .child(
                    div()
                      .id("streams-open-work")
                      .px(8)
                      .py(3)
                      .rounded(5)
                      .bg(t.selected)
                      .hover((s) => s.bg(t.hover))
                      .on_click((_e, clickCx) => {
                        coordinator.openWork(
                          { stream: stream.id },
                          {
                            activeSpaceId: activeSpace?.id ?? null,
                            run,
                            openUrl: (u) => clickCx.open_url(u),
                          }
                        );
                      })
                      .child("Open Stream")
                  )
                  .child(
                    div()
                      .id("streams-follow-mode")
                      .px(8)
                      .py(3)
                      .rounded(5)
                      .bg(t.raised)
                      .hover((s) => s.bg(t.hover))
                      .on_click(() => {
                        if (activeSpace) coordinator.pinSession(activeSpace.id, null);
                      })
                      .child("Follow")
                  )
              )
          )
          .child(
            v_flex()
              .gap(4)
              .child(div().font_medium().text_color(t.muted).child("Sessions"))
              .children(
                stream.sessions.map((sess) =>
                  h_flex()
                    .id(`sess-${sess.id}`)
                    .px(8)
                    .py(4)
                    .rounded(5)
                    .justify_between()
                    .items_center()
                    .bg(sess.id === selectedSessionId ? t.selected : t.raised)
                    .hover((s) => s.bg(t.hover))
                    .on_click(() => {
                      if (activeSpace) coordinator.pinSession(activeSpace.id, sess.id);
                    })
                    .child(div().truncate().child(sess.title))
                    .child(div().text_color(t.dim).child(sess.status))
                )
              )
          )
          .when(Boolean(selectedSession), (el) =>
            el.child(
              v_flex()
                .p(8)
                .gap(4)
                .rounded(6)
                .bg(t.raised)
                .child(div().font_medium().child(selectedSession.title))
                .when(Boolean(selectedSession.state_short), (s) =>
                  s.child(div().text_color(t.muted).child(selectedSession.state_short))
                )
            )
          )
          .when(Boolean(stream.documents?.length), (el) =>
            el.child(
              v_flex()
                .gap(4)
                .child(div().font_medium().text_color(t.muted).child("Documents"))
                .children(
                  stream.documents.map((doc, idx) =>
                    div()
                      .id(`doc-${idx}`)
                      .px(8)
                      .py(4)
                      .rounded(5)
                      .bg(t.raised)
                      .hover((s) => s.bg(t.hover))
                      .on_click((_e, clickCx) => {
                        if (!selectedSessionId) return;
                        coordinator.openReference(
                          {
                            kind: "document",
                            stream: stream.id,
                            viewing_session: selectedSessionId,
                            document: doc.key,
                          },
                          {
                            activeSpaceId: activeSpace?.id ?? null,
                            run,
                            openUrl: (u) => clickCx.open_url(u),
                          }
                        );
                      })
                      .child(doc.description || doc.key.path)
                  )
                )
            )
          )
          .when(Boolean(stream.artifacts?.length), (el) =>
            el.child(
              v_flex()
                .gap(4)
                .child(div().font_medium().text_color(t.muted).child("Artifacts"))
                .children(
                  stream.artifacts.flatMap((group) =>
                    group.entries.map((entry) =>
                      div()
                        .id(`art-${entry.key.session}-${entry.key.name}`)
                        .px(8)
                        .py(4)
                        .rounded(5)
                        .bg(t.raised)
                        .hover((s) => s.bg(t.hover))
                        .on_click((_e, clickCx) => {
                          coordinator.openReference(
                            {
                              kind: "artifact",
                              key: entry.key,
                            },
                            {
                              activeSpaceId: activeSpace?.id ?? null,
                              run,
                              openUrl: (u) => clickCx.open_url(u),
                            }
                          );
                        })
                        .child(`${entry.key.name} — ${entry.description}`)
                    )
                  )
                )
            )
          )
          .when(Boolean(stream.links?.length), (el) =>
            el.child(
              v_flex()
                .gap(4)
                .child(div().font_medium().text_color(t.muted).child("Links"))
                .children(
                  stream.links.map((link, idx) =>
                    div()
                      .id(`link-${idx}`)
                      .px(8)
                      .py(4)
                      .rounded(5)
                      .bg(t.raised)
                      .hover((s) => s.bg(t.hover))
                      .on_click((_e, clickCx) => {
                        coordinator.openReference(
                          {
                            kind: "link",
                            stream: stream.id,
                            link: link.key,
                          },
                          {
                            activeSpaceId: activeSpace?.id ?? null,
                            run,
                            openUrl: (u) => clickCx.open_url(u),
                          }
                        );
                      })
                      .child(link.description || link.key.url)
                  )
                )
            )
          )
      );
  }

  render(cx) {
    this.ensureSubscribed(cx);
    const role = resolveRole(props());
    if (role === "popup") return this.renderPopup(cx);
    if (role === "frame") return this.renderFrame(cx);
    return this.renderSidebar(cx);
  }
}
