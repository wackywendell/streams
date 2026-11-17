/**
 * Portable coordinator, request builder, and viewer dispatcher for the herdr-app Streams mod.
 * Kept free of gpui-kit / herdr imports so unit tests can exercise all state and RPC rules.
 */

export const PROTOCOL_VERSION = 1;
export const MAX_SELECTIONS = 64;
export const STALE_FALLBACK_MS = 5 * 60 * 1000;

/**
 * Determine whether an enrolled session is actually live in its terminal/pane,
 * surfacing "offline" when an active/waiting record no longer has a live agent.
 */
export function effectiveSessionStatus(session, { pane = null, terminal = null, agent = null } = {}) {
  if (!session) return "unknown";
  const status = session.status;
  if (status === "done" || status === "parked") return status;
  if (terminal?.state && terminal.state !== "live") return "offline";
  if (pane?.program && !pane.program.agent && !agent) return "offline";
  return status;
}

/**
 * Determine which role a View instance should render from its props.
 */
export function resolveRole(rawProps) {
  if (
    rawProps?.role === "popup" ||
    rawProps?.role === "frame" ||
    rawProps?.role === "sidebar" ||
    rawProps?.role === "viewer"
  ) {
    return rawProps.role;
  }
  if (rawProps?.kind === "reference-viewer") {
    return "viewer";
  }
  if (rawProps?.kind === "session-popup" || rawProps?.kind === "stream-menu") {
    return "popup";
  }
  if (rawProps?.pane || rawProps?.terminal) return "frame";
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
 * Build the batched `ViewSelection` list for visible terminals in the active tab
 * and optionally the tab's known enrolled Stream.
 */
export function buildViewSelections({
  endpoint,
  visibleTerminals = [],
  knownStreamId = null,
}) {
  if (!endpoint) return [];
  const selections = [];
  if (typeof knownStreamId === "string" && knownStreamId.length > 0) {
    selections.push({
      kind: "saved_stream",
      stream: knownStreamId,
    });
  } else {
    selections.push({ kind: "host_streams" });
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
 * `herdr-app`'s `terminals()` uses `hc ls --json` shape (`address`, no `machine` or `id`);
 * accept both `address` and `id` while excluding remote `<machine>/<id>` entries.
 */
export function observeNoticeToken(terminalList, notificationKey) {
  if (!notificationKey || !Array.isArray(terminalList)) return "";
  const pairs = [];
  for (const term of terminalList) {
    if (!term || (term.machine !== undefined && term.machine !== null)) continue;
    const addr = term.address ?? term.id;
    if (typeof addr !== "string" || addr.length === 0 || addr.includes("/")) continue;
    const token = term.labels?.[notificationKey];
    if (typeof token === "string" && token.length > 0) {
      pairs.push(`${addr}:${token}`);
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
  activeTabId = null,
  visibleTerminals,
  knownStreamId = null,
  noticeToken,
}) {
  if (!activeSpace) return "none";
  if (activeSpace.machine !== null) return `remote:${activeSpace.id}`;
  return JSON.stringify({
    space: activeSpace.id,
    tab: activeTabId,
    terminals: [...(visibleTerminals ?? [])].sort(),
    stream: knownStreamId ?? null,
    notice: noticeToken ?? "",
  });
}

/**
 * Resolve the primary enrolled session for the active tab (preferring the focused terminal).
 */
export function activeTabSession({
  visibleTerminals = [],
  focusedTerminal = null,
  frames,
}) {
  if (!frames) return null;
  if (focusedTerminal && visibleTerminals.includes(focusedTerminal)) {
    const focusedFrame = frames.get(focusedTerminal);
    if (focusedFrame?.kind === "enrolled" && focusedFrame.session) {
      return { terminal: focusedTerminal, session: focusedFrame.session };
    }
  }
  for (const terminalKey of visibleTerminals) {
    const frame = frames.get(terminalKey);
    if (frame?.kind === "enrolled" && frame.session) {
      return { terminal: terminalKey, session: frame.session };
    }
  }
  return null;
}

/**
 * Determine whether the active `herdr-app` tab should be renamed to match its enrolled Session title.
 */
export function resolveTabRenameTarget({
  activeTab,
  visibleTerminals = [],
  frames,
  tabNaming = true,
}) {
  if (!tabNaming || !activeTab?.id) return null;
  for (const termKey of visibleTerminals) {
    const frame = frames?.get?.(termKey);
    if (frame?.kind === "enrolled" && frame.session?.title) {
      const desired = frame.session.title.trim();
      if (desired && activeTab.name !== desired) {
        return {
          tabId: activeTab.id,
          currentName: activeTab.name ?? "",
          desiredName: desired,
        };
      }
      return null;
    }
  }
  return null;
}

function formatHostPath(url) {
  try {
    const parsed = new URL(url);
    const path = parsed.pathname === "/" ? "" : parsed.pathname;
    return `${parsed.host}${path}${parsed.search ?? ""}`;
  } catch {
    return url;
  }
}

/**
 * Combine Stream documents, links/PRs/issues, and artifacts into one unified References list
 * directly from structured fields without parsing or rewriting titles.
 */
export function buildReferenceItems(stream, { viewingSessionId = null } = {}) {
  if (!stream) return [];
  const items = [];

  for (const [idx, link] of (stream.links ?? []).entries()) {
    const url = link.key?.url ?? "";
    const hostPath = url ? formatHostPath(url) : "";
    const name = typeof link.name === "string" ? link.name.trim() : "";
    const title = name || hostPath || url;
    const location = hostPath && hostPath !== title ? hostPath : null;
    items.push({
      id: `link-${idx}-${url}`,
      kind: link.key?.kind ?? "link",
      title,
      location,
      description: typeof link.description === "string" ? link.description.trim() : "",
      monoTitle: !name,
      external: true,
      reference: {
        kind: "link",
        stream: stream.id,
        link: link.key,
      },
      disabled: false,
    });
  }

  for (const [idx, doc] of (stream.documents ?? []).entries()) {
    const path = doc.key?.path ?? "";
    const name = typeof doc.name === "string" ? doc.name.trim() : "";
    const title = name || path;
    const location = path && path !== title ? path : null;
    items.push({
      id: `doc-${idx}-${doc.key?.repository ?? ""}-${path}`,
      kind: "document",
      title,
      location,
      description: typeof doc.description === "string" ? doc.description.trim() : "",
      monoTitle: !name,
      external: false,
      reference: viewingSessionId
        ? {
            kind: "document",
            stream: stream.id,
            viewing_session: viewingSessionId,
            document: doc.key,
          }
        : null,
      disabled: !viewingSessionId,
    });
  }

  for (const group of stream.artifacts ?? []) {
    for (const entry of group.entries ?? []) {
      items.push({
        id: `art-${entry.key?.session ?? ""}-${entry.key?.name ?? ""}`,
        kind: "artifact",
        title: entry.key?.name ?? "",
        location: null,
        description: typeof entry.description === "string" ? entry.description.trim() : "",
        monoTitle: true,
        external: false,
        reference: {
          kind: "artifact",
          key: entry.key,
        },
        disabled: false,
      });
    }
  }

  return items;
}

/**
 * Execute `ViewerAction` items once from a fresh action reply.
 */
export function dispatchViewerActions(actions, { run, openUrl, focusTerminal } = {}) {
  if (!Array.isArray(actions)) return [];
  const dispatched = [];
  for (const action of actions) {
    if (!action || typeof action !== "object") continue;
    if (action.kind === "select_space" && action.space?.key) {
      run?.("space.select", { id: action.space.key });
      dispatched.push({ kind: "select_space", id: action.space.key });
    } else if (action.kind === "focus_terminal" && action.terminal?.key) {
      if (focusTerminal) {
        focusTerminal(action.terminal.key);
      } else {
        run?.("pane.focus", { terminal: action.terminal.key });
      }
      dispatched.push({ kind: "focus_terminal", terminal: action.terminal.key });
    } else if (action.kind === "open_url" && typeof action.url === "string") {
      openUrl?.(action.url);
      dispatched.push({ kind: "open_url", url: action.url });
    }
  }
  return dispatched;
}

export function dispatchReferencePresentation(
  result,
  {
    activeSpaceId = null,
    activeTabId = null,
    targetPaneId = null,
    reference = null,
    tabNaming = true,
    streamsBin = "streams",
    run,
    openOverlay,
    onPendingViewer,
  } = {}
) {
  if (!result || typeof result !== "object") return { ok: true };
  if (result.kind === "launch_command") {
    const argv = [streamsBin, ...(result.args ?? [])];
    if (result.placement === "tab") {
      run?.("tab.new", {
        ...(activeSpaceId ? { space: activeSpaceId } : {}),
        ...(tabNaming && result.title ? { name: result.title } : {}),
        argv,
      });
      return { ok: true };
    }
    if (result.placement === "pane-right" || result.placement === "pane-down") {
      if (!targetPaneId) {
        return { ok: false, error: "No active pane to split for reference viewer." };
      }
      const direction = result.placement === "pane-down" ? "down" : "right";
      run?.("pane.split", { direction, pane: targetPaneId, argv });
      return { ok: true };
    }
    return { ok: false, error: `Unsupported terminal placement: ${result.placement}` };
  }
  if (result.kind === "show_content") {
    const props = {
      kind: "reference-viewer",
      title: result.title ?? result.filename ?? "",
      filename: result.filename ?? "",
      content: result.content ?? "",
      truncated: Boolean(result.truncated),
      ...(reference ? { reference } : {}),
      ...(activeSpaceId ? { spaceId: activeSpaceId } : {}),
      ...(activeTabId ? { tabId: activeTabId } : {}),
      ...(targetPaneId ? { targetPaneId } : {}),
    };
    if (result.placement === "tab") {
      onPendingViewer?.({
        tabId: null,
        spaceId: activeSpaceId,
        title: props.title,
        filename: props.filename,
        truncated: props.truncated,
        reference,
      });
      run?.("tab.new", {
        ...(activeSpaceId ? { space: activeSpaceId } : {}),
        ...(tabNaming && result.title ? { name: result.title } : {}),
        mod: "streams",
        props,
      });
      return { ok: true };
    }
    if (result.placement === "pane-right" || result.placement === "pane-down") {
      if (!targetPaneId) {
        return { ok: false, error: "No active pane to split for reference viewer." };
      }
      onPendingViewer?.({
        tabId: activeTabId,
        spaceId: activeSpaceId,
        title: props.title,
        filename: props.filename,
        truncated: props.truncated,
        reference,
      });
      const direction = result.placement === "pane-down" ? "down" : "right";
      run?.("pane.split", { direction, pane: targetPaneId, mod: "streams", props });
      return { ok: true };
    }
    if (result.placement === "floating") {
      openOverlay?.({
        kind: "window",
        key: "streams-viewer",
        width: 0.65,
        height: 0.75,
        mod: "streams",
        props,
      });
      return { ok: true };
    }
    return { ok: false, error: `Unsupported native placement: ${result.placement}` };
  }
  return { ok: true };
}

/**
 * Stateful coordinator that deduplicates `describe` and batched `read` calls,
 * discards stale async responses, and runs one-shot `open_work` / `open_reference` actions.
 */
export class StreamsCoordinator {
  constructor({ exec, notify, streamsBin = "streams", staleFallbackMs = STALE_FALLBACK_MS } = {}) {
    this.exec = exec;
    this.notify = notify;
    this.streamsBin = streamsBin;
    this.staleFallbackMs = staleFallbackMs;

    this.binding = null; // { scope, endpoint, notification_key }
    this.describePromise = null;
    this.describeError = null;
    this.tabNaming = true;
    this.herdrConfig = {
      tab_naming: true,
      viewer_mode: "editor",
      viewer_placement: "pane-right",
    };

    this.readGeneration = 0;
    this.inFlightKey = null;
    this.inFlightPromise = null;
    this.lastCompletedKey = null;
    this.lastFailedKey = null;
    this.lastCompletedAt = 0;

    this.streams = new Map(); // streamId -> StreamView
    this.hostStreams = []; // StreamSummary[]
    this.inspectedStreamId = null;
    this.frames = new Map(); // terminalKey -> FrameContext
    this.streamIdByTerminal = new Map(); // terminalKey -> streamId
    this.pendingViewers = [];
    this.viewerMetaByPane = new Map();
    this.lastError = null;
    this._retriedScope = false;
  }

  async inspectStream(streamId) {
    if (!streamId) {
      this.inspectedStreamId = null;
      this.notifyViews();
      return null;
    }
    this.inspectedStreamId = streamId;
    const binding = await this.ensureDescribed();
    if (!binding) return null;
    const envelope = buildRequestEnvelope(binding.scope, binding.endpoint, {
      kind: "read",
      selections: [{ kind: "saved_stream", stream: streamId }],
    });
    const out = await this.exec(this.streamsBin, [
      "integration",
      "request",
      "--request-json",
      JSON.stringify(envelope),
    ]);
    if (out.code !== 0) {
      this.lastError = out.stderr?.trim() || "streams integration request failed";
      this.notifyViews();
      return null;
    }
    const reply = JSON.parse(out.stdout);
    const item = reply?.result?.ok?.values?.[0];
    if (item?.ok?.kind === "stream") {
      this.streams.set(item.ok.value.id, item.ok.value);
      this.lastError = null;
    } else if (item?.error) {
      this.lastError = item.error.message || "saved Stream view is unavailable";
    }
    this.notifyViews();
    return reply;
  }

  recordPendingViewer(entry) {
    if (!entry) return;
    this.pendingViewers.push(entry);
    if (this.pendingViewers.length > 16) {
      this.pendingViewers.shift();
    }
  }

  resolveViewerPaneMeta(paneId, tabId, allPanes) {
    if (Array.isArray(allPanes)) {
      const livePanes = new Set(allPanes.map((p) => p.pane));
      for (const knownPane of this.viewerMetaByPane.keys()) {
        if (!livePanes.has(knownPane)) this.viewerMetaByPane.delete(knownPane);
      }
    }
    if (this.viewerMetaByPane.has(paneId)) {
      return this.viewerMetaByPane.get(paneId);
    }
    let idx = this.pendingViewers.findIndex((entry) => entry.tabId === tabId);
    if (idx < 0) {
      idx = this.pendingViewers.findIndex((entry) => entry.tabId === null);
    }
    if (idx < 0) return null;
    const [matched] = this.pendingViewers.splice(idx, 1);
    this.viewerMetaByPane.set(paneId, matched);
    return matched;
  }

  streamTitle(streamId) {
    if (!streamId) return "";
    const cached = this.streams.get(streamId);
    if (cached?.title) return cached.title;
    return streamId;
  }

  knownStreamForTab({ visibleTerminals = [], focusedTerminal = null } = {}) {
    if (focusedTerminal && visibleTerminals.includes(focusedTerminal)) {
      const focusedStream = this.streamIdByTerminal.get(focusedTerminal);
      if (focusedStream) return focusedStream;
    }
    for (const termKey of visibleTerminals) {
      const streamId = this.streamIdByTerminal.get(termKey);
      if (streamId) return streamId;
    }
    return null;
  }

  notifyViews() {
    this.notify?.();
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
    activeTabId = null,
    visibleTerminals = [],
    terminalList = [],
    focusedTerminal = null,
    force = false,
    now = Date.now(),
  }) {
    if (activeSpace && activeSpace.machine !== null) {
      return null;
    }
    if (!this.binding) {
      return this.ensureDescribed().then((binding) => {
        if (binding) {
          return this.observe({
            activeSpace,
            activeTabId,
            visibleTerminals,
            terminalList,
            focusedTerminal,
            force,
            now,
          });
        }
        return null;
      });
    }

    const knownStreamId = this.knownStreamForTab({ visibleTerminals, focusedTerminal });
    const noticeToken = observeNoticeToken(terminalList, this.binding.notification_key);
    const key = computeObservationKey({
      activeSpace,
      activeTabId,
      visibleTerminals,
      knownStreamId,
      noticeToken,
    });

    const stale =
      this.staleFallbackMs > 0 &&
      this.lastCompletedAt > 0 &&
      now - this.lastCompletedAt >= this.staleFallbackMs;

    if (
      !force &&
      !stale &&
      (key === this.lastCompletedKey ||
        key === this.lastFailedKey ||
        (key === this.inFlightKey && this.inFlightPromise))
    ) {
      return this.inFlightPromise ?? null;
    }
    return this.refresh({
      activeSpace,
      activeTabId,
      visibleTerminals,
      focusedTerminal,
      knownStreamId,
      key,
      now,
    });
  }

  async refresh({
    activeSpace,
    activeTabId = null,
    visibleTerminals = [],
    focusedTerminal = null,
    knownStreamId = null,
    key,
    now = Date.now(),
  }) {
    const binding = await this.ensureDescribed();
    if (!binding) return null;

    const selections = buildViewSelections({
      endpoint: binding.endpoint,
      visibleTerminals,
      knownStreamId,
    });
    if (selections.length === 0) {
      this.lastCompletedKey = key ?? null;
      this.lastFailedKey = null;
      this.lastCompletedAt = now;
      this.notifyViews();
      return null;
    }

    const gen = ++this.readGeneration;
    this.inFlightKey = key ?? null;
    const envelope = buildRequestEnvelope(binding.scope, binding.endpoint, {
      kind: "read",
      selections,
    });

    const promise = (async () => {
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
          this.lastFailedKey = key ?? null;
          return null;
        }
        const reply = JSON.parse(out.stdout);
        if (reply?.result?.error) {
          const msg = reply.result.error.message || "integration read error";
          if (
            msg === "configured catalog and host do not match the requested scope" &&
            !this._retriedScope
          ) {
            this._retriedScope = true;
            this.binding = null;
            this.frames.clear();
            this.streams.clear();
            this.streamIdByTerminal.clear();
            this.lastCompletedKey = null;
            this.lastFailedKey = null;
            this.lastError = msg;
            return reply;
          }
          this.lastError = msg;
          this.lastFailedKey = key ?? null;
          return reply;
        }
        this._retriedScope = false;
        const readRes = reply?.result?.ok;
        if (readRes?.kind === "read" && Array.isArray(readRes.values)) {
          if (typeof readRes.notification_key === "string") {
            binding.notification_key = readRes.notification_key;
          }
          if (readRes.herdr_config && typeof readRes.herdr_config === "object") {
            this.herdrConfig = readRes.herdr_config;
            if (typeof readRes.herdr_config.tab_naming === "boolean") {
              this.tabNaming = readRes.herdr_config.tab_naming;
            }
          }
          const fetchedStreams = new Set();
          selections.forEach((sel, idx) => {
            const item = readRes.values[idx];
            if (sel.kind === "saved_stream") {
              if (item?.ok?.kind === "stream") {
                this.streams.set(item.ok.value.id, item.ok.value);
                fetchedStreams.add(item.ok.value.id);
              }
            } else if (sel.kind === "host_streams") {
              if (item?.ok?.kind === "host_streams") {
                this.hostStreams = item.ok.value ?? [];
              }
            } else if (sel.kind === "frame") {
              if (item?.ok?.kind === "frame") {
                this.frames.set(sel.terminal.key, item.ok.context);
                if (item.ok.context?.kind === "enrolled" && item.ok.context.session?.stream) {
                  this.streamIdByTerminal.set(
                    sel.terminal.key,
                    item.ok.context.session.stream
                  );
                } else {
                  this.streamIdByTerminal.delete(sel.terminal.key);
                }
              } else if (item?.error) {
                this.frames.set(sel.terminal.key, {
                  kind: "unavailable",
                  reason: item.error.message,
                });
              }
            }
          });

          const tabSession = activeTabSession({
            visibleTerminals,
            focusedTerminal,
            frames: this.frames,
          });
          if (tabSession?.session?.stream && !fetchedStreams.has(tabSession.session.stream)) {
            if (key !== undefined) {
              this.inFlightKey = JSON.stringify({
                ...JSON.parse(key),
                stream: tabSession.session.stream,
              });
            }
            const streamEnvelope = buildRequestEnvelope(binding.scope, binding.endpoint, {
              kind: "read",
              selections: [{ kind: "saved_stream", stream: tabSession.session.stream }],
            });
            const streamOut = await this.exec(this.streamsBin, [
              "integration",
              "request",
              "--request-json",
              JSON.stringify(streamEnvelope),
            ]);
            if (gen !== this.readGeneration) return null;
            if (streamOut.code === 0) {
              const streamReply = JSON.parse(streamOut.stdout);
              const streamRead = streamReply?.result?.ok;
              const streamItem = streamRead?.values?.[0];
              if (streamItem?.ok?.kind === "stream") {
                this.streams.set(streamItem.ok.value.id, streamItem.ok.value);
              }
            }
          }

          this.lastError = null;
          this.lastFailedKey = null;
          this.lastCompletedAt = now;
          this.lastCompletedKey =
            key !== undefined
              ? JSON.stringify({
                  ...JSON.parse(key),
                  stream: this.knownStreamForTab({ visibleTerminals, focusedTerminal }),
                })
              : null;
        }
        return reply;
      } catch (err) {
        if (gen === this.readGeneration) {
          this.lastError = err instanceof Error ? err.message : String(err);
          this.lastFailedKey = key ?? null;
        }
        return null;
      } finally {
        if (gen === this.readGeneration) {
          this.inFlightKey = null;
          this.inFlightPromise = null;
          this.notifyViews();
        }
      }
    })();
    this.inFlightPromise = promise;
    return promise;
  }

  /**
   * Execute a one-shot `open_work` or `open_reference` action and dispatch any returned
   * `ViewerAction` items once on the fresh reply.
   */
  async act(
    action,
    { activeSpaceId = null, activeTabId = null, targetPaneId = null, run, openUrl, openOverlay, focusTerminal } = {}
  ) {
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
    if (reply?.result?.error) {
      this.lastError = reply.result.error.message || "streams integration action failed";
      this.notifyViews();
      return reply;
    }
    const ok = reply?.result?.ok;
    let viewerActions = [];
    if (ok?.kind === "work_opened") {
      viewerActions = ok.value?.viewer_actions ?? [];
      this.lastError = null;
    } else if (ok?.kind === "reference_opened") {
      const res = ok.value?.result;
      if (res?.kind === "needs_viewer") {
        viewerActions = res.value ?? [];
        this.lastError = null;
      } else if (res?.kind === "launch_command" || res?.kind === "show_content") {
        try {
          const outcome = dispatchReferencePresentation(res, {
            activeSpaceId,
            activeTabId,
            targetPaneId,
            reference: action?.reference ?? null,
            tabNaming: this.tabNaming,
            streamsBin: this.streamsBin,
            run,
            openOverlay,
            onPendingViewer: (entry) => this.recordPendingViewer(entry),
          });
          this.lastError = outcome.ok ? null : outcome.error;
        } catch (err) {
          this.lastError = err instanceof Error ? err.message : String(err);
        }
      } else if (res?.kind === "not_applied" || res?.kind === "uncertain") {
        this.lastError = res.reason || `reference ${res.kind}`;
      } else {
        this.lastError = null;
      }
    }
    if (Array.isArray(reply?.viewer_projection) && reply.viewer_projection.length > 0) {
      viewerActions = [...viewerActions, ...reply.viewer_projection];
    }
    dispatchViewerActions(viewerActions, { run, openUrl, focusTerminal });
    this.lastCompletedKey = null; // Force next observation to refresh views
    this.notifyViews();
    return reply;
  }

  openReference(
    reference,
    {
      activeSpaceId = null,
      activeTabId = null,
      targetPaneId = null,
      mode = null,
      placement = null,
      run,
      openUrl,
      openOverlay,
      focusTerminal,
    } = {}
  ) {
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
        ...(mode ? { mode } : {}),
        ...(placement ? { placement } : {}),
      },
      { activeSpaceId, activeTabId, targetPaneId, run, openUrl, openOverlay, focusTerminal }
    );
  }

  openWork(
    { stream, sessions = { kind: "all_eligible" }, cwd = null },
    { run, openUrl, focusTerminal } = {}
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
      { run, openUrl, focusTerminal }
    );
  }
}
