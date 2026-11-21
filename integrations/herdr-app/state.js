/**
 * Portable coordinator, label helpers, and reference viewer dispatcher for the herdr-app Streams mod.
 * Kept free of gpui-kit / herdr imports so unit tests can exercise all state and CLI rules.
 */

export const LABEL_SESSION = "streams.session";
export const LABEL_STREAM = "streams.stream";
export const LABEL_SAVED = "streams.saved";
export const LABEL_HERDR_SPACE = "herdr.space";
export const LABEL_HERDR_TAB = "herdr.tab";

export const STALE_FALLBACK_MS = 5 * 60 * 1000;
export const MAX_NATIVE_VIEW_BYTES = 400_000;

const ROLE_ORDER = {
  deliverable: 0,
  dependency: 1,
  brief: 2,
  background: 3,
};

/** Extract plain text from a format-5 Title/Goal/TextPair object or a plain string. */
export function textOf(value) {
  if (typeof value === "string") return value;
  if (value && typeof value === "object" && typeof value.text === "string") return value.text;
  return "";
}

/** Extract short text (falling back to full text) from a format-5 Goal/TextPair object or string. */
export function shortOf(value) {
  if (typeof value === "string") return value;
  if (value && typeof value === "object") {
    if (typeof value.short === "string" && value.short.length > 0) return value.short;
    if (typeof value.text === "string") return value.text;
  }
  return "";
}

/** Normalize a format-5 Stream record (from `stream show` or `stream list`) into a consistent shape. */
export function normalizeStream(raw) {
  if (!raw || typeof raw !== "object") return null;
  const key =
    typeof raw.key === "string"
      ? raw.key
      : raw.host_id && raw.id
        ? `${raw.host_id}/${raw.id}`
        : "";
  const id = typeof raw.id === "string" ? raw.id : key.includes("/") ? key.split("/").slice(1).join("/") : key;
  const hostId = typeof raw.host_id === "string" ? raw.host_id : key.includes("/") ? key.split("/")[0] : "";
  const sessions = Array.isArray(raw.sessions) ? raw.sessions.map(normalizeSession).filter(Boolean) : [];
  const activeSessions = sessions.filter(
    (s) => s.status === "active" || s.status === "waiting" || s.status === "blocked"
  ).length;
  return {
    key,
    id,
    host_id: hostId,
    title: textOf(raw.title),
    title_manual: Boolean(raw.title?.manual),
    goal: raw.goal ? textOf(raw.goal) : null,
    goal_short: raw.goal ? shortOf(raw.goal) : null,
    status: raw.status ?? "parked",
    sessions,
    active_sessions: typeof raw.active_sessions === "number" ? raw.active_sessions : activeSessions,
    total_sessions: typeof raw.total_sessions === "number" ? raw.total_sessions : sessions.length,
    unreadable: Array.isArray(raw.unreadable) ? raw.unreadable : [],
    last_activity_ms: raw.last_activity_ms ?? raw.updated_at_ms ?? 0,
    updated_at_ms: raw.updated_at_ms ?? raw.last_activity_ms ?? 0,
    revision: raw.revision ?? 0,
  };
}

/** Normalize a format-5 Session record (from `session show`, `session list`, or `stream.sessions`). */
export function normalizeSession(raw) {
  if (!raw || typeof raw !== "object") return null;
  const key =
    typeof raw.key === "string"
      ? raw.key
      : raw.host_id && raw.harness && raw.id
        ? `${raw.host_id}/${raw.harness}/${raw.id}`
        : "";
  const parts = key.split("/");
  const hostId = typeof raw.host_id === "string" ? raw.host_id : parts.length === 3 ? parts[0] : "";
  const harness = typeof raw.harness === "string" ? raw.harness : parts.length === 3 ? parts[1] : "";
  const id = typeof raw.id === "string" ? raw.id : parts.length === 3 ? parts[2] : key;
  const streamId = typeof raw.stream_id === "string" ? raw.stream_id : "";
  const streamKey = hostId && streamId ? `${hostId}/${streamId}` : streamId;
  return {
    key,
    host_id: hostId,
    harness,
    id,
    stream_id: streamId,
    stream: streamKey,
    title: textOf(raw.title),
    title_manual: Boolean(raw.title?.manual),
    status: raw.status ?? "parked",
    recap: raw.recap ? textOf(raw.recap) : null,
    recap_short: raw.recap ? shortOf(raw.recap) : null,
    state: raw.state ? textOf(raw.state) : null,
    state_short: raw.state ? shortOf(raw.state) : null,
    remaining: raw.remaining ? textOf(raw.remaining) : null,
    remaining_short: raw.remaining ? shortOf(raw.remaining) : null,
    directory: raw.directory ?? null,
    references: raw.references && typeof raw.references === "object" ? raw.references : {},
    artifacts: raw.artifacts && typeof raw.artifacts === "object" ? raw.artifacts : {},
    document_files: raw.document_files && typeof raw.document_files === "object" ? raw.document_files : {},
    updated_at_ms: raw.updated_at_ms ?? 0,
    revision: raw.revision ?? 0,
  };
}

