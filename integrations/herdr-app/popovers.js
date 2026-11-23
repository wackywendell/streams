// Interactive sidebar popovers and inline editors: action/context menus,
// Move-to-Stream picker, and inline Session/Stream title rename.

import { div } from "gpui-kit";
import { h_flex, v_flex, Popover, Input } from "gpui-base";
import { recordHostId, indexLive, buildMoveCandidates } from "./state.js";
import { coordinator, collectSnapshot } from "./runtime.js";
import { statusStyle } from "./ui.js";

export function setInteraction(view, next, cx) {
  view.interaction = next;
  cx?.notify?.();
}

export function isMenuOpen(view, id) {
  return view.interaction?.kind === "menu" && view.interaction.id === id;
}

export function openMoveMenu(view, { sessionKey, sourceStreamKey, sessionTitle = "" }, cx) {
  if (!sessionKey || !view.moveSearchInput) return;
  const owner = coordinator.resolveReadMachineForKey(sessionKey);
  const machine = owner.unavailable ? null : owner.machine ?? null;
  const hostId = recordHostId(sessionKey) ?? recordHostId(sourceStreamKey);
  view.moveSearchInput.set_value("");
  setInteraction(
    view,
    {
      kind: "move",
      sessionKey,
      sourceStreamKey,
      sessionTitle,
      machine,
      hostId,
      selectedStreamKey: null,
    },
    cx
  );
  view.moveSearchInput.focus();
  if (cx?.spawn) {
    cx.spawn(async (asyncCx) => {
      try {
        await coordinator.ensureHostStreams(false, { machine, scope: "local" });
      } finally {
        asyncCx.notify();
      }
    });
  }
}

export function closeMoveMenu(view, cx) {
  if (view.interaction?.kind !== "move") return;
  setInteraction(view, null, cx);
}

export function moveCandidates(view, state, snap) {
  if (!state) return [];
  const byKey = new Map();
  const bucket = coordinator.getStreamList(state.machine, "local");
  for (const item of bucket.items ?? []) {
    if (item?.key) byKey.set(item.key, item);
  }
  for (const st of coordinator.streams.values()) {
    if (st?.key && !byKey.has(st.key)) byKey.set(st.key, st);
  }
  const live = indexLive(snap, {
    sessions: coordinator.sessions,
    staleSessionKeys: coordinator.staleSessionKeys,
    lastFocusedStreamBySpace: coordinator.lastFocusedStreamBySpace,
  });
  return buildMoveCandidates({
    sourceStreamKey: state.sourceStreamKey,
    hostId: state.hostId,
    streams: Array.from(byKey.values()),
    openCounts: live.openCountByStream,
    query: view.moveSearchInput?.value?.() ?? "",
  });
}

export function commitMove(view, cx, explicitTargetKey = null) {
  if (view.interaction?.kind !== "move") return;
  const state = view.interaction;
  const snap = collectSnapshot();
  const candidates = moveCandidates(view, state, snap);
  const chosenKey =
    explicitTargetKey ??
    (state.selectedStreamKey && candidates.some((c) => c.stream.key === state.selectedStreamKey)
      ? state.selectedStreamKey
      : null);
  if (!chosenKey) return;
  const chosen = candidates.find((c) => c.stream.key === chosenKey);
  if (!chosen) return;

  const live = indexLive(snap, {
    sessions: coordinator.sessions,
    staleSessionKeys: coordinator.staleSessionKeys,
    lastFocusedStreamBySpace: coordinator.lastFocusedStreamBySpace,
  });
  setInteraction(view, null, cx);
  if (!cx?.spawn) return;
  cx.spawn(async (asyncCx) => {
    try {
      await coordinator.moveSessionToStream({
        sessionKey: state.sessionKey,
        sourceStreamKey: state.sourceStreamKey,
        targetStreamKey: chosen.stream.key,
        live,
        terminalList: snap.terminalList,
        panes: snap.panes,
        agents: snap.agents,
      });
    } finally {
      asyncCx.notify();
    }
  });
}

