// Herdr runtime integration: coordinator singleton, view subscription waiters,
// desktop snapshot collection, layout helpers, and observation scheduling.

import {
  spaces,
  panes,
  terminals,
  agents,
  machines,
  focused,
  props,
  run,
  home,
  layout,
  window as herdrWindow,
  checking,
  exec,
  shared,
  setShared,
  notify as herdrNotify,
} from "herdr";
import {
  resolveTabRenameTarget,
  runningSessionTerminals,
  StreamsCoordinator,
} from "./state.js";

let lastWindowActive = false;
let lastScheduledObsKey = null;
let lastTabRenameAttempt = null;
let mountedSidebars = 0;

const sidebarWaiters = new Set();
const frameWaiters = new Set();

export function findNode(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = findNode(child, id);
    if (found) return found;
  }
  return null;
}

export function paneFrame(node) {
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

export function isStreamsSidebarVisible(node) {
  if (!node || node.hidden) return false;
  if (node.type === "block") {
    return node.content?.mod === "streams" && node.id !== "streams-frame";
  }
  if (node.type === "stack") {
    const active = (node.children ?? []).find((c) => c.id === node.active);
    return active ? isStreamsSidebarVisible(active) : false;
  }
  for (const child of node.children ?? []) {
    if (isStreamsSidebarVisible(child)) return true;
  }
  return false;
}

export function registerSidebarMount() {
  mountedSidebars += 1;
}

export function unregisterSidebarMount() {
  mountedSidebars = Math.max(0, mountedSidebars - 1);
}

export function isSidebarDrivingObserve() {
  if (mountedSidebars === 0) return false;
  try {
    return isStreamsSidebarVisible(layout());
  } catch {
    return false;
  }
}

function wakeWaiters(set) {
  if (set.size === 0) return;
  const waiters = [...set];
  set.clear();
  for (const resolve of waiters) {
    resolve();
  }
}

export function notifyStreamsViews(scope = "sidebar") {
  if (checking()) return;
  if (scope === "frame") {
    wakeWaiters(frameWaiters);
    return;
  }
  wakeWaiters(sidebarWaiters);
  if (scope === "all") {
    wakeWaiters(frameWaiters);
  }
}

export function waiterSetForRole(role) {
  return role === "sidebar" ? sidebarWaiters : frameWaiters;
}

function readSharedString(key) {
  if (checking()) return null;
  try {
    const val = shared(key);
    return typeof val === "string" ? val : null;
  } catch {
    return null;
  }
}

export const coordinator = new StreamsCoordinator({
  exec: (cmd, args, opts = {}) =>
    exec(cmd, args, {
      timeout: 30000,
      ...(opts?.on ? { on: opts.on } : {}),
    }),
  publishShared: (key, value) => setShared(key, value),
  notifyToast: (n) => herdrNotify(n),
  initialPreferredView: readSharedString("streams.preferredView") ?? "session",
  initialHostScope: readSharedString("streams.hostScope") ?? "local",
  onPreferenceChange: (key, value) => {
    if (checking()) return;
    try {
      setShared(`streams.${key}`, value);
    } catch {
      // Non-fatal in check/test mode
    }
  },
  homeDir: (() => {
    try {
      return home();
    } catch {
      return null;
    }
  })(),
  notify: (scope) => {
    notifyStreamsViews(scope);
  },
});

export function syncConfiguredBin() {
  const configuredBin = props()?.streams_bin;
  if (configuredBin && coordinator.streamsBin !== configuredBin) {
    coordinator.streamsBin = configuredBin;
  }
}

export function folder(cwd, machine = null) {
  if (!cwd) return "";
  const h = coordinator.homeDirFor(machine);
  if (!h) return cwd;
  return cwd === h ? "~" : cwd.startsWith(h + "/") ? "~" + cwd.slice(h.length) : cwd;
}

export function collectSnapshot() {
  const allSpaces = spaces();
  const activeSpace = allSpaces.find((s) => s.active) ?? null;
  const activeTab = activeSpace?.tabs?.find((t) => t.active) ?? null;
  const allPanes = panes();
  const visibleTerminals = activeTab
    ? allPanes
        .filter((p) => p.tab === activeTab.id && p.terminal)
        .map((p) => p.terminal)
    : [];
  const liveTerminals = allPanes
    .filter((p) => p.terminal && p.program?.agent)
    .map((p) => p.terminal);
  const focus = focused();
  const focusedPane = focus?.pane ? allPanes.find((p) => p.pane === focus.pane) : null;
  const focusedTerminal =
    focusedPane?.program?.agent && focusedPane?.terminal
      ? focusedPane.terminal
      : focus?.agent ?? null;
  const allAgents = agents();
  const machineList = (() => {
    try {
      return typeof machines === "function" ? machines() : [];
    } catch {
      return [];
    }
  })();
  return {
    allSpaces,
    activeSpace,
    activeTab,
    activeTabId: activeTab?.id ?? null,
    visibleTerminals,
    liveTerminals,
    panes: allPanes,
    agents: allAgents,
    terminalList: terminals(),
    machineList,
    focusedTerminal,
  };
}

export function resetRuntimeStateForTest() {
  lastWindowActive = false;
  lastScheduledObsKey = null;
  lastTabRenameAttempt = null;
  mountedSidebars = 0;
  sidebarWaiters.clear();
  frameWaiters.clear();
}

export function syncActiveTabName(snap) {
  const runningByTerm = runningSessionTerminals({
    ...snap,
    staleSessionKeys: coordinator.staleSessionKeys,
  });
  const target = resolveTabRenameTarget({
    activeTab: snap.activeTab,
    visibleTerminals: snap.visibleTerminals,
    runningByTerm,
    sessions: coordinator.sessions,
    tabNaming: coordinator.tabNaming,
  });
  if (!target) {
    lastTabRenameAttempt = null;
    return;
  }
  const key = `${target.tabId}\0${target.currentName}\0${target.desiredName}`;
  if (lastTabRenameAttempt === key) return;
  lastTabRenameAttempt = key;
  run("tab.rename", { id: target.tabId, name: target.desiredName });
}

export function scheduleObserve(view, cx, snap, opts = {}) {
  if (checking()) return;
  const force = typeof opts === "boolean" ? opts : Boolean(opts?.force);
  const live = typeof opts === "object" ? opts?.live ?? null : null;
  syncConfiguredBin();
  const isWinActive = Boolean(herdrWindow()?.active);
  const windowActivated = isWinActive && !lastWindowActive;
  lastWindowActive = isWinActive;
  const obsKey = coordinator.peekObservationKey(snap, live);
  const stale =
    coordinator.staleFallbackMs > 0 &&
    coordinator.lastCompletedAt > 0 &&
    Date.now() - coordinator.lastCompletedAt >= coordinator.staleFallbackMs;
  if (
    !force &&
    !(windowActivated && stale) &&
    obsKey === lastScheduledObsKey &&
    (coordinator.lastCompletedAt > 0 ||
      coordinator.readError ||
      coordinator.lastFailedKey !== null ||
      coordinator.inFlightKey !== null)
  ) {
    return;
  }
  lastScheduledObsKey = obsKey;
  const runObserve = async () => {
    await coordinator.observe({
      ...snap,
      force,
      contextSynced: view._role === "sidebar",
    });
    syncActiveTabName(collectSnapshot());
  };
  if (cx?.spawn) {
    view._observeTask = cx.spawn(runObserve);
  } else {
    void runObserve();
  }
}
