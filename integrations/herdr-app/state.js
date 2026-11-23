/**
 * Portable coordinator, label helpers, and reference viewer dispatcher for the herdr-app Streams mod.
 * Kept free of gpui-kit / herdr imports so unit tests can exercise all state and CLI rules.
 */

export const LABEL_SESSION = "streams.session";
export const LABEL_STREAM = "streams.stream";
export const LABEL_SAVED = "streams.saved";
export const LABEL_HERDR_SPACE = "herdr.space";
export const LABEL_HERDR_TAB = "herdr.tab";
export const SHARED_TERMINAL_MARKS_KEY = "terminal.marks";

export const SESSION_STATUSES = ["active", "waiting", "blocked", "parked", "done", "closed"];

export const STALE_FALLBACK_MS = 5 * 60 * 1000;
export const MAX_NATIVE_VIEW_BYTES = 400_000;

const HOST_STREAM_LIST_FIELDS = "key,title,goal,status,active_sessions,total_sessions,last_activity_ms";

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
  const sessions = Array.isArray(raw.sessions)
    ? raw.sessions
        .map((s) =>
          s && typeof s === "object"
            ? normalizeSession({
                ...s,
                host_id: s.host_id || hostId,
                stream_id: s.stream_id || id,
              })
            : null
        )
        .filter(Boolean)
    : [];
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
export function hostStreamsEmptyMessage({
  error,
  streams,
  shown,
  unreadable,
  scope = "local",
}) {
  if (error) return `Streams are unavailable: ${error}`;
  if (shown.length > 0) return null;
  if (streams.length > 0) return "No matching Streams.";
  if (scope === "all") {
    if (unreadable.length > 0) return "No readable Streams in the catalog.";
    return "No Streams in the catalog.";
  }
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

/**
 * Parse a terminal object or address string into `{ address, machine, localId }`.
 * Local terminals return `machine: null` and `address === localId` (e.g. `"t1"`).
 * Remote terminals return `machine: "retriever"`, `localId: "t8p"`, `address: "retriever/t8p"`.
 */
export function parseTerminalAddress(term) {
  if (!term) return null;
  if (typeof term === "string") {
    const addr = term.trim();
    if (addr.length === 0) return null;
    const slash = addr.indexOf("/");
    if (slash === -1) {
      return { address: addr, machine: null, localId: addr };
    }
    const machine = addr.slice(0, slash);
    const localId = addr.slice(slash + 1);
    if (!machine || !localId) return null;
    return { address: addr, machine, localId };
  }
  const rawAddr = term.address ?? term.id;
  if (typeof rawAddr !== "string" || rawAddr.trim().length === 0) return null;
  const trimmed = rawAddr.trim();
  const explicitMachine =
    typeof term.machine === "string" && term.machine.trim().length > 0
      ? term.machine.trim()
      : null;
  const slash = trimmed.indexOf("/");
  if (slash !== -1) {
    const machine = trimmed.slice(0, slash);
    const localId = trimmed.slice(slash + 1);
    if (!machine || !localId) return null;
    return { address: trimmed, machine, localId };
  }
  if (explicitMachine) {
    return {
      address: `${explicitMachine}/${trimmed}`,
      machine: explicitMachine,
      localId: trimmed,
    };
  }
  return { address: trimmed, machine: null, localId: trimmed };
}

/** Extract the owning Streams host_id prefix from a Stream or Session key. */
export function recordHostId(key) {
  if (typeof key !== "string" || key.length === 0) return null;
  const slash = key.indexOf("/");
  if (slash <= 0) return null;
  return key.slice(0, slash);
}

/** Qualify a raw remote space/tab/pane/terminal ID with `<machine>/` for desktop herdr-app APIs. */
export function qualifyForMachine(id, machine = null) {
  if (typeof id !== "string" || id.length === 0) return id ?? null;
  if (!machine) return id;
  return id.includes("/") ? id : `${machine}/${id}`;
}

/** Strip a leading `<machine>/` prefix so remote `hc` labels stay unqualified. */
export function unqualifyForMachine(id, machine = null) {
  if (typeof id !== "string" || id.length === 0) return id ?? null;
  if (!machine) return id;
  const prefix = `${machine}/`;
  return id.startsWith(prefix) ? id.slice(prefix.length) : id;
}

/** Return the local terminal address (id/address), or null for remote terminals. */
export function localTerminalAddress(term) {
  const parsed = parseTerminalAddress(term);
  if (!parsed || parsed.machine !== null) return null;
  return parsed.address;
}

/**
 * Build a map of terminalKey -> sessionKey for local and remote terminals that carry a non-empty
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
    const parsed = parseTerminalAddress(term);
    if (!parsed) continue;
    const addr = parsed.address;
    const sessionKey = term.labels?.[LABEL_SESSION];
    if (typeof sessionKey !== "string" || sessionKey.trim().length === 0) continue;
    if (term.state && term.state !== "live") continue;
    const ag = agentByTerm.get(addr);
    const pane = paneByTerm.get(addr);
    // In real herdr-app, AgentInfo/PaneInfo expose `app: "pi"` and `program: { key: "pi", agent: true }`.
    // Unit tests may also pass shorthand `agent: "pi"` or `program: { agent: "pi" }`.
    const explicitKind =
      (typeof ag?.app === "string" && ag.app) ||
      (typeof ag?.program?.key === "string" && ag.program.key) ||
      (typeof ag?.agent === "string" && ag.agent) ||
      (typeof pane?.app === "string" && pane.app) ||
      (typeof pane?.program?.key === "string" && pane.program.key) ||
      (typeof pane?.program?.agent === "string" && pane.program.agent) ||
      (typeof term?.app === "string" && term.app) ||
      (typeof term?.program?.key === "string" && term.program.key) ||
      (typeof term?.agent === "string" && term.agent) ||
      null;
    const hasAgentBool = Boolean(
      ag?.program?.agent === true ||
        pane?.program?.agent === true ||
        term?.program?.agent === true
    );
    const isLivePi = explicitKind
      ? explicitKind.toLowerCase() === "pi"
      : hasAgentBool;
    if (!isLivePi) continue;
    running.set(addr, sessionKey.trim());
  }
  return running;
}

/**
 * Find the unqualified space key for `streamKey` from the `herdr.space` label of any terminal
 * on `machine` (`null` for local) labelled `streams.stream=<streamKey>`. Returns null when none exists.
 */
export function findSpaceForStream(terminalList, streamKey, machine = null) {
  if (!streamKey || !Array.isArray(terminalList)) return null;
  const targetMachine = machine ?? null;
  for (const term of terminalList) {
    const parsed = parseTerminalAddress(term);
    if (!parsed || parsed.machine !== targetMachine) continue;
    if (term.labels?.[LABEL_STREAM] !== streamKey) continue;
    const space = term.labels?.[LABEL_HERDR_SPACE];
    if (typeof space === "string" && space.trim().length > 0) {
      return unqualifyForMachine(space.trim(), targetMachine);
    }
  }
  return null;
}

/**
 * Determine whether moving `sessionKey` to `targetStreamKey` can also move its live tab
 * to an open Space belonging to `targetStreamKey` on the same machine.
 */
export function resolveMoveSpaceTarget({
  sessionKey,
  targetStreamKey,
  machine = null,
  terminalList = [],
  panes = [],
  spaces = [],
} = {}) {
  const targetMachine = machine ?? null;
  const termByAddr = new Map();
  for (const term of terminalList ?? []) {
    const parsed = parseTerminalAddress(term);
    if (parsed) termByAddr.set(parsed.address, { term, parsed });
  }
  const spaceById = new Map();
  for (const sp of spaces ?? []) {
    if (sp?.id && (sp.machine ?? null) === targetMachine) {
      spaceById.set(sp.id, sp);
    }
  }

  let liveTerm = null;
  let livePane = null;
  if (sessionKey) {
    for (const pane of panes ?? []) {
      const entry = termByAddr.get(pane?.terminal);
      if (!entry || entry.parsed.machine !== targetMachine) continue;
      if (entry.term.labels?.[LABEL_SESSION] === sessionKey) {
        liveTerm = entry.term;
        livePane = pane;
        break;
      }
    }
    if (!liveTerm) {
      for (const { term, parsed } of termByAddr.values()) {
        if (parsed.machine !== targetMachine) continue;
        if (term.labels?.[LABEL_SESSION] === sessionKey && term.state === "live") {
          liveTerm = term;
          break;
        }
      }
    }
  }

  const currentSpaceId = livePane?.space ?? null;
  let currentTabName = null;
  if (currentSpaceId && livePane?.tab) {
    const sp = spaceById.get(currentSpaceId);
    const tb = sp?.tabs?.find((t) => t.id === livePane.tab);
    if (tb?.named && typeof tb.name === "string" && tb.name.trim()) {
      currentTabName = tb.name.trim();
    }
  }

  let targetSpace = null;
  if (targetStreamKey) {
    // Prefer an open pane currently enrolled in targetStreamKey.
    for (const pane of panes ?? []) {
      const entry = termByAddr.get(pane?.terminal);
      if (!entry || entry.parsed.machine !== targetMachine) continue;
      if (entry.term.labels?.[LABEL_STREAM] !== targetStreamKey) continue;
      const sp = spaceById.get(pane.space);
      if (sp) {
        targetSpace = sp;
        break;
      }
    }
    if (!targetSpace) {
      const liveTerms = (terminalList ?? []).filter((t) => t?.state === "live");
      const rawSpaceId = findSpaceForStream(liveTerms, targetStreamKey, targetMachine);
      if (rawSpaceId) {
        const lookupId =
          targetMachine !== null ? qualifyForMachine(rawSpaceId, targetMachine) : rawSpaceId;
        targetSpace = spaceById.get(lookupId) ?? null;
      }
    }
  }

  if (!liveTerm || !livePane) {
    return {
      liveTerm,
      livePane,
      currentSpaceId,
      currentTabName,
      targetSpace,
      canMoveTab: false,
      reason: "Session is not open",
    };
  }
  if (!targetSpace) {
    return {
      liveTerm,
      livePane,
      currentSpaceId,
      currentTabName,
      targetSpace: null,
      canMoveTab: false,
      reason: "Target stream has no open space",
    };
  }
  if (targetSpace.id === currentSpaceId) {
    return {
      liveTerm,
      livePane,
      currentSpaceId,
      currentTabName,
      targetSpace,
      canMoveTab: false,
      reason: "Already in this space",
    };
  }
  return {
    liveTerm,
    livePane,
    currentSpaceId,
    currentTabName,
    targetSpace,
    canMoveTab: true,
    reason: null,
  };
}

/**
 * Pure candidate builder for the "Move to stream…" picker.
 * Accepts snapshot-shaped input (`allSpaces` or `spaces`, `panes`, `terminalList`).
 */
export function buildMoveCandidates({
  sessionKey,
  sourceStreamKey,
  hostId = null,
  machine = null,
  streams = [],
  query = "",
  snapshot = {},
} = {}) {
  const targetMachine = machine ?? null;
  const spaces = snapshot.allSpaces ?? snapshot.spaces ?? [];
  const panes = snapshot.panes ?? [];
  const terminalList = snapshot.terminalList ?? [];

  const q = (query ?? "").trim().toLowerCase();
  const list = [];
  for (const item of streams ?? []) {
    if (!item?.key || item.key === sourceStreamKey) continue;
    if (hostId && item.host_id && item.host_id !== hostId) continue;
    if (q) {
      const hay = `${item.title ?? ""} ${item.goal ?? ""} ${item.id ?? ""}`.toLowerCase();
      if (!hay.includes(q)) continue;
    }
    const spaceInfo = resolveMoveSpaceTarget({
      sessionKey,
      targetStreamKey: item.key,
      machine: targetMachine,
      terminalList,
      panes,
      spaces,
    });
    list.push({
      stream: item,
      spaceInfo,
      isArchived: item.status === "done" || item.status === "closed",
    });
  }
  list.sort((a, b) => {
    if (a.isArchived !== b.isArchived) return a.isArchived ? 1 : -1;
    const aOpen = a.spaceInfo.targetSpace ? 1 : 0;
    const bOpen = b.spaceInfo.targetSpace ? 1 : 0;
    if (aOpen !== bOpen) return bOpen - aOpen;
    return (b.stream.last_activity_ms ?? 0) - (a.stream.last_activity_ms ?? 0);
  });
  return list;
}

/**
 * Identify Stream keys that currently have a live running agent terminal in Herdr,
 * from either `frames` or `streams.stream` labels on live terminals (local or remote).
 */
export function liveStreamIds({ panes = [], terminalList = [], agents = [], frames } = {}) {
  const live = new Set();
  const running = runningSessionTerminals({ terminalList, panes, agents });
  const termById = new Map();
  for (const term of terminalList ?? []) {
    const parsed = parseTerminalAddress(term);
    if (parsed) termById.set(parsed.address, term);
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
 * Count open live enrolled terminals per Stream key.
 */
export function countLiveTerminalsByStream({
  panes = [],
  terminalList = [],
  agents = [],
  frames,
} = {}) {
  const counts = new Map();
  const running = runningSessionTerminals({ terminalList, panes, agents });
  const termById = new Map();
  for (const term of terminalList ?? []) {
    const parsed = parseTerminalAddress(term);
    if (parsed) termById.set(parsed.address, term);
  }
  for (const [addr] of running) {
    const frame = frames?.get?.(addr);
    let streamKey =
      frame?.kind === "enrolled" && frame.session?.stream ? frame.session.stream : null;
    if (!streamKey) {
      const streamLabel = termById.get(addr)?.labels?.[LABEL_STREAM];
      if (typeof streamLabel === "string" && streamLabel.trim()) {
        streamKey = streamLabel.trim();
      }
    }
    if (!streamKey) continue;
    counts.set(streamKey, (counts.get(streamKey) ?? 0) + 1);
  }
  return counts;
}

/**
 * Find a live enrolled pane belonging to `streamKey`, preferring a Space's active Tab when
 * enrolled in `streamKey` and otherwise picking the first live enrolled pane in layout order.
 */
export function findLivePaneForStream({
  streamKey,
  spaces = [],
  panes = [],
  terminalList = [],
  agents = [],
  frames,
} = {}) {
  if (!streamKey) return null;
  const running = runningSessionTerminals({ terminalList, panes, agents });
  const termById = new Map();
  for (const term of terminalList ?? []) {
    const parsed = parseTerminalAddress(term);
    if (parsed) termById.set(parsed.address, term);
  }
  const matchingTerms = new Set();
  for (const [addr] of running) {
    const frame = frames?.get?.(addr);
    const frameStream = frame?.kind === "enrolled" ? frame.session?.stream : null;
    const labelStream = termById.get(addr)?.labels?.[LABEL_STREAM]?.trim() ?? null;
    if (frameStream === streamKey || labelStream === streamKey) {
      matchingTerms.add(addr);
    }
  }
  if (matchingTerms.size === 0) return null;

  const activeTabIds = new Set();
  for (const space of spaces ?? []) {
    const activeTab = space?.tabs?.find?.((t) => t.active);
    if (activeTab?.id) activeTabIds.add(activeTab.id);
  }

  let firstMatch = null;
  for (const pane of panes ?? []) {
    if (!pane?.terminal || !matchingTerms.has(pane.terminal)) continue;
    const candidate = { pane: pane.pane, terminal: pane.terminal, tab: pane.tab ?? null };
    if (!firstMatch) firstMatch = candidate;
    if (pane.tab && activeTabIds.has(pane.tab)) {
      return candidate;
    }
  }
  return firstMatch;
}

/**
 * Find a live enrolled pane currently running `sessionKey`.
 */
export function findLivePaneForSession({
  sessionKey,
  panes = [],
  terminalList = [],
  agents = [],
} = {}) {
  if (!sessionKey) return null;
  const running = runningSessionTerminals({ terminalList, panes, agents });
  for (const [addr, runningKey] of running) {
    if (runningKey !== sessionKey) continue;
    const pane = (panes ?? []).find((p) => p?.terminal === addr);
    if (pane) {
      return { pane: pane.pane, terminal: addr, tab: pane.tab ?? null };
    }
  }
  return null;
}

/**
 * Translate a Streams work status into a generic, Streams-agnostic `terminal.marks` entry.
 */
export function statusToTerminalMark(status) {
  switch (status) {
    case "active":
      return { symbol: "●", color: "ansi2", attention: true, label: "Active", source: "streams" };
    case "waiting":
      return { symbol: "●", color: "working", attention: true, label: "Waiting", source: "streams" };
    case "blocked":
      return { symbol: "●", color: "blocked", attention: true, label: "Blocked", source: "streams" };
    case "done":
      return { symbol: "✓", color: "done", attention: false, label: "Done", source: "streams" };
    case "parked":
      return { symbol: "○", color: "dim", attention: false, label: "Parked", source: "streams" };
    case "closed":
      return { symbol: "×", color: "dim", attention: false, label: "Closed", source: "streams" };
    default:
      return null;
  }
}

/**
 * Build the generic `terminal.marks` shared-state payload keyed by local terminal address,
 * keeping tab bar and sidebar consumers orthogonal to Streams.
 */
export function buildSharedTerminalMarks({ frames, running } = {}) {
  const result = {};
  if (!frames || !running) return result;
  const addrs = [...running.keys()].sort();
  for (const addr of addrs) {
    const frame = frames.get(addr);
    if (frame?.kind !== "enrolled" || !frame.session) continue;
    const mark = statusToTerminalMark(frame.session.status ?? "parked");
    if (mark) result[addr] = mark;
  }
  return result;
}

/**
 * Partition and sort a Stream's Sessions into three foldable status sections:
 * 1. Active & waiting (default open)
 * 2. Blocked & parked (default open)
 * 3. Done & closed (default closed)
 *
 * Within each section: current Tab's Session first, then live/open Sessions,
/**
 * Partition a Stream's Sessions by openness first, then archive:
 *  - `open`: every Session with a live open terminal (regardless of status, so finished-but-open
 *    tabs remain visible and easy to jump to or close)
 *  - `not_open`: unopened active, waiting, blocked, and parked Sessions
 *  - `done_closed`: unopened done and closed Sessions (collapsed by default)
 *
 * Within each group, rows sort by `recent` (most recent first, using `updated_at_ms` or CLI
 * recency order) or `alpha` (case-insensitive title A–Z).
 */
export function groupStreamSessions(
  sessions = [],
  { liveSessionKeys = new Set(), currentSessionKey = null, sort = "recent" } = {}
) {
  const buckets = {
    open: [],
    not_open: [],
    done_closed: [],
  };
  (sessions ?? []).forEach((sess, index) => {
    if (!sess) return;
    const isOpen = liveSessionKeys.has(sess.key);
    const status = sess.status ?? "active";
    let groupId = "not_open";
    if (isOpen) {
      groupId = "open";
    } else if (status === "done" || status === "closed") {
      groupId = "done_closed";
    }
    buckets[groupId].push({ sess, index });
  });

  const sortBucket = (entries) =>
    entries
      .sort((a, b) => {
        if (sort === "alpha") {
          const cmp = (a.sess.title ?? "").localeCompare(b.sess.title ?? "", undefined, {
            sensitivity: "base",
          });
          if (cmp !== 0) return cmp;
          return (a.sess.id ?? a.sess.key ?? "").localeCompare(b.sess.id ?? b.sess.key ?? "");
        }
        const aTs = a.sess.updated_at_ms ?? 0;
        const bTs = b.sess.updated_at_ms ?? 0;
        if (aTs !== bTs) return bTs - aTs;
        return a.index - b.index;
      })
      .map((entry) => entry.sess);

  return [
    {
      id: "open",
      label: "Open",
      defaultOpen: true,
      sessions: sortBucket(buckets.open),
    },
    {
      id: "not_open",
      label: "Not open",
      defaultOpen: true,
      sessions: sortBucket(buckets.not_open),
    },
    {
      id: "done_closed",
      label: "Done & closed",
      defaultOpen: false,
      sessions: sortBucket(buckets.done_closed),
    },
  ];
}

/**
 * Filter and partition host Streams into the same three openness-first sections as Stream
 * Sessions (`Open`, `Not open`, `Done & closed`), sorted by `recent` (default) or `alpha`.
 */
export function groupHostStreams(
  streams = [],
  { openCounts = new Map(), sort = "recent", query = "" } = {}
) {
  const q = typeof query === "string" ? query.trim().toLowerCase() : "";
  const buckets = {
    open: [],
    not_open: [],
    done_closed: [],
  };
  for (const summary of streams ?? []) {
    if (!summary) continue;
    if (q) {
      const title = (summary.title ?? "").toLowerCase();
      const goalShort = (summary.goal_short ?? "").toLowerCase();
      const id = (summary.id ?? "").toLowerCase();
      if (!title.includes(q) && !goalShort.includes(q) && !id.includes(q)) continue;
    }
    const openCount = openCounts?.get?.(summary.key) ?? 0;
    const status = summary.status ?? "active";
    let groupId = "not_open";
    if (openCount > 0) {
      groupId = "open";
    } else if (status === "done" || status === "closed") {
      groupId = "done_closed";
    }
    buckets[groupId].push(summary);
  }

  const sortBucket = (items) =>
    items.sort((a, b) => {
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

  return [
    {
      id: "open",
      label: "Open",
      defaultOpen: true,
      streams: sortBucket(buckets.open),
    },
    {
      id: "not_open",
      label: "Not open",
      defaultOpen: true,
      streams: sortBucket(buckets.not_open),
    },
    {
      id: "done_closed",
      label: "Done & closed",
      defaultOpen: false,
      streams: sortBucket(buckets.done_closed),
    },
  ];
}

/**
 * Format the secondary row metadata for an `All` view Stream card.
 * Keeps `goal_short` on the left, suppresses redundant zero counts (`none open`, `0 active`),
 * and omits counts on `done_closed` rows so only meaningful signals appear.
 */
export function formatHostStreamMeta({
  groupId = "not_open",
  openCount = 0,
  activeSessions = 0,
  goalShort = null,
  hostBadge = null,
} = {}) {
  const goalText = typeof goalShort === "string" ? goalShort.trim() : "";
  const openN = typeof openCount === "number" && openCount > 0 ? openCount : 0;
  const activeN =
    typeof activeSessions === "number" && activeSessions > 0 ? activeSessions : 0;
  let countText = "";
  if (groupId === "open" && openN > 0) {
    countText = `${openN} open`;
  } else if (groupId === "not_open" && activeN > 0) {
    countText = `${activeN} active`;
  }
  const badge = hostBadge && typeof hostBadge.label === "string" && hostBadge.label ? hostBadge : null;
  return {
    goalText,
    countText,
    hostBadge: badge,
    hasSecondLine: Boolean(goalText || countText || badge),
  };
}

/**
 * Compute convention-gated bulk actions for a format-5 Stream header menu.
 */
export function streamBulkActionState(stream) {
  const sessions = Array.isArray(stream?.sessions) ? stream.sessions : [];
  const activeSessions = sessions.filter((s) => s.status === "active");
  const openableActivePi = activeSessions.filter((s) => !s.harness || s.harness === "pi");
  const parkableSessions = sessions.filter(
    (s) => s.status === "active" || s.status === "waiting" || s.status === "blocked"
  );
  const closableSessions = sessions.filter(
    (s) => s.status !== "done" && s.status !== "closed"
  );
  return {
    activeSessions,
    parkableSessions,
    closableSessions,
    canOpenActive: openableActivePi.length > 0,
    canParkAll: parkableSessions.length > 0,
    canCloseAll: closableSessions.length > 0,
    canDelete: Boolean(stream) && sessions.length === 0,
  };
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
  if (rawProps?.kind === "session-popup") {
    return "popup";
  }
  if (rawProps?.pane || rawProps?.terminal) return "frame";
  return "sidebar";
}

/**
 * The active tab's terminals (local or remote) in pane order, deduplicated.
 */
export function tabTerminals(visibleTerminals = []) {
  const valid = (key) => Boolean(parseTerminalAddress(key));
  return [...new Set(visibleTerminals.filter(valid))];
}

/**
 * Extract the combined `streams.saved` token across all local and remote terminals.
 */
export function observeNoticeToken(terminalList, notificationKey = LABEL_SAVED) {
  if (!notificationKey || !Array.isArray(terminalList)) return "";
  const pairs = [];
  for (const term of terminalList) {
    const parsed = parseTerminalAddress(term);
    if (!parsed) continue;
    const token = term.labels?.[notificationKey];
    if (typeof token === "string" && token.length > 0) {
      pairs.push(`${parsed.address}:${token}`);
    }
  }
  pairs.sort();
  return pairs.join("|");
}

/**
 * Compute a deterministic context signature for automatic view refresh.
 * Includes `focusedTerminal`, machine readiness, and `hostScope` so switching machines,
 * reconnecting remotes, or toggling host scope triggers a refresh.
 */
export function computeObservationKey({
  activeSpace,
  activeTabId = null,
  visibleTerminals = [],
  focusedTerminal = null,
  terminalSessions = [],
  allRunningSessions = [],
  noticeToken = "",
  machineStates = [],
  hostScope = "local",
}) {
  if (!activeSpace) return "none";
  return JSON.stringify({
    space: activeSpace.id,
    machine: activeSpace.machine ?? null,
    tab: activeTabId,
    terminals: tabTerminals(visibleTerminals),
    focused: focusedTerminal ?? null,
    sessions: terminalSessions,
    running: allRunningSessions,
    notice: noticeToken ?? "",
    machines: machineStates,
    hostScope,
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
 * Resolve the Stream associated with a Space when the active tab itself is not enrolled.
 * Prefers the last-focused Stream in that Space if it still has a live enrolled terminal there,
 * falling back to the first live enrolled Stream in Space tab/pane layout order.
 */
export function resolveSpaceStreamKey({
  space,
  panes = [],
  terminalList = [],
  agents = [],
  frames,
  lastFocusedStreamBySpace,
} = {}) {
  if (!space || !Array.isArray(space.tabs) || space.tabs.length === 0) return null;
  const running = runningSessionTerminals({ terminalList, panes, agents });
  if (running.size === 0) return null;

  const termByAddr = new Map();
  for (const t of terminalList) {
    const parsed = parseTerminalAddress(t);
    if (parsed) termByAddr.set(parsed.address, t);
  }

  const panesByTab = new Map();
  for (const pane of panes) {
    const list = panesByTab.get(pane.tab) ?? [];
    list.push(pane);
    panesByTab.set(pane.tab, list);
  }

  const liveStreams = new Set();
  for (const tab of space.tabs) {
    const tabId = typeof tab === "string" ? tab : tab?.id;
    if (!tabId) continue;
    for (const pane of panesByTab.get(tabId) ?? []) {
      const rawTerm = pane.terminal ?? pane.content?.terminal;
      const addr = parseTerminalAddress(rawTerm)?.address ?? null;
      if (!addr || !running.has(addr)) continue;
      const fromFrame = frames?.get?.(addr)?.session?.stream ?? null;
      const fromLabel = termByAddr.get(addr)?.labels?.[LABEL_STREAM] ?? null;
      const streamKey = fromFrame || fromLabel;
      if (streamKey) liveStreams.add(streamKey);
    }
  }

  if (liveStreams.size === 0) return null;
  const lastFocused = lastFocusedStreamBySpace?.get?.(space.id) ?? null;
  if (lastFocused && liveStreams.has(lastFocused)) return lastFocused;
  return liveStreams.values().next().value ?? null;
}

/**
 * Resolve the three-view right panel state (`session`, `stream`, `all`, or `loading`),
 * applying sticky user intent (`preferredView`) with graceful fallback (`session -> stream -> all`)
 * while keeping detached preview overrides separate.
 */
export function resolveSidebarView({
  preferredView = "session",
  activeView = null,
  tabId,
  tabStreams,
  streams,
  previewSessions,
  frames,
  visibleTerminals = [],
  focusedTerminal = null,
  spaceStreamKey = null,
  previewStreamKey = null,
  previewSessionKey = null,
  inFlight = false,
}) {
  const desired = activeView ?? preferredView ?? "session";
  const currentStreamKey = tabStreams?.get?.(tabId) ?? "";
  const currentTabStream = currentStreamKey ? streams?.get?.(currentStreamKey) ?? null : null;
  const currentViewing = currentTabStream
    ? activeTabSession({
        visibleTerminals,
        focusedTerminal,
        frames,
        stream: currentTabStream.key,
      })
    : null;
  const isEnrolled = Boolean(currentTabStream && currentViewing);
  const spaceStream = !currentTabStream && spaceStreamKey ? streams?.get?.(spaceStreamKey) ?? null : null;
  const contextStream = currentTabStream ?? spaceStream;

  const hasSessionAccess = isEnrolled || Boolean(previewSessionKey);
  const hasStreamAccess = Boolean(contextStream) || Boolean(spaceStreamKey) || Boolean(previewStreamKey);

  if (
    !tabStreams?.has?.(tabId) &&
    inFlight &&
    !previewStreamKey &&
    !previewSessionKey &&
    desired !== "all"
  ) {
    return {
      view: "loading",
      preferredView: desired,
      hasSessionAccess: false,
      hasStreamAccess,
      isEnrolled: false,
      currentStream: contextStream,
      currentSession: null,
    };
  }

  let effectiveView = "all";
  if (desired === "session") {
    if (hasSessionAccess) effectiveView = "session";
    else if (hasStreamAccess) effectiveView = "stream";
    else effectiveView = "all";
  } else if (desired === "stream") {
    if (hasStreamAccess) effectiveView = "stream";
    else effectiveView = "all";
  } else {
    effectiveView = "all";
  }

  if (effectiveView === "all") {
    return {
      view: "all",
      preferredView: desired,
      hasSessionAccess,
      hasStreamAccess,
      isEnrolled,
      currentStream: contextStream,
      currentSession: currentViewing?.session ?? null,
    };
  }

  if (effectiveView === "stream") {
    const stream = (previewStreamKey ? streams?.get?.(previewStreamKey) : null) ?? contextStream;
    const isPreview = Boolean(
      previewStreamKey && (!contextStream || previewStreamKey !== contextStream.key)
    );
    return {
      view: "stream",
      preferredView: desired,
      hasSessionAccess,
      hasStreamAccess,
      stream,
      isPreview,
      canReturnToCurrent: isPreview && Boolean(contextStream),
      isEnrolled,
      currentStream: contextStream,
      currentSession: currentViewing?.session ?? null,
    };
  }

  // effectiveView === "session"
  if (previewSessionKey) {
    const session = previewSessions?.get?.(previewSessionKey) ?? null;
    const parentStreamKey = previewStreamKey ?? session?.stream ?? null;
    const stream = parentStreamKey ? streams?.get?.(parentStreamKey) ?? null : null;
    return {
      view: "session",
      preferredView: desired,
      hasSessionAccess,
      hasStreamAccess,
      session,
      stream,
      terminal: null,
      isPreview: true,
      isUnopened: true,
      canReturnToCurrent: Boolean(contextStream),
      isEnrolled,
      currentStream: contextStream,
      currentSession: currentViewing?.session ?? null,
    };
  }

  if (currentViewing) {
    return {
      view: "session",
      preferredView: desired,
      hasSessionAccess: true,
      hasStreamAccess: true,
      session: currentViewing.session,
      stream: currentTabStream,
      terminal: currentViewing.terminal,
      isPreview: false,
      isUnopened: false,
      canReturnToCurrent: false,
      isEnrolled: true,
      currentStream: contextStream,
      currentSession: currentViewing.session,
    };
  }

  return {
    view: "all",
    preferredView: desired,
    hasSessionAccess: false,
    hasStreamAccess,
    isEnrolled: false,
    currentStream: contextStream,
    currentSession: null,
  };
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
        docState &&
        typeof docState === "object" &&
        (docState.status === "unavailable" || (docState.reason && !docState.path))
          ? docState.reason ?? "document unavailable"
          : null;
      const resolvedPath =
        docState && typeof docState === "object" && typeof docState.path === "string"
          ? docState.path
          : refKey;
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
        sessionKey: session.key,
        streamKey: session.stream ?? null,
        refKey,
        copyText: resolvedPath,
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
        sessionKey: session.key,
        streamKey: session.stream ?? null,
        refKey,
        copyText: refKey,
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
      sessionKey: session.key,
      refKey: null,
      copyText: artifactKey,
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

export function extractCliError(out, fallback) {
  for (const raw of [out?.stdout, out?.stderr]) {
    const trimmed = typeof raw === "string" ? raw.trim() : "";
    if (!trimmed) continue;
    const parsed = parseJsonSafe(trimmed);
    if (parsed?.error?.message) {
      return {
        kind: parsed.error.kind ?? "error",
        message: parsed.error.message,
        hint: typeof parsed.error.hint === "string" ? parsed.error.hint : null,
      };
    }
  }
  const msg = out?.stderr?.trim() || out?.stdout?.trim() || fallback;
  return { kind: "error", message: msg, hint: null };
}

function shortenHomePath(pathStr, homeDir) {
  if (!pathStr || !homeDir) return pathStr;
  if (pathStr === homeDir) return "~";
  return pathStr.startsWith(`${homeDir}/`) ? `~${pathStr.slice(homeDir.length)}` : pathStr;
}

export function formatMissingDirectoryError({ session, cwd, homeDir = null }) {
  const label = session?.title || session?.key || "Session";
  const displayPath = shortenHomePath(cwd, homeDir);
  const repoUrl = session?.directory?.repository?.url ?? null;
  const repoBranch = session?.directory?.repository?.branch ?? null;
  const repairCmd = `streams session edit ${session.key} --directory <path>`;
  const lines = [`${displayPath} no longer exists.`];
  if (repoUrl) {
    const branchSuffix = repoBranch ? ` (${repoBranch})` : "";
    lines.push(
      `Recreate the worktree from ${repoUrl}${branchSuffix}, or point this Session elsewhere:`
    );
  } else {
    lines.push("Recreate the folder, or point this Session elsewhere:");
  }
  lines.push(repairCmd);
  return {
    title: `Folder missing for "${label}"`,
    message: lines.join("\n"),
    copyText: repairCmd,
    copyLabel: "Copy command",
  };
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
    publishShared = null,
    notifyToast = null,
    homeDir = null,
    streamsBin = "streams",
    hcBin = "hc",
    staleFallbackMs = STALE_FALLBACK_MS,
    newId = () => `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
  } = {}) {
    this.exec = exec;
    this.notify = notify;
    this.publishShared = publishShared;
    this.notifyToast = notifyToast;
    this.homeDir = homeDir;
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
    this.lastFocusedStreamBySpace = new Map(); // spaceId -> streamKey
    this.activeMachine = null; // null for local desktop, or remote machine name
    this.machineLayouts = new Map(); // machineName -> "loading" | "ready" | "unavailable"
    this._hasMachineSnapshot = false;
    this.hostScope = "local"; // "local" (this host) | "all" (all synced hosts)
    this.hostStreamsByScope = new Map(); // `${machineKey}:${scope}` -> { items, unreadable, error, loaded, seq }
    this.hostToMachine = new Map(); // hostId -> machineName | null
    this.machineToHost = new Map(); // machineKey -> hostId
    this._localOwnerChecked = new Set(); // machineKeys whose `--local` Streams were already checked
    this._localOwnerProbes = new Map(); // machineKey -> in-flight probe Promise
    this.homeDirByMachine = new Map(); // machineName -> remote $HOME
    this._homeDirPromises = new Map(); // machineName -> Promise<string | null>
    this.preferredView = "session"; // "session" | "stream" | "all"
    this.inspectedStreamId = null; // alias for previewStreamKey
    this.previewStreamKey = null; // streamKey
    this.previewSessionKey = null; // sessionKey
    this.previewSessions = new Map(); // sessionKey -> normalized Session
    this.frames = new Map(); // terminalKey -> { kind: "enrolled", session }
    this.pendingViewers = [];
    this.viewerMetaByPane = new Map();
    this.readError = null;
    this.actionError = null;

    this.lastNoticeToken = "";
    this.lastSpaceTabKey = null;
    this._lastPublishedSharedJson = null;
    this._lastRunning = new Map();
    this.pendingStatusSessions = new Set();
    this.pendingResumeSessions = new Set();
    this.pendingStreamActions = new Map();
    this.pendingDropReferences = new Set();
    this.pendingRenames = new Set();
    this.pendingMoves = new Set();
  }

  _machineKey(machine = this.activeMachine) {
    return machine ?? "__local__";
  }

  _hostStreamsBucketKey(machine = this.activeMachine, scope = this.hostScope) {
    const normalizedScope = scope === "all" ? "all" : "local";
    return `${this._machineKey(machine)}:${normalizedScope}`;
  }

  _getHostStreamsEntry(machine = this.activeMachine, scope = this.hostScope) {
    const key = this._hostStreamsBucketKey(machine, scope);
    let entry = this.hostStreamsByScope.get(key);
    if (!entry) {
      entry = { items: [], unreadable: [], error: null, loaded: false, seq: 0 };
      this.hostStreamsByScope.set(key, entry);
    }
    return entry;
  }

  hostStreamsState(machine = this.activeMachine, scope = this.hostScope) {
    const entry = this._getHostStreamsEntry(machine, scope);
    return {
      items: entry.items,
      unreadable: entry.unreadable,
      error: entry.error,
      loaded: entry.loaded,
    };
  }

  async _withPendingSet(set, key, fn) {
    if (set.has(key)) return { ok: false, duplicate: true };
    set.add(key);
    this.notifyViews();
    try {
      return await fn();
    } finally {
      set.delete(key);
      this.notifyViews();
    }
  }

  async _withPendingMap(map, key, label, fn) {
    if (map.has(key)) return { ok: false, duplicate: true };
    map.set(key, label);
    this.notifyViews();
    try {
      return await fn();
    } finally {
      map.delete(key);
      this.notifyViews();
    }
  }

  get hostStreams() {
    return this._getHostStreamsEntry().items;
  }

  set hostStreams(value) {
    this._getHostStreamsEntry().items = Array.isArray(value) ? value : [];
  }

  get hostUnreadable() {
    return this._getHostStreamsEntry().unreadable;
  }

  set hostUnreadable(value) {
    this._getHostStreamsEntry().unreadable = Array.isArray(value) ? value : [];
  }

  get hostStreamsError() {
    return this._getHostStreamsEntry().error;
  }

  set hostStreamsError(value) {
    this._getHostStreamsEntry().error = value ?? null;
  }

  get hostStreamsLoaded() {
    return this._getHostStreamsEntry().loaded;
  }

  set hostStreamsLoaded(value) {
    this._getHostStreamsEntry().loaded = Boolean(value);
  }

  isMachineReady(machine = null) {
    if (!machine) return true;
    if (!this._hasMachineSnapshot) return true;
    return this.machineLayouts.get(machine) === "ready";
  }

  machineLayout(machine = null) {
    if (!machine) return "ready";
    if (!this._hasMachineSnapshot) return "ready";
    return this.machineLayouts.get(machine) ?? "unavailable";
  }

  streamsBinFor(machine = null) {
    if (!machine) return this.streamsBin;
    if (
      typeof this.streamsBin === "string" &&
      this.streamsBin.length > 0 &&
      !this.streamsBin.includes("/")
    ) {
      return this.streamsBin;
    }
    return "streams";
  }

  recordHostMachine(hostId, machine = null) {
    if (typeof hostId !== "string" || hostId.length === 0) return;
    const targetMachine = machine ?? null;
    const mKey = this._machineKey(targetMachine);
    this.hostToMachine.set(hostId, targetMachine);
    this.machineToHost.set(mKey, hostId);
    this._localOwnerChecked.add(mKey);
  }

  machineForHost(hostId) {
    if (!hostId || !this.hostToMachine.has(hostId)) return undefined;
    return this.hostToMachine.get(hostId);
  }

  async _probeLocalOwner(machine = this.activeMachine, { force = false } = {}) {
    const targetMachine = machine ?? null;
    const mKey = this._machineKey(targetMachine);
    if (
      this.machineToHost.has(mKey) ||
      (!force && this._localOwnerChecked.has(mKey)) ||
      !this.isMachineReady(targetMachine)
    ) {
      return { ok: true };
    }
    if (this._localOwnerProbes.has(mKey)) {
      return this._localOwnerProbes.get(mKey);
    }
    const promise = (async () => {
      try {
        const probe = await this.runExec(
          this.streamsBinFor(targetMachine),
          ["stream", "list", "--local", "--all", "--limit", "1", "--fields", "key", "--json"],
          { on: targetMachine }
        );
        if (probe.code === 0) {
          const hadHost = this.machineToHost.has(mKey);
          this._localOwnerChecked.add(mKey);
          const parsed = parseJsonSafe(probe.stdout);
          for (const item of parsed?.items ?? []) {
            const foundHost = item?.host_id ?? recordHostId(item?.key);
            if (foundHost) this.recordHostMachine(foundHost, targetMachine);
          }
          if (!hadHost && this.machineToHost.has(mKey)) {
            this.notifyViews();
          }
          return { ok: true };
        }
        if (probe.transportError) {
          return {
            ok: false,
            error: extractCliError(probe, "could not reach active host").message,
          };
        }
        return { ok: true };
      } finally {
        this._localOwnerProbes.delete(mKey);
      }
    })();
    this._localOwnerProbes.set(mKey, promise);
    return promise;
  }

  /**
   * Return a foreign-host pill descriptor (`{ label, state, tooltip }`) when a record belongs
   * to a different host than `activeMachine`, or `null` when it is local to `activeMachine`
   * (or the active machine's host ID has not yet been discovered).
   *
   * `state` is one of `"online" | "loading" | "offline" | "unmapped"`.
   */
  foreignHostBadge(recordOrKey, activeMachine = this.activeMachine) {
    const hostId =
      typeof recordOrKey === "string"
        ? recordHostId(recordOrKey)
        : recordOrKey?.host_id || recordHostId(recordOrKey?.key);
    if (!hostId) return null;
    const targetMachine = activeMachine ?? null;
    const mKey = this._machineKey(targetMachine);
    const activeHostId = this.machineToHost.get(mKey) ?? null;

    if (this.hostToMachine.has(hostId)) {
      const owner = this.hostToMachine.get(hostId) ?? null;
      if (owner === targetMachine) return null;
      const label = owner ?? "local";
      if (this.isMachineReady(owner)) {
        return {
          label,
          state: "online",
          tooltip: `On ${label}`,
        };
      }
      if (this.machineLayout(owner) === "loading") {
        return {
          label,
          state: "loading",
          tooltip: `Connecting to ${label} — read-only until ready`,
        };
      }
      return {
        label,
        state: "offline",
        tooltip: `${label} is offline — read-only (writes and resume unavailable)`,
      };
    }

    if (activeHostId || this._localOwnerChecked.has(mKey)) {
      const short = hostId.slice(0, 8);
      return {
        label: short,
        state: "unmapped",
        tooltip: `Host ${short} is not linked yet — visit its space or open a terminal there (read-only)`,
      };
    }

    return null;
  }

  resolveReadMachineForKey(key) {
    const hostId = recordHostId(key);
    if (hostId && this.hostToMachine.has(hostId)) {
      const owner = this.hostToMachine.get(hostId);
      if (this.isMachineReady(owner)) {
        return { machine: owner, owned: true, unavailable: null };
      }
    }
    if (this.isMachineReady(this.activeMachine)) {
      return { machine: this.activeMachine, owned: false, unavailable: null };
    }
    return {
      machine: this.activeMachine,
      owned: false,
      unavailable: this.activeMachine,
    };
  }

  resolveWriteMachineForKey(key) {
    const hostId = recordHostId(key);
    if (!hostId) {
      return { ok: false, error: "record key is missing a host identifier" };
    }
    if (this.hostToMachine.has(hostId)) {
      const owner = this.hostToMachine.get(hostId);
      if (!this.isMachineReady(owner)) {
        return {
          ok: false,
          error: `${owner} is offline — cannot run this action on ${owner}.`,
        };
      }
      return { ok: true, machine: owner };
    }
    return {
      ok: false,
      error: `Host ${hostId.slice(0, 8)} is not linked to an online machine — switch to that machine's space or open a terminal on it.`,
    };
  }

  /**
   * Resolve the owning machine for an owner-only action (writes, resume, document read).
   * Requires an explicit `hostId -> machine` mapping. If the active machine has not yet been
   * checked for local Streams (for example, arriving at a space with `All hosts` already active),
   * performs one bounded `stream list --local --limit 1` check on `activeMachine` before refusing.
   */
  async ensureWriteMachineForKey(key) {
    const hostId = recordHostId(key);
    if (!hostId) {
      return { ok: false, error: "record key is missing a host identifier" };
    }
    if (this.hostToMachine.has(hostId)) {
      return this.resolveWriteMachineForKey(key);
    }
    const activeMachine = this.activeMachine ?? null;
    const probeRes = await this._probeLocalOwner(activeMachine);
    if (!probeRes.ok) {
      return probeRes;
    }
    return this.resolveWriteMachineForKey(key);
  }

  async runExec(bin, args, { on = null } = {}) {
    try {
      const out = on
        ? await this.exec(bin, args, { on })
        : await this.exec(bin, args);
      if (!out || typeof out !== "object") {
        return { code: -1, stdout: "", stderr: "empty command response", transportError: true };
      }
      return {
        code: typeof out.code === "number" ? out.code : -1,
        stdout: typeof out.stdout === "string" ? out.stdout : "",
        stderr: typeof out.stderr === "string" ? out.stderr : "",
        transportError: Boolean(out.transportError),
      };
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      return { code: -1, stdout: "", stderr: msg, transportError: true };
    }
  }

  homeDirFor(machine = null) {
    if (!machine) return this.homeDir;
    return this.homeDirByMachine.get(machine) ?? null;
  }

  async ensureHomeDir(machine) {
    if (!machine) return this.homeDir;
    if (this.homeDirByMachine.has(machine)) {
      return this.homeDirByMachine.get(machine);
    }
    if (!this.isMachineReady(machine)) return null;
    if (this._homeDirPromises.has(machine)) {
      return this._homeDirPromises.get(machine);
    }
    const promise = (async () => {
      try {
        const out = await this.runExec("sh", ["-c", 'printf %s "$HOME"'], { on: machine });
        if (out.code === 0) {
          const resolved = out.stdout.trim();
          if (resolved.length > 0) {
            this.homeDirByMachine.set(machine, resolved);
            this.notifyViews();
            return resolved;
          }
        }
        return null;
      } finally {
        this._homeDirPromises.delete(machine);
      }
    })();
    this._homeDirPromises.set(machine, promise);
    return promise;
  }

  get lastError() {
    return this.actionError?.message ?? this.readError ?? null;
  }

  set lastError(value) {
    if (!value) {
      this.actionError = null;
      this.readError = null;
      return;
    }
    this.actionError = { title: "Streams error", message: String(value), at: Date.now() };
  }

  reportActionError(title, message, { level = "error", copyText = null, copyLabel = null } = {}) {
    const cleanTitle = title || "Streams error";
    const cleanMessage = message || "Operation failed";
    this.actionError = {
      title: cleanTitle,
      message: cleanMessage,
      level,
      copyText,
      copyLabel,
      at: Date.now(),
    };
    try {
      this.notifyToast?.({ title: cleanTitle, body: cleanMessage, level });
    } catch {
      // Non-fatal if runtime rejects notification outside active window
    }
    this.notifyViews();
  }

  clearActionError() {
    this.actionError = null;
    this.notifyViews();
  }

  clearReadError() {
    this.readError = null;
    this.notifyViews();
  }

  /** Read `streams config get herdr --json` once from the local desktop running herdr-app. */
  async ensureConfig() {
    if (this.configLoaded) return this.herdrConfig;
    if (this.configPromise) return this.configPromise;
    this.configPromise = (async () => {
      try {
        const out = await this.runExec(this.streamsBinFor(null), ["config", "get", "herdr", "--json"]);
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

  publishSharedTerminals(running = this._lastRunning) {
    if (!this.publishShared) return;
    const payload = buildSharedTerminalMarks({
      frames: this.frames,
      running,
    });
    const json = JSON.stringify(payload);
    if (json === this._lastPublishedSharedJson) return;
    this._lastPublishedSharedJson = json;
    try {
      this.publishShared(SHARED_TERMINAL_MARKS_KEY, payload);
    } catch {
      // Non-fatal if runtime rejects shared write outside frame
    }
  }

  /**
   * Fetch host Streams for a specific `(machine, scope)` bucket.
   * Captures the target bucket and a per-bucket sequence before `await` so delayed responses
   * never overwrite another machine/scope bucket or a newer request for the same bucket.
   */
  async _fetchHostStreams({
    machine = this.activeMachine,
    scope = this.hostScope,
    force = false,
    forceProbe = force,
  } = {}) {
    const targetMachine = machine ?? null;
    const targetScope = scope === "all" ? "all" : "local";
    const bucket = this._getHostStreamsEntry(targetMachine, targetScope);
    if (bucket.loaded && !force) {
      return { ok: true, items: bucket.items, bucket };
    }
    const reqSeq = ++bucket.seq;
    if (!this.isMachineReady(targetMachine)) {
      const msg = `${targetMachine} is unavailable`;
      if (bucket.seq === reqSeq) {
        bucket.error = msg;
        this.notifyViews();
      }
      return { ok: false, error: msg, bucket };
    }

    const args = ["stream", "list"];
    if (targetScope === "local") args.push("--local");
    args.push("--all", "--json", "--fields", HOST_STREAM_LIST_FIELDS);

    const mKey = this._machineKey(targetMachine);
    const listPromise = this.runExec(this.streamsBinFor(targetMachine), args, {
      on: targetMachine,
    });
    if (
      targetScope === "all" &&
      !this.machineToHost.has(mKey) &&
      (forceProbe || !this._localOwnerChecked.has(mKey))
    ) {
      // Run ownership discovery in the background so a slow probe never blocks
      // rendering the All hosts list behind the sidebar loading guard.
      void this._probeLocalOwner(targetMachine, { force: forceProbe });
    }
    const listOut = await listPromise;
    if (bucket.seq !== reqSeq) {
      return { ok: false, stale: true, bucket };
    }
    if (listOut.code !== 0) {
      const err = extractCliError(listOut, "host Streams are unavailable");
      bucket.items = [];
      bucket.unreadable = [];
      bucket.error = err.message;
      this.notifyViews();
      return { ok: false, error: err.message, bucket };
    }
    const parsed = parseJsonSafe(listOut.stdout);
    if (!parsed || !Array.isArray(parsed.items)) {
      bucket.items = [];
      bucket.unreadable = [];
      bucket.error = "unexpected stream list response from streams";
      this.notifyViews();
      return { ok: false, error: bucket.error, bucket };
    }
    const items = parsed.items.map(normalizeStream).filter(Boolean);
    if (targetScope === "local") {
      this._localOwnerChecked.add(this._machineKey(targetMachine));
      for (const item of items) {
        if (item.host_id) this.recordHostMachine(item.host_id, targetMachine);
      }
    }
    bucket.items = items;
    bucket.unreadable = Array.isArray(parsed.unreadable) ? parsed.unreadable : [];
    bucket.error = null;
    bucket.loaded = true;
    this.publishSharedTerminals();
    this.notifyViews();
    return { ok: true, items: bucket.items, bucket };
  }

  async ensureHostStreams(
    force = false,
    { machine = this.activeMachine, scope = this.hostScope } = {}
  ) {
    const res = await this._fetchHostStreams({ machine, scope, force });
    return res.ok ? res.items : null;
  }

  setHostScope(scope) {
    const normalized = scope === "all" ? "all" : "local";
    if (this.hostScope === normalized) {
      return this.ensureHostStreams(false);
    }
    this.hostScope = normalized;
    this.notifyViews();
    return this.ensureHostStreams(false);
  }

  setView(view) {
    this.preferredView = view;
    if (view === "all") {
      this.previewStreamKey = null;
      this.inspectedStreamId = null;
      this.previewSessionKey = null;
      this.notifyViews();
      return this.ensureHostStreams(false);
    }
    if (view === "session") {
      this.previewStreamKey = null;
      this.inspectedStreamId = null;
      this.previewSessionKey = null;
    } else if (view === "stream") {
      this.previewSessionKey = null;
    }
    this.notifyViews();
    return Promise.resolve(null);
  }

  returnToCurrent({ isEnrolled = true } = {}) {
    this.previewStreamKey = null;
    this.inspectedStreamId = null;
    this.previewSessionKey = null;
    this.preferredView = isEnrolled ? "session" : "stream";
    this.notifyViews();
  }

  backToStream() {
    this.previewSessionKey = null;
    this.preferredView = "stream";
    this.notifyViews();
  }

  async inspectStream(streamKey) {
    if (!streamKey) {
      this.inspectedStreamId = null;
      this.previewStreamKey = null;
      this.previewSessionKey = null;
      this.preferredView = "all";
      this.notifyViews();
      return null;
    }
    this.inspectedStreamId = streamKey;
    this.previewStreamKey = streamKey;
    this.previewSessionKey = null;
    this.preferredView = "stream";
    this.notifyViews();
    const target = this.resolveReadMachineForKey(streamKey);
    if (target.unavailable) {
      this.reportActionError(
        "Could not load Stream",
        `${target.unavailable} is offline`
      );
      return null;
    }
    const out = await this.runExec(
      this.streamsBinFor(target.machine),
      ["stream", "show", streamKey, "--json"],
      { on: target.machine }
    );
    if (out.code !== 0) {
      this.reportActionError(
        "Could not load Stream",
        extractCliError(out, "saved Stream view is unavailable").message
      );
      return null;
    }
    const raw = parseJsonSafe(out.stdout);
    const stream = normalizeStream(raw);
    if (stream) {
      this.streams.set(stream.key, stream);
      this.actionError = null;
    }
    this.notifyViews();
    return stream;
  }

  async previewSession(sessionKey, streamKey = null) {
    if (!sessionKey) {
      this.previewSessionKey = null;
      this.notifyViews();
      return null;
    }
    this.previewSessionKey = sessionKey;
    if (streamKey) {
      this.previewStreamKey = streamKey;
      this.inspectedStreamId = streamKey;
    }
    this.preferredView = "session";
    this.notifyViews();
    const target = this.resolveReadMachineForKey(sessionKey);
    if (target.unavailable) {
      this.reportActionError(
        "Could not load Session",
        `${target.unavailable} is offline`
      );
      return null;
    }
    const out = await this.runExec(
      this.streamsBinFor(target.machine),
      ["session", "show", sessionKey, "--json"],
      { on: target.machine }
    );
    if (out.code !== 0) {
      this.reportActionError(
        "Could not load Session",
        extractCliError(out, "saved Session view is unavailable").message
      );
      return null;
    }
    const session = normalizeSession(parseJsonSafe(out.stdout));
    if (session) {
      this.previewSessions.set(session.key, session);
      if (!this.previewStreamKey && session.stream) {
        this.previewStreamKey = session.stream;
        this.inspectedStreamId = session.stream;
      }
      this.actionError = null;
    }
    this.notifyViews();
    return session;
  }

  async selectStreamFromAll(
    streamKey,
    { spaces = [], panes = [], terminalList = [], agents = [] } = {},
    { run } = {}
  ) {
    if (!streamKey) return null;
    const live = findLivePaneForStream({
      streamKey,
      spaces,
      panes,
      terminalList,
      agents,
      frames: this.frames,
    });
    if (live) {
      this.preferredView = "stream";
      this.previewStreamKey = null;
      this.inspectedStreamId = null;
      this.previewSessionKey = null;
      run?.("pane.focus", { pane: live.pane });
      this.notifyViews();
      return { focused: true, pane: live.pane, terminal: live.terminal };
    }
    const stream = await this.inspectStream(streamKey);
    return { focused: false, stream };
  }

  async selectSessionFromStream(
    sessionKey,
    streamKey,
    { panes = [], terminalList = [], agents = [] } = {},
    { run } = {}
  ) {
    if (!sessionKey) return null;
    const live = findLivePaneForSession({
      sessionKey,
      panes,
      terminalList,
      agents,
    });
    if (live) {
      this.preferredView = "session";
      this.previewStreamKey = null;
      this.inspectedStreamId = null;
      this.previewSessionKey = null;
      run?.("pane.focus", { pane: live.pane });
      this.notifyViews();
      return { focused: true, pane: live.pane, terminal: live.terminal };
    }
    const session = await this.previewSession(sessionKey, streamKey);
    return { focused: false, session };
  }

  /**
   * Targeted, authoritative post-mutation refresh: advances `readGeneration` so any slower
   * pre-mutation background observation is discarded, re-reads only the affected Session and/or
   * Stream(s), updates every in-memory cache (`frames`, `previewSessions`, `streams`,
   * `hostStreams`), republishes `terminal.marks`, and notifies views.
   */
  async refreshAfterMutation({
    sessionKey = null,
    streamKey = null,
    extraStreamKeys = [],
    machine = undefined,
  } = {}) {
    this.readGeneration += 1;
    for (const bucket of this.hostStreamsByScope.values()) {
      bucket.seq += 1;
    }

    let resolvedStreamKey = streamKey ?? null;
    const readTarget =
      machine !== undefined
        ? { machine, unavailable: this.isMachineReady(machine) ? null : machine }
        : this.resolveReadMachineForKey(sessionKey ?? streamKey);
    const targetMachine = readTarget.unavailable ? null : readTarget.machine;

    if (sessionKey && !readTarget.unavailable) {
      const sessOut = await this.runExec(
        this.streamsBinFor(targetMachine),
        ["session", "show", sessionKey, "--json"],
        { on: targetMachine }
      );
      if (sessOut.code === 0) {
        const freshSession = normalizeSession(parseJsonSafe(sessOut.stdout));
        if (freshSession) {
          if (!resolvedStreamKey && freshSession.stream) {
            resolvedStreamKey = freshSession.stream;
          }
          for (const [term, frame] of this.frames.entries()) {
            if (frame?.kind === "enrolled" && frame.session?.key === freshSession.key) {
              this.frames.set(term, { kind: "enrolled", session: freshSession });
            }
          }
          if (
            this.previewSessions.has(freshSession.key) ||
            this.previewSessionKey === freshSession.key
          ) {
            this.previewSessions.set(freshSession.key, freshSession);
          }
        }
      }
    }

    const streamKeysToRefresh = [];
    if (resolvedStreamKey) streamKeysToRefresh.push(resolvedStreamKey);
    for (const k of extraStreamKeys ?? []) {
      if (k && !streamKeysToRefresh.includes(k)) streamKeysToRefresh.push(k);
    }

    for (const stKey of streamKeysToRefresh) {
      const streamTarget =
        machine !== undefined ? readTarget : this.resolveReadMachineForKey(stKey);
      const streamMachine = streamTarget.unavailable ? null : streamTarget.machine;
      if (streamTarget.unavailable) continue;
      const stOut = await this.runExec(
        this.streamsBinFor(streamMachine),
        ["stream", "show", stKey, "--json"],
        { on: streamMachine }
      );
      if (stOut.code !== 0) continue;
      const freshStream = normalizeStream(parseJsonSafe(stOut.stdout));
      if (!freshStream) continue;
      this.streams.set(freshStream.key, freshStream);

      const memberByKey = new Map();
      for (const member of freshStream.sessions ?? []) {
        memberByKey.set(member.key, member);
      }

      // Sync any live frames or cached preview Sessions whose status/title/state changed in bulk.
      for (const [term, frame] of this.frames.entries()) {
        if (frame?.kind !== "enrolled" || !frame.session) continue;
        if (sessionKey && frame.session.key === sessionKey) continue;
        const updatedMember = memberByKey.get(frame.session.key);
        if (updatedMember) {
          this.frames.set(term, {
            kind: "enrolled",
            session: {
              ...frame.session,
              title: updatedMember.title ?? frame.session.title,
              status: updatedMember.status ?? frame.session.status,
              state: updatedMember.state ?? frame.session.state,
            },
          });
        }
      }
      for (const [key, cached] of this.previewSessions.entries()) {
        if (sessionKey && key === sessionKey) continue;
        const updatedMember = memberByKey.get(key);
        if (updatedMember) {
          this.previewSessions.set(key, {
            ...cached,
            title: updatedMember.title ?? cached.title,
            status: updatedMember.status ?? cached.status,
            state: updatedMember.state ?? cached.state,
          });
        }
      }

      // Patch matching summary entries across every cached hostStreams bucket so `All` stays consistent.
      for (const bucket of this.hostStreamsByScope.values()) {
        if (!Array.isArray(bucket.items) || bucket.items.length === 0) continue;
        const idx = bucket.items.findIndex((s) => s.key === freshStream.key);
        if (idx >= 0) {
          const prev = bucket.items[idx];
          bucket.items[idx] = {
            ...prev,
            title: freshStream.title ?? prev.title,
            goal: freshStream.goal ?? prev.goal,
            goal_short: freshStream.goal_short ?? prev.goal_short,
            status: freshStream.status ?? prev.status,
            active_sessions: freshStream.active_sessions,
            total_sessions: freshStream.total_sessions,
            last_activity_ms: freshStream.last_activity_ms ?? prev.last_activity_ms,
          };
        }
      }
    }

    this.publishSharedTerminals();
    this.notifyViews();
  }

  async dropReference(sessionKey, refKey, streamKey = null) {
    if (!sessionKey || !refKey) return { ok: false, error: "missing session or reference key" };
    const pendingKey = `${sessionKey}\0${refKey}`;
    return this._withPendingSet(this.pendingDropReferences, pendingKey, async () => {
      const target = await this.ensureWriteMachineForKey(sessionKey);
      if (!target.ok) {
        this.reportActionError("Could not drop reference", target.error);
        return { ok: false, error: target.error };
      }
      const out = await this.runExec(
        this.streamsBinFor(target.machine),
        ["session", "edit", sessionKey, "--drop-reference", refKey],
        { on: target.machine }
      );
      if (out.code !== 0) {
        const err = extractCliError(out, "could not drop reference");
        this.reportActionError("Could not drop reference", err.message);
        return { ok: false, error: err.message };
      }
      this.actionError = null;
      this.lastCompletedKey = null;
      await this.refreshAfterMutation({ sessionKey, streamKey, machine: target.machine });
      return { ok: true };
    });
  }

  async setSessionStatus(sessionKey, status, streamKey = null) {
    if (!sessionKey || !SESSION_STATUSES.includes(status)) {
      const msg = `invalid status: ${status}`;
      this.reportActionError("Could not update status", msg);
      return { ok: false, error: msg };
    }
    return this._withPendingSet(this.pendingStatusSessions, sessionKey, async () => {
      const target = await this.ensureWriteMachineForKey(sessionKey);
      if (!target.ok) {
        this.reportActionError("Could not update status", target.error);
        return { ok: false, error: target.error };
      }
      const out = await this.runExec(
        this.streamsBinFor(target.machine),
        ["session", "edit", sessionKey, "--status", status],
        { on: target.machine }
      );
      if (out.code !== 0) {
        const err = extractCliError(out, "could not update Session status");
        this.reportActionError("Could not update status", err.message);
        return { ok: false, error: err.message };
      }
      this.actionError = null;
      this.lastCompletedKey = null;
      await this.refreshAfterMutation({ sessionKey, streamKey, machine: target.machine });
      return { ok: true };
    });
  }

  async renameSession(sessionKey, newTitle, streamKey = null) {
    const trimmed = typeof newTitle === "string" ? newTitle.trim() : "";
    if (!sessionKey || !trimmed) {
      const msg = !sessionKey ? "missing Session key" : "Title cannot be empty.";
      this.reportActionError("Could not rename Session", msg);
      return { ok: false, error: msg };
    }
    return this._withPendingSet(this.pendingRenames, sessionKey, async () => {
      const target = await this.ensureWriteMachineForKey(sessionKey);
      if (!target.ok) {
        this.reportActionError("Could not rename Session", target.error);
        return { ok: false, error: target.error };
      }
      const out = await this.runExec(
        this.streamsBinFor(target.machine),
        ["session", "edit", sessionKey, "--title", trimmed],
        { on: target.machine }
      );
      if (out.code !== 0) {
        const err = extractCliError(out, "could not rename Session");
        this.reportActionError("Could not rename Session", err.message);
        return { ok: false, error: err.message };
      }
      this.actionError = null;
      this.lastCompletedKey = null;
      await this.refreshAfterMutation({ sessionKey, streamKey, machine: target.machine });
      return { ok: true, title: trimmed };
    });
  }

  async renameStream(streamKey, newTitle) {
    const trimmed = typeof newTitle === "string" ? newTitle.trim() : "";
    if (!streamKey || !trimmed) {
      const msg = !streamKey ? "missing Stream key" : "Title cannot be empty.";
      this.reportActionError("Could not rename Stream", msg);
      return { ok: false, error: msg };
    }
    return this._withPendingSet(this.pendingRenames, streamKey, async () => {
      const target = await this.ensureWriteMachineForKey(streamKey);
      if (!target.ok) {
        this.reportActionError("Could not rename Stream", target.error);
        return { ok: false, error: target.error };
      }
      const out = await this.runExec(
        this.streamsBinFor(target.machine),
        ["stream", "edit", streamKey, "--title", trimmed],
        { on: target.machine }
      );
      if (out.code !== 0) {
        const err = extractCliError(out, "could not rename Stream");
        this.reportActionError("Could not rename Stream", err.message);
        return { ok: false, error: err.message };
      }
      this.actionError = null;
      this.lastCompletedKey = null;
      await this.refreshAfterMutation({ streamKey, machine: target.machine });
      return { ok: true, title: trimmed };
    });
  }

  async moveSessionToStream(
    {
      sessionKey,
      sourceStreamKey = null,
      targetStreamKey,
      moveTab = false,
      terminalList = [],
      panes = [],
      spaces = [],
    } = {},
    { run, readPanes, readSpaces, sleep } = {}
  ) {
    if (!sessionKey || !targetStreamKey) {
      const msg = "missing Session or target Stream key";
      this.reportActionError("Could not move Session", msg);
      return { ok: false, error: msg };
    }
    if (sourceStreamKey && targetStreamKey === sourceStreamKey) {
      return { ok: true, movedTab: false };
    }
    const sessHost = recordHostId(sessionKey);
    const targetHost = recordHostId(targetStreamKey);
    if (sessHost && targetHost && sessHost !== targetHost) {
      const msg = "Sessions can only move to a Stream on the same host.";
      this.reportActionError("Could not move Session", msg);
      return { ok: false, error: msg };
    }

    return this._withPendingSet(this.pendingMoves, sessionKey, async () => {
      const target = await this.ensureWriteMachineForKey(sessionKey);
      if (!target.ok) {
        this.reportActionError("Could not move Session", target.error);
        return { ok: false, error: target.error };
      }

      const out = await this.runExec(
        this.streamsBinFor(target.machine),
        ["session", "edit", sessionKey, "--stream", targetStreamKey],
        { on: target.machine }
      );
      if (out.code !== 0) {
        const err = extractCliError(out, "could not move Session");
        this.reportActionError("Could not move Session", err.message);
        return { ok: false, error: err.message };
      }

      let movedTab = false;
      let tabMoveFailed = false;
      try {
        const latestPanes = typeof readPanes === "function" ? readPanes() ?? panes : panes;
        const latestSpaces = typeof readSpaces === "function" ? readSpaces() ?? spaces : spaces;
        const moveInfo = resolveMoveSpaceTarget({
          sessionKey,
          targetStreamKey,
          machine: target.machine,
          terminalList,
          panes: latestPanes,
          spaces: latestSpaces,
        });

        const liveTermAddress = moveInfo.liveTerm?.address ?? null;
        if (moveTab && moveInfo.canMoveTab && liveTermAddress && typeof run === "function") {
          if (typeof sleep !== "function" || typeof readPanes !== "function") {
            tabMoveFailed = true;
            this.reportActionError(
              "Moved Session, skipped tab transfer",
              "Tab transfer requires Herdr async sleep and pane polling."
            );
            await this.runExec(this.hcBin, [
              "label",
              liveTermAddress,
              `${LABEL_STREAM}=${targetStreamKey}`,
            ]);
          } else {
            const unqualSpace = unqualifyForMachine(moveInfo.targetSpace.id, target.machine);
            const freshTabId = `tb_mv_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 6)}`;
            const qualifiedTabId = qualifyForMachine(freshTabId, target.machine);
            run("terminal.park", { terminal: liveTermAddress });
            let detached = false;
            for (let attempt = 0; attempt < 30; attempt++) {
              await sleep(50);
              const currentPanes = readPanes() ?? [];
              if (!currentPanes.some((p) => p?.terminal === liveTermAddress)) {
                detached = true;
                break;
              }
            }
            if (!detached) {
              tabMoveFailed = true;
              this.reportActionError(
                "Moved Session, tab stayed put",
                "The tab did not detach in time for transfer to the target Space."
              );
              await this.runExec(this.hcBin, [
                "label",
                liveTermAddress,
                `${LABEL_STREAM}=${targetStreamKey}`,
              ]);
            } else {
              const labelOut = await this.runExec(this.hcBin, [
                "label",
                liveTermAddress,
                `${LABEL_STREAM}=${targetStreamKey}`,
                `${LABEL_HERDR_SPACE}=${unqualSpace}`,
                `${LABEL_HERDR_TAB}=${freshTabId}`,
              ]);
              if (labelOut.code !== 0) {
                tabMoveFailed = true;
                const err = extractCliError(labelOut, "could not label parked terminal");
                this.reportActionError(
                  "Moved Session, terminal was parked",
                  `Could not place tab in ${moveInfo.targetSpace.name} (${err.message}). Re-keep it from the Agents list.`
                );
              } else {
                let adopted = false;
                for (let attempt = 0; attempt < 30; attempt++) {
                  await sleep(50);
                  const currentPanes = readPanes() ?? [];
                  if (
                    currentPanes.some(
                      (p) =>
                        p?.terminal === liveTermAddress &&
                        p?.space === moveInfo.targetSpace.id
                    )
                  ) {
                    adopted = true;
                    break;
                  }
                }
                if (adopted) {
                  movedTab = true;
                  if (moveInfo.currentTabName) {
                    run("tab.rename", { id: qualifiedTabId, name: moveInfo.currentTabName });
                  }
                } else {
                  tabMoveFailed = true;
                  this.reportActionError(
                    "Moved Session, tab placement unconfirmed",
                    `Placement in ${moveInfo.targetSpace.name} was not confirmed in time.`
                  );
                }
              }
            }
          }
        } else if (liveTermAddress) {
          await this.runExec(this.hcBin, [
            "label",
            liveTermAddress,
            `${LABEL_STREAM}=${targetStreamKey}`,
          ]);
        }
      } catch (err) {
        tabMoveFailed = true;
        this.reportActionError(
          "Moved Session, tab transfer failed",
          String(err?.message ?? err ?? "unknown error")
        );
      }

      if (!tabMoveFailed) {
        this.actionError = null;
      }
      this.lastCompletedKey = null;
      await this.refreshAfterMutation({
        sessionKey,
        streamKey: targetStreamKey,
        extraStreamKeys:
          sourceStreamKey && sourceStreamKey !== targetStreamKey ? [sourceStreamKey] : [],
        machine: target.machine,
      });
      return { ok: true, movedTab };
    });
  }

  async _bulkSetStreamSessionsStatus(streamKey, targetStatus, predicate) {
    if (!streamKey) return { ok: false, error: "missing Stream key", updated: [] };
    const errorTitle =
      targetStatus === "parked" ? "Could not park all sessions" : "Could not close all sessions";
    const pendingLabel = targetStatus === "parked" ? "Parking…" : "Closing…";
    return this._withPendingMap(this.pendingStreamActions, streamKey, pendingLabel, async () => {
      const target = await this.ensureWriteMachineForKey(streamKey);
      if (!target.ok) {
        this.reportActionError(errorTitle, target.error);
        return { ok: false, error: target.error, updated: [] };
      }
      const targetMachine = target.machine;
      const bin = this.streamsBinFor(targetMachine);
      let stream = this.streams.get(streamKey) ?? null;
      if (!stream || !Array.isArray(stream.sessions) || stream.sessions.length === 0) {
        const showOut = await this.runExec(bin, ["stream", "show", streamKey, "--json"], {
          on: targetMachine,
        });
        if (showOut.code !== 0) {
          const err = extractCliError(showOut, `could not load ${streamKey}`);
          this.reportActionError(errorTitle, err.message);
          return { ok: false, error: err.message, updated: [] };
        }
        stream = normalizeStream(parseJsonSafe(showOut.stdout));
        if (!stream) {
          const msg = "invalid stream show JSON";
          this.reportActionError(errorTitle, msg);
          return { ok: false, error: msg, updated: [] };
        }
        this.streams.set(stream.key, stream);
      }
      const targets = (stream?.sessions ?? []).filter(predicate);
      const updated = [];
      for (const sess of targets) {
        const out = await this.runExec(
          bin,
          ["session", "edit", sess.key, "--status", targetStatus],
          { on: targetMachine }
        );
        if (out.code !== 0) {
          const err = extractCliError(out, `could not update ${sess.key}`);
          const msg = `Failed on ${sess.title || sess.key}: ${err.message}`;
          this.lastCompletedKey = null;
          if (updated.length > 0) {
            await this.refreshAfterMutation({ streamKey, machine: targetMachine });
          }
          this.reportActionError(errorTitle, msg);
          return { ok: false, error: msg, updated, failed: sess.key };
        }
        updated.push(sess.key);
      }
      this.actionError = null;
      this.lastCompletedKey = null;
      await this.refreshAfterMutation({ streamKey, machine: targetMachine });
      return { ok: true, updated };
    });
  }

  async parkAllSessions(streamKey) {
    return this._bulkSetStreamSessionsStatus(
      streamKey,
      "parked",
      (s) => s.status === "active" || s.status === "waiting" || s.status === "blocked"
    );
  }

  async closeAllSessions(streamKey) {
    return this._bulkSetStreamSessionsStatus(
      streamKey,
      "closed",
      (s) => s.status !== "done" && s.status !== "closed"
    );
  }

  async deleteStream(streamKey) {
    if (!streamKey) return { ok: false, error: "missing Stream key" };
    return this._withPendingMap(this.pendingStreamActions, streamKey, "Deleting…", async () => {
      const target = await this.ensureWriteMachineForKey(streamKey);
      if (!target.ok) {
        this.reportActionError("Could not delete Stream", target.error);
        return { ok: false, error: target.error };
      }
      const out = await this.runExec(
        this.streamsBinFor(target.machine),
        ["stream", "delete", streamKey, "--json"],
        { on: target.machine }
      );
      if (out.code !== 0) {
        const err = extractCliError(out, `could not delete ${streamKey}`);
        this.reportActionError("Could not delete Stream", err.message);
        return { ok: false, error: err.message };
      }
      this.actionError = null;
      this.streams.delete(streamKey);
      for (const bucket of this.hostStreamsByScope.values()) {
        bucket.seq += 1;
        if (Array.isArray(bucket.items)) {
          bucket.items = bucket.items.filter((s) => s.key !== streamKey);
        }
      }
      if (this.previewStreamKey === streamKey) {
        this.previewStreamKey = null;
        this.previewSessionKey = null;
        this.preferredView = "all";
      }
      this.lastCompletedKey = null;
      return { ok: true, key: streamKey };
    });
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
   * Batch-refresh off-tab live enrolled terminals grouped by machine, skipping offline/loading
   * machines and any machine in `skipMachines`. Keeps healthy machines' `terminal.marks` updated
   * even when another machine fails.
   */
  async _refreshOffTabFrames({
    running,
    byKey,
    skipTerm = null,
    skipMachines = new Set(),
    gen,
  }) {
    const missingByMachine = new Map();
    for (const [term, sk] of running) {
      if (term === skipTerm) continue;
      if (byKey.has(sk)) {
        this.frames.set(term, { kind: "enrolled", session: byKey.get(sk) });
        continue;
      }
      const termMachine = parseTerminalAddress(term)?.machine ?? null;
      if (skipMachines.has(termMachine) || !this.isMachineReady(termMachine)) continue;
      const list = missingByMachine.get(termMachine) ?? [];
      list.push({ term, key: sk });
      missingByMachine.set(termMachine, list);
    }

    for (const [termMachine, items] of missingByMachine) {
      const uniqueKeys = [...new Set(items.map((m) => m.key))];
      const listArgs = ["session", "list"];
      for (const sk of uniqueKeys) {
        listArgs.push("--key", sk);
      }
      listArgs.push("--all", "--fields", "key,stream_id,title,status,state", "--json");
      const listOut = await this.runExec(this.streamsBinFor(termMachine), listArgs, {
        on: termMachine,
      });
      if (gen !== this.readGeneration) return false;
      if (listOut.code === 0) {
        const parsed = parseJsonSafe(listOut.stdout);
        for (const raw of parsed?.items ?? []) {
          const sess = normalizeSession(raw);
          if (sess) byKey.set(sess.key, sess);
        }
        for (const item of items) {
          const found = byKey.get(item.key);
          if (found) {
            this.frames.set(item.term, { kind: "enrolled", session: found });
          } else {
            this.frames.delete(item.term);
          }
        }
      }
    }
    return true;
  }

  _learnHostsFromTerminals(terminalList = [], running = null) {
    if (running) {
      for (const [addr, sessionKey] of running) {
        const termMachine = parseTerminalAddress(addr)?.machine ?? null;
        const hostId = recordHostId(sessionKey);
        if (hostId) this.recordHostMachine(hostId, termMachine);
      }
    }
    for (const term of terminalList ?? []) {
      const parsed = parseTerminalAddress(term);
      if (!parsed) continue;
      const sessionHost = recordHostId(term.labels?.[LABEL_SESSION]);
      if (sessionHost) this.recordHostMachine(sessionHost, parsed.machine);
      const streamHost = recordHostId(term.labels?.[LABEL_STREAM]);
      if (streamHost) this.recordHostMachine(streamHost, parsed.machine);
    }
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
    machineList,
    tabIds,
    force = false,
    now = Date.now(),
  }) {
    if (Array.isArray(machineList)) {
      this._hasMachineSnapshot = true;
      this.machineLayouts.clear();
      for (const m of machineList) {
        if (m && typeof m.name === "string" && m.name.length > 0) {
          this.machineLayouts.set(m.name, m.layout ?? "ready");
        }
      }
    }

    const activeMachine = activeSpace?.machine ?? null;
    this.activeMachine = activeMachine;

    if (activeMachine && this.isMachineReady(activeMachine)) {
      void this.ensureHomeDir(activeMachine);
    }

    const spaceTabKey = `${activeSpace?.id ?? ""}:${activeTabId ?? ""}`;
    if (this.lastSpaceTabKey !== null && spaceTabKey !== this.lastSpaceTabKey) {
      this.previewStreamKey = null;
      this.inspectedStreamId = null;
      this.previewSessionKey = null;
    }
    this.lastSpaceTabKey = spaceTabKey;

    if (activeSpace?.id && activeTabId) {
      const knownTabStream = this.tabStreams.get(activeTabId);
      if (knownTabStream) {
        this.lastFocusedStreamBySpace.set(activeSpace.id, knownTabStream);
      }
    }

    const running = runningSessionTerminals({ terminalList, panes, agents });
    this._lastRunning = running;

    // Passively learn hostId -> machine from live running terminals and stream labels.
    this._learnHostsFromTerminals(terminalList, running);

    const visible = tabTerminals(visibleTerminals);
    const terminalSessions = visible
      .filter((t) => running.has(t))
      .map((t) => `${t}=${running.get(t)}`);
    const allRunningSessions = [...running.entries()]
      .map(([t, s]) => `${t}=${s}`)
      .sort();
    const machineStates = this._hasMachineSnapshot
      ? [...this.machineLayouts.entries()]
          .map(([name, layout]) => `${name}=${layout}`)
          .sort()
      : [];
    const noticeToken = observeNoticeToken(terminalList, LABEL_SAVED);
    const observation = {
      activeSpace,
      activeTabId,
      visibleTerminals: visible,
      focusedTerminal,
      terminalSessions,
      allRunningSessions,
      noticeToken,
      machineStates,
      hostScope: this.hostScope,
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
      panes,
      agents,
      tabIds,
      key,
      force,
      now,
    });
  }

  /**
   * Read records for the active tab and off-tab live Sessions through the public `streams` CLI,
   * reconcile `streams.stream` labels on visible running-Session terminals, and publish
   * `terminal.marks` to herdr-app shared state.
   */
  async refresh({
    observation,
    running,
    terminalList = [],
    panes = [],
    agents = [],
    tabIds,
    key,
    force = false,
    now = Date.now(),
  }) {
    if (Array.isArray(tabIds)) {
      for (const tabId of this.tabStreams.keys()) {
        if (!tabIds.includes(tabId)) this.tabStreams.delete(tabId);
      }
    }

    // Prune exited or unenrolled terminals from frames against current running snapshot.
    for (const term of [...this.frames.keys()]) {
      if (!running.has(term)) {
        this.frames.delete(term);
      }
    }

    const activeMachine = observation.activeSpace?.machine ?? null;
    const activeScope = observation.hostScope ?? this.hostScope;
    const gen = ++this.readGeneration;
    this.inFlightKey = key ?? null;
    const noticeChanged = observation.noticeToken !== this.lastNoticeToken;

    const promise = (async () => {
      try {
        await this.ensureConfig();
        if (gen !== this.readGeneration) return null;

        const byKey = new Map();

        // If the active remote machine is offline or still loading, keep healthy machines'
        // off-tab frames and terminal.marks up to date without issuing failing remote calls.
        if (!this.isMachineReady(activeMachine)) {
          const ok = await this._refreshOffTabFrames({
            running,
            byKey,
            skipMachines: new Set([activeMachine]),
            gen,
          });
          if (!ok || gen !== this.readGeneration) return null;
          this.lastNoticeToken = observation.noticeToken ?? "";
          this.readError = null;
          this.lastFailedKey = null;
          this.lastCompletedAt = now;
          this.lastCompletedKey = key ?? null;
          this.publishSharedTerminals(running);
          return null;
        }

        const { activeTabId = null, visibleTerminals = [], focusedTerminal = null } = observation;
        const activeBin = this.streamsBinFor(activeMachine);

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

        // 1. Try reading the focused (or first visible) running Session with `session show`.
        let primaryTerm = null;
        let primarySession = null;
        for (const term of candidates) {
          const sessionKey = running.get(term);
          const out = await this.runExec(activeBin, ["session", "show", sessionKey, "--json"], {
            on: activeMachine,
          });
          if (gen !== this.readGeneration) return null;
          if (out.code === 0) {
            const parsed = parseJsonSafe(out.stdout);
            const session = normalizeSession(parsed);
            if (session) {
              primaryTerm = term;
              primarySession = session;
              if (session.host_id) this.recordHostMachine(session.host_id, activeMachine);
              this.frames.set(term, { kind: "enrolled", session });
              break;
            }
          } else {
            const err = extractCliError(out, "session show failed");
            if (!out.transportError && err.kind === "not-found") {
              // Stale `streams.session` label: ignore it and move on to the next candidate.
              this.frames.delete(term);
              continue;
            }
            this.readError = err.message;
            this.lastFailedKey = key ?? null;
            // Still update off-tab frames & terminal.marks for healthy machines.
            const ok = await this._refreshOffTabFrames({
              running,
              byKey,
              skipMachines: new Set([activeMachine]),
              gen,
            });
            if (!ok || gen !== this.readGeneration) return null;
            this.publishSharedTerminals(running);
            return null;
          }
        }

        let activeStream = null;

        if (primarySession) {
          const streamKey = `${primarySession.host_id}/${primarySession.stream_id}`;
          // 2. Read that Session's Stream with `stream show`.
          const streamOut = await this.runExec(activeBin, ["stream", "show", streamKey, "--json"], {
            on: activeMachine,
          });
          if (gen !== this.readGeneration) return null;
          if (streamOut.code !== 0) {
            const err = extractCliError(streamOut, "stream show failed");
            this.readError = err.message;
            this.lastFailedKey = key ?? null;
            const ok = await this._refreshOffTabFrames({
              running,
              byKey,
              skipTerm: primaryTerm,
              skipMachines: new Set([activeMachine]),
              gen,
            });
            if (!ok || gen !== this.readGeneration) return null;
            this.publishSharedTerminals(running);
            return null;
          }
          const stream = normalizeStream(parseJsonSafe(streamOut.stdout));
          if (!stream) {
            this.readError = "invalid stream show JSON";
            this.lastFailedKey = key ?? null;
            const ok = await this._refreshOffTabFrames({
              running,
              byKey,
              skipTerm: primaryTerm,
              skipMachines: new Set([activeMachine]),
              gen,
            });
            if (!ok || gen !== this.readGeneration) return null;
            this.publishSharedTerminals(running);
            return null;
          }
          activeStream = stream;
          if (stream.host_id) this.recordHostMachine(stream.host_id, activeMachine);
          this.streams.set(stream.key, stream);
          this.tabStreams.set(activeTabId, stream.key);
          if (observation.activeSpace?.id) {
            this.lastFocusedStreamBySpace.set(observation.activeSpace.id, stream.key);
          }

          // Index the Stream's member Sessions by key for pane headers and off-tab frames.
          byKey.set(primarySession.key, primarySession);
          for (const member of stream.sessions) {
            if (!byKey.has(member.key)) byKey.set(member.key, member);
          }
        } else {
          this.tabStreams.set(activeTabId, null);
        }

        // 3. Populate frames for all other live running terminals across all Spaces and machines.
        const offTabOk = await this._refreshOffTabFrames({
          running,
          byKey,
          skipTerm: primaryTerm,
          gen,
        });
        if (!offTabOk || gen !== this.readGeneration) return null;

        // 3b. When the active tab is unenrolled, ensure the Space's fallback Stream (if any)
        //     is loaded in `this.streams` so `Stream` fallback renders complete Sessions & goal.
        if (!primarySession && observation.activeSpace) {
          const spaceStreamKey = resolveSpaceStreamKey({
            space: observation.activeSpace,
            panes,
            terminalList,
            agents,
            frames: this.frames,
            lastFocusedStreamBySpace: this.lastFocusedStreamBySpace,
          });
          if (spaceStreamKey && (!this.streams.has(spaceStreamKey) || force || noticeChanged)) {
            const spaceStreamOut = await this.runExec(
              activeBin,
              ["stream", "show", spaceStreamKey, "--json"],
              { on: activeMachine }
            );
            if (gen !== this.readGeneration) return null;
            if (spaceStreamOut.code === 0) {
              const spaceStream = normalizeStream(parseJsonSafe(spaceStreamOut.stdout));
              if (spaceStream) {
                this.streams.set(spaceStream.key, spaceStream);
              }
            }
          }
        }

        // 4. Reconcile `streams.stream` hc labels on visible terminals running known Sessions.
        //    Local `hc label <machine>/<t> k=v` routes natively through the local core.
        if (primarySession) {
          const termByAddr = new Map();
          for (const t of terminalList) {
            const parsed = parseTerminalAddress(t);
            if (parsed) termByAddr.set(parsed.address, t);
          }
          for (const term of visibleTerminals) {
            const frame = this.frames.get(term);
            if (frame?.kind !== "enrolled" || !frame.session?.stream) continue;
            const desiredStream = frame.session.stream;
            const currentLabel = termByAddr.get(term)?.labels?.[LABEL_STREAM] ?? null;
            if (currentLabel !== desiredStream) {
              await this.runExec(this.hcBin, ["label", term, `${LABEL_STREAM}=${desiredStream}`]);
            }
          }
        }

        // 5. Refresh the active `(activeMachine, activeScope)` Streams bucket when unenrolled,
        //    viewing `All`, on explicit/notice refresh after listing was loaded, or when an
        //    off-tab live Session belongs to an unknown Stream.
        const activeBucket = this._getHostStreamsEntry(activeMachine, activeScope);
        const hostByKey = new Set(activeBucket.items.map((s) => s.key));
        let hasUnknownOffTabStream = false;
        for (const [term] of running) {
          const frame = this.frames.get(term);
          if (
            frame?.kind === "enrolled" &&
            frame.session?.stream &&
            !this.streams.has(frame.session.stream) &&
            !hostByKey.has(frame.session.stream)
          ) {
            hasUnknownOffTabStream = true;
            break;
          }
        }

        const shouldFetchHostStreams =
          !primarySession ||
          this.preferredView === "all" ||
          hasUnknownOffTabStream ||
          (activeBucket.loaded && (force || noticeChanged));

        let listStale = false;
        if (shouldFetchHostStreams) {
          const listRes = await this._fetchHostStreams({
            machine: activeMachine,
            scope: activeScope,
            force: true,
            forceProbe: force,
          });
          if (gen !== this.readGeneration) return null;
          if (listRes.stale) {
            listStale = true;
          } else if (!listRes.ok && !primarySession) {
            this.lastFailedKey = key ?? null;
            this.publishSharedTerminals(running);
            return null;
          }
        }

        // Refresh detached previews when a save notice or manual force refresh occurred.
        if (force || noticeChanged) {
          if (this.previewStreamKey && this.previewStreamKey !== activeStream?.key) {
            const pTarget = this.resolveReadMachineForKey(this.previewStreamKey);
            if (!pTarget.unavailable) {
              const pOut = await this.runExec(
                this.streamsBinFor(pTarget.machine),
                ["stream", "show", this.previewStreamKey, "--json"],
                { on: pTarget.machine }
              );
              if (gen === this.readGeneration && pOut.code === 0) {
                const refreshed = normalizeStream(parseJsonSafe(pOut.stdout));
                if (refreshed) this.streams.set(refreshed.key, refreshed);
              }
            }
          }
          if (this.previewSessionKey) {
            const sTarget = this.resolveReadMachineForKey(this.previewSessionKey);
            if (!sTarget.unavailable) {
              const sOut = await this.runExec(
                this.streamsBinFor(sTarget.machine),
                ["session", "show", this.previewSessionKey, "--json"],
                { on: sTarget.machine }
              );
              if (gen === this.readGeneration && sOut.code === 0) {
                const refreshed = normalizeSession(parseJsonSafe(sOut.stdout));
                if (refreshed) this.previewSessions.set(refreshed.key, refreshed);
              }
            }
          }
        }

        this.lastNoticeToken = observation.noticeToken ?? "";
        this.readError = null;
        this.lastFailedKey = null;
        this.publishSharedTerminals(running);
        if (listStale && !primarySession) {
          return null;
        }
        this.lastCompletedAt = now;
        this.lastCompletedKey = key ?? null;
        return primarySession
          ? { kind: "stream", stream: activeStream, session: primarySession }
          : { kind: "listing", streams: activeBucket.items };
      } catch (err) {
        if (gen === this.readGeneration) {
          this.readError = err instanceof Error ? err.message : String(err);
          this.lastFailedKey = key ?? null;
          this.publishSharedTerminals(running);
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
        this.actionError = null;
        this.notifyViews();
        return { ok: true };
      }
      this.reportActionError("Could not open reference", "reference has no URL");
      return { ok: false, error: "reference has no URL" };
    }

    const effectiveMode = mode ?? this.herdrConfig.viewer_mode ?? "editor";
    const effectivePlacement = placement ?? this.herdrConfig.viewer_placement ?? "pane-right";
    const title = reference.title ?? reference.filename ?? "";
    const filename = reference.filename ?? title;

    // 2. Documents and Artifacts.
    let readArgs = null;
    let viewArgs = null;
    let targetMachine = this.activeMachine;
    if (reference.kind === "document") {
      if (!reference.session || !reference.path) {
        this.reportActionError(
          "Could not open document",
          "document reference is missing session or path"
        );
        return { ok: false, error: "document reference is missing session or path" };
      }
      const owner = await this.ensureWriteMachineForKey(reference.session);
      if (!owner.ok) {
        this.reportActionError(`Could not open ${title || "document"}`, owner.error);
        return { ok: false, error: owner.error };
      }
      targetMachine = owner.machine;
      readArgs = ["session", "document", "read", reference.session, reference.path];
      viewArgs = ["session", "document", "view", reference.session, reference.path];
    } else if (reference.kind === "artifact") {
      if (!reference.key) {
        this.reportActionError("Could not open artifact", "artifact reference is missing key");
        return { ok: false, error: "artifact reference is missing key" };
      }
      const readTarget = this.resolveReadMachineForKey(reference.key);
      if (readTarget.unavailable) {
        const msg = `${readTarget.unavailable} is offline`;
        this.reportActionError(`Could not open ${title || "artifact"}`, msg);
        return { ok: false, error: msg };
      }
      targetMachine = readTarget.machine;
      readArgs = ["artifact", "read", reference.key];
      viewArgs = ["artifact", "view", reference.key];
    } else {
      const msg = `unsupported reference kind: ${reference.kind}`;
      this.reportActionError("Could not open reference", msg);
      return { ok: false, error: msg };
    }

    // Cross-machine preview (e.g. viewing a remote Session's document while on a local space)
    // cannot split the active space's pane into the owning machine, so fall back to native mode.
    const runMode =
      effectiveMode !== "native" && targetMachine !== this.activeMachine
        ? "native"
        : effectiveMode;

    if (runMode === "native") {
      const out = await this.runExec(this.streamsBinFor(targetMachine), readArgs, {
        on: targetMachine,
      });
      if (out.code !== 0) {
        const msg = extractCliError(out, "could not read reference").message;
        this.reportActionError(`Could not open ${title || reference.kind}`, msg);
        return { ok: false, error: msg };
      }
      const prepared = prepareNativeContent(out.stdout ?? "");
      if (!prepared.ok) {
        this.reportActionError(`Could not open ${title || reference.kind}`, prepared.error);
        return { ok: false, error: prepared.error };
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
          streamsBin: this.streamsBinFor(targetMachine),
          run,
          openOverlay,
          onPendingViewer: (entry) => this.recordPendingViewer(entry),
        }
      );
      if (!outcome.ok) {
        this.reportActionError(`Could not open ${title || reference.kind}`, outcome.error);
      } else {
        this.actionError = null;
        this.notifyViews();
      }
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
        streamsBin: this.streamsBinFor(this.activeMachine),
        run,
        openOverlay,
      }
    );
    if (!outcome.ok) {
      this.reportActionError(`Could not open ${title || reference.kind}`, outcome.error);
    } else {
      this.actionError = null;
      this.notifyViews();
    }
    return outcome;
  }

  /**
   * Open a Stream (or specific Session(s) of a Stream) by querying `session list --stream`,
   * reusing already-running Sessions, finding the Stream's space from `streams.stream` labels,
   * and spawning missing Sessions with `hc run [--on <machine>] ... streams session resume KEY`.
   */
  async openWork(
    { stream, sessions = { kind: "all_eligible" }, terminalList = [], panes = [], agents = [] },
    { run, focusTerminal } = {}
  ) {
    if (!stream) return null;
    const isSelected = sessions?.kind === "selected" && Array.isArray(sessions.session_ids);
    const isBulkOpen =
      !isSelected &&
      (sessions == null || sessions.kind === "all_eligible" || sessions.kind === "only_active");

    let effectiveSessions = sessions;
    const addedSessionIds = [];
    if (isSelected) {
      const freshIds = sessions.session_ids.filter((id) => !this.pendingResumeSessions.has(id));
      if (freshIds.length === 0) return { ok: false, duplicate: true };
      for (const id of freshIds) {
        this.pendingResumeSessions.add(id);
        addedSessionIds.push(id);
      }
      effectiveSessions = { ...sessions, session_ids: freshIds };
    } else if (isBulkOpen) {
      if (this.pendingStreamActions.has(stream)) return { ok: false, duplicate: true };
      this.pendingStreamActions.set(stream, "Opening…");
    }
    this.notifyViews();
    try {
      return await this._openWorkInner(
        { stream, sessions: effectiveSessions, terminalList, panes, agents },
        { run, focusTerminal }
      );
    } finally {
      for (const id of addedSessionIds) this.pendingResumeSessions.delete(id);
      if (isBulkOpen) this.pendingStreamActions.delete(stream);
      this.notifyViews();
    }
  }

  async _openWorkInner(
    { stream, sessions = { kind: "all_eligible" }, terminalList = [], panes = [], agents = [] },
    { run, focusTerminal } = {}
  ) {
    await this.ensureConfig();
    this._learnHostsFromTerminals(terminalList);

    const target = await this.ensureWriteMachineForKey(stream);
    if (!target.ok) {
      this.reportActionError("Could not open Stream", target.error);
      return { ok: false, error: target.error };
    }
    const targetMachine = target.machine;
    const bin = this.streamsBinFor(targetMachine);

    const listArgs = [
      "session",
      "list",
      "--stream",
      stream,
      "--all",
      "--fields",
      "key,title,status,harness,directory",
      "--json",
    ];
    const listOut = await this.runExec(bin, listArgs, { on: targetMachine });
    if (listOut.code !== 0) {
      const msg = extractCliError(listOut, "could not list Sessions for Stream").message;
      this.reportActionError("Could not open Stream", msg);
      return { ok: false, error: msg };
    }
    const parsed = parseJsonSafe(listOut.stdout);
    const allSessions = (parsed?.items ?? []).map(normalizeSession).filter(Boolean);

    // Keep requested Sessions, active-only Pi Sessions, or all eligible (active + parked) Pi Sessions.
    let kept;
    if (sessions?.kind === "selected" && Array.isArray(sessions.session_ids)) {
      const wanted = new Set(sessions.session_ids);
      kept = allSessions.filter((s) => wanted.has(s.key) || wanted.has(s.id));
    } else if (sessions?.kind === "only_active") {
      kept = allSessions.filter((s) => s.harness === "pi" && s.status === "active");
    } else {
      kept = allSessions.filter(
        (s) => s.harness === "pi" && (s.status === "active" || s.status === "parked")
      );
    }

    if (kept.length === 0) {
      const msg =
        sessions?.kind === "selected"
          ? "Selected Session was not found in this Stream."
          : "No eligible Pi Sessions to open for this Stream.";
      this.reportActionError("Could not resume Session", msg);
      return { ok: false, error: msg, launched: [], reused: [], missingDirectory: [] };
    }

    const running = runningSessionTerminals({ terminalList, panes, agents });
    const termBySessionKey = new Map();
    for (const [termAddr, sessionKey] of running) {
      termBySessionKey.set(sessionKey, termAddr);
    }

    const existingSpace = findSpaceForStream(terminalList, stream, targetMachine);
    const spaceKey = existingSpace ?? `streams-${this.newId()}`;
    const desktopSpaceId = existingSpace ? qualifyForMachine(existingSpace, targetMachine) : null;
    let selectedSpace = false;
    const ensureSpaceSelected = () => {
      if (desktopSpaceId && !selectedSpace) {
        selectedSpace = true;
        run?.("space.select", { id: desktopSpaceId });
      }
    };

    const launched = [];
    const reused = [];
    const missingDirectory = [];

    for (const sess of kept) {
      const sessLabel = sess.title || sess.key;
      const runningTerm = termBySessionKey.get(sess.key);
      if (runningTerm) {
        ensureSpaceSelected();
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
        ...(targetMachine ? ["--on", targetMachine] : []),
        "--cwd",
        cwd,
        "--label",
        `${LABEL_STREAM}=${stream}`,
        "--label",
        `${LABEL_HERDR_SPACE}=${spaceKey}`,
        "--label",
        `${LABEL_HERDR_TAB}=${tabKey}`,
        bin,
        "session",
        "resume",
        sess.key,
      ];
      const runOut = await this.runExec(this.hcBin, hcArgs);
      if (runOut.code !== 0) {
        const parsedErr = extractCliError(runOut, `could not resume ${sessLabel}`);
        const maybeMissingCwd =
          !runOut.transportError &&
          (parsedErr.kind === "spawn_failed" ||
            /no such file or directory|os error 2|does not exist/i.test(parsedErr.message));
        let dirMissing = false;
        if (maybeMissingCwd) {
          const dirCheck = await this.runExec("test", ["-d", cwd], { on: targetMachine });
          dirMissing = !dirCheck.transportError && dirCheck.code === 1;
        }
        if (dirMissing) {
          const resolvedHome =
            this.homeDirFor(targetMachine) ??
            (targetMachine ? await this.ensureHomeDir(targetMachine) : this.homeDir);
          const formatted = formatMissingDirectoryError({
            session: sess,
            cwd,
            homeDir: resolvedHome,
          });
          this.reportActionError(formatted.title, formatted.message, {
            copyText: formatted.copyText,
            copyLabel: formatted.copyLabel,
          });
          return { ok: false, error: formatted.message, launched, reused, missingDirectory };
        }
        const hintSuffix = parsedErr.hint ? ` (${parsedErr.hint})` : "";
        const msg = `${parsedErr.message}${hintSuffix}`;
        this.reportActionError(`Could not resume "${sessLabel}"`, msg);
        return { ok: false, error: msg, launched, reused, missingDirectory };
      }
      ensureSpaceSelected();
      launched.push({ session: sess.key, space: spaceKey, tab: tabKey, cwd });
    }

    if (missingDirectory.length > 0) {
      const msg = `Skipped Session without directory: ${missingDirectory.join(", ")}`;
      this.reportActionError("Session has no saved directory", msg, { level: "warning" });
    } else {
      this.actionError = null;
    }
    this.lastCompletedKey = null;
    this.notifyViews();
    return { ok: true, space: spaceKey, launched, reused, missingDirectory };
  }
}