export function renderMovePopover(view, { id, trigger, snap, t }) {
  if (view.interaction?.kind !== "move") return trigger;
  const state = view.interaction;
  const candidates = moveCandidates(view, state, snap);
  const bucket = coordinator.getStreamList(state.machine, "local");
  const isLoadingStreams = !bucket.loaded && !bucket.error && candidates.length === 0;
  const queryText = (view.moveSearchInput?.value?.() ?? "").trim();

  if (state.selectedStreamKey === null && candidates.length > 0) {
    state.selectedStreamKey = candidates[0].stream.key;
  }
  const selectedIdx = candidates.findIndex((c) => c.stream.key === state.selectedStreamKey);
  const effectiveCandidate = selectedIdx >= 0 ? candidates[selectedIdx] : null;

  const panel = v_flex()
    .id(`${id}-panel`)
    .w(260)
    .p(8)
    .gap(6)
    .rounded(6)
    .bg(t.popover)
    .border(1)
    .border_color(t.border)
    .shadow_md()
    .text_size(12)
    .text_color(t.foreground)
    .on_mouse_down("left", (_e, downCx) => downCx.stop_propagation())
    .on_click((_e, clickCx) => clickCx.stop_propagation())
    .on_key_down((e, keyCx) => {
      const k = String(e.key ?? "").toLowerCase();
      if (k === "escape") {
        keyCx.stop_propagation();
        closeMoveMenu(view, keyCx);
        return;
      }
      if ((k === "down" || k === "arrowdown") && candidates.length > 0) {
        keyCx.stop_propagation();
        const next = selectedIdx < 0 ? 0 : (selectedIdx + 1) % candidates.length;
        setInteraction(
          view,
          {
            ...state,
            selectedStreamKey: candidates[next].stream.key,
          },
          keyCx
        );
        return;
      }
      if ((k === "up" || k === "arrowup") && candidates.length > 0) {
        keyCx.stop_propagation();
        const next =
          selectedIdx <= 0 ? candidates.length - 1 : (selectedIdx - 1) % candidates.length;
        setInteraction(
          view,
          {
            ...state,
            selectedStreamKey: candidates[next].stream.key,
          },
          keyCx
        );
        return;
      }
      if (k === "enter" || k === "return") {
        keyCx.stop_propagation();
        commitMove(view, keyCx);
      }
    })
    .child(
      h_flex()
        .gap(6)
        .items_center()
        .justify_between()
        .child(div().font_semibold().text_size(12).child("Move to stream"))
        .child(
          div()
            .flex_1()
            .min_w(0)
            .truncate()
            .text_size(10.5)
            .text_color(t.dim)
            .child(state.sessionTitle || "")
        )
    )
    .child(
      h_flex()
        .h(26)
        .px(7)
        .items_center()
        .rounded(4)
        .border(1)
        .border_color(t.border)
        .bg(t.background)
        .child(
          Input.new(view.moveSearchInput)
            .flex_1()
            .h_full()
            .text_size(11.5)
            .text_color(t.foreground)
        )
    )
    .when(Boolean(bucket.error), (el) =>
      el.child(
        div()
          .px(6)
          .py(4)
          .rounded(4)
          .bg(t.raised)
          .text_size(11)
          .text_color(t.blocked)
          .child(`Could not load streams: ${bucket.error}`)
      )
    )
    .child(
      v_flex()
        .id(`${id}-list`)
        .max_h(180)
        .overflow_y_scrollbar()
        .rounded(4)
        .border(1)
        .border_color(t.border)
        .bg(t.background)
        .when(isLoadingStreams, (el) =>
          el.child(
            div().p(8).text_size(11.5).text_color(t.dim).child("Loading streams…")
          )
        )
        .when(!isLoadingStreams && candidates.length === 0, (el) =>
          el.child(
            div()
              .p(8)
              .text_size(11.5)
              .text_color(t.dim)
              .child(
                bucket.error
                  ? "Stream list unavailable."
                  : queryText
                    ? "No matching streams on this host."
                    : "No other streams on this host."
              )
          )
        )
        .when(candidates.length > 0, (el) =>
          el.children(
            candidates.map((cand, idx) => {
              const isSelected = idx === selectedIdx;
              const stStyle = statusStyle(cand.stream.status, t);
              return h_flex()
                .id(`${id}-cand-${idx}`)
                .px(8)
                .py(5)
                .gap(6)
                .items_center()
                .cursor_pointer()
                .when(idx > 0, (r) => r.border_t(1).border_color(t.border))
                .when(isSelected, (r) => r.bg(t.selected))
                .hover((s) => s.bg(t.hover))
                .on_mouse_down("left", (ev, downCx) => {
                  downCx.stop_propagation();
                  if (ev.click_count === 2) {
                    commitMove(view, downCx, cand.stream.key);
                    return;
                  }
                  if (state.selectedStreamKey !== cand.stream.key) {
                    setInteraction(
                      view,
                      {
                        ...state,
                        selectedStreamKey: cand.stream.key,
                      },
                      downCx
                    );
                  }
                })
                .child(
                  div()
                    .flex_none()
                    .text_size(9)
                    .text_color(stStyle.color)
                    .child(stStyle.symbol)
                )
                .child(
                  div()
                    .flex_1()
                    .min_w(0)
                    .truncate()
                    .text_size(11.5)
                    .when(isSelected, (elTitle) => elTitle.font_medium())
                    .text_color(cand.isArchived ? t.muted : t.foreground)
                    .child(cand.stream.title)
                )
                .when(cand.openCount > 0, (r) =>
                  r.child(
                    div()
                      .flex_none()
                      .px(4)
                      .py(1)
                      .rounded(3)
                      .bg(t.raised)
                      .text_size(10)
                      .text_color(t.dim)
                      .child(`${cand.openCount} open`)
                  )
                );
            })
          )
        )
    )
    .child(
      h_flex()
        .pt(2)
        .gap(6)
        .justify_end()
        .items_center()
        .child(
          div()
            .id(`${id}-cancel`)
            .px(8)
            .py(3)
            .rounded(4)
            .cursor_pointer()
            .text_size(11.5)
            .text_color(t.muted)
            .hover((s) => s.bg(t.hover).text_color(t.foreground))
            .on_click((_e, clickCx) => {
              clickCx.stop_propagation();
              closeMoveMenu(view, clickCx);
            })
            .child("Cancel")
        )
        .child(
          div()
            .id(`${id}-confirm`)
            .px(10)
            .py(3)
            .rounded(4)
            .font_medium()
            .text_size(11.5)
            .bg(effectiveCandidate ? t.accent : t.raised)
            .text_color(effectiveCandidate ? t.background : t.dim)
            .when(Boolean(effectiveCandidate), (btn) =>
              btn
                .cursor_pointer()
                .hover((s) => s.opacity(0.9))
                .on_click((_e, clickCx) => {
                  clickCx.stop_propagation();
                  commitMove(view, clickCx);
                })
            )
            .child("Move")
        )
    );

  return Popover.new(id)
    .open(true)
    .on_open_change((nextOpen, popCx) => {
      if (!nextOpen) closeMoveMenu(view, popCx);
    })
    .anchor("top_right")
    .trigger(trigger)
    .content(panel);
}

