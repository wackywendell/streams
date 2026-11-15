/**
 * Portable coordinator, request builder, and viewer dispatcher for the herdr-app Streams mod.
 * Kept free of gpui-kit / herdr imports so unit tests can exercise all state and RPC rules.
 */

export const PROTOCOL_VERSION = 1;
export const MAX_SELECTIONS = 64;

/**
 * Determine which role a View instance should render from its props.
 */
export function resolveRole(rawProps) {
  if (rawProps?.role === "popup") return "popup";
  if (rawProps?.role === "frame" || rawProps?.pane) return "frame";
  return "sidebar";
}

/**
 * Build a version-1 `ModRequest` envelope matching `src/integration.rs::ModRequest`.
 */
export function buildRequestEnvelope(scope, endpoint, body) {
  return {
    version: PROTOCOL_VERSION,
    scope: { catalog: scope.catalog, host: scope.host },
    endpoint,
    body,
  };
}

/**
 * Build the batched `ViewSelection` list for the active local space and visible terminal panes.
 */
export function buildViewSelections({
  endpoint,
  activeSpace,
  visibleTerminals = [],
  viewingChoice = { kind: "follow", last_agent: null },
}) {
  if (!endpoint) return [];
  const selections = [];
  if (activeSpace && activeSpace.machine === null) {
    selections.push({
      kind: "sidebar",
      space: { adapter: "hc", endpoint, key: activeSpace.id },
      viewing: viewingChoice,
    });
  }
  const seen = new Set();
  for (const terminalKey of visibleTerminals) {
    if (!terminalKey || terminalKey.includes("/") || seen.has(terminalKey)) continue;
    if (selections.length >= MAX_SELECTIONS) break;
    seen.add(terminalKey);
    selections.push({
      kind: "frame",
      terminal: { adapter: "hc", endpoint, key: terminalKey },
    });
  }
  return selections;
}

/**
 * Extract the combined save-notice token across all local terminals for a given notification key.
 */
export function observeNoticeToken(terminalList, notificationKey) {
  if (!notificationKey || !Array.isArray(terminalList)) return "";
  const pairs = [];
  for (const term of terminalList) {
    if (!term || term.machine !== null) continue;
    const token = term.labels?.[notificationKey];
    if (typeof token === "string" && token.length > 0) {
      pairs.push(`${term.id}:${token}`);
    }
  }
  pairs.sort();
  return pairs.join("|");
}

/**
 * Compute a deterministic context signature for automatic view refresh.
 */
export function computeObservationKey({
  activeSpace,
  visibleTerminals,
  viewingChoice,
  noticeToken,
}) {
  if (!activeSpace) return "none";
  if (activeSpace.machine !== null) return `remote:${activeSpace.id}`;
  return JSON.stringify({
    space: activeSpace.id,
    terminals: [...(visibleTerminals ?? [])].sort(),
    viewing: viewingChoice ?? { kind: "follow", last_agent: null },
    notice: noticeToken ?? "",
  });
}

/**
 * Execute `ViewerAction` items once from a fresh action reply.
 */
export function dispatchViewerActions(actions, { run, openUrl }) {
  if (!Array.isArray(actions)) return [];
  const dispatched = [];
  for (const action of actions) {
    if (!action || typeof action !== "object") continue;
    if (action.kind === "select_space" && action.space?.key) {
      run?.("space.select", { id: action.space.key });
      dispatched.push({ kind: "select_space", id: action.space.key });
    } else if (action.kind === "focus_terminal" && action.terminal?.key) {
      run?.("pane.focus", { terminal: action.terminal.key });
      dispatched.push({ kind: "focus_terminal", terminal: action.terminal.key });
    } else if (action.kind === "open_url" && typeof action.url === "string") {
      openUrl?.(action.url);
      dispatched.push({ kind: "open_url", url: action.url });
    }
  }
  return dispatched;
}

/**
 * Stateful coordinator that deduplicates `describe` and batched `read` calls,
 * discards stale async responses, and runs one-shot `open_work` / `open_reference` actions.
 */
export class StreamsCoordinator {
  constructor({ exec, notify, streamsBin = "streams" } = {}) {
    this.exec = exec;
    this.notify = notify;
    this.streamsBin = streamsBin;

    this.binding = null; // { scope, endpoint, notification_key }
    this.describePromise = null;
    this.describeError = null;

    this.readGeneration = 0;
    this.inFlightKey = null;
    this.lastCompletedKey = null;

    this.sidebar = null; // SidebarContext or null
    this.frames = new Map(); // terminalKey -> FrameContext
    this.lastError = null;
    this.viewingBySpace = new Map(); // spaceId -> ViewingChoice
    this.lastAgentBySpace = new Map(); // spaceId -> terminalKey
  }

