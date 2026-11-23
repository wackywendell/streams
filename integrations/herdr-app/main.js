// Streams mod for herdr-app (roles: sidebar, frame, popup, viewer).
// Thin View lifecycle shell delegating rendering to role/view modules.

import { View } from "gpui-kit";
import { ScrollHandle, InputState } from "gpui-base";
import { props, checking } from "herdr";
import { STALE_FALLBACK_MS, resolveRole } from "./state.js";
import {
  registerSidebarMount,
  unregisterSidebarMount,
  isSidebarDrivingObserve,
  waiterSetForRole,
  syncConfiguredBin,
  collectSnapshot,
  syncActiveTabName,
  scheduleObserve,
} from "./runtime.js";
import { commitMove, commitRename } from "./popovers.js";
import { renderFrame } from "./frame.js";
import { syncViewerState, renderPopup, renderViewer } from "./viewer.js";
import { renderSidebar } from "./sidebar.js";

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
    this.hoveredRefId = null;
    this.interaction = null;
    this._lastContextKey = null;
    this.menuFocus = null;
    this.sessionSort = "recent";
    this.streamSort = "recent";
    this.streamQuery = "";
    this.scrollSession = ScrollHandle.new();
    this.scrollStream = ScrollHandle.new();
    this.scrollAll = ScrollHandle.new();
    this._subTask = null;
    this._subResolve = null;
    this._subWaiters = null;
    this._countedSidebar = false;
    this._observeTask = null;
    this._timerTask = null;
    this._editorState = null;
    this._viewerKey = null;
    this.searchInput = null;
    this.renameInput = null;
    this.moveSearchInput = null;

    const rawProps = props() ?? initialProps ?? {};
    const role = resolveRole(rawProps);
    this._role = role;

    if (role === "sidebar") {
      this.menuFocus = cx.focus_handle();
      this.searchInput = InputState.new({ placeholder: "Filter streams…" });
      this.searchInput.on("change", (_e, changeCx) => {
        this.streamQuery = this.searchInput.value();
        changeCx.notify();
      });
      this.renameInput = InputState.new({ placeholder: "Title" });
      this.renameInput.on("change", (_e, changeCx) => {
        changeCx.notify();
      });
      this.renameInput.on("submit", (_e, submitCx) => {
        commitRename(this, submitCx);
      });
      this.moveSearchInput = InputState.new({ placeholder: "Filter streams…" });
      this.moveSearchInput.on("change", (_e, changeCx) => {
        changeCx.notify();
      });
      this.moveSearchInput.on("submit", (_e, submitCx) => {
        commitMove(this, submitCx);
      });
    }

    if (role === "viewer") {
      syncViewerState(this, rawProps);
      return;
    }
    if (role !== "sidebar" && role !== "frame") {
      return;
    }
    if (checking()) {
      return;
    }

    syncConfiguredBin();
    if (role === "sidebar") {
      registerSidebarMount();
      this._countedSidebar = true;
    }

    const waiterSet = waiterSetForRole(role);
    if (cx?.spawn) {
      this._subTask = cx.spawn(async (asyncCx) => {
        for (;;) {
          await new Promise((resolve) => {
            this._subResolve = resolve;
            this._subWaiters = waiterSet;
            waiterSet.add(resolve);
          });
          this._subResolve = null;
          if (this._role === "sidebar") {
            syncActiveTabName(collectSnapshot());
          }
          asyncCx.notify();
        }
      });
    }

    if (role === "sidebar" || !isSidebarDrivingObserve()) {
      scheduleObserve(this, cx, collectSnapshot());
    }
    if (role === "sidebar" && cx?.timer?.every) {
      this._timerTask = cx.timer.every(STALE_FALLBACK_MS, (timerCx) => {
        scheduleObserve(this, timerCx, collectSnapshot(), { force: true });
      });
    }
  }

  async close() {
    if (this._countedSidebar) {
      this._countedSidebar = false;
      unregisterSidebarMount();
    }
    if (this._subResolve) {
      this._subWaiters?.delete(this._subResolve);
      this._subResolve = null;
    }
    this._subWaiters = null;
    this._subTask?.cancel?.();
    this._subTask = null;
    this._observeTask?.cancel?.();
    this._observeTask = null;
    this._timerTask?.cancel?.();
    this._timerTask = null;
    this.searchInput?.release?.();
    this.searchInput = null;
    this.renameInput?.release?.();
    this.renameInput = null;
    this.moveSearchInput?.release?.();
    this.moveSearchInput = null;
    this._editorState?.release?.();
    this._editorState = null;
    this._viewerKey = null;
  }

  render(cx) {
    const role = resolveRole(props());
    if (role === "viewer") return renderViewer(this);
    if (role === "popup") return renderPopup();
    syncConfiguredBin();
    if (role === "frame") return renderFrame(this, cx);
    return renderSidebar(this, cx);
  }
}