export function startRename(view, { kind, key, streamKey = null, title = "" }, cx) {
  if (!key || !view.renameInput) return;
  view.renameInput.set_value(title);
  if (title) {
    view.renameInput.set_selected_range({ start: 0, end: title.length });
  }
  setInteraction(
    view,
    { kind: "rename", entityKind: kind, key, streamKey, originalTitle: title },
    cx
  );
  view.renameInput.focus();
}

export function cancelRename(view, cx) {
  if (view.interaction?.kind !== "rename") return;
  setInteraction(view, null, cx);
}

export function commitRename(view, cx) {
  if (view.interaction?.kind !== "rename") return;
  const target = view.interaction;
  const trimmed = (view.renameInput?.value?.() ?? "").trim();
  if (!trimmed) return;
  setInteraction(view, null, cx);
  if (trimmed === target.originalTitle) return;
  if (!cx?.spawn) return;
  cx.spawn(async (asyncCx) => {
    try {
      if (target.entityKind === "stream") {
        await coordinator.renameStream(target.key, trimmed);
      } else {
        await coordinator.renameSession(target.key, trimmed, target.streamKey);
      }
    } finally {
      asyncCx.notify();
    }
  });
}

export function renderInlineRename(view, t, { id, compact = false } = {}) {
  const canSave = (view.renameInput?.value?.() ?? "").trim().length > 0;
  return h_flex()
    .id(id)
    .flex_1()
    .min_w(0)
    .gap(4)
    .items_center()
    .on_mouse_down("left", (_e, downCx) => downCx.stop_propagation())
    .on_click((_e, clickCx) => clickCx.stop_propagation())
    .on_key_down((e, keyCx) => {
      if (String(e.key ?? "").toLowerCase() === "escape") {
        keyCx.stop_propagation();
        cancelRename(view, keyCx);
      }
    })
    .child(
      div()
        .flex_1()
        .min_w(0)
        .h(compact ? 22 : 26)
        .px(6)
        .flex()
        .items_center()
        .rounded(4)
        .border(1)
        .border_color(t.accent)
        .bg(t.background)
        .child(
          Input.new(view.renameInput)
            .flex_1()
            .h_full()
            .text_size(compact ? 11.5 : 12.5)
            .text_color(t.foreground)
        )
    )
    .child(
      div()
        .id(`${id}-save`)
        .flex_none()
        .px(5)
        .py(compact ? 1 : 2)
        .rounded(4)
        .text_size(11)
        .text_color(canSave ? t.ansi2 : t.dim)
        .when(canSave, (el) =>
          el
            .cursor_pointer()
            .hover((s) => s.bg(t.hover))
            .on_click((_e, clickCx) => {
              clickCx.stop_propagation();
              commitRename(view, clickCx);
            })
        )
        .tooltip("Save (Enter)")
        .child("✓")
    )
    .child(
      div()
        .id(`${id}-cancel`)
        .flex_none()
        .px(5)
        .py(compact ? 1 : 2)
        .rounded(4)
        .cursor_pointer()
        .text_size(11)
        .text_color(t.dim)
        .hover((s) => s.bg(t.hover).text_color(t.foreground))
        .tooltip("Cancel (Esc)")
        .on_click((_e, clickCx) => {
          clickCx.stop_propagation();
          cancelRename(view, clickCx);
        })
        .child("×")
    );
}