  notifyViews() {
    this.notify?.();
  }

  viewingForSpace(spaceId, endpoint) {
    const pinned = this.viewingBySpace.get(spaceId);
    if (pinned && pinned.kind === "pinned") return pinned;
    const lastAgent = this.lastAgentBySpace.get(spaceId) ?? null;
    return {
      kind: "follow",
      last_agent:
        lastAgent && endpoint
          ? { adapter: "hc", endpoint, key: lastAgent }
          : null,
    };
  }

  pinSession(spaceId, sessionId) {
    if (!spaceId) return;
    if (!sessionId) {
      this.viewingBySpace.delete(spaceId);
    } else {
      this.viewingBySpace.set(spaceId, { kind: "pinned", session: sessionId });
    }
    this.lastCompletedKey = null;
    this.notifyViews();
  }

  recordFocusedTerminal(spaceId, terminalId) {
    if (!spaceId || !terminalId || terminalId.includes("/")) return false;
    if (this.lastAgentBySpace.get(spaceId) === terminalId) return false;
    this.lastAgentBySpace.set(spaceId, terminalId);
    return true;
  }

  async ensureDescribed() {
    if (this.binding) return this.binding;
    if (this.describePromise) return this.describePromise;
    this.describePromise = (async () => {
      try {
        const out = await this.exec(this.streamsBin, [
          "integration",
          "describe",
          "--frontend",
          "hc",
        ]);
        if (out.code !== 0) {
          throw new Error(out.stderr?.trim() || "streams integration describe failed");
        }
        const parsed = JSON.parse(out.stdout);
        if (
          parsed?.version !== PROTOCOL_VERSION ||
          !parsed?.scope?.catalog ||
          !parsed?.scope?.host ||
          typeof parsed?.endpoint !== "string"
        ) {
          throw new Error("invalid streams integration describe reply");
        }
        this.binding = {
          scope: parsed.scope,
          endpoint: parsed.endpoint,
          notification_key: parsed.notification_key ?? null,
        };
        this.describeError = null;
        return this.binding;
      } catch (err) {
        this.describeError = err instanceof Error ? err.message : String(err);
        return null;
      } finally {
        this.describePromise = null;
        this.notifyViews();
      }
    })();
    return this.describePromise;
  }

  /**
   * Observe current reactive desktop state and trigger a batched read only when the
   * observation key changed (or when `force` is true).
   */
  observe({
    activeSpace,
    visibleTerminals = [],
    terminalList = [],
    focusedTerminal = null,
    force = false,
  }) {
    if (activeSpace && activeSpace.machine === null && focusedTerminal) {
      this.recordFocusedTerminal(activeSpace.id, focusedTerminal);
    }
    if (activeSpace && activeSpace.machine !== null) {
      this.sidebar = { kind: "not_applicable", reason: "remote space" };
      return null;
    }
    if (!this.binding) {
      if (!this.describePromise && (!this.describeError || force)) {
        return this.ensureDescribed().then((binding) => {
          if (binding) {
            return this.observe({
              activeSpace,
              visibleTerminals,
              terminalList,
              focusedTerminal,
              force: true,
            });
          }
          return null;
        });
      }
      return null;
    }

    const viewingChoice = activeSpace
      ? this.viewingForSpace(activeSpace.id, this.binding.endpoint)
      : { kind: "follow", last_agent: null };
    const noticeToken = observeNoticeToken(terminalList, this.binding.notification_key);
    const key = computeObservationKey({
      activeSpace,
      visibleTerminals,
      viewingChoice,
      noticeToken,
    });

    if (!force && (key === this.lastCompletedKey || key === this.inFlightKey)) {
      return null;
    }
    return this.refresh({ activeSpace, visibleTerminals, viewingChoice, key });
  }