/**
 * Determine whether an enrolled session is actually live in its terminal/pane,
 * surfacing "offline" when an active/waiting record no longer has a live agent.
 */
export function effectiveSessionStatus(session, { pane = null, terminal = null, agent = null } = {}) {
  if (!session) return "unknown";
  const status = session.status;
  if (status === "done" || status === "parked" || status === "closed") return status;
  if (terminal?.state && terminal.state !== "live") return "offline";
  if (pane?.program && !pane.program.agent && !agent) return "offline";
  return status;
}

export const HOST_STREAM_FILTERS = [
  { id: "in_progress", label: "In Progress" },
  { id: "parked", label: "Parked" },
  { id: "done", label: "Done" },
  { id: "closed", label: "Closed" },
  { id: "all", label: "All" },
];

export const HOST_STREAM_SORTS = [
  { id: "recent", label: "Recent" },
  { id: "alpha", label: "A–Z" },
];

function matchesHostStreamFilter(status, filter) {
  switch (filter) {
    case "in_progress":
      return status === "active" || status === "waiting" || status === "blocked";
    case "parked":
      return status === "parked";
    case "done":
      return status === "done";
    case "closed":
      return status === "closed";
    case "all":
      return true;
    default:
      return true;
  }
}

/**
 * Filter and sort host Stream summaries in the unassociated sidebar.
 */
export function filterHostStreams(
  streams,
  { filter = "in_progress", sort = "recent", query = "" } = {}
) {
  if (!Array.isArray(streams)) return [];
  const q = typeof query === "string" ? query.trim().toLowerCase() : "";
  const filtered = streams.filter((summary) => {
    if (!summary || !matchesHostStreamFilter(summary.status, filter)) return false;
    if (!q) return true;
    const title = (summary.title ?? "").toLowerCase();
    const goalShort = (summary.goal_short ?? "").toLowerCase();
    const id = (summary.id ?? "").toLowerCase();
    return title.includes(q) || goalShort.includes(q) || id.includes(q);
  });
  filtered.sort((a, b) => {
    if (sort === "alpha") {
      const cmp = (a.title ?? "").localeCompare(b.title ?? "", undefined, {
        sensitivity: "base",
      });
      if (cmp !== 0) return cmp;
      return (a.id ?? "").localeCompare(b.id ?? "");
    }
    const tsDiff = (b.last_activity_ms ?? 0) - (a.last_activity_ms ?? 0);
    if (tsDiff !== 0) return tsDiff;
    const cmp = (a.title ?? "").localeCompare(b.title ?? "", undefined, {
      sensitivity: "base",
    });
    if (cmp !== 0) return cmp;
    return (a.id ?? "").localeCompare(b.id ?? "");
  });
  return filtered;
}

/**
 * The short sidebar notice for records a listing or Stream view could not read: one entry per
 * record with the repair Streams chose for its kind. Null when every record was read.
 */
export function unreadableNotice(unreadable) {
  if (!Array.isArray(unreadable) || unreadable.length === 0) return null;
  const count = unreadable.length;
  return {
    title: `${count} unreadable record${count === 1 ? " is" : "s are"} not shown`,
    records: unreadable.map((record) => ({
      label: `${record.path}: ${record.cause}`,
      repair: record.repair,
    })),
  };
}

/**
 * What the host Stream list says when it shows no rows, or null when it shows some. A failed
 * listing names its error; otherwise the message says whether a filter, unreadable records or
 * an empty catalog left the list empty.
 */