export function renderMenuPopover(
  view,
  {
    id,
    trigger,
    items,
    width = 185,
    anchor = "top_right",
    mouseButton = "left",
    stopRowPropagation = false,
    t,
  }
) {
  const isOpen = isMenuOpen(view, id);

  const buildContent = () => {
    const resolvedItems = typeof items === "function" ? items() : items;
    const selectable = [];
    for (let i = 0; i < resolvedItems.length; i += 1) {
      if (!resolvedItems[i].type && !resolvedItems[i].disabled) selectable.push(i);
    }
    const firstIndex = selectable.length > 0 ? selectable[0] : -1;
    const curIndex = view.interaction?.kind === "menu" ? view.interaction.index : -1;
    const activeIndex = selectable.includes(curIndex) ? curIndex : firstIndex;

    const closeMenu = (closeCx) => {
      if (isMenuOpen(view, id)) setInteraction(view, null, closeCx);
    };

    const runSelected = (idx, actCx) => {
      const item = resolvedItems[idx];
      if (!item || item.type || item.disabled) return;
      view.interaction = null;
      try {
        item.onSelect?.(actCx);
      } finally {
        actCx?.notify?.();
      }
    };

    return v_flex()
      .id(`${id}-content`)
      .w(width)
      .p(5)
      .rounded(8)
      .border_1()
      .border_color(t.border)
      .bg(t.popover)
      .shadow_lg()
      .text_size(12)
      .text_color(t.foreground)
      .when(Boolean(view.menuFocus), (el) => el.track_focus(view.menuFocus))
      .on_mouse_down("left", (_e, downCx) => downCx.stop_propagation())
      .on_key_down((e, keyCx) => {
        if (e.modifiers?.control || e.modifiers?.platform || e.modifiers?.alt) return;
        const key = String(e.key ?? "").toLowerCase();
        if (key === "escape") {
          keyCx.stop_propagation();
          closeMenu(keyCx);
          return;
        }
        if (selectable.length === 0) return;
        if (key === "down" || key === "arrowdown" || key === "j") {
          keyCx.stop_propagation();
          const pos = selectable.indexOf(activeIndex);
          setInteraction(
            view,
            { kind: "menu", id, index: selectable[(pos + 1) % selectable.length] },
            keyCx
          );
          return;
        }
        if (key === "up" || key === "arrowup" || key === "k") {
          keyCx.stop_propagation();
          const pos = selectable.indexOf(activeIndex);
          const nextIdx =
            pos <= 0 ? selectable[selectable.length - 1] : selectable[pos - 1];
          setInteraction(view, { kind: "menu", id, index: nextIdx }, keyCx);
          return;
        }
        if (key === "enter" || key === "return") {
          keyCx.stop_propagation();
          runSelected(activeIndex, keyCx);
        }
      })
      .children(
        resolvedItems.map((item, idx) => {
          if (item.type === "divider") {
            return div().my(3).h(1).w_full().bg(t.border);
          }
          if (item.type === "label") {
            return div()
              .px(10)
              .py(3)
              .text_size(10.5)
              .font_medium()
              .text_color(t.dim)
              .child(item.text);
          }
          const highlighted = !item.disabled && idx === activeIndex;
          return h_flex()
            .id(`${id}-${item.id}`)
            .h(28)
            .px(10)
            .gap(8)
            .items_center()
            .justify_between()
            .rounded(5)
            .text_size(12)
            .when(item.disabled, (el) => el.text_color(t.dim))
            .when(!item.disabled, (el) =>
              el
                .cursor_pointer()
                .text_color(item.danger ? t.blocked : t.foreground)
                .when(highlighted, (row) => row.bg(t.hover))
                .hover((s) => s.bg(t.hover))
                .on_hover((hovered, hoverCx) => {
                  if (hovered && isMenuOpen(view, id) && view.interaction.index !== idx) {
                    setInteraction(view, { kind: "menu", id, index: idx }, hoverCx);
                  }
                })
                .on_mouse_down("left", (_e, downCx) => downCx.stop_propagation())
                .on_click((_e, clickCx) => {
                  clickCx.stop_propagation();
                  runSelected(idx, clickCx);
                })
            )
            .when(Boolean(item.icon), (el) =>
              el.child(
                div()
                  .flex_none()
                  .w(10)
                  .text_size(9)
                  .text_color(item.disabled ? t.dim : item.icon.color)
                  .child(item.icon.symbol)
              )
            )
            .child(div().flex_1().min_w(0).truncate().child(item.label))
            .when(Boolean(item.checked), (el) =>
              el.child(div().flex_none().text_size(11).text_color(t.dim).child("✓"))
            );
        })
      );
  };

  let pop = Popover.new(id)
    .anchor(anchor)
    .open(isOpen)
    .on_open_change((open, openCx) => {
      if (open) {
        setInteraction(view, { kind: "menu", id, index: -1 }, openCx);
        view.menuFocus?.focus?.();
      } else if (isMenuOpen(view, id)) {
        setInteraction(view, null, openCx);
      }
    })
    .trigger(trigger);
  if (mouseButton !== "left") {
    pop = pop.mouse_button(mouseButton);
  }
  if (view.menuFocus) {
    pop = pop.track_focus(view.menuFocus);
  }
  if (isOpen) {
    pop = pop.content(buildContent());
  }

  if (!stopRowPropagation) return pop;
  return div()
    .flex_none()
    .on_mouse_down("left", (_e, downCx) => downCx.stop_propagation())
    .on_click((_e, clickCx) => clickCx.stop_propagation())
    .child(pop);
}