  async refresh({ activeSpace, visibleTerminals = [], viewingChoice, key }) {
    const binding = await this.ensureDescribed();
    if (!binding) return null;

    const selections = buildViewSelections({
      endpoint: binding.endpoint,
      activeSpace,
      visibleTerminals,
      viewingChoice:
        viewingChoice ??
        (activeSpace
          ? this.viewingForSpace(activeSpace.id, binding.endpoint)
          : { kind: "follow", last_agent: null }),
    });
    if (selections.length === 0) {
      this.sidebar = null;
      this.lastCompletedKey = key ?? null;
      this.notifyViews();
      return null;
    }

    const gen = ++this.readGeneration;
    this.inFlightKey = key ?? null;
    const envelope = buildRequestEnvelope(binding.scope, binding.endpoint, {
      kind: "read",
      selections,
    });

    try {
      const out = await this.exec(this.streamsBin, [
        "integration",
        "request",
        "--request-json",
        JSON.stringify(envelope),
      ]);
      if (gen !== this.readGeneration) return null; // Stale response discarded
      if (out.code !== 0) {
        this.lastError = out.stderr?.trim() || "streams integration request failed";
        this.lastCompletedKey = key ?? null;
        return null;
      }
      const reply = JSON.parse(out.stdout);
      if (reply?.result?.error) {
        this.lastError = reply.result.error.message || "integration read error";
        this.lastCompletedKey = key ?? null;
        return reply;
      }
      const readRes = reply?.result?.ok;
      if (readRes?.kind === "read" && Array.isArray(readRes.values)) {
        if (typeof readRes.notification_key === "string") {
          binding.notification_key = readRes.notification_key;
        }
        selections.forEach((sel, idx) => {
          const item = readRes.values[idx];
          if (sel.kind === "sidebar") {
            if (item?.ok?.kind === "sidebar") {
              this.sidebar = item.ok.value;
            } else if (item?.error) {
              this.sidebar = { kind: "unavailable", reason: item.error.message };
            }
          } else if (sel.kind === "frame") {
            if (item?.ok?.kind === "frame") {
              this.frames.set(sel.terminal.key, item.ok.context);
            } else if (item?.error) {
              this.frames.set(sel.terminal.key, {
                kind: "unavailable",
                reason: item.error.message,
              });
            }
          }
        });
        this.lastError = null;
        this.lastCompletedKey = key ?? null;
      }
      return reply;
    } catch (err) {
      if (gen === this.readGeneration) {
        this.lastError = err instanceof Error ? err.message : String(err);
        this.lastCompletedKey = key ?? null;
      }
      return null;
    } finally {
      if (gen === this.readGeneration) {
        this.inFlightKey = null;
        this.notifyViews();
      }
    }
  }

  /**
   * Execute a one-shot `open_work` or `open_reference` action and dispatch any returned
   * `ViewerAction` items once on the fresh reply.
   */
  async act(action, { run, openUrl } = {}) {
    const binding = await this.ensureDescribed();
    if (!binding) return null;
    const envelope = buildRequestEnvelope(binding.scope, binding.endpoint, {
      kind: "act",
      action,
    });
    const out = await this.exec(this.streamsBin, [
      "integration",
      "request",
      "--request-json",
      JSON.stringify(envelope),
    ]);
    if (out.code !== 0) {
      this.lastError = out.stderr?.trim() || "streams integration action failed";
      this.notifyViews();
      return null;
    }
    const reply = JSON.parse(out.stdout);
    const ok = reply?.result?.ok;
    let viewerActions = [];
    if (ok?.kind === "work_opened") {
      viewerActions = ok.value?.viewer_actions ?? [];
    } else if (ok?.kind === "reference_opened") {
      const res = ok.value?.result;
      if (res?.kind === "needs_viewer") {
        viewerActions = res.value ?? [];
      }
    }
    if (Array.isArray(reply?.viewer_projection) && reply.viewer_projection.length > 0) {
      viewerActions = [...viewerActions, ...reply.viewer_projection];
    }
    dispatchViewerActions(viewerActions, { run, openUrl });
    this.lastCompletedKey = null; // Force next observation to refresh views
    this.notifyViews();
    return reply;
  }

  openReference(reference, { activeSpaceId = null, run, openUrl } = {}) {
    const endpoint = this.binding?.endpoint;
    const isLink = reference?.kind === "link";
    const at =
      !isLink && activeSpaceId && endpoint
        ? { adapter: "hc", endpoint, key: activeSpaceId }
        : null;
    return this.act(
      {
        kind: "open_reference",
        reference,
        context: { kind: "explicit" },
        at,
      },
      { run, openUrl }
    );
  }

  openWork(
    { stream, sessions = { kind: "all_eligible" }, cwd = null },
    { run, openUrl } = {}
  ) {
    return this.act(
      {
        kind: "open_work",
        request: {
          stream,
          sessions,
          cwd,
        },
      },
      { run, openUrl }
    );
  }
}