export function hostStreamsEmptyMessage({ error, streams, shown, unreadable }) {
  if (error) return `Streams are unavailable: ${error}`;
  if (shown.length > 0) return null;
  if (streams.length > 0) return "No matching Streams.";
  if (unreadable.length > 0) return "No readable Streams on this host.";
  return "No Streams on this host.";
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/**
 * Format a compact relative age ("now", "12m", "3h", "2d", or "Nov 16").
 */
export function formatRelativeAge(ms, now = Date.now()) {
  if (typeof ms !== "number" || !Number.isFinite(ms) || ms <= 0) return "";
  const diff = Math.max(0, now - ms);
  if (diff < 60_000) return "now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h`;
  if (diff < 30 * 86_400_000) return `${Math.floor(diff / 86_400_000)}d`;
  const d = new Date(ms);
  return `${MONTHS[d.getMonth()]} ${d.getDate()}`;
}

/** Return the local terminal address (id/address), or null for remote terminals. */
export function localTerminalAddress(term) {
  if (!term || (term.machine !== undefined && term.machine !== null)) return null;
  const addr = term.address ?? term.id;
  if (typeof addr !== "string" || addr.length === 0 || addr.includes("/")) return null;
  return addr;
}

/**
 * Build a map of terminalKey -> sessionKey for local terminals that carry a non-empty
 * `streams.session` label AND have a live Pi agent reported by Herdr.
 */
export function runningSessionTerminals({ terminalList = [], panes = [], agents = [] } = {}) {
  const paneByTerm = new Map();
  for (const pane of panes ?? []) {
    if (pane?.terminal) paneByTerm.set(pane.terminal, pane);
  }
  const agentByTerm = new Map();
  for (const ag of agents ?? []) {
    const addr = ag?.address ?? ag?.terminal;
    if (addr) agentByTerm.set(addr, ag);
  }

  const running = new Map();
  for (const term of terminalList ?? []) {
    const addr = localTerminalAddress(term);
    if (!addr) continue;
    const sessionKey = term.labels?.[LABEL_SESSION];
    if (typeof sessionKey !== "string" || sessionKey.trim().length === 0) continue;
    if (term.state && term.state !== "live") continue;
    const ag = agentByTerm.get(addr);
    const pane = paneByTerm.get(addr);
    const agentKind = ag?.agent ?? ag?.program ?? pane?.program?.agent ?? null;
    // A terminal is running a Session when Herdr reports a live Pi agent in it.
    const isLivePi =
      agentKind === "pi" ||
      (agentKind === true && Boolean(pane?.program?.agent)) ||
      (typeof agentKind === "string" && agentKind.toLowerCase() === "pi");
    if (!isLivePi) continue;
    running.set(addr, sessionKey.trim());
  }
  return running;
}

/**
 * Find the space key for `streamKey` from the `herdr.space` label of any local terminal
 * labelled `streams.stream=<streamKey>`. Returns null when none exists.
 */
export function findSpaceForStream(terminalList, streamKey) {
  if (!streamKey || !Array.isArray(terminalList)) return null;
  for (const term of terminalList) {
    if (!localTerminalAddress(term)) continue;
    if (term.labels?.[LABEL_STREAM] !== streamKey) continue;
    const space = term.labels?.[LABEL_HERDR_SPACE];
    if (typeof space === "string" && space.trim().length > 0) {
      return space.trim();
    }
  }
  return null;
}

/**
 * Identify Stream keys that currently have a live running agent terminal in Herdr,
 * from either `frames` or `streams.stream` labels on live local terminals.
 */
export function liveStreamIds({ panes = [], terminalList = [], agents = [], frames } = {}) {
  const live = new Set();
  const running = runningSessionTerminals({ terminalList, panes, agents });
  const termById = new Map();
  for (const term of terminalList ?? []) {
    const addr = localTerminalAddress(term);
    if (addr) termById.set(addr, term);
  }
  for (const [addr] of running) {
    const frame = frames?.get?.(addr);
    if (frame?.kind === "enrolled" && frame.session?.stream) {
      live.add(frame.session.stream);
      if (frame.session.stream_id) live.add(frame.session.stream_id);
    }
    const streamLabel = termById.get(addr)?.labels?.[LABEL_STREAM];
    if (typeof streamLabel === "string" && streamLabel.trim()) {
      live.add(streamLabel.trim());
      const parts = streamLabel.trim().split("/");
      if (parts.length === 2) live.add(parts[1]);
    }
  }
  return live;
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
 * The active tab's local terminals in pane order, deduplicated.
 */
export function tabTerminals(visibleTerminals = []) {
  const local = (key) => typeof key === "string" && key.length > 0 && !key.includes("/");
  return [...new Set(visibleTerminals.filter(local))];
}

/**
 * Extract the combined `streams.saved` token across all local terminals.
 */
export function observeNoticeToken(terminalList, notificationKey = LABEL_SAVED) {
  if (!notificationKey || !Array.isArray(terminalList)) return "";
  const pairs = [];
  for (const term of terminalList) {
    const addr = localTerminalAddress(term);
    if (!addr) continue;
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
 * Includes `focusedTerminal` so moving focus within a tab refreshes the sidebar's focused Session.
 */
export function computeObservationKey({
  activeSpace,
  activeTabId = null,
  visibleTerminals = [],
  focusedTerminal = null,
  terminalSessions = [],
  noticeToken = "",
}) {
  if (!activeSpace) return "none";
  if (activeSpace.machine !== null) return `remote:${activeSpace.id}`;
  return JSON.stringify({
    space: activeSpace.id,
    tab: activeTabId,
    terminals: tabTerminals(visibleTerminals),
    focused: focusedTerminal ?? null,
    sessions: terminalSessions,
    notice: noticeToken ?? "",
  });
}

/**
 * Resolve the primary enrolled session for the active tab (preferring the focused terminal),
 * counting only Sessions of `stream` when it is given.
 */
export function activeTabSession({
  visibleTerminals = [],
  focusedTerminal = null,
  frames,
  stream = null,
}) {
  if (!frames) return null;
  const enrolled = (terminalKey) => {
    const frame = frames.get(terminalKey);
    return frame?.kind === "enrolled" &&
      frame.session &&
      (stream === null || frame.session.stream === stream || frame.session.stream_id === stream)
      ? { terminal: terminalKey, session: frame.session }
      : null;
  };
  if (focusedTerminal && visibleTerminals.includes(focusedTerminal)) {
    const focused = enrolled(focusedTerminal);
    if (focused) return focused;
  }
  for (const terminalKey of visibleTerminals) {
    const found = enrolled(terminalKey);
    if (found) return found;
  }
  return null;
}

/**
 * Choose what the sidebar shows for the active tab:
 * - `loading`: the tab has not been read yet and a read is in flight.
 * - `session`: the tab's Stream with one of its enrolled Sessions (the focused one first), including Stream ID.
 * - `stream`: the tab's Stream when none of its Sessions is focused/visible in the tab (no references).
 * - `inspected`: a tab without a Stream, showing the Stream picked from the host listing (no references).
 * - `listing`: a tab without a Stream, showing the host's Streams.
 */
export function tabSidebarView({
  tabId,
  tabStreams,
  streams,
  frames,
  visibleTerminals = [],
  focusedTerminal = null,
  inspectedStreamId = null,
  inFlight = false,
}) {
  if (!tabStreams.has(tabId) && inFlight) return { kind: "loading" };
  const tabStreamKey = tabStreams.get(tabId) ?? "";
  const tabStream = streams.get(tabStreamKey) ?? null;
  if (tabStream) {
    const viewing = activeTabSession({
      visibleTerminals,
      focusedTerminal,
      frames,
      stream: tabStream.key,
    });
    return viewing
      ? { kind: "session", stream: tabStream, streamIdDisplay: tabStream.id, ...viewing }
      : { kind: "stream", stream: tabStream, streamIdDisplay: tabStream.id };
  }
  const inspected = inspectedStreamId ? streams.get(inspectedStreamId) ?? null : null;
  return inspected
    ? { kind: "inspected", stream: inspected, streamIdDisplay: inspected.id }
    : { kind: "listing" };
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
 * Build the sidebar References list from the **focused Session's** format-5 `references` map
 * (hiding `status === "dropped"`) and its `artifacts` map.
 * Returns `[]` when no Session is focused.
 */
export function buildReferenceItems(session) {
  if (!session || typeof session !== "object") return [];
  const items = [];

  const refs = Object.entries(session.references ?? {})
    .filter(([, entry]) => entry && entry.status !== "dropped")
    .sort(([keyA, a], [keyB, b]) => {
      const roleDiff = (ROLE_ORDER[a.role] ?? 99) - (ROLE_ORDER[b.role] ?? 99);
      if (roleDiff !== 0) return roleDiff;
      const nameDiff = (a.name ?? "").localeCompare(b.name ?? "");
      if (nameDiff !== 0) return nameDiff;
      return keyA.localeCompare(keyB);
    });

  for (const [refKey, entry] of refs) {
    const name = typeof entry.name === "string" ? entry.name.trim() : "";
    const purpose = typeof entry.purpose === "string" ? entry.purpose.trim() : "";
    if (entry.kind === "document") {
      const title = name || refKey;
      const location = refKey !== title ? refKey : null;
      const docState = session.document_files?.[refKey] ?? null;
      const unavailableReason =
        docState && typeof docState === "object" && docState.status === "unavailable"
          ? docState.reason ?? "document unavailable"
          : null;
      items.push({
        id: `doc-${refKey}`,
        kind: "document",
        role: entry.role,
        status: entry.status,
        title,
        location,
        description: purpose,
        monoTitle: !name,
        external: false,
        reference: {
          kind: "document",
          session: session.key,
          path: refKey,
          filename: refKey.split("/").pop() || refKey,
          title,
        },
        disabled: Boolean(unavailableReason),
        unavailableReason,
      });
    } else {
      const hostPath = formatHostPath(refKey);
      const title = name || hostPath || refKey;
      const location = hostPath && hostPath !== title ? hostPath : null;
      items.push({
        id: `link-${refKey}`,
        kind: entry.kind ?? "link",
        role: entry.role,
        status: entry.status,
        title,
        location,
        description: purpose,
        monoTitle: !name,
        external: true,
        reference: {
          kind: entry.kind ?? "link",
          url: refKey,
          title,
        },
        disabled: false,
      });
    }
  }

  const artifacts = Object.entries(session.artifacts ?? {}).sort(([a], [b]) => a.localeCompare(b));
  for (const [name, entry] of artifacts) {
    const artifactKey = `${session.key}/${name}`;
    items.push({
      id: `art-${artifactKey}`,
      kind: "artifact",
      title: name,
      location: null,
      description: typeof entry?.description === "string" ? entry.description.trim() : "",
      monoTitle: true,
      external: false,
      reference: {
        kind: "artifact",
        key: artifactKey,
        filename: name,
        title: name,
      },
      disabled: false,
    });
  }

  return items;
}

/**
 * Validate and bound text read from `session document read` or `artifact read` for the native
 * viewer, mirroring the safety rules of the old Rust `prepare_native_content`.
 */
export function prepareNativeContent(rawText) {
  if (typeof rawText !== "string") {
    return { ok: false, error: "non-UTF-8 file cannot be opened in native viewer" };
  }
  if (rawText.includes("\0")) {
    return { ok: false, error: "binary file cannot be opened in native viewer" };
  }
  const bytes = new TextEncoder().encode(rawText);
  if (bytes.byteLength <= MAX_NATIVE_VIEW_BYTES) {
    return { ok: true, content: rawText, truncated: false };
  }
  const decoder = new TextDecoder("utf-8", { fatal: false });
  const sliced = decoder.decode(bytes.subarray(0, MAX_NATIVE_VIEW_BYTES));
  return {
    ok: true,
    content: `${sliced}\n… (file truncated)`,
    truncated: true,
  };
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

function parseJsonSafe(text) {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

function extractCliError(out, fallback) {
  const parsed = parseJsonSafe(out?.stdout ?? "");
  if (parsed?.error?.message) {
    return { kind: parsed.error.kind ?? "error", message: parsed.error.message };
  }
  const msg = out?.stderr?.trim() || out?.stdout?.trim() || fallback;
  return { kind: "error", message: msg };
}

/**
 * Stateful coordinator that reads format-5 records through the public `streams` CLI,
 * reconciles `streams.stream` labels via `hc`, opens Streams via `hc run ... streams session resume`,
 * and opens references via `openUrl` / `session document read|view` / `artifact read|view`.
 */
export class StreamsCoordinator {
  constructor({
    exec,
    notify,
    streamsBin = "streams",
    hcBin = "hc",
    staleFallbackMs = STALE_FALLBACK_MS,
    newId = () => `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
  } = {}) {
    this.exec = exec;
    this.notify = notify;
    this.streamsBin = streamsBin;
    this.hcBin = hcBin;
    this.staleFallbackMs = staleFallbackMs;
    this.newId = newId;

    this.configLoaded = false;
    this.configPromise = null;
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

    this.streams = new Map(); // streamKey -> normalized Stream
    this.tabStreams = new Map(); // tabId -> streamKey | null
    this.hostStreams = []; // normalized Stream summaries
    this.hostUnreadable = [];
    this.hostStreamsError = null;
    this.inspectedStreamId = null; // streamKey
    this.frames = new Map(); // terminalKey -> { kind: "enrolled", session } | { kind: "unavailable", reason }
    this.pendingViewers = [];
    this.viewerMetaByPane = new Map();
    this.lastError = null;
  }

  /** Read `streams config get herdr --json` once when starting. */
  async ensureConfig() {
    if (this.configLoaded) return this.herdrConfig;
    if (this.configPromise) return this.configPromise;
    this.configPromise = (async () => {
      try {
        const out = await this.exec(this.streamsBin, ["config", "get", "herdr", "--json"]);
        if (out.code === 0) {
          const parsed = parseJsonSafe(out.stdout);
          const herdr = parsed?.herdr ?? parsed ?? {};
          const tabNaming = herdr?.naming?.tab ?? herdr?.tab_naming ?? true;
          const viewerMode = herdr?.viewer?.mode ?? herdr?.viewer_mode ?? "editor";
          const viewerPlacement = herdr?.viewer?.placement ?? herdr?.viewer_placement ?? "pane-right";
          this.herdrConfig = {
            tab_naming: Boolean(tabNaming),
            viewer_mode: viewerMode,
            viewer_placement: viewerPlacement,
          };
          this.tabNaming = this.herdrConfig.tab_naming;
        }
      } catch {
        // Keep defaults if config read fails
      } finally {
        this.configLoaded = true;
        this.configPromise = null;
      }
      return this.herdrConfig;
    })();
    return this.configPromise;
  }

  async inspectStream(streamKey) {
    if (!streamKey) {
      this.inspectedStreamId = null;
      this.notifyViews();
      return null;
    }
    this.inspectedStreamId = streamKey;
    const out = await this.exec(this.streamsBin, ["stream", "show", streamKey, "--json"]);
    if (out.code !== 0) {
      this.lastError = extractCliError(out, "saved Stream view is unavailable").message;
      this.notifyViews();
      return null;
    }
    const raw = parseJsonSafe(out.stdout);
    const stream = normalizeStream(raw);
    if (stream) {
      this.streams.set(stream.key, stream);
      this.lastError = null;
    }
    this.notifyViews();
    return stream;
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

  streamTitle(streamKey) {
    if (!streamKey) return "";
    const cached = this.streams.get(streamKey);
    if (cached?.title) return cached.title;
    return streamKey;
  }

  notifyViews() {
    this.notify?.();
  }

  /**
   * Observe current reactive desktop state and trigger a refresh only when the
   * observation key changed (or when `force` is true or the fallback timer elapsed).
   */
  observe({
    activeSpace,
    activeTabId = null,
    visibleTerminals = [],
    focusedTerminal = null,
    terminalList = [],
    panes = [],
    agents = [],
    tabIds,
    force = false,
    now = Date.now(),
  }) {
    if (activeSpace && activeSpace.machine !== null) {
      return null;
    }

    const running = runningSessionTerminals({ terminalList, panes, agents });
    const visible = tabTerminals(visibleTerminals);
    const terminalSessions = visible
      .filter((t) => running.has(t))
      .map((t) => `${t}=${running.get(t)}`);
    const noticeToken = observeNoticeToken(terminalList, LABEL_SAVED);
    const observation = {
      activeSpace,
      activeTabId,
      visibleTerminals: visible,
      focusedTerminal,
      terminalSessions,
      noticeToken,
    };
    const key = computeObservationKey(observation);

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
      observation,
      running,
      terminalList,
      tabIds,
      key,
      now,
    });
  }

  /**
   * Read records for the active tab through the public `streams` CLI and reconcile `streams.stream`
   * labels on visible running-Session terminals.
   */
  async refresh({ observation, running, terminalList = [], tabIds, key, now = Date.now() }) {
    if (Array.isArray(tabIds)) {
      for (const tabId of this.tabStreams.keys()) {
        if (!tabIds.includes(tabId)) this.tabStreams.delete(tabId);
      }
    }

    const gen = ++this.readGeneration;
    this.inFlightKey = key ?? null;

    const promise = (async () => {
      try {
        await this.ensureConfig();
        if (gen !== this.readGeneration) return null;

        const { activeTabId = null, visibleTerminals = [], focusedTerminal = null } = observation;

        // Candidate running-Session terminals in priority order: focused first, then pane order.
        const candidates = [];
        if (focusedTerminal && visibleTerminals.includes(focusedTerminal) && running.has(focusedTerminal)) {
          candidates.push(focusedTerminal);
        }
        for (const term of visibleTerminals) {
          if (running.has(term) && !candidates.includes(term)) {
            candidates.push(term);
          }
        }

        // Clear stale frames for visible terminals that are no longer running a Session.
        for (const term of visibleTerminals) {
          if (!running.has(term)) {
            this.frames.delete(term);
          }
        }

        // 1. Try reading the focused (or first visible) running Session with `session show`.
        let primaryTerm = null;
        let primarySession = null;
        for (const term of candidates) {
          const sessionKey = running.get(term);
          const out = await this.exec(this.streamsBin, ["session", "show", sessionKey, "--json"]);
          if (gen !== this.readGeneration) return null;
          if (out.code === 0) {
            const parsed = parseJsonSafe(out.stdout);
            const session = normalizeSession(parsed);
            if (session) {
              primaryTerm = term;
              primarySession = session;
              this.frames.set(term, { kind: "enrolled", session });
              break;
            }
          } else {
            const err = extractCliError(out, "session show failed");
            if (err.kind === "not-found") {
              // Stale `streams.session` label: ignore it and move on to the next candidate.
              this.frames.delete(term);
              continue;
            }
            this.lastError = err.message;
            this.lastFailedKey = key ?? null;
            return null;
          }
        }

        if (primarySession) {
          const streamKey = `${primarySession.host_id}/${primarySession.stream_id}`;
          // 2. Read that Session's Stream with `stream show`.
          const streamOut = await this.exec(this.streamsBin, ["stream", "show", streamKey, "--json"]);
          if (gen !== this.readGeneration) return null;
          if (streamOut.code !== 0) {
            const err = extractCliError(streamOut, "stream show failed");
            this.lastError = err.message;
            this.lastFailedKey = key ?? null;
            return null;
          }
          const stream = normalizeStream(parseJsonSafe(streamOut.stdout));
          if (!stream) {
            this.lastError = "invalid stream show JSON";
            this.lastFailedKey = key ?? null;
            return null;
          }
          this.streams.set(stream.key, stream);
          this.tabStreams.set(activeTabId, stream.key);
          this.inspectedStreamId = null;

          // Index the Stream's member Sessions by key for pane headers.
          const byKey = new Map();
          byKey.set(primarySession.key, primarySession);
          for (const member of stream.sessions) {
            if (!byKey.has(member.key)) byKey.set(member.key, member);
          }

          // 3. Only if another visible terminal carries a Session not in `stream.sessions`,
          // fetch those keys in one `session list --key ...` call.
          const missingTerms = [];
          for (const term of visibleTerminals) {
            if (term === primaryTerm) continue;
            const sk = running.get(term);
            if (!sk) continue;
            if (byKey.has(sk)) {
              this.frames.set(term, { kind: "enrolled", session: byKey.get(sk) });
            } else {
              missingTerms.push({ term, key: sk });
            }
          }

          if (missingTerms.length > 0) {
            const listArgs = ["session", "list"];
            for (const item of missingTerms) {
              listArgs.push("--key", item.key);
            }
            listArgs.push("--all", "--fields", "key,stream_id,title,status,state", "--json");
            const listOut = await this.exec(this.streamsBin, listArgs);
            if (gen !== this.readGeneration) return null;
            if (listOut.code === 0) {
              const parsed = parseJsonSafe(listOut.stdout);
              for (const raw of parsed?.items ?? []) {
                const sess = normalizeSession(raw);
                if (sess) byKey.set(sess.key, sess);
              }
            }
            for (const item of missingTerms) {
              const found = byKey.get(item.key);
              if (found) {
                this.frames.set(item.term, { kind: "enrolled", session: found });
              } else {
                this.frames.delete(item.term);
              }
            }
          }

          // 4. Reconcile `streams.stream` hc labels on visible terminals running known Sessions.
          const termByAddr = new Map();
          for (const t of terminalList) {
            const addr = localTerminalAddress(t);
            if (addr) termByAddr.set(addr, t);
          }
          for (const term of visibleTerminals) {
            const frame = this.frames.get(term);
            if (frame?.kind !== "enrolled" || !frame.session?.stream) continue;
            const desiredStream = frame.session.stream;
            const currentLabel = termByAddr.get(term)?.labels?.[LABEL_STREAM] ?? null;
            if (currentLabel !== desiredStream) {
              try {
                await this.exec(this.hcBin, ["label", term, `${LABEL_STREAM}=${desiredStream}`]);
              } catch {
                // Label reconciliation failure is non-fatal
              }
            }
          }

          this.lastError = null;
          this.lastFailedKey = null;
          this.lastCompletedAt = now;
          this.lastCompletedKey = key ?? null;
          return { kind: "stream", stream, session: primarySession };
        }

        // No visible running Session in this tab: fetch the local Stream picker list.
        this.tabStreams.set(activeTabId, null);
        const listOut = await this.exec(this.streamsBin, [
          "stream",
          "list",
          "--local",
          "--all",
          "--json",
          "--fields",
          "key,title,status,last_activity_ms",
        ]);
        if (gen !== this.readGeneration) return null;
        if (listOut.code !== 0) {
          const err = extractCliError(listOut, "host Streams are unavailable");
          this.hostStreams = [];
          this.hostUnreadable = [];
          this.hostStreamsError = err.message;
          this.lastFailedKey = key ?? null;
          return null;
        }
        const parsed = parseJsonSafe(listOut.stdout);
        if (!parsed || !Array.isArray(parsed.items)) {
          this.hostStreams = [];
          this.hostUnreadable = [];
          this.hostStreamsError = "unexpected stream list response from streams";
          this.lastFailedKey = key ?? null;
          return null;
        }
        this.hostStreams = parsed.items.map(normalizeStream).filter(Boolean);
        this.hostUnreadable = Array.isArray(parsed.unreadable) ? parsed.unreadable : [];
        this.hostStreamsError = null;
        this.lastError = null;
        this.lastFailedKey = null;
        this.lastCompletedAt = now;
        this.lastCompletedKey = key ?? null;
        return { kind: "listing", streams: this.hostStreams };
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
   * Open a reference (URL, document, or artifact) directly using the public CLI commands.
   */
  async openReference(
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
    } = {}
  ) {
    if (!reference || typeof reference !== "object") return null;
    await this.ensureConfig();

    // 1. Links (PR, issue, link): open directly via openUrl.
    if (reference.kind === "pr" || reference.kind === "issue" || reference.kind === "link") {
      if (typeof reference.url === "string" && reference.url.length > 0) {
        openUrl?.(reference.url);
        this.lastError = null;
      } else {
        this.lastError = "reference has no URL";
      }
      this.notifyViews();
      return { ok: !this.lastError };
    }

    const effectiveMode = mode ?? this.herdrConfig.viewer_mode ?? "editor";
    const effectivePlacement = placement ?? this.herdrConfig.viewer_placement ?? "pane-right";
    const title = reference.title ?? reference.filename ?? "";
    const filename = reference.filename ?? title;

    // 2. Documents and Artifacts.
    let readArgs = null;
    let viewArgs = null;
    if (reference.kind === "document") {
      if (!reference.session || !reference.path) {
        this.lastError = "document reference is missing session or path";
        this.notifyViews();
        return { ok: false };
      }
      readArgs = ["session", "document", "read", reference.session, reference.path];
      viewArgs = ["session", "document", "view", reference.session, reference.path];
    } else if (reference.kind === "artifact") {
      if (!reference.key) {
        this.lastError = "artifact reference is missing key";
        this.notifyViews();
        return { ok: false };
      }
      readArgs = ["artifact", "read", reference.key];
      viewArgs = ["artifact", "view", reference.key];
    } else {
      this.lastError = `unsupported reference kind: ${reference.kind}`;
      this.notifyViews();
      return { ok: false };
    }

    if (effectiveMode === "native") {
      const out = await this.exec(this.streamsBin, readArgs);
      if (out.code !== 0) {
        this.lastError = extractCliError(out, "could not read reference").message;
        this.notifyViews();
        return { ok: false };
      }
      const prepared = prepareNativeContent(out.stdout ?? "");
      if (!prepared.ok) {
        this.lastError = prepared.error;
        this.notifyViews();
        return { ok: false };
      }
      const outcome = dispatchReferencePresentation(
        {
          kind: "show_content",
          placement: effectivePlacement,
          title,
          filename,
          content: prepared.content,
          truncated: prepared.truncated,
        },
        {
          activeSpaceId,
          activeTabId,
          targetPaneId,
          reference,
          tabNaming: this.tabNaming,
          streamsBin: this.streamsBin,
          run,
          openOverlay,
          onPendingViewer: (entry) => this.recordPendingViewer(entry),
        }
      );
      this.lastError = outcome.ok ? null : outcome.error;
      this.notifyViews();
      return outcome;
    }

    const outcome = dispatchReferencePresentation(
      {
        kind: "launch_command",
        placement: effectivePlacement,
        title,
        args: viewArgs,
      },
      {
        activeSpaceId,
        activeTabId,
        targetPaneId,
        reference,
        tabNaming: this.tabNaming,
        streamsBin: this.streamsBin,
        run,
        openOverlay,
      }
    );
    this.lastError = outcome.ok ? null : outcome.error;
    this.notifyViews();
    return outcome;
  }

  /**
   * Open a Stream (or specific Session(s) of a Stream) by querying `session list --stream`,
   * reusing already-running Sessions, finding the Stream's space from `streams.stream` labels,
   * and spawning missing Sessions with `hc run ... streams session resume KEY`.
   */
  async openWork(
    { stream, sessions = { kind: "all_eligible" }, terminalList = [], panes = [], agents = [] },
    { run, focusTerminal } = {}
  ) {
    if (!stream) return null;
    await this.ensureConfig();

    const listOut = await this.exec(this.streamsBin, [
      "session",
      "list",
      "--stream",
      stream,
      "--fields",
      "key,status,harness,directory",
      "--json",
    ]);
    if (listOut.code !== 0) {
      this.lastError = extractCliError(listOut, "could not list Sessions for Stream").message;
      this.notifyViews();
      return { ok: false, error: this.lastError };
    }
    const parsed = parseJsonSafe(listOut.stdout);
    const allSessions = (parsed?.items ?? []).map(normalizeSession).filter(Boolean);

    // Keep Pi Sessions that are active or parked (or specifically requested session IDs/keys).
    let kept;
    if (sessions?.kind === "selected" && Array.isArray(sessions.session_ids)) {
      const wanted = new Set(sessions.session_ids);
      kept = allSessions.filter((s) => wanted.has(s.key) || wanted.has(s.id));
    } else {
      kept = allSessions.filter(
        (s) => s.harness === "pi" && (s.status === "active" || s.status === "parked")
      );
    }

    const running = runningSessionTerminals({ terminalList, panes, agents });
    const termBySessionKey = new Map();
    for (const [termAddr, sessionKey] of running) {
      termBySessionKey.set(sessionKey, termAddr);
    }

    const existingSpace = findSpaceForStream(terminalList, stream);
    const spaceKey = existingSpace ?? `streams-${this.newId()}`;
    if (existingSpace) {
      run?.("space.select", { id: existingSpace });
    }

    const launched = [];
    const reused = [];
    const missingDirectory = [];

    for (const sess of kept) {
      const runningTerm = termBySessionKey.get(sess.key);
      if (runningTerm) {
        reused.push({ session: sess.key, terminal: runningTerm });
        if (focusTerminal) {
          focusTerminal(runningTerm);
        } else {
          run?.("pane.focus", { terminal: runningTerm });
        }
        continue;
      }
      const cwd = sess.directory?.path;
      if (typeof cwd !== "string" || cwd.length === 0) {
        missingDirectory.push(sess.key);
        continue;
      }
      const tabKey = `streams-tab-${this.newId()}`;
      const hcArgs = [
        "run",
        "--bg",
        "--no-parent",
        "--cwd",
        cwd,
        "--label",
        `${LABEL_STREAM}=${stream}`,
        "--label",
        `${LABEL_HERDR_SPACE}=${spaceKey}`,
        "--label",
        `${LABEL_HERDR_TAB}=${tabKey}`,
        this.streamsBin,
        "session",
        "resume",
        sess.key,
      ];
      const runOut = await this.exec(this.hcBin, hcArgs);
      if (runOut.code !== 0) {
        this.lastError = extractCliError(runOut, `could not resume ${sess.key}`).message;
        this.notifyViews();
        return { ok: false, error: this.lastError, launched, reused, missingDirectory };
      }
      launched.push({ session: sess.key, space: spaceKey, tab: tabKey, cwd });
    }

    if (missingDirectory.length > 0) {
      this.lastError = `Skipped Session without directory: ${missingDirectory.join(", ")}`;
    } else {
      this.lastError = null;
    }
    this.lastCompletedKey = null;
    this.notifyViews();
    return { ok: true, space: spaceKey, launched, reused, missingDirectory };
  }
}
