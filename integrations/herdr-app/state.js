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
  loaded = true,
}) {
  if (shown.length > 0) return null;
  if (error) return `Streams are unavailable: ${error}`;
  if (!loaded) return "Loading Streams…";
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

function isLivePiTerminal(term, pane, ag) {
  if (!term || (term.state && term.state !== "live")) return false;
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
  return explicitKind ? explicitKind.toLowerCase() === "pi" : hasAgentBool;
}

/**
 * Build a map of terminalKey -> sessionKey for local and remote terminals that carry a non-empty
 * `streams.session` label AND have a live Pi agent reported by Herdr.
 */
export function runningSessionTerminals({
  terminalList = [],
  panes = [],
  agents = [],
  staleSessionKeys = null,
} = {}) {
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
    if (!isLivePiTerminal(term, paneByTerm.get(addr), agentByTerm.get(addr))) continue;
    const trimmed = sessionKey.trim();
    if (staleSessionKeys?.has?.(trimmed)) continue;
    running.set(addr, trimmed);
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
 * Single synchronous pass over a Herdr desktop snapshot, computing every live lookup needed
 * by `renderFrame`, `renderSidebar`, `resolveSidebarView`, and background fetchers.
 */
export function indexLive(
  snap = {},
  { sessions = null, staleSessionKeys = null, lastFocusedStreamBySpace = null } = {}
) {
  const {
    allSpaces = null,
    spaces = [],
    activeSpace = null,
    activeTab = null,
    activeTabId = activeTab?.id ?? null,
    visibleTerminals = [],
    focusedTerminal = null,
    terminalList = [],
    panes = [],
    agents = [],
  } = snap ?? {};
  const spaceList = allSpaces ?? spaces ?? [];

  const termByAddr = new Map();
  const savedTokensByTerm = new Map();
  for (const term of terminalList ?? []) {
    const parsed = parseTerminalAddress(term);
    if (!parsed) continue;
    termByAddr.set(parsed.address, term);
    const tok = term.labels?.[LABEL_SAVED];
    if (typeof tok === "string" && tok.length > 0) {
      savedTokensByTerm.set(parsed.address, tok);
    }
  }

  const paneByTerm = new Map();
  const panesByTab = new Map();
  for (const pane of panes ?? []) {
    if (!pane) continue;
    const addr = parseTerminalAddress(pane.terminal ?? pane.content?.terminal)?.address;
    if (addr && !paneByTerm.has(addr)) paneByTerm.set(addr, pane);
    if (pane.tab) {
      const list = panesByTab.get(pane.tab) ?? [];
      list.push(pane);
      panesByTab.set(pane.tab, list);
    }
  }

  const runningByTerm = runningSessionTerminals({
    terminalList,
    panes,
    agents,
    staleSessionKeys,
  });

  const streamKeyForTerm = (addr, sessionKey) => {
    const cachedStream = sessionKey ? sessions?.get?.(sessionKey)?.stream : null;
    if (cachedStream) return cachedStream;
    const label = termByAddr.get(addr)?.labels?.[LABEL_STREAM];
    return typeof label === "string" && label.trim() ? label.trim() : null;
  };

  const paneBySession = new Map();
  const openCountByStream = new Map();
  const liveSessionKeys = new Set();
  for (const [addr, sessionKey] of runningByTerm) {
    liveSessionKeys.add(sessionKey);
    const pane = paneByTerm.get(addr);
    if (pane && !paneBySession.has(sessionKey)) {
      paneBySession.set(sessionKey, {
        pane: pane.pane,
        terminal: addr,
        tab: pane.tab ?? null,
      });
    }
    const streamKey = streamKeyForTerm(addr, sessionKey);
    if (streamKey) {
      openCountByStream.set(streamKey, (openCountByStream.get(streamKey) ?? 0) + 1);
    }
  }

  const visible = tabTerminals(visibleTerminals);
  let activeTerm = null;
  if (focusedTerminal && visible.includes(focusedTerminal) && runningByTerm.has(focusedTerminal)) {
    activeTerm = focusedTerminal;
  } else {
    for (const addr of visible) {
      if (runningByTerm.has(addr)) {
        activeTerm = addr;
        break;
      }
    }
  }
  const activeSessionKey = activeTerm ? runningByTerm.get(activeTerm) ?? null : null;
  const activeStreamKey = activeTerm ? streamKeyForTerm(activeTerm, activeSessionKey) : null;

  let spaceStreamKey = null;
  if (
    activeSpace &&
    Array.isArray(activeSpace.tabs) &&
    activeSpace.tabs.length > 0 &&
    runningByTerm.size > 0
  ) {
    const liveSpaceStreams = new Set();
    for (const tab of activeSpace.tabs) {
      const tabId = typeof tab === "string" ? tab : tab?.id;
      if (!tabId) continue;
      for (const pane of panesByTab.get(tabId) ?? []) {
        const addr = parseTerminalAddress(pane.terminal ?? pane.content?.terminal)?.address;
        if (!addr || !runningByTerm.has(addr)) continue;
        const sk = streamKeyForTerm(addr, runningByTerm.get(addr));
        if (sk) liveSpaceStreams.add(sk);
      }
    }
    if (liveSpaceStreams.size > 0) {
      const lastFocused = lastFocusedStreamBySpace?.get?.(activeSpace.id) ?? null;
      spaceStreamKey =
        lastFocused && liveSpaceStreams.has(lastFocused)
          ? lastFocused
          : liveSpaceStreams.values().next().value ?? null;
    }
  }

  return {
    spaces: spaceList,
    activeSpace,
    activeTab,
    activeTabId,
    visibleTerminals: visible,
    focusedTerminal,
    termByAddr,
    paneByTerm,
    panesByTab,
    runningByTerm,
    paneBySession,
    openCountByStream,
    liveSessionKeys,
    savedTokensByTerm,
    activeTerm,
    activeSessionKey,
    activeStreamKey,
    spaceStreamKey,
  };
}

/**
 * Resolve the enrolled Session (if any) for a single pane frame in O(panes/terminals) without
 * indexing the entire workspace.
 */
export function sessionForFramePane({
  pane,
  terminalList = [],
  agents = [],
  staleSessionKeys = null,
  sessions = null,
} = {}) {
  const addr = parseTerminalAddress(pane?.terminal ?? pane?.content?.terminal)?.address;
  if (!addr) return null;
  const term = (terminalList ?? []).find((t) => parseTerminalAddress(t)?.address === addr) ?? null;
  if (!term) return null;
  const rawKey = term.labels?.[LABEL_SESSION];
  if (typeof rawKey !== "string" || rawKey.trim().length === 0) return null;
  const sessionKey = rawKey.trim();
  if (staleSessionKeys?.has?.(sessionKey)) return null;
  const liveAgent = (agents ?? []).find((a) => parseTerminalAddress(a)?.address === addr) ?? null;
  if (!isLivePiTerminal(term, pane, liveAgent)) return null;
  return {
    sessionKey,
    session: sessions?.get?.(sessionKey) ?? null,
    terminal: term,
    agent: liveAgent,
  };
}

/**
 * Filter and sort target Streams for the "Move to stream…" popover.
 * Only lists same-host Streams (excluding the current source Stream), non-archived first,
 * open Streams first, then most recently active.
 */
export function buildMoveCandidates({
  sourceStreamKey,
  hostId = null,
  streams = [],
  openCounts = new Map(),
  query = "",
} = {}) {
  const q = (query ?? "").trim().toLowerCase();
  const list = [];
  for (const item of streams ?? []) {
    if (!item?.key || item.key === sourceStreamKey) continue;
    if (hostId && item.host_id && item.host_id !== hostId) continue;
    if (q) {
      const hay = `${item.title ?? ""} ${item.goal ?? ""} ${item.id ?? ""}`.toLowerCase();
      if (!hay.includes(q)) continue;
    }
    const openCount = openCounts?.get?.(item.key) ?? 0;
    list.push({
      stream: item,
      openCount,
      isOpen: openCount > 0,
      isArchived: item.status === "done" || item.status === "closed",
    });
  }
  list.sort((a, b) => {
    if (a.isArchived !== b.isArchived) return a.isArchived ? 1 : -1;
    if (a.isOpen !== b.isOpen) return a.isOpen ? -1 : 1;
    return (b.stream.last_activity_ms ?? 0) - (a.stream.last_activity_ms ?? 0);
  });
  return list;
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
  sessions = null,
} = {}) {
  if (!streamKey) return null;
  const live = indexLive({ spaces, panes, terminalList, agents }, { sessions });
  const matchingTerms = new Set();
  for (const [addr, sk] of live.runningByTerm) {
    const cachedStream = sessions?.get?.(sk)?.stream ?? null;
    const labelStream = live.termByAddr.get(addr)?.labels?.[LABEL_STREAM]?.trim() ?? null;
    if (cachedStream === streamKey || labelStream === streamKey) {
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
  return indexLive({ panes, terminalList, agents }).paneBySession.get(sessionKey) ?? null;
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
 * Build the generic `terminal.marks` shared-state payload keyed by terminal address,
 * keeping tab bar and sidebar consumers orthogonal to Streams.
 */
export function buildSharedTerminalMarks({ sessions, running, staleSessionKeys = null } = {}) {
  const result = {};
  if (!sessions || !running) return result;
  const addrs = [...running.keys()].sort();
  for (const addr of addrs) {
    const sessionKey = running.get(addr);
    if (!sessionKey || staleSessionKeys?.has?.(sessionKey)) continue;
    const session = sessions.get(sessionKey);
    if (!session) continue;
    const mark = statusToTerminalMark(session.status ?? "parked");
    if (mark) result[addr] = mark;
  }
  return result;
}

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
 * Resolve the three-view right panel state (`session`, `stream`, or `all`),
 * applying sticky user intent (`preferredView`) with graceful fallback (`session -> stream -> all`)
 * while keeping detached preview overrides separate.
 *
 * Enrollment (`hasSessionAccess`, `hasStreamAccess`) is derived synchronously from Herdr's live
 * terminal labels (`live`), never from async CLI caches, so cache misses render inline loading
 * inside the intended view instead of spontaneously jumping to `Stream` or `All`.
 */
export function resolveSidebarView({
  preferredView = "session",
  sessions,
  streams,
  live,
  previewStreamKey = null,
  previewSessionKey = null,
}) {
  const desired = preferredView ?? "session";
  const currentSessionKey = live?.activeSessionKey ?? null;
  const currentSession = currentSessionKey ? sessions?.get?.(currentSessionKey) ?? null : null;
  const currentStreamKey = currentSession?.stream ?? live?.activeStreamKey ?? null;
  const currentTabStream = currentStreamKey ? streams?.get?.(currentStreamKey) ?? null : null;
  const isEnrolled = Boolean(currentSessionKey);
  const contextStreamKey = currentStreamKey ?? live?.spaceStreamKey ?? null;
  const contextStream =
    currentTabStream ?? (live?.spaceStreamKey ? streams?.get?.(live.spaceStreamKey) ?? null : null);

  const hasSessionAccess = isEnrolled || Boolean(previewSessionKey);
  const hasStreamAccess = Boolean(contextStreamKey || previewStreamKey || isEnrolled);

  let effectiveView = "all";
  if (desired === "session") {
    if (hasSessionAccess) effectiveView = "session";
    else if (hasStreamAccess) effectiveView = "stream";
    else effectiveView = "all";
  } else if (desired === "stream") {
    if (hasStreamAccess) effectiveView = "stream";
    else effectiveView = "all";
  }

  if (effectiveView === "all") {
    return {
      view: "all",
      preferredView: desired,
      hasSessionAccess,
      hasStreamAccess,
      isEnrolled,
      currentStreamKey: contextStreamKey,
      currentStream: contextStream,
      currentSession,
    };
  }

  if (effectiveView === "stream") {
    const streamKey = previewStreamKey ?? contextStreamKey;
    const stream = streamKey ? streams?.get?.(streamKey) ?? null : null;
    const isPreview = Boolean(previewStreamKey && previewStreamKey !== contextStreamKey);
    return {
      view: "stream",
      preferredView: desired,
      hasSessionAccess,
      hasStreamAccess,
      streamKey,
      stream,
      isPreview,
      canReturnToCurrent: isPreview && Boolean(contextStreamKey),
      isEnrolled,
      currentStreamKey: contextStreamKey,
      currentStream: contextStream,
      currentSession,
    };
  }

  // effectiveView === "session"
  if (previewSessionKey) {
    const session = sessions?.get?.(previewSessionKey) ?? null;
    const parentStreamKey = previewStreamKey ?? session?.stream ?? null;
    const stream = parentStreamKey ? streams?.get?.(parentStreamKey) ?? null : null;
    return {
      view: "session",
      preferredView: desired,
      hasSessionAccess,
      hasStreamAccess,
      sessionKey: previewSessionKey,
      session,
      streamKey: parentStreamKey,
      stream,
      terminal: null,
      isPreview: true,
      isUnopened: true,
      canReturnToCurrent: Boolean(contextStreamKey),
      isEnrolled,
      currentStreamKey: contextStreamKey,
      currentStream: contextStream,
      currentSession,
    };
  }

  return {
    view: "session",
    preferredView: desired,
    hasSessionAccess: true,
    hasStreamAccess: true,
    sessionKey: currentSessionKey,
    session: currentSession,
    streamKey: currentStreamKey,
    stream: currentTabStream,
    terminal: live?.activeTerm ?? null,
    isPreview: false,
    isUnopened: false,
    canReturnToCurrent: false,
    isEnrolled: true,
    currentStreamKey: contextStreamKey,
    currentStream: contextStream,
    currentSession,
  };
}

/**
 * Determine whether the active `herdr-app` tab should be renamed to match its enrolled Session title.
 */
export function resolveTabRenameTarget({
  activeTab,
  visibleTerminals = [],
  runningByTerm,
  sessions,
  tabNaming = true,
}) {
  if (!tabNaming || !activeTab?.id) return null;
  for (const termKey of visibleTerminals) {
    const sessionKey = runningByTerm?.get?.(termKey);
    const session = sessionKey ? sessions?.get?.(sessionKey) : null;
    if (session?.title) {
      const desired = session.title.trim();
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
const VALID_PREFERRED_VIEWS = new Set(["session", "stream", "all"]);
const VALID_HOST_SCOPES = new Set(["local", "all"]);

export class StreamsCoordinator {
  constructor({
    exec,
    notify,
    publishShared = null,
    notifyToast = null,
    onPreferenceChange = null,
    initialPreferredView = "session",
    initialHostScope = "local",
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
    this.onPreferenceChange = onPreferenceChange;
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
    this._globalEpoch = 0;
    this._machineEpoch = new Map(); // machineKey -> int (bumped on mutation / machine transition)
    this._savedBySession = new Map(); // sessionKey -> sorted "addr=saved" from running Pi terminals
    this._shellSavedByMachine = new Map(); // machineKey -> sorted "addr=saved" from non-Pi terminals
    this._liveStreamBySession = new Map(); // sessionKey -> streamKey from live terminal `streams.stream` label
    this._lastFrameSig = null;
    this.inFlightKey = null;
    this.inFlightPromise = null;
    this.lastCompletedKey = null;
    this.lastFailedKey = null;
    this.lastCompletedAt = 0;

    this.sessions = new Map(); // sessionKey -> normalized Session & { detail, fetchedToken, detailToken }
    this.streams = new Map(); // streamKey -> normalized Stream & { detail, detailToken }
    this.lists = new Map(); // `${machineKey}:${scope}` -> { keys, unreadable, error, loaded, fetchedToken, seq, inFlight }
    this.staleSessionKeys = new Set(); // sessionKeys that returned CLI `not-found`
    this.lastFocusedStreamBySpace = new Map(); // spaceId -> streamKey
    this.activeMachine = null; // null for local desktop, or remote machine name
    this.machineLayouts = new Map(); // machineName -> "loading" | "ready" | "unavailable"
    this._hasMachineSnapshot = false;
    this.hostScope = VALID_HOST_SCOPES.has(initialHostScope) ? initialHostScope : "local"; // "local" | "all"
    this.hostToMachine = new Map(); // hostId -> machineName | null
    this.machineToHost = new Map(); // machineKey -> hostId
    this._identityProbes = new Map(); // machineKey -> in-flight identity Promise
    this.homeDirByMachine = new Map(); // machineName -> remote $HOME
    this._homeDirPromises = new Map(); // machineName -> Promise<string | null>
    this.preferredView = VALID_PREFERRED_VIEWS.has(initialPreferredView)
      ? initialPreferredView
      : "session"; // "session" | "stream" | "all"
    this.previewStreamKey = null; // streamKey
    this.previewSessionKey = null; // sessionKey
    this.readError = null;
    this.actionError = null;

    this.lastSpaceTabKey = null;
    this._lastPublishedSharedJson = null;
    this._lastRunning = new Map();
    this.pending = new Map(); // key -> true | label string
  }

  _machineKey(machine = this.activeMachine) {
    return machine ?? "__local__";
  }

  _bumpMachineEpoch(machine) {
    const mKey = this._machineKey(machine);
    this._machineEpoch.set(mKey, (this._machineEpoch.get(mKey) ?? 0) + 1);
  }

  /**
   * Snapshot live `streams.saved` and `streams.stream` labels into per-session and per-machine
   * maps so every cached record (`sessions`, `streams`, `lists`) shares one uniform freshness check.
   */
  _recordLiveTokens(live) {
    const bySession = new Map();
    const shellByMachine = new Map();
    this._liveStreamBySession.clear();
    for (const [addr, sk] of live?.runningByTerm ?? []) {
      const st = live.termByAddr?.get(addr)?.labels?.[LABEL_STREAM];
      if (st) this._liveStreamBySession.set(sk, st);
    }
    for (const [addr, saved] of live?.savedTokensByTerm ?? []) {
      const sk = live.runningByTerm.get(addr);
      if (sk) {
        const list = bySession.get(sk) ?? [];
        list.push(`${addr}=${saved}`);
        bySession.set(sk, list);
      } else {
        const mKey = this._machineKey(parseTerminalAddress(addr)?.machine ?? null);
        const list = shellByMachine.get(mKey) ?? [];
        list.push(`${addr}=${saved}`);
        shellByMachine.set(mKey, list);
      }
    }
    this._savedBySession.clear();
    for (const [sk, list] of bySession) {
      this._savedBySession.set(sk, list.sort().join("|"));
    }
    this._shellSavedByMachine.clear();
    for (const [mKey, list] of shellByMachine) {
      this._shellSavedByMachine.set(mKey, list.sort().join("|"));
    }
  }

  _machineBaseToken(machine = this.activeMachine) {
    const mKey = this._machineKey(machine);
    return `${this._globalEpoch}:${this._machineEpoch.get(mKey) ?? 0}:${this._shellSavedByMachine.get(mKey) ?? ""}`;
  }

  _sessionToken(sessionKey, machine = this.activeMachine) {
    return `${this._machineBaseToken(machine)}|${this._savedBySession.get(sessionKey) ?? ""}`;
  }

  _streamToken(streamKey, machine = this.activeMachine) {
    const memberSaved = [];
    for (const [sk, tok] of this._savedBySession) {
      const st = this.sessions.get(sk)?.stream ?? this._liveStreamBySession.get(sk);
      if (st === streamKey) memberSaved.push(`${sk}=${tok}`);
    }
    memberSaved.sort();
    return `${this._machineBaseToken(machine)}|${memberSaved.join("|")}`;
  }

  _listToken(machine = this.activeMachine, scope = this.hostScope) {
    if (scope === "all") {
      const allKeys = new Set([
        this._machineKey(machine),
        ...this._machineEpoch.keys(),
        ...this._shellSavedByMachine.keys(),
      ]);
      const machinesPart = [...allKeys]
        .sort()
        .map((k) => `${k}=${this._machineEpoch.get(k) ?? 0}:${this._shellSavedByMachine.get(k) ?? ""}`)
        .join(";");
      const sessPart = [...this._savedBySession.entries()]
        .map(([k, v]) => `${k}=${v}`)
        .sort()
        .join("|");
      return `${this._globalEpoch}|${machinesPart}|${sessPart}`;
    }
    const sessPart = [];
    for (const [addr, sk] of this._lastRunning) {
      if ((parseTerminalAddress(addr)?.machine ?? null) !== machine) continue;
      const tok = this._savedBySession.get(sk);
      if (tok) sessPart.push(`${sk}=${tok}`);
    }
    sessPart.sort();
    return `${this._machineBaseToken(machine)}|${sessPart.join("|")}`;
  }

  _listBucketKey(machine = this.activeMachine, scope = this.hostScope) {
    const normalizedScope = scope === "all" ? "all" : "local";
    return `${this._machineKey(machine)}:${normalizedScope}`;
  }

  _getListEntry(machine = this.activeMachine, scope = this.hostScope) {
    const key = this._listBucketKey(machine, scope);
    let entry = this.lists.get(key);
    if (!entry) {
      entry = {
        keys: [],
        unreadable: [],
        error: null,
        loaded: false,
        fetchedToken: null,
        seq: 0,
        inFlight: 0,
      };
      this.lists.set(key, entry);
    }
    return entry;
  }

  getStreamList(machine = this.activeMachine, scope = this.hostScope) {
    const entry = this._getListEntry(machine, scope);
    return {
      items: entry.keys.map((k) => this.streams.get(k)).filter(Boolean),
      unreadable: entry.unreadable,
      error: entry.error,
      loaded: entry.loaded,
    };
  }

  /**
   * Return the member Sessions of a Stream resolved from the canonical `this.sessions` map
   * so off-tab, preview, and mutation updates always appear in Stream view immediately.
   */
  getStreamSessions(streamKey) {
    const stream = streamKey ? this.streams.get(streamKey) : null;
    if (!stream) return [];
    if (Array.isArray(stream.sessionKeys) && stream.sessionKeys.length > 0) {
      return stream.sessionKeys.map((k) => this.sessions.get(k)).filter(Boolean);
    }
    return Array.isArray(stream.sessions)
      ? stream.sessions.map((s) => (s?.key && this.sessions.get(s.key)) || s).filter(Boolean)
      : [];
  }

  _upsertSession(session, { detail = false, token = null } = {}) {
    if (!session?.key) return null;
    this.staleSessionKeys.delete(session.key);
    const prev = this.sessions.get(session.key);
    if (detail || !prev?.detail) {
      const stored = {
        ...session,
        detail: Boolean(detail),
        fetchedToken: token ?? prev?.fetchedToken ?? null,
        detailToken: detail ? token : prev?.detailToken ?? null,
      };
      this.sessions.set(session.key, stored);
      return stored;
    }
    // Preserve full detail fields and `detailToken` when a summary arrives from `session list`
    // or `stream.sessions` so switching to this Session later still refetches stale details.
    const merged = {
      ...prev,
      title: session.title || prev.title,
      title_manual: session.title_manual || prev.title_manual,
      status: session.status || prev.status,
      state: session.state !== null && session.state !== undefined ? session.state : prev.state,
      state_short:
        session.state_short !== null && session.state_short !== undefined
          ? session.state_short
          : session.state !== null && session.state !== undefined
            ? session.state
            : prev.state_short,
      recap_short:
        session.recap_short !== null && session.recap_short !== undefined
          ? session.recap_short
          : prev.recap_short,
      stream: session.stream || prev.stream,
      stream_id: session.stream_id || prev.stream_id,
      host_id: session.host_id || prev.host_id,
      updated_at_ms: session.updated_at_ms > 0 ? session.updated_at_ms : prev.updated_at_ms,
      directory: session.directory ?? prev.directory,
      detail: true,
      fetchedToken: token ?? prev.fetchedToken ?? null,
      detailToken: prev.detailToken ?? null,
    };
    this.sessions.set(session.key, merged);
    return merged;
  }

  _upsertStream(stream, { detail = false, token = null, machine = undefined } = {}) {
    if (!stream?.key) return null;
    const prev = this.streams.get(stream.key);
    let stored;
    if (detail || !prev?.detail) {
      const sessionKeys = Array.isArray(stream.sessions)
        ? stream.sessions.map((s) => s?.key).filter(Boolean)
        : [];
      stored = {
        ...stream,
        sessionKeys,
        detail: Boolean(detail),
        detailToken: detail ? token : prev?.detailToken ?? null,
      };
    } else {
      stored = {
        ...prev,
        title: stream.title || prev.title,
        status: stream.status || prev.status,
        goal_short:
          stream.goal_short !== null && stream.goal_short !== undefined
            ? stream.goal_short
            : prev.goal_short,
        active_sessions:
          typeof stream.active_sessions === "number" ? stream.active_sessions : prev.active_sessions,
        total_sessions:
          typeof stream.total_sessions === "number" ? stream.total_sessions : prev.total_sessions,
        last_activity_ms:
          stream.last_activity_ms > 0 ? stream.last_activity_ms : prev.last_activity_ms,
        host_id: stream.host_id || prev.host_id,
        detail: true,
        detailToken: prev.detailToken ?? null,
      };
    }
    this.streams.set(stream.key, stored);
    if (detail && Array.isArray(stream.sessions)) {
      for (const member of stream.sessions) {
        this._upsertSession(member, {
          detail: false,
          token: machine !== undefined ? this._sessionToken(member.key, machine) : null,
        });
      }
    }
    return stored;
  }

  async _fetchSession(sessionKey, machine, { gen = null, fallbackError = "session show failed" } = {}) {
    const token = this._sessionToken(sessionKey, machine);
    const out = await this.runExec(
      this.streamsBinFor(machine),
      ["session", "show", sessionKey, "--json"],
      { on: machine }
    );
    if (gen !== null && gen !== this.readGeneration) return { ok: false, stale: true };
    if (out.code !== 0) {
      const err = extractCliError(out, fallbackError);
      return { ok: false, notFound: !out.transportError && err.kind === "not-found", error: err.message };
    }
    const session = normalizeSession(parseJsonSafe(out.stdout));
    if (!session) return { ok: false, error: "invalid session show JSON" };
    if (session.host_id) this.recordHostMachine(session.host_id, machine);
    this._upsertSession(session, { detail: true, token });
    return { ok: true, session };
  }

  async _fetchStream(streamKey, machine, { gen = null, fallbackError = "stream show failed" } = {}) {
    const token = this._streamToken(streamKey, machine);
    const out = await this.runExec(
      this.streamsBinFor(machine),
      ["stream", "show", streamKey, "--json"],
      { on: machine }
    );
    if (gen !== null && gen !== this.readGeneration) return { ok: false, stale: true };
    if (out.code !== 0) {
      const err = extractCliError(out, fallbackError);
      return { ok: false, error: err.message };
    }
    const stream = normalizeStream(parseJsonSafe(out.stdout));
    if (!stream) return { ok: false, error: "invalid stream show JSON" };
    if (stream.host_id) this.recordHostMachine(stream.host_id, machine);
    this._upsertStream(stream, { detail: true, token, machine });
    return { ok: true, stream };
  }

  /**
   * Synchronously reconcile active machine, detached previews, and Space stream memory against
   * the current live snapshot BEFORE resolving the sidebar view (zero 1-frame stale flashes).
   */
  syncContext(live) {
    if (!live) return;
    this.activeMachine = live.activeSpace?.machine ?? null;
    const spaceTabKey = `${live.activeSpace?.id ?? ""}:${live.activeTabId ?? ""}`;
    if (this.lastSpaceTabKey !== null && spaceTabKey !== this.lastSpaceTabKey) {
      this.previewStreamKey = null;
      this.previewSessionKey = null;
    }
    this.lastSpaceTabKey = spaceTabKey;
    if (live.activeSpace?.id && live.activeStreamKey) {
      this.lastFocusedStreamBySpace.set(live.activeSpace.id, live.activeStreamKey);
    }
  }

  isPending(key) {
    return this.pending.has(key);
  }

  pendingValue(key) {
    return this.pending.get(key) ?? null;
  }

  async _withPending(key, value, fn) {
    if (this.pending.has(key)) return { ok: false, duplicate: true };
    this.pending.set(key, value);
    this.notifyViews();
    try {
      return await fn();
    } finally {
      this.pending.delete(key);
      this.notifyViews();
    }
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
  }

  machineForHost(hostId) {
    if (!hostId || !this.hostToMachine.has(hostId)) return undefined;
    return this.hostToMachine.get(hostId);
  }

  /**
   * Discover the owning `host_id` of `machine` via `streams catalog label --json` (authoritative
   * even when the machine's catalog has zero Streams). Deduplicates concurrent calls per machine.
   */
  async ensureMachineIdentity(machine = this.activeMachine) {
    const targetMachine = machine ?? null;
    const mKey = this._machineKey(targetMachine);
    if (this.machineToHost.has(mKey) || !this.isMachineReady(targetMachine)) {
      return { ok: true };
    }
    if (this._identityProbes.has(mKey)) {
      return this._identityProbes.get(mKey);
    }
    const promise = (async () => {
      let learned = false;
      try {
        const out = await this.runExec(
          this.streamsBinFor(targetMachine),
          ["catalog", "label", "--json"],
          { on: targetMachine }
        );
        if (out.code === 0) {
          const parsed = parseJsonSafe(out.stdout);
          if (typeof parsed?.host_id === "string" && parsed.host_id.length > 0) {
            this.recordHostMachine(parsed.host_id, targetMachine);
            learned = true;
          }
          return { ok: true };
        }
        if (out.transportError) {
          return {
            ok: false,
            error: extractCliError(out, "could not reach active host").message,
          };
        }
        return { ok: true };
      } finally {
        this._identityProbes.delete(mKey);
        if (learned) {
          this.notifyViews("sidebar");
        }
      }
    })();
    this._identityProbes.set(mKey, promise);
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
        ? recordOrKey.includes("/")
          ? recordHostId(recordOrKey)
          : recordOrKey.trim() || null
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

    if (activeHostId && !this._identityProbes.has(mKey)) {
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
   * Requires an explicit `hostId -> machine` mapping. If the active machine's identity has not
   * yet been queried, runs one bounded `streams catalog label --json` on `activeMachine`.
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
    const probeRes = await this.ensureMachineIdentity(activeMachine);
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
            this.notifyViews("all");
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
    this._getListEntry().error = null;
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
      sessions: this.sessions,
      running,
      staleSessionKeys: this.staleSessionKeys,
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

  _setPreferredView(view, { persist = false } = {}) {
    if (!VALID_PREFERRED_VIEWS.has(view)) return;
    if (this.preferredView === view) return;
    this.preferredView = view;
    if (persist) {
      this.onPreferenceChange?.("preferredView", view);
    }
  }

  _setHostScopeValue(scope) {
    const normalized = scope === "all" ? "all" : "local";
    if (this.hostScope === normalized) return false;
    this.hostScope = normalized;
    this.onPreferenceChange?.("hostScope", normalized);
    return true;
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
    notify = true,
  } = {}) {
    const targetMachine = machine ?? null;
    const targetScope = scope === "all" ? "all" : "local";
    const bucket = this._getListEntry(targetMachine, targetScope);
    const token = this._listToken(targetMachine, targetScope);
    if (!force && bucket.loaded && bucket.fetchedToken === token) {
      return { ok: true, items: this.getStreamList(targetMachine, targetScope).items, bucket };
    }
    const reqSeq = ++bucket.seq;
    if (!this.isMachineReady(targetMachine)) {
      const msg = `${targetMachine} is unavailable`;
      if (bucket.seq === reqSeq) {
        bucket.error = msg;
        if (notify) this.notifyViews("sidebar");
      }
      return { ok: false, error: msg, bucket };
    }

    const args = ["stream", "list"];
    if (targetScope === "local") args.push("--local");
    args.push("--all", "--json", "--fields", HOST_STREAM_LIST_FIELDS);

    const mKey = this._machineKey(targetMachine);
    bucket.inFlight += 1;
    let listOut;
    try {
      const listPromise = this.runExec(this.streamsBinFor(targetMachine), args, {
        on: targetMachine,
      });
      if (targetScope === "all" && !this.machineToHost.has(mKey)) {
        void this.ensureMachineIdentity(targetMachine);
      }
      listOut = await listPromise;
    } finally {
      bucket.inFlight = Math.max(0, bucket.inFlight - 1);
    }
    if (bucket.seq !== reqSeq) {
      return { ok: false, stale: true, bucket };
    }
    if (listOut.code !== 0) {
      const err = extractCliError(listOut, "host Streams are unavailable");
      bucket.error = err.message;
      if (notify) this.notifyViews("sidebar");
      return { ok: false, error: err.message, bucket };
    }
    const parsed = parseJsonSafe(listOut.stdout);
    if (!parsed || !Array.isArray(parsed.items)) {
      bucket.error = "unexpected stream list response from streams";
      if (notify) this.notifyViews("sidebar");
      return { ok: false, error: bucket.error, bucket };
    }
    const items = parsed.items.map(normalizeStream).filter(Boolean);
    if (targetScope === "local") {
      for (const item of items) {
        if (item.host_id) this.recordHostMachine(item.host_id, targetMachine);
      }
    }
    const keys = [];
    for (const item of items) {
      this._upsertStream(item, { detail: false });
      keys.push(item.key);
    }
    bucket.keys = keys;
    bucket.unreadable = Array.isArray(parsed.unreadable) ? parsed.unreadable : [];
    bucket.error = null;
    bucket.loaded = true;
    bucket.fetchedToken = token;
    this.publishSharedTerminals();
    if (notify) this.notifyViews("auto");
    return { ok: true, items: this.getStreamList(targetMachine, targetScope).items, bucket };
  }

  async ensureHostStreams(
    force = false,
    { machine = this.activeMachine, scope = this.hostScope } = {}
  ) {
    const res = await this._fetchHostStreams({ machine, scope, force });
    return res.ok ? res.items : null;
  }

  setHostScope(scope) {
    if (!this._setHostScopeValue(scope)) {
      return this.ensureHostStreams(false);
    }
    this.notifyViews();
    return this.ensureHostStreams(false);
  }

  setView(view) {
    this._setPreferredView(view, { persist: true });
    if (view === "all") {
      this.previewStreamKey = null;
      this.previewSessionKey = null;
      this.notifyViews();
      return this.ensureHostStreams(false);
    }
    if (view === "session") {
      this.previewStreamKey = null;
      this.previewSessionKey = null;
    } else if (view === "stream") {
      this.previewSessionKey = null;
    }
    this.notifyViews();
    return Promise.resolve(null);
  }

  returnToCurrent({ isEnrolled = true } = {}) {
    this.previewStreamKey = null;
    this.previewSessionKey = null;
    this._setPreferredView(isEnrolled ? "session" : "stream");
    this.notifyViews();
  }

  backToStream() {
    this.previewSessionKey = null;
    this._setPreferredView("stream");
    this.notifyViews();
  }

  async inspectStream(streamKey) {
    if (!streamKey) {
      this.previewStreamKey = null;
      this.previewSessionKey = null;
      this._setPreferredView("all");
      this.notifyViews();
      return null;
    }
    this.previewStreamKey = streamKey;
    this.previewSessionKey = null;
    this._setPreferredView("stream");
    this.notifyViews();
    const target = this.resolveReadMachineForKey(streamKey);
    if (target.unavailable) {
      this.reportActionError("Could not load Stream", `${target.unavailable} is offline`);
      return null;
    }
    const res = await this._fetchStream(streamKey, target.machine, {
      fallbackError: "saved Stream view is unavailable",
    });
    if (!res.ok) {
      this.reportActionError("Could not load Stream", res.error);
      return null;
    }
    this.actionError = null;
    this.notifyViews("auto");
    return res.stream;
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
    }
    this._setPreferredView("session");
    this.notifyViews();
    const target = this.resolveReadMachineForKey(sessionKey);
    if (target.unavailable) {
      this.reportActionError("Could not load Session", `${target.unavailable} is offline`);
      return null;
    }
    const res = await this._fetchSession(sessionKey, target.machine, {
      fallbackError: "saved Session view is unavailable",
    });
    if (!res.ok) {
      this.reportActionError("Could not load Session", res.error);
      return null;
    }
    if (!this.previewStreamKey && res.session.stream) {
      this.previewStreamKey = res.session.stream;
    }
    this.actionError = null;
    this.notifyViews("auto");
    return res.session;
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
      sessions: this.sessions,
    });
    if (live) {
      this._setPreferredView("stream");
      this.previewStreamKey = null;
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
      this._setPreferredView("session");
      this.previewStreamKey = null;
      this.previewSessionKey = null;
      run?.("pane.focus", { pane: live.pane });
      this.notifyViews();
      return { focused: true, pane: live.pane, terminal: live.terminal };
    }
    const session = await this.previewSession(sessionKey, streamKey);
    return { focused: false, session };
  }

  _invalidateForMutation(machine = undefined) {
    const activeBucket = this._getListEntry();
    const shouldRefetchActive = activeBucket.inFlight > 0 && !activeBucket.loaded;
    this.readGeneration += 1;
    if (machine !== undefined) {
      this._bumpMachineEpoch(machine);
    } else {
      this._globalEpoch += 1;
    }
    this.inFlightKey = null;
    this.inFlightPromise = null;
    for (const bucket of this.lists.values()) {
      bucket.seq += 1;
    }
    if (shouldRefetchActive) {
      void this._fetchHostStreams({ force: true });
    }
  }

  /**
   * Targeted, authoritative post-mutation refresh: invalidates pre-mutation reads and epochs,
   * re-reads the affected Session and Stream(s), republishes `terminal.marks`, and wakes views.
   */
  async refreshAfterMutation({
    sessionKey = null,
    streamKey = null,
    extraStreamKeys = [],
    machine = undefined,
  } = {}) {
    const readTarget =
      machine !== undefined
        ? { machine, unavailable: this.isMachineReady(machine) ? null : machine }
        : this.resolveReadMachineForKey(sessionKey ?? streamKey);
    const targetMachine = readTarget.unavailable ? null : readTarget.machine;
    this._invalidateForMutation(readTarget.unavailable ? undefined : targetMachine);

    let resolvedStreamKey = streamKey ?? null;
    if (sessionKey && !readTarget.unavailable) {
      const res = await this._fetchSession(sessionKey, targetMachine);
      if (res.ok && !resolvedStreamKey && res.session.stream) {
        resolvedStreamKey = res.session.stream;
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
      if (streamTarget.unavailable) continue;
      await this._fetchStream(stKey, streamTarget.machine);
    }

    this.publishSharedTerminals();
    this.notifyViews("auto");
  }

  async _mutate({
    pendingKey,
    pendingValue = true,
    entityKey,
    errorTitle,
    fallbackError,
    args,
    refreshSessionKey = null,
    refreshStreamKey = null,
    extraStreamKeys = [],
    resultExtra = null,
  }) {
    return this._withPending(pendingKey, pendingValue, async () => {
      const target = await this.ensureWriteMachineForKey(entityKey);
      if (!target.ok) {
        this.reportActionError(errorTitle, target.error);
        return { ok: false, error: target.error };
      }
      const out = await this.runExec(this.streamsBinFor(target.machine), args, {
        on: target.machine,
      });
      if (out.code !== 0) {
        const err = extractCliError(out, fallbackError);
        this.reportActionError(errorTitle, err.message);
        return { ok: false, error: err.message };
      }
      this.actionError = null;
      this.lastCompletedKey = null;
      await this.refreshAfterMutation({
        sessionKey: refreshSessionKey,
        streamKey: refreshStreamKey,
        extraStreamKeys,
        machine: target.machine,
      });
      return { ok: true, ...(resultExtra ?? {}) };
    });
  }

  async dropReference(sessionKey, refKey, streamKey = null) {
    if (!sessionKey || !refKey) return { ok: false, error: "missing session or reference key" };
    return this._mutate({
      pendingKey: `drop:${sessionKey}\0${refKey}`,
      entityKey: sessionKey,
      errorTitle: "Could not drop reference",
      fallbackError: "could not drop reference",
      args: ["session", "edit", sessionKey, "--drop-reference", refKey],
      refreshSessionKey: sessionKey,
      refreshStreamKey: streamKey,
    });
  }

  async setSessionStatus(sessionKey, status, streamKey = null) {
    if (!sessionKey || !SESSION_STATUSES.includes(status)) {
      const msg = `invalid status: ${status}`;
      this.reportActionError("Could not update status", msg);
      return { ok: false, error: msg };
    }
    return this._mutate({
      pendingKey: `status:${sessionKey}`,
      entityKey: sessionKey,
      errorTitle: "Could not update status",
      fallbackError: "could not update Session status",
      args: ["session", "edit", sessionKey, "--status", status],
      refreshSessionKey: sessionKey,
      refreshStreamKey: streamKey,
    });
  }

  async renameSession(sessionKey, newTitle, streamKey = null) {
    const trimmed = typeof newTitle === "string" ? newTitle.trim() : "";
    if (!sessionKey || !trimmed) {
      const msg = !sessionKey ? "missing Session key" : "Title cannot be empty.";
      this.reportActionError("Could not rename Session", msg);
      return { ok: false, error: msg };
    }
    return this._mutate({
      pendingKey: `rename:${sessionKey}`,
      entityKey: sessionKey,
      errorTitle: "Could not rename Session",
      fallbackError: "could not rename Session",
      args: ["session", "edit", sessionKey, "--title", trimmed],
      refreshSessionKey: sessionKey,
      refreshStreamKey: streamKey,
      resultExtra: { title: trimmed },
    });
  }

  async renameStream(streamKey, newTitle) {
    const trimmed = typeof newTitle === "string" ? newTitle.trim() : "";
    if (!streamKey || !trimmed) {
      const msg = !streamKey ? "missing Stream key" : "Title cannot be empty.";
      this.reportActionError("Could not rename Stream", msg);
      return { ok: false, error: msg };
    }
    return this._mutate({
      pendingKey: `rename:${streamKey}`,
      entityKey: streamKey,
      errorTitle: "Could not rename Stream",
      fallbackError: "could not rename Stream",
      args: ["stream", "edit", streamKey, "--title", trimmed],
      refreshStreamKey: streamKey,
      resultExtra: { title: trimmed },
    });
  }

  async moveSessionToStream({
    sessionKey,
    sourceStreamKey = null,
    targetStreamKey,
    live = null,
    terminalList = [],
    panes = [],
    agents = [],
  } = {}) {
    if (!sessionKey || !targetStreamKey) {
      const msg = "missing Session or target Stream key";
      this.reportActionError("Could not move Session", msg);
      return { ok: false, error: msg };
    }
    if (sourceStreamKey && targetStreamKey === sourceStreamKey) {
      return { ok: true };
    }
    const sessHost = recordHostId(sessionKey);
    const targetHost = recordHostId(targetStreamKey);
    if (sessHost && targetHost && sessHost !== targetHost) {
      const msg = "Sessions can only move to a Stream on the same host.";
      this.reportActionError("Could not move Session", msg);
      return { ok: false, error: msg };
    }

    return this._withPending(`move:${sessionKey}`, true, async () => {
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

      const liveTermAddress =
        live?.paneBySession?.get?.(sessionKey)?.terminal ??
        findLivePaneForSession({ sessionKey, panes, terminalList, agents })?.terminal ??
        null;
      if (liveTermAddress) {
        const labelOut = await this.runExec(this.hcBin, [
          "label",
          liveTermAddress,
          `${LABEL_STREAM}=${targetStreamKey}`,
        ]);
        if (labelOut.code !== 0) {
          const err = extractCliError(labelOut, "could not update terminal label");
          this.reportActionError("Moved Session, label unchanged", err.message, {
            level: "warning",
          });
        } else {
          this.actionError = null;
        }
      } else {
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
      return { ok: true };
    });
  }

  async _bulkSetStreamSessionsStatus(streamKey, targetStatus, predicate) {
    if (!streamKey) return { ok: false, error: "missing Stream key", updated: [] };
    const errorTitle =
      targetStatus === "parked" ? "Could not park all sessions" : "Could not close all sessions";
    const pendingLabel = targetStatus === "parked" ? "Parking…" : "Closing…";
    return this._withPending(`stream:${streamKey}`, pendingLabel, async () => {
      const target = await this.ensureWriteMachineForKey(streamKey);
      if (!target.ok) {
        this.reportActionError(errorTitle, target.error);
        return { ok: false, error: target.error, updated: [] };
      }
      const targetMachine = target.machine;
      const bin = this.streamsBinFor(targetMachine);
      let stream = this.streams.get(streamKey) ?? null;
      if (!stream || !stream.detail) {
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
        this._upsertStream(stream, { detail: true });
      }
      const targets = (stream?.sessions ?? []).filter(predicate);
      if (targets.length === 0) {
        return { ok: true, updated: [] };
      }
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
    return this._withPending(`stream:${streamKey}`, "Deleting…", async () => {
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
      for (const bucket of this.lists.values()) {
        bucket.keys = bucket.keys.filter((k) => k !== streamKey);
      }
      this._invalidateForMutation(target.machine);
      if (this.previewStreamKey === streamKey) {
        this.previewStreamKey = null;
        this.previewSessionKey = null;
        this._setPreferredView("all");
      }
      this.lastCompletedKey = null;
      this.publishSharedTerminals();
      this.notifyViews("auto");
      return { ok: true, key: streamKey };
    });
  }

  streamTitle(streamKey) {
    if (!streamKey) return "";
    return this.streams.get(streamKey)?.title || streamKey;
  }

  _computeFrameSig(running = this._lastRunning) {
    if (!running || running.size === 0) return "";
    const parts = [];
    for (const [term, sk] of running) {
      if (this.staleSessionKeys.has(sk)) continue;
      const sess = this.sessions.get(sk);
      const stTitle = sess?.stream ? this.streamTitle(sess.stream) : "";
      parts.push(
        `${term}:${sk}:${sess?.title ?? ""}:${sess?.status ?? ""}:${sess?.state ?? ""}:${sess?.stream ?? ""}:${stTitle}`
      );
    }
    parts.sort();
    return parts.join("|");
  }

  /**
   * Wake mounted views. Defaults to `"sidebar"` so sidebar-only UI updates (menus, pending
   * states, previews, errors) never redraw pane headers; `"auto"` checks whether any live
   * terminal's Session/Stream header info actually changed and wakes `"all"` only when it did.
   */
  notifyViews(scope = "sidebar") {
    if (!this.notify) return;
    if (scope === "auto") {
      const frameSig = this._computeFrameSig();
      if (frameSig !== this._lastFrameSig) {
        this._lastFrameSig = frameSig;
        this.notify("all");
      } else {
        this.notify("sidebar");
      }
      return;
    }
    this.notify(scope);
  }

  /**
   * Scoped fetcher 1: ensure the active tab's focused enrolled Session and its Stream have fresh
   * detail for the current cache tokens.
   */
  async _syncActiveTarget({ live, activeMachine, gen }) {
    const running = live.runningByTerm;
    const visibleTerminals = live.visibleTerminals;
    const focusedTerminal = live.focusedTerminal;

    const candidates = [];
    if (
      focusedTerminal &&
      visibleTerminals.includes(focusedTerminal) &&
      running.has(focusedTerminal)
    ) {
      candidates.push(focusedTerminal);
    }
    for (const term of visibleTerminals) {
      if (running.has(term) && !candidates.includes(term)) {
        candidates.push(term);
      }
    }

    let primaryTerm = null;
    let primarySession = null;
    for (const term of candidates) {
      const sessionKey = running.get(term);
      const cached = this.sessions.get(sessionKey);
      if (cached?.detail && cached.detailToken === this._sessionToken(sessionKey, activeMachine)) {
        primaryTerm = term;
        primarySession = cached;
        break;
      }
      const res = await this._fetchSession(sessionKey, activeMachine, { gen });
      if (res.stale) return { ok: false, stale: true };
      if (res.ok) {
        primaryTerm = term;
        primarySession = res.session;
        break;
      }
      if (res.notFound) {
        this.staleSessionKeys.add(sessionKey);
        continue;
      }
      return { ok: false, error: res.error, skipTerm: null };
    }

    let activeStream = null;
    if (primarySession) {
      const streamKey = primarySession.stream || `${primarySession.host_id}/${primarySession.stream_id}`;
      const cachedStream = this.streams.get(streamKey);
      if (cachedStream?.detail && cachedStream.detailToken === this._streamToken(streamKey, activeMachine)) {
        activeStream = cachedStream;
      } else {
        const res = await this._fetchStream(streamKey, activeMachine, { gen });
        if (res.stale) return { ok: false, stale: true };
        if (!res.ok) return { ok: false, error: res.error, skipTerm: primaryTerm };
        activeStream = res.stream;
      }
      if (live.activeSpace?.id && activeStream?.key) {
        this.lastFocusedStreamBySpace.set(live.activeSpace.id, activeStream.key);
      }
    }

    return { ok: true, primaryTerm, primarySession, activeStream };
  }

  /**
   * Scoped fetcher 2: batch-refresh off-tab live enrolled terminals grouped by machine,
   * skipping offline/loading machines and any session whose `fetchedToken` is already current.
   */
  async _syncOffTabMarks({ live, skipTerm = null, skipMachines = new Set(), gen }) {
    const missingByMachine = new Map();
    for (const [term, sk] of live.runningByTerm) {
      if (term === skipTerm || this.staleSessionKeys.has(sk)) continue;
      const termMachine = parseTerminalAddress(term)?.machine ?? null;
      if (skipMachines.has(termMachine) || !this.isMachineReady(termMachine)) continue;
      const token = this._sessionToken(sk, termMachine);
      if (this.sessions.get(sk)?.fetchedToken === token) continue;
      const byKey = missingByMachine.get(termMachine) ?? new Map();
      byKey.set(sk, token);
      missingByMachine.set(termMachine, byKey);
    }

    for (const [termMachine, keyTokens] of missingByMachine) {
      const listArgs = ["session", "list"];
      for (const sk of keyTokens.keys()) {
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
          if (sess) {
            this._upsertSession(sess, { detail: false, token: keyTokens.get(sess.key) ?? null });
          }
        }
      }
    }
    return true;
  }

  /**
   * Scoped fetcher 3: refresh detached preview Stream/Session when their cache token changed.
   */
  async _syncPreviews({ activeStreamKey, gen }) {
    if (this.previewStreamKey && this.previewStreamKey !== activeStreamKey) {
      const pTarget = this.resolveReadMachineForKey(this.previewStreamKey);
      if (!pTarget.unavailable) {
        const cached = this.streams.get(this.previewStreamKey);
        if (!(cached?.detail && cached.detailToken === this._streamToken(this.previewStreamKey, pTarget.machine))) {
          const res = await this._fetchStream(this.previewStreamKey, pTarget.machine, { gen });
          if (res.stale) return;
        }
      }
    }
    if (this.previewSessionKey) {
      const sTarget = this.resolveReadMachineForKey(this.previewSessionKey);
      if (!sTarget.unavailable) {
        const cached = this.sessions.get(this.previewSessionKey);
        if (!(cached?.detail && cached.detailToken === this._sessionToken(this.previewSessionKey, sTarget.machine))) {
          await this._fetchSession(this.previewSessionKey, sTarget.machine, { gen });
        }
      }
    }
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

  _buildObservation({
    activeSpace,
    activeTabId = null,
    visibleTerminals = [],
    focusedTerminal = null,
    terminalList = [],
    panes = [],
    agents = [],
    machineList,
  }, live = null) {
    const resolvedLive =
      live ??
      indexLive(
        { activeSpace, activeTabId, visibleTerminals, focusedTerminal, terminalList, panes, agents },
        {
          sessions: this.sessions,
          staleSessionKeys: this.staleSessionKeys,
          lastFocusedStreamBySpace: this.lastFocusedStreamBySpace,
        }
      );
    const running = resolvedLive.runningByTerm;
    const visible = resolvedLive.visibleTerminals;
    const terminalSessions = visible
      .filter((t) => running.has(t))
      .map((t) => `${t}=${running.get(t)}`);
    const allRunningSessions = [...running.entries()]
      .map(([t, s]) => `${t}=${s}`)
      .sort();
    const machineStates = Array.isArray(machineList)
      ? machineList
          .filter((m) => m && typeof m.name === "string" && m.name.length > 0)
          .map((m) => `${m.name}=${m.layout ?? "ready"}`)
          .sort()
      : this._hasMachineSnapshot
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
    return {
      live: resolvedLive,
      observation,
      key: computeObservationKey(observation),
    };
  }

  /**
   * Compute the canonical observation key for a desktop snapshot without mutating coordinator
   * state. Pass a precomputed `live` index (e.g. from `renderSidebar`) to avoid re-indexing.
   */
  peekObservationKey(snap, live = null) {
    return this._buildObservation(snap ?? {}, live).key;
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
    force = false,
    contextSynced = false,
    now = Date.now(),
  }) {
    if (Array.isArray(machineList)) {
      const prevLayouts = new Map(this.machineLayouts);
      this._hasMachineSnapshot = true;
      this.machineLayouts.clear();
      for (const m of machineList) {
        if (m && typeof m.name === "string" && m.name.length > 0) {
          const layout = m.layout ?? "ready";
          this.machineLayouts.set(m.name, layout);
          const prev = prevLayouts.get(m.name);
          if (prev !== undefined && prev !== layout) {
            this._bumpMachineEpoch(m.name);
          }
        }
      }
      for (const name of prevLayouts.keys()) {
        if (!this.machineLayouts.has(name)) {
          this._bumpMachineEpoch(name);
        }
      }
    }

    if (force) {
      this.staleSessionKeys.clear();
    }

    const { live, observation, key } = this._buildObservation({
      activeSpace,
      activeTabId,
      visibleTerminals,
      focusedTerminal,
      terminalList,
      panes,
      agents,
      machineList,
    });
    if (!contextSynced) {
      this.syncContext(live);
    }

    if (this.activeMachine && this.isMachineReady(this.activeMachine)) {
      void this.ensureHomeDir(this.activeMachine);
    }

    const running = live.runningByTerm;
    this._lastRunning = running;
    this._recordLiveTokens(live);

    // Passively learn hostId -> machine from live running terminals and stream labels.
    this._learnHostsFromTerminals(terminalList, running);

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
      live,
      terminalList,
      panes,
      agents,
      key,
      force: force || stale,
      now,
    });
  }

  /**
   * Coordinate the scoped fetchers (`_syncActiveTarget`, `_syncOffTabMarks`, `_fetchHostStreams`,
   * `_syncPreviews`) for the current observation, then publish `terminal.marks` and wake views.
   */
  async refresh({
    observation,
    live,
    terminalList = [],
    panes = [],
    agents = [],
    key,
    force = false,
    now = Date.now(),
  }) {
    const activeMachine = observation.activeSpace?.machine ?? null;
    const activeScope = observation.hostScope ?? this.hostScope;
    const running = live.runningByTerm;
    const gen = ++this.readGeneration;
    this.inFlightKey = key ?? null;
    this._lastRunning = running;
    this._recordLiveTokens(live);
    if (force) {
      this._globalEpoch += 1;
    }

    const promise = (async () => {
      try {
        await this.ensureConfig();
        if (gen !== this.readGeneration) return null;

        // If the active remote machine is offline or still loading, record list error and keep
        // healthy machines' off-tab marks up to date.
        if (!this.isMachineReady(activeMachine)) {
          this._getListEntry(activeMachine, activeScope).error = `${activeMachine} is unavailable`;
          const ok = await this._syncOffTabMarks({
            live,
            skipMachines: new Set([activeMachine]),
            gen,
          });
          if (!ok || gen !== this.readGeneration) return null;
          this.readError = null;
          this.lastFailedKey = null;
          this.lastCompletedAt = now;
          this.lastCompletedKey = key ?? null;
          this.publishSharedTerminals(running);
          return null;
        }

        const activeRes = await this._syncActiveTarget({ live, activeMachine, gen });
        if (activeRes.stale || gen !== this.readGeneration) return null;

        if (!activeRes.ok) {
          this.readError = activeRes.error;
          this.lastFailedKey = key ?? null;
          const ok = await this._syncOffTabMarks({
            live,
            skipTerm: activeRes.skipTerm ?? null,
            skipMachines: new Set([activeMachine]),
            gen,
          });
          if (!ok || gen !== this.readGeneration) return null;
          this.publishSharedTerminals(running);
          return null;
        }

        const { primaryTerm, primarySession, activeStream } = activeRes;

        const offTabOk = await this._syncOffTabMarks({
          live,
          skipTerm: primaryTerm,
          gen,
        });
        if (!offTabOk || gen !== this.readGeneration) return null;

        // Reconcile `streams.stream` hc labels on visible enrolled terminals in one pass.
        for (const term of live.visibleTerminals) {
          const sk = running.get(term);
          const sess = sk ? this.sessions.get(sk) : null;
          if (!sess?.stream) continue;
          const currentLabel = live.termByAddr.get(term)?.labels?.[LABEL_STREAM] ?? null;
          if (currentLabel !== sess.stream) {
            await this.runExec(this.hcBin, ["label", term, `${LABEL_STREAM}=${sess.stream}`]);
            if (gen !== this.readGeneration) return null;
          }
        }

        // After off-tab sessions are populated, ensure an unenrolled tab's Space fallback Stream
        // (even on cold boot without a `streams.stream` label) has fresh detail in `this.streams`.
        if (!primarySession && observation.activeSpace) {
          const liveNow = indexLive(
            { activeSpace: observation.activeSpace, panes, terminalList, agents },
            {
              sessions: this.sessions,
              staleSessionKeys: this.staleSessionKeys,
              lastFocusedStreamBySpace: this.lastFocusedStreamBySpace,
            }
          );
          const spaceStreamKey = liveNow.spaceStreamKey;
          const cachedSpaceStream = spaceStreamKey ? this.streams.get(spaceStreamKey) : null;
          if (
            spaceStreamKey &&
            !(
              cachedSpaceStream?.detail &&
              cachedSpaceStream.detailToken === this._streamToken(spaceStreamKey, activeMachine)
            )
          ) {
            const res = await this._fetchStream(spaceStreamKey, activeMachine, { gen });
            if (res.stale || gen !== this.readGeneration) return null;
          }
        }

        // Refresh the active `(activeMachine, activeScope)` Streams bucket when stale and visible,
        // or when an off-tab session on the active scope references an unknown Stream title.
        const activeBucket = this._getListEntry(activeMachine, activeScope);
        const listStale =
          !activeBucket.loaded ||
          activeBucket.fetchedToken !== this._listToken(activeMachine, activeScope);
        let hasUnknownOffTabStream = false;
        for (const [term, sk] of running) {
          const termMachine = parseTerminalAddress(term)?.machine ?? null;
          if (activeScope !== "all" && termMachine !== activeMachine) continue;
          const sess = this.sessions.get(sk);
          if (sess?.stream && !this.streams.has(sess.stream)) {
            hasUnknownOffTabStream = true;
            break;
          }
        }

        const shouldFetchHostStreams =
          (!primarySession && listStale) ||
          (this.preferredView === "all" && listStale) ||
          hasUnknownOffTabStream ||
          (activeBucket.loaded && force);

        if (shouldFetchHostStreams) {
          const listRes = await this._fetchHostStreams({
            machine: activeMachine,
            scope: activeScope,
            force: hasUnknownOffTabStream || force,
            notify: false,
          });
          if (gen !== this.readGeneration || listRes.stale) return null;
          if (!listRes.ok && !primarySession) {
            this.lastFailedKey = key ?? null;
            this.publishSharedTerminals(running);
            return null;
          }
        }

        await this._syncPreviews({
          activeStreamKey: activeStream?.key ?? null,
          gen,
        });
        if (gen !== this.readGeneration) return null;

        this.readError = null;
        this.lastFailedKey = null;
        this.lastCompletedAt = now;
        this.lastCompletedKey = key ?? null;
        this.publishSharedTerminals(running);
        return primarySession
          ? { kind: "stream", stream: activeStream, session: primarySession }
          : { kind: "listing", streams: this.getStreamList(activeMachine, activeScope).items };
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
          this.notifyViews("auto");
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
    const addedKeys = [];
    if (isSelected) {
      const freshIds = sessions.session_ids.filter((id) => !this.pending.has(`resume:${id}`));
      if (freshIds.length === 0) return { ok: false, duplicate: true };
      for (const id of freshIds) {
        const pKey = `resume:${id}`;
        this.pending.set(pKey, true);
        addedKeys.push(pKey);
      }
      effectiveSessions = { ...sessions, session_ids: freshIds };
    } else if (isBulkOpen) {
      const pKey = `stream:${stream}`;
      if (this.pending.has(pKey)) return { ok: false, duplicate: true };
      this.pending.set(pKey, "Opening…");
      addedKeys.push(pKey);
    }
    this.notifyViews();
    try {
      return await this._openWorkInner(
        { stream, sessions: effectiveSessions, terminalList, panes, agents },
        { run, focusTerminal }
      );
    } finally {
      for (const pKey of addedKeys) this.pending.delete(pKey);
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
